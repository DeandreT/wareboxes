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
    DisposeInboundInspectionResponse, ErrorReason, ErrorResponse, ExpectedReceivingLoadStatus,
    InboundInspectionOutcome, InventoryBalanceStatus, UnexpectedReceiptConfirmationResponse,
    UnexpectedReceiptReason,
};

fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

fn command_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/expected-receiving/loads/{load_id}/unexpected-receipts"
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn inspection_request(
    token: &str,
    tenant_id: TenantId,
    hold_id: i64,
    key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/inbound-inspections/{hold_id}/dispositions"
        ))
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
    facility_id: i64,
    owner_id: i64,
    load_id: i64,
    line_id: i64,
}

async fn setup(fixture: &Fixture, email: &str) -> Setup {
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("unexpected-inspector-{}", operator.id),
        Some("Inspect unexpected inbound receipts"),
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
    let facility_id = fixture.facility(tenant_id, "Unexpected Receipt DC").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Unexpected Receipt Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let dock_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("UNEXPECTED-DOCK-01"),
        Some("Unexpected receiving dock"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let expected_item_id = fixture
        .item(tenant_id, "Expected receipt case", "case")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        expected_item_id,
        "EXPECTED-CASE-01",
        "code128",
        None,
    )
    .await
    .unwrap();
    let unexpected_item_id = fixture
        .item(tenant_id, "Unexpected receipt case", "case")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        unexpected_item_id,
        "UNEXPECTED-CASE-01",
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
        owner_id,
        "UNEXPECTED-LOAD-01",
        LoadType::Inbound,
        Some("ASN-UNEXPECTED-01"),
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
        expected_item_id,
        None,
        2,
        Some("EXPECTED-LOT-01"),
        None,
        None,
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE loads SET status='receiving' WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(load_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Setup {
        tenant_id,
        operator_id: operator.id,
        facility_id,
        owner_id,
        load_id,
        line_id,
    }
}

fn body(item_barcode: &str, reason: &str, quantity: i64) -> Value {
    json!({
        "item_barcode": item_barcode,
        "receiving_location_barcode": "UNEXPECTED-DOCK-01",
        "quantity": quantity,
        "license_plate_barcode": "UNEXPECTED-LP-01",
        "lot": "UNEXPECTED-LOT-01",
        "serial": null,
        "expiration": null,
        "reason": reason,
        "note": "Physical stock was not part of the expected quantity"
    })
}

#[tokio::test]
async fn unexpected_receipt_is_quarantined_held_audited_and_replay_safe() {
    init_test_tracing();
    let fixture = Fixture::new().await;
    let setup = setup(&fixture, "unexpected-receipt-success@test.local").await;
    let token = auth::create_session(&fixture.db, setup.operator_id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let response = app
        .clone()
        .oneshot(command_request(
            &token,
            setup.tenant_id,
            setup.load_id,
            "unexpected-success",
            &body("UNEXPECTED-CASE-01", "unexpected_item", 3),
        ))
        .await
        .unwrap();
    if response.status() != StatusCode::OK {
        panic!(
            "unexpected receipt failed: {}",
            response_json::<Value>(response).await
        );
    }
    let result: UnexpectedReceiptConfirmationResponse = response_json(response).await;
    assert_eq!(result.load_id, setup.load_id);
    assert_eq!(result.inventory_owner_id, setup.owner_id);
    assert_eq!(result.facility_id, setup.facility_id);
    assert_eq!(result.quantity, 3);
    assert_eq!(result.uom, "case");
    assert_eq!(result.reason, UnexpectedReceiptReason::UnexpectedItem);
    assert_eq!(result.inventory_status, InventoryBalanceStatus::Quarantine);
    assert_eq!(result.load_status, ExpectedReceivingLoadStatus::Receiving);

    let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    let line: (i64, i64, i64, String) = sqlx::query_as(
        "SELECT received_qty,rejected_qty,missing_qty,status FROM load_lines WHERE id=$1",
    )
    .bind(setup.line_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(line, (0, 0, 0, "pending".into()));
    let balance: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT status,qty_on_hand,qty_reserved,qty_held FROM inventory_balances WHERE id=$1",
    )
    .bind(result.inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(balance, ("quarantine".into(), 3, 0, 3));
    let hold: (String, i64, String, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT status,qty,reason_code,reference_type,reference_id FROM inventory_holds WHERE id=$1",
    )
    .bind(result.inventory_hold_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        hold,
        (
            "active".into(),
            3,
            "inventory_discrepancy".into(),
            Some("unexpected_receipt".into()),
            Some(result.unexpected_receipt_id)
        )
    );
    let evidence_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM unexpected_receipts WHERE id=$1 AND inventory_transaction_id=$2 AND inventory_hold_id=$3",
    )
    .bind(result.unexpected_receipt_id)
    .bind(result.inventory_transaction_id)
    .bind(result.inventory_hold_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence_count, 1);
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM outbox_events WHERE event_type IN ('inbound.unexpected_receipt.confirmed','inventory.hold.placed') ORDER BY event_type",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        events,
        [
            "inbound.unexpected_receipt.confirmed",
            "inventory.hold.placed"
        ]
    );
    tx.rollback().await.unwrap();

    let replay = app
        .clone()
        .oneshot(command_request(
            &token,
            setup.tenant_id,
            setup.load_id,
            "unexpected-success",
            &body("UNEXPECTED-CASE-01", "unexpected_item", 3),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<UnexpectedReceiptConfirmationResponse>(replay).await,
        result
    );
    assert_error(
        app.clone()
            .oneshot(command_request(
                &token,
                setup.tenant_id,
                setup.load_id,
                "unexpected-success",
                &body("UNEXPECTED-CASE-01", "unexpected_item", 2),
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
    )
    .await;

    let inspection = app
        .oneshot(inspection_request(
            &token,
            setup.tenant_id,
            result.inventory_hold_id,
            "unexpected-damage-inspection",
            &json!({
                "outcome": "damaged",
                "note": "Mis-shipped cases failed inbound inspection"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(inspection.status(), StatusCode::OK);
    let inspection: DisposeInboundInspectionResponse = response_json(inspection).await;
    assert_eq!(inspection.outcome, InboundInspectionOutcome::Damaged);
    assert_eq!(inspection.target_status, InventoryBalanceStatus::Damaged);
    assert_eq!(inspection.quantity, 3);
    assert_eq!(inspection.inventory_hold_id, result.inventory_hold_id);

    let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    let disposition_reference: (String, i64, String, i64) = sqlx::query_as(
        r#"
        SELECT source_reference_type, source_reference_id, target_status, quantity
        FROM inbound_inspection_dispositions
        WHERE id=$1
        "#,
    )
    .bind(inspection.disposition_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        disposition_reference,
        (
            "unexpected_receipt".into(),
            result.unexpected_receipt_id,
            "damaged".into(),
            3
        )
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn unexpected_receipt_reason_identity_and_scope_fail_closed_without_effects() {
    init_test_tracing();
    let fixture = Fixture::new().await;
    let setup = setup(&fixture, "unexpected-receipt-invalid@test.local").await;
    let token = auth::create_session(&fixture.db, setup.operator_id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    for (key, request_body, status) in [
        (
            "unexpected-wrong-reason",
            body("EXPECTED-CASE-01", "unexpected_item", 1),
            StatusCode::CONFLICT,
        ),
        (
            "unexpected-unknown-item",
            body("NO-SUCH-ITEM", "unexpected_item", 1),
            StatusCode::CONFLICT,
        ),
        (
            "unexpected-wrong-dock",
            {
                let mut value = body("UNEXPECTED-CASE-01", "unexpected_item", 1);
                value["receiving_location_barcode"] = json!("WRONG-DOCK");
                value
            },
            StatusCode::CONFLICT,
        ),
        (
            "unexpected-other-note",
            {
                let mut value = body("UNEXPECTED-CASE-01", "other", 1);
                value["note"] = Value::Null;
                value
            },
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(command_request(
                &token,
                setup.tenant_id,
                setup.load_id,
                key,
                &request_body,
            ))
            .await
            .unwrap();
        assert_error(
            response,
            status,
            if status == StatusCode::BAD_REQUEST {
                ErrorReason::InvalidRequest
            } else {
                ErrorReason::Conflict
            },
        )
        .await;
    }
    let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM unexpected_receipts), (SELECT COUNT(*) FROM inventory_transactions), (SELECT COUNT(*) FROM inventory_holds), (SELECT COUNT(*) FROM command_idempotency_records)",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (0, 0, 0, 0));
    tx.rollback().await.unwrap();

    let successful = app
        .clone()
        .oneshot(command_request(
            &token,
            setup.tenant_id,
            setup.load_id,
            "unexpected-scope",
            &body("UNEXPECTED-CASE-01", "unexpected_item", 1),
        ))
        .await
        .unwrap();
    if successful.status() != StatusCode::OK {
        panic!(
            "unexpected scoped receipt failed: {}",
            response_json::<Value>(successful).await
        );
    }
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
    for (key, quantity) in [
        ("unexpected-scope", 1),
        ("unexpected-scope", 2),
        ("unexpected-new-after-scope", 1),
    ] {
        assert_error(
            app.clone()
                .oneshot(command_request(
                    &token,
                    setup.tenant_id,
                    setup.load_id,
                    key,
                    &body("UNEXPECTED-CASE-01", "unexpected_item", quantity),
                ))
                .await
                .unwrap(),
            StatusCode::NOT_FOUND,
            ErrorReason::NotFound,
        )
        .await;
    }
}

#[tokio::test]
async fn received_load_accepts_excess_without_reopening_expectations_and_evidence_is_immutable() {
    let fixture = Fixture::new().await;
    let setup = setup(&fixture, "unexpected-receipt-late@test.local").await;
    let token = auth::create_session(&fixture.db, setup.operator_id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let expected_resolution = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/expected-receiving/lines/{}/confirmations",
            setup.line_id
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, setup.tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, "unexpected-late-close")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "disposition": "missing",
                "quantity": 2,
                "reason": "short_shipment",
                "note": null
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(expected_resolution)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let response = app
        .oneshot(command_request(
            &token,
            setup.tenant_id,
            setup.load_id,
            "unexpected-late-excess",
            &body("EXPECTED-CASE-01", "excess", 4),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: UnexpectedReceiptConfirmationResponse = response_json(response).await;
    assert_eq!(result.reason, UnexpectedReceiptReason::Excess);
    assert_eq!(result.load_status, ExpectedReceivingLoadStatus::Received);
    assert_eq!(result.quantity, 4);

    let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    let load_status: String =
        sqlx::query_scalar("SELECT status FROM loads WHERE tenant_id=$1 AND id=$2")
            .bind(setup.tenant_id.get())
            .bind(setup.load_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(load_status, "received");
    let line: (i64, i64, i64, String) = sqlx::query_as(
        "SELECT received_qty,rejected_qty,missing_qty,status FROM load_lines WHERE id=$1",
    )
    .bind(setup.line_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(line, (0, 0, 2, "missing".into()));
    tx.rollback().await.unwrap();

    let admin = admin_db_for(&fixture.db).await;
    let privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege('wareboxes_app','unexpected_receipts','SELECT'),
               has_table_privilege('wareboxes_app','unexpected_receipts','INSERT'),
               has_table_privilege('wareboxes_app','unexpected_receipts','UPDATE'),
               has_table_privilege('wareboxes_app','unexpected_receipts','DELETE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
    let rls: (bool, bool) = sqlx::query_as(
        "SELECT relrowsecurity,relforcerowsecurity FROM pg_class WHERE oid='unexpected_receipts'::regclass",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(rls, (true, true));
    let immutable = sqlx::query("UPDATE unexpected_receipts SET note=note WHERE id=$1")
        .bind(result.unexpected_receipt_id)
        .execute(&admin)
        .await
        .unwrap_err();
    assert!(immutable
        .to_string()
        .contains("unexpected receipts are immutable"));
}
