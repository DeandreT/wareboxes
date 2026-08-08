use super::*;

struct RaceRig {
    fixture: Fixture,
    app: axum::Router,
    token: String,
    access: wareboxes_core::models::TenantAccess,
    facility_id: i64,
    staging_id: i64,
    packing_barcode: String,
    staging_barcode: String,
    ready: ReadyShipment,
    created: CreateShipmentResponse,
    manifested: RecordManualManifestResponse,
}

impl RaceRig {
    fn plan_body(&self, reference: &str) -> Value {
        json!({
            "facility_id": self.facility_id,
            "load_reference": reference,
            "carrier_code": "UPS",
            "staging_location_id": self.staging_id,
            "shipments": [{
                "shipment_id": self.created.shipment.shipment_id,
                "expected_shipment_revision": self.manifested.revision,
                "expected_order_revision": self.created.order_revision,
                "shipment_sequence": 1,
                "cartons": self.ready.carton_ids.iter().copied().enumerate().map(|(index, carton_id)| {
                    json!({"carton_id": carton_id, "load_sequence": index + 1})
                }).collect::<Vec<_>>()
            }]
        })
    }

    fn departure_body(&self) -> Value {
        json!({
            "expected_shipment_revision": self.manifested.revision,
            "expected_order_revision": self.created.order_revision,
            "scanned_carton_barcodes": self.ready.carton_barcodes
        })
    }
}

async fn race_rig(key: &str, email: &str) -> RaceRig {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user(email).await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "orders",
        &format!("{key}-orders"),
    )
    .await;
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "wms_supervisor",
        &format!("{key}-supervisor"),
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, &format!("{key} Owner"))
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, &format!("{key} Facility"))
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let packing_barcode = format!("{key}-PACK");
    let staging_barcode = format!("{key}-STAGE");
    let packing_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        &packing_barcode,
        "packing",
    )
    .await;
    let staging_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        &staging_barcode,
        "staging",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        &format!("{key}-origin"),
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        packing_id,
        &format!("{key}-TOTE"),
    )
    .await;
    let ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        packing_id,
        key,
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
                Some(&format!("{key}-shipment")),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create race shipment",
        )
        .await,
    )
    .await;
    let manifested: RecordManualManifestResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
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
            "manifest race shipment",
        )
        .await,
    )
    .await;
    RaceRig {
        fixture,
        app,
        token,
        access,
        facility_id,
        staging_id,
        packing_barcode,
        staging_barcode,
        ready,
        created,
        manifested,
    }
}

fn assert_one_winner(first: StatusCode, second: StatusCode) {
    let mut statuses = [first.as_u16(), second.as_u16()];
    statuses.sort_unstable();
    assert_eq!(
        statuses,
        [StatusCode::OK.as_u16(), StatusCode::CONFLICT.as_u16()]
    );
}

#[tokio::test]
async fn load_assignment_and_carton_movement_races_have_one_exact_winner() {
    let rig = race_rig("OUTBOUND-RACE", "outbound-load-race@test.local").await;
    let body_a = rig.plan_body("LOAD-RACE-A");
    let body_b = rig.plan_body("LOAD-RACE-B");
    let (first, second) = tokio::join!(
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            "/api/v1/outbound-loads",
            Some("outbound-race-plan-a"),
            Some(body_a.clone()),
        ),
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            "/api/v1/outbound-loads",
            Some("outbound-race-plan-b"),
            Some(body_b.clone()),
        )
    );
    assert_one_winner(first.status(), second.status());
    let (winner, winner_key, winner_body) = if first.status() == StatusCode::OK {
        (first, "outbound-race-plan-a", body_a)
    } else {
        (second, "outbound-race-plan-b", body_b)
    };
    let planned: PlanOutboundLoadResponse = response_json(winner).await;
    let load_id = planned.outbound_load.outbound_load_id;
    let replay: PlanOutboundLoadResponse = response_json(
        expect_status(
            send(
                &rig.app,
                &rig.token,
                rig.access.tenant_id,
                Method::POST,
                "/api/v1/outbound-loads",
                Some(winner_key),
                Some(winner_body),
            )
            .await,
            StatusCode::OK,
            "replay winning race plan",
        )
        .await,
    )
    .await;
    assert_eq!(replay, planned);

    expect_status(
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            &format!("/api/v1/outbound-loads/{load_id}/releases"),
            Some("outbound-race-release"),
            Some(json!({"expected_revision": 1})),
        )
        .await,
        StatusCode::OK,
        "release race load",
    )
    .await;
    let carton_id = rig.ready.carton_ids[0];
    let move_body = json!({
        "expected_load_revision": 2,
        "expected_position_revision": 1,
        "source_location_barcode": rig.packing_barcode,
        "carton_barcode": rig.ready.carton_barcodes[0],
        "staging_location_barcode": rig.staging_barcode
    });
    let path = format!("/api/v1/outbound-loads/{load_id}/cartons/{carton_id}/staging-movements");
    let (first, second) = tokio::join!(
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            &path,
            Some("outbound-race-stage-a"),
            Some(move_body.clone()),
        ),
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            &path,
            Some("outbound-race-stage-b"),
            Some(move_body),
        )
    );
    assert_one_winner(first.status(), second.status());

    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let (loads, links, moves): (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM outbound_loads WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM outbound_load_shipments WHERE tenant_id=$1 AND shipment_id=$2),
          (SELECT COUNT(*) FROM packed_carton_move_confirmations WHERE tenant_id=$1 AND carton_id=$3)
        "#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(rig.created.shipment.shipment_id)
    .bind(carton_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!((loads, links, moves), (1, 1, 1));
}

#[tokio::test]
async fn direct_departure_and_load_assignment_race_without_double_shipping() {
    let rig = race_rig(
        "OUTBOUND-DEPART-RACE",
        "outbound-load-depart-race@test.local",
    )
    .await;
    let plan_body = rig.plan_body("LOAD-DEPART-RACE");
    let departure_body = rig.departure_body();
    let shipment_id = rig.created.shipment.shipment_id;
    let departure_path = format!("/api/v1/shipments/{shipment_id}/departures");
    let (plan, departure) = tokio::join!(
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            "/api/v1/outbound-loads",
            Some("outbound-depart-race-plan"),
            Some(plan_body.clone()),
        ),
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            &departure_path,
            Some("outbound-depart-race-direct"),
            Some(departure_body.clone()),
        )
    );
    assert_one_winner(plan.status(), departure.status());
    let plan_won = plan.status() == StatusCode::OK;
    let replay = if plan_won {
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            "/api/v1/outbound-loads",
            Some("outbound-depart-race-plan"),
            Some(plan_body),
        )
        .await
    } else {
        send(
            &rig.app,
            &rig.token,
            rig.access.tenant_id,
            Method::POST,
            &departure_path,
            Some("outbound-depart-race-direct"),
            Some(departure_body),
        )
        .await
    };
    assert_eq!(replay.status(), StatusCode::OK);

    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let (active_links, departures, ship_transactions): (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM outbound_load_shipments WHERE tenant_id=$1 AND shipment_id=$2 AND closed_at IS NULL),
          (SELECT COUNT(*) FROM shipment_confirmations WHERE tenant_id=$1 AND shipment_id=$2),
          (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id=$1 AND reference_type='shipment' AND reference_id=$2 AND transaction_type='ship')
        "#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(shipment_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    if plan_won {
        assert_eq!((active_links, departures, ship_transactions), (1, 0, 0));
    } else {
        assert_eq!((active_links, departures, ship_transactions), (0, 1, 1));
    }
}
