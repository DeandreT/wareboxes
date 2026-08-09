use super::*;
use wareboxes_api_contract::v1::{
    CreateShipmentResponse, OutboundQaPolicyResponse, OutboundQaRequirement,
    OutboundQaSessionResponse, OutboundQaSessionStatus, ShippingQueuePage,
};

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
            Some("Supervise warehouse execution"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "outbound-qa-supervisor",
        Some("Outbound QA configuration"),
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

#[tokio::test]
async fn required_outbound_qa_scans_the_exact_carton_set_before_shipping() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("outbound-qa-flow@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "outbound-qa-orders",
    )
    .await;
    grant_supervisor(&fixture, access.tenant_id, operator.id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Outbound QA Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Outbound QA Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "OUTBOUND-QA-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "OUTBOUND-QA-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "outbound-qa-origin",
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
        "OUTBOUND-QA",
    )
    .await;

    let policy_body = json!({
        "inventory_owner_id": owner_id,
        "facility_id": facility_id,
        "requirement": "scan_every_carton"
    });
    let policy_response = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/outbound-qa-policies",
        Some("outbound-qa-policy"),
        Some(policy_body.clone()),
    )
    .await;
    let policy: OutboundQaPolicyResponse = response_json(
        expect_status(
            policy_response,
            StatusCode::OK,
            "configure outbound QA policy",
        )
        .await,
    )
    .await;
    assert_eq!(policy.requirement, OutboundQaRequirement::ScanEveryCarton);
    assert_eq!(policy.revision.get(), 1);
    let replayed_policy: OutboundQaPolicyResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/outbound-qa-policies",
                Some("outbound-qa-policy"),
                Some(policy_body),
            )
            .await,
            StatusCode::OK,
            "replay outbound QA policy",
        )
        .await,
    )
    .await;
    assert_eq!(replayed_policy, policy);
    let stale_policy = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/outbound-qa-policies",
        Some("outbound-qa-policy-stale"),
        Some(json!({
            "inventory_owner_id": owner_id,
            "facility_id": facility_id,
            "requirement": "scan_every_carton",
            "expected_revision": 99
        })),
    )
    .await;
    assert_eq!(stale_policy.status(), StatusCode::CONFLICT);
    let current_policy: OutboundQaPolicyResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/outbound-qa-policies",
                Some("outbound-qa-policy-v2"),
                Some(json!({
                    "inventory_owner_id": owner_id,
                    "facility_id": facility_id,
                    "requirement": "scan_every_carton",
                    "expected_revision": 1
                })),
            )
            .await,
            StatusCode::OK,
            "replace outbound QA policy",
        )
        .await,
    )
    .await;
    assert_eq!(current_policy.revision.get(), 2);

    let inventory_transactions_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id=$1")
            .bind(access.tenant_id.get())
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    let create_path = format!("/api/v1/orders/{}/shipments", ready.order_id);
    let blocked = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &create_path,
        Some("outbound-qa-create-blocked"),
        Some(create_shipment_body(&ready)),
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::CONFLICT);

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
            "read QA-aware shipping queue",
        )
        .await,
    )
    .await;
    let entry = queue
        .items
        .iter()
        .find(|entry| entry.order_id == ready.order_id)
        .expect("ready order appears in shipping queue");
    assert_eq!(
        entry
            .outbound_qa_policy
            .as_ref()
            .map(|policy| (policy.requirement, policy.revision.get())),
        Some((OutboundQaRequirement::ScanEveryCarton, 2))
    );
    assert!(entry.outbound_qa_session.is_none());

    let start_path = format!(
        "/api/v1/packing-sessions/{}/outbound-qa-sessions",
        ready.packing_session_id
    );
    let started: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &start_path,
                Some("outbound-qa-start"),
                Some(json!({"expected_order_revision": ready.order_revision})),
            )
            .await,
            StatusCode::OK,
            "start outbound QA",
        )
        .await,
    )
    .await;
    assert_eq!(started.status, OutboundQaSessionStatus::Open);
    assert_eq!(started.policy_revision.get(), 2);
    assert_eq!(started.progress.expected_carton_count, 2);
    assert_eq!(started.progress.verified_carton_count, 0);

    let complete_path = format!(
        "/api/v1/outbound-qa-sessions/{}/completions",
        started.session_id
    );
    let early = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &complete_path,
        Some("outbound-qa-complete-early"),
        Some(json!({"expected_revision": 1})),
    )
    .await;
    assert_eq!(early.status(), StatusCode::CONFLICT);
    let verify_path = format!(
        "/api/v1/outbound-qa-sessions/{}/carton-verifications",
        started.session_id
    );
    let wrong = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &verify_path,
        Some("outbound-qa-wrong-carton"),
        Some(json!({"expected_revision": 1, "carton_barcode": "NOT-THIS-ORDER"})),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);

    let first_body = json!({
        "expected_revision": 1,
        "carton_barcode": ready.carton_barcodes[0]
    });
    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &verify_path,
        Some("outbound-qa-carton-1"),
        Some(first_body.clone()),
    )
    .await;
    let first: Value = response_json(
        expect_status(first, StatusCode::OK, "verify first outbound QA carton").await,
    )
    .await;
    let replay: Value = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &verify_path,
                Some("outbound-qa-carton-1"),
                Some(first_body),
            )
            .await,
            StatusCode::OK,
            "replay first outbound QA carton",
        )
        .await,
    )
    .await;
    assert_eq!(replay, first);
    let changed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &verify_path,
        Some("outbound-qa-carton-1"),
        Some(json!({
            "expected_revision": 1,
            "carton_barcode": ready.carton_barcodes[1]
        })),
    )
    .await;
    assert_eq!(changed_replay.status(), StatusCode::CONFLICT);

    let second_body = json!({
        "expected_revision": 2,
        "carton_barcode": ready.carton_barcodes[1]
    });
    let second_a = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &verify_path,
        Some("outbound-qa-carton-2-a"),
        Some(second_body.clone()),
    );
    let second_b = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &verify_path,
        Some("outbound-qa-carton-2-b"),
        Some(second_body.clone()),
    );
    let (second_a, second_b) = tokio::join!(second_a, second_b);
    let (winner, replay_key) = match (second_a.status(), second_b.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (second_a, "outbound-qa-carton-2-a"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second_b, "outbound-qa-carton-2-b"),
        statuses => panic!("expected one outbound QA scan winner, got {statuses:?}"),
    };
    let second: OutboundQaSessionResponse = response_json(winner).await;
    assert_eq!(second.progress.verified_carton_count, 2);
    assert_eq!(second.revision.get(), 3);
    let second_replay: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &verify_path,
                Some(replay_key),
                Some(second_body),
            )
            .await,
            StatusCode::OK,
            "replay winning second outbound QA scan",
        )
        .await,
    )
    .await;
    assert_eq!(second_replay, second);

    let passed: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &complete_path,
                Some("outbound-qa-complete"),
                Some(json!({"expected_revision": 3})),
            )
            .await,
            StatusCode::OK,
            "complete outbound QA",
        )
        .await,
    )
    .await;
    assert_eq!(passed.status, OutboundQaSessionStatus::Passed);
    assert_eq!(passed.revision.get(), 4);
    assert_eq!(passed.verifications.len(), 2);

    let inventory_transactions_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id=$1")
            .bind(access.tenant_id.get())
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(inventory_transactions_after, inventory_transactions_before);

    let created: CreateShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &create_path,
                Some("outbound-qa-create-after-pass"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create shipment after outbound QA",
        )
        .await,
    )
    .await;
    assert_eq!(created.shipment.cartons.len(), 2);

    let admin = admin_db_for(&fixture.db).await;
    let evidence: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM outbound_qa_policies WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM outbound_qa_sessions WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM outbound_qa_carton_verifications WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM outbound_qa_completions WHERE tenant_id=$1)
        "#,
    )
    .bind(access.tenant_id.get())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(evidence, (2, 1, 2, 1));
    let event_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM outbox_events
        WHERE tenant_id=$1 AND event_type IN (
          'outbound.qa.policy_configured','outbound.qa.started',
          'outbound.qa.carton_verified','outbound.qa.passed')
        "#,
    )
    .bind(access.tenant_id.get())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(event_count, 6);
    for table in [
        "outbound_qa_policies",
        "outbound_qa_sessions",
        "outbound_qa_carton_verifications",
        "outbound_qa_completions",
    ] {
        assert!(
            sqlx::query(&format!("UPDATE {table} SET id=id WHERE tenant_id=$1"))
                .bind(access.tenant_id.get())
                .execute(&admin)
                .await
                .is_err()
        );
        assert!(
            sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id=$1"))
                .bind(access.tenant_id.get())
                .execute(&admin)
                .await
                .is_err()
        );
    }
    admin.close().await;

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    let session_path = format!("/api/v1/outbound-qa-sessions/{}", passed.session_id);
    for response in [
        send(
            &app,
            &token,
            access.tenant_id,
            Method::GET,
            &session_path,
            None,
            None,
        )
        .await,
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &complete_path,
            Some("outbound-qa-complete"),
            Some(json!({"expected_revision": 3})),
        )
        .await,
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &complete_path,
            Some("outbound-qa-complete"),
            Some(json!({"expected_revision": 2})),
        )
        .await,
    ] {
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
