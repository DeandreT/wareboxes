use wareboxes_api_contract::v1::{ErrorReason, ErrorResponse, ShippingQueuePage};

use super::*;

#[tokio::test]
async fn shipping_queue_is_scoped_paginated_resumable_and_removes_departures() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("shipping-queue@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "shipping-queue-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Shipping Queue Owner")
        .await;
    let other_owner_id = fixture
        .inventory_owner(access.tenant_id, "Hidden Shipping Queue Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Shipping Queue Facility")
        .await;
    let other_facility_id = fixture
        .facility(access.tenant_id, "Hidden Shipping Queue Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "SHIP-QUEUE-PACK").await;
    for barcode in ["SHIP-QUEUE-A-TOTE", "SHIP-QUEUE-B-TOTE"] {
        plate_at(
            &fixture,
            access.tenant_id,
            owner_id,
            facility_id,
            station_id,
            barcode,
        )
        .await;
    }
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "ship-queue-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first_ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "SHIP-QUEUE-A",
    )
    .await;
    let second_ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "SHIP-QUEUE-B",
    )
    .await;

    let first_page = queue_page(
        &app,
        &token,
        access.tenant_id,
        &format!("/api/v1/shipping-queue?facility_id={facility_id}&limit=1"),
    )
    .await;
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.items[0].order_id, first_ready.order_id);
    assert!(first_page.items[0].origin_ready);
    assert!(first_page.items[0].destination_ready);
    assert!(first_page.items[0].shipment.is_none());
    assert_eq!(
        first_page.items[0].order_revision.get(),
        first_ready.order_revision
    );
    let cursor = first_page.next_cursor.expect("queue has a second page");
    let second_page = queue_page(
        &app,
        &token,
        access.tenant_id,
        &format!("/api/v1/shipping-queue?facility_id={facility_id}&limit=1&cursor={cursor}"),
    )
    .await;
    assert_eq!(second_page.items.len(), 1);
    assert_eq!(second_page.items[0].order_id, second_ready.order_id);
    assert!(second_page.next_cursor.is_none());

    let mismatched_cursor = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!("/api/v1/shipping-queue?facility_id={other_facility_id}&limit=1&cursor={cursor}"),
        None,
        None,
    )
    .await;
    assert_eq!(mismatched_cursor.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(mismatched_cursor)
            .await
            .reason,
        ErrorReason::InvalidCursor
    );

    let create_path = format!("/api/v1/orders/{}/shipments", first_ready.order_id);
    let created = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("ship-queue-create"),
        Some(create_shipment_body(&first_ready)),
    )
    .await;
    let created: CreateShipmentResponse =
        response_json(expect_status(created, StatusCode::OK, "create queued shipment").await).await;
    let active_page = queue_page(
        &app,
        &token,
        access.tenant_id,
        &format!("/api/v1/shipping-queue?facility_id={facility_id}"),
    )
    .await;
    let active = active_page
        .items
        .iter()
        .find(|entry| entry.order_id == first_ready.order_id)
        .expect("active shipment remains queued");
    let shipment = active.shipment.as_ref().expect("shipment is resumable");
    assert_eq!(shipment.shipment_id, created.shipment.shipment_id);
    assert_eq!(active.order_revision, created.order_revision);
    assert_eq!(shipment.revision.get(), 1);

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        vec![other_facility_id],
        vec![other_owner_id],
    )
    .await;
    let concealed = queue_page(&app, &token, access.tenant_id, "/api/v1/shipping-queue").await;
    assert!(concealed.items.is_empty());
    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;

    let shipment_path = format!("/api/v1/shipments/{}", created.shipment.shipment_id);
    let manifest = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("{shipment_path}/manifests"),
        Some("ship-queue-manifest"),
        Some(manifest_body(&first_ready, "SHIP-QUEUE-MANIFEST", 1)),
    )
    .await;
    let _: RecordManualManifestResponse =
        response_json(expect_status(manifest, StatusCode::OK, "manifest queued shipment").await)
            .await;
    let departed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("{shipment_path}/departures"),
        Some("ship-queue-depart"),
        Some(json!({
            "scanned_carton_barcodes": first_ready.carton_barcodes,
            "expected_shipment_revision": 2,
            "expected_order_revision": created.order_revision.get()
        })),
    )
    .await;
    let _: ConfirmShipmentDepartureResponse =
        response_json(expect_status(departed, StatusCode::OK, "depart queued shipment").await)
            .await;
    let after_departure = queue_page(
        &app,
        &token,
        access.tenant_id,
        &format!("/api/v1/shipping-queue?facility_id={facility_id}"),
    )
    .await;
    assert_eq!(after_departure.items.len(), 1);
    assert_eq!(after_departure.items[0].order_id, second_ready.order_id);
    assert!(after_departure.items[0].shipment.is_none());
}

async fn queue_page(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    path: &str,
) -> ShippingQueuePage {
    let response = send(app, token, tenant_id, Method::GET, path, None, None).await;
    response_json(expect_status(response, StatusCode::OK, "read shipping queue").await).await
}
