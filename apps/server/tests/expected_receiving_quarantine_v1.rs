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
    DisposeInboundInspectionResponse, ErrorReason, ErrorResponse,
    ExpectedReceiptConfirmationResponse, ExpectedReceiptDisposition, ExpectedReceiptLineStatus,
    ExpectedReceivingLoadStatus, InboundInspectionOutcome, InventoryBalanceStatus,
};

fn command_request(
    token: &str,
    tenant_id: TenantId,
    load_line_id: i64,
    key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/expected-receiving/lines/{load_line_id}/confirmations"
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn mutation_request(
    token: &str,
    tenant_id: TenantId,
    path: String,
    key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn assert_error(response: axum::response::Response, status: StatusCode, reason: ErrorReason) {
    assert_eq!(response.status(), status);
    assert_eq!(
        response_json::<ErrorResponse>(response).await.reason,
        reason
    );
}

struct Setup {
    tenant_id: TenantId,
    operator_id: i64,
    line_id: i64,
}

async fn setup(fixture: &Fixture, email: &str) -> Setup {
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("inbound-inspector-{}", operator.id),
        Some("Dispose quarantined inbound receipts"),
    )
    .await
    .unwrap();
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        "wms_supervisor",
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            "wms_supervisor",
            Some("Supervise warehouse exceptions"),
        )
        .await
        .unwrap(),
    };
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        operator.id,
        role,
    )
    .await
    .unwrap();
    let facility_id = fixture.facility(tenant_id, "Inbound Quarantine DC").await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Inbound Quarantine Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let dock_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("QA-DOCK-01"),
        Some("QA receiving dock"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let item_id = fixture
        .item(tenant_id, "Inbound quarantine case", "case")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        "QA-CASE-01",
        "code128",
        None,
    )
    .await
    .unwrap();
    let load_id = repo::loads::add_load_with_execution_barcode(
        &fixture.db,
        tenant_id,
        operator.id,
        facility_id,
        inventory_owner_id,
        "QA-LOAD-01",
        LoadType::Inbound,
        Some("ASN-QA-01"),
        None,
        None,
        None,
        None,
        Some(dock_id),
        None,
        None,
    )
    .await
    .unwrap();
    let line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        operator.id,
        load_id,
        item_id,
        None,
        4,
        Some("LOT-QA-01"),
        None,
        None,
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE loads SET status = 'arrived' WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(load_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    Setup {
        tenant_id,
        operator_id: operator.id,
        line_id,
    }
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Effects {
    received_qty: i64,
    rejected_qty: i64,
    missing_qty: i64,
    line_status: String,
    load_status: String,
    transaction_count: i64,
    entry_count: i64,
    balance_count: i64,
    hold_count: i64,
    held_qty: i64,
    command_count: i64,
    receipt_event_count: i64,
    hold_event_count: i64,
}

async fn effects(fixture: &Fixture, tenant_id: TenantId, line_id: i64) -> Effects {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let row = sqlx::query_as(
        r#"
        SELECT line.received_qty, line.rejected_qty, line.missing_qty,
               line.status AS line_status, load.status AS load_status,
               (SELECT COUNT(*) FROM inventory_transactions) AS transaction_count,
               (SELECT COUNT(*) FROM inventory_entries) AS entry_count,
               (SELECT COUNT(*) FROM inventory_balances WHERE deleted IS NULL) AS balance_count,
               (SELECT COUNT(*) FROM inventory_holds) AS hold_count,
               (SELECT COALESCE(SUM(qty_held), 0)::BIGINT
                  FROM inventory_balances WHERE deleted IS NULL) AS held_qty,
               (SELECT COUNT(*) FROM command_idempotency_records) AS command_count,
               (SELECT COUNT(*) FROM outbox_events
                 WHERE event_type = 'inbound.expected_receipt.confirmed') AS receipt_event_count,
               (SELECT COUNT(*) FROM outbox_events
                 WHERE event_type = 'inventory.hold.placed') AS hold_event_count
        FROM load_lines line
        INNER JOIN loads load
          ON load.tenant_id = line.tenant_id AND load.id = line.load_id
        WHERE line.tenant_id = $1 AND line.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(line_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    row
}

fn quarantined_body(quantity: i64) -> Value {
    json!({
        "disposition": "quarantined",
        "item_barcode": "QA-CASE-01",
        "receiving_location_barcode": "QA-DOCK-01",
        "quantity": quantity,
        "license_plate_barcode": "QA-LP-01",
        "lot": "LOT-QA-01",
        "serial": null,
        "expiration": null,
        "reason": "damaged",
        "note": "Outer case was crushed"
    })
}

async fn quarantine_receipt(
    app: &axum::Router,
    token: &str,
    setup: &Setup,
    key: &str,
    quantity: i64,
) -> ExpectedReceiptConfirmationResponse {
    let response = app
        .clone()
        .oneshot(command_request(
            token,
            setup.tenant_id,
            setup.line_id,
            key,
            &quarantined_body(quantity),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

#[tokio::test]
async fn quarantined_receipt_conserves_physical_stock_and_replays_exactly() {
    let fixture = Fixture::new().await;
    let setup = setup(&fixture, "expected-quarantine-success@test.local").await;
    let token = auth::create_session(&fixture.db, setup.operator_id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let before = effects(&fixture, setup.tenant_id, setup.line_id).await;

    let response = app
        .clone()
        .oneshot(command_request(
            &token,
            setup.tenant_id,
            setup.line_id,
            "quarantine-success",
            &quarantined_body(2),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: ExpectedReceiptConfirmationResponse = response_json(response).await;
    assert_eq!(result.disposition, ExpectedReceiptDisposition::Quarantined);
    assert_eq!(result.quantity, 2);
    assert_eq!(
        result.inventory_status,
        Some(InventoryBalanceStatus::Quarantine)
    );
    assert_eq!(result.line_status, ExpectedReceiptLineStatus::Partial);
    assert_eq!(result.load_status, ExpectedReceivingLoadStatus::Receiving);
    assert_eq!(result.cumulative_received_quantity, 0);
    assert_eq!(result.cumulative_rejected_quantity, 2);
    assert_eq!(result.remaining_quantity, 2);
    let balance_id = result.inventory_balance_id.unwrap();
    let hold_id = result.inventory_hold_id.unwrap();

    let after = effects(&fixture, setup.tenant_id, setup.line_id).await;
    assert_eq!(after.received_qty, 0);
    assert_eq!(after.rejected_qty, 2);
    assert_eq!(after.missing_qty, 0);
    assert_eq!(after.line_status, "partial");
    assert_eq!(after.load_status, "receiving");
    assert_eq!(after.transaction_count, before.transaction_count + 1);
    assert_eq!(after.entry_count, before.entry_count + 1);
    assert_eq!(after.balance_count, before.balance_count + 1);
    assert_eq!(after.hold_count, before.hold_count + 1);
    assert_eq!(after.held_qty, before.held_qty + 2);
    assert_eq!(after.command_count, before.command_count + 1);
    assert_eq!(after.receipt_event_count, before.receipt_event_count + 1);
    assert_eq!(after.hold_event_count, before.hold_event_count + 1);

    let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    let balance: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT status, qty_on_hand, qty_reserved, qty_held FROM inventory_balances WHERE id = $1",
    )
    .bind(balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(balance, ("quarantine".into(), 2, 0, 2));
    let hold: (String, i64, String, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT status, qty, reason_code, reference_type, reference_id FROM inventory_holds WHERE id = $1",
    )
    .bind(hold_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        hold,
        (
            "active".into(),
            2,
            "damage_suspected".into(),
            Some("expected_receipt_line".into()),
            Some(setup.line_id)
        )
    );
    let entry: (String, i64) = sqlx::query_as(
        "SELECT status, quantity_delta FROM inventory_entries WHERE transaction_id = $1",
    )
    .bind(result.inventory_transaction_id.unwrap())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(entry, ("quarantine".into(), 2));
    tx.rollback().await.unwrap();

    let replay = app
        .clone()
        .oneshot(command_request(
            &token,
            setup.tenant_id,
            setup.line_id,
            "quarantine-success",
            &quarantined_body(2),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ExpectedReceiptConfirmationResponse>(replay).await,
        result
    );
    assert_eq!(
        effects(&fixture, setup.tenant_id, setup.line_id).await,
        after
    );

    let app_db = app_db_for(&fixture.db).await;
    let mut forged_release = tenant_tx(&app_db, setup.tenant_id).await;
    sqlx::query(
        r#"
        UPDATE inventory_holds
        SET modified=statement_timestamp(), deleted=statement_timestamp(),
            released_by=$1, released_at=statement_timestamp(), status='released'
        WHERE tenant_id=$2 AND id=$3
        "#,
    )
    .bind(setup.operator_id)
    .bind(setup.tenant_id.get())
    .bind(hold_id)
    .execute(&mut *forged_release)
    .await
    .unwrap();
    let error = forged_release.commit().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("release requires an inspection disposition"),
        "unexpected direct-release error: {error}"
    );

    assert_error(
        app.clone()
            .oneshot(command_request(
                &token,
                setup.tenant_id,
                setup.line_id,
                "quarantine-success",
                &quarantined_body(1),
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
    )
    .await;

    assert_error(
        app.clone()
            .oneshot(mutation_request(
                &token,
                setup.tenant_id,
                format!("/api/v1/inventory/holds/{hold_id}/releases"),
                "generic-release-rejected",
                &json!({}),
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
    )
    .await;

    let disposition = app
        .clone()
        .oneshot(mutation_request(
            &token,
            setup.tenant_id,
            format!("/api/v1/inbound-inspections/{hold_id}/dispositions"),
            "approve-receipt-inspection",
            &json!({
                "outcome": "approved",
                "note": "Seal and contents passed inbound inspection"
            }),
        ))
        .await
        .unwrap();
    if disposition.status() != StatusCode::OK {
        let status = disposition.status();
        let body: Value = response_json(disposition).await;
        panic!("inspection disposition returned {status}: {body}");
    }
    let disposition: DisposeInboundInspectionResponse = response_json(disposition).await;
    assert_eq!(disposition.inventory_hold_id, hold_id);
    assert_eq!(disposition.source_inventory_balance_id, balance_id);
    assert_eq!(disposition.quantity, 2);
    assert_eq!(disposition.outcome, InboundInspectionOutcome::Approved);
    assert_eq!(disposition.target_status, InventoryBalanceStatus::Available);

    let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    let projection: (String, i64, i64, String, i64, i64) = sqlx::query_as(
        r#"
        SELECT hold.status, source.qty_on_hand, source.qty_held,
               target.status, target.qty_on_hand,
               (SELECT COUNT(*) FROM inbound_inspection_dispositions
                WHERE inventory_hold_id = hold.id)
        FROM inventory_holds hold
        INNER JOIN inventory_balances source
          ON source.tenant_id = hold.tenant_id
         AND source.id = hold.inventory_balance_id
        INNER JOIN inventory_balances target
          ON target.tenant_id = hold.tenant_id
         AND target.id = $2
        WHERE hold.tenant_id = $1 AND hold.id = $3
        "#,
    )
    .bind(setup.tenant_id.get())
    .bind(disposition.target_inventory_balance_id)
    .bind(hold_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        projection,
        ("released".into(), 0, 0, "available".into(), 2, 1)
    );
    let journal: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, quantity_delta FROM inventory_entries WHERE transaction_id=$1 ORDER BY quantity_delta",
    )
    .bind(disposition.inventory_transaction_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        journal,
        vec![("quarantine".into(), -2), ("available".into(), 2)]
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn quarantined_receipt_rejects_invalid_identity_quantity_and_reason_without_effects() {
    let fixture = Fixture::new().await;
    let setup = setup(&fixture, "expected-quarantine-invalid@test.local").await;
    let token = auth::create_session(&fixture.db, setup.operator_id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let before = effects(&fixture, setup.tenant_id, setup.line_id).await;

    let mut wrong_item = quarantined_body(1);
    wrong_item["item_barcode"] = json!("WRONG-ITEM");
    assert_error(
        app.clone()
            .oneshot(command_request(
                &token,
                setup.tenant_id,
                setup.line_id,
                "quarantine-wrong-item",
                &wrong_item,
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
    )
    .await;

    let mut other_without_note = quarantined_body(1);
    other_without_note["reason"] = json!("other");
    other_without_note["note"] = Value::Null;
    assert_error(
        app.clone()
            .oneshot(command_request(
                &token,
                setup.tenant_id,
                setup.line_id,
                "quarantine-other-no-note",
                &other_without_note,
            ))
            .await
            .unwrap(),
        StatusCode::BAD_REQUEST,
        ErrorReason::InvalidRequest,
    )
    .await;

    assert_error(
        app.clone()
            .oneshot(command_request(
                &token,
                setup.tenant_id,
                setup.line_id,
                "quarantine-over-quantity",
                &quarantined_body(5),
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
    )
    .await;
    assert_eq!(
        effects(&fixture, setup.tenant_id, setup.line_id).await,
        before
    );
}

#[tokio::test]
async fn quarantined_receipt_replay_is_concealed_after_scope_revocation() {
    let fixture = Fixture::new().await;
    let setup = setup(&fixture, "expected-quarantine-scope@test.local").await;
    let token = auth::create_session(&fixture.db, setup.operator_id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let response = app
        .clone()
        .oneshot(command_request(
            &token,
            setup.tenant_id,
            setup.line_id,
            "quarantine-scope",
            &quarantined_body(1),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let after = effects(&fixture, setup.tenant_id, setup.line_id).await;

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        setup.tenant_id,
        &wareboxes_core::dto::UpdateUserAccessScope {
            user_id: setup.operator_id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    for (key, body) in [
        ("quarantine-scope", quarantined_body(1)),
        ("quarantine-scope", quarantined_body(2)),
        ("quarantine-new-after-scope", quarantined_body(1)),
    ] {
        assert_error(
            app.clone()
                .oneshot(command_request(
                    &token,
                    setup.tenant_id,
                    setup.line_id,
                    key,
                    &body,
                ))
                .await
                .unwrap(),
            StatusCode::NOT_FOUND,
            ErrorReason::NotFound,
        )
        .await;
    }
    assert_eq!(
        effects(&fixture, setup.tenant_id, setup.line_id).await,
        after
    );
}

#[tokio::test]
async fn concurrent_inspection_dispositions_have_one_replay_safe_winner() {
    let fixture = Fixture::new().await;
    let setup = setup(&fixture, "inbound-inspection-race@test.local").await;
    let token = auth::create_session(&fixture.db, setup.operator_id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let receipt = quarantine_receipt(&app, &token, &setup, "inspection-race-receipt", 4).await;
    let hold_id = receipt.inventory_hold_id.unwrap();
    let approve = json!({"outcome":"approved","note":"All inspected units passed"});
    let damage = json!({"outcome":"damaged","note":"All inspected units were damaged"});
    let path = format!("/api/v1/inbound-inspections/{hold_id}/dispositions");
    let (first, second) = tokio::join!(
        app.clone().oneshot(mutation_request(
            &token,
            setup.tenant_id,
            path.clone(),
            "inspection-race-approve",
            &approve,
        )),
        app.clone().oneshot(mutation_request(
            &token,
            setup.tenant_id,
            path.clone(),
            "inspection-race-damage",
            &damage,
        )),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        [first.status(), second.status()]
            .into_iter()
            .filter(|status| *status == StatusCode::OK)
            .count(),
        1
    );
    let first_wins = first.status() == StatusCode::OK;
    let (winner_key, winner_body, winner) = if first_wins {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        (
            "inspection-race-approve",
            approve.clone(),
            response_json::<DisposeInboundInspectionResponse>(first).await,
        )
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        (
            "inspection-race-damage",
            damage.clone(),
            response_json::<DisposeInboundInspectionResponse>(second).await,
        )
    };

    let replay = app
        .clone()
        .oneshot(mutation_request(
            &token,
            setup.tenant_id,
            path.clone(),
            winner_key,
            &winner_body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<DisposeInboundInspectionResponse>(replay).await,
        winner
    );
    let changed = app
        .oneshot(mutation_request(
            &token,
            setup.tenant_id,
            path,
            winner_key,
            &json!({"outcome":"approved","note":"Changed evidence"}),
        ))
        .await
        .unwrap();
    assert_error(
        changed,
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
    )
    .await;

    let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_inspection_dispositions WHERE inventory_hold_id=$1),
          (SELECT COUNT(*) FROM inventory_transactions
           WHERE operation='inbound.inspection.dispose.v1'),
          (SELECT COUNT(*) FROM outbox_events
           WHERE event_type='inbound.inspection.disposed'),
          (SELECT COUNT(*) FROM load_activity
           WHERE action='inbound_inspection_disposed')
        "#,
    )
    .bind(hold_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(counts, (1, 1, 1, 1));
    tx.rollback().await.unwrap();

    let privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege(current_user, 'inbound_inspection_dispositions', 'SELECT'),
               has_table_privilege(current_user, 'inbound_inspection_dispositions', 'INSERT'),
               has_table_privilege(current_user, 'inbound_inspection_dispositions', 'UPDATE'),
               has_table_privilege(current_user, 'inbound_inspection_dispositions', 'DELETE')
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
    let mut app_tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    assert!(
        sqlx::query("UPDATE inbound_inspection_dispositions SET note='forged'")
            .execute(&mut *app_tx)
            .await
            .is_err()
    );
    app_tx.rollback().await.unwrap();
    let admin = admin_db_for(&fixture.db).await;
    let immutable = sqlx::query(
        "UPDATE inbound_inspection_dispositions SET note='forged' WHERE inventory_hold_id=$1",
    )
    .bind(hold_id)
    .execute(&admin)
    .await
    .unwrap_err();
    assert_eq!(
        immutable.as_database_error().unwrap().message(),
        "inbound inspection dispositions are immutable"
    );
    admin.close().await;
}

#[tokio::test]
async fn inspection_requires_supervisor_and_conceals_cross_tenant_holds() {
    let fixture = Fixture::new().await;
    let owner_setup = setup(&fixture, "inbound-inspection-owner@test.local").await;
    let owner_token = auth::create_session(&fixture.db, owner_setup.operator_id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let receipt = quarantine_receipt(
        &app,
        &owner_token,
        &owner_setup,
        "inspection-scope-receipt",
        1,
    )
    .await;
    let hold_id = receipt.inventory_hold_id.unwrap();
    let body = json!({"outcome":"approved","note":"Inspected and approved"});

    let worker = fixture
        .wms_user("inbound-inspection-worker@test.local")
        .await;
    let worker_tenant = tenant_for_user(&fixture.db, worker.id).await;
    let worker_token = auth::create_session(&fixture.db, worker.id).await.unwrap();
    let denied = app
        .clone()
        .oneshot(mutation_request(
            &worker_token,
            worker_tenant,
            format!("/api/v1/inbound-inspections/{hold_id}/dispositions"),
            "inspection-worker-denied",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let other_setup = setup(&fixture, "inbound-inspection-other@test.local").await;
    let other_token = auth::create_session(&fixture.db, other_setup.operator_id)
        .await
        .unwrap();
    let concealed = app
        .oneshot(mutation_request(
            &other_token,
            other_setup.tenant_id,
            format!("/api/v1/inbound-inspections/{hold_id}/dispositions"),
            "inspection-cross-tenant",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);

    let mut tx = tenant_tx(&fixture.db, owner_setup.tenant_id).await;
    let unchanged: (String, i64, i64) = sqlx::query_as(
        r#"
        SELECT hold.status, balance.qty_held,
               (SELECT COUNT(*) FROM inbound_inspection_dispositions)
        FROM inventory_holds hold
        INNER JOIN inventory_balances balance
          ON balance.tenant_id=hold.tenant_id AND balance.id=hold.inventory_balance_id
        WHERE hold.id=$1
        "#,
    )
    .bind(hold_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(unchanged, ("active".into(), 1, 0));
    tx.rollback().await.unwrap();
}
