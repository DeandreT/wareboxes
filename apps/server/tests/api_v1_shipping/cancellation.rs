use super::*;
use wareboxes_api_contract::v1::{
    CancelShipmentResponse, CloseCartonResponse, ReopenCartonResponse, ShipmentStatus,
    ShippingQueuePage,
};

#[tokio::test]
async fn predeparture_shipment_cancellation_replays_recovers_and_permits_new_attempts() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("shipment-cancel@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "shipment-cancel-orders",
    )
    .await;
    outbound_qa::grant_supervisor(&fixture, access.tenant_id, operator.id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Shipment Cancellation Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Shipment Cancellation Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "SHIP-CANCEL-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "SHIP-CANCEL-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "ship-cancel-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let mut ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "SHIP-CANCEL",
    )
    .await;

    let create_path = format!("/api/v1/orders/{}/shipments", ready.order_id);
    let first: CreateShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &create_path,
                Some("ship-cancel-create-1"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create shipment attempt one",
        )
        .await,
    )
    .await;
    assert_eq!(first.shipment.attempt, 1);
    assert_eq!(first.shipment.status, ShipmentStatus::AwaitingManifest);
    assert_eq!(first.order_revision.get(), ready.order_revision + 1);

    let cancel_path = format!(
        "/api/v1/shipments/{}/cancellations",
        first.shipment.shipment_id
    );
    let cancel_body = json!({
        "expected_shipment_revision": first.shipment.revision,
        "expected_order_revision": first.order_revision,
        "reason": "packing_correction",
        "note": "Carton closure requires correction"
    });
    let regular_operator = add_wms_operator(
        &fixture,
        access.tenant_id,
        "shipment-cancel-operator@test.local",
        "shipment-cancel-operator",
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        regular_operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let regular_token = auth::create_session(&fixture.db, regular_operator.id)
        .await
        .unwrap();
    assert_eq!(
        send(
            &app,
            &regular_token,
            access.tenant_id,
            Method::POST,
            &cancel_path,
            Some("ship-cancel-no-supervisor"),
            Some(cancel_body.clone()),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let one = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cancel_path,
        Some("ship-cancel-a"),
        Some(cancel_body.clone()),
    );
    let two = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cancel_path,
        Some("ship-cancel-b"),
        Some(cancel_body.clone()),
    );
    let (one, two) = tokio::join!(one, two);
    let (winner, replay_key) = match (one.status(), two.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (one, "ship-cancel-a"),
        (StatusCode::CONFLICT, StatusCode::OK) => (two, "ship-cancel-b"),
        statuses => panic!("expected one cancellation winner, got {statuses:?}"),
    };
    let cancelled: CancelShipmentResponse = response_json(winner).await;
    assert_eq!(cancelled.shipment.status, ShipmentStatus::Cancelled);
    assert_eq!(cancelled.shipment.attempt, 1);
    assert_eq!(cancelled.shipment.order_revision, first.order_revision);
    assert_eq!(cancelled.packing_session_revision, first.order_revision);
    assert_eq!(
        cancelled
            .shipment
            .cancellation
            .as_ref()
            .expect("cancellation evidence")
            .previous_status,
        ShipmentStatus::AwaitingManifest
    );
    assert_eq!(
        cancelled
            .shipment
            .cancellation
            .as_ref()
            .expect("cancellation evidence")
            .note
            .as_deref(),
        Some("Carton closure requires correction")
    );

    let replayed: CancelShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &cancel_path,
                Some(replay_key),
                Some(cancel_body.clone()),
            )
            .await,
            StatusCode::OK,
            "replay shipment cancellation",
        )
        .await,
    )
    .await;
    assert_eq!(replayed, cancelled);
    let mut changed = cancel_body;
    changed["reason"] = json!("operator_error");
    assert_eq!(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &cancel_path,
            Some(replay_key),
            Some(changed),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    for body in [
        json!({
            "expected_shipment_revision": first.shipment.revision,
            "expected_order_revision": first.order_revision,
            "reason": "packing_correction",
            "note": "Carton closure requires correction"
        }),
        json!({
            "expected_shipment_revision": first.shipment.revision,
            "expected_order_revision": first.order_revision,
            "reason": "operator_error"
        }),
    ] {
        assert_eq!(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &cancel_path,
                Some(replay_key),
                Some(body),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;

    assert_eq!(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!(
                "/api/v1/shipments/{}/documents/packing-slips",
                first.shipment.shipment_id
            ),
            Some("ship-cancel-packing-slip"),
            Some(json!({
                "expected_shipment_revision": cancelled.shipment.revision
            })),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let queue: ShippingQueuePage = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &format!("/api/v1/shipping-queue?facility_id={facility_id}"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "queue after shipment cancellation",
        )
        .await,
    )
    .await;
    assert!(queue
        .items
        .iter()
        .find(|entry| entry.order_id == ready.order_id)
        .is_some_and(|entry| entry.shipment.is_none()));

    let reopened: ReopenCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/packing-sessions/{}/cartons/{}/reopenings",
                    ready.packing_session_id, ready.carton_ids[0]
                ),
                Some("ship-cancel-reopen"),
                Some(json!({
                    "carton_barcode": ready.carton_barcodes[0],
                    "expected_revision": cancelled.packing_session_revision,
                    "reason": "packing_correction",
                    "note": "Correct carton after cancelling the shipment"
                })),
            )
            .await,
            StatusCode::OK,
            "reopen carton after shipment cancellation",
        )
        .await,
    )
    .await;
    let reclosed: CloseCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/packing-sessions/{}/cartons/{}/closures",
                    ready.packing_session_id, ready.carton_ids[0]
                ),
                Some("ship-cancel-reclose"),
                Some(json!({
                    "carton_barcode": ready.carton_barcodes[0],
                    "measurements": {
                        "weight_grams": 1350,
                        "dimensions": {"length_mm": 310,"width_mm": 210,"height_mm": 160}
                    },
                    "expected_revision": reopened.revision
                })),
            )
            .await,
            StatusCode::OK,
            "reclose carton after shipment cancellation",
        )
        .await,
    )
    .await;
    ready.order_revision = reclosed.revision.get();
    let second: CreateShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &create_path,
                Some("ship-cancel-create-2"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create replacement shipment",
        )
        .await,
    )
    .await;
    assert_eq!(second.shipment.attempt, 2);
    assert_ne!(second.shipment.shipment_id, first.shipment.shipment_id);

    let manifested: RecordManualManifestResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/shipments/{}/manifests",
                    second.shipment.shipment_id
                ),
                Some("ship-cancel-manifest-2"),
                Some(manifest_body(&ready, "SHIP-CANCEL-MANIFEST", 1)),
            )
            .await,
            StatusCode::OK,
            "manifest replacement shipment",
        )
        .await,
    )
    .await;
    let manifested_cancelled: CancelShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/shipments/{}/cancellations",
                    second.shipment.shipment_id
                ),
                Some("ship-cancel-manifested"),
                Some(json!({
                    "expected_shipment_revision": manifested.revision,
                    "expected_order_revision": second.order_revision,
                    "reason": "shipping_data_correction",
                    "note": "Carrier data requires a new manifested attempt"
                })),
            )
            .await,
            StatusCode::OK,
            "cancel manifested shipment before departure",
        )
        .await,
    )
    .await;
    assert_eq!(manifested_cancelled.shipment.revision.get(), 3);
    assert_eq!(
        manifested_cancelled.shipment.manifest,
        Some(manifested.manifest)
    );
    assert!(manifested_cancelled
        .shipment
        .cartons
        .iter()
        .all(|carton| carton.tracking_number.is_some()));
    assert_eq!(
        manifested_cancelled
            .shipment
            .cancellation
            .as_ref()
            .expect("manifested cancellation evidence")
            .previous_status,
        ShipmentStatus::Manifested
    );

    ready.order_revision = manifested_cancelled.packing_session_revision.get();
    let third: CreateShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &create_path,
                Some("ship-cancel-create-3"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create third shipment attempt",
        )
        .await,
    )
    .await;
    assert_eq!(third.shipment.attempt, 3);
    let third_manifest: RecordManualManifestResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/shipments/{}/manifests", third.shipment.shipment_id),
                Some("ship-cancel-manifest-3"),
                Some(json!({
                    "carrier_code": "UPS",
                    "service_code": "GROUND",
                    "manifest_reference": "SHIP-CANCEL-MANIFEST-3",
                    "carton_tracking_assignments": [
                        {"carton_id": ready.carton_ids[0], "tracking_number": format!("TRACK-{}-3-1", ready.order_id)},
                        {"carton_id": ready.carton_ids[1], "tracking_number": format!("TRACK-{}-3-2", ready.order_id)}
                    ],
                    "expected_revision": 1
                })),
            )
            .await,
            StatusCode::OK,
            "manifest third shipment attempt",
        )
        .await,
    )
    .await;
    let partial: ConfirmShipmentDepartureResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/shipments/{}/departures",
                    third.shipment.shipment_id
                ),
                Some("ship-cancel-partial-departure"),
                Some(json!({
                    "scanned_carton_barcodes": [ready.carton_barcodes[0]],
                    "expected_shipment_revision": third_manifest.revision,
                    "expected_order_revision": third.order_revision
                })),
            )
            .await,
            StatusCode::OK,
            "partially depart third shipment attempt",
        )
        .await,
    )
    .await;
    assert_eq!(partial.shipment_status, ShipmentStatus::PartiallyDeparted);
    assert_eq!(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!(
                "/api/v1/shipments/{}/cancellations",
                third.shipment.shipment_id
            ),
            Some("ship-cancel-too-late"),
            Some(json!({
                "expected_shipment_revision": partial.shipment_revision,
                "expected_order_revision": partial.order_revision,
                "reason": "operator_error"
            })),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let admin = admin_db_for(&fixture.db).await;
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM shipments WHERE tenant_id=$1 AND order_id=$2),
               (SELECT COUNT(*) FROM shipment_cancellations WHERE tenant_id=$1 AND order_id=$2),
               (SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1 AND event_type='shipping.shipment_cancelled'
                    AND payload->>'order_id'=$2::text),
               (SELECT COUNT(*) FROM shipment_cancellations WHERE tenant_id=$1 AND order_id=$2
                    AND previous_shipment_state='awaiting manifest' AND carrier_manifest_id IS NULL),
               (SELECT COUNT(*) FROM shipment_cancellations WHERE tenant_id=$1 AND order_id=$2
                    AND previous_shipment_state='manifested' AND carrier_manifest_id IS NOT NULL)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(ready.order_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(counts, (3, 2, 2, 1, 1));
    assert!(
        sqlx::query("UPDATE shipment_cancellations SET note='forged' WHERE tenant_id=$1")
            .bind(access.tenant_id.get())
            .execute(&admin)
            .await
            .is_err()
    );
    admin.close().await;
}
