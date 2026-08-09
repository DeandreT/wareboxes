use super::*;
use wareboxes_api_contract::v1::{
    CreateShipmentResponse, OutboundQaCancellationReason, OutboundQaPolicyResponse,
    OutboundQaRequirement, OutboundQaSessionResponse, OutboundQaSessionStatus,
    ReopenCartonResponse, ShippingQueuePage,
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

#[tokio::test]
async fn cancelled_qa_attempts_preserve_history_and_unlock_carton_recovery() {
    let fixture = Fixture::new().await;
    let supervisor = fixture.wms_user("outbound-qa-recovery@test.local").await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        "outbound-qa-recovery-orders",
    )
    .await;
    grant_supervisor(&fixture, access.tenant_id, supervisor.id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Outbound QA Recovery Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Outbound QA Recovery Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "OUTBOUND-QA-RECOVERY-PACK",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "OUTBOUND-QA-RECOVERY-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "outbound-qa-recovery-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, supervisor.id)
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
        "OUTBOUND-QA-RECOVERY",
    )
    .await;
    let _: OutboundQaPolicyResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/outbound-qa-policies",
                Some("outbound-qa-recovery-policy"),
                Some(json!({
                    "inventory_owner_id": owner_id,
                    "facility_id": facility_id,
                    "requirement": "scan_every_carton"
                })),
            )
            .await,
            StatusCode::OK,
            "configure recovery QA policy",
        )
        .await,
    )
    .await;
    let start_path = format!(
        "/api/v1/packing-sessions/{}/outbound-qa-sessions",
        ready.packing_session_id
    );
    let attempt_one: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &start_path,
                Some("outbound-qa-recovery-start-1"),
                Some(json!({"expected_order_revision": ready.order_revision})),
            )
            .await,
            StatusCode::OK,
            "start first recovery QA attempt",
        )
        .await,
    )
    .await;
    assert_eq!(attempt_one.attempt, 1);
    let verify_one_path = format!(
        "/api/v1/outbound-qa-sessions/{}/carton-verifications",
        attempt_one.session_id
    );
    let _: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &verify_one_path,
                Some("outbound-qa-recovery-verify-1"),
                Some(json!({
                    "expected_revision": 1,
                    "carton_barcode": ready.carton_barcodes[0]
                })),
            )
            .await,
            StatusCode::OK,
            "verify one carton before QA cancellation",
        )
        .await,
    )
    .await;

    let operator = add_wms_operator(
        &fixture,
        access.tenant_id,
        "outbound-qa-cancel-denied@test.local",
        "outbound-qa-cancel-denied",
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let cancel_one_path = format!(
        "/api/v1/outbound-qa-sessions/{}/cancellations",
        attempt_one.session_id
    );
    let cancel_one_body = json!({
        "expected_revision": 2,
        "reason": "packing_correction",
        "note": "Carton closure requires correction"
    });
    assert_eq!(
        send(
            &app,
            &operator_token,
            access.tenant_id,
            Method::POST,
            &cancel_one_path,
            Some("outbound-qa-recovery-denied"),
            Some(cancel_one_body.clone()),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let left = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cancel_one_path,
        Some("outbound-qa-recovery-cancel-left"),
        Some(cancel_one_body.clone()),
    );
    let right = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cancel_one_path,
        Some("outbound-qa-recovery-cancel-right"),
        Some(cancel_one_body.clone()),
    );
    let (left, right) = tokio::join!(left, right);
    let (winner, winner_key) = match (left.status(), right.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (left, "outbound-qa-recovery-cancel-left"),
        (StatusCode::CONFLICT, StatusCode::OK) => (right, "outbound-qa-recovery-cancel-right"),
        statuses => panic!("expected one QA cancellation winner, got {statuses:?}"),
    };
    let cancelled_one: OutboundQaSessionResponse = response_json(winner).await;
    assert_eq!(cancelled_one.status, OutboundQaSessionStatus::Cancelled);
    assert_eq!(cancelled_one.attempt, 1);
    assert_eq!(cancelled_one.revision.get(), 3);
    assert_eq!(cancelled_one.verifications.len(), 1);
    let cancellation = cancelled_one
        .cancellation
        .as_ref()
        .expect("cancelled session carries immutable evidence");
    assert_eq!(cancellation.previous_status, OutboundQaSessionStatus::Open);
    assert_eq!(
        cancellation.reason,
        OutboundQaCancellationReason::PackingCorrection
    );
    let replayed: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &cancel_one_path,
                Some(winner_key),
                Some(cancel_one_body.clone()),
            )
            .await,
            StatusCode::OK,
            "replay first QA cancellation",
        )
        .await,
    )
    .await;
    assert_eq!(replayed, cancelled_one);
    let mut changed_cancel = cancel_one_body;
    changed_cancel["reason"] = json!("quality_issue");
    assert_eq!(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &cancel_one_path,
            Some(winner_key),
            Some(changed_cancel),
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
            "read queue after QA cancellation",
        )
        .await,
    )
    .await;
    assert!(queue
        .items
        .iter()
        .find(|entry| entry.order_id == ready.order_id)
        .is_some_and(|entry| entry.outbound_qa_session.is_none()));

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
                Some("outbound-qa-recovery-reopen"),
                Some(json!({
                    "carton_barcode": ready.carton_barcodes[0],
                    "expected_revision": ready.order_revision,
                    "reason": "quality_issue",
                    "note": "Correct closure evidence after QA cancellation"
                })),
            )
            .await,
            StatusCode::OK,
            "reopen carton after cancelled QA",
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
                Some("outbound-qa-recovery-reclose"),
                Some(json!({
                    "carton_barcode": ready.carton_barcodes[0],
                    "measurements": {
                        "weight_grams": 1300,
                        "dimensions": {"length_mm": 305, "width_mm": 205, "height_mm": 155}
                    },
                    "expected_revision": reopened.revision
                })),
            )
            .await,
            StatusCode::OK,
            "reclose carton after QA cancellation",
        )
        .await,
    )
    .await;
    ready.order_revision = reclosed.revision.get();

    let attempt_two = start_and_pass_attempt(
        &app,
        &token,
        access.tenant_id,
        &start_path,
        &ready,
        2,
        "outbound-qa-recovery-attempt-2",
    )
    .await;
    let cancel_two_path = format!(
        "/api/v1/outbound-qa-sessions/{}/cancellations",
        attempt_two.session_id
    );
    let cancelled_two: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &cancel_two_path,
                Some("outbound-qa-recovery-cancel-passed"),
                Some(json!({
                    "expected_revision": attempt_two.revision,
                    "reason": "quality_issue",
                    "note": "Supervisor invalidated the passed attempt"
                })),
            )
            .await,
            StatusCode::OK,
            "cancel passed QA attempt",
        )
        .await,
    )
    .await;
    assert_eq!(cancelled_two.status, OutboundQaSessionStatus::Cancelled);
    assert_eq!(cancelled_two.attempt, 2);
    assert_eq!(
        cancelled_two
            .cancellation
            .as_ref()
            .map(|cancellation| cancellation.previous_status),
        Some(OutboundQaSessionStatus::Passed)
    );

    let attempt_three = start_and_pass_attempt(
        &app,
        &token,
        access.tenant_id,
        &start_path,
        &ready,
        3,
        "outbound-qa-recovery-attempt-3",
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
                Some("outbound-qa-recovery-shipment"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create shipment after replacement QA attempt",
        )
        .await,
    )
    .await;
    assert_eq!(created.shipment.cartons.len(), 2);
    assert_eq!(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!(
                "/api/v1/outbound-qa-sessions/{}/cancellations",
                attempt_three.session_id
            ),
            Some("outbound-qa-recovery-too-late"),
            Some(json!({
                "expected_revision": attempt_three.revision,
                "reason": "operator_error"
            })),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let admin = admin_db_for(&fixture.db).await;
    let evidence: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM outbound_qa_sessions WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM outbound_qa_cancellations WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM outbound_qa_carton_verifications WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM outbound_qa_completions WHERE tenant_id=$1),
          (SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1
             AND event_type='outbound.qa.cancelled')
        "#,
    )
    .bind(access.tenant_id.get())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(evidence, (3, 2, 5, 2, 2));
    assert!(sqlx::query(
        "UPDATE outbound_qa_cancellations SET reason_code=reason_code WHERE tenant_id=$1",
    )
    .bind(access.tenant_id.get())
    .execute(&admin)
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM outbound_qa_cancellations WHERE tenant_id=$1")
            .bind(access.tenant_id.get())
            .execute(&admin)
            .await
            .is_err()
    );
    admin.close().await;

    set_scope(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    for body in [
        json!({
            "expected_revision": attempt_two.revision,
            "reason": "quality_issue",
            "note": "Supervisor invalidated the passed attempt"
        }),
        json!({
            "expected_revision": attempt_two.revision,
            "reason": "policy_error"
        }),
    ] {
        assert_eq!(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &cancel_two_path,
                Some("outbound-qa-recovery-cancel-passed"),
                Some(body),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
}

async fn start_and_pass_attempt(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    start_path: &str,
    ready: &ReadyShipment,
    expected_attempt: i64,
    key: &str,
) -> OutboundQaSessionResponse {
    let started: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                app,
                token,
                tenant_id,
                Method::POST,
                start_path,
                Some(&format!("{key}-start")),
                Some(json!({"expected_order_revision": ready.order_revision})),
            )
            .await,
            StatusCode::OK,
            "start replacement QA attempt",
        )
        .await,
    )
    .await;
    assert_eq!(started.attempt, expected_attempt);
    let verify_path = format!(
        "/api/v1/outbound-qa-sessions/{}/carton-verifications",
        started.session_id
    );
    let mut revision = started.revision.get();
    for (index, barcode) in ready.carton_barcodes.iter().enumerate() {
        let verified: OutboundQaSessionResponse = response_json(
            expect_status(
                send(
                    app,
                    token,
                    tenant_id,
                    Method::POST,
                    &verify_path,
                    Some(&format!("{key}-verify-{index}")),
                    Some(json!({
                        "expected_revision": revision,
                        "carton_barcode": barcode
                    })),
                )
                .await,
                StatusCode::OK,
                "verify replacement QA carton",
            )
            .await,
        )
        .await;
        revision = verified.revision.get();
    }
    let passed: OutboundQaSessionResponse = response_json(
        expect_status(
            send(
                app,
                token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/outbound-qa-sessions/{}/completions",
                    started.session_id
                ),
                Some(&format!("{key}-complete")),
                Some(json!({"expected_revision": revision})),
            )
            .await,
            StatusCode::OK,
            "pass replacement QA attempt",
        )
        .await,
    )
    .await;
    assert_eq!(passed.status, OutboundQaSessionStatus::Passed);
    passed
}
