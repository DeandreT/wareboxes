mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ArriveInboundLoadResponse, ArrivedInboundLoadStatus, CloseInboundLoadResponse, ErrorReason,
    ErrorResponse, ExpectedReceivingSessionResponse, InboundLoadClosedStatus,
    InboundLoadEntryItemResponse, PlanInboundLoadResponse, PlannedInboundLoadStatus,
    StartInboundLoadUnloadingResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn plan_request(
    token: &str,
    tenant_id: TenantId,
    idempotency_key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/inbound-loads")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn session_request(token: &str, tenant_id: TenantId, load_id: i64) -> Request<Body> {
    Request::builder()
        .uri(format!("/api/v1/expected-receiving/loads/{load_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

fn arrival_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    idempotency_key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/inbound-loads/{load_id}/arrivals"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn unloading_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    idempotency_key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/inbound-loads/{load_id}/unloading-starts"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn receipt_request(
    token: &str,
    tenant_id: TenantId,
    load_line_id: i64,
    idempotency_key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/expected-receiving/lines/{load_line_id}/confirmations"
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn closure_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    idempotency_key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/inbound-loads/{load_id}/closures"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn legacy_load_update_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    status: &str,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/loads/update")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "load_id": load_id,
                "status": status,
                "type": null,
                "reference_number": null,
                "invoice_number": null,
                "carrier": null,
                "trailer_number": null,
                "seal_number": null,
                "dock_door_location_id": null,
                "expected_time": null,
                "appointment_time": null,
                "actual_time": null,
                "arrival": null,
                "departure": null,
                "rejected": null,
                "closed": null
            })
            .to_string(),
        ))
        .unwrap()
}

fn entry_items_request(token: &str, tenant_id: TenantId, inventory_owner_id: i64) -> Request<Body> {
    Request::builder()
        .uri(format!(
            "/api/v1/inventory-owners/{inventory_owner_id}/inbound-load-entry-items?limit=100"
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

async fn json_body<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn receiving_dock(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: &str,
) -> i64 {
    wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(barcode),
        Some(barcode),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap()
}

async fn link_item(fixture: &Fixture, tenant_id: TenantId, inventory_owner_id: i64, item_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_items
            (tenant_id, created, inventory_owner_id, item_id)
        VALUES ($1,$2,$3,$4)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(inventory_owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let has_barcode: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM barcodes WHERE tenant_id=$1 AND item_id=$2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(item_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    if !has_barcode {
        repo::items::add_barcode(
            &fixture.db,
            tenant_id,
            item_id,
            &format!("ITEM-{item_id}"),
            "code128",
            None,
        )
        .await
        .unwrap();
    }
}

fn body(owner: i64, facility: i64, dock: i64, item: i64, reference: &str) -> Value {
    json!({
        "inventory_owner_id": owner,
        "facility_id": facility,
        "receiving_location_id": dock,
        "reference": reference,
        "invoice_number": "INV-100",
        "carrier": "Parcel Freight",
        "trailer_number": "TRL-100",
        "seal_number": "SEAL-100",
        "expected_at": "2027-08-11T17:00:00Z",
        "appointment_at": "2027-08-12T17:00:00Z",
        "lines": [
            {
                "item_id": item,
                "expected_quantity": 12,
                "lot": "LOT-A",
                "serial": null,
                "expiration": "2028-08-12T00:00:00Z"
            },
            {
                "item_id": item,
                "expected_quantity": 3,
                "lot": "LOT-B",
                "serial": null,
                "expiration": "2028-09-12T00:00:00Z"
            }
        ]
    })
}

#[derive(Debug, sqlx::FromRow)]
struct Effects {
    loads: i64,
    lines: i64,
    expected_quantity: i64,
    activities: i64,
    commands: i64,
    events: i64,
}

async fn effects(fixture: &Fixture, tenant_id: TenantId, reference: &str) -> Effects {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM loads WHERE reference_number=$2) AS loads,
          (SELECT COUNT(*) FROM load_lines line INNER JOIN loads load
             ON load.tenant_id=line.tenant_id AND load.id=line.load_id
             WHERE load.reference_number=$2) AS lines,
          (SELECT COALESCE(SUM(line.expected_qty),0)::BIGINT FROM load_lines line
             INNER JOIN loads load ON load.tenant_id=line.tenant_id AND load.id=line.load_id
             WHERE load.reference_number=$2) AS expected_quantity,
          (SELECT COUNT(*) FROM load_activity activity INNER JOIN loads load
             ON load.tenant_id=activity.tenant_id AND load.id=activity.load_id
             WHERE load.reference_number=$2) AS activities,
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE tenant_id=$1 AND operation='inbound.load.plan.v1') AS commands,
          (SELECT COUNT(*) FROM outbox_events
             WHERE tenant_id=$1 AND event_type='inbound.load.planned') AS events
        "#,
    )
    .bind(tenant_id.get())
    .bind(reference)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    effects
}

#[derive(Debug, sqlx::FromRow)]
struct ArrivalEffects {
    arrivals: i64,
    arrival_commands: i64,
    arrival_events: i64,
    arrived_activities: i64,
    status: String,
}

async fn arrival_effects(fixture: &Fixture, tenant_id: TenantId, load_id: i64) -> ArrivalEffects {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_arrivals WHERE load_id=$2) AS arrivals,
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.load.arrive.v1'
               AND (result_json->>'load_id')::BIGINT=$2) AS arrival_commands,
          (SELECT COUNT(*) FROM outbox_events
             WHERE event_type='inbound.load.arrived'
               AND aggregate_id=$2::TEXT) AS arrival_events,
          (SELECT COUNT(*) FROM load_activity
             WHERE load_id=$2 AND action='arrived') AS arrived_activities,
          (SELECT status FROM loads WHERE id=$2) AS status
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    effects
}

#[tokio::test]
async fn atomic_plan_replays_exactly_and_enters_expected_receiving() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("inbound-plan@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility = fixture.facility(tenant_id, "Inbound Plan DC").await;
    let owner = fixture
        .inventory_owner(tenant_id, "Inbound Plan Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner, facility)
        .await;
    let dock = receiving_dock(&fixture, tenant_id, facility, "PLAN-DOCK").await;
    let item = fixture.item(tenant_id, "Inbound Plan Item", "case").await;
    link_item(&fixture, tenant_id, owner, item).await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let request_body = body(owner, facility, dock, item, "ASN-ATOMIC-100");

    let entry_items = app
        .clone()
        .oneshot(entry_items_request(&token, tenant_id, owner))
        .await
        .unwrap();
    assert_eq!(entry_items.status(), StatusCode::OK);
    let entry_items: Vec<InboundLoadEntryItemResponse> = json_body(entry_items).await;
    assert_eq!(entry_items.len(), 1);
    assert_eq!(entry_items[0].item_id, item);
    assert_eq!(entry_items[0].uom, "case");

    let first = app
        .clone()
        .oneshot(plan_request(
            &token,
            tenant_id,
            "plan-atomic",
            &request_body,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let result: PlanInboundLoadResponse = json_body(first).await;
    assert_eq!(result.reference, "ASN-ATOMIC-100");
    assert_eq!(result.status, PlannedInboundLoadStatus::Planned);
    assert_eq!(result.lines.len(), 2);
    assert_eq!(result.total_expected_quantity, 15);
    assert!(result.execution_barcode.starts_with("WB-LOAD-"));

    let replay = app
        .clone()
        .oneshot(plan_request(
            &token,
            tenant_id,
            "plan-atomic",
            &request_body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body::<PlanInboundLoadResponse>(replay).await, result);

    let mut changed = request_body.clone();
    changed["lines"][0]["expected_quantity"] = json!(13);
    let conflict = app
        .clone()
        .oneshot(plan_request(&token, tenant_id, "plan-atomic", &changed))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(conflict).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let current = effects(&fixture, tenant_id, "ASN-ATOMIC-100").await;
    assert_eq!(current.loads, 1);
    assert_eq!(current.lines, 2);
    assert_eq!(current.expected_quantity, 15);
    assert_eq!(current.activities, 1);
    assert_eq!(current.commands, 1);
    assert_eq!(current.events, 1);

    let arrival_body = json!({
        "load_scan": result.execution_barcode,
        "receiving_location_scan": "PLAN-DOCK",
        "arrived_at": null
    });
    let arrival = app
        .clone()
        .oneshot(arrival_request(
            &token,
            tenant_id,
            result.load_id,
            "arrive-atomic",
            &arrival_body,
        ))
        .await
        .unwrap();
    assert_eq!(arrival.status(), StatusCode::OK);
    let arrival: ArriveInboundLoadResponse = json_body(arrival).await;
    assert_eq!(arrival.status, ArrivedInboundLoadStatus::Arrived);
    assert_eq!(arrival.receiving_location_id, dock);

    let replay = app
        .clone()
        .oneshot(arrival_request(
            &token,
            tenant_id,
            result.load_id,
            "arrive-atomic",
            &arrival_body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<ArriveInboundLoadResponse>(replay).await,
        arrival
    );

    let mut changed_arrival = arrival_body.clone();
    changed_arrival["receiving_location_scan"] = json!("OTHER-DOCK");
    let changed = app
        .clone()
        .oneshot(arrival_request(
            &token,
            tenant_id,
            result.load_id,
            "arrive-atomic",
            &changed_arrival,
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );
    let arrival_effects = arrival_effects(&fixture, tenant_id, result.load_id).await;
    assert_eq!(
        (
            arrival_effects.arrivals,
            arrival_effects.arrival_commands,
            arrival_effects.arrival_events,
            arrival_effects.arrived_activities,
            arrival_effects.status.as_str(),
        ),
        (1, 1, 1, 1, "arrived")
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let evidence = sqlx::query(
        r#"
        SELECT arrival.previous_status, arrival.observed_load_barcode,
               arrival.observed_receiving_location_barcode,
               event.aggregate_sequence, event.payload->>'status' AS event_status,
               event.payload->>'arrival_id' AS event_arrival_id
        FROM inbound_load_arrivals arrival
        INNER JOIN outbox_events event
          ON event.tenant_id=arrival.tenant_id
         AND event.event_type='inbound.load.arrived'
         AND event.aggregate_id=arrival.load_id::TEXT
        WHERE arrival.load_id=$1
        "#,
    )
    .bind(result.load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence.get::<String, _>("previous_status"), "planned");
    assert_eq!(
        evidence.get::<String, _>("observed_load_barcode"),
        arrival_body["load_scan"].as_str().unwrap()
    );
    assert_eq!(
        evidence.get::<String, _>("observed_receiving_location_barcode"),
        "PLAN-DOCK"
    );
    assert_eq!(evidence.get::<i64, _>("aggregate_sequence"), 2);
    assert_eq!(evidence.get::<String, _>("event_status"), "arrived");
    assert_eq!(
        evidence.get::<String, _>("event_arrival_id"),
        arrival.arrival_id.to_string()
    );
    tx.rollback().await.unwrap();

    let receipt_body = json!({
        "disposition": "received",
        "item_barcode": format!("ITEM-{item}"),
        "receiving_location_barcode": "PLAN-DOCK",
        "quantity": 1,
        "license_plate_barcode": null,
        "lot": "LOT-A",
        "serial": null,
        "expiration": "2028-08-12T00:00:00Z"
    });
    let premature_receipt = app
        .clone()
        .oneshot(receipt_request(
            &token,
            tenant_id,
            result.lines[0].load_line_id,
            "receipt-before-unloading",
            &receipt_body,
        ))
        .await
        .unwrap();
    assert_eq!(premature_receipt.status(), StatusCode::CONFLICT);

    let unloading_body = json!({
        "load_scan": arrival_body["load_scan"],
        "receiving_location_scan": "PLAN-DOCK",
        "seal_scan": "SEAL-100",
        "started_at": null
    });
    let started = app
        .clone()
        .oneshot(unloading_request(
            &token,
            tenant_id,
            result.load_id,
            "unloading-atomic",
            &unloading_body,
        ))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let started: StartInboundLoadUnloadingResponse = json_body(started).await;
    assert_eq!(started.load_id, result.load_id);
    assert_eq!(started.receiving_location_id, dock);
    let replay = app
        .clone()
        .oneshot(unloading_request(
            &token,
            tenant_id,
            result.load_id,
            "unloading-atomic",
            &unloading_body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<StartInboundLoadUnloadingResponse>(replay).await,
        started
    );
    let mut changed_unloading = unloading_body.clone();
    changed_unloading["seal_scan"] = json!("OTHER-SEAL");
    let changed = app
        .clone()
        .oneshot(unloading_request(
            &token,
            tenant_id,
            result.load_id,
            "unloading-atomic",
            &changed_unloading,
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let execution: (i64, i64, i64, String) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_unloading_starts WHERE load_id=$1),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.load.unloading.start.v1'
               AND (result_json->>'load_id')::BIGINT=$1),
          (SELECT aggregate_sequence FROM outbox_events
             WHERE event_type='inbound.load.unloading_started' AND aggregate_id=$1::TEXT),
          (SELECT status FROM loads WHERE id=$1)
        "#,
    )
    .bind(result.load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(execution, (1, 1, 3, "receiving".to_owned()));

    let receipt = app
        .clone()
        .oneshot(receipt_request(
            &token,
            tenant_id,
            result.lines[0].load_line_id,
            "receipt-after-unloading",
            &receipt_body,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.status(), StatusCode::OK);

    let session = app
        .clone()
        .oneshot(session_request(&token, tenant_id, result.load_id))
        .await
        .unwrap();
    assert_eq!(session.status(), StatusCode::OK);
    let session: ExpectedReceivingSessionResponse = json_body(session).await;
    assert_eq!(session.lines.len(), 2);
    assert_eq!(
        session
            .lines
            .iter()
            .map(|line| line.expected_quantity)
            .sum::<i64>(),
        15
    );
    assert_eq!(session.receiving_location.location_id, dock);

    let closure_body = json!({
        "load_scan": arrival_body["load_scan"],
        "receiving_location_scan": "PLAN-DOCK",
        "closed_at": null
    });
    let premature_close = app
        .clone()
        .oneshot(closure_request(
            &token,
            tenant_id,
            result.load_id,
            "close-before-receiving-complete",
            &closure_body,
        ))
        .await
        .unwrap();
    assert_eq!(premature_close.status(), StatusCode::CONFLICT);

    let remaining_first = json!({
        "disposition": "received",
        "item_barcode": format!("ITEM-{item}"),
        "receiving_location_barcode": "PLAN-DOCK",
        "quantity": 11,
        "license_plate_barcode": null,
        "lot": "LOT-A",
        "serial": null,
        "expiration": "2028-08-12T00:00:00Z"
    });
    let completed_first = app
        .clone()
        .oneshot(receipt_request(
            &token,
            tenant_id,
            result.lines[0].load_line_id,
            "receipt-complete-first-line",
            &remaining_first,
        ))
        .await
        .unwrap();
    assert_eq!(completed_first.status(), StatusCode::OK);
    let missing_second = app
        .clone()
        .oneshot(receipt_request(
            &token,
            tenant_id,
            result.lines[1].load_line_id,
            "receipt-resolve-second-line",
            &json!({
                "disposition": "missing",
                "quantity": 3,
                "reason": "short_shipment",
                "note": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(missing_second.status(), StatusCode::OK);

    let legacy_close = app
        .clone()
        .oneshot(legacy_load_update_request(
            &token,
            tenant_id,
            result.load_id,
            "closed",
        ))
        .await
        .unwrap();
    assert_eq!(legacy_close.status(), StatusCode::CONFLICT);

    let closed = app
        .clone()
        .oneshot(closure_request(
            &token,
            tenant_id,
            result.load_id,
            "close-atomic",
            &closure_body,
        ))
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    let closed: CloseInboundLoadResponse = json_body(closed).await;
    assert_eq!(closed.status, InboundLoadClosedStatus::Closed);
    assert_eq!(closed.receiving_location_id, dock);
    let replay = app
        .clone()
        .oneshot(closure_request(
            &token,
            tenant_id,
            result.load_id,
            "close-atomic",
            &closure_body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body::<CloseInboundLoadResponse>(replay).await, closed);
    let mut changed_closure = closure_body.clone();
    changed_closure["receiving_location_scan"] = json!("OTHER-DOCK");
    let changed = app
        .clone()
        .oneshot(closure_request(
            &token,
            tenant_id,
            result.load_id,
            "close-atomic",
            &changed_closure,
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let closure_effects: (i64, i64, i64, i64, String) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_closures WHERE load_id=$1),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.load.close.v1'
               AND (result_json->>'load_id')::BIGINT=$1),
          (SELECT COUNT(*) FROM load_activity WHERE load_id=$1 AND action='closed'),
          (SELECT aggregate_sequence FROM outbox_events
             WHERE event_type='inbound.load.closed' AND aggregate_id=$1::TEXT),
          (SELECT status FROM loads WHERE id=$1)
        "#,
    )
    .bind(result.load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(closure_effects.0, 1);
    assert_eq!(closure_effects.1, 1);
    assert_eq!(closure_effects.2, 1);
    assert!(closure_effects.3 > 3);
    assert_eq!(closure_effects.4, "closed");

    let terminal_session = app
        .oneshot(session_request(&token, tenant_id, result.load_id))
        .await
        .unwrap();
    assert_eq!(terminal_session.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn arrival_rejects_wrong_evidence_and_races_to_one_effect() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("inbound-arrival-race@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility = fixture.facility(tenant_id, "Inbound Arrival Race DC").await;
    let owner = fixture
        .inventory_owner(tenant_id, "Inbound Arrival Race Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner, facility)
        .await;
    let dock = receiving_dock(&fixture, tenant_id, facility, "ARRIVAL-RACE-DOCK").await;
    let item = fixture
        .item(tenant_id, "Inbound Arrival Race Item", "each")
        .await;
    link_item(&fixture, tenant_id, owner, item).await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let planned = app
        .clone()
        .oneshot(plan_request(
            &token,
            tenant_id,
            "arrival-race-plan",
            &body(owner, facility, dock, item, "ASN-ARRIVAL-RACE"),
        ))
        .await
        .unwrap();
    assert_eq!(planned.status(), StatusCode::OK);
    let planned: PlanInboundLoadResponse = json_body(planned).await;

    for (key, request_body) in [
        (
            "arrival-wrong-load",
            json!({
                "load_scan": "WRONG-LOAD",
                "receiving_location_scan": "ARRIVAL-RACE-DOCK",
                "arrived_at": null
            }),
        ),
        (
            "arrival-wrong-location",
            json!({
                "load_scan": planned.execution_barcode.clone(),
                "receiving_location_scan": "WRONG-DOCK",
                "arrived_at": null
            }),
        ),
        (
            "arrival-future",
            json!({
                "load_scan": planned.execution_barcode.clone(),
                "receiving_location_scan": "ARRIVAL-RACE-DOCK",
                "arrived_at": "2099-01-01T00:00:00Z"
            }),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(arrival_request(
                &token,
                tenant_id,
                planned.load_id,
                key,
                &request_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let before = arrival_effects(&fixture, tenant_id, planned.load_id).await;
    assert_eq!((before.arrivals, before.arrival_commands), (0, 0));
    assert_eq!(before.status, "planned");

    let arrival_body = json!({
        "load_scan": planned.execution_barcode,
        "receiving_location_scan": "ARRIVAL-RACE-DOCK",
        "arrived_at": null
    });
    let first = app.clone().oneshot(arrival_request(
        &token,
        tenant_id,
        planned.load_id,
        "arrival-race-a",
        &arrival_body,
    ));
    let second = app.clone().oneshot(arrival_request(
        &token,
        tenant_id,
        planned.load_id,
        "arrival-race-b",
        &arrival_body,
    ));
    let (first, second) = tokio::join!(first, second);
    let statuses = [first.unwrap().status(), second.unwrap().status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let after = arrival_effects(&fixture, tenant_id, planned.load_id).await;
    assert_eq!(
        (
            after.arrivals,
            after.arrival_commands,
            after.arrival_events,
            after.arrived_activities,
        ),
        (1, 1, 1, 1)
    );

    let wrong_seal = json!({
        "load_scan": planned.execution_barcode,
        "receiving_location_scan": "ARRIVAL-RACE-DOCK",
        "seal_scan": "WRONG-SEAL",
        "started_at": null
    });
    let rejected = app
        .clone()
        .oneshot(unloading_request(
            &token,
            tenant_id,
            planned.load_id,
            "unloading-wrong-seal",
            &wrong_seal,
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let unloading_body = json!({
        "load_scan": wrong_seal["load_scan"],
        "receiving_location_scan": "ARRIVAL-RACE-DOCK",
        "seal_scan": "SEAL-100",
        "started_at": null
    });
    let first = app.clone().oneshot(unloading_request(
        &token,
        tenant_id,
        planned.load_id,
        "unloading-race-a",
        &unloading_body,
    ));
    let second = app.clone().oneshot(unloading_request(
        &token,
        tenant_id,
        planned.load_id,
        "unloading-race-b",
        &unloading_body,
    ));
    let (first, second) = tokio::join!(first, second);
    let statuses = [first.unwrap().status(), second.unwrap().status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, String) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_unloading_starts WHERE load_id=$1),
          (SELECT COUNT(*) FROM outbox_events
             WHERE event_type='inbound.load.unloading_started' AND aggregate_id=$1::TEXT),
          (SELECT status FROM loads WHERE id=$1)
        "#,
    )
    .bind(planned.load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (1, 1, "receiving".to_owned()));
}

#[tokio::test]
async fn reference_race_has_one_winner_and_invalid_plans_have_zero_effects() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("inbound-plan-race@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility = fixture.facility(tenant_id, "Inbound Race DC").await;
    let owner = fixture
        .inventory_owner(tenant_id, "Inbound Race Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner, facility)
        .await;
    let dock = receiving_dock(&fixture, tenant_id, facility, "RACE-DOCK").await;
    let item = fixture.item(tenant_id, "Inbound Race Item", "each").await;
    link_item(&fixture, tenant_id, owner, item).await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let request_body = body(owner, facility, dock, item, "ASN-RACE-100");
    let first = app.clone().oneshot(plan_request(
        &token,
        tenant_id,
        "plan-race-a",
        &request_body,
    ));
    let second = app.clone().oneshot(plan_request(
        &token,
        tenant_id,
        "plan-race-b",
        &request_body,
    ));
    let (first, second) = tokio::join!(first, second);
    let statuses = [first.unwrap().status(), second.unwrap().status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let current = effects(&fixture, tenant_id, "ASN-RACE-100").await;
    assert_eq!(
        (
            current.loads,
            current.lines,
            current.commands,
            current.events
        ),
        (1, 2, 1, 1)
    );

    let mut invalid = body(owner, facility, dock, item, "ASN-INVALID-100");
    invalid["receiving_location_id"] = json!(9_999_999);
    let response = app
        .oneshot(plan_request(&token, tenant_id, "plan-invalid", &invalid))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let invalid_effects = effects(&fixture, tenant_id, "ASN-INVALID-100").await;
    assert_eq!((invalid_effects.loads, invalid_effects.lines), (0, 0));
}

#[tokio::test]
async fn planning_enforces_current_owner_and_facility_scope() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("inbound-plan-scope@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let allowed_facility = fixture.facility(tenant_id, "Allowed Inbound DC").await;
    let denied_facility = fixture.facility(tenant_id, "Denied Inbound DC").await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Allowed Inbound Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Denied Inbound Owner")
        .await;
    for (owner, facility) in [
        (allowed_owner, allowed_facility),
        (allowed_owner, denied_facility),
        (denied_owner, allowed_facility),
    ] {
        fixture
            .assign_owner_to_facility(tenant_id, owner, facility)
            .await;
    }
    let allowed_dock = receiving_dock(&fixture, tenant_id, allowed_facility, "ALLOWED-DOCK").await;
    let denied_dock = receiving_dock(&fixture, tenant_id, denied_facility, "DENIED-DOCK").await;
    let item = fixture.item(tenant_id, "Scoped Inbound Item", "case").await;
    link_item(&fixture, tenant_id, allowed_owner, item).await;
    link_item(&fixture, tenant_id, denied_owner, item).await;
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap());
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    for (key, request_body) in [
        (
            "scope-denied-facility",
            body(
                allowed_owner,
                denied_facility,
                denied_dock,
                item,
                "ASN-DENIED-FACILITY",
            ),
        ),
        (
            "scope-denied-owner",
            body(
                denied_owner,
                allowed_facility,
                allowed_dock,
                item,
                "ASN-DENIED-OWNER",
            ),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(plan_request(&token, tenant_id, key, &request_body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    assert_eq!(
        effects(&fixture, tenant_id, "ASN-DENIED-FACILITY")
            .await
            .loads,
        0
    );
    assert_eq!(
        effects(&fixture, tenant_id, "ASN-DENIED-OWNER").await.loads,
        0
    );

    let allowed = body(
        allowed_owner,
        allowed_facility,
        allowed_dock,
        item,
        "ASN-SCOPE-REPLAY",
    );
    let response = app
        .clone()
        .oneshot(plan_request(&token, tenant_id, "scope-replay", &allowed))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    let mut changed = allowed.clone();
    changed["carrier"] = json!("Changed Carrier");
    for request_body in [allowed, changed] {
        let response = app
            .clone()
            .oneshot(plan_request(
                &token,
                tenant_id,
                "scope-replay",
                &request_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body::<ErrorResponse>(response).await.reason,
            ErrorReason::NotFound
        );
    }
    assert_eq!(
        effects(&fixture, tenant_id, "ASN-SCOPE-REPLAY").await.loads,
        1
    );
}

#[tokio::test]
async fn arrival_replays_are_concealed_after_scope_revocation() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("inbound-arrival-scope@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility = fixture.facility(tenant_id, "Arrival Scoped DC").await;
    let owner = fixture
        .inventory_owner(tenant_id, "Arrival Scoped Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner, facility)
        .await;
    let dock = receiving_dock(&fixture, tenant_id, facility, "ARRIVAL-SCOPE-DOCK").await;
    let item = fixture.item(tenant_id, "Arrival Scoped Item", "case").await;
    link_item(&fixture, tenant_id, owner, item).await;
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![owner],
        },
    )
    .await
    .unwrap());
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let planned = app
        .clone()
        .oneshot(plan_request(
            &token,
            tenant_id,
            "arrival-scope-plan",
            &body(owner, facility, dock, item, "ASN-ARRIVAL-SCOPE"),
        ))
        .await
        .unwrap();
    assert_eq!(planned.status(), StatusCode::OK);
    let planned: PlanInboundLoadResponse = json_body(planned).await;
    let arrival_body = json!({
        "load_scan": planned.execution_barcode,
        "receiving_location_scan": "ARRIVAL-SCOPE-DOCK",
        "arrived_at": null
    });
    let arrived = app
        .clone()
        .oneshot(arrival_request(
            &token,
            tenant_id,
            planned.load_id,
            "arrival-scope",
            &arrival_body,
        ))
        .await
        .unwrap();
    assert_eq!(arrived.status(), StatusCode::OK);
    let unloading_body = json!({
        "load_scan": arrival_body["load_scan"],
        "receiving_location_scan": "ARRIVAL-SCOPE-DOCK",
        "seal_scan": "SEAL-100",
        "started_at": null
    });
    let unloading = app
        .clone()
        .oneshot(unloading_request(
            &token,
            tenant_id,
            planned.load_id,
            "unloading-scope",
            &unloading_body,
        ))
        .await
        .unwrap();
    assert_eq!(unloading.status(), StatusCode::OK);

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    let mut changed = arrival_body.clone();
    changed["receiving_location_scan"] = json!("CHANGED-DOCK");
    for request_body in [arrival_body, changed] {
        let response = app
            .clone()
            .oneshot(arrival_request(
                &token,
                tenant_id,
                planned.load_id,
                "arrival-scope",
                &request_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body::<ErrorResponse>(response).await.reason,
            ErrorReason::NotFound
        );
    }
    let mut changed_unloading = unloading_body.clone();
    changed_unloading["seal_scan"] = json!("CHANGED-SEAL");
    for request_body in [unloading_body, changed_unloading] {
        let response = app
            .clone()
            .oneshot(unloading_request(
                &token,
                tenant_id,
                planned.load_id,
                "unloading-scope",
                &request_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body::<ErrorResponse>(response).await.reason,
            ErrorReason::NotFound
        );
    }
    let effects = arrival_effects(&fixture, tenant_id, planned.load_id).await;
    assert_eq!((effects.arrivals, effects.arrival_commands), (1, 1));
}
