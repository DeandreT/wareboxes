use super::*;
use wareboxes_api_contract::v1::{
    DynamicReleaseReadinessResponse, DynamicReleaseRunResponse, RunDynamicReleaseRequest,
};

async fn preview(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    facility_id: i64,
    owner_id: i64,
) -> DynamicReleaseReadinessResponse {
    json_response(
        expect(
            send(
                app,
                token,
                tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/dynamic-releases/readiness?facility_id={facility_id}&inventory_owner_id={owner_id}"
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "preview dynamic release",
        )
        .await,
    )
    .await
}

fn run_body(preview: &DynamicReleaseReadinessResponse, destination_location_id: i64) -> Value {
    serde_json::to_value(RunDynamicReleaseRequest {
        facility_id: preview.facility_id,
        inventory_owner_id: preview.inventory_owner_id,
        destination_location_id,
        expected_policy: preview.policy.expectation(),
    })
    .unwrap()
}

#[tokio::test]
async fn allocated_priority_queue_releases_atomically_and_replays_exactly() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("dynamic-release@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    grant_permissions(&fixture.db, access.tenant_id, user.id, "dynamic").await;
    let owner = fixture
        .inventory_owner(access.tenant_id, "Dynamic Release Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "Dynamic Release Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let destination = staging_location(&fixture, access.tenant_id, facility, "DYNAMIC-STAGE").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let standard = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "DYNAMIC-STANDARD",
        2,
    )
    .await;
    let rush = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "DYNAMIC-RUSH",
        3,
    )
    .await;
    let preview = preview(&app, &token, access.tenant_id, facility, owner).await;
    assert_eq!(preview.eligible_order_count, 2);
    assert_eq!(preview.selected_order_count, 2);
    assert_eq!(preview.deferred_order_count, 0);
    assert_eq!(preview.selected_orders[0].order_id, standard.0);
    assert!(!preview.selected_orders[0].rush);
    assert_eq!(preview.selected_orders[1].order_id, rush.0);
    assert_eq!(preview.selected_orders[0].rank, 1);
    assert_eq!(preview.selected_orders[1].rank, 2);

    let body = run_body(&preview, destination);
    let first: DynamicReleaseRunResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/dynamic-releases",
                Some("dynamic-release-once"),
                Some(body.clone()),
            )
            .await,
            StatusCode::OK,
            "run dynamic release",
        )
        .await,
    )
    .await;
    let replay: DynamicReleaseRunResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/dynamic-releases",
                Some("dynamic-release-once"),
                Some(body.clone()),
            )
            .await,
            StatusCode::OK,
            "replay dynamic release",
        )
        .await,
    )
    .await;
    assert_eq!(replay, first);
    assert_eq!(first.selected_orders[0].order_id, standard.0);
    let wave = first
        .wave
        .as_ref()
        .expect("allocated orders produce a wave");
    assert_eq!(wave.status, PickWaveStatus::Released);
    assert_eq!(wave.order_count, 2);
    assert_eq!(wave.released_quantity, 5);

    let changed_destination =
        staging_location(&fixture, access.tenant_id, facility, "DYNAMIC-STAGE-2").await;
    let changed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/dynamic-releases",
        Some("dynamic-release-once"),
        Some(run_body(&preview, changed_destination)),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_response::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (String, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT run.status,run.eligible_order_count,run.selected_order_count,
          (SELECT count(*) FROM dynamic_release_candidates candidate
           WHERE candidate.tenant_id=run.tenant_id AND candidate.dynamic_release_run_id=run.id),
          (SELECT count(*) FROM orders order_header
           WHERE order_header.tenant_id=run.tenant_id
             AND order_header.id=ANY($3) AND order_header.status='processing'),
          (SELECT count(*) FROM outbox_events event
           WHERE event.tenant_id=run.tenant_id AND event.aggregate_type='dynamic_release'
             AND event.aggregate_id=run.id::text)
        FROM dynamic_release_runs run WHERE run.tenant_id=$1 AND run.id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(first.run_id)
    .bind([standard.0, rush.0])
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, ("sealed".into(), 2, 2, 2, 2, 1));

    let admin = admin_db_for(&fixture.db).await;
    let tamper = sqlx::query(
        "UPDATE dynamic_release_candidates SET selection_rank=99 WHERE tenant_id=$1 AND dynamic_release_run_id=$2 AND selection_rank=1",
    )
    .bind(access.tenant_id.get())
    .bind(first.run_id)
    .execute(&admin)
    .await
    .unwrap_err();
    assert_eq!(
        tamper
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55000")
    );
    admin.close().await;

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        access.tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: false,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap());
    let concealed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/dynamic-releases",
        Some("dynamic-release-once"),
        Some(body),
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn concurrent_exact_run_has_one_effect_set_and_empty_queue_is_audited() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("dynamic-release-race@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    grant_permissions(&fixture.db, access.tenant_id, user.id, "dynamic-race").await;
    let owner = fixture
        .inventory_owner(access.tenant_id, "Dynamic Race Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "Dynamic Race Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let destination =
        staging_location(&fixture, access.tenant_id, facility, "DYNAMIC-RACE-STAGE").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "DYNAMIC-RACE-ORDER",
        4,
    )
    .await;
    let first_preview = preview(&app, &token, access.tenant_id, facility, owner).await;
    let body = run_body(&first_preview, destination);
    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/dynamic-releases",
        Some("dynamic-release-race"),
        Some(body.clone()),
    );
    let retry = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/dynamic-releases",
        Some("dynamic-release-race"),
        Some(body),
    );
    let (first, retry) = tokio::join!(first, retry);
    let first: DynamicReleaseRunResponse =
        json_response(expect(first, StatusCode::OK, "concurrent first").await).await;
    let retry: DynamicReleaseRunResponse =
        json_response(expect(retry, StatusCode::OK, "concurrent retry").await).await;
    assert_eq!(first, retry);

    let empty_preview = preview(&app, &token, access.tenant_id, facility, owner).await;
    assert_eq!(empty_preview.eligible_order_count, 0);
    let empty: DynamicReleaseRunResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/dynamic-releases",
                Some("dynamic-release-empty"),
                Some(run_body(&empty_preview, destination)),
            )
            .await,
            StatusCode::OK,
            "audit empty dynamic release",
        )
        .await,
    )
    .await;
    assert_eq!(empty.selected_order_count, 0);
    assert!(empty.wave.is_none());

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT count(*) FROM dynamic_release_runs),
          (SELECT count(*) FROM pick_waves),
          (SELECT count(*) FROM order_releases),
          (SELECT count(*) FROM command_idempotency_records
           WHERE operation='outbound.dynamic_release.run.v1')"#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (2, 1, 1, 2));
}

#[tokio::test]
async fn policy_change_invalidates_preview_and_configured_cap_defers_lower_priority_work() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("dynamic-policy@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_permissions(&fixture.db, access.tenant_id, operator.id, "dynamic-policy").await;
    super::policy::grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "admin",
        "dynamic-policy-manage",
    )
    .await;
    let approver = fixture.user("dynamic-policy-approver@test.local").await;
    super::policy::add_membership(&fixture, access.tenant_id, approver.id).await;
    super::policy::grant_permission(
        &fixture,
        access.tenant_id,
        approver.id,
        "admin",
        "dynamic-policy-approve",
    )
    .await;
    let owner = fixture
        .inventory_owner(access.tenant_id, "Dynamic Policy Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "Dynamic Policy Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let destination =
        staging_location(&fixture, access.tenant_id, facility, "DYNAMIC-POLICY-STAGE").await;
    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let approver_token = auth::create_session(&fixture.db, approver.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first = allocated_order(
        &fixture,
        &app,
        &operator_token,
        &access,
        owner,
        facility,
        "DYNAMIC-POLICY-A",
        2,
    )
    .await;
    let second = allocated_order(
        &fixture,
        &app,
        &operator_token,
        &access,
        owner,
        facility,
        "DYNAMIC-POLICY-B",
        3,
    )
    .await;
    let stale = preview(&app, &operator_token, access.tenant_id, facility, owner).await;
    assert_eq!(stale.selected_order_count, 2);

    super::policy::activate_policy(
        &app,
        &operator_token,
        &approver_token,
        access.tenant_id,
        owner,
        facility,
        1,
        true,
        None,
        "dynamic-cap",
    )
    .await;
    let stale_run = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        "/api/v1/dynamic-releases",
        Some("dynamic-stale-policy"),
        Some(run_body(&stale, destination)),
    )
    .await;
    assert_eq!(stale_run.status(), StatusCode::CONFLICT);

    let current = preview(&app, &operator_token, access.tenant_id, facility, owner).await;
    assert_eq!(current.policy.max_orders, 1);
    assert_eq!(current.eligible_order_count, 2);
    assert_eq!(current.selected_order_count, 1);
    assert_eq!(current.deferred_order_count, 1);
    assert_eq!(current.selected_orders[0].order_id, first.0);
    let result: DynamicReleaseRunResponse = json_response(
        expect(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                "/api/v1/dynamic-releases",
                Some("dynamic-current-policy"),
                Some(run_body(&current, destination)),
            )
            .await,
            StatusCode::OK,
            "run capped dynamic release",
        )
        .await,
    )
    .await;
    assert_eq!(result.selected_order_count, 1);
    assert_eq!(result.deferred_order_count, 1);
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let states: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id,status FROM orders WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id",
    )
    .bind(access.tenant_id.get())
    .bind([first.0, second.0])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        states,
        vec![(first.0, "processing".into()), (second.0, "open".into())]
    );
}
