use super::*;

use wareboxes_api_contract::v1::{ShipmentOrderStatus, ShipmentStatus, ShippingQueuePage};

#[tokio::test]
async fn carton_subsets_depart_once_and_only_final_departure_ships_the_order() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("shipping-partial@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "shipping-partial-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Shipping Partial Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Shipping Partial Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "SHIP-PARTIAL-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "SHIP-PARTIAL-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "ship-partial-origin",
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
        "SHIP-PARTIAL",
    )
    .await;

    let created: CreateShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/orders/{}/shipments", ready.order_id),
                Some("ship-partial-create"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create partial shipment",
        )
        .await,
    )
    .await;
    let shipment_id = created.shipment.shipment_id;
    let shipment_path = format!("/api/v1/shipments/{shipment_id}");
    let _: RecordManualManifestResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("{shipment_path}/manifests"),
                Some("ship-partial-manifest"),
                Some(manifest_body(&ready, "PARTIAL-MANIFEST", 1)),
            )
            .await,
            StatusCode::OK,
            "manifest partial shipment",
        )
        .await,
    )
    .await;

    let departure_path = format!("{shipment_path}/departures");
    let first_body = departure_body(&ready.carton_barcodes[0], 2, 12);
    let second_body = departure_body(&ready.carton_barcodes[1], 2, 12);
    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &departure_path,
        Some("ship-partial-race-a"),
        Some(first_body.clone()),
    );
    let second = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &departure_path,
        Some("ship-partial-race-b"),
        Some(second_body.clone()),
    );
    let (first, second) = tokio::join!(first, second);
    let (winner, loser, replay_key, replay_body) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => {
            (first, second, "ship-partial-race-a", first_body)
        }
        (StatusCode::CONFLICT, StatusCode::OK) => {
            (second, first, "ship-partial-race-b", second_body)
        }
        statuses => panic!("expected one partial departure winner, got {statuses:?}"),
    };
    drop(loser);
    let partial: ConfirmShipmentDepartureResponse = response_json(winner).await;
    assert_eq!(partial.shipment_status, ShipmentStatus::PartiallyDeparted);
    assert_eq!(partial.order_status, ShipmentOrderStatus::AwaitingShipment);
    assert_eq!(partial.shipment_revision.get(), 3);
    assert_eq!(partial.order_revision.get(), 13);
    assert_eq!(partial.scanned_carton_count, 1);
    assert_eq!(partial.remaining_carton_count, 1);
    assert_eq!(
        partial.cumulative_departed_quantity,
        partial.departure_quantity
    );
    assert_eq!(partial.remaining_quantity + partial.departure_quantity, 5);

    let replay: ConfirmShipmentDepartureResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &departure_path,
                Some(replay_key),
                Some(replay_body.clone()),
            )
            .await,
            StatusCode::OK,
            "replay partial departure",
        )
        .await,
    )
    .await;
    assert_eq!(replay, partial);

    let after_partial: ShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &shipment_path,
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "read partial shipment",
        )
        .await,
    )
    .await;
    assert_eq!(after_partial.status, ShipmentStatus::PartiallyDeparted);
    assert_eq!(after_partial.departure_progress.departed_carton_count, 1);
    assert_eq!(after_partial.departure_progress.remaining_carton_count, 1);
    let departed_barcode = after_partial
        .cartons
        .iter()
        .find(|carton| carton.departed_at.is_some())
        .map(|carton| carton.carton_barcode.clone())
        .unwrap();
    let remaining_barcode = after_partial
        .cartons
        .iter()
        .find(|carton| carton.departed_at.is_none())
        .map(|carton| carton.carton_barcode.clone())
        .unwrap();

    let mut changed_replay_body = replay_body;
    changed_replay_body["scanned_carton_barcodes"] = json!([remaining_barcode.clone()]);
    let changed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &departure_path,
        Some(replay_key),
        Some(changed_replay_body),
    )
    .await;
    assert_eq!(changed_replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_replay).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    for (key, shipment_revision, order_revision) in [("shipment", 2, 13), ("order", 3, 12)] {
        let stale = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &departure_path,
            Some(&format!("ship-partial-stale-{key}")),
            Some(departure_body(
                &remaining_barcode,
                shipment_revision,
                order_revision,
            )),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT, "stale {key}");
    }

    let duplicate = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &departure_path,
        Some("ship-partial-duplicate"),
        Some(departure_body(&departed_barcode, 3, 13)),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);

    let queue: ShippingQueuePage = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                "/api/v1/shipping-queue?limit=100",
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "read partial shipping queue",
        )
        .await,
    )
    .await;
    let queued = queue
        .items
        .iter()
        .find(|entry| entry.order_id == ready.order_id)
        .and_then(|entry| entry.shipment.as_ref())
        .unwrap();
    assert_eq!(queued.status, ShipmentStatus::PartiallyDeparted);
    assert_eq!(queued.departed_carton_count, 1);
    assert_eq!(queued.departed_quantity, partial.departure_quantity);

    let final_departure: ConfirmShipmentDepartureResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &departure_path,
                Some("ship-partial-final"),
                Some(departure_body(&remaining_barcode, 3, 13)),
            )
            .await,
            StatusCode::OK,
            "final carton departure",
        )
        .await,
    )
    .await;
    assert_eq!(final_departure.shipment_status, ShipmentStatus::Departed);
    assert_eq!(final_departure.order_status, ShipmentOrderStatus::Shipped);
    assert_eq!(final_departure.shipment_revision.get(), 4);
    assert_eq!(final_departure.order_revision.get(), 14);
    assert_eq!(final_departure.remaining_carton_count, 0);
    assert_eq!(final_departure.remaining_quantity, 0);
    assert_eq!(final_departure.cumulative_departed_quantity, 5);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM shipment_confirmations WHERE tenant_id=$1 AND shipment_id=$2),
          (SELECT COUNT(*) FROM shipment_confirmation_cartons WHERE tenant_id=$1 AND shipment_id=$2),
          (SELECT COUNT(DISTINCT inventory_transaction_id) FROM shipment_confirmations WHERE tenant_id=$1 AND shipment_id=$2),
          (SELECT COUNT(*) FROM inventory_reservations WHERE tenant_id=$1 AND order_id=$3 AND status='fulfilled'),
          (SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_id=$4 AND event_type='shipping.shipment_partially_departed'),
          (SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_id=$4 AND event_type='shipping.shipment_departed')
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id)
    .bind(ready.order_id)
    .bind(ready.order_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, (2, 2, 2, 2, 1, 1));
}

fn departure_body(barcode: &str, shipment_revision: i64, order_revision: i64) -> Value {
    json!({
        "scanned_carton_barcodes": [barcode],
        "expected_shipment_revision": shipment_revision,
        "expected_order_revision": order_revision
    })
}
