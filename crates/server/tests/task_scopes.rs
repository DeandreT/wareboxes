mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::WorkTask;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

fn api_request(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap()
}

async fn send_api(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(api_request(token, tenant_id, method, uri, body))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn cancel_order(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    order_id: i64,
    facility_id: i64,
) {
    let access = repo::tenants::access_for_user(db, user_id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    assert!(repo::orders::cancel_order_with_unpack_task(
        db,
        &access,
        user_id,
        order_id,
        facility_id,
    )
    .await
    .unwrap()
    .is_some());
}

#[tokio::test]
async fn work_task_routes_enforce_facility_and_owner_scopes() {
    let fixture = Fixture::new().await;
    let administrator = fixture.wms_user("task-scope-admin@test.com").await;
    let operator = fixture.user("task-scope-worker@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&fixture.db)
        .await
        .unwrap();
    let wms_permission = repo::permissions::find_by_name(&fixture.db, tenant_id, "wms")
        .await
        .unwrap()
        .unwrap();
    let operator_role = repo::roles::add_role(
        &fixture.db,
        tenant_id,
        "task-scope-worker-role",
        Some("Scoped task worker"),
    )
    .await
    .unwrap();
    repo::roles::add_role_permission(&fixture.db, tenant_id, operator_role, wms_permission.id)
        .await
        .unwrap();
    let orders_permission =
        repo::permissions::add_permission(&fixture.db, tenant_id, "orders", Some("Orders"))
            .await
            .unwrap();
    repo::roles::add_role_permission(&fixture.db, tenant_id, operator_role, orders_permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&fixture.db, tenant_id, operator.id, operator_role)
        .await
        .unwrap();

    let allowed_facility = fixture.facility(tenant_id, "Allowed Task DC").await;
    let cross_facility = fixture.facility(tenant_id, "Cross Task DC").await;
    let denied_facility = fixture.facility(tenant_id, "Denied Task DC").await;
    let allowed_location = fixture
        .location(tenant_id, allowed_facility, "TASK-ALLOWED")
        .await;
    let denied_location = fixture
        .location(tenant_id, denied_facility, "TASK-DENIED")
        .await;
    let cross_location = fixture
        .location(tenant_id, cross_facility, "TASK-CROSS")
        .await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Allowed Task Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Denied Task Owner")
        .await;
    for (owner_id, facility_id) in [
        (allowed_owner, allowed_facility),
        (denied_owner, denied_facility),
    ] {
        sqlx::query(
            "INSERT INTO inventory_owner_facilities (tenant_id, created, inventory_owner_id, facility_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(tenant_id.get())
        .bind(db::now_iso())
        .bind(owner_id)
        .bind(facility_id)
        .execute(&fixture.db)
        .await
        .unwrap();
    }
    let item = fixture.item(tenant_id, "Task Scope Item", "each").await;
    let second_item = fixture
        .item(tenant_id, "Second Task Scope Item", "each")
        .await;
    let master_item = fixture.item(tenant_id, "Task Scope Master", "case").await;
    repo::items::add_item_pack_link(
        &fixture.db,
        tenant_id,
        master_item,
        item,
        12,
        Some("task scope pack"),
    )
    .await
    .unwrap();

    let allowed_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        allowed_owner,
        item,
        None,
        Some("TASK-SCOPE-A"),
        None,
        None,
    )
    .await
    .unwrap();
    let second_allowed_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        allowed_owner,
        second_item,
        None,
        Some("TASK-SCOPE-B"),
        None,
        None,
    )
    .await
    .unwrap();
    for batch_id in [allowed_batch, second_allowed_batch] {
        repo::inventory::receive_inventory(
            &fixture.db,
            tenant_id,
            administrator.id,
            batch_id,
            allowed_location,
            5,
            None,
            Some("task scope setup"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }
    let balances = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap();
    let allowed_balance = balances
        .iter()
        .find(|balance| balance.item_batch_id == allowed_batch)
        .unwrap();
    let second_allowed_balance = balances
        .iter()
        .find(|balance| balance.item_batch_id == second_allowed_batch)
        .unwrap();

    let allowed_order = fixture
        .order(tenant_id, "TASK-SCOPE-ALLOWED-ORDER", allowed_owner)
        .await;
    fixture.order_item(allowed_order, item, 2).await;
    cancel_order(
        &fixture.db,
        tenant_id,
        administrator.id,
        allowed_order,
        allowed_facility,
    )
    .await;
    let denied_order = fixture
        .order(tenant_id, "TASK-SCOPE-DENIED-ORDER", denied_owner)
        .await;
    fixture.order_item(denied_order, item, 2).await;
    cancel_order(
        &fixture.db,
        tenant_id,
        administrator.id,
        denied_order,
        denied_facility,
    )
    .await;
    let allowed_owner_task = repo::tasks::create_unpack_cancelled_order_task(
        &fixture.db,
        tenant_id,
        Some(administrator.id),
        allowed_order,
        allowed_facility,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let denied_owner_task = repo::tasks::create_unpack_cancelled_order_task(
        &fixture.db,
        tenant_id,
        Some(administrator.id),
        denied_order,
        denied_facility,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let allowed_claim = repo::tasks::create_item_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        administrator.id,
        allowed_location,
        item,
        Some("scope test"),
        None,
        None,
        Some(allowed_balance.id),
        Some("claimable allowed task"),
    )
    .await
    .unwrap();
    let allowed_release = repo::tasks::create_item_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        administrator.id,
        allowed_location,
        second_item,
        Some("scope test"),
        None,
        None,
        Some(second_allowed_balance.id),
        Some("expired allowed task"),
    )
    .await
    .unwrap();
    let denied_open = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        administrator.id,
        denied_location,
        Some(300),
        None,
        None,
        None,
        Some("hidden denied task".to_owned()),
    )
    .await
    .unwrap();
    let owner_wide_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        administrator.id,
        allowed_location,
        Some(250),
        None,
        None,
        None,
        Some("all-owner task".to_owned()),
    )
    .await
    .unwrap();
    let denied_active = repo::tasks::create_break_master_pack_task(
        &fixture.db,
        tenant_id,
        administrator.id,
        master_item,
        item,
        denied_location,
        1,
        Some(400),
        Some(operator.id),
        None,
        None,
        Some("hidden active task".to_owned()),
    )
    .await
    .unwrap();
    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility, cross_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap();
    let released_on_scope_change: (String, Option<i64>, i64) = sqlx::query_as(
        r#"
        SELECT task.status, task.assigned_user_id, COUNT(progress.id)
        FROM work_tasks task
        LEFT JOIN work_task_progress progress
          ON progress.tenant_id = task.tenant_id
         AND progress.task_id = task.id
         AND progress.action = 'scope_revoked'
        WHERE task.tenant_id = $1 AND task.id = $2
        GROUP BY task.status, task.assigned_user_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(denied_active)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(released_on_scope_change, ("open".to_owned(), None, 1));

    let tasks = repo::tasks::get_tasks(&fixture.db, tenant_id, Default::default())
        .await
        .unwrap();
    assert_eq!(
        tasks
            .iter()
            .find(|task| task.id == allowed_claim)
            .unwrap()
            .facility_id,
        Some(allowed_facility)
    );

    let unqualified = fixture.user("task-no-permission@test.com").await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(unqualified.id)
        .execute(&fixture.db)
        .await
        .unwrap();
    assert!(
        !repo::tasks::assign_task(&fixture.db, tenant_id, allowed_claim, unqualified.id,)
            .await
            .unwrap()
    );

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let response = send_api(&app, &token, tenant_id, Method::GET, "/api/tasks", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let visible_tasks = response_json::<Vec<WorkTask>>(response).await;
    assert!(visible_tasks.iter().any(|task| task.id == allowed_claim));
    assert!(visible_tasks
        .iter()
        .any(|task| task.id == allowed_owner_task));
    assert!(visible_tasks.iter().all(|task| task.id != owner_wide_task));
    assert!(visible_tasks.iter().all(|task| {
        task.facility_id != Some(denied_facility) && task.inventory_owner_id != Some(denied_owner)
    }));

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/cycle-counts/location/add",
        Some(json!({"location_id": denied_location})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/unpack-cancelled-orders/add",
        Some(json!({"order_id": denied_order, "facility_id": denied_facility})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/orders/cancel",
        Some(json!({"order_id": allowed_order, "facility_id": allowed_facility})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json::<i64>(response).await, allowed_owner_task);
    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/orders/cancel",
        Some(json!({"order_id": denied_order, "facility_id": denied_facility})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        &format!("/api/tasks/unpack-cancelled-orders/lines?task_id={denied_owner_task}"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<Vec<Value>>(response).await.is_empty());

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/start-next",
        Some(json!({})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed = response_json::<Option<WorkTask>>(response).await.unwrap();
    assert_eq!(claimed.id, allowed_claim);
    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/complete",
        Some(json!({"task_id": allowed_claim})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let unpack_line_id: i64 = sqlx::query_scalar(
        "SELECT id FROM unpack_cancelled_order_task_lines WHERE tenant_id = $1 AND task_id = $2",
    )
    .bind(tenant_id.get())
    .bind(allowed_owner_task)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/start",
        Some(json!({"task_id": allowed_owner_task})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);
    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/progress",
        Some(json!({
            "task_id": allowed_owner_task,
            "task_line_id": unpack_line_id,
            "action": "unpacked",
            "qty_completed": 1,
            "from_location_id": cross_location
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/abort",
        Some(json!({"task_id": allowed_owner_task})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);

    for (uri, body) in [
        (
            "/api/tasks/assign",
            json!({"task_id": denied_open, "assigned_user_id": operator.id}),
        ),
        ("/api/tasks/start", json!({"task_id": denied_open})),
        ("/api/tasks/cancel", json!({"task_id": denied_open})),
    ] {
        let response = send_api(&app, &token, tenant_id, Method::POST, uri, Some(body)).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{uri}");
    }

    sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'in_progress', assigned_user_id = $1,
            started_at = CURRENT_TIMESTAMP - INTERVAL '2 minutes',
            lease_expires_at = CURRENT_TIMESTAMP - INTERVAL '1 minute'
        WHERE tenant_id = $2 AND id = $3
        "#,
    )
    .bind(operator.id)
    .bind(tenant_id.get())
    .bind(denied_active)
    .execute(&fixture.db)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'in_progress', started_at = CURRENT_TIMESTAMP - INTERVAL '2 minutes',
            lease_expires_at = CURRENT_TIMESTAMP - INTERVAL '1 minute'
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(allowed_release)
    .execute(&fixture.db)
    .await
    .unwrap();

    for (uri, body) in [
        (
            "/api/tasks/progress",
            json!({"task_id": denied_active, "action": "progress", "qty_completed": 1}),
        ),
        (
            "/api/tasks/complete",
            json!({"task_id": denied_active, "qty_completed": 1}),
        ),
        ("/api/tasks/abort", json!({"task_id": denied_active})),
    ] {
        let response = send_api(&app, &token, tenant_id, Method::POST, uri, Some(body)).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{uri}");
    }

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/release-expired",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json::<u64>(response).await, 1);
    let statuses: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, status FROM work_tasks WHERE tenant_id = $1 AND id = ANY($2) ORDER BY id",
    )
    .bind(tenant_id.get())
    .bind(vec![allowed_release, denied_active])
    .fetch_all(&fixture.db)
    .await
    .unwrap();
    assert!(statuses.contains(&(allowed_release, "open".to_owned())));
    assert!(statuses.contains(&(denied_active, "in_progress".to_owned())));

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/tasks/start-next",
        Some(json!({})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed = response_json::<Option<WorkTask>>(response).await.unwrap();
    assert_eq!(claimed.id, allowed_release);
    let revoked: (String, Option<i64>, i64) = sqlx::query_as(
        r#"
        SELECT task.status, task.assigned_user_id, COUNT(progress.id)
        FROM work_tasks task
        LEFT JOIN work_task_progress progress
          ON progress.tenant_id = task.tenant_id
         AND progress.task_id = task.id
         AND progress.action = 'scope_revoked'
        WHERE task.tenant_id = $1 AND task.id = $2
        GROUP BY task.status, task.assigned_user_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(denied_active)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(revoked, ("open".to_owned(), None, 2));

    assert!(
        sqlx::query("UPDATE work_tasks SET facility_id = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(denied_facility)
            .bind(tenant_id.get())
            .bind(allowed_claim)
            .execute(&fixture.db)
            .await
            .is_err()
    );

    let raw_task: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO work_tasks (
            tenant_id, facility_id, created, task_type, status, required_permission,
            priority, title, task_timeout_seconds
        )
        VALUES ($1, $2, CURRENT_TIMESTAMP, 'cycle_count_location', 'open', 'wms', 0, $3, 60)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(allowed_facility)
    .bind("scope constraint task")
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert!(sqlx::query(
        "INSERT INTO cycle_count_location_tasks (tenant_id, task_id, facility_id, location_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_id.get())
    .bind(raw_task)
    .bind(denied_facility)
    .bind(denied_location)
    .execute(&fixture.db)
    .await
    .is_err());

    let denied_unpack_line_id: i64 = sqlx::query_scalar(
        "SELECT id FROM unpack_cancelled_order_task_lines WHERE tenant_id = $1 AND task_id = $2",
    )
    .bind(tenant_id.get())
    .bind(denied_owner_task)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert!(sqlx::query(
        r#"
        INSERT INTO work_task_progress (
            tenant_id, created, task_id, facility_id, inventory_owner_id, task_line_id, action
        )
        VALUES ($1, CURRENT_TIMESTAMP, $2, $3, $4, $5, 'started')
        "#,
    )
    .bind(tenant_id.get())
    .bind(allowed_owner_task)
    .bind(allowed_facility)
    .bind(allowed_owner)
    .bind(denied_unpack_line_id)
    .execute(&fixture.db)
    .await
    .is_err());
    assert!(sqlx::query(
        r#"
        INSERT INTO work_task_progress (
            tenant_id, created, task_id, facility_id, inventory_owner_id, from_location_id, action
        )
        VALUES ($1, CURRENT_TIMESTAMP, $2, $3, $4, $5, 'started')
        "#,
    )
    .bind(tenant_id.get())
    .bind(allowed_owner_task)
    .bind(allowed_facility)
    .bind(allowed_owner)
    .bind(denied_location)
    .execute(&fixture.db)
    .await
    .is_err());
}

#[tokio::test]
async fn concurrent_scope_shrink_cannot_leave_task_active() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("task-scope-race@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility = fixture.facility(tenant_id, "Task Scope Race DC").await;
    let location = fixture.location(tenant_id, facility, "TASK-RACE").await;
    let task_id = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        operator.id,
        location,
        None,
        None,
        None,
        None,
        Some("scope race".to_owned()),
    )
    .await
    .unwrap();
    let stale_access = repo::tenants::access_for_user(&fixture.db, operator.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let shrink = UpdateUserAccessScope {
        user_id: operator.id,
        all_facilities: false,
        facility_ids: Vec::new(),
        all_inventory_owners: true,
        inventory_owner_ids: Vec::new(),
    };

    let (started, scope_updated) = tokio::join!(
        repo::tasks::start_task_in_scope(&fixture.db, &stale_access, task_id, operator.id),
        repo::tenants::update_user_access_scope(&fixture.db, tenant_id, &shrink),
    );
    started.unwrap();
    assert!(scope_updated.unwrap());

    let task: (String, Option<i64>) = sqlx::query_as(
        "SELECT status, assigned_user_id FROM work_tasks WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(task, ("open".to_owned(), None));
}
