mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, OrderHoldOrderStatus, OrderHoldReason, PlaceOrderHoldRequest,
    PlaceOrderHoldResponse, ReleaseOrderHoldRequest, ReleaseOrderHoldResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::OrderStatus;

fn api_request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    path: &str,
    idempotency_key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(idempotency_key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    request
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        db,
        tenant_id,
        "orders",
        Some("Orders"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        "order-hold-operator",
        Some("Place and release order holds"),
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(db, tenant_id, role, permission)
        .await
        .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

#[tokio::test]
async fn order_holds_are_scoped_replay_safe_and_release_only_after_the_last_hold() {
    let fixture = Fixture::new().await;
    let user = fixture.user("order-hold-operator@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Order Hold Client")
        .await;
    let order_id = fixture
        .order_header(tenant_id, "ORDER-HOLD-001", owner_id)
        .await;
    let concurrent_order_id = fixture
        .order_header(tenant_id, "ORDER-HOLD-CONCURRENT", owner_id)
        .await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first_hold = PlaceOrderHoldRequest {
        reason: OrderHoldReason::CustomerRequest,
        note: Some("Client requested an address check".into()),
    };
    let first_path = format!("/api/v1/orders/{order_id}/holds");

    let missing_key = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &first_path,
            None,
            &first_hold,
        ))
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.reason,
        ErrorReason::IdempotencyKeyRequired
    );

    let unexplained_other = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &first_path,
            Some("order-hold-other-without-note"),
            &PlaceOrderHoldRequest {
                reason: OrderHoldReason::Other,
                note: None,
            },
        ))
        .await
        .unwrap();
    assert_eq!(unexplained_other.status(), StatusCode::BAD_REQUEST);

    let placed = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &first_path,
            Some("order-hold-place-1"),
            &first_hold,
        ))
        .await
        .unwrap();
    assert_eq!(placed.status(), StatusCode::OK);
    let placed: PlaceOrderHoldResponse = response_json(placed).await;
    assert_eq!(placed.order_status, OrderHoldOrderStatus::Held);
    assert_eq!(placed.active_hold_count, 1);

    let replay = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &first_path,
            Some("order-hold-place-1"),
            &first_hold,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PlaceOrderHoldResponse>(replay).await,
        placed
    );

    let changed_replay = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &first_path,
            Some("order-hold-place-1"),
            &PlaceOrderHoldRequest {
                reason: OrderHoldReason::InventoryShortage,
                note: None,
            },
        ))
        .await
        .unwrap();
    assert_eq!(changed_replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_replay).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let duplicate_reason = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &first_path,
            Some("order-hold-duplicate-reason"),
            &first_hold,
        ))
        .await
        .unwrap();
    assert_eq!(duplicate_reason.status(), StatusCode::CONFLICT);

    let second = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &first_path,
            Some("order-hold-place-2"),
            &PlaceOrderHoldRequest {
                reason: OrderHoldReason::ComplianceReview,
                note: Some("Export classification review".into()),
            },
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second: PlaceOrderHoldResponse = response_json(second).await;
    assert_eq!(second.active_hold_count, 2);

    let release_first_path = format!(
        "/api/v1/orders/{order_id}/holds/{}/releases",
        placed.hold_id
    );
    let release_first = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &release_first_path,
            Some("order-hold-release-1"),
            &ReleaseOrderHoldRequest {
                note: Some("Client approved the address".into()),
            },
        ))
        .await
        .unwrap();
    assert_eq!(release_first.status(), StatusCode::OK);
    let release_first: ReleaseOrderHoldResponse = response_json(release_first).await;
    assert_eq!(release_first.order_status, OrderHoldOrderStatus::Held);
    assert_eq!(release_first.active_hold_count, 1);

    let release_second_path = format!(
        "/api/v1/orders/{order_id}/holds/{}/releases",
        second.hold_id
    );
    let release_second = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &release_second_path,
            Some("order-hold-release-2"),
            &ReleaseOrderHoldRequest::default(),
        ))
        .await
        .unwrap();
    assert_eq!(release_second.status(), StatusCode::OK);
    let release_second: ReleaseOrderHoldResponse = response_json(release_second).await;
    assert_eq!(release_second.order_status, OrderHoldOrderStatus::Open);
    assert_eq!(release_second.active_hold_count, 0);

    let concurrent_path = format!("/api/v1/orders/{concurrent_order_id}/holds");
    let concurrent_request = PlaceOrderHoldRequest {
        reason: OrderHoldReason::InventoryShortage,
        note: None,
    };
    let first = app.clone().oneshot(api_request(
        &token,
        tenant_id,
        &concurrent_path,
        Some("order-hold-concurrent"),
        &concurrent_request,
    ));
    let retry = app.clone().oneshot(api_request(
        &token,
        tenant_id,
        &concurrent_path,
        Some("order-hold-concurrent"),
        &concurrent_request,
    ));
    let (first, retry) = tokio::join!(first, retry);
    let first = first.unwrap();
    let retry = retry.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PlaceOrderHoldResponse>(first).await,
        response_json::<PlaceOrderHoldResponse>(retry).await
    );

    let order = repo::orders::get_order(&fixture.db, tenant_id, order_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(order.status, OrderStatus::Open);
    assert_eq!(order.holds.len(), 2);
    assert!(order.holds.iter().all(|hold| !hold.is_active()));

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let durable_effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_holds
                WHERE tenant_id = $1 AND order_id IN ($2, $3)),
               (SELECT COUNT(*) FROM order_activity
                WHERE tenant_id = $1 AND order_id = $2
                  AND action IN ('placed order hold', 'released order hold')),
               (SELECT COUNT(*) FROM outbox_events
                WHERE tenant_id = $1 AND aggregate_type = 'order'
                  AND aggregate_id IN ($2::TEXT, $3::TEXT)),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = $1
                  AND operation IN ('order.place_hold.v1', 'order.release_hold.v1'))
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(concurrent_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let sequences: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT aggregate_sequence
        FROM outbox_events
        WHERE tenant_id = $1 AND ordering_key = $2
        ORDER BY aggregate_sequence
        "#,
    )
    .bind(tenant_id.get())
    .bind(format!("order:{order_id}"))
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(durable_effects, (3, 4, 5, 5));
    assert_eq!(sequences, vec![1, 2, 3, 4]);

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let revoked_replay = app
        .oneshot(api_request(
            &token,
            tenant_id,
            &first_path,
            Some("order-hold-place-1"),
            &first_hold,
        ))
        .await
        .unwrap();
    assert_eq!(revoked_replay.status(), StatusCode::NOT_FOUND);
}
