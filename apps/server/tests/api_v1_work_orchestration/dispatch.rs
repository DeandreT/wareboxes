use super::*;
use wareboxes_api_contract::v1::{
    WorkOrchestrationDispatchCancellationReason, WorkOrchestrationDispatchResponse,
    WorkOrchestrationDispatchStatus,
};

async fn make_supervisor_an_eligible_worker(rig: &Rig) -> i64 {
    let admin = admin_db_for(&rig.fixture.db).await;
    let mut tx = admin.begin().await.unwrap();
    let employee_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO employees (
          tenant_id,created,user_id,first_name,last_name,title,type,hired,
          identity_revision,identity_changed_by_user_id,identity_changed_at
        ) VALUES ($1,transaction_timestamp(),$2,'Dispatch','Worker',
          'Material handler','test',transaction_timestamp()-INTERVAL '1 day',
          1,$2,transaction_timestamp()) RETURNING id"#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.user_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO employee_facilities (tenant_id,created,employee_id,facility_id)
        VALUES ($1,transaction_timestamp(),$2,$3)"#,
    )
    .bind(rig.tenant_id.get())
    .bind(employee_id)
    .bind(rig.facility_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    employee_id
}

async fn configured_worker_plan(
    rig: &Rig,
    inventory_owner_id: i64,
    policy_key: &str,
    plan_key: &str,
) -> WorkOrchestrationPlanResponse {
    let mut policy_body = rig.policy_body("enabled", None);
    policy_body["inventory_owner_id"] = json!(inventory_owner_id);
    let policy: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some(policy_key),
            Some(policy_body),
        )
        .await,
    )
    .await;
    let mut body = rig.plan_body(policy.policy_id, policy.revision.get());
    body["inventory_owner_id"] = json!(inventory_owner_id);
    body["generated_for_user_id"] = json!(rig.user_id);
    let response = rig
        .send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some(plan_key),
            Some(body),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    json_response(response).await
}

async fn add_claimable_count_tasks(rig: &Rig, inventory_owner_id: i64) -> [i64; 2] {
    let access = default_tenant_for_user(&rig.fixture.db, rig.user_id)
        .await
        .unwrap();
    let item_id = rig
        .fixture
        .item(rig.tenant_id, "Dispatch count item", "each")
        .await;
    wareboxes_api::repo::items::add_barcode(
        &rig.fixture.db,
        rig.tenant_id,
        item_id,
        "DISPATCH-COUNT-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let first = rig
        .fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id: rig.facility_id,
                item_id,
                qty: 10,
                key: "DISPATCH-COUNT-A",
            },
        )
        .await;
    let second = rig
        .fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id: rig.facility_id,
                item_id,
                qty: 12,
                key: "DISPATCH-COUNT-B",
            },
        )
        .await;
    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let mut task_ids = [0_i64; 2];
    for (index, balance) in [first, second].into_iter().enumerate() {
        let task_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO work_tasks (
              tenant_id,facility_id,inventory_owner_id,created,task_type,status,
              required_permission,priority,title,due_at,task_timeout_seconds)
            VALUES ($1,$2,$3,transaction_timestamp()-make_interval(mins=>$4),
              'cycle_count_item_location','open','wms',$5,$6,
              transaction_timestamp()+INTERVAL '1 hour',1800) RETURNING id"#,
        )
        .bind(rig.tenant_id.get())
        .bind(rig.facility_id)
        .bind(inventory_owner_id)
        .bind(i32::try_from(index + 1).unwrap())
        .bind(i64::try_from(index + 1).unwrap())
        .bind(format!("Dispatch count {}", index + 1))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO cycle_count_item_location_tasks (
              tenant_id,task_id,facility_id,inventory_owner_id,location_id,item_id,
              inventory_balance_id,source)
            VALUES ($1,$2,$3,$4,$5,$6,$7,'work_orchestration_dispatch')"#,
        )
        .bind(rig.tenant_id.get())
        .bind(task_id)
        .bind(rig.facility_id)
        .bind(inventory_owner_id)
        .bind(balance.location_id)
        .bind(item_id)
        .bind(balance.balance_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        task_ids[index] = task_id;
    }
    tx.commit().await.unwrap();
    task_ids
}

#[tokio::test]
async fn worker_dispatch_reserves_advances_cancels_and_replays_atomically() {
    init_test_tracing();
    let rig = Rig::new().await;
    make_supervisor_an_eligible_worker(&rig).await;
    let inventory_owner_id = rig
        .fixture
        .inventory_owner(rig.tenant_id, "Dispatch client")
        .await;
    rig.fixture
        .assign_owner_to_facility(rig.tenant_id, inventory_owner_id, rig.facility_id)
        .await;
    let claimable_tasks = add_claimable_count_tasks(&rig, inventory_owner_id).await;
    let plan =
        configured_worker_plan(&rig, inventory_owner_id, "dispatch-policy", "dispatch-plan").await;
    assert_eq!(plan.item_count, 2);

    let activation_path = format!(
        "/api/v1/work-orchestration/plans/{}/dispatches",
        plan.plan_id
    );
    let activation = rig
        .send(
            Method::POST,
            &activation_path,
            Some("dispatch-activate"),
            Some(json!({})),
        )
        .await;
    assert_eq!(activation.status(), StatusCode::OK);
    let active: WorkOrchestrationDispatchResponse = json_response(activation).await;
    assert_eq!(active.status, WorkOrchestrationDispatchStatus::Active);
    assert_eq!(active.revision.get(), 1);
    assert_eq!(active.worker_user_id, rig.user_id);
    assert_eq!(active.remaining_item_count, 2);
    assert_eq!(active.cancelled_item_count, 0);
    let first_task_id = active.current_work_task_id.unwrap();
    assert!(claimable_tasks.contains(&first_task_id));
    assert_eq!(active.current_sequence, Some(1));

    let mut tamper = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    bind_database_actor(&mut tamper, rig.user_id).await;
    let tamper_error = sqlx::query(
        r#"UPDATE work_orchestration_dispatch_items
        SET state='completed',released_at=transaction_timestamp()
        WHERE tenant_id=$1 AND dispatch_id=$2 AND sequence=2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(active.dispatch_id)
    .execute(&mut *tamper)
    .await
    .unwrap_err();
    assert!(tamper_error.to_string().contains("immutable"));
    tamper.rollback().await.unwrap();

    let other_user = rig
        .fixture
        .wms_user("dispatch-other-tenant@test.local")
        .await;
    let other_tenant = tenant_for_user(&rig.fixture.db, other_user.id).await;
    grant_supervisor(&rig.fixture, other_tenant, other_user.id).await;
    let other_token = wareboxes_api::auth::create_session(&rig.fixture.db, other_user.id)
        .await
        .unwrap();
    let guessed = send_request(
        rig.app.clone(),
        &other_token,
        other_tenant,
        Method::POST,
        &format!(
            "/api/v1/work-orchestration/dispatches/{}/cancellations",
            active.dispatch_id
        ),
        Some("dispatch-guessed-cancel"),
        Some(json!({"expected_revision":1,"reason":"operator_cancelled"})),
    )
    .await;
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);

    let replay: WorkOrchestrationDispatchResponse = json_response(
        rig.send(
            Method::POST,
            &activation_path,
            Some("dispatch-activate"),
            Some(json!({})),
        )
        .await,
    )
    .await;
    assert_eq!(replay, active);
    let duplicate = rig
        .send(
            Method::POST,
            &activation_path,
            Some("dispatch-activate-again"),
            Some(json!({})),
        )
        .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let refreshed: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::GET,
            &format!("/api/v1/work-orchestration/plans/{}", plan.plan_id),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(refreshed.dispatch.as_ref(), Some(&active));
    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let reservations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM work_orchestration_dispatch_items WHERE tenant_id=$1 AND dispatch_id=$2 AND state='reserved'",
    )
    .bind(rig.tenant_id.get())
    .bind(active.dispatch_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(reservations, 2);
    let first_state: (String, Option<i64>) = sqlx::query_as(
        "SELECT status,assigned_user_id FROM work_tasks WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(first_task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(first_state, ("assigned".to_owned(), Some(rig.user_id)));
    tx.commit().await.unwrap();

    let claimed = rig
        .send(
            Method::POST,
            &format!("/api/v1/cycle-count-claims/{first_task_id}"),
            Some("dispatch-claim-first"),
            Some(json!({})),
        )
        .await;
    assert_eq!(claimed.status(), StatusCode::OK);
    let blocked_cancel = rig
        .send(
            Method::POST,
            &format!(
                "/api/v1/work-orchestration/dispatches/{}/cancellations",
                active.dispatch_id
            ),
            Some("dispatch-cancel-in-progress"),
            Some(json!({"expected_revision":1,"reason":"worker_unavailable"})),
        )
        .await;
    assert_eq!(blocked_cancel.status(), StatusCode::CONFLICT);

    let mut terminal = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    bind_database_actor(&mut terminal, rig.user_id).await;
    sqlx::query(
        r#"UPDATE work_tasks SET status='cancelled',assigned_user_id=NULL,
          lease_expires_at=NULL,completed_by=$1,completed_at=transaction_timestamp(),
          modified=transaction_timestamp() WHERE tenant_id=$2 AND id=$3"#,
    )
    .bind(rig.user_id)
    .bind(rig.tenant_id.get())
    .bind(first_task_id)
    .execute(&mut *terminal)
    .await
    .unwrap();
    terminal.commit().await.unwrap();

    let advanced: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::GET,
            &format!("/api/v1/work-orchestration/plans/{}", plan.plan_id),
            None,
            None,
        )
        .await,
    )
    .await;
    let advanced = advanced.dispatch.unwrap();
    assert_eq!(advanced.status, WorkOrchestrationDispatchStatus::Active);
    assert_eq!(advanced.current_sequence, Some(2));
    assert_eq!(advanced.remaining_item_count, 1);
    assert_eq!(advanced.cancelled_item_count, 1);
    let second_task_id = advanced.current_work_task_id.unwrap();
    assert_ne!(second_task_id, first_task_id);

    let cancel_path = format!(
        "/api/v1/work-orchestration/dispatches/{}/cancellations",
        active.dispatch_id
    );
    let cancelled: WorkOrchestrationDispatchResponse = json_response(
        rig.send(
            Method::POST,
            &cancel_path,
            Some("dispatch-cancel"),
            Some(json!({"expected_revision":1,"reason":"operator_cancelled"})),
        )
        .await,
    )
    .await;
    assert_eq!(cancelled.status, WorkOrchestrationDispatchStatus::Cancelled);
    assert_eq!(cancelled.revision.get(), 2);
    assert_eq!(cancelled.remaining_item_count, 0);
    assert_eq!(cancelled.cancelled_item_count, 2);
    assert_eq!(cancelled.current_work_task_id, None);
    let cancel_replay: WorkOrchestrationDispatchResponse = json_response(
        rig.send(
            Method::POST,
            &cancel_path,
            Some("dispatch-cancel"),
            Some(json!({"expected_revision":1,"reason":"operator_cancelled"})),
        )
        .await,
    )
    .await;
    assert_eq!(cancel_replay, cancelled);

    let mut evidence = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let second_state: (String, Option<i64>) = sqlx::query_as(
        "SELECT status,assigned_user_id FROM work_tasks WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(second_task_id)
    .fetch_one(&mut *evidence)
    .await
    .unwrap();
    assert_eq!(second_state, ("open".to_owned(), None));
    let events: Vec<String> = sqlx::query_scalar(
        r#"SELECT event_type FROM outbox_events WHERE tenant_id=$1
        AND aggregate_type='work_orchestration_dispatch' AND aggregate_id=$2
        ORDER BY aggregate_sequence"#,
    )
    .bind(rig.tenant_id.get())
    .bind(active.dispatch_id.to_string())
    .fetch_all(&mut *evidence)
    .await
    .unwrap();
    assert_eq!(
        events,
        vec![
            "optimization.work_orchestration.dispatch.activated",
            "optimization.work_orchestration.dispatch.cancelled"
        ]
    );
    evidence.commit().await.unwrap();
}

#[tokio::test]
async fn concurrent_dispatch_activation_has_one_winner_and_one_reservation_set() {
    init_test_tracing();
    let rig = Rig::new().await;
    make_supervisor_an_eligible_worker(&rig).await;
    let inventory_owner_id = rig
        .fixture
        .inventory_owner(rig.tenant_id, "Concurrent dispatch client")
        .await;
    rig.fixture
        .assign_owner_to_facility(rig.tenant_id, inventory_owner_id, rig.facility_id)
        .await;
    add_claimable_count_tasks(&rig, inventory_owner_id).await;
    let mut policy_body = rig.policy_body("enabled", None);
    policy_body["inventory_owner_id"] = json!(inventory_owner_id);
    let policy: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("dispatch-race-policy"),
            Some(policy_body),
        )
        .await,
    )
    .await;
    let mut plan_body = rig.plan_body(policy.policy_id, policy.revision.get());
    plan_body["inventory_owner_id"] = json!(inventory_owner_id);
    plan_body["generated_for_user_id"] = json!(rig.user_id);
    let first_plan: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some("dispatch-race-plan-a"),
            Some(plan_body.clone()),
        )
        .await,
    )
    .await;
    let second_plan: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some("dispatch-race-plan-b"),
            Some(plan_body),
        )
        .await,
    )
    .await;
    let first_path = format!(
        "/api/v1/work-orchestration/plans/{}/dispatches",
        first_plan.plan_id
    );
    let second_path = format!(
        "/api/v1/work-orchestration/plans/{}/dispatches",
        second_plan.plan_id
    );
    let (first, second) = tokio::join!(
        rig.send(
            Method::POST,
            &first_path,
            Some("dispatch-race-activate-a"),
            Some(json!({})),
        ),
        rig.send(
            Method::POST,
            &second_path,
            Some("dispatch-race-activate-b"),
            Some(json!({})),
        )
    );
    let statuses = [first.status(), second.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let evidence: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT count(*) FROM work_orchestration_dispatches WHERE tenant_id=$1),
          (SELECT count(*) FROM work_orchestration_dispatch_items
            WHERE tenant_id=$1 AND state='reserved'),
          (SELECT count(*) FROM outbox_events WHERE tenant_id=$1
            AND aggregate_type='work_orchestration_dispatch'
            AND event_type='optimization.work_orchestration.dispatch.activated')"#,
    )
    .bind(rig.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 2, 1));
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn dispatch_records_plan_invalidation_after_terminal_work_without_session_actor_state() {
    init_test_tracing();
    let rig = Rig::new().await;
    make_supervisor_an_eligible_worker(&rig).await;
    let inventory_owner_id = rig
        .fixture
        .inventory_owner(rig.tenant_id, "Completed dispatch client")
        .await;
    rig.fixture
        .assign_owner_to_facility(rig.tenant_id, inventory_owner_id, rig.facility_id)
        .await;
    add_claimable_count_tasks(&rig, inventory_owner_id).await;
    let plan = configured_worker_plan(
        &rig,
        inventory_owner_id,
        "dispatch-complete-policy",
        "dispatch-complete-plan",
    )
    .await;
    let active: WorkOrchestrationDispatchResponse = json_response(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/work-orchestration/plans/{}/dispatches",
                plan.plan_id
            ),
            Some("dispatch-complete-activate"),
            Some(json!({})),
        )
        .await,
    )
    .await;
    for _ in 0..2 {
        let refreshed: WorkOrchestrationPlanResponse = json_response(
            rig.send(
                Method::GET,
                &format!("/api/v1/work-orchestration/plans/{}", plan.plan_id),
                None,
                None,
            )
            .await,
        )
        .await;
        let task_id = refreshed
            .dispatch
            .and_then(|dispatch| dispatch.current_work_task_id)
            .unwrap();
        let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
        sqlx::query(
            r#"UPDATE work_tasks SET status='cancelled',assigned_user_id=NULL,
              lease_expires_at=NULL,completed_by=$1,completed_at=transaction_timestamp(),
              modified=transaction_timestamp() WHERE tenant_id=$2 AND id=$3"#,
        )
        .bind(rig.user_id)
        .bind(rig.tenant_id.get())
        .bind(task_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let completed: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::GET,
            &format!("/api/v1/work-orchestration/plans/{}", plan.plan_id),
            None,
            None,
        )
        .await,
    )
    .await;
    let completed = completed.dispatch.unwrap();
    assert_eq!(completed.dispatch_id, active.dispatch_id);
    assert_eq!(completed.status, WorkOrchestrationDispatchStatus::Cancelled);
    assert_eq!(completed.revision.get(), 2);
    assert_eq!(completed.remaining_item_count, 0);
    assert_eq!(completed.cancelled_item_count, 2);
    assert_eq!(
        completed.cancellation_reason,
        Some(WorkOrchestrationDispatchCancellationReason::PlanInvalidated)
    );
    assert_eq!(completed.ended_by, Some(rig.user_id));
    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let events: Vec<String> = sqlx::query_scalar(
        r#"SELECT event_type FROM outbox_events WHERE tenant_id=$1
        AND aggregate_type='work_orchestration_dispatch' AND aggregate_id=$2
        ORDER BY aggregate_sequence"#,
    )
    .bind(rig.tenant_id.get())
    .bind(active.dispatch_id.to_string())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        events,
        vec![
            "optimization.work_orchestration.dispatch.activated",
            "optimization.work_orchestration.dispatch.cancelled"
        ]
    );
    tx.commit().await.unwrap();
}
