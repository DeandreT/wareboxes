mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CreatePurchaseOrderResponse, ErrorReason, ErrorResponse, PurchaseOrderDetailResponse,
    PurchaseOrderPage, PurchaseOrderStatus, ReleasePurchaseOrderResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct PurchaseOrderFixture {
    fixture: Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    owner_id: i64,
    item_id: i64,
    second_item_id: i64,
    token: String,
}

async fn fixture(email: &str) -> PurchaseOrderFixture {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture
        .facility(tenant_id, "Purchase Order Distribution Center")
        .await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Purchase Order Client")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(tenant_id, "Purchase Order Beans", "case")
        .await;
    let second_item_id = fixture
        .item(tenant_id, "Purchase Order Towels", "each")
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    for item_id in [item_id, second_item_id] {
        sqlx::query(
            "INSERT INTO inventory_owner_items(tenant_id,created,inventory_owner_id,item_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(tenant_id.get())
        .bind(db::now_iso())
        .bind(owner_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    PurchaseOrderFixture {
        fixture,
        tenant_id,
        actor_id: operator.id,
        facility_id,
        owner_id,
        item_id,
        second_item_id,
        token,
    }
}

fn command_request(
    context: &PurchaseOrderFixture,
    path: &str,
    key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", context.token))
        .header(TENANT_ID_HEADER, context.tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get_request(context: &PurchaseOrderFixture, path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", context.token))
        .header(TENANT_ID_HEADER, context.tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

fn create_body(context: &PurchaseOrderFixture, number: &str, first_qty: i64) -> Value {
    json!({
        "inventory_owner_id": context.owner_id,
        "facility_id": context.facility_id,
        "number": number,
        "supplier": "Northstar Foods",
        "expected_by": "2027-08-20T17:00:00Z",
        "lines": [
            {"item_id": context.item_id, "ordered_quantity": first_qty},
            {"item_id": context.second_item_id, "ordered_quantity": 8}
        ]
    })
}

async fn json_body<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn create_order(context: &PurchaseOrderFixture, number: &str) -> CreatePurchaseOrderResponse {
    let response = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(command_request(
            context,
            "purchase-orders",
            &format!("create-{number}"),
            &create_body(context, number, 12),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

#[tokio::test]
async fn creation_is_atomic_race_safe_replayable_and_immutable() {
    let context = fixture("purchase-order-create@test.local").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let body = create_body(&context, "PO-RACE-100", 12);
    let first = app.clone().oneshot(command_request(
        &context,
        "purchase-orders",
        "po-race-a",
        &body,
    ));
    let second = app.clone().oneshot(command_request(
        &context,
        "purchase-orders",
        "po-race-b",
        &body,
    ));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (winner_key, winner_response) = if first.status() == StatusCode::OK {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        ("po-race-a", first)
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        ("po-race-b", second)
    };
    let winner = json_body::<CreatePurchaseOrderResponse>(winner_response).await;
    assert_eq!(winner.status, PurchaseOrderStatus::Draft);
    assert_eq!(winner.revision.get(), 1);
    assert_eq!(winner.lines.len(), 2);
    assert_eq!(winner.total_ordered_quantity, 20);

    let replay = app
        .clone()
        .oneshot(command_request(
            &context,
            "purchase-orders",
            winner_key,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<CreatePurchaseOrderResponse>(replay).await,
        winner
    );
    let changed = app
        .oneshot(command_request(
            &context,
            "purchase-orders",
            winner_key,
            &create_body(&context, "PO-RACE-100", 13),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM purchase_orders WHERE number='PO-RACE-100'),
          (SELECT COUNT(*) FROM purchase_order_lines WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.purchase_order.created'
             AND aggregate_id=$1::TEXT),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.purchase_order.create.v1'
               AND (result_json->>'purchase_order_id')::BIGINT=$1)
        "#,
    )
    .bind(winner.purchase_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (1, 2, 1, 1));
    let immutable = sqlx::query(
        "UPDATE purchase_order_lines SET ordered_quantity=99 WHERE purchase_order_id=$1",
    )
    .bind(winner.purchase_order_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(!immutable.to_string().is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn release_is_revision_guarded_race_safe_and_audited() {
    let context = fixture("purchase-order-release@test.local").await;
    let created = create_order(&context, "PO-RELEASE-100").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let path = format!("purchase-orders/{}/releases", created.purchase_order_id);
    let body = json!({"expected_revision": 1});
    let first = app
        .clone()
        .oneshot(command_request(&context, &path, "po-release-a", &body));
    let second = app
        .clone()
        .oneshot(command_request(&context, &path, "po-release-b", &body));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (winner_key, winner_response) = if first.status() == StatusCode::OK {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        ("po-release-a", first)
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        ("po-release-b", second)
    };
    let winner = json_body::<ReleasePurchaseOrderResponse>(winner_response).await;
    assert_eq!(winner.previous_status, PurchaseOrderStatus::Draft);
    assert_eq!(winner.status, PurchaseOrderStatus::Released);
    assert_eq!(winner.revision.get(), 2);

    let replay = app
        .clone()
        .oneshot(command_request(&context, &path, winner_key, &body))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<ReleasePurchaseOrderResponse>(replay).await,
        winner
    );
    let stale = app
        .clone()
        .oneshot(command_request(&context, &path, "po-release-stale", &body))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let detail = app
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", created.purchase_order_id),
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = json_body::<PurchaseOrderDetailResponse>(detail).await;
    assert_eq!(detail.summary.status, PurchaseOrderStatus::Released);
    assert_eq!(detail.summary.revision.get(), 2);
    assert_eq!(detail.lines.len(), 2);

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let evidence: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM purchase_order_releases WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.purchase_order.released'
             AND aggregate_id=$1::TEXT AND aggregate_sequence=2),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.purchase_order.release.v1'
               AND (result_json->>'purchase_order_id')::BIGINT=$1),
          (SELECT COUNT(*) FROM purchase_orders
             WHERE id=$1 AND status='released' AND revision=2)
        "#,
    )
    .bind(created.purchase_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 1, 1, 1));
    let immutable = sqlx::query("DELETE FROM purchase_order_releases WHERE purchase_order_id=$1")
        .bind(created.purchase_order_id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(!immutable.to_string().is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn pages_replays_and_ledgers_are_scope_bound_with_minimal_grants() {
    let context = fixture("purchase-order-scope@test.local").await;
    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![context.facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![context.owner_id],
        },
    )
    .await
    .unwrap());
    let first = create_order(&context, "PO-PAGE-100").await;
    let _second = create_order(&context, "PO-PAGE-101").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let page = app
        .clone()
        .oneshot(get_request(
            &context,
            "purchase-orders?status=draft&limit=1",
        ))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page = json_body::<PurchaseOrderPage>(page).await;
    assert_eq!(page.items.len(), 1);
    let cursor = page.next_cursor.unwrap();
    let next = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("purchase-orders?status=draft&limit=1&cursor={cursor}"),
        ))
        .await
        .unwrap();
    assert_eq!(next.status(), StatusCode::OK);
    assert_eq!(json_body::<PurchaseOrderPage>(next).await.items.len(), 1);
    let mismatched = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("purchase-orders?status=released&limit=1&cursor={cursor}"),
        ))
        .await
        .unwrap();
    assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body::<ErrorResponse>(mismatched).await.reason,
        ErrorReason::InvalidCursor
    );

    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    for body in [
        create_body(&context, "PO-PAGE-100", 12),
        create_body(&context, "PO-PAGE-100", 13),
    ] {
        let hidden = app
            .clone()
            .oneshot(command_request(
                &context,
                "purchase-orders",
                "create-PO-PAGE-100",
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }
    let hidden_detail = app
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", first.purchase_order_id),
        ))
        .await
        .unwrap();
    assert_eq!(hidden_detail.status(), StatusCode::NOT_FOUND);

    let admin = admin_db_for(&context.fixture.db).await;
    for table in [
        "purchase_orders",
        "purchase_order_lines",
        "purchase_order_releases",
    ] {
        let checks: (bool, bool) = sqlx::query_as(
            "SELECT relforcerowsecurity,has_table_privilege('wareboxes_app',$1,'DELETE') FROM pg_class WHERE oid=$1::regclass",
        )
        .bind(format!("public.{table}"))
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(checks.0);
        assert!(!checks.1);
    }
    let privileges: (bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege('wareboxes_app','purchase_order_lines','UPDATE'),
               has_table_privilege('wareboxes_app','purchase_order_releases','UPDATE'),
               has_column_privilege('wareboxes_app','purchase_orders','number','UPDATE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(privileges, (false, false, false));
    admin.close().await;
}
