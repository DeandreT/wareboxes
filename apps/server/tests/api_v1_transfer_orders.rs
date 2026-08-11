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
    CancelTransferOrderResponse, CreateTransferOrderResponse, DispatchTransferOrderResponse,
    ErrorReason, ErrorResponse, ReceiveTransferOrderResponse, ReleaseTransferOrderResponse,
    TransferExecutionReadinessResponse, TransferOrderDetailResponse, TransferOrderPage,
    TransferOrderStatus,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::TenantAccess;

struct Context {
    fixture: Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    source_id: i64,
    destination_id: i64,
    third_facility_id: i64,
    owner_id: i64,
    item_id: i64,
    second_item_id: i64,
    token: String,
    access: TenantAccess,
}

async fn fixture(email: &str) -> Context {
    let fixture = Fixture::new().await;
    let actor = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, actor.id).await;
    let source_id = fixture.facility(tenant_id, "Transfer Origin DC").await;
    let destination_id = fixture.facility(tenant_id, "Transfer Destination DC").await;
    let third_facility_id = fixture.facility(tenant_id, "Transfer Hidden DC").await;
    let owner_id = fixture.inventory_owner(tenant_id, "Transfer Client").await;
    for facility_id in [source_id, destination_id, third_facility_id] {
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
    }
    let item_id = fixture.item(tenant_id, "Transfer Beans", "case").await;
    let second_item_id = fixture.item(tenant_id, "Transfer Towels", "each").await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    for item_id in [item_id, second_item_id] {
        sqlx::query("INSERT INTO inventory_owner_items(tenant_id,created,inventory_owner_id,item_id) VALUES ($1,$2,$3,$4)")
            .bind(tenant_id.get()).bind(db::now_iso()).bind(owner_id).bind(item_id).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let token = auth::create_session(&fixture.db, actor.id).await.unwrap();
    let access = default_tenant_for_user(&fixture.db, actor.id)
        .await
        .unwrap();
    Context {
        fixture,
        tenant_id,
        actor_id: actor.id,
        source_id,
        destination_id,
        third_facility_id,
        owner_id,
        item_id,
        second_item_id,
        token,
        access,
    }
}

fn command(context: &Context, path: &str, key: &str, body: &Value) -> Request<Body> {
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

fn get(context: &Context, path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", context.token))
        .header(TENANT_ID_HEADER, context.tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

fn create_body(context: &Context, number: &str) -> Value {
    json!({
        "inventory_owner_id": context.owner_id,
        "source_facility_id": context.source_id,
        "destination_facility_id": context.destination_id,
        "number": number,
        "expected_departure_at": "2027-08-20T12:00:00Z",
        "expected_arrival_at": "2027-08-21T12:00:00Z",
        "lines": [
            {"item_id": context.item_id, "requested_quantity": 12},
            {"item_id": context.second_item_id, "requested_quantity": 8}
        ]
    })
}

async fn json_body<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    serde_json::from_slice(&to_bytes(response.into_body(), 512 * 1024).await.unwrap()).unwrap()
}

async fn create_order(context: &Context, number: &str, key: &str) -> CreateTransferOrderResponse {
    let response = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(command(
            context,
            "transfer-orders",
            key,
            &create_body(context, number),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

#[tokio::test]
async fn create_release_and_cancel_are_atomic_replay_safe_and_audited() {
    let context = fixture("transfer-lifecycle@test.local").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let body = create_body(&context, "TO-LIFECYCLE-100");
    let first = app.clone().oneshot(command(
        &context,
        "transfer-orders",
        "transfer-create-a",
        &body,
    ));
    let second = app.clone().oneshot(command(
        &context,
        "transfer-orders",
        "transfer-create-b",
        &body,
    ));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (winner_key, response) = if first.status() == StatusCode::OK {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        ("transfer-create-a", first)
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        ("transfer-create-b", second)
    };
    let created: CreateTransferOrderResponse = json_body(response).await;
    assert_eq!(created.status, TransferOrderStatus::Draft);
    assert_eq!(created.revision.get(), 1);
    assert_eq!(created.total_requested_quantity, 20);
    let replay = app
        .clone()
        .oneshot(command(&context, "transfer-orders", winner_key, &body))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<CreateTransferOrderResponse>(replay).await,
        created
    );
    let mut changed = body.clone();
    changed["lines"][0]["requested_quantity"] = json!(13);
    let conflict = app
        .clone()
        .oneshot(command(&context, "transfer-orders", winner_key, &changed))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(conflict).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let release_path = format!("transfer-orders/{}/releases", created.transfer_order_id);
    let release = app
        .clone()
        .oneshot(command(
            &context,
            &release_path,
            "transfer-release",
            &json!({"expected_revision":1}),
        ))
        .await
        .unwrap();
    assert_eq!(release.status(), StatusCode::OK);
    let release: ReleaseTransferOrderResponse = json_body(release).await;
    assert_eq!(release.status, TransferOrderStatus::Released);
    assert_eq!(release.revision.get(), 2);
    let cancel_path = format!(
        "transfer-orders/{}/cancellations",
        created.transfer_order_id
    );
    let cancel_body =
        json!({"expected_revision":2,"reason":"route_cancelled","note":"Linehaul lane closed"});
    let cancel = app
        .clone()
        .oneshot(command(
            &context,
            &cancel_path,
            "transfer-cancel",
            &cancel_body,
        ))
        .await
        .unwrap();
    assert_eq!(cancel.status(), StatusCode::OK);
    let cancelled: CancelTransferOrderResponse = json_body(cancel).await;
    assert_eq!(cancelled.previous_status, TransferOrderStatus::Released);
    assert_eq!(cancelled.status, TransferOrderStatus::Cancelled);
    assert_eq!(cancelled.revision.get(), 3);
    let cancel_replay = app
        .clone()
        .oneshot(command(
            &context,
            &cancel_path,
            "transfer-cancel",
            &cancel_body,
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body::<CancelTransferOrderResponse>(cancel_replay).await,
        cancelled
    );

    let detail = app
        .oneshot(get(
            &context,
            &format!("transfer-orders/{}", created.transfer_order_id),
        ))
        .await
        .unwrap();
    let detail: TransferOrderDetailResponse = json_body(detail).await;
    assert_eq!(detail.lines.len(), 2);
    assert_eq!(detail.summary.status, TransferOrderStatus::Cancelled);
    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects:(i64,i64,i64,i64,i64)=sqlx::query_as(r#"SELECT
        (SELECT COUNT(*) FROM transfer_orders WHERE id=$1),
        (SELECT COUNT(*) FROM transfer_order_lines WHERE transfer_order_id=$1),
        (SELECT COUNT(*) FROM transfer_order_releases WHERE transfer_order_id=$1),
        (SELECT COUNT(*) FROM transfer_order_cancellations WHERE transfer_order_id=$1),
        (SELECT COUNT(*) FROM outbox_events WHERE aggregate_type='transfer_order' AND aggregate_id=$1::TEXT)"#)
        .bind(created.transfer_order_id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(effects, (1, 2, 1, 1, 3));
    assert!(sqlx::query(
        "UPDATE transfer_order_cancellations SET note='forged' WHERE transfer_order_id=$1"
    )
    .bind(created.transfer_order_id)
    .execute(&mut *tx)
    .await
    .is_err());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn dispatch_and_receipt_conserve_stock_across_facilities() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
    let context = fixture("transfer-execution@test.local").await;
    let first_batch = context
        .fixture
        .received_balance(
            &context.access,
            ReceivedBalanceSetup {
                key: "TRANSFER-BEANS-A",
                inventory_owner_id: context.owner_id,
                facility_id: context.source_id,
                item_id: context.item_id,
                qty: 12,
            },
        )
        .await;
    let towels = context
        .fixture
        .received_balance(
            &context.access,
            ReceivedBalanceSetup {
                key: "TRANSFER-TOWELS",
                inventory_owner_id: context.owner_id,
                facility_id: context.source_id,
                item_id: context.second_item_id,
                qty: 8,
            },
        )
        .await;
    let transit_barcode = "TRANSFER-IN-TRANSIT-01";
    let transit_location_id = wareboxes_persistence_postgres::locations::add_location(
        &context.fixture.db,
        context.tenant_id,
        context.source_id,
        None,
        Some(transit_barcode),
        Some("Transfer dispatch lane"),
        "transfer_in_transit",
        true,
        false,
        false,
    )
    .await
    .unwrap();
    let receiving_barcode = "TRANSFER-DEST-RECV-01";
    let receiving_location_id = wareboxes_persistence_postgres::locations::add_location(
        &context.fixture.db,
        context.tenant_id,
        context.destination_id,
        None,
        Some(receiving_barcode),
        Some("Transfer receiving lane"),
        "receiving",
        true,
        false,
        true,
    )
    .await
    .unwrap();

    let created = create_order(&context, "TO-EXECUTION-100", "transfer-execution-create").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let release = app
        .clone()
        .oneshot(command(
            &context,
            &format!("transfer-orders/{}/releases", created.transfer_order_id),
            "transfer-execution-release",
            &json!({"expected_revision": 1}),
        ))
        .await
        .unwrap();
    assert_eq!(release.status(), StatusCode::OK);

    let readiness = app
        .clone()
        .oneshot(get(
            &context,
            &format!(
                "transfer-orders/{}/execution-readiness",
                created.transfer_order_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(readiness.status(), StatusCode::OK);
    let readiness: TransferExecutionReadinessResponse = json_body(readiness).await;
    assert_eq!(readiness.status, TransferOrderStatus::Released);
    assert_eq!(readiness.dispatch_candidates.len(), 2);
    assert!(readiness
        .transit_locations
        .iter()
        .any(|location| location.location_id == transit_location_id));
    assert!(readiness
        .receiving_locations
        .iter()
        .any(|location| location.location_id == receiving_location_id));

    let beans_line = created
        .lines
        .iter()
        .find(|line| line.item_id == context.item_id)
        .unwrap()
        .line_id;
    let towels_line = created
        .lines
        .iter()
        .find(|line| line.item_id == context.second_item_id)
        .unwrap()
        .line_id;
    let dispatch_body = json!({
        "expected_revision": 2,
        "transit_location_id": transit_location_id,
        "transit_location_barcode": transit_barcode,
        "lines": [
            {"transfer_order_line_id": beans_line,
             "source_inventory_balance_id": first_batch.balance_id,
             "quantity": 12,"source_location_barcode": "TRANSFER-BEANS-A"},
            {"transfer_order_line_id": towels_line,
             "source_inventory_balance_id": towels.balance_id,
             "quantity": 8,"source_location_barcode": "TRANSFER-TOWELS"}
        ]
    });
    let dispatch_path = format!("transfer-orders/{}/dispatches", created.transfer_order_id);
    let mut wrong_dispatch_body = dispatch_body.clone();
    wrong_dispatch_body["transit_location_barcode"] = json!("WRONG-TRANSFER-LANE");
    let wrong_dispatch = app
        .clone()
        .oneshot(command(
            &context,
            &dispatch_path,
            "transfer-execution-wrong-dispatch-scan",
            &wrong_dispatch_body,
        ))
        .await
        .unwrap();
    assert_eq!(wrong_dispatch.status(), StatusCode::BAD_REQUEST);
    let mut effects_tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects_before_dispatch: (String, i64, i64, i64) = sqlx::query_as(
        r#"SELECT transfer.status,transfer.revision,
                  (SELECT COUNT(*) FROM transfer_order_dispatches WHERE transfer_order_id=transfer.id),
                  (SELECT COUNT(*) FROM inventory_transactions
                   WHERE reference_type='transfer_order' AND reference_id=transfer.id)
           FROM transfer_orders transfer WHERE transfer.id=$1"#,
    )
    .bind(created.transfer_order_id)
    .fetch_one(&mut *effects_tx)
    .await
    .unwrap();
    assert_eq!(effects_before_dispatch, ("released".into(), 2, 0, 0));
    effects_tx.commit().await.unwrap();
    let dispatch = app
        .clone()
        .oneshot(command(
            &context,
            &dispatch_path,
            "transfer-execution-dispatch",
            &dispatch_body,
        ))
        .await
        .unwrap();
    if dispatch.status() != StatusCode::OK {
        let status = dispatch.status();
        let body = to_bytes(dispatch.into_body(), 512 * 1024).await.unwrap();
        panic!(
            "dispatch failed with {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let dispatched: DispatchTransferOrderResponse = json_body(dispatch).await;
    assert_eq!(dispatched.status, TransferOrderStatus::InTransit);
    assert_eq!(dispatched.revision.get(), 3);
    assert_eq!(dispatched.lines.len(), 2);
    assert_eq!(dispatched.total_dispatched_quantity, 20);
    let replay = app
        .clone()
        .oneshot(command(
            &context,
            &dispatch_path,
            "transfer-execution-dispatch",
            &dispatch_body,
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body::<DispatchTransferOrderResponse>(replay).await,
        dispatched
    );

    let receipt_body = json!({
        "expected_revision": 3,
        "destination_location_id": receiving_location_id,
        "destination_location_barcode": receiving_barcode
    });
    let receipt_path = format!("transfer-orders/{}/receipts", created.transfer_order_id);
    let mut wrong_receipt_body = receipt_body.clone();
    wrong_receipt_body["destination_location_barcode"] = json!("WRONG-RECEIVING-LANE");
    let wrong_receipt = app
        .clone()
        .oneshot(command(
            &context,
            &receipt_path,
            "transfer-execution-wrong-receipt-scan",
            &wrong_receipt_body,
        ))
        .await
        .unwrap();
    assert_eq!(wrong_receipt.status(), StatusCode::BAD_REQUEST);
    let mut effects_tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects_before_receipt: (String, i64, i64, i64) = sqlx::query_as(
        r#"SELECT transfer.status,transfer.revision,
                  (SELECT COUNT(*) FROM transfer_order_receipts WHERE transfer_order_id=transfer.id),
                  (SELECT COUNT(*) FROM inventory_transactions
                   WHERE reference_type='transfer_order' AND reference_id=transfer.id)
           FROM transfer_orders transfer WHERE transfer.id=$1"#,
    )
    .bind(created.transfer_order_id)
    .fetch_one(&mut *effects_tx)
    .await
    .unwrap();
    assert_eq!(effects_before_receipt, ("in_transit".into(), 3, 0, 1));
    effects_tx.commit().await.unwrap();
    let receipt = app
        .clone()
        .oneshot(command(
            &context,
            &receipt_path,
            "transfer-execution-receipt",
            &receipt_body,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status(), StatusCode::OK);
    let received: ReceiveTransferOrderResponse = json_body(receipt).await;
    assert_eq!(received.status, TransferOrderStatus::Received);
    assert_eq!(received.revision.get(), 4);
    assert_eq!(received.lines.len(), 2);
    assert_eq!(received.total_received_quantity, 20);

    let detail = app
        .oneshot(get(
            &context,
            &format!("transfer-orders/{}", created.transfer_order_id),
        ))
        .await
        .unwrap();
    let detail: TransferOrderDetailResponse = json_body(detail).await;
    assert_eq!(detail.summary.status, TransferOrderStatus::Received);
    assert_eq!(detail.summary.dispatch_id, Some(dispatched.dispatch_id));
    assert_eq!(detail.summary.receipt_id, Some(received.receipt_id));
    assert!(detail
        .lines
        .iter()
        .all(|line| line.requested_quantity == line.dispatched_quantity
            && line.requested_quantity == line.received_quantity));

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let transactions: Vec<(String, String, i64, i64)> = sqlx::query_as(
        r#"SELECT transaction.transaction_type,transaction.operation,
                  COUNT(entry.id),COALESCE(SUM(entry.quantity_delta),0)::BIGINT
           FROM inventory_transactions transaction
           JOIN inventory_entries entry ON entry.tenant_id=transaction.tenant_id
             AND entry.transaction_id=transaction.id
           WHERE transaction.id=ANY($1)
           GROUP BY transaction.id,transaction.transaction_type,transaction.operation
           ORDER BY transaction.id"#,
    )
    .bind(vec![
        dispatched.inventory_transaction_id,
        received.inventory_transaction_id,
    ])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        transactions,
        vec![
            (
                "move".into(),
                "inventory.transfer_order.dispatch.v1".into(),
                4,
                0
            ),
            (
                "transfer".into(),
                "inventory.transfer_order.receive.v1".into(),
                4,
                0
            ),
        ]
    );
    let source_on_hand: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(qty_on_hand),0)::BIGINT FROM inventory_balances WHERE id=ANY($1)",
    )
    .bind(vec![first_batch.balance_id, towels.balance_id])
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(source_on_hand, 0);
    let transit_on_hand: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(qty_on_hand),0)::BIGINT FROM inventory_balances WHERE location_id=$1",
    )
    .bind(transit_location_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(transit_on_hand, 0);
    let destination_on_hand: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(qty_on_hand),0)::BIGINT FROM inventory_balances WHERE location_id=$1",
    )
    .bind(receiving_location_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(destination_on_hand, 20);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn dual_facility_scope_and_cursor_filters_fail_closed() {
    let context = fixture("transfer-scope@test.local").await;
    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![context.source_id, context.destination_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![context.owner_id]
        }
    )
    .await
    .unwrap());
    let first = create_order(&context, "TO-PAGE-100", "transfer-page-100").await;
    let _second = create_order(&context, "TO-PAGE-101", "transfer-page-101").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let page = app
        .clone()
        .oneshot(get(&context, "transfer-orders?status=draft&limit=1"))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page: TransferOrderPage = json_body(page).await;
    assert_eq!(page.items.len(), 1);
    let cursor = page.next_cursor.unwrap();
    let next = app
        .clone()
        .oneshot(get(
            &context,
            &format!("transfer-orders?status=draft&limit=1&cursor={cursor}"),
        ))
        .await
        .unwrap();
    assert_eq!(json_body::<TransferOrderPage>(next).await.items.len(), 1);
    let mismatch = app
        .clone()
        .oneshot(get(
            &context,
            &format!("transfer-orders?status=released&limit=1&cursor={cursor}"),
        ))
        .await
        .unwrap();
    assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body::<ErrorResponse>(mismatch).await.reason,
        ErrorReason::InvalidCursor
    );

    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![context.source_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![context.owner_id]
        }
    )
    .await
    .unwrap());
    let hidden = app
        .clone()
        .oneshot(get(
            &context,
            &format!("transfer-orders/{}", first.transfer_order_id),
        ))
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let replay = app
        .oneshot(command(
            &context,
            "transfer-orders",
            "transfer-page-100",
            &create_body(&context, "TO-PAGE-100"),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malformed_routes_and_schema_bypass_have_zero_effects() {
    let context = fixture("transfer-validation@test.local").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    for (key, mut body) in [
        ("same-facility", create_body(&context, "TO-BAD-SAME")),
        ("bad-schedule", create_body(&context, "TO-BAD-DATE")),
        ("duplicate-item", create_body(&context, "TO-BAD-LINES")),
    ] {
        match key {
            "same-facility" => body["destination_facility_id"] = json!(context.source_id),
            "bad-schedule" => body["expected_arrival_at"] = json!("2027-08-19T12:00:00Z"),
            _ => body["lines"][1]["item_id"] = json!(context.item_id),
        }
        let response = app
            .clone()
            .oneshot(command(&context, "transfer-orders", key, &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transfer_orders")
        .fetch_one(&context.fixture.db)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let created = create_order(&context, "TO-GUARD-100", "transfer-guard").await;
    let admin = admin_db_for(&context.fixture.db).await;
    let forged=sqlx::query("UPDATE transfer_orders SET status='released',revision=2,released_by_user_id=$1,released_at=$2 WHERE id=$3")
        .bind(context.actor_id).bind(db::now_iso()).bind(created.transfer_order_id).execute(&admin).await.unwrap_err();
    assert!(!forged.to_string().is_empty());
    admin.close().await;
    let app_db = app_db_for(&context.fixture.db).await;
    let grants: (bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"SELECT
        has_table_privilege('wareboxes_app','transfer_order_releases','SELECT'),
        has_table_privilege('wareboxes_app','transfer_order_releases','UPDATE'),
        has_table_privilege('wareboxes_app','transfer_order_dispatches','INSERT'),
        has_table_privilege('wareboxes_app','transfer_order_dispatches','UPDATE'),
        has_table_privilege('wareboxes_app','transfer_order_receipts','INSERT'),
        has_table_privilege('wareboxes_app','transfer_order_receipts','DELETE')"#,
    )
    .fetch_one(&app_db)
    .await
    .unwrap();
    assert_eq!(grants, (true, false, true, false, true, false));
    app_db.close().await;
    assert_ne!(context.third_facility_id, context.destination_id);
}
