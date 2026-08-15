use super::*;

#[tokio::test]
async fn orchestration_scope_rls_grants_and_evidence_guards_fail_closed() {
    init_test_tracing();
    let rig = Rig::new().await;
    let policy: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-guard-policy"),
            Some(rig.policy_body("enabled", None)),
        )
        .await,
    )
    .await;
    let plan: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some("orchestration-guard-plan"),
            Some(rig.plan_body(policy.policy_id, policy.revision.get())),
        )
        .await,
    )
    .await;

    let other_user = rig
        .fixture
        .wms_user("orchestration-other-tenant@test.local")
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
        Method::GET,
        &format!("/api/v1/work-orchestration/plans/{}", plan.plan_id),
        None,
        None,
    )
    .await;
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);

    let mut unbound = rig.fixture.db.begin().await.unwrap();
    let unbound_counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT count(*) FROM work_orchestration_policies),
          (SELECT count(*) FROM work_orchestration_zone_signals),
          (SELECT count(*) FROM work_orchestration_resource_signals),
          (SELECT count(*) FROM work_orchestration_plans),
          (SELECT count(*) FROM work_orchestration_plan_items)"#,
    )
    .fetch_one(&mut *unbound)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0, 0, 0));
    unbound.rollback().await.unwrap();

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let immutable_error = sqlx::query(
        "UPDATE work_orchestration_plan_items SET title='forged' WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(plan.items[0].plan_item_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(
        immutable_error.to_string().contains("permission denied"),
        "unexpected runtime mutation error: {immutable_error}"
    );
    tx.rollback().await.unwrap();

    let admin = admin_db_for(&rig.fixture.db).await;
    let grants: Vec<bool> = sqlx::query_scalar(
        r#"SELECT ARRAY[
          has_table_privilege('wareboxes_app','work_orchestration_policies','SELECT'),
          has_table_privilege('wareboxes_app','work_orchestration_policies','INSERT'),
          has_column_privilege('wareboxes_app','work_orchestration_policies','effective_to','UPDATE'),
          has_table_privilege('wareboxes_app','work_orchestration_policies','DELETE'),
          has_table_privilege('wareboxes_app','work_orchestration_zone_signals','UPDATE'),
          has_table_privilege('wareboxes_app','work_orchestration_resource_signals','DELETE'),
          has_table_privilege('wareboxes_app','work_orchestration_plans','UPDATE'),
          has_table_privilege('wareboxes_app','work_orchestration_plan_items','DELETE'),
          has_sequence_privilege('wareboxes_app','work_orchestration_policies_id_seq','USAGE'),
          has_sequence_privilege('wareboxes_app','work_orchestration_zone_signals_id_seq','USAGE'),
          has_sequence_privilege('wareboxes_app','work_orchestration_resource_signals_id_seq','USAGE'),
          has_sequence_privilege('wareboxes_app','work_orchestration_plans_id_seq','USAGE'),
          has_sequence_privilege('wareboxes_app','work_orchestration_plan_items_id_seq','USAGE')]"#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        grants,
        vec![true, true, true, false, false, false, false, false, true, true, true, true, true]
    );

    let mut immutable = admin.begin().await.unwrap();
    let trigger_error = sqlx::query(
        "UPDATE work_orchestration_plan_items SET title='forged' WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(plan.items[0].plan_item_id)
    .execute(&mut *immutable)
    .await
    .unwrap_err();
    assert!(
        trigger_error.to_string().contains("immutable"),
        "unexpected immutable trigger error: {trigger_error}"
    );
    immutable.rollback().await.unwrap();

    let mut forged = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(rig.user_id.to_string())
        .execute(&mut *forged)
        .await
        .unwrap();
    let forged_plan_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO work_orchestration_plans OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(NULL::public.work_orchestration_plans,
          to_jsonb(original)||jsonb_build_object(
            'id',nextval('work_orchestration_plans_id_seq'),
            'candidate_count',1,'item_count',1))).*
        FROM work_orchestration_plans original
        WHERE original.tenant_id=$1 AND original.id=$2 RETURNING id"#,
    )
    .bind(rig.tenant_id.get())
    .bind(plan.plan_id)
    .fetch_one(&mut *forged)
    .await
    .unwrap();
    let forged_error = sqlx::query(
        r#"INSERT INTO work_orchestration_plan_items OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(NULL::public.work_orchestration_plan_items,
          to_jsonb(original)||jsonb_build_object(
            'id',nextval('work_orchestration_plan_items_id_seq'),
            'plan_id',$3,'sequence',1,'source_location_id',$4))).*
        FROM work_orchestration_plan_items original
        WHERE original.tenant_id=$1 AND original.id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(plan.items[0].plan_item_id)
    .bind(forged_plan_id)
    .bind(rig.current_location_id)
    .execute(&mut *forged)
    .await
    .unwrap_err();
    assert!(
        forged_error
            .to_string()
            .contains("invalid work orchestration plan item"),
        "unexpected forged evidence error: {forged_error}"
    );
    forged.rollback().await.unwrap();

    let mut capped = admin.begin().await.unwrap();
    bind_database_actor(&mut capped, rig.user_id).await;
    let cap_error = clone_plan_with_patch(
        &mut capped,
        rig.tenant_id,
        plan.plan_id,
        json!({"candidate_count":101,"item_count":101}),
    )
    .await
    .unwrap_err();
    assert!(
        cap_error
            .to_string()
            .contains("invalid work orchestration plan"),
        "unexpected policy candidate cap error: {cap_error}"
    );
    capped.rollback().await.unwrap();

    let mut scheduled = admin.begin().await.unwrap();
    bind_database_actor(&mut scheduled, rig.user_id).await;
    let scheduled_plan_id = clone_plan_with_patch(
        &mut scheduled,
        rig.tenant_id,
        plan.plan_id,
        json!({"candidate_count":1,"item_count":1}),
    )
    .await
    .unwrap();
    sqlx::query(
        r#"UPDATE work_tasks task SET scheduled_for=source.input_snapshot_at+INTERVAL '1 hour'
        FROM work_orchestration_plans source
        WHERE task.tenant_id=$1 AND task.id=$2
          AND source.tenant_id=$1 AND source.id=$3"#,
    )
    .bind(rig.tenant_id.get())
    .bind(plan.items[0].work_task_id)
    .bind(plan.plan_id)
    .execute(&mut *scheduled)
    .await
    .unwrap();
    let scheduled_error = sqlx::query(
        r#"INSERT INTO work_orchestration_plan_items OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(NULL::public.work_orchestration_plan_items,
          to_jsonb(original)||jsonb_build_object(
            'id',nextval('work_orchestration_plan_items_id_seq'),
            'plan_id',$3,'sequence',1))).*
        FROM work_orchestration_plan_items original
        WHERE original.tenant_id=$1 AND original.id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(plan.items[0].plan_item_id)
    .bind(scheduled_plan_id)
    .execute(&mut *scheduled)
    .await
    .unwrap_err();
    assert!(
        scheduled_error
            .to_string()
            .contains("invalid work orchestration plan item"),
        "unexpected future schedule evidence error: {scheduled_error}"
    );
    scheduled.rollback().await.unwrap();

    let mut future_created = admin.begin().await.unwrap();
    bind_database_actor(&mut future_created, rig.user_id).await;
    let future_created_plan_id = clone_plan_with_patch(
        &mut future_created,
        rig.tenant_id,
        plan.plan_id,
        json!({"candidate_count":1,"item_count":1}),
    )
    .await
    .unwrap();
    sqlx::query(
        r#"UPDATE work_tasks task SET created=source.input_snapshot_at+INTERVAL '1 hour'
        FROM work_orchestration_plans source
        WHERE task.tenant_id=$1 AND task.id=$2
          AND source.tenant_id=$1 AND source.id=$3"#,
    )
    .bind(rig.tenant_id.get())
    .bind(plan.items[0].work_task_id)
    .bind(plan.plan_id)
    .execute(&mut *future_created)
    .await
    .unwrap();
    let future_created_error = sqlx::query(
        r#"INSERT INTO work_orchestration_plan_items OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(NULL::public.work_orchestration_plan_items,
          to_jsonb(original)||jsonb_build_object(
            'id',nextval('work_orchestration_plan_items_id_seq'),
            'plan_id',$3,'sequence',1,'task_created_at',task.created))).*
        FROM work_orchestration_plan_items original
        JOIN work_tasks task ON task.tenant_id=original.tenant_id
          AND task.id=original.work_task_id
        WHERE original.tenant_id=$1 AND original.id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(plan.items[0].plan_item_id)
    .bind(future_created_plan_id)
    .execute(&mut *future_created)
    .await
    .unwrap_err();
    assert!(
        future_created_error
            .to_string()
            .contains("invalid work orchestration plan item"),
        "unexpected future creation evidence error: {future_created_error}"
    );
    future_created.rollback().await.unwrap();

    let mut sparse = admin.begin().await.unwrap();
    bind_database_actor(&mut sparse, rig.user_id).await;
    let sparse_plan_id = clone_plan_with_patch(&mut sparse, rig.tenant_id, plan.plan_id, json!({}))
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO work_orchestration_plan_items OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(NULL::public.work_orchestration_plan_items,
          to_jsonb(original)||jsonb_build_object(
            'id',nextval('work_orchestration_plan_items_id_seq'),'plan_id',$3,
            'sequence',CASE WHEN original.sequence=2 THEN 3 ELSE original.sequence END))).*
        FROM work_orchestration_plan_items original
        WHERE original.tenant_id=$1 AND original.plan_id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(plan.plan_id)
    .bind(sparse_plan_id)
    .execute(&mut *sparse)
    .await
    .unwrap();
    let sparse_error = sparse.commit().await.unwrap_err();
    assert!(
        sparse_error.to_string().contains("sequence is not dense"),
        "unexpected sparse sequence error: {sparse_error}"
    );

    let mut misordered = admin.begin().await.unwrap();
    bind_database_actor(&mut misordered, rig.user_id).await;
    let misordered_plan_id =
        clone_plan_with_patch(&mut misordered, rig.tenant_id, plan.plan_id, json!({}))
            .await
            .unwrap();
    sqlx::query(
        r#"INSERT INTO work_orchestration_plan_items OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(NULL::public.work_orchestration_plan_items,
          to_jsonb(original)||jsonb_build_object(
            'id',nextval('work_orchestration_plan_items_id_seq'),
            'plan_id',$3,'sequence',3-original.sequence))).*
        FROM work_orchestration_plan_items original
        WHERE original.tenant_id=$1 AND original.plan_id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(plan.plan_id)
    .bind(misordered_plan_id)
    .execute(&mut *misordered)
    .await
    .unwrap();
    let misordered_error = misordered.commit().await.unwrap_err();
    assert!(
        misordered_error.to_string().contains("not score ordered"),
        "unexpected optimized order error: {misordered_error}"
    );

    let unscoped_user = rig
        .fixture
        .user("orchestration-unscoped-worker@test.local")
        .await;
    let mut unscoped_worker = admin.begin().await.unwrap();
    sqlx::query(
        r#"INSERT INTO tenant_memberships
          (tenant_id,user_id,is_default,all_facilities,all_inventory_owners)
        VALUES ($1,$2,false,false,true)"#,
    )
    .bind(rig.tenant_id.get())
    .bind(unscoped_user.id)
    .execute(&mut *unscoped_worker)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO user_roles (tenant_id,created,user_id,role_id)
        SELECT $1,transaction_timestamp(),$2,role.id FROM roles role
        JOIN role_permissions role_permission ON role_permission.tenant_id=role.tenant_id
          AND role_permission.role_id=role.id AND role_permission.deleted IS NULL
        JOIN permissions permission ON permission.tenant_id=role_permission.tenant_id
          AND permission.id=role_permission.permission_id AND permission.deleted IS NULL
        WHERE role.tenant_id=$1 AND role.deleted IS NULL AND lower(permission.name)='wms'
        ORDER BY role.id LIMIT 1"#,
    )
    .bind(rig.tenant_id.get())
    .bind(unscoped_user.id)
    .execute(&mut *unscoped_worker)
    .await
    .unwrap();
    let employee_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO employees (
          tenant_id,created,user_id,first_name,last_name,title,type,hired,
          identity_revision,identity_changed_by_user_id,identity_changed_at
        ) VALUES ($1,transaction_timestamp(),$2,'Unscoped','Worker','Operator','test',
          transaction_timestamp()-INTERVAL '1 day',1,$3,transaction_timestamp())
        RETURNING id"#,
    )
    .bind(rig.tenant_id.get())
    .bind(unscoped_user.id)
    .bind(rig.user_id)
    .fetch_one(&mut *unscoped_worker)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO employee_facilities
          (tenant_id,created,employee_id,facility_id)
        VALUES ($1,transaction_timestamp(),$2,$3)"#,
    )
    .bind(rig.tenant_id.get())
    .bind(employee_id)
    .bind(rig.facility_id)
    .execute(&mut *unscoped_worker)
    .await
    .unwrap();
    bind_database_actor(&mut unscoped_worker, rig.user_id).await;
    let unscoped_error = clone_plan_with_patch(
        &mut unscoped_worker,
        rig.tenant_id,
        plan.plan_id,
        json!({"generated_for_user_id":unscoped_user.id}),
    )
    .await
    .unwrap_err();
    assert!(
        unscoped_error
            .to_string()
            .contains("invalid work orchestration plan"),
        "unexpected generated-for scope error: {unscoped_error}"
    );
    unscoped_worker.rollback().await.unwrap();

    let mut deleted_actor = admin.begin().await.unwrap();
    sqlx::query(
        "UPDATE tenant_memberships SET deleted=transaction_timestamp() WHERE tenant_id=$1 AND user_id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(rig.user_id)
    .execute(&mut *deleted_actor)
    .await
    .unwrap();
    bind_database_actor(&mut deleted_actor, rig.user_id).await;
    let deleted_actor_error = sqlx::query(
        r#"INSERT INTO work_orchestration_resource_signals (
          tenant_id,facility_id,resource_kind,available_units,demand_units,
          utilization_basis_points,ttl_seconds,recorded_by_user_id,observed_at,expires_at
        ) VALUES ($1,$2,'automation',$3,$3,10000,60,$4,
          transaction_timestamp(),transaction_timestamp()+INTERVAL '60 seconds')"#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.facility_id)
    .bind(i64::MAX)
    .bind(rig.user_id)
    .execute(&mut *deleted_actor)
    .await
    .unwrap_err();
    assert!(
        deleted_actor_error
            .to_string()
            .contains("invalid work orchestration resource signal"),
        "unexpected deleted actor error: {deleted_actor_error}"
    );
    deleted_actor.rollback().await.unwrap();

    let disabled: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-guard-policy-disable"),
            Some(rig.policy_body("disabled", Some(policy.revision.get()))),
        )
        .await,
    )
    .await;
    let manual: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some("orchestration-guard-manual-plan"),
            Some(rig.plan_body(disabled.policy_id, disabled.revision.get())),
        )
        .await,
    )
    .await;
    let mut manual_misordered = admin.begin().await.unwrap();
    bind_database_actor(&mut manual_misordered, rig.user_id).await;
    let manual_misordered_plan_id = clone_plan_with_patch(
        &mut manual_misordered,
        rig.tenant_id,
        manual.plan_id,
        json!({}),
    )
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO work_orchestration_plan_items OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(NULL::public.work_orchestration_plan_items,
          to_jsonb(original)||jsonb_build_object(
            'id',nextval('work_orchestration_plan_items_id_seq'),
            'plan_id',$3,'sequence',3-original.sequence))).*
        FROM work_orchestration_plan_items original
        WHERE original.tenant_id=$1 AND original.plan_id=$2"#,
    )
    .bind(rig.tenant_id.get())
    .bind(manual.plan_id)
    .bind(manual_misordered_plan_id)
    .execute(&mut *manual_misordered)
    .await
    .unwrap();
    let manual_order_error = manual_misordered.commit().await.unwrap_err();
    assert!(
        manual_order_error.to_string().contains("not FIFO ordered"),
        "unexpected manual FIFO order error: {manual_order_error}"
    );

    admin.close().await;
}

#[tokio::test]
async fn concurrent_policy_supersession_has_one_winner() {
    init_test_tracing();
    let rig = Rig::new().await;
    let initial: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-race-initial"),
            Some(rig.policy_body("enabled", None)),
        )
        .await,
    )
    .await;
    let body = rig.policy_body("disabled", Some(initial.revision.get()));
    let (first, second) = tokio::join!(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-race-a"),
            Some(body.clone())
        ),
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-race-b"),
            Some(body)
        )
    );
    assert_eq!(
        usize::from(first.status() == StatusCode::OK)
            + usize::from(second.status() == StatusCode::OK),
        1
    );
    assert!(first.status() == StatusCode::CONFLICT || second.status() == StatusCode::CONFLICT);
    let page: WorkOrchestrationPolicyPage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/work-orchestration/policies?facility_id={}&include_history=true",
                rig.facility_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].revision.get(), 2);
    assert_eq!(
        page.items[1].effective_to,
        page.items[0].effective_from.clone().into()
    );
}
