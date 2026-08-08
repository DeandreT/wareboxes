mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::web::{ErrorCode, ErrorResponse};
use wareboxes_core::dto::UpdateUserAccessScope;

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct OrderMetadataState {
    rush: bool,
    status: String,
    address_count: i64,
    activity_count: i64,
    command_count: i64,
    line1: String,
    city: Option<String>,
    state: Option<String>,
    postal_code: Option<String>,
    country: String,
}

fn update_request(
    token: &str,
    tenant_id: TenantId,
    idempotency_key: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/orders/update")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(idempotency_key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn send_update(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    idempotency_key: Option<&str>,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(update_request(token, tenant_id, idempotency_key, body))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64, suffix: &str) -> i64 {
    let permission =
        match wareboxes_persistence_postgres::permissions::find_by_name(db, tenant_id, "orders")
            .await
            .unwrap()
        {
            Some(permission) => permission.id,
            None => wareboxes_persistence_postgres::permissions::add_permission(
                db,
                tenant_id,
                "orders",
                Some("Orders"),
            )
            .await
            .unwrap(),
        };
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        &format!("orders-{suffix}"),
        None,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(db, tenant_id, role, permission)
        .await
        .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
    role
}

#[tokio::test]
async fn order_metadata_updates_are_replay_safe_and_scope_checked() {
    let fixture = Fixture::new().await;
    let worker = fixture.wms_user("order-update-idempotency@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, worker.id).await;
    grant_orders(&fixture.db, tenant_id, worker.id, "metadata").await;
    let token = auth::create_session(&fixture.db, worker.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let owner = fixture
        .inventory_owner(tenant_id, "Order Metadata Owner")
        .await;
    let order = fixture
        .order_header(tenant_id, "ORDER-METADATA-1", owner)
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let address_count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM addresses")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let body = json!({
        "order_id": order,
        "rush": true,
        "line1": "200 Replay Street"
    });

    let missing_key = send_update(&app, &token, tenant_id, None, body.clone()).await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.code,
        ErrorCode::IdempotencyKeyRequired
    );

    let empty_update = send_update(
        &app,
        &token,
        tenant_id,
        Some("empty-order-metadata"),
        json!({"order_id": order}),
    )
    .await;
    assert_eq!(empty_update.status(), StatusCode::BAD_REQUEST);

    for (idempotency_key, order_key) in [
        ("blank-order-key", " ".to_owned()),
        ("long-order-key", "X".repeat(201)),
    ] {
        let invalid_key = send_update(
            &app,
            &token,
            tenant_id,
            Some(idempotency_key),
            json!({"order_id": order, "order_key": order_key}),
        )
        .await;
        assert_eq!(invalid_key.status(), StatusCode::BAD_REQUEST);
    }

    let workflow_patch = send_update(
        &app,
        &token,
        tenant_id,
        Some("workflow-patch"),
        json!({"order_id": order, "status": "shipped"}),
    )
    .await;
    assert_eq!(workflow_patch.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let first = send_update(
        &app,
        &token,
        tenant_id,
        Some("order-metadata-1"),
        body.clone(),
    );
    let replay = send_update(
        &app,
        &token,
        tenant_id,
        Some("order-metadata-1"),
        body.clone(),
    );
    let (first, replay) = tokio::join!(first, replay);
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(replay.status(), StatusCode::OK);
    assert!(response_json::<bool>(first).await);
    assert!(response_json::<bool>(replay).await);

    let changed_payload = send_update(
        &app,
        &token,
        tenant_id,
        Some("order-metadata-1"),
        json!({"order_id": order, "rush": false}),
    )
    .await;
    assert_eq!(changed_payload.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_payload).await.code,
        ErrorCode::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let state: OrderMetadataState = sqlx::query_as(
        r#"
        SELECT orders.rush,
               orders.status,
               (SELECT COUNT(*) FROM addresses
                WHERE tenant_id = orders.tenant_id) AS address_count,
               (SELECT COUNT(*) FROM order_activity
                WHERE tenant_id = orders.tenant_id
                  AND order_id = orders.id
                  AND action = 'updated order metadata') AS activity_count,
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = orders.tenant_id
                  AND operation = 'order.update_metadata.v1') AS command_count,
               address.line1,
               address.city,
               address.state,
               address.postal_code,
               address.country
        FROM orders
        INNER JOIN addresses address
            ON address.tenant_id = orders.tenant_id
           AND address.id = orders.address_id
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(order)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        state,
        OrderMetadataState {
            rush: true,
            status: "open".to_owned(),
            address_count: address_count_before + 1,
            activity_count: 1,
            command_count: 1,
            line1: "200 Replay Street".to_owned(),
            city: Some("Reno".to_owned()),
            state: Some("NV".to_owned()),
            postal_code: Some("89501".to_owned()),
            country: "US".to_owned(),
        }
    );

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: worker.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let revoked_replay = send_update(&app, &token, tenant_id, Some("order-metadata-1"), body).await;
    assert_eq!(revoked_replay.status(), StatusCode::NOT_FOUND);
}
