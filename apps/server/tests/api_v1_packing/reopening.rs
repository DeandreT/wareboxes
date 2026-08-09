use super::super::*;
use wareboxes_api_contract::v1::ReopenCartonResponse;

async fn grant_supervisor(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
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
        "packing-reopening-supervisor",
        Some("Closed carton recovery"),
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

fn reopen_body(barcode: &str, revision: i64) -> Value {
    json!({
        "carton_barcode": barcode,
        "reason": "packing_correction",
        "note": "Correct contents before downstream execution",
        "expected_revision": revision
    })
}

#[tokio::test]
async fn closed_carton_reopens_once_replays_conceals_and_blocks_after_qa() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("packing-reopening@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "packing-reopening-orders",
    )
    .await;
    grant_supervisor(&fixture, access.tenant_id, operator.id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Packing Reopening Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Packing Reopening Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-REOPEN-STATION",
        "packing",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "PACK-REOPEN-TOTE",
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
        "PACK-REOPEN",
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
        "pack-reopen-release",
    )
    .await;
    pick_order(
        &app,
        &token,
        access.tenant_id,
        "PACK-REOPEN-TOTE",
        1,
        "pack-reopen",
    )
    .await;
    let opened = open_session(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        station_id,
        "pack-reopen-open",
    )
    .await;
    let session_id = opened.session.session_id;
    let allocation = opened.session.allocations[0].clone();
    let carton = create_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        "PACK-REOPEN-CARTON",
        5,
        "pack-reopen-carton",
    )
    .await;
    let carton_id = carton.carton.carton_id;
    let packed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/contents"),
        Some("pack-reopen-pack"),
        Some(pack_body(
            allocation.inventory_allocation_id,
            &allocation.item_barcodes[0],
            allocation.lot.as_deref().unwrap(),
            "PACK-REOPEN-TOTE",
            "PACK-REOPEN-CARTON",
            6,
        )),
    )
    .await;
    expect_status(packed, StatusCode::OK, "pack content before reopen").await;
    let closed = close_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        carton_id,
        "PACK-REOPEN-CARTON",
        7,
        "pack-reopen-close-first",
    )
    .await;
    assert_eq!(closed.revision.get(), 8);
    assert!(closed.ready_to_manifest);

    let path = format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/reopenings");
    let wrong_scan = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some("pack-reopen-wrong-scan"),
        Some(reopen_body("WRONG-CARTON", 8)),
    )
    .await;
    assert_eq!(wrong_scan.status(), StatusCode::BAD_REQUEST);

    let body = reopen_body("PACK-REOPEN-CARTON", 8);
    let left = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some("pack-reopen-left"),
        Some(body.clone()),
    );
    let right = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some("pack-reopen-right"),
        Some(body.clone()),
    );
    let (left, right) = tokio::join!(left, right);
    let (winner, loser, winner_key) = if left.status() == StatusCode::OK {
        (left, right, "pack-reopen-left")
    } else {
        (right, left, "pack-reopen-right")
    };
    assert_eq!(loser.status(), StatusCode::CONFLICT);
    let reopened: ReopenCartonResponse = response_json(winner).await;
    assert_eq!(reopened.revision.get(), 9);
    assert_eq!(
        reopened.previous_order_status,
        PackingOrderStatus::AwaitingShipment
    );
    assert_eq!(reopened.order_status, PackingOrderStatus::Packing);
    assert_eq!(reopened.lifecycle, PackCartonLifecycleResponse::Open);
    assert_eq!(
        reopened.previous_measurements.weight_grams.unwrap().get(),
        1250
    );
    assert_eq!(reopened.previous_closed_by, operator.id);
    assert_eq!(reopened.progress.open_carton_count, 1);
    assert_eq!(reopened.progress.closed_carton_count, 0);

    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some(winner_key),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<ReopenCartonResponse>(replay).await,
        reopened
    );
    let changed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some(winner_key),
        Some(json!({
            "carton_barcode": "PACK-REOPEN-CARTON",
            "reason": "quality_issue",
            "expected_revision": 8
        })),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    for changed_body in [false, true] {
        let concealed = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &path,
            Some(winner_key),
            Some(if changed_body {
                json!({
                    "carton_barcode": "PACK-REOPEN-CARTON",
                    "reason": "order_cancellation",
                    "expected_revision": 8
                })
            } else {
                body.clone()
            }),
        )
        .await;
        assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    }
    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;

    let reclosed = close_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        carton_id,
        "PACK-REOPEN-CARTON",
        9,
        "pack-reopen-close-second",
    )
    .await;
    assert_eq!(reclosed.revision.get(), 10);
    assert!(reclosed.ready_to_manifest);

    let policy = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/outbound-qa-policies",
        Some("pack-reopen-qa-policy"),
        Some(json!({
            "inventory_owner_id": owner_id,
            "facility_id": facility_id,
            "requirement": "scan_every_carton"
        })),
    )
    .await;
    expect_status(
        policy,
        StatusCode::OK,
        "configure QA before downstream guard",
    )
    .await;
    let qa = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/packing-sessions/{session_id}/outbound-qa-sessions"),
        Some("pack-reopen-qa-start"),
        Some(json!({"expected_order_revision": 10})),
    )
    .await;
    expect_status(qa, StatusCode::OK, "start downstream QA").await;
    let blocked = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some("pack-reopen-after-qa"),
        Some(reopen_body("PACK-REOPEN-CARTON", 10)),
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (String, i64, String, i64, String, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT carton.state,carton.reopen_count,session.state,session.revision,
               order_header.status,order_header.revision,
               (SELECT COUNT(*) FROM carton_reopenings reopening
                WHERE reopening.tenant_id=$1 AND reopening.carton_id=$2),
               (SELECT COUNT(*) FROM outbox_events event
                WHERE event.tenant_id=$1 AND event.aggregate_id=$3::TEXT
                  AND event.event_type='packing.carton_reopened')
        FROM cartons carton
        JOIN packing_sessions session ON session.tenant_id=carton.tenant_id
                                     AND session.id=carton.packing_session_id
        JOIN orders order_header ON order_header.tenant_id=carton.tenant_id
                                AND order_header.id=carton.order_id
        WHERE carton.tenant_id=$1 AND carton.id=$2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(carton_id)
    .bind(order.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        evidence,
        (
            "closed".into(),
            1,
            "ready_to_manifest".into(),
            10,
            "awaiting shipment".into(),
            10,
            1,
            1
        )
    );

    let admin = admin_db_for(&fixture.db).await;
    let forced: (bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT class.relforcerowsecurity,
               has_table_privilege('wareboxes_app','carton_reopenings','SELECT'),
               has_table_privilege('wareboxes_app','carton_reopenings','INSERT'),
               has_table_privilege('wareboxes_app','carton_reopenings','UPDATE'),
               has_table_privilege('wareboxes_app','carton_reopenings','DELETE'),
               has_sequence_privilege('wareboxes_app','carton_reopenings_id_seq','USAGE')
        FROM pg_class class WHERE class.oid='carton_reopenings'::regclass
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(forced, (true, true, true, false, false, true));
    for statement in [
        "UPDATE carton_reopenings SET reason_code=reason_code WHERE tenant_id=$1 AND id=$2",
        "DELETE FROM carton_reopenings WHERE tenant_id=$1 AND id=$2",
    ] {
        assert!(sqlx::query(statement)
            .bind(access.tenant_id.get())
            .bind(reopened.reopening_id)
            .execute(&admin)
            .await
            .is_err());
    }
    admin.close().await;
}
