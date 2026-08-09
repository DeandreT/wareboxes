use super::super::*;
use wareboxes_api_contract::v1::{
    AbandonPackSessionResponse, CancelOrderResponse, ConfirmShipmentDepartureResponse,
    CreateShipmentResponse, PackAllocationDispositionResponse, PackSessionStatus,
    RecordManualManifestResponse, RemovePackedContentResponse, ReversePickConfirmationResponse,
};

async fn grant_supervisor(fixture: &Fixture, tenant_id: TenantId, user_id: i64, key: &str) {
    let permission_id = match wareboxes_persistence_postgres::permissions::find_by_name(
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
            Some("Packing recovery supervisor permission"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        key,
        Some("Packing recovery supervisor"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission_id,
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

async fn configure_shipping_origin(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    actor_id: i64,
) {
    let admin = admin_db_for(&fixture.db).await;
    let mut tx = admin.begin().await.unwrap();
    let address_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO addresses
           (tenant_id,created,name,company,line1,postal_code,country,phone,email,state,city)
           VALUES ($1,clock_timestamp(),'Removal shipping','Wareboxes','100 Dock Way',
                   '89501','US','+1-775-555-0100','packing-removal@test.local','NV','Reno')
           RETURNING id"#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let (previous_address_id, revision): (Option<i64>, i64) = sqlx::query_as(
        "SELECT address_id,revision FROM facilities WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE facilities SET address_id=$1,revision=revision+1 WHERE tenant_id=$2 AND id=$3",
    )
    .bind(address_id)
    .bind(tenant_id.get())
    .bind(facility_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO facility_shipping_origin_configurations
           (tenant_id,facility_id,previous_address_id,address_id,configured_by_user_id,
            configured_at,expected_revision,resulting_revision)
           VALUES ($1,$2,$3,$4,$5,clock_timestamp(),$6,$6+1)"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(previous_address_id)
    .bind(address_id)
    .bind(actor_id)
    .bind(revision)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    admin.close().await;
}

fn removal_body(
    allocation: &wareboxes_api_contract::v1::PackableAllocationResponse,
    carton_barcode: &str,
    tote_barcode: &str,
    revision: i64,
) -> Value {
    let mut body = json!({
        "carton_barcode": carton_barcode,
        "item_barcode": allocation.item_barcodes[0],
        "destination_license_plate_barcode": tote_barcode,
        "reason": "wrong_carton",
        "expected_revision": revision
    });
    if let Some(lot) = allocation.lot.as_ref() {
        body["lot_scan"] = json!(lot);
    }
    if let Some(serial) = allocation.serial.as_ref() {
        body["serial_scan"] = json!(serial);
    }
    body
}

#[tokio::test]
async fn open_carton_content_can_be_returned_replayed_and_repacked_exactly() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("packing-removal@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "packing-removal-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Packing Removal Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Packing Removal Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-REMOVE-STATION",
        "packing",
    )
    .await;
    let tote_id = plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "PACK-REMOVE-TOTE",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = prepare_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PACK-REMOVE",
        &[3],
    )
    .await;
    release_order(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        station_id,
        "pack-remove-release",
    )
    .await;
    let picks = pick_order(
        &app,
        &token,
        access.tenant_id,
        "PACK-REMOVE-TOTE",
        1,
        "pack-remove",
    )
    .await;
    assert!(picks[0].order_ready_to_pack);
    let opened = open_session(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        station_id,
        "pack-remove-open",
    )
    .await;
    let session_id = opened.session.session_id;
    let original = opened.session.allocations[0].clone();
    let carton = create_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        "PACK-REMOVE-CARTON",
        5,
        "pack-remove-carton",
    )
    .await;
    let carton_id = carton.carton.carton_id;
    let pack_path = format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/contents");
    let packed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &pack_path,
        Some("pack-remove-pack"),
        Some(pack_body(
            original.inventory_allocation_id,
            &original.item_barcodes[0],
            original.lot.as_deref().unwrap(),
            "PACK-REMOVE-TOTE",
            "PACK-REMOVE-CARTON",
            6,
        )),
    )
    .await;
    let packed: PackPickedAllocationResponse =
        response_json(expect_status(packed, StatusCode::OK, "initial pack").await).await;
    assert_eq!(packed.revision.get(), 7);
    let removal_path = format!(
        "/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/contents/{}/removals",
        packed.content_id
    );
    let mut bad_scan = removal_body(&original, "PACK-REMOVE-CARTON", "PACK-REMOVE-TOTE", 7);
    bad_scan["item_barcode"] = json!("WRONG-ITEM");
    let rejected = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &removal_path,
        Some("pack-remove-bad-item"),
        Some(bad_scan),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rejected_effects: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT COUNT(*) FROM carton_content_removals WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM inventory_transactions
           WHERE tenant_id=$1 AND operation='packing.content.remove.v1'),
          (SELECT revision FROM packing_sessions WHERE tenant_id=$1 AND id=$2)"#,
    )
    .bind(access.tenant_id.get())
    .bind(session_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rejected_effects, (0, 0, 7));

    let request_body = removal_body(&original, "PACK-REMOVE-CARTON", "PACK-REMOVE-TOTE", 7);
    let left_removal = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &removal_path,
        Some("pack-remove-confirm-left"),
        Some(request_body.clone()),
    );
    let right_removal = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &removal_path,
        Some("pack-remove-confirm-right"),
        Some(request_body.clone()),
    );
    let (left_removal, right_removal) = tokio::join!(left_removal, right_removal);
    let (removed, winner_key, loser) = if left_removal.status() == StatusCode::OK {
        (left_removal, "pack-remove-confirm-left", right_removal)
    } else {
        (right_removal, "pack-remove-confirm-right", left_removal)
    };
    assert_eq!(removed.status(), StatusCode::OK);
    assert_eq!(loser.status(), StatusCode::CONFLICT);
    let removed: RemovePackedContentResponse =
        response_json(expect_status(removed, StatusCode::OK, "remove packed content").await).await;
    assert_eq!(removed.revision.get(), 8);
    assert_eq!(removed.quantity, 3);
    assert_eq!(removed.destination_license_plate_id, tote_id);
    assert_eq!(removed.progress.packed_allocation_count, 0);
    assert_eq!(removed.progress.packed_quantity, 0);
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &removal_path,
        Some(winner_key),
        Some(request_body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<RemovePackedContentResponse>(
            expect_status(replay, StatusCode::OK, "replay removal").await
        )
        .await,
        removed
    );
    let mut changed = request_body;
    changed["reason"] = json!("wrong_item");
    assert_eq!(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &removal_path,
            Some(winner_key),
            Some(changed),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let current = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!("/api/v1/packing-sessions/{session_id}"),
        None,
        None,
    )
    .await;
    let current: PackSessionResponse =
        response_json(expect_status(current, StatusCode::OK, "read returned content").await).await;
    assert_eq!(current.cartons[0].content_count, 0);
    assert_eq!(current.allocations.len(), 1);
    let returned = &current.allocations[0];
    assert_eq!(
        returned.inventory_allocation_id,
        removed.destination_inventory_allocation_id
    );
    assert_eq!(returned.license_plate_id, tote_id);
    assert_eq!(
        returned.disposition,
        PackAllocationDispositionResponse::Available
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence = sqlx::query(
        r#"SELECT source.status AS source_status,destination.status AS destination_status,
                  source.execution_stage AS source_stage,destination.execution_stage AS destination_stage,
                  source_balance.qty_reserved AS source_reserved,
                  destination_balance.qty_reserved AS destination_reserved,
                  position.state AS allocation_state,position.revision AS allocation_revision,
                  packed.state AS packed_state,packed.revision AS packed_revision,
                  (SELECT COUNT(*) FROM inventory_entries entry
                   WHERE entry.tenant_id=$1 AND entry.transaction_id=$2) AS entry_count,
                  (SELECT COALESCE(SUM(quantity_delta),0)::BIGINT FROM inventory_entries entry
                   WHERE entry.tenant_id=$1 AND entry.transaction_id=$2) AS journal_net
           FROM inventory_allocations source
           JOIN inventory_allocations destination ON destination.tenant_id=source.tenant_id AND destination.id=$3
           JOIN inventory_balances source_balance ON source_balance.tenant_id=source.tenant_id AND source_balance.id=$4
           JOIN inventory_balances destination_balance ON destination_balance.tenant_id=source.tenant_id AND destination_balance.id=$5
           JOIN packing_allocation_positions position ON position.tenant_id=source.tenant_id
                                                    AND position.current_inventory_allocation_id=destination.id
           JOIN packed_inventory_positions packed ON packed.tenant_id=source.tenant_id
                                                 AND packed.carton_content_id=$6
           WHERE source.tenant_id=$1 AND source.id=$7"#,
    )
    .bind(access.tenant_id.get())
    .bind(removed.inventory_transaction_id)
    .bind(removed.destination_inventory_allocation_id)
    .bind(removed.source_inventory_balance_id)
    .bind(removed.destination_inventory_balance_id)
    .bind(removed.content_id)
    .bind(removed.source_inventory_allocation_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        evidence.try_get::<String, _>("source_status").unwrap(),
        "fulfilled"
    );
    assert_eq!(
        evidence.try_get::<String, _>("destination_status").unwrap(),
        "allocated"
    );
    assert_eq!(
        evidence.try_get::<String, _>("source_stage").unwrap(),
        "packed"
    );
    assert_eq!(
        evidence.try_get::<String, _>("destination_stage").unwrap(),
        "staged"
    );
    assert_eq!(evidence.try_get::<i64, _>("source_reserved").unwrap(), 0);
    assert_eq!(
        evidence.try_get::<i64, _>("destination_reserved").unwrap(),
        3
    );
    assert_eq!(
        evidence.try_get::<String, _>("allocation_state").unwrap(),
        "available"
    );
    assert_eq!(
        evidence.try_get::<i64, _>("allocation_revision").unwrap(),
        3
    );
    assert_eq!(
        evidence.try_get::<String, _>("packed_state").unwrap(),
        "unpacked"
    );
    assert_eq!(evidence.try_get::<i64, _>("packed_revision").unwrap(), 2);
    assert_eq!(evidence.try_get::<i64, _>("entry_count").unwrap(), 2);
    assert_eq!(evidence.try_get::<i64, _>("journal_net").unwrap(), 0);

    let admin = admin_db_for(&fixture.db).await;
    for (operation, statement) in [
        (
            "removal evidence update",
            "UPDATE carton_content_removals SET reason_code=reason_code WHERE tenant_id=$1 AND id=$2",
        ),
        (
            "removal evidence delete",
            "DELETE FROM carton_content_removals WHERE tenant_id=$1 AND id=$2",
        ),
    ] {
        let result = sqlx::query(statement)
            .bind(access.tenant_id.get())
            .bind(removed.removal_id)
            .execute(&admin)
            .await;
        assert!(result.is_err(), "{operation} must fail");
    }
    admin.close().await;

    let repacked = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &pack_path,
        Some("pack-remove-repack"),
        Some(pack_body(
            returned.inventory_allocation_id,
            &returned.item_barcodes[0],
            returned.lot.as_deref().unwrap(),
            "PACK-REMOVE-TOTE",
            "PACK-REMOVE-CARTON",
            8,
        )),
    )
    .await;
    let repacked: PackPickedAllocationResponse =
        response_json(expect_status(repacked, StatusCode::OK, "repack returned content").await)
            .await;
    assert_ne!(repacked.content_id, packed.content_id);
    assert_eq!(repacked.revision.get(), 9);
    let closed = close_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        carton_id,
        "PACK-REMOVE-CARTON",
        9,
        "pack-remove-close",
    )
    .await;
    assert!(closed.ready_to_manifest);
    assert_eq!(closed.revision.get(), 10);

    configure_shipping_origin(&fixture, access.tenant_id, facility_id, operator.id).await;
    let created = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/shipments", order.order_id),
        Some("pack-remove-shipment"),
        Some(json!({
            "packing_session_id": session_id,
            "expected_revision": 10
        })),
    )
    .await;
    let created: CreateShipmentResponse =
        response_json(expect_status(created, StatusCode::OK, "create repacked shipment").await)
            .await;
    assert_eq!(created.shipment.cartons.len(), 1);
    assert_eq!(created.shipment.cartons[0].content_count, 1);
    assert_eq!(created.shipment.cartons[0].packed_quantity, 3);
    assert_eq!(created.order_revision.get(), 11);
    let shipment_id = created.shipment.shipment_id;
    let manifest = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/shipments/{shipment_id}/manifests"),
        Some("pack-remove-manifest"),
        Some(json!({
            "carrier_code": "UPS",
            "service_code": "GROUND",
            "manifest_reference": "PACK-REMOVE-MANIFEST",
            "carton_tracking_assignments": [{
                "carton_id": carton_id,
                "tracking_number": "PACK-REMOVE-TRACKING"
            }],
            "expected_revision": 1
        })),
    )
    .await;
    let manifested: RecordManualManifestResponse =
        response_json(expect_status(manifest, StatusCode::OK, "manifest repacked shipment").await)
            .await;
    assert_eq!(manifested.revision.get(), 2);
    let departed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/shipments/{shipment_id}/departures"),
        Some("pack-remove-depart"),
        Some(json!({
            "scanned_carton_barcodes": ["PACK-REMOVE-CARTON"],
            "expected_shipment_revision": 2,
            "expected_order_revision": 11
        })),
    )
    .await;
    let departed: ConfirmShipmentDepartureResponse =
        response_json(expect_status(departed, StatusCode::OK, "depart repacked shipment").await)
            .await;
    assert_eq!(departed.departure_quantity, 3);
    assert_eq!(departed.demand.shipped_quantity, 3);
    assert_eq!(departed.shipment_revision.get(), 3);
    assert_eq!(departed.order_revision.get(), 12);
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let shipped: (String, i64, i64, i64) = sqlx::query_as(
        r#"SELECT order_header.status,shipment.shipped_qty,
                  (SELECT COUNT(*) FROM carton_contents content
                   WHERE content.tenant_id=$1 AND content.carton_id=$2),
                  (SELECT COUNT(*) FROM packed_inventory_positions position
                   WHERE position.tenant_id=$1 AND position.carton_id=$2
                     AND position.state='departed')
           FROM orders order_header
           JOIN shipments shipment ON shipment.tenant_id=order_header.tenant_id
                                  AND shipment.order_id=order_header.id
           WHERE order_header.tenant_id=$1 AND order_header.id=$3"#,
    )
    .bind(access.tenant_id.get())
    .bind(carton_id)
    .bind(order.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(shipped, ("shipped".into(), 3, 2, 1));
}

#[tokio::test]
async fn empty_session_abandonment_unlocks_pick_reversal_and_order_cancellation() {
    let fixture = Fixture::new().await;
    let supervisor = fixture.wms_user("packing-abandonment@test.local").await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        "packing-abandonment-orders",
    )
    .await;
    grant_supervisor(
        &fixture,
        access.tenant_id,
        supervisor.id,
        "packing-abandonment-supervisor",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Packing Abandonment Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Packing Abandonment Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-ABANDON-STATION",
        "packing",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "PACK-ABANDON-TOTE",
    )
    .await;
    let token = auth::create_session(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = prepare_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PACK-ABANDON",
        &[3],
    )
    .await;
    release_order(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        station_id,
        "pack-abandon-release",
    )
    .await;
    let picks = pick_order(
        &app,
        &token,
        access.tenant_id,
        "PACK-ABANDON-TOTE",
        1,
        "pack-abandon",
    )
    .await;
    let confirmation_id = picks[0].result_id;
    let opened = open_session(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        station_id,
        "pack-abandon-open",
    )
    .await;
    let session_id = opened.session.session_id;
    let original = opened.session.allocations[0].clone();
    let carton = create_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        "PACK-ABANDON-CARTON",
        5,
        "pack-abandon-carton",
    )
    .await;
    let carton_id = carton.carton.carton_id;
    let packed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/contents"),
        Some("pack-abandon-pack"),
        Some(pack_body(
            original.inventory_allocation_id,
            &original.item_barcodes[0],
            original.lot.as_deref().unwrap(),
            "PACK-ABANDON-TOTE",
            "PACK-ABANDON-CARTON",
            6,
        )),
    )
    .await;
    let packed: PackPickedAllocationResponse =
        response_json(expect_status(packed, StatusCode::OK, "pack before abandonment").await).await;
    let removed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/contents/{}/removals",
            packed.content_id
        ),
        Some("pack-abandon-remove"),
        Some(removal_body(
            &original,
            "PACK-ABANDON-CARTON",
            "PACK-ABANDON-TOTE",
            7,
        )),
    )
    .await;
    let removed: RemovePackedContentResponse = response_json(
        expect_status(removed, StatusCode::OK, "return content before abandonment").await,
    )
    .await;
    assert_eq!(removed.revision.get(), 8);

    let abandon_path = format!("/api/v1/packing-sessions/{session_id}/abandonments");
    let abandon_body = json!({
        "reason": "order_cancellation",
        "note": "Client cancelled after packing began",
        "expected_revision": 8
    });
    let not_voided = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &abandon_path,
        Some("pack-abandon-carton-not-voided"),
        Some(abandon_body.clone()),
    )
    .await;
    assert_eq!(not_voided.status(), StatusCode::CONFLICT);
    let voided = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/voids"),
        Some("pack-abandon-void"),
        Some(json!({
            "carton_barcode": "PACK-ABANDON-CARTON",
            "expected_revision": 8
        })),
    )
    .await;
    let voided: VoidCartonResponse =
        response_json(expect_status(voided, StatusCode::OK, "void restored carton").await).await;
    assert_eq!(voided.revision.get(), 9);

    let abandon_body = json!({
        "reason": "order_cancellation",
        "note": "Client cancelled after packing began",
        "expected_revision": 9
    });
    let left = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &abandon_path,
        Some("pack-abandon-left"),
        Some(abandon_body.clone()),
    );
    let right = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &abandon_path,
        Some("pack-abandon-right"),
        Some(abandon_body.clone()),
    );
    let (left, right) = tokio::join!(left, right);
    let (winner, loser, winner_key) = if left.status() == StatusCode::OK {
        (left, right, "pack-abandon-left")
    } else {
        (right, left, "pack-abandon-right")
    };
    assert_eq!(loser.status(), StatusCode::CONFLICT);
    let abandoned: AbandonPackSessionResponse = response_json(winner).await;
    assert_eq!(abandoned.revision.get(), 10);
    assert_eq!(abandoned.session_status, PackSessionStatus::Abandoned);
    assert_eq!(abandoned.progress.status, PackSessionStatus::Abandoned);
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &abandon_path,
        Some(winner_key),
        Some(abandon_body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<AbandonPackSessionResponse>(replay).await,
        abandoned
    );
    set_scope(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    let concealed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &abandon_path,
        Some(winner_key),
        Some(abandon_body.clone()),
    )
    .await;
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);
    let concealed_changed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &abandon_path,
        Some(winner_key),
        Some(json!({
            "reason": "station_issue",
            "note": "Changed while concealed",
            "expected_revision": 9
        })),
    )
    .await;
    assert_eq!(concealed_changed_replay.status(), StatusCode::NOT_FOUND);
    set_scope(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let active = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!("/api/v1/orders/{}/packing-session", order.order_id),
        None,
        None,
    )
    .await;
    assert!(response_json::<Option<PackSessionResponse>>(active)
        .await
        .is_none());
    let premature_reopen = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/packing-sessions", order.order_id),
        Some("pack-abandon-premature-reopen"),
        Some(json!({
            "facility_id": facility_id,
            "station_location_id": station_id,
            "expected_revision": 10
        })),
    )
    .await;
    assert_eq!(premature_reopen.status(), StatusCode::CONFLICT);
    let history = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!("/api/v1/packing-sessions/{session_id}"),
        None,
        None,
    )
    .await;
    let history: PackSessionResponse = response_json(history).await;
    assert_eq!(history.status, PackSessionStatus::Abandoned);
    assert!(history.abandonment.is_some());

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let scan = sqlx::query(
        r#"SELECT source_location.barcode AS source_location_barcode,
                  source_plate.barcode AS source_plate_barcode,
                  batch.lot,batch.serial
           FROM pick_confirmations confirmation
           JOIN locations source_location
             ON source_location.tenant_id=confirmation.tenant_id
            AND source_location.id=confirmation.source_location_id
           LEFT JOIN license_plates source_plate
             ON source_plate.tenant_id=confirmation.tenant_id
            AND source_plate.id=confirmation.source_license_plate_id
           JOIN item_batches batch
             ON batch.tenant_id=confirmation.tenant_id
            AND batch.id=confirmation.item_batch_id
           WHERE confirmation.tenant_id=$1 AND confirmation.id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(confirmation_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let reverse = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/pick-confirmations/{confirmation_id}/reversals"),
        Some("pack-abandon-reverse-pick"),
        Some(json!({
            "expected_order_revision": 10,
            "staged_location_barcode": "PACK-ABANDON-STATION",
            "staged_license_plate_barcode": "PACK-ABANDON-TOTE",
            "item_barcode": original.item_barcodes[0],
            "lot_scan": scan.try_get::<Option<String>, _>("lot").unwrap(),
            "serial_scan": scan.try_get::<Option<String>, _>("serial").unwrap(),
            "return_location_barcode": scan.try_get::<String, _>("source_location_barcode").unwrap(),
            "return_license_plate_barcode": scan.try_get::<Option<String>, _>("source_plate_barcode").unwrap(),
            "reason": "order_exception",
            "note": "Restored packing content before client cancellation"
        })),
    )
    .await;
    let reversed: ReversePickConfirmationResponse =
        response_json(expect_status(reverse, StatusCode::OK, "reverse restored pick").await).await;
    assert_eq!(
        reversed.staged_inventory_allocation_id,
        removed.destination_inventory_allocation_id
    );
    assert_eq!(reversed.order_revision.get(), 11);

    let cancellation = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/cancellations", order.order_id),
        Some("pack-abandon-cancel-order"),
        Some(json!({
            "expected_revision": 11,
            "reason": "client_request",
            "note": "Client cancelled after physical recovery"
        })),
    )
    .await;
    let cancelled: CancelOrderResponse = response_json(
        expect_status(
            cancellation,
            StatusCode::OK,
            "cancel recovered packed order",
        )
        .await,
    )
    .await;
    assert_eq!(cancelled.revision.get(), 12);
    assert_eq!(cancelled.reversed_pick_confirmation_count, 1);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (String, String, i64, i64, i64) = sqlx::query_as(
        r#"SELECT order_header.status,session.state,
                  (SELECT COUNT(*) FROM outbox_events event
                   WHERE event.tenant_id=$1 AND event.event_type='packing.session_abandoned'
                     AND event.aggregate_id=$2::TEXT),
                  (SELECT COUNT(*) FROM command_idempotency_records record
                   WHERE record.tenant_id=$1 AND record.operation='packing.session.abandon.v1'),
                  (SELECT COUNT(*) FROM packing_sessions current_session
                   WHERE current_session.tenant_id=$1 AND current_session.order_id=$2
                     AND current_session.state<>'abandoned')
           FROM orders order_header
           JOIN packing_sessions session ON session.tenant_id=order_header.tenant_id
                                        AND session.order_id=order_header.id
           WHERE order_header.tenant_id=$1 AND order_header.id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(order.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, ("cancelled".into(), "abandoned".into(), 1, 1, 0));

    let admin = admin_db_for(&fixture.db).await;
    let mut admin_tx = admin.begin().await.unwrap();
    let mutation = sqlx::query(
        "UPDATE packing_sessions SET abandonment_note='forged' WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(session_id)
    .execute(&mut *admin_tx)
    .await;
    assert!(mutation.is_err(), "abandonment evidence must be immutable");
    admin_tx.rollback().await.unwrap();
}
