mod common;
#[path = "api_v1_outbound_loads/concurrency.rs"]
mod concurrency;
#[path = "api_v1_outbound_loads/recovery.rs"]
mod recovery;
#[allow(dead_code)]
#[path = "api_v1_shipping/support.rs"]
mod support;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    CloseCartonResponse, CompleteOutboundLoadLoadingResponse, ConfirmOutboundLoadDepartureResponse,
    CreateCartonResponse, CreateShipmentResponse, LoadOutboundCartonRequest,
    MovePackedCartonResponse, OpenPackSessionResponse, OutboundLoadQueuePage, OutboundLoadResponse,
    PackPickedAllocationResponse, PackedCartonPositionResponse, PackedCartonPositionStateResponse,
    PickClaimResponse, PickContentConfirmationResponse, PlanOutboundLoadResponse,
    RecordManualManifestResponse, ReleaseOutboundLoadResponse, StartOutboundLoadLoadingResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

use support::*;

fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

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
        Some(value) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
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
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
    operation: &str,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body: Value = response_json(response).await;
        panic!("{operation}: expected {expected}, got {actual}: {body}");
    }
    response
}

async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    permission_name: &str,
    role_name: &str,
) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Outbound load test role"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user_id,
        role,
    )
    .await
    .unwrap());
}

async fn set_scope(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    facility_ids: Vec<i64>,
    inventory_owner_ids: Vec<i64>,
) {
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
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

async fn execution_location(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: &str,
    kind: &str,
) -> i64 {
    wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(barcode),
        Some(barcode),
        kind,
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
    sqlx::query("UPDATE license_plates SET location_id=$1 WHERE tenant_id=$2 AND id=$3")
        .bind(location_id)
        .bind(tenant_id.get())
        .bind(plate_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[allow(clippy::too_many_arguments)]
async fn prepare_manifested_shipment(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    packing_location_id: i64,
    key: &str,
) -> (
    ReadyShipment,
    CreateShipmentResponse,
    RecordManualManifestResponse,
) {
    plate_at(
        fixture,
        access.tenant_id,
        inventory_owner_id,
        facility_id,
        packing_location_id,
        &format!("{key}-TOTE"),
    )
    .await;
    let ready = prepare_ready_shipment(
        fixture,
        app,
        token,
        access,
        inventory_owner_id,
        facility_id,
        packing_location_id,
        key,
    )
    .await;
    let created: CreateShipmentResponse = response_json(
        expect_status(
            send(
                app,
                token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/orders/{}/shipments", ready.order_id),
                Some(&format!("{key}-shipment")),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create shipment",
        )
        .await,
    )
    .await;
    let manifested: RecordManualManifestResponse = response_json(
        expect_status(
            send(
                app,
                token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/shipments/{}/manifests",
                    created.shipment.shipment_id
                ),
                Some(&format!("{key}-manifest")),
                Some(manifest_body(&ready, &format!("{key}-MANIFEST"), 1)),
            )
            .await,
            StatusCode::OK,
            "manifest shipment",
        )
        .await,
    )
    .await;
    (ready, created, manifested)
}

#[tokio::test]
async fn manifested_cartons_move_through_load_and_depart_atomically() {
    init_test_tracing();
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("outbound-load-flow@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "orders",
        "outbound-load-orders",
    )
    .await;
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "wms_supervisor",
        "outbound-load-supervisor",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Outbound Load Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Outbound Load Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let packing_barcode = "OUTBOUND-PACK";
    let staging_barcode = "OUTBOUND-STAGE";
    let dock_barcode = "OUTBOUND-DOCK";
    let packing_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        packing_barcode,
        "packing",
    )
    .await;
    let staging_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        staging_barcode,
        "staging",
    )
    .await;
    execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        dock_barcode,
        "dock",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "outbound-load-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let (ready, created, manifested) = prepare_manifested_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        packing_id,
        "OUTBOUND-LOAD",
    )
    .await;
    let shipment_id = created.shipment.shipment_id;
    let plan_body = json!({
        "facility_id": facility_id,
        "load_reference": "LOAD-100",
        "carrier_code": "UPS",
        "staging_location_id": staging_id,
        "shipments": [{
            "shipment_id": shipment_id,
            "expected_shipment_revision": manifested.revision,
            "expected_order_revision": created.order_revision,
            "shipment_sequence": 1,
            "cartons": [
                {"carton_id": ready.carton_ids[0], "load_sequence": 1},
                {"carton_id": ready.carton_ids[1], "load_sequence": 2}
            ]
        }]
    });
    let planned: PlanOutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/outbound-loads",
                Some("outbound-plan"),
                Some(plan_body.clone()),
            )
            .await,
            StatusCode::OK,
            "plan outbound load",
        )
        .await,
    )
    .await;
    let load_id = planned.outbound_load.outbound_load_id;
    assert_eq!(
        planned.outbound_load.status,
        wareboxes_api_contract::v1::OutboundLoadStatus::Planned
    );
    assert_eq!(planned.outbound_load.cartons.len(), 2);
    let looked_up: OutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/outbound-loads/by-barcode/{}",
                    planned.outbound_load.load_barcode
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "look up outbound load by execution barcode",
        )
        .await,
    )
    .await;
    assert_eq!(looked_up, planned.outbound_load);
    expect_status(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::GET,
            &format!(
                "/api/v1/outbound-loads/by-barcode/{}",
                planned.outbound_load.load_barcode.to_ascii_lowercase()
            ),
            None,
            None,
        )
        .await,
        StatusCode::NOT_FOUND,
        "execution barcode lookup is exact",
    )
    .await;
    let replay: PlanOutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/outbound-loads",
                Some("outbound-plan"),
                Some(plan_body),
            )
            .await,
            StatusCode::OK,
            "replay outbound plan",
        )
        .await,
    )
    .await;
    assert_eq!(replay, planned);
    let direct_departure = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/shipments/{shipment_id}/departures"),
        Some("outbound-direct-departure-blocked"),
        Some(json!({
            "expected_shipment_revision": manifested.revision,
            "expected_order_revision": created.order_revision,
            "scanned_carton_barcodes": ready.carton_barcodes
        })),
    )
    .await;
    assert_eq!(direct_departure.status(), StatusCode::CONFLICT);

    let released: ReleaseOutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/outbound-loads/{load_id}/releases"),
                Some("outbound-release"),
                Some(json!({"expected_revision": 1})),
            )
            .await,
            StatusCode::OK,
            "release outbound load",
        )
        .await,
    )
    .await;
    assert_eq!(released.revision.get(), 2);

    let mut position_revision = Vec::new();
    for (index, carton_id) in ready.carton_ids.iter().copied().enumerate() {
        let staged: MovePackedCartonResponse = response_json(
            expect_status(
                send(
                    &app,
                    &token,
                    access.tenant_id,
                    Method::POST,
                    &format!(
                        "/api/v1/outbound-loads/{load_id}/cartons/{carton_id}/staging-movements"
                    ),
                    Some(&format!("outbound-stage-{index}")),
                    Some(json!({
                        "expected_load_revision": 2, "expected_position_revision": 1,
                        "source_location_barcode": packing_barcode,
                        "carton_barcode": ready.carton_barcodes[index],
                        "staging_location_barcode": staging_barcode
                    })),
                )
                .await,
                StatusCode::OK,
                "stage outbound carton",
            )
            .await,
        )
        .await;
        assert_eq!(staged.movement.quantity, if index == 0 { 3 } else { 2 });
        assert_eq!(staged.load_revision.get(), 2);
        position_revision.push(staged.position.revision.get());
    }
    let started: StartOutboundLoadLoadingResponse = response_json(
        expect_status(
            send(&app, &token, access.tenant_id, Method::POST, &format!("/api/v1/outbound-loads/{load_id}/loading-starts"), Some("outbound-start"), Some(json!({
                "expected_revision": 2, "load_barcode": "OUTBOUND:LOAD-100",
                "staging_location_barcode": staging_barcode, "dock_location_barcode": dock_barcode,
                "trailer_number": "TRAILER-100"
            }))).await,
            StatusCode::OK, "start outbound loading").await,
    ).await;
    assert_eq!(started.revision.get(), 3);

    let out_of_sequence = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/outbound-loads/{load_id}/cartons/{}/loading-movements",
            ready.carton_ids[1]
        ),
        Some("outbound-load-wrong-sequence"),
        Some(json!({
            "expected_load_revision": 3, "expected_position_revision": position_revision[1],
            "staging_location_barcode": staging_barcode, "carton_barcode": ready.carton_barcodes[1],
            "trailer_number": "TRAILER-100"
        })),
    )
    .await;
    assert_eq!(out_of_sequence.status(), StatusCode::CONFLICT);

    for (index, carton_id) in ready.carton_ids.iter().copied().enumerate() {
        let loaded: MovePackedCartonResponse = response_json(
            expect_status(
                send(
                    &app,
                    &token,
                    access.tenant_id,
                    Method::POST,
                    &format!(
                        "/api/v1/outbound-loads/{load_id}/cartons/{carton_id}/loading-movements"
                    ),
                    Some(&format!("outbound-load-{index}")),
                    Some(
                        serde_json::to_value(LoadOutboundCartonRequest {
                            expected_load_revision: wareboxes_api_contract::v1::Revision::new(3)
                                .unwrap(),
                            expected_position_revision: wareboxes_api_contract::v1::Revision::new(
                                position_revision[index],
                            )
                            .unwrap(),
                            staging_location_barcode: staging_barcode.into(),
                            carton_barcode: ready.carton_barcodes[index].clone(),
                            trailer_number: "TRAILER-100".into(),
                        })
                        .unwrap(),
                    ),
                )
                .await,
                StatusCode::OK,
                "load outbound carton",
            )
            .await,
        )
        .await;
        assert!(matches!(
            loaded.position.state,
            PackedCartonPositionStateResponse::Loaded { .. }
        ));
        position_revision[index] = loaded.position.revision.get();
    }
    let unload_body = json!({
        "expected_load_revision": 3,
        "expected_position_revision": position_revision[1],
        "trailer_number": "TRAILER-100",
        "carton_barcode": ready.carton_barcodes[1],
        "staging_location_barcode": staging_barcode
    });
    let unloaded: MovePackedCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/outbound-loads/{load_id}/cartons/{}/unloading-movements",
                    ready.carton_ids[1]
                ),
                Some("outbound-unload-1"),
                Some(unload_body.clone()),
            )
            .await,
            StatusCode::OK,
            "unload outbound carton",
        )
        .await,
    )
    .await;
    assert_eq!(unloaded.load_revision.get(), 3);
    assert!(matches!(
        unloaded.position.state,
        PackedCartonPositionStateResponse::Staged { .. }
    ));
    let unload_replay: MovePackedCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/outbound-loads/{load_id}/cartons/{}/unloading-movements",
                    ready.carton_ids[1]
                ),
                Some("outbound-unload-1"),
                Some(unload_body),
            )
            .await,
            StatusCode::OK,
            "replay outbound carton unload",
        )
        .await,
    )
    .await;
    assert_eq!(unload_replay, unloaded);
    position_revision[1] = unloaded.position.revision.get();
    let reloaded: MovePackedCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/outbound-loads/{load_id}/cartons/{}/loading-movements",
                    ready.carton_ids[1]
                ),
                Some("outbound-reload-1"),
                Some(json!({
                    "expected_load_revision": 3,
                    "expected_position_revision": position_revision[1],
                    "staging_location_barcode": staging_barcode,
                    "carton_barcode": ready.carton_barcodes[1],
                    "trailer_number": "TRAILER-100"
                })),
            )
            .await,
            StatusCode::OK,
            "reload outbound carton",
        )
        .await,
    )
    .await;
    assert_eq!(reloaded.load_revision.get(), 3);
    position_revision[1] = reloaded.position.revision.get();
    let completed: CompleteOutboundLoadLoadingResponse = response_json(
        expect_status(
            send(&app, &token, access.tenant_id, Method::POST, &format!("/api/v1/outbound-loads/{load_id}/loading-completions"), Some("outbound-complete"), Some(json!({
                "expected_revision": 3, "load_barcode": "OUTBOUND:LOAD-100", "dock_location_barcode": dock_barcode,
                "trailer_number": "TRAILER-100", "seal_number": "SEAL-100"
            }))).await,
            StatusCode::OK, "complete outbound loading").await,
    ).await;
    assert_eq!(completed.revision.get(), 4);

    let departed: ConfirmOutboundLoadDepartureResponse = response_json(
        expect_status(
            send(&app, &token, access.tenant_id, Method::POST, &format!("/api/v1/outbound-loads/{load_id}/departures"), Some("outbound-depart"), Some(json!({
                "expected_revision": 4, "load_barcode": "OUTBOUND:LOAD-100", "dock_location_barcode": dock_barcode,
                "trailer_number": "TRAILER-100", "seal_number": "SEAL-100"
            }))).await,
            StatusCode::OK, "depart outbound load").await,
    ).await;
    assert_eq!(departed.revision.get(), 5);
    assert_eq!(departed.shipment_departures.len(), 1);
    assert_eq!(departed.shipment_departures[0].demand.shipped_quantity, 5);

    for carton_id in &ready.carton_ids {
        let position: PackedCartonPositionResponse = response_json(
            expect_status(
                send(
                    &app,
                    &token,
                    access.tenant_id,
                    Method::GET,
                    &format!("/api/v1/packed-cartons/{carton_id}/position"),
                    None,
                    None,
                )
                .await,
                StatusCode::OK,
                "read departed packed position",
            )
            .await,
        )
        .await;
        assert!(
            matches!(position.state, PackedCartonPositionStateResponse::Departed { outbound_load_id: Some(id), .. } if id == load_id)
        );
        assert!(position
            .contents
            .iter()
            .all(|content| content.current_inventory_allocation_id.is_none()
                && content.current_inventory_balance_id.is_none()));
    }
    let queue: OutboundLoadQueuePage = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                "/api/v1/outbound-loads",
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "list active outbound loads",
        )
        .await,
    )
    .await;
    assert!(queue
        .items
        .iter()
        .all(|entry| entry.outbound_load_id != load_id));

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let (ship_transactions, move_transactions, active_allocations, active_balances): (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id=$1 AND operation='shipping.shipment.departure.confirm.v1'),
          (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id=$1 AND operation LIKE 'outbound.load.carton.%.v1'),
          (SELECT COUNT(*) FROM inventory_allocations WHERE tenant_id=$1 AND reservation_id IN (SELECT id FROM inventory_reservations WHERE tenant_id=$1 AND order_id=$2) AND status='allocated' AND deleted IS NULL),
          (SELECT COUNT(*) FROM inventory_balances WHERE tenant_id=$1 AND license_plate_id IN (SELECT license_plate_id FROM shipment_cartons WHERE tenant_id=$1 AND shipment_id=$3) AND deleted IS NULL AND qty_on_hand>0)
        "#,
    ).bind(access.tenant_id.get()).bind(ready.order_id).bind(shipment_id).fetch_one(&mut *tx).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(ship_transactions, 1);
    assert_eq!(move_transactions, 6);
    assert_eq!(active_allocations, 0);
    assert_eq!(active_balances, 0);
}

#[tokio::test]
async fn multi_owner_load_departure_is_atomic_and_owner_journals_stay_separate() {
    init_test_tracing();
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("outbound-load-multi@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "orders",
        "outbound-load-multi-orders",
    )
    .await;
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "wms_supervisor",
        "outbound-load-multi-supervisor",
    )
    .await;
    let owner_a = fixture
        .inventory_owner(access.tenant_id, "Outbound Load Owner A")
        .await;
    let owner_b = fixture
        .inventory_owner(access.tenant_id, "Outbound Load Owner B")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Outbound Load Multi Facility")
        .await;
    for owner_id in [owner_a, owner_b] {
        fixture
            .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
            .await;
    }
    let packing_barcode = "OUTBOUND-MULTI-PACK";
    let staging_barcode = "OUTBOUND-MULTI-STAGE";
    let dock_barcode = "OUTBOUND-MULTI-DOCK";
    let packing_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        packing_barcode,
        "packing",
    )
    .await;
    let staging_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        staging_barcode,
        "staging",
    )
    .await;
    execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        dock_barcode,
        "dock",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "outbound-load-multi-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let (ready_a, created_a, manifested_a) = prepare_manifested_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_a,
        facility_id,
        packing_id,
        "OUTBOUND-MULTI-A",
    )
    .await;
    let (ready_b, created_b, manifested_b) = prepare_manifested_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_b,
        facility_id,
        packing_id,
        "OUTBOUND-MULTI-B",
    )
    .await;
    let plan_body = json!({
        "facility_id": facility_id,
        "load_reference": "LOAD-MULTI-200",
        "carrier_code": "UPS",
        "staging_location_id": staging_id,
        "shipments": [
            {
                "shipment_id": created_a.shipment.shipment_id,
                "expected_shipment_revision": manifested_a.revision,
                "expected_order_revision": created_a.order_revision,
                "shipment_sequence": 1,
                "cartons": [
                    {"carton_id": ready_a.carton_ids[0], "load_sequence": 1},
                    {"carton_id": ready_a.carton_ids[1], "load_sequence": 2}
                ]
            },
            {
                "shipment_id": created_b.shipment.shipment_id,
                "expected_shipment_revision": manifested_b.revision,
                "expected_order_revision": created_b.order_revision,
                "shipment_sequence": 2,
                "cartons": [
                    {"carton_id": ready_b.carton_ids[0], "load_sequence": 3},
                    {"carton_id": ready_b.carton_ids[1], "load_sequence": 4}
                ]
            }
        ]
    });
    let planned: PlanOutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/outbound-loads",
                Some("outbound-multi-plan"),
                Some(plan_body),
            )
            .await,
            StatusCode::OK,
            "plan multi-owner outbound load",
        )
        .await,
    )
    .await;
    let load_id = planned.outbound_load.outbound_load_id;
    assert_eq!(planned.outbound_load.shipments.len(), 2);
    expect_status(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/outbound-loads/{load_id}/releases"),
            Some("outbound-multi-release"),
            Some(json!({"expected_revision": 1})),
        )
        .await,
        StatusCode::OK,
        "release multi-owner outbound load",
    )
    .await;

    let cartons = ready_a
        .carton_ids
        .iter()
        .copied()
        .zip(ready_a.carton_barcodes.iter().cloned())
        .chain(
            ready_b
                .carton_ids
                .iter()
                .copied()
                .zip(ready_b.carton_barcodes.iter().cloned()),
        )
        .collect::<Vec<_>>();
    let mut revisions = Vec::with_capacity(cartons.len());
    for (index, (carton_id, carton_barcode)) in cartons.iter().enumerate() {
        let staged: MovePackedCartonResponse = response_json(
            expect_status(
                send(
                    &app,
                    &token,
                    access.tenant_id,
                    Method::POST,
                    &format!(
                        "/api/v1/outbound-loads/{load_id}/cartons/{carton_id}/staging-movements"
                    ),
                    Some(&format!("outbound-multi-stage-{index}")),
                    Some(json!({
                        "expected_load_revision": 2,
                        "expected_position_revision": 1,
                        "source_location_barcode": packing_barcode,
                        "carton_barcode": carton_barcode,
                        "staging_location_barcode": staging_barcode
                    })),
                )
                .await,
                StatusCode::OK,
                "stage multi-owner outbound carton",
            )
            .await,
        )
        .await;
        revisions.push(staged.position.revision.get());
    }
    expect_status(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/outbound-loads/{load_id}/loading-starts"),
            Some("outbound-multi-start"),
            Some(json!({
                "expected_revision": 2,
                "load_barcode": "OUTBOUND:LOAD-MULTI-200",
                "staging_location_barcode": staging_barcode,
                "dock_location_barcode": dock_barcode,
                "trailer_number": "TRAILER-MULTI-200"
            })),
        )
        .await,
        StatusCode::OK,
        "start multi-owner outbound loading",
    )
    .await;
    for (index, (carton_id, carton_barcode)) in cartons.iter().enumerate() {
        let loaded: MovePackedCartonResponse = response_json(
            expect_status(
                send(
                    &app,
                    &token,
                    access.tenant_id,
                    Method::POST,
                    &format!(
                        "/api/v1/outbound-loads/{load_id}/cartons/{carton_id}/loading-movements"
                    ),
                    Some(&format!("outbound-multi-load-{index}")),
                    Some(json!({
                        "expected_load_revision": 3,
                        "expected_position_revision": revisions[index],
                        "staging_location_barcode": staging_barcode,
                        "carton_barcode": carton_barcode,
                        "trailer_number": "TRAILER-MULTI-200"
                    })),
                )
                .await,
                StatusCode::OK,
                "load multi-owner outbound carton",
            )
            .await,
        )
        .await;
        revisions[index] = loaded.position.revision.get();
    }
    expect_status(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/outbound-loads/{load_id}/loading-completions"),
            Some("outbound-multi-complete"),
            Some(json!({
                "expected_revision": 3,
                "load_barcode": "OUTBOUND:LOAD-MULTI-200",
                "dock_location_barcode": dock_barcode,
                "trailer_number": "TRAILER-MULTI-200",
                "seal_number": "SEAL-MULTI-200"
            })),
        )
        .await,
        StatusCode::OK,
        "complete multi-owner outbound loading",
    )
    .await;
    let departure_body = json!({
        "expected_revision": 4,
        "load_barcode": "OUTBOUND:LOAD-MULTI-200",
        "dock_location_barcode": dock_barcode,
        "trailer_number": "TRAILER-MULTI-200",
        "seal_number": "SEAL-MULTI-200"
    });
    let departed: ConfirmOutboundLoadDepartureResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/outbound-loads/{load_id}/departures"),
                Some("outbound-multi-depart"),
                Some(departure_body.clone()),
            )
            .await,
            StatusCode::OK,
            "depart multi-owner outbound load",
        )
        .await,
    )
    .await;
    assert_eq!(departed.shipment_departures.len(), 2);
    assert_eq!(
        departed
            .shipment_departures
            .iter()
            .map(|shipment| shipment.inventory_owner_id)
            .collect::<std::collections::BTreeSet<_>>(),
        [owner_a, owner_b].into_iter().collect()
    );
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT inventory_owner_id,COUNT(*)
        FROM inventory_transactions
        WHERE tenant_id=$1 AND operation='shipping.shipment.departure.confirm.v1'
          AND reference_id=ANY($2)
        GROUP BY inventory_owner_id ORDER BY inventory_owner_id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind([
        created_a.shipment.shipment_id,
        created_b.shipment.shipment_id,
    ])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rows, vec![(owner_a, 1), (owner_b, 1)]);

    set_scope(
        &fixture,
        access.tenant_id,
        operator.id,
        vec![facility_id],
        vec![owner_a],
    )
    .await;
    let concealed = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!("/api/v1/outbound-loads/{load_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    let concealed_barcode = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/outbound-loads/by-barcode/{}",
            planned.outbound_load.load_barcode
        ),
        None,
        None,
    )
    .await;
    assert_eq!(concealed_barcode.status(), StatusCode::NOT_FOUND);
    let departed_queue: OutboundLoadQueuePage = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                "/api/v1/outbound-loads?status=departed",
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "list concealed departed loads",
        )
        .await,
    )
    .await;
    assert!(departed_queue
        .items
        .iter()
        .all(|entry| entry.outbound_load_id != load_id));
    let concealed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/outbound-loads/{load_id}/departures"),
        Some("outbound-multi-depart"),
        Some(departure_body),
    )
    .await;
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);
}
