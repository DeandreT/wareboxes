mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::dto::{ErrorCode, ErrorResponse};
use wareboxes_core::models::ReceiveExpectedInventoryResult;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_server::{routes, state::AppState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
struct ReceiptEffects {
    transactions: i64,
    entries: i64,
    item_batches: i64,
    balances: i64,
    command_records: i64,
    outbox_events: i64,
    license_plates: i64,
    load_activity: i64,
    qty_on_hand: i64,
    line_received_qty: i64,
    line_rejected_qty: i64,
    line_missing_qty: i64,
}

fn receipt_request(
    token: &str,
    tenant_id: TenantId,
    load_line_id: i64,
    idempotency_key: &str,
    body: Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/inbound/load-lines/{load_line_id}/receipts"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn send_receipt(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    load_line_id: i64,
    idempotency_key: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(receipt_request(
            token,
            tenant_id,
            load_line_id,
            idempotency_key,
            body,
        ))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn receipt_effects(db: &db::Db, tenant_id: TenantId, load_line_id: i64) -> ReceiptEffects {
    let mut tx = tenant_tx(db, tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM inventory_transactions) AS transactions,
            (SELECT COUNT(*) FROM inventory_entries) AS entries,
            (SELECT COUNT(*) FROM item_batches) AS item_batches,
            (SELECT COUNT(*) FROM inventory_balances) AS balances,
            (SELECT COUNT(*) FROM command_idempotency_records)
                AS command_records,
            (SELECT COUNT(*) FROM outbox_events) AS outbox_events,
            (SELECT COUNT(*) FROM license_plates) AS license_plates,
            (SELECT COUNT(*) FROM load_activity) AS load_activity,
            (
                SELECT COALESCE(SUM(qty_on_hand), 0)::BIGINT
                FROM inventory_balances
                WHERE deleted IS NULL
            ) AS qty_on_hand,
            line.received_qty AS line_received_qty,
            line.rejected_qty AS line_rejected_qty,
            line.missing_qty AS line_missing_qty
        FROM load_lines line
        WHERE line.tenant_id = $1 AND line.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_line_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    effects
}

#[tokio::test]
async fn canonical_expected_receipt_contract_is_replay_safe_and_fail_closed() {
    let fixture = Fixture::new().await;
    let user = fixture
        .wms_user("canonical-expected-receipt@test.local")
        .await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let facility_id = fixture
        .facility(tenant_id, "Canonical Expected Receipt DC")
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Canonical Expected Receipt Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let receiving_location_id = repo::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("CANONICAL-RECEIVING"),
        Some("Canonical Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let item_id = fixture
        .item(tenant_id, "Canonical Expected Receipt Item", "case")
        .await;
    let load_id = repo::loads::add_load(
        &fixture.db,
        tenant_id,
        user.id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some("CANONICAL-EXPECTED-RECEIPT"),
        None,
        None,
        None,
        None,
        Some(receiving_location_id),
        None,
        None,
    )
    .await
    .unwrap();
    let replay_line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        10,
        Some("CANONICAL-LOT"),
        None,
        None,
    )
    .await
    .unwrap();
    let no_location_line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        2,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let exception_line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        4,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let rejection_only_line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        3,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let unknown_field_line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        2,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::loads::update_load(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        Some(LoadStatus::Arrived),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap());

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let receipt_body = json!({
        "receiving_location_id": receiving_location_id,
        "received_qty": 4,
        "rejected_qty": 0,
        "missing_qty": 0,
        "lot": "CANONICAL-LOT"
    });
    let first = send_receipt(
        &app,
        &token,
        tenant_id,
        replay_line_id,
        "canonical-expected-receipt-replay",
        receipt_body.clone(),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first: ReceiveExpectedInventoryResult = response_json(first).await;
    assert_eq!(
        first,
        ReceiveExpectedInventoryResult {
            load_id,
            load_line_id: replay_line_id,
            inventory_transaction_id: first.inventory_transaction_id,
            item_batch_id: first.item_batch_id,
            license_plate_id: None,
            load_status: LoadStatus::Receiving,
            line_status: LoadLineStatus::Partial,
            cumulative_received_qty: 4,
            cumulative_rejected_qty: 0,
            cumulative_missing_qty: 0,
            receive_completed: false,
        }
    );
    assert!(first.inventory_transaction_id.is_some());
    assert!(first.item_batch_id.is_some());

    let effects_after_first = receipt_effects(&fixture.db, tenant_id, replay_line_id).await;
    assert_eq!(effects_after_first.qty_on_hand, 4);
    assert_eq!(effects_after_first.line_received_qty, 4);
    let replay = send_receipt(
        &app,
        &token,
        tenant_id,
        replay_line_id,
        "canonical-expected-receipt-replay",
        receipt_body,
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ReceiveExpectedInventoryResult>(replay).await,
        first
    );
    assert_eq!(
        receipt_effects(&fixture.db, tenant_id, replay_line_id).await,
        effects_after_first
    );

    let changed_reuse = send_receipt(
        &app,
        &token,
        tenant_id,
        replay_line_id,
        "canonical-expected-receipt-replay",
        json!({
            "receiving_location_id": receiving_location_id,
            "received_qty": 5,
            "rejected_qty": 0,
            "missing_qty": 0,
            "lot": "CANONICAL-LOT"
        }),
    )
    .await;
    assert_eq!(changed_reuse.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_reuse).await.code,
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        receipt_effects(&fixture.db, tenant_id, replay_line_id).await,
        effects_after_first
    );

    let missing_location_before =
        receipt_effects(&fixture.db, tenant_id, no_location_line_id).await;
    let missing_location = send_receipt(
        &app,
        &token,
        tenant_id,
        no_location_line_id,
        "canonical-expected-receipt-no-location",
        json!({
            "received_qty": 1,
            "rejected_qty": 0,
            "missing_qty": 0
        }),
    )
    .await;
    assert_eq!(missing_location.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        receipt_effects(&fixture.db, tenant_id, no_location_line_id).await,
        missing_location_before
    );

    let missing_exception_before = receipt_effects(&fixture.db, tenant_id, exception_line_id).await;
    for (key, rejected_qty, missing_qty) in [
        ("canonical-expected-receipt-rejected-reason", 1, 0),
        ("canonical-expected-receipt-missing-reason", 0, 1),
    ] {
        let missing_reason = send_receipt(
            &app,
            &token,
            tenant_id,
            exception_line_id,
            key,
            json!({
                "received_qty": 0,
                "rejected_qty": rejected_qty,
                "missing_qty": missing_qty
            }),
        )
        .await;
        assert_eq!(missing_reason.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        receipt_effects(&fixture.db, tenant_id, exception_line_id).await,
        missing_exception_before
    );

    let rejection_before = receipt_effects(&fixture.db, tenant_id, rejection_only_line_id).await;
    let rejection_only = send_receipt(
        &app,
        &token,
        tenant_id,
        rejection_only_line_id,
        "canonical-expected-receipt-rejection-only",
        json!({
            "received_qty": 0,
            "rejected_qty": 3,
            "missing_qty": 0,
            "exception_reason": "quality_rejected",
            "exception_note": "Cases failed receiving inspection"
        }),
    )
    .await;
    assert_eq!(rejection_only.status(), StatusCode::OK);
    let rejection_only: ReceiveExpectedInventoryResult = response_json(rejection_only).await;
    assert_eq!(rejection_only.load_id, load_id);
    assert_eq!(rejection_only.load_line_id, rejection_only_line_id);
    assert_eq!(rejection_only.inventory_transaction_id, None);
    assert_eq!(rejection_only.item_batch_id, None);
    assert_eq!(rejection_only.license_plate_id, None);
    assert_eq!(rejection_only.cumulative_received_qty, 0);
    assert_eq!(rejection_only.cumulative_rejected_qty, 3);
    assert_eq!(rejection_only.cumulative_missing_qty, 0);
    let rejection_after = receipt_effects(&fixture.db, tenant_id, rejection_only_line_id).await;
    assert_eq!(rejection_after.transactions, rejection_before.transactions);
    assert_eq!(rejection_after.entries, rejection_before.entries);
    assert_eq!(rejection_after.item_batches, rejection_before.item_batches);
    assert_eq!(rejection_after.balances, rejection_before.balances);
    assert_eq!(rejection_after.qty_on_hand, rejection_before.qty_on_hand);
    assert_eq!(
        rejection_after.license_plates,
        rejection_before.license_plates
    );
    assert_eq!(rejection_after.line_received_qty, 0);
    assert_eq!(rejection_after.line_rejected_qty, 3);

    let unknown_before = receipt_effects(&fixture.db, tenant_id, unknown_field_line_id).await;
    let unknown_field = send_receipt(
        &app,
        &token,
        tenant_id,
        unknown_field_line_id,
        "canonical-expected-receipt-unknown-field",
        json!({
            "receiving_location_id": receiving_location_id,
            "received_qty": 1,
            "rejected_qty": 0,
            "missing_qty": 0,
            "status": "available"
        }),
    )
    .await;
    assert_eq!(unknown_field.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        receipt_effects(&fixture.db, tenant_id, unknown_field_line_id).await,
        unknown_before
    );
}
