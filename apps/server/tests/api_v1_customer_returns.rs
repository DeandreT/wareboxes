mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    CancelCustomerReturnResponse, CreateCustomerReturnResponse, CustomerReturnDetailResponse,
    CustomerReturnPage, CustomerReturnStatus, ErrorReason, ErrorResponse,
    ExpectedReceiptConfirmationResponse, ExpectedReceiptDisposition,
    PlanCustomerReturnLoadResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct ReturnFixture {
    fixture: Fixture,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    dock_id: i64,
    item_id: i64,
    user_id: i64,
    token: String,
}

async fn fixture(email: &str) -> ReturnFixture {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture.facility(tenant_id, "Customer Returns DC").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Customer Returns Client")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let dock_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("RETURN-DOCK-01"),
        Some("Returns receiving dock"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let item_id = fixture
        .item(tenant_id, "Returned canned goods", "case")
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
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
    tx.commit().await.unwrap();
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        "RETURN-ITEM-01",
        "code128",
        None,
    )
    .await
    .unwrap();
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    ReturnFixture {
        fixture,
        tenant_id,
        owner_id,
        facility_id,
        dock_id,
        item_id,
        user_id: operator.id,
        token,
    }
}

#[tokio::test]
async fn guessed_ids_and_replays_are_concealed_after_scope_changes() {
    let context = fixture("customer-return-scope@test.local").await;
    let body = create_body(&context, "RMA-RETURN-SCOPE");
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let created = app
        .clone()
        .oneshot(command_request(
            &context,
            "customer-returns",
            "return-create-scope",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created: CreateCustomerReturnResponse = response_json(created).await;

    let other = fixture("customer-return-other-tenant@test.local").await;
    let guessed = routes::app(AppState::new(other.fixture.db.clone()))
        .oneshot(get_request(
            &other,
            &format!("customer-returns/{}", created.customer_return_id),
        ))
        .await
        .unwrap();
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);

    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.user_id,
            all_facilities: true,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());

    let hidden = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("customer-returns/{}", created.customer_return_id),
        ))
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let replay = app
        .oneshot(command_request(
            &context,
            "customer-returns",
            "return-create-scope",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::NOT_FOUND);
}

fn command_request(context: &ReturnFixture, path: &str, key: &str, body: &Value) -> Request<Body> {
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

fn get_request(context: &ReturnFixture, path: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("/api/v1/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", context.token))
        .header(TENANT_ID_HEADER, context.tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

fn create_body(context: &ReturnFixture, number: &str) -> Value {
    json!({
        "inventory_owner_id": context.owner_id,
        "facility_id": context.facility_id,
        "number": number,
        "customer_reference": "WB-DEMO-ORDER-0001",
        "expected_at": "2027-09-12T17:00:00Z",
        "lines": [{
            "item_id": context.item_id,
            "authorized_quantity": 4,
            "reason": "damaged",
            "note": "Outer shipping carton crushed",
            "lot": "RETURN-LOT-01",
            "serial": null
        }]
    })
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn create_return(context: &ReturnFixture, number: &str) -> CreateCustomerReturnResponse {
    let response = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(command_request(
            context,
            "customer-returns",
            &format!("create-{number}"),
            &create_body(context, number),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

#[tokio::test]
async fn authorization_is_replayable_queryable_cancellable_and_immutable() {
    let context = fixture("customer-return-create@test.local").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let body = create_body(&context, "RMA-RETURN-100");
    let response = app
        .clone()
        .oneshot(command_request(
            &context,
            "customer-returns",
            "return-create-100",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created: CreateCustomerReturnResponse = response_json(response).await;
    assert_eq!(created.status, CustomerReturnStatus::Open);
    assert_eq!(created.revision.get(), 1);
    assert_eq!(created.lines.len(), 1);

    let replay = app
        .clone()
        .oneshot(command_request(
            &context,
            "customer-returns",
            "return-create-100",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(
        response_json::<CreateCustomerReturnResponse>(replay).await,
        created
    );
    let mut changed = body;
    changed["lines"][0]["authorized_quantity"] = json!(5);
    let conflict = app
        .clone()
        .oneshot(command_request(
            &context,
            "customer-returns",
            "return-create-100",
            &changed,
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(conflict).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let page = app
        .clone()
        .oneshot(get_request(
            &context,
            "customer-returns?status=open&search=RMA-RETURN-100",
        ))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        response_json::<CustomerReturnPage>(page).await.items.len(),
        1
    );
    let hidden_from_asns = app
        .clone()
        .oneshot(get_request(&context, "inbound-asns?search=RMA-RETURN-100"))
        .await
        .unwrap();
    assert_eq!(hidden_from_asns.status(), StatusCode::OK);
    let hidden: wareboxes_api_contract::v1::InboundAsnPage = response_json(hidden_from_asns).await;
    assert!(hidden.items.is_empty());

    let cancelled = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!(
                "customer-returns/{}/cancellations",
                created.customer_return_id
            ),
            "return-cancel-100",
            &json!({
                "expected_revision": 1,
                "reason": "customer_cancelled",
                "note": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled: CancelCustomerReturnResponse = response_json(cancelled).await;
    assert_eq!(cancelled.status, CustomerReturnStatus::Cancelled);
    assert_eq!(cancelled.revision.get(), 2);

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM customer_returns WHERE id=$1),
          (SELECT COUNT(*) FROM customer_return_lines WHERE customer_return_id=$1),
          (SELECT COUNT(*) FROM customer_return_cancellations WHERE customer_return_id=$1),
          (SELECT COUNT(*) FROM outbox_events WHERE aggregate_type='customer_return'
             AND aggregate_id=$1::TEXT),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE (result_json->>'customer_return_id')::BIGINT=$1)
        "#,
    )
    .bind(created.customer_return_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (1, 1, 1, 2, 2));
    let immutable = sqlx::query(
        "UPDATE customer_return_lines SET reason_code='warranty' WHERE customer_return_id=$1",
    )
    .bind(created.customer_return_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(!immutable.to_string().is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn planned_return_requires_quarantine_and_exposes_inspection_hold() {
    let context = fixture("customer-return-receive@test.local").await;
    let created = create_return(&context, "RMA-RETURN-200").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let planned = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("customer-returns/{}/load-plans", created.customer_return_id),
            "return-plan-200",
            &json!({
                "expected_revision": 1,
                "receiving_location_id": context.dock_id,
                "carrier": "Parity Returns",
                "trailer_number": "RETURN-TRL-200",
                "seal_number": "RETURN-SEAL-200"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(planned.status(), StatusCode::OK);
    let planned: PlanCustomerReturnLoadResponse = response_json(planned).await;
    assert_eq!(planned.status, CustomerReturnStatus::Planned);
    assert_eq!(planned.lines.len(), 1);
    let load_line_id = planned.lines[0].load_line_id;

    let scheduled = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("inbound-loads/{}/appointments", planned.load_id),
            "return-appointment-200",
            &json!({ "scheduled_for": "2027-09-12T17:00:00Z" }),
        ))
        .await
        .unwrap();
    assert_eq!(scheduled.status(), StatusCode::OK);
    let arrived = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("inbound-loads/{}/arrivals", planned.load_id),
            "return-arrival-200",
            &json!({
                "load_scan": planned.execution_barcode,
                "receiving_location_scan": "RETURN-DOCK-01",
                "arrived_at": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(arrived.status(), StatusCode::OK);
    let unloading = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("inbound-loads/{}/unloading-starts", planned.load_id),
            "return-unload-200",
            &json!({
                "load_scan": planned.execution_barcode,
                "receiving_location_scan": "RETURN-DOCK-01",
                "seal_scan": "RETURN-SEAL-200",
                "started_at": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(unloading.status(), StatusCode::OK);

    let ordinary = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("expected-receiving/lines/{load_line_id}/confirmations"),
            "return-receive-available-200",
            &json!({
                "disposition": "received",
                "item_barcode": "RETURN-ITEM-01",
                "receiving_location_barcode": "RETURN-DOCK-01",
                "quantity": 4,
                "license_plate_barcode": "RETURN-LP-200",
                "lot": "RETURN-LOT-01",
                "serial": null,
                "expiration": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(ordinary.status(), StatusCode::CONFLICT);

    let quarantined = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("expected-receiving/lines/{load_line_id}/confirmations"),
            "return-receive-quarantine-200",
            &json!({
                "disposition": "quarantined",
                "item_barcode": "RETURN-ITEM-01",
                "receiving_location_barcode": "RETURN-DOCK-01",
                "quantity": 4,
                "license_plate_barcode": "RETURN-LP-200",
                "lot": "RETURN-LOT-01",
                "serial": null,
                "expiration": null,
                "reason": "damaged",
                "note": "Return requires inspection"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(quarantined.status(), StatusCode::OK);
    let quarantined: ExpectedReceiptConfirmationResponse = response_json(quarantined).await;
    assert_eq!(
        quarantined.disposition,
        ExpectedReceiptDisposition::Quarantined
    );
    assert!(quarantined.inventory_hold_id.is_some());

    let detail = app
        .oneshot(get_request(
            &context,
            &format!("customer-returns/{}", created.customer_return_id),
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail: CustomerReturnDetailResponse = response_json(detail).await;
    assert_eq!(detail.summary.total_received_quantity, 0);
    assert_eq!(detail.summary.total_rejected_quantity, 4);
    assert_eq!(detail.summary.total_remaining_quantity, 0);
    assert_eq!(detail.lines[0].inspection_hold_ids.len(), 1);

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM customer_return_load_plans WHERE customer_return_id=$1),
          (SELECT COUNT(*) FROM inventory_transactions
             WHERE operation='inbound.confirm_expected_receipt.v1'),
          (SELECT COUNT(*) FROM inventory_entries WHERE quantity_delta=4),
          (SELECT COUNT(*) FROM inventory_holds
             WHERE reference_type='expected_receipt_line' AND reference_id=$2),
          (SELECT COUNT(*) FROM inventory_balances
             WHERE status='quarantine' AND qty_on_hand=4 AND qty_held=4)
        "#,
    )
    .bind(created.customer_return_id)
    .bind(load_line_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (1, 1, 1, 1, 1));
    tx.rollback().await.unwrap();
}
