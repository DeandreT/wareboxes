use super::*;

impl Rig {
    async fn arrived_inbound_load(&self, suffix: &str, quantity: i64) -> i64 {
        let location_scan = format!("RECEIVE-{suffix}");
        let receiving_location_id = wareboxes_persistence_postgres::locations::add_location(
            &self.fixture.db,
            self.tenant_id,
            self.facility_id,
            None,
            Some(&location_scan),
            Some(&location_scan),
            "dock",
            true,
            false,
            true,
        )
        .await
        .unwrap();
        let item_id = self
            .fixture
            .item(self.tenant_id, &format!("Labor item {suffix}"), "each")
            .await;
        let mut tx = tenant_tx(&self.fixture.db, self.tenant_id).await;
        sqlx::query(
            r#"INSERT INTO inventory_owner_items
               (tenant_id,created,inventory_owner_id,item_id) VALUES($1,$2,$3,$4)"#,
        )
        .bind(self.tenant_id.get())
        .bind(db::now_iso())
        .bind(self.owner_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        repo::items::add_barcode(
            &self.fixture.db,
            self.tenant_id,
            item_id,
            &format!("LABOR-ITEM-{suffix}"),
            "code128",
            None,
        )
        .await
        .unwrap();
        let access =
            repo::tenants::access_for_user(&self.fixture.db, self.supervisor_id, self.tenant_id)
                .await
                .unwrap()
                .unwrap();
        let plan = wareboxes_domain::NewInboundLoadPlan::new(
            wareboxes_domain::InventoryOwnerId::new(self.owner_id).unwrap(),
            wareboxes_domain::FacilityId::new(self.facility_id).unwrap(),
            wareboxes_domain::LocationId::new(receiving_location_id).unwrap(),
            wareboxes_domain::InboundLoadReference::new(format!("LABOR-{suffix}")).unwrap(),
            None,
            None,
            None,
            None,
            None,
            vec![wareboxes_domain::InboundLoadPlanLine::new(
                wareboxes_domain::CatalogItemId::new(item_id).unwrap(),
                wareboxes_domain::InboundExpectedQuantity::new(quantity).unwrap(),
                None,
                None,
                None,
            )
            .unwrap()],
        )
        .unwrap();
        let planned = repo::inbound_load::plan_inbound_load(
            &self.fixture.db,
            &access,
            &wareboxes_application::CommandContext {
                tenant_id: self.tenant_id,
                actor_id: access.user_id,
                request_id: format!("labor-load-plan-{suffix}"),
                idempotency_key: Some(format!("labor-load-plan-{suffix}")),
            },
            &wareboxes_application::inbound_load::PlanInboundLoadCommand::new(plan),
        )
        .await
        .unwrap();
        repo::inbound_load::arrive_inbound_load(
            &self.fixture.db,
            &access,
            &wareboxes_application::CommandContext {
                tenant_id: self.tenant_id,
                actor_id: access.user_id,
                request_id: format!("labor-load-arrive-{suffix}"),
                idempotency_key: Some(format!("labor-load-arrive-{suffix}")),
            },
            &wareboxes_application::inbound_load::ArriveInboundLoadCommand::new(
                planned.load_id,
                wareboxes_domain::InboundLoadScanValue::new(planned.execution_barcode).unwrap(),
                wareboxes_domain::InboundLoadScanValue::new(location_scan).unwrap(),
                None,
            ),
        )
        .await
        .unwrap();
        planned.load_id.get()
    }
}

#[tokio::test]
async fn immutable_corrections_are_replay_safe_reversible_and_drive_effective_reporting() {
    let rig = Rig::new("corrections").await;
    let load_id = rig.arrived_inbound_load("corrections", 5).await;
    let attendance = rig.clock_in_operator("labor-correction-clock-in").await;
    let started: LaborActivityResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            "/api/v1/labor/activities",
            Some("labor-correction-start"),
            Some(&StartLaborActivityRequest {
                attendance_interval_id: attendance.attendance_interval_id,
                inventory_owner_id: Some(rig.owner_id),
                activity_kind: LaborActivityKind::Receiving,
                quantity_basis: Some(LaborQuantityBasis::Unit),
                labor_standard_id: None,
                equipment_asset_id: None,
                reference_type: Some("inbound_load".into()),
                reference_id: Some(load_id),
                note: Some("Unload first portion".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let completed: LaborActivityResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/completions",
                started.labor_activity_id
            ),
            Some("labor-correction-complete"),
            Some(&CompleteLaborActivityRequest {
                expected_revision: started.revision,
                quantity: Some(3),
                exception_seconds: 0,
                exception_reason: None,
                exception_note: None,
                note: Some("Initial device-reported completion".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let closed: AttendanceIntervalResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/clock-outs",
                attendance.attendance_interval_id
            ),
            Some("labor-correction-clock-out"),
            Some(&ClockOutRequest {
                expected_revision: attendance.revision,
                note: Some("Initial punch".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;

    let original_clock_in: wareboxes_domain::Timestamp = closed.clocked_in_at.parse().unwrap();
    let original_clock_out: wareboxes_domain::Timestamp =
        closed.clocked_out_at.as_deref().unwrap().parse().unwrap();
    let attendance_request = CorrectAttendanceRequest {
        expected_revision: closed.effective_revision,
        corrected_clocked_in_at: (original_clock_in - Duration::from_secs(60)).to_rfc3339(),
        corrected_clocked_out_at: original_clock_out.to_rfc3339(),
        reason: LaborCorrectionReason::MissedPunch,
        note: "Employee began unloading one minute before the recorded punch".into(),
    };
    let operator_forbidden = rig
        .send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/corrections",
                closed.attendance_interval_id
            ),
            Some("labor-correction-operator-forbidden"),
            Some(&attendance_request),
        )
        .await;
    assert_status(operator_forbidden, StatusCode::FORBIDDEN).await;

    let attendance_adjustment: AttendanceAdjustmentResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/corrections",
                closed.attendance_interval_id
            ),
            Some("labor-attendance-correction"),
            Some(&attendance_request),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        attendance_adjustment.expected_revision,
        closed.effective_revision
    );
    assert_eq!(attendance_adjustment.resulting_revision.get(), 3);
    assert!(attendance_adjustment.supersedes_adjustment_id.is_none());
    assert_eq!(
        attendance_adjustment.before_clocked_in_at,
        closed.clocked_in_at
    );
    assert_eq!(attendance_adjustment.adjusted_by, rig.supervisor_id);
    let replay: AttendanceAdjustmentResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/corrections",
                closed.attendance_interval_id
            ),
            Some("labor-attendance-correction"),
            Some(&attendance_request),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, attendance_adjustment);
    let mut reused_attendance = attendance_request.clone();
    reused_attendance.note = "Different correction under the same key".into();
    assert_status(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/corrections",
                closed.attendance_interval_id
            ),
            Some("labor-attendance-correction"),
            Some(&reused_attendance),
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;

    let original_started_at: wareboxes_domain::Timestamp = completed.started_at.parse().unwrap();
    let original_completed_at: wareboxes_domain::Timestamp =
        completed.completed_at.as_deref().unwrap().parse().unwrap();
    let activity_request = CorrectLaborActivityRequest {
        expected_revision: completed.effective_revision,
        corrected_started_at: Some((original_started_at - Duration::from_secs(30)).to_rfc3339()),
        corrected_completed_at: Some(original_completed_at.to_rfc3339()),
        quantity: Some(4),
        exception_seconds: 1,
        exception_reason: Some(LaborExceptionReason::Congestion),
        exception_note: Some("Aisle access was temporarily blocked".into()),
        reason: LaborCorrectionReason::QuantityError,
        note: "Supervisor reconciled device quantity and start time".into(),
    };
    let activity_adjustment: LaborActivityAdjustmentResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/corrections",
                completed.labor_activity_id
            ),
            Some("labor-activity-correction"),
            Some(&activity_request),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(activity_adjustment.before_quantity, Some(3));
    assert_eq!(activity_adjustment.corrected_quantity, Some(4));
    assert_eq!(activity_adjustment.before_exception_seconds, 0);
    assert_eq!(activity_adjustment.corrected_exception_seconds, 1);
    assert_eq!(
        activity_adjustment.corrected_exception_approved_by,
        Some(rig.supervisor_id)
    );
    assert!(
        activity_adjustment.corrected_actual_seconds > activity_adjustment.before_actual_seconds
    );
    let activity_replay: LaborActivityAdjustmentResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/corrections",
                completed.labor_activity_id
            ),
            Some("labor-activity-correction"),
            Some(&activity_request),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(activity_replay, activity_adjustment);
    let over_cap = CorrectLaborActivityRequest {
        expected_revision: activity_adjustment.resulting_revision,
        quantity: Some(6),
        corrected_started_at: None,
        corrected_completed_at: None,
        exception_seconds: 1,
        exception_reason: Some(LaborExceptionReason::Congestion),
        exception_note: Some("Aisle access was temporarily blocked".into()),
        reason: LaborCorrectionReason::QuantityError,
        note: "Attempt to exceed source evidence".into(),
    };
    assert_status(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/corrections",
                completed.labor_activity_id
            ),
            Some("labor-activity-over-cap"),
            Some(&over_cap),
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;

    let activity_reversal: LaborActivityAdjustmentResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/corrections",
                completed.labor_activity_id
            ),
            Some("labor-activity-correction-reversal"),
            Some(&CorrectLaborActivityRequest {
                expected_revision: activity_adjustment.resulting_revision,
                corrected_started_at: Some(original_started_at.to_rfc3339()),
                corrected_completed_at: Some(original_completed_at.to_rfc3339()),
                quantity: Some(3),
                exception_seconds: 0,
                exception_reason: None,
                exception_note: None,
                reason: LaborCorrectionReason::Other,
                note: "Review restored the original device evidence".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        activity_reversal.supersedes_adjustment_id,
        Some(activity_adjustment.labor_activity_adjustment_id)
    );
    assert_eq!(activity_reversal.resulting_revision.get(), 4);
    assert_eq!(activity_reversal.corrected_quantity, Some(3));
    assert_eq!(activity_reversal.corrected_exception_seconds, 0);

    let attendance_reversal: AttendanceAdjustmentResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/corrections",
                closed.attendance_interval_id
            ),
            Some("labor-attendance-correction-reversal"),
            Some(&CorrectAttendanceRequest {
                expected_revision: attendance_adjustment.resulting_revision,
                corrected_clocked_in_at: original_clock_in.to_rfc3339(),
                corrected_clocked_out_at: original_clock_out.to_rfc3339(),
                reason: LaborCorrectionReason::Other,
                note: "Review restored the original punch evidence".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        attendance_reversal.supersedes_adjustment_id,
        Some(attendance_adjustment.attendance_adjustment_id)
    );
    assert_eq!(attendance_reversal.resulting_revision.get(), 4);

    let workspace: LaborWorkspaceResponse = json(
        rig.send::<serde_json::Value>(
            &rig.supervisor_token,
            Method::GET,
            &format!(
                "/api/v1/labor/workspace?facility_id={}&include_history=true",
                rig.facility_id
            ),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let effective_attendance = workspace
        .attendance
        .iter()
        .find(|row| row.attendance_interval_id == closed.attendance_interval_id)
        .unwrap();
    assert_eq!(effective_attendance.clocked_in_at, closed.clocked_in_at);
    assert_eq!(
        effective_attendance.effective_clocked_in_at,
        closed.clocked_in_at
    );
    assert_eq!(effective_attendance.effective_revision.get(), 4);
    let effective_activity = workspace
        .activities
        .iter()
        .find(|row| row.labor_activity_id == completed.labor_activity_id)
        .unwrap();
    assert_eq!(effective_activity.quantity, Some(3));
    assert_eq!(effective_activity.effective_quantity, Some(3));
    assert_eq!(effective_activity.effective_exception_seconds, Some(0));
    assert_eq!(effective_activity.effective_revision.get(), 4);
    assert_eq!(workspace.attendance_adjustments.len(), 2);
    assert_eq!(workspace.activity_adjustments.len(), 2);
    let summary = workspace
        .summaries
        .iter()
        .find(|row| row.employee_id == rig.operator_employee_id)
        .unwrap();
    assert_eq!(summary.exception_seconds, 0);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let original_activity: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT revision,completed_quantity,exception_seconds FROM labor_activities
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(completed.labor_activity_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let original_attendance: (i64, wareboxes_domain::Timestamp, i64) = sqlx::query_as(
        r#"SELECT revision,clocked_in_at,paid_seconds FROM attendance_intervals
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(closed.attendance_interval_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let attendance_correction_events: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM outbox_events
           WHERE tenant_id=$1 AND event_type='labor.attendance.corrected'
             AND aggregate_id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(closed.attendance_interval_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let activity_correction_events: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM outbox_events
           WHERE tenant_id=$1 AND event_type='labor.activity.corrected'
             AND aggregate_id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(completed.labor_activity_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(original_activity, (2, 3, 0));
    assert_eq!(original_attendance.0, 2);
    assert_eq!(original_attendance.1, original_clock_in);
    assert_eq!(original_attendance.2, closed.paid_seconds.unwrap());
    assert_eq!(attendance_correction_events, 2);
    assert_eq!(activity_correction_events, 2);
}

#[tokio::test]
async fn concurrent_same_revision_attendance_corrections_have_one_winner() {
    let rig = Rig::new("correction-race").await;
    let attendance = rig
        .clock_in_operator("labor-correction-race-clock-in")
        .await;
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let closed: AttendanceIntervalResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/clock-outs",
                attendance.attendance_interval_id
            ),
            Some("labor-correction-race-clock-out"),
            Some(&ClockOutRequest {
                expected_revision: attendance.revision,
                note: None,
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let clock_in: wareboxes_domain::Timestamp = closed.clocked_in_at.parse().unwrap();
    let clock_out = closed.clocked_out_at.clone().unwrap();
    let first_request = CorrectAttendanceRequest {
        expected_revision: closed.effective_revision,
        corrected_clocked_in_at: (clock_in - Duration::from_secs(30)).to_rfc3339(),
        corrected_clocked_out_at: clock_out.clone(),
        reason: LaborCorrectionReason::TimekeepingError,
        note: "First concurrent supervisor correction".into(),
    };
    let second_request = CorrectAttendanceRequest {
        corrected_clocked_in_at: (clock_in - Duration::from_secs(45)).to_rfc3339(),
        note: "Second concurrent supervisor correction".into(),
        ..first_request.clone()
    };
    let correction_path = format!(
        "/api/v1/labor/attendance/{}/corrections",
        closed.attendance_interval_id
    );
    let first = rig.send(
        &rig.supervisor_token,
        Method::POST,
        &correction_path,
        Some("labor-correction-race-a"),
        Some(&first_request),
    );
    let second = rig.send(
        &rig.supervisor_token,
        Method::POST,
        &correction_path,
        Some("labor-correction-race-b"),
        Some(&second_request),
    );
    let (first_response, second_response) = tokio::join!(first, second);
    let (first_status, first_body) = response(first_response).await;
    let (second_status, second_body) = response(second_response).await;
    assert_eq!(
        [first_status, second_status]
            .into_iter()
            .filter(|status| *status == StatusCode::OK)
            .count(),
        1,
        "one correction wins: first={}, second={}",
        String::from_utf8_lossy(&first_body),
        String::from_utf8_lossy(&second_body)
    );
    assert_eq!(
        [first_status, second_status]
            .into_iter()
            .filter(|status| *status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM attendance_adjustments WHERE tenant_id=$1 AND attendance_interval_id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(closed.attendance_interval_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn concurrent_double_start_has_one_winner_and_one_equipment_assignment() {
    let rig = Rig::new("race").await;
    let configured = rig.configure("race").await;
    let attendance = rig.clock_in_operator("labor-clock-in-race").await;
    let task_id = rig.assigned_location_count_task("race").await;

    let first_request = StartLaborActivityRequest {
        attendance_interval_id: attendance.attendance_interval_id,
        inventory_owner_id: None,
        activity_kind: LaborActivityKind::CycleCount,
        quantity_basis: Some(LaborQuantityBasis::Task),
        labor_standard_id: Some(configured.standard.labor_standard_id),
        equipment_asset_id: Some(configured.asset.equipment_asset_id),
        reference_type: Some("work_task".into()),
        reference_id: Some(task_id),
        note: Some("First concurrent activity".into()),
    };
    let second_request = StartLaborActivityRequest {
        note: Some("Second concurrent activity".into()),
        ..first_request.clone()
    };

    let first = rig.send(
        &rig.operator_token,
        Method::POST,
        "/api/v1/labor/activities",
        Some("labor-start-race-a"),
        Some(&first_request),
    );
    let second = rig.send(
        &rig.operator_token,
        Method::POST,
        "/api/v1/labor/activities",
        Some("labor-start-race-b"),
        Some(&second_request),
    );
    let (first_response, second_response) = tokio::join!(first, second);
    let (first_status, first_body) = response(first_response).await;
    let (second_status, second_body) = response(second_response).await;
    assert_eq!(
        [first_status, second_status]
            .into_iter()
            .filter(|status| *status == StatusCode::OK)
            .count(),
        1,
        "exactly one labor start wins: first={}, second={}",
        String::from_utf8_lossy(&first_body),
        String::from_utf8_lossy(&second_body)
    );
    assert_eq!(
        [first_status, second_status]
            .into_iter()
            .filter(|status| *status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let winning_body = if first_status == StatusCode::OK {
        &first_body
    } else {
        &second_body
    };
    let winner: LaborActivityResponse = serde_json::from_slice(winning_body).unwrap();
    assert_eq!(winner.status, LaborActivityStatus::Active);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM labor_activities WHERE tenant_id=$1 AND employee_id=$2 AND status='active'",
    )
    .bind(rig.tenant_id.get())
    .bind(rig.operator_employee_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let equipment: (String, Option<i64>) = sqlx::query_as(
        "SELECT status,assigned_employee_id FROM equipment_assets WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(configured.asset.equipment_asset_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let started_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1 AND event_type='labor.activity.started'",
    )
    .bind(rig.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(active_count, 1);
    assert_eq!(
        equipment,
        ("assigned".into(), Some(rig.operator_employee_id))
    );
    assert_eq!(started_events, 1);

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let cancelled: LaborActivityResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/cancellations",
                winner.labor_activity_id
            ),
            Some("labor-race-cleanup"),
            Some(&CancelLaborActivityRequest {
                expected_revision: winner.revision,
                note: "Concurrency acceptance cleanup".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(cancelled.status, LaborActivityStatus::Cancelled);
}
