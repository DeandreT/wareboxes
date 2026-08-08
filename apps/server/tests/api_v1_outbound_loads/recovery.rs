use super::*;

use wareboxes_api_contract::v1::{
    CancelOutboundLoadResponse, ConfirmShipmentDepartureResponse, OutboundLoadResponse,
};

#[tokio::test]
async fn staged_carton_must_be_restored_before_replay_safe_cancellation() {
    init_test_tracing();
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("outbound-load-recovery@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "orders",
        "outbound-load-recovery-orders",
    )
    .await;
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "wms_supervisor",
        "outbound-load-recovery-supervisor",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Outbound Load Recovery Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Outbound Load Recovery Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let packing_barcode = "OUTBOUND-RECOVERY-PACK";
    let staging_barcode = "OUTBOUND-RECOVERY-STAGE";
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
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "outbound-load-recovery-origin",
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
        "OUTBOUND-RECOVERY",
    )
    .await;
    let planned: PlanOutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/outbound-loads",
                Some("outbound-recovery-plan"),
                Some(json!({
                    "facility_id": facility_id,
                    "load_reference": "LOAD-RECOVERY-300",
                    "carrier_code": "UPS",
                    "staging_location_id": staging_id,
                    "shipments": [{
                        "shipment_id": created.shipment.shipment_id,
                        "expected_shipment_revision": manifested.revision,
                        "expected_order_revision": created.order_revision,
                        "shipment_sequence": 1,
                        "cartons": [
                            {"carton_id": ready.carton_ids[0], "load_sequence": 1},
                            {"carton_id": ready.carton_ids[1], "load_sequence": 2}
                        ]
                    }]
                })),
            )
            .await,
            StatusCode::OK,
            "plan recovery outbound load",
        )
        .await,
    )
    .await;
    let load_id = planned.outbound_load.outbound_load_id;
    expect_status(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/outbound-loads/{load_id}/releases"),
            Some("outbound-recovery-release"),
            Some(json!({"expected_revision": 1})),
        )
        .await,
        StatusCode::OK,
        "release recovery outbound load",
    )
    .await;
    let staged: MovePackedCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/outbound-loads/{load_id}/cartons/{}/staging-movements",
                    ready.carton_ids[0]
                ),
                Some("outbound-recovery-stage"),
                Some(json!({
                    "expected_load_revision": 2,
                    "expected_position_revision": 1,
                    "source_location_barcode": packing_barcode,
                    "carton_barcode": ready.carton_barcodes[0],
                    "staging_location_barcode": staging_barcode
                })),
            )
            .await,
            StatusCode::OK,
            "stage recovery carton",
        )
        .await,
    )
    .await;
    let cancellation_body = json!({
        "expected_revision": 2,
        "reason": "planning_error",
        "note": "return staged carton"
    });
    let premature = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/outbound-loads/{load_id}/cancellations"),
        Some("outbound-recovery-cancel-premature"),
        Some(cancellation_body.clone()),
    )
    .await;
    assert_eq!(premature.status(), StatusCode::CONFLICT);

    let unstaged: MovePackedCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/outbound-loads/{load_id}/cartons/{}/unstaging-movements",
                    ready.carton_ids[0]
                ),
                Some("outbound-recovery-unstage"),
                Some(json!({
                    "expected_load_revision": 2,
                    "expected_position_revision": staged.position.revision,
                    "staging_location_barcode": staging_barcode,
                    "carton_barcode": ready.carton_barcodes[0],
                    "return_location_barcode": packing_barcode
                })),
            )
            .await,
            StatusCode::OK,
            "unstage recovery carton",
        )
        .await,
    )
    .await;
    assert!(matches!(
        unstaged.position.state,
        PackedCartonPositionStateResponse::Packed { location_id } if location_id == packing_id
    ));
    let cancelled: CancelOutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/outbound-loads/{load_id}/cancellations"),
                Some("outbound-recovery-cancel"),
                Some(cancellation_body.clone()),
            )
            .await,
            StatusCode::OK,
            "cancel restored outbound load",
        )
        .await,
    )
    .await;
    assert_eq!(cancelled.revision.get(), 3);
    let replay: CancelOutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/outbound-loads/{load_id}/cancellations"),
                Some("outbound-recovery-cancel"),
                Some(cancellation_body),
            )
            .await,
            StatusCode::OK,
            "replay outbound load cancellation",
        )
        .await,
    )
    .await;
    assert_eq!(replay, cancelled);
    let load: OutboundLoadResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &format!("/api/v1/outbound-loads/{load_id}"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "read cancelled outbound load",
        )
        .await,
    )
    .await;
    assert_eq!(
        load.status,
        wareboxes_api_contract::v1::OutboundLoadStatus::Cancelled
    );

    let departed: ConfirmShipmentDepartureResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/shipments/{}/departures",
                    created.shipment.shipment_id
                ),
                Some("outbound-recovery-direct-depart"),
                Some(json!({
                    "expected_shipment_revision": manifested.revision,
                    "expected_order_revision": created.order_revision,
                    "scanned_carton_barcodes": ready.carton_barcodes
                })),
            )
            .await,
            StatusCode::OK,
            "depart shipment after load cancellation",
        )
        .await,
    )
    .await;
    assert_eq!(departed.scanned_carton_count, 2);
}
