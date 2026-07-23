mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::dto::{ErrorCode, ErrorResponse, UpdateUserAccessScope};
use wareboxes_core::models::WorkTask;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_server::{routes, state::AppState};

fn request(
    token: &str,
    tenant_id: TenantId,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(idempotency_key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(token, tenant_id, uri, idempotency_key, body))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn create_twice(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    uri: &str,
    idempotency_key: &str,
    body: Value,
) -> i64 {
    let (first, second) = tokio::join!(
        send(
            app,
            token,
            tenant_id,
            uri,
            Some(idempotency_key),
            body.clone(),
        ),
        send(app, token, tenant_id, uri, Some(idempotency_key), body,),
    );
    assert_eq!(first.status(), StatusCode::OK, "{uri}");
    assert_eq!(second.status(), StatusCode::OK, "{uri}");
    let first_id = response_json::<i64>(first).await;
    let second_id = response_json::<i64>(second).await;
    assert_eq!(first_id, second_id, "{uri}");
    first_id
}

async fn create_break_task(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    facility_id: i64,
) -> i64 {
    let location = fixture
        .location(tenant_id, facility_id, "IDEMPOTENCY-PACK")
        .await;
    let master = fixture.item(tenant_id, "Idempotency Case", "case").await;
    let single = fixture.item(tenant_id, "Idempotency Each", "each").await;
    repo::items::add_item_pack_link(
        &fixture.db,
        tenant_id,
        master,
        single,
        12,
        Some("idempotency pack"),
    )
    .await
    .unwrap();
    repo::tasks::create_break_master_pack_task(
        &fixture.db,
        tenant_id,
        user_id,
        master,
        single,
        location,
        5,
        None,
        None,
        None,
        None,
        Some("replay-safe pack work".into()),
    )
    .await
    .unwrap()
}

async fn grant_wms(db: &db::Db, tenant_id: TenantId, user_id: i64, suffix: &str) {
    let permission = repo::permissions::find_by_name(db, tenant_id, "wms")
        .await
        .unwrap()
        .unwrap();
    let role = repo::roles::add_role(db, tenant_id, &format!("wms-{suffix}"), None)
        .await
        .unwrap();
    repo::roles::add_role_permission(db, tenant_id, role, permission.id)
        .await
        .unwrap();
    repo::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

#[tokio::test]
async fn task_commands_replay_results_without_repeating_work() {
    let fixture = Fixture::new().await;
    let worker = fixture.wms_user("task-idempotency@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, worker.id).await;
    let token = auth::create_session(&fixture.db, worker.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let facility = fixture.facility(tenant_id, "Idempotency DC").await;
    let task_id = create_break_task(&fixture, tenant_id, worker.id, facility).await;
    let task_location: i64 = sqlx::query_scalar(
        "SELECT location_id FROM break_master_pack_tasks WHERE tenant_id = $1 AND task_id = $2",
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_one(&fixture.db)
    .await
    .unwrap();

    let missing_key = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start",
        None,
        json!({"task_id": task_id}),
    )
    .await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.code,
        ErrorCode::IdempotencyKeyRequired
    );

    for _ in 0..2 {
        let started = send(
            &app,
            &token,
            tenant_id,
            "/api/tasks/start",
            Some("start-pack-1"),
            json!({"task_id": task_id}),
        )
        .await;
        assert_eq!(started.status(), StatusCode::OK);
        assert!(response_json::<bool>(started).await);
    }
    let started_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_task_progress WHERE tenant_id = $1 AND task_id = $2 AND action = 'started'",
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(started_events, 1);

    let progress_body = json!({
        "task_id": task_id,
        "action": "progress",
        "qty_completed": 2,
        "from_location_id": task_location
    });
    let (first_progress, second_progress) = tokio::join!(
        send(
            &app,
            &token,
            tenant_id,
            "/api/tasks/progress",
            Some("progress-pack-1"),
            progress_body.clone(),
        ),
        send(
            &app,
            &token,
            tenant_id,
            "/api/tasks/progress",
            Some("progress-pack-1"),
            progress_body,
        ),
    );
    for response in [first_progress, second_progress] {
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_json::<bool>(response).await);
    }
    let progress_state: (i64, i64) = sqlx::query_as(
        r#"
        SELECT detail.master_qty_completed, COUNT(progress.id)
        FROM break_master_pack_tasks detail
        LEFT JOIN work_task_progress progress
          ON progress.tenant_id = detail.tenant_id
         AND progress.task_id = detail.task_id
         AND progress.action = 'progress'
        WHERE detail.tenant_id = $1 AND detail.task_id = $2
        GROUP BY detail.master_qty_completed
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(progress_state, (2, 1));

    sqlx::query("UPDATE locations SET active = FALSE WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(task_location)
        .execute(&fixture.db)
        .await
        .unwrap();
    let replay_after_location_change = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/progress",
        Some("progress-pack-1"),
        json!({
            "task_id": task_id,
            "action": "progress",
            "qty_completed": 2,
            "from_location_id": task_location
        }),
    )
    .await;
    assert_eq!(replay_after_location_change.status(), StatusCode::OK);
    assert!(response_json::<bool>(replay_after_location_change).await);

    let changed = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/progress",
        Some("progress-pack-1"),
        json!({
            "task_id": task_id,
            "action": "progress",
            "qty_completed": 1,
            "from_location_id": task_location
        }),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.code,
        ErrorCode::IdempotencyKeyReused
    );

    let completion_body = json!({"task_id": task_id, "qty_completed": 3});
    let (first_completion, second_completion) = tokio::join!(
        send(
            &app,
            &token,
            tenant_id,
            "/api/tasks/complete",
            Some("complete-pack-1"),
            completion_body.clone(),
        ),
        send(
            &app,
            &token,
            tenant_id,
            "/api/tasks/complete",
            Some("complete-pack-1"),
            completion_body,
        ),
    );
    for response in [first_completion, second_completion] {
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_json::<bool>(response).await);
    }
    let completed_state: (i64, String, i64, i64) = sqlx::query_as(
        r#"
        SELECT detail.master_qty_completed, task.status,
               COUNT(progress.id) FILTER (WHERE progress.action = 'progress'),
               COUNT(progress.id) FILTER (WHERE progress.action = 'completed')
        FROM break_master_pack_tasks detail
        INNER JOIN work_tasks task
          ON task.tenant_id = detail.tenant_id AND task.id = detail.task_id
        LEFT JOIN work_task_progress progress
          ON progress.tenant_id = task.tenant_id AND progress.task_id = task.id
        WHERE task.tenant_id = $1 AND task.id = $2
        GROUP BY detail.master_qty_completed, task.status
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(completed_state, (5, "completed".into(), 2, 1));

    let location = fixture
        .location(tenant_id, facility, "IDEMPOTENCY-COUNT")
        .await;
    let next_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        worker.id,
        location,
        None,
        None,
        None,
        None,
        Some("replay-safe next task".into()),
    )
    .await
    .unwrap();
    let start_next_body = json!({"task_type": "cycle_count_location"});
    let (first_next, second_next) = tokio::join!(
        send(
            &app,
            &token,
            tenant_id,
            "/api/tasks/start-next",
            Some("start-next-1"),
            start_next_body.clone(),
        ),
        send(
            &app,
            &token,
            tenant_id,
            "/api/tasks/start-next",
            Some("start-next-1"),
            start_next_body,
        ),
    );
    assert_eq!(first_next.status(), StatusCode::OK);
    assert_eq!(second_next.status(), StatusCode::OK);
    let first_task = response_json::<Option<WorkTask>>(first_next).await.unwrap();
    let second_task = response_json::<Option<WorkTask>>(second_next)
        .await
        .unwrap();
    assert_eq!(first_task, second_task);
    assert_eq!(first_task.id, next_task);
    let next_started_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_task_progress WHERE tenant_id = $1 AND task_id = $2 AND action = 'started'",
    )
    .bind(tenant_id.get())
    .bind(next_task)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(next_started_events, 1);

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: worker.id,
            all_facilities: false,
            facility_ids: Vec::new(),
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let revoked_replay = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start-next",
        Some("start-next-1"),
        json!({"task_type": "cycle_count_location"}),
    )
    .await;
    assert_eq!(revoked_replay.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json::<ErrorResponse>(revoked_replay).await.code,
        ErrorCode::NotFound
    );

    let revoked_explicit_start = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start",
        Some("start-pack-1"),
        json!({"task_id": task_id}),
    )
    .await;
    assert_eq!(revoked_explicit_start.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json::<ErrorResponse>(revoked_explicit_start)
            .await
            .code,
        ErrorCode::NotFound
    );

    let peer = fixture.user("task-idempotency-peer@test.com").await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(peer.id)
        .execute(&fixture.db)
        .await
        .unwrap();
    grant_wms(&fixture.db, tenant_id, peer.id, "idempotency-peer").await;
    let peer_token = auth::create_session(&fixture.db, peer.id).await.unwrap();
    let actor_mismatch = send(
        &app,
        &peer_token,
        tenant_id,
        "/api/tasks/start-next",
        Some("start-next-1"),
        json!({"task_type": "cycle_count_location"}),
    )
    .await;
    assert_eq!(actor_mismatch.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(actor_mismatch).await.code,
        ErrorCode::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let stored_metadata: (i64, String) = sqlx::query_as(
        r#"
        SELECT actor_user_id, request_id
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = 'task.progress.v1' AND idempotency_key = 'progress-pack-1'
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(stored_metadata.0, worker.id);
    assert!(!stored_metadata.1.is_empty());
}

#[tokio::test]
async fn idempotency_keys_are_tenant_scoped_and_records_are_immutable() {
    let fixture = Fixture::new().await;
    let first = fixture.wms_user("task-idempotency-a@test.com").await;
    let second = fixture.wms_user("task-idempotency-b@test.com").await;
    let first_tenant = tenant_for_user(&fixture.db, first.id).await;
    let second_tenant = tenant_for_user(&fixture.db, second.id).await;
    let first_token = auth::create_session(&fixture.db, first.id).await.unwrap();
    let second_token = auth::create_session(&fixture.db, second.id).await.unwrap();
    let first_facility = fixture.facility(first_tenant, "First Replay DC").await;
    let second_facility = fixture.facility(second_tenant, "Second Replay DC").await;
    let first_location = fixture
        .location(first_tenant, first_facility, "FIRST-REPLAY")
        .await;
    let second_location = fixture
        .location(second_tenant, second_facility, "SECOND-REPLAY")
        .await;
    let first_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        first_tenant,
        first.id,
        first_location,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let second_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        second_tenant,
        second.id,
        second_location,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let body = json!({"task_type": "cycle_count_location"});
    let first_response = send(
        &app,
        &first_token,
        first_tenant,
        "/api/tasks/start-next",
        Some("shared-tenant-key"),
        body.clone(),
    )
    .await;
    let second_response = send(
        &app,
        &second_token,
        second_tenant,
        "/api/tasks/start-next",
        Some("shared-tenant-key"),
        body,
    )
    .await;
    assert_eq!(
        response_json::<Option<WorkTask>>(first_response)
            .await
            .unwrap()
            .id,
        first_task
    );
    assert_eq!(
        response_json::<Option<WorkTask>>(second_response)
            .await
            .unwrap()
            .id,
        second_task
    );

    for (tenant_id, actor_user_id) in [(first_tenant, first.id), (second_tenant, second.id)] {
        let mut tx = tenant_tx(&fixture.db, tenant_id).await;
        let stored_actors: Vec<(i64, i64)> = sqlx::query_as(
            r#"
            SELECT tenant_id, actor_user_id
            FROM command_idempotency_records
            WHERE operation = 'task.start_next.v1'
              AND idempotency_key = 'shared-tenant-key'
            ORDER BY tenant_id
            "#,
        )
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(stored_actors, vec![(tenant_id.get(), actor_user_id)]);
    }

    let mut tx = tenant_tx(&fixture.db, first_tenant).await;
    let early_delete = sqlx::query(
        r#"
        DELETE FROM command_idempotency_records
        WHERE tenant_id = $1
          AND operation = 'task.start_next.v1'
          AND idempotency_key = 'shared-tenant-key'
        "#,
    )
    .bind(first_tenant.get())
    .execute(&mut *tx)
    .await;
    tx.rollback().await.unwrap();
    assert!(early_delete.is_err());
}

#[tokio::test]
async fn new_task_claims_release_expired_leases_atomically() {
    let fixture = Fixture::new().await;
    let worker = fixture.wms_user("task-expired-lease@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, worker.id).await;
    let token = auth::create_session(&fixture.db, worker.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let facility = fixture.facility(tenant_id, "Lease DC").await;

    let first_location = fixture.location(tenant_id, facility, "LEASE-01").await;
    let first_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        worker.id,
        first_location,
        None,
        None,
        None,
        None,
        Some("first leased task".into()),
    )
    .await
    .unwrap();
    let first_start = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start",
        Some("start-expiring-task"),
        json!({"task_id": first_task}),
    )
    .await;
    assert_eq!(first_start.status(), StatusCode::OK);

    sqlx::query(
        "UPDATE work_tasks SET lease_expires_at = statement_timestamp() - INTERVAL '1 second' WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(first_task)
    .execute(&fixture.db)
    .await
    .unwrap();
    let second_location = fixture.location(tenant_id, facility, "LEASE-02").await;
    repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        worker.id,
        second_location,
        None,
        None,
        None,
        None,
        Some("next leased task".into()),
    )
    .await
    .unwrap();

    let next_response = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start-next",
        Some("claim-after-expiry"),
        json!({"task_type": "cycle_count_location"}),
    )
    .await;
    assert_eq!(next_response.status(), StatusCode::OK);
    let claimed = response_json::<Option<WorkTask>>(next_response)
        .await
        .unwrap();
    let first_release_count: i64 =
        sqlx::query_scalar("SELECT release_count FROM work_tasks WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id.get())
            .bind(first_task)
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(first_release_count, 1);

    sqlx::query(
        "UPDATE work_tasks SET lease_expires_at = statement_timestamp() - INTERVAL '1 second' WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(claimed.id)
    .execute(&fixture.db)
    .await
    .unwrap();
    let target_location = fixture.location(tenant_id, facility, "LEASE-03").await;
    let target_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        worker.id,
        target_location,
        None,
        None,
        None,
        None,
        Some("explicit task after expiry".into()),
    )
    .await
    .unwrap();
    let explicit_response = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start",
        Some("explicit-after-expiry"),
        json!({"task_id": target_task}),
    )
    .await;
    assert_eq!(explicit_response.status(), StatusCode::OK);
    assert!(response_json::<bool>(explicit_response).await);

    let active_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM work_tasks
        WHERE tenant_id = $1
          AND assigned_user_id = $2
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
        "#,
    )
    .bind(tenant_id.get())
    .bind(worker.id)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(active_count, 1);
}

#[tokio::test]
async fn task_creation_and_lease_release_commands_are_replay_safe() {
    let fixture = Fixture::new().await;
    let worker = fixture.wms_user("task-creation-idempotency@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, worker.id).await;
    let token = auth::create_session(&fixture.db, worker.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let facility = fixture.facility(tenant_id, "Creation Replay DC").await;
    let location = fixture
        .location(tenant_id, facility, "CREATE-REPLAY-01")
        .await;
    let master = fixture
        .item(tenant_id, "Creation Replay Case", "case")
        .await;
    let single = fixture
        .item(tenant_id, "Creation Replay Each", "each")
        .await;
    repo::items::add_item_pack_link(
        &fixture.db,
        tenant_id,
        master,
        single,
        8,
        Some("creation replay pack"),
    )
    .await
    .unwrap();
    let owner = fixture
        .inventory_owner(tenant_id, "Creation Replay Owner")
        .await;
    sqlx::query(
        "INSERT INTO inventory_owner_facilities (tenant_id, created, inventory_owner_id, facility_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(owner)
    .bind(facility)
    .execute(&fixture.db)
    .await
    .unwrap();
    let order = fixture.order(tenant_id, "CREATE-REPLAY-ORDER", owner).await;
    fixture.order_item(order, single, 3).await;
    sqlx::query("UPDATE orders SET status = 'cancelled' WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(order)
        .execute(&fixture.db)
        .await
        .unwrap();

    let cases = [
        (
            "/api/tasks/cycle-counts/item-location/add",
            json!({"location_id": location, "item_id": single, "source": "manual"}),
        ),
        (
            "/api/tasks/cycle-counts/location/add",
            json!({"location_id": location, "priority": 31}),
        ),
        (
            "/api/tasks/break-master-packs/add",
            json!({
                "master_item_id": master,
                "single_item_id": single,
                "location_id": location,
                "qty": 2
            }),
        ),
        (
            "/api/tasks/unpack-cancelled-orders/add",
            json!({"order_id": order, "facility_id": facility}),
        ),
        ("/api/tasks/release-expired", json!({})),
    ];
    for (uri, body) in &cases {
        let response = send(&app, &token, tenant_id, uri, None, body.clone()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(
            response_json::<ErrorResponse>(response).await.code,
            ErrorCode::IdempotencyKeyRequired,
            "{uri}"
        );
    }

    let shared_key = "shared-create-operation-key";
    let item_task = create_twice(
        &app,
        &token,
        tenant_id,
        cases[0].0,
        shared_key,
        cases[0].1.clone(),
    )
    .await;
    let location_task = create_twice(
        &app,
        &token,
        tenant_id,
        cases[1].0,
        shared_key,
        cases[1].1.clone(),
    )
    .await;
    let break_task = create_twice(
        &app,
        &token,
        tenant_id,
        cases[2].0,
        shared_key,
        cases[2].1.clone(),
    )
    .await;
    let unpack_task = create_twice(
        &app,
        &token,
        tenant_id,
        cases[3].0,
        shared_key,
        cases[3].1.clone(),
    )
    .await;

    for (uri, body) in [
        (
            cases[0].0,
            json!({"location_id": location, "item_id": single, "source": "changed"}),
        ),
        (cases[1].0, json!({"location_id": location, "priority": 32})),
        (
            cases[2].0,
            json!({
                "master_item_id": master,
                "single_item_id": single,
                "location_id": location,
                "qty": 3
            }),
        ),
        (
            cases[3].0,
            json!({"order_id": order, "facility_id": facility, "priority": 71}),
        ),
    ] {
        let response = send(&app, &token, tenant_id, uri, Some(shared_key), body).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{uri}");
        assert_eq!(
            response_json::<ErrorResponse>(response).await.code,
            ErrorCode::IdempotencyKeyReused,
            "{uri}"
        );
    }

    let task_ids = vec![item_task, location_task, break_task, unpack_task];
    let task_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM work_tasks WHERE tenant_id = $1 AND id = ANY($2)")
            .bind(tenant_id.get())
            .bind(&task_ids)
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(task_count, 4);
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let command_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM command_idempotency_records WHERE tenant_id = $1 AND idempotency_key = $2 AND operation LIKE 'task.create_%'",
    )
    .bind(tenant_id.get())
    .bind(shared_key)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(command_count, 4);

    sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'in_progress', assigned_user_id = $1,
            started_at = statement_timestamp() - INTERVAL '2 minutes',
            lease_expires_at = statement_timestamp() - INTERVAL '1 minute'
        WHERE tenant_id = $2 AND id = $3
        "#,
    )
    .bind(worker.id)
    .bind(tenant_id.get())
    .bind(location_task)
    .execute(&fixture.db)
    .await
    .unwrap();
    let first_release_request = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/release-expired",
        Some("release-sweep-1"),
        json!({}),
    );
    let concurrent_replay_request = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/release-expired",
        Some("release-sweep-1"),
        json!({}),
    );
    let (first_release, concurrent_replay) =
        tokio::join!(first_release_request, concurrent_replay_request);
    assert_eq!(first_release.status(), StatusCode::OK);
    assert_eq!(response_json::<u64>(first_release).await, 1);
    assert_eq!(concurrent_replay.status(), StatusCode::OK);
    assert_eq!(response_json::<u64>(concurrent_replay).await, 1);
    let first_expired_progress: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_task_progress WHERE tenant_id = $1 AND task_id = $2 AND action = 'expired'",
    )
    .bind(tenant_id.get())
    .bind(location_task)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(first_expired_progress, 1);

    let late_location = fixture
        .location(tenant_id, facility, "CREATE-REPLAY-02")
        .await;
    let late_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        worker.id,
        late_location,
        None,
        None,
        None,
        None,
        Some("later expired work".into()),
    )
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'in_progress', assigned_user_id = $1,
            started_at = statement_timestamp() - INTERVAL '2 minutes',
            lease_expires_at = statement_timestamp() - INTERVAL '1 minute'
        WHERE tenant_id = $2 AND id = $3
        "#,
    )
    .bind(worker.id)
    .bind(tenant_id.get())
    .bind(late_task)
    .execute(&fixture.db)
    .await
    .unwrap();
    let release_replay = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/release-expired",
        Some("release-sweep-1"),
        json!({}),
    )
    .await;
    assert_eq!(release_replay.status(), StatusCode::OK);
    assert_eq!(response_json::<u64>(release_replay).await, 1);
    let late_status: String =
        sqlx::query_scalar("SELECT status FROM work_tasks WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id.get())
            .bind(late_task)
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(late_status, "in_progress");

    let second_release = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/release-expired",
        Some("release-sweep-2"),
        json!({}),
    )
    .await;
    assert_eq!(second_release.status(), StatusCode::OK);
    assert_eq!(response_json::<u64>(second_release).await, 1);
    let expired_progress: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_task_progress WHERE tenant_id = $1 AND task_id = ANY($2) AND action = 'expired'",
    )
    .bind(tenant_id.get())
    .bind(vec![location_task, late_task])
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(expired_progress, 2);

    sqlx::query("UPDATE work_tasks SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(db::now_iso())
        .bind(tenant_id.get())
        .bind(break_task)
        .execute(&fixture.db)
        .await
        .unwrap();
    let deleted_replay = send(
        &app,
        &token,
        tenant_id,
        cases[2].0,
        Some(shared_key),
        cases[2].1.clone(),
    )
    .await;
    assert_eq!(deleted_replay.status(), StatusCode::OK);
    assert_eq!(response_json::<i64>(deleted_replay).await, break_task);

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: worker.id,
            all_facilities: false,
            facility_ids: Vec::new(),
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let revoked_replay = send(
        &app,
        &token,
        tenant_id,
        cases[1].0,
        Some(shared_key),
        cases[1].1.clone(),
    )
    .await;
    assert_eq!(revoked_replay.status(), StatusCode::NOT_FOUND);
}
