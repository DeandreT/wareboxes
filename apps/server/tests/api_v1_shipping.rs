mod common;
#[path = "api_v1_shipping/documents.rs"]
mod documents;
#[path = "api_v1_shipping/partial_departure.rs"]
mod partial_departure;
#[path = "api_v1_shipping/queue.rs"]
mod queue;
#[path = "api_v1_shipping/support.rs"]
mod support;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CloseCartonResponse, ConfirmShipmentDepartureResponse, CreateCartonResponse,
    CreateShipmentResponse, ErrorReason, ErrorResponse, GenerateCartonLabelSetResponse,
    OpenPackSessionResponse, PackPickedAllocationResponse, PickClaimResponse,
    PickContentConfirmationResponse, RecordManualManifestResponse, ShipmentResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

use support::*;

fn request(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = idempotency_key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(
            token,
            tenant_id,
            method,
            path,
            idempotency_key,
            body,
        ))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
    operation: &str,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body = response_json::<Value>(response).await;
        panic!("{operation}: expected {expected}, got {actual}: {body}");
    }
    response
}

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64, role_name: &str) {
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
                Some("Fulfillment orders"),
            )
            .await
            .unwrap(),
        };
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        role_name,
        Some("Shipping execution"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db, tenant_id, role, permission
    )
    .await
    .unwrap());
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
            .await
            .unwrap()
    );
}

async fn set_scope(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    facility_ids: Vec<i64>,
    inventory_owner_ids: Vec<i64>,
) {
    assert!(repo::tenants::update_user_access_scope(
        db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id,
            all_facilities: false,
            facility_ids,
            all_inventory_owners: false,
            inventory_owner_ids,
        },
    )
    .await
    .unwrap());
}

async fn add_wms_operator(
    fixture: &Fixture,
    tenant_id: TenantId,
    email: &str,
    role_name: &str,
) -> wareboxes_core::models::User {
    let user = fixture.user(email).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let permission =
        wareboxes_persistence_postgres::permissions::find_by_name(&fixture.db, tenant_id, "wms")
            .await
            .unwrap()
            .expect("tenant has a WMS permission");
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Shipping operator"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission.id,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user.id,
        role,
    )
    .await
    .unwrap());
    user
}

async fn execution_location(
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
        "packing",
        true,
        false,
        false,
    )
    .await
    .unwrap()
}

async fn plate_at(
    fixture: &Fixture,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    barcode: &str,
) {
    let plate_id = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        facility_id,
        Some(barcode),
    )
    .await
    .unwrap();
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE license_plates SET location_id = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(location_id)
        .bind(tenant_id.get())
        .bind(plate_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
async fn full_order_shipping_is_exact_replay_safe_and_reconciles_inventory() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("shipping-flow@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "shipping-flow-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Shipping Flow Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Shipping Flow Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "SHIP-FLOW-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "SHIP-FLOW-TOTE",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "SHIP-FLOW",
    )
    .await;
    let create_path = format!("/api/v1/orders/{}/shipments", ready.order_id);

    let missing_origin = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-flow-missing-origin"),
        Some(create_shipment_body(&ready)),
    )
    .await;
    assert_eq!(missing_origin.status(), StatusCode::CONFLICT);
    assert_eq!(
        shipping_effect_counts(&fixture, access.tenant_id, ready.order_id).await,
        (0, 0, 0, 0)
    );

    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "ship-flow-incomplete",
        false,
    )
    .await;
    let incomplete_origin = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-flow-incomplete-origin"),
        Some(create_shipment_body(&ready)),
    )
    .await;
    assert_eq!(incomplete_origin.status(), StatusCode::CONFLICT);
    assert_eq!(
        shipping_effect_counts(&fixture, access.tenant_id, ready.order_id).await,
        (0, 0, 0, 0)
    );
    let origin_address_id = set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "ship-flow-origin",
        true,
    )
    .await;

    let admin = admin_db_for(&fixture.db).await;
    let destination_address_id: i64 =
        sqlx::query_scalar("SELECT address_id FROM orders WHERE tenant_id = $1 AND id = $2")
            .bind(access.tenant_id.get())
            .bind(ready.order_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    sqlx::query("UPDATE addresses SET postal_code = NULL WHERE tenant_id = $1 AND id = $2")
        .bind(access.tenant_id.get())
        .bind(destination_address_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    let incomplete_destination = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-flow-incomplete-destination"),
        Some(create_shipment_body(&ready)),
    )
    .await;
    assert_eq!(incomplete_destination.status(), StatusCode::CONFLICT);
    assert_eq!(
        shipping_effect_counts(&fixture, access.tenant_id, ready.order_id).await,
        (0, 0, 0, 0)
    );
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE addresses SET postal_code = '89501' WHERE tenant_id = $1 AND id = $2")
        .bind(access.tenant_id.get())
        .bind(destination_address_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let create_body = create_shipment_body(&ready);
    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-flow-create-a"),
        Some(create_body.clone()),
    );
    let second = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-flow-create-b"),
        Some(create_body.clone()),
    );
    let (first, second) = tokio::join!(first, second);
    let (winner, loser, replay_key) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second, "ship-flow-create-a"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first, "ship-flow-create-b"),
        statuses => panic!("expected one shipment winner and one conflict, got {statuses:?}"),
    };
    drop(loser);
    let created: CreateShipmentResponse = response_json(winner).await;
    assert_eq!(created.shipment.revision.get(), 1);
    assert_eq!(created.order_revision.get(), 12);
    assert_eq!(created.shipment.cartons.len(), 2);
    assert_eq!(
        created
            .shipment
            .cartons
            .iter()
            .map(|carton| carton.carton_id)
            .collect::<Vec<_>>(),
        ready.carton_ids
    );
    let shipment_id = created.shipment.shipment_id;
    assert_eq!(
        shipping_effect_counts(&fixture, access.tenant_id, ready.order_id).await,
        (1, 2, 2, 1)
    );
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some(replay_key),
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<CreateShipmentResponse>(
            expect_status(replay, StatusCode::OK, "replay shipment creation").await
        )
        .await,
        created
    );
    let mut changed_create_body = create_body;
    changed_create_body["expected_revision"] = json!(10);
    let changed_create = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some(replay_key),
        Some(changed_create_body),
    )
    .await;
    assert_eq!(changed_create.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_create).await.reason,
        ErrorReason::IdempotencyKeyReused
    );
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let creation_events: Vec<(String, String, String, Value)> = sqlx::query_as(
        r#"
        SELECT event_key, aggregate_id, ordering_key, payload
        FROM outbox_events
        WHERE tenant_id = $1 AND aggregate_type = 'order' AND aggregate_id = $2
          AND event_type = 'shipping.shipment_created'
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(ready.order_id.to_string())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(creation_events.len(), 1);
    assert_eq!(
        creation_events[0].0,
        format!("shipment:{shipment_id}:created")
    );
    assert_eq!(creation_events[0].1, ready.order_id.to_string());
    assert_eq!(creation_events[0].2, format!("order:{}", ready.order_id));
    assert_eq!(creation_events[0].3["shipment_id"], shipment_id);
    assert_eq!(creation_events[0].3["order_id"], ready.order_id);
    let shipment_path = format!("/api/v1/shipments/{shipment_id}");
    let resumed = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &shipment_path,
        None,
        None,
    )
    .await;
    assert_eq!(
        response_json::<ShipmentResponse>(
            expect_status(resumed, StatusCode::OK, "resume created shipment").await
        )
        .await,
        created.shipment
    );
    assert_shipment_snapshots(
        &fixture,
        access.tenant_id,
        shipment_id,
        destination_address_id,
        origin_address_id,
        &ready,
    )
    .await;

    let manifest_path = format!("{shipment_path}/manifests");
    let incomplete_manifest = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &manifest_path,
        Some("ship-flow-inexact-manifest"),
        Some(json!({
            "carrier_code": "UPS",
            "service_code": "GROUND",
            "manifest_reference": "SHIP-FLOW-INEXACT",
            "carton_tracking_assignments": [{
                "carton_id": ready.carton_ids[0],
                "tracking_number": format!("TRACK-{}-1", ready.order_id)
            }],
            "expected_revision": 1
        })),
    )
    .await;
    assert_eq!(incomplete_manifest.status(), StatusCode::BAD_REQUEST);
    let manifest_request = manifest_body(&ready, "SHIP-FLOW-MANIFEST", 1);
    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &manifest_path,
        Some("ship-flow-manifest-a"),
        Some(manifest_request.clone()),
    );
    let second = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &manifest_path,
        Some("ship-flow-manifest-b"),
        Some(manifest_request.clone()),
    );
    let (first, second) = tokio::join!(first, second);
    let (winner, loser, replay_key) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second, "ship-flow-manifest-a"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first, "ship-flow-manifest-b"),
        statuses => panic!("expected one manifest winner and one conflict, got {statuses:?}"),
    };
    drop(loser);
    let manifested: RecordManualManifestResponse = response_json(winner).await;
    assert_eq!(manifested.revision.get(), 2);
    assert_eq!(manifested.manifest.carton_tracking_assignments.len(), 2);
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &manifest_path,
        Some(replay_key),
        Some(manifest_request.clone()),
    )
    .await;
    assert_eq!(
        response_json::<RecordManualManifestResponse>(
            expect_status(replay, StatusCode::OK, "replay manifest").await
        )
        .await,
        manifested
    );
    let changed_manifest = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &manifest_path,
        Some(replay_key),
        Some(manifest_body(&ready, "SHIP-FLOW-CHANGED", 1)),
    )
    .await;
    assert_eq!(changed_manifest.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_manifest)
            .await
            .reason,
        ErrorReason::IdempotencyKeyReused
    );

    let departure_path = format!("{shipment_path}/departures");
    for (key, scans) in [
        ("empty", vec![]),
        (
            "duplicate",
            vec![
                ready.carton_barcodes[0].clone(),
                ready.carton_barcodes[0].clone(),
            ],
        ),
    ] {
        let rejected = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &departure_path,
            Some(&format!("ship-flow-depart-{key}")),
            Some(json!({
                "scanned_carton_barcodes": scans,
                "expected_shipment_revision": 2,
                "expected_order_revision": 12
            })),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST, "{key}");
    }
    let departure_body = json!({
        "scanned_carton_barcodes": ready.carton_barcodes,
        "expected_shipment_revision": 2,
        "expected_order_revision": 12
    });
    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &departure_path,
        Some("ship-flow-depart-a"),
        Some(departure_body.clone()),
    );
    let second = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &departure_path,
        Some("ship-flow-depart-b"),
        Some(departure_body.clone()),
    );
    let (first, second) = tokio::join!(first, second);
    let (winner, loser, replay_key) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second, "ship-flow-depart-a"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first, "ship-flow-depart-b"),
        statuses => panic!("expected one departure winner and one conflict, got {statuses:?}"),
    };
    drop(loser);
    let departed: ConfirmShipmentDepartureResponse = response_json(winner).await;
    assert_eq!(departed.shipment_revision.get(), 3);
    assert_eq!(departed.order_revision.get(), 13);
    assert_eq!(departed.scanned_carton_count, 2);
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &departure_path,
        Some(replay_key),
        Some(departure_body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<ConfirmShipmentDepartureResponse>(
            expect_status(replay, StatusCode::OK, "replay shipment departure").await
        )
        .await,
        departed
    );
    let mut changed_departure = departure_body;
    changed_departure["scanned_carton_barcodes"] = json!([ready.carton_barcodes[0]]);
    let changed_departure = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &departure_path,
        Some(replay_key),
        Some(changed_departure),
    )
    .await;
    assert_eq!(changed_departure.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_departure)
            .await
            .reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let reconciled = sqlx::query(
        r#"
        SELECT shipment.state AS shipment_state, shipment.revision AS shipment_revision,
               order_header.status AS order_status, order_header.revision AS order_revision,
               transaction.transaction_type, transaction.operation,
               (SELECT COUNT(*) FROM inventory_entries entry
                 WHERE entry.tenant_id = shipment.tenant_id
                   AND entry.transaction_id = confirmation.inventory_transaction_id) AS entry_count,
               (SELECT COALESCE(SUM(entry.quantity_delta), 0)::bigint
                  FROM inventory_entries entry
                 WHERE entry.tenant_id = shipment.tenant_id
                   AND entry.transaction_id = confirmation.inventory_transaction_id) AS journal_qty,
               (SELECT COUNT(*) FROM inventory_balances balance
                 INNER JOIN carton_contents content
                   ON content.tenant_id = balance.tenant_id
                  AND content.destination_inventory_balance_id = balance.id
                 WHERE content.tenant_id = shipment.tenant_id
                   AND content.packing_session_id = shipment.packing_session_id
                   AND (balance.qty_on_hand <> 0 OR balance.qty_reserved <> 0
                        OR balance.deleted IS NULL)) AS bad_balances,
               (SELECT COUNT(*) FROM inventory_allocations allocation
                 INNER JOIN inventory_reservations reservation
                   ON reservation.tenant_id = allocation.tenant_id
                  AND reservation.id = allocation.reservation_id
                 WHERE reservation.tenant_id = shipment.tenant_id
                   AND reservation.order_id = shipment.order_id
                   AND (allocation.status <> 'fulfilled' OR allocation.deleted IS NULL)) AS bad_allocations,
               (SELECT COUNT(*) FROM inventory_reservations reservation
                 WHERE reservation.tenant_id = shipment.tenant_id
                   AND reservation.order_id = shipment.order_id
                   AND (reservation.status <> 'fulfilled' OR reservation.deleted IS NULL)) AS bad_reservations,
               (SELECT COUNT(*) FROM shipment_cartons carton
                 INNER JOIN license_plates plate
                   ON plate.tenant_id = carton.tenant_id AND plate.id = carton.license_plate_id
                 WHERE carton.tenant_id = shipment.tenant_id
                   AND carton.shipment_id = shipment.id
                   AND (plate.location_id IS NOT NULL OR plate.deleted IS NULL)) AS bad_plates
        FROM shipments shipment
        INNER JOIN orders order_header
          ON order_header.tenant_id = shipment.tenant_id AND order_header.id = shipment.order_id
        INNER JOIN shipment_confirmations confirmation
          ON confirmation.tenant_id = shipment.tenant_id
         AND confirmation.shipment_id = shipment.id
        INNER JOIN inventory_transactions transaction
          ON transaction.tenant_id = confirmation.tenant_id
         AND transaction.id = confirmation.inventory_transaction_id
        WHERE shipment.tenant_id = $1 AND shipment.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        reconciled.try_get::<String, _>("shipment_state").unwrap(),
        "departed"
    );
    assert_eq!(
        reconciled.try_get::<i64, _>("shipment_revision").unwrap(),
        3
    );
    assert_eq!(
        reconciled.try_get::<String, _>("order_status").unwrap(),
        "shipped"
    );
    assert_eq!(reconciled.try_get::<i64, _>("order_revision").unwrap(), 13);
    assert_eq!(
        reconciled.try_get::<String, _>("transaction_type").unwrap(),
        "ship"
    );
    assert_eq!(
        reconciled.try_get::<String, _>("operation").unwrap(),
        "shipping.shipment.departure.confirm.v1"
    );
    assert_eq!(reconciled.try_get::<i64, _>("entry_count").unwrap(), 2);
    assert_eq!(reconciled.try_get::<i64, _>("journal_qty").unwrap(), -5);
    for column in [
        "bad_balances",
        "bad_allocations",
        "bad_reservations",
        "bad_plates",
    ] {
        assert_eq!(reconciled.try_get::<i64, _>(column).unwrap(), 0, "{column}");
    }

    let admin = admin_db_for(&fixture.db).await;
    for (operation, statement) in [
        (
            "shipment identity update",
            "UPDATE shipments SET carton_count = carton_count + 1 WHERE tenant_id = $1 AND id = $2",
        ),
        (
            "shipment delete",
            "DELETE FROM shipments WHERE tenant_id = $1 AND id = $2",
        ),
        (
            "address snapshot update",
            "UPDATE shipment_address_snapshots SET line1 = line1 WHERE tenant_id = $1 AND shipment_id = $2",
        ),
        (
            "carton snapshot delete",
            "DELETE FROM shipment_cartons WHERE tenant_id = $1 AND shipment_id = $2",
        ),
        (
            "manifest update",
            "UPDATE shipment_manifests SET carrier = carrier WHERE tenant_id = $1 AND shipment_id = $2",
        ),
        (
            "manifest package delete",
            "DELETE FROM shipment_manifest_packages WHERE tenant_id = $1 AND shipment_id = $2",
        ),
        (
            "confirmation update",
            "UPDATE shipment_confirmations SET shipped_qty = shipped_qty WHERE tenant_id = $1 AND shipment_id = $2",
        ),
        (
            "confirmation carton delete",
            "DELETE FROM shipment_confirmation_cartons WHERE tenant_id = $1 AND shipment_id = $2",
        ),
    ] {
        let result = sqlx::query(statement)
            .bind(access.tenant_id.get())
            .bind(shipment_id)
            .execute(&admin)
            .await;
        assert!(result.is_err(), "{operation} must be rejected");
    }
    admin.close().await;
}

#[tokio::test]
async fn shipping_reads_and_replays_fail_closed_across_scopes_and_tenants() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("shipping-scope@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "shipping-scope-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Shipping Scope Owner")
        .await;
    let other_owner_id = fixture
        .inventory_owner(access.tenant_id, "Shipping Other Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Shipping Scope Facility")
        .await;
    let other_facility_id = fixture
        .facility(access.tenant_id, "Shipping Other Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "SHIP-SCOPE-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "SHIP-SCOPE-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "ship-scope-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "SHIP-SCOPE",
    )
    .await;
    let create_path = format!("/api/v1/orders/{}/shipments", ready.order_id);
    let create_body = create_shipment_body(&ready);
    let created = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-scope-create"),
        Some(create_body.clone()),
    )
    .await;
    let created: CreateShipmentResponse =
        response_json(expect_status(created, StatusCode::OK, "create scoped shipment").await).await;
    let shipment_path = format!("/api/v1/shipments/{}", created.shipment.shipment_id);
    let manifest_path = format!("{shipment_path}/manifests");
    let scope_manifest_body = manifest_body(&ready, "SHIP-SCOPE-MANIFEST", 1);
    let manifested = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &manifest_path,
        Some("ship-scope-manifest"),
        Some(scope_manifest_body.clone()),
    )
    .await;
    let manifested: RecordManualManifestResponse =
        response_json(expect_status(manifested, StatusCode::OK, "manifest scoped shipment").await)
            .await;

    let restricted = add_wms_operator(
        &fixture,
        access.tenant_id,
        "shipping-restricted@test.local",
        "shipping-restricted",
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        restricted.id,
        vec![other_facility_id],
        vec![other_owner_id],
    )
    .await;
    let restricted_token = auth::create_session(&fixture.db, restricted.id)
        .await
        .unwrap();
    let concealed = send(
        &app,
        &restricted_token,
        access.tenant_id,
        Method::GET,
        &shipment_path,
        None,
        None,
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let visible_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-scope-create"),
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<CreateShipmentResponse>(
            expect_status(visible_replay, StatusCode::OK, "visible create replay").await
        )
        .await,
        created
    );
    let visible_manifest_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &manifest_path,
        Some("ship-scope-manifest"),
        Some(scope_manifest_body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<RecordManualManifestResponse>(
            expect_status(
                visible_manifest_replay,
                StatusCode::OK,
                "visible manifest replay",
            )
            .await,
        )
        .await,
        manifested
    );
    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        vec![other_facility_id],
        vec![other_owner_id],
    )
    .await;
    let revoked_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-scope-create"),
        Some(create_body),
    )
    .await;
    assert_eq!(revoked_replay.status(), StatusCode::NOT_FOUND);
    let revoked_manifest_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &manifest_path,
        Some("ship-scope-manifest"),
        Some(scope_manifest_body.clone()),
    )
    .await;
    assert_eq!(revoked_manifest_replay.status(), StatusCode::NOT_FOUND);

    let foreign = fixture.wms_user("shipping-foreign@test.local").await;
    let foreign_access = default_tenant_for_user(&fixture.db, foreign.id)
        .await
        .unwrap();
    let foreign_token = auth::create_session(&fixture.db, foreign.id).await.unwrap();
    let foreign_read = send(
        &app,
        &foreign_token,
        foreign_access.tenant_id,
        Method::GET,
        &shipment_path,
        None,
        None,
    )
    .await;
    assert_eq!(foreign_read.status(), StatusCode::NOT_FOUND);
    let foreign_manifest_replay = send(
        &app,
        &foreign_token,
        foreign_access.tenant_id,
        Method::POST,
        &manifest_path,
        Some("ship-scope-manifest"),
        Some(scope_manifest_body),
    )
    .await;
    assert_eq!(foreign_manifest_replay.status(), StatusCode::NOT_FOUND);
    let wrong_membership = send(
        &app,
        &foreign_token,
        access.tenant_id,
        Method::GET,
        &shipment_path,
        None,
        None,
    )
    .await;
    assert_eq!(wrong_membership.status(), StatusCode::FORBIDDEN);

    let mut foreign_tx = tenant_tx(&fixture.db, foreign_access.tenant_id).await;
    for table in [
        "shipments",
        "shipment_address_snapshots",
        "shipment_cartons",
        "shipment_manifests",
        "shipment_manifest_packages",
        "shipment_confirmations",
        "shipment_confirmation_cartons",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&mut *foreign_tx)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} leaked across tenants");
    }
    foreign_tx.rollback().await.unwrap();
}

#[tokio::test]
async fn shipping_ledgers_are_forced_rls_and_minimally_granted() {
    let fixture = Fixture::new().await;
    let admin = admin_db_for(&fixture.db).await;
    for (table, can_update) in [
        ("shipments", true),
        ("shipment_address_snapshots", false),
        ("shipment_cartons", false),
        ("shipment_manifests", false),
        ("shipment_manifest_packages", false),
        ("shipment_confirmations", false),
        ("shipment_confirmation_cartons", false),
    ] {
        let privileges: (bool, bool, bool, bool) = sqlx::query_as(
            r#"
            SELECT has_table_privilege('wareboxes_app', $1, 'SELECT'),
                   has_table_privilege('wareboxes_app', $1, 'INSERT'),
                   has_table_privilege('wareboxes_app', $1, 'UPDATE'),
                   has_table_privilege('wareboxes_app', $1, 'DELETE')
            "#,
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(privileges, (true, true, can_update, false), "{table}");
        let rls: (bool, bool) = sqlx::query_as(
            "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = $1::regclass",
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(rls, (true, true), "{table}");
    }
    for sequence in [
        "shipments_id_seq",
        "shipment_address_snapshots_id_seq",
        "shipment_cartons_id_seq",
        "shipment_manifests_id_seq",
        "shipment_manifest_packages_id_seq",
        "shipment_confirmations_id_seq",
        "shipment_confirmation_cartons_id_seq",
    ] {
        let privileges: (bool, bool, bool) = sqlx::query_as(
            r#"
            SELECT has_sequence_privilege('wareboxes_app', $1, 'USAGE'),
                   has_sequence_privilege('wareboxes_app', $1, 'SELECT'),
                   has_sequence_privilege('wareboxes_app', $1, 'UPDATE')
            "#,
        )
        .bind(sequence)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(privileges, (true, false, false), "{sequence}");
    }
    admin.close().await;
}
