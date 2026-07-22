mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_core::models::WorkTask;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

#[tokio::test]
async fn work_tasks_are_precise_and_deduplicate_generated_tasks() {
    let db = setup().await;

    let user = auth::register_user(&db, "tasks@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let assignee = auth::register_user(&db, "task-worker@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_id.get())
    .bind(assignee.id)
    .execute(&db)
    .await
    .unwrap();
    let wms_perm = repo::permissions::add_permission(&db, tenant_id, "wms", Some("WMS"))
        .await
        .unwrap();
    let wms_role = repo::roles::add_role(&db, tenant_id, "task-wms", Some("task worker"))
        .await
        .unwrap();
    repo::roles::add_role_permission(&db, tenant_id, wms_role, wms_perm)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, tenant_id, assignee.id, wms_role)
        .await
        .unwrap();
    let facility = repo::facilities::add_facility(&db, tenant_id, "Task DC")
        .await
        .unwrap();
    let freezer = repo::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("FRZ-01"),
        Some("Freezer"),
        "freezer",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let shelf = repo::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("SHF-01"),
        Some("Shelf"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let master_item = repo::items::add_item(
        &db,
        tenant_id,
        "Frozen Meal 12 Pack",
        None,
        "case",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let single_item = repo::items::add_item(
        &db,
        tenant_id,
        "Frozen Meal Single",
        None,
        "each",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Task Inventory Owner",
        "task-owner@test.com",
    )
    .await
    .unwrap();
    let batch = repo::inventory::add_item_batch(
        &db,
        tenant_id,
        inventory_owner,
        master_item,
        None,
        Some("LOT-TASK"),
        None,
        Some(db::now_iso()),
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &db,
        tenant_id,
        user.id,
        batch,
        freezer,
        20,
        None,
        Some("task setup"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let balance = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.item_batch_id == batch && balance.location_id == freezer)
        .unwrap();

    let cycle_a = repo::tasks::create_item_location_cycle_count_task(
        &db,
        tenant_id,
        user.id,
        freezer,
        master_item,
        Some("pick_not_found"),
        None,
        None,
        Some(balance.id),
        Some("picker could not find item"),
    )
    .await
    .unwrap();
    let cycle_b = repo::tasks::create_item_location_cycle_count_task(
        &db,
        tenant_id,
        user.id,
        freezer,
        master_item,
        Some("pick_not_found"),
        None,
        None,
        Some(balance.id),
        Some("same item/location should reuse open task"),
    )
    .await
    .unwrap();
    assert_eq!(cycle_a, cycle_b);

    let err = repo::tasks::create_break_master_pack_task(
        &db,
        tenant_id,
        user.id,
        master_item,
        single_item,
        shelf,
        2,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::BadRequest(_))));

    let pack_link = repo::items::add_item_pack_link(
        &db,
        tenant_id,
        master_item,
        single_item,
        12,
        Some("12 pack"),
    )
    .await
    .unwrap();
    assert_eq!(
        repo::items::get_item_pack_links(&db, tenant_id, false)
            .await
            .unwrap()[0]
            .id,
        pack_link
    );
    let break_task = repo::tasks::create_break_master_pack_task(
        &db,
        tenant_id,
        user.id,
        master_item,
        single_item,
        shelf,
        2,
        None,
        None,
        None,
        None,
        Some("open cases into eaches".to_owned()),
    )
    .await
    .unwrap();
    assert!(break_task > 0);

    let started = repo::tasks::start_next_task(&db, tenant_id, assignee.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started.id, cycle_a);
    assert_eq!(started.assigned_user_id, Some(assignee.id));
    assert_eq!(started.status, WorkTaskStatus::InProgress);
    assert!(started.lease_expires_at.is_some());
    assert!(
        repo::tasks::complete_task(&db, tenant_id, cycle_a, assignee.id, None)
            .await
            .unwrap()
    );

    let started = repo::tasks::start_next_task(&db, tenant_id, assignee.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started.id, break_task);
    assert!(!repo::tasks::record_task_progress(
        &db,
        tenant_id,
        user.id,
        break_task,
        None,
        WorkTaskProgressAction::Progress,
        1,
        None,
        None,
        None,
    )
    .await
    .unwrap());
    assert!(repo::tasks::record_task_progress(
        &db,
        tenant_id,
        assignee.id,
        break_task,
        None,
        WorkTaskProgressAction::Progress,
        1,
        None,
        None,
        None,
    )
    .await
    .unwrap());
    assert!(
        repo::tasks::abort_task(&db, tenant_id, break_task, assignee.id)
            .await
            .unwrap()
    );
    let aborted = repo::tasks::get_tasks(
        &db,
        tenant_id,
        repo::tasks::WorkTaskFilters {
            show_deleted: false,
            status: Some(WorkTaskStatus::Open),
            task_type: Some(WorkTaskType::BreakMasterPack),
            assigned_user_id: None,
            location_id: None,
            order_id: None,
        },
    )
    .await
    .unwrap()
    .into_iter()
    .find(|task| task.id == break_task)
    .unwrap();
    assert_eq!(aborted.release_count, 1);
    let master_qty_completed: i64 = sqlx::query_scalar(
        "SELECT master_qty_completed FROM break_master_pack_tasks WHERE task_id = $1",
    )
    .bind(break_task)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(master_qty_completed, 1);

    let restarted = repo::tasks::start_next_task(&db, tenant_id, assignee.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restarted.id, break_task);
    assert!(
        repo::tasks::complete_task(&db, tenant_id, break_task, assignee.id, Some(1))
            .await
            .unwrap()
    );

    let tasks = repo::tasks::get_tasks(
        &db,
        tenant_id,
        repo::tasks::WorkTaskFilters {
            show_deleted: false,
            status: None,
            task_type: None,
            assigned_user_id: None,
            location_id: None,
            order_id: None,
        },
    )
    .await
    .unwrap();
    assert!(tasks.iter().any(|task| {
        task.id == cycle_a
            && task.task_type == WorkTaskType::CycleCountItemLocation
            && task.status == WorkTaskStatus::Completed
    }));
    assert!(tasks
        .iter()
        .any(|task| task.task_type == WorkTaskType::BreakMasterPack));
    let detail: (i64, i64) = sqlx::query_as(
        "SELECT master_item_id, single_item_id FROM break_master_pack_tasks WHERE task_id = $1",
    )
    .bind(break_task)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(detail, (master_item, single_item));
}

#[tokio::test]
async fn cancelling_order_creates_unpack_task() {
    let fixture = Fixture::new().await;
    let db = &fixture.db;

    let user = fixture.user("cancel-task@test.com").await;
    let tenant_id = tenant_for_user(db, user.id).await;
    let inventory_owner = fixture
        .inventory_owner(tenant_id, "Cancel Task InventoryOwner")
        .await;
    let item = fixture
        .item(tenant_id, "Cancelled Order Item", "each")
        .await;
    let order_id = fixture
        .order(tenant_id, "CANCEL-TASK-1", inventory_owner)
        .await;
    let order_item_id = fixture.order_item(order_id, item, 3).await;
    let update = OrderUpdate {
        order_id,
        order_key: None,
        rush: None,
        status: Some(OrderStatus::Cancelled),
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    assert!(
        repo::orders::update_order_by_user(db, tenant_id, &update, user.id)
            .await
            .unwrap()
    );
    assert!(
        repo::orders::update_order_by_user(db, tenant_id, &update, user.id)
            .await
            .unwrap()
    );

    let tasks = repo::tasks::get_tasks(
        db,
        tenant_id,
        repo::tasks::WorkTaskFilters {
            show_deleted: false,
            status: None,
            task_type: Some(WorkTaskType::UnpackCancelledOrder),
            assigned_user_id: None,
            location_id: None,
            order_id: Some(order_id),
        },
    )
    .await
    .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].created_by, Some(user.id));
    let line: (i64, i64, i64, String) = sqlx::query_as(
        "SELECT id, order_item_id, expected_qty, status FROM unpack_cancelled_order_task_lines WHERE task_id = $1",
    )
    .bind(tasks[0].id)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(line.1, order_item_id);
    assert_eq!(line.2, 3);
    assert_eq!(line.3, "open");

    assert!(repo::tasks::start_task(db, tenant_id, tasks[0].id, user.id)
        .await
        .unwrap());
    assert!(repo::tasks::record_task_progress(
        db,
        tenant_id,
        user.id,
        tasks[0].id,
        Some(line.0),
        WorkTaskProgressAction::Unpacked,
        2,
        None,
        None,
        Some("first two unpacked"),
    )
    .await
    .unwrap());
    assert!(
        !repo::tasks::complete_task(db, tenant_id, tasks[0].id, user.id, None)
            .await
            .unwrap()
    );
    assert!(repo::tasks::record_task_progress(
        db,
        tenant_id,
        user.id,
        tasks[0].id,
        Some(line.0),
        WorkTaskProgressAction::Missing,
        1,
        None,
        None,
        Some("one missing from cancelled order"),
    )
    .await
    .unwrap());
    let line: (i64, i64, i64, String) = sqlx::query_as(
        "SELECT unpacked_qty, missing_qty, damaged_qty, status FROM unpack_cancelled_order_task_lines WHERE id = $1",
    )
    .bind(line.0)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(line, (2, 1, 0, "exception".to_owned()));
    assert!(
        repo::tasks::complete_task(db, tenant_id, tasks[0].id, user.id, None)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn task_queue_is_tenant_isolated_and_claims_once() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("task-scope-operator@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, operator.id).await;
    let second_tenant_user = fixture.user("task-scope-tenant-b@test.com").await;
    let tenant_b = tenant_for_user(&fixture.db, second_tenant_user.id).await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_b.get())
    .bind(operator.id)
    .execute(&fixture.db)
    .await
    .unwrap();

    let facility_a = fixture.facility(tenant_a, "Task Scope Facility A").await;
    let facility_b = fixture.facility(tenant_b, "Task Scope Facility B").await;
    let location_a = fixture.location(tenant_a, facility_a, "TASK-SCOPE-A").await;
    let location_b = fixture.location(tenant_b, facility_b, "TASK-SCOPE-B").await;
    let task_a = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_a,
        operator.id,
        location_a,
        Some(100),
        None,
        None,
        None,
        Some("tenant A count".to_owned()),
    )
    .await
    .unwrap();
    let task_b = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_b,
        operator.id,
        location_b,
        Some(100),
        None,
        None,
        None,
        Some("tenant B count".to_owned()),
    )
    .await
    .unwrap();

    let filters = repo::tasks::WorkTaskFilters {
        show_deleted: false,
        status: None,
        task_type: None,
        assigned_user_id: None,
        location_id: None,
        order_id: None,
    };
    let tenant_a_tasks = repo::tasks::get_tasks(&fixture.db, tenant_a, filters.clone())
        .await
        .unwrap();
    let tenant_b_tasks = repo::tasks::get_tasks(&fixture.db, tenant_b, filters)
        .await
        .unwrap();
    assert_eq!(tenant_a_tasks.len(), 1);
    assert_eq!(tenant_a_tasks[0].id, task_a);
    assert_eq!(tenant_a_tasks[0].tenant_id, tenant_a);
    assert_eq!(tenant_b_tasks.len(), 1);
    assert_eq!(tenant_b_tasks[0].id, task_b);
    assert_eq!(tenant_b_tasks[0].tenant_id, tenant_b);
    assert!(
        !repo::tasks::start_task(&fixture.db, tenant_b, task_a, operator.id)
            .await
            .unwrap()
    );

    let worker_a = fixture.user("task-scope-worker-a@test.com").await;
    let worker_b = fixture.user("task-scope-worker-b@test.com").await;
    let role_id: i64 = sqlx::query_scalar(
        "SELECT id FROM roles WHERE tenant_id = $1 AND name = 'task-scope-operator@test.com-wms'",
    )
    .bind(tenant_a.get())
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    for worker in [&worker_a, &worker_b] {
        sqlx::query(
            "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
        )
        .bind(tenant_a.get())
        .bind(worker.id)
        .execute(&fixture.db)
        .await
        .unwrap();
        repo::roles::add_role_to_user(&fixture.db, tenant_a, worker.id, role_id)
            .await
            .unwrap();
    }

    let first_db = fixture.db.clone();
    let second_db = fixture.db.clone();
    let (first, second) = tokio::join!(
        repo::tasks::start_next_task(&first_db, tenant_a, worker_a.id, None),
        repo::tasks::start_next_task(&second_db, tenant_a, worker_b.id, None),
    );
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    let claimed_task = claims.iter().flatten().next().unwrap();
    assert_eq!(
        (claimed_task.id, claimed_task.tenant_id),
        (task_a, tenant_a)
    );
    let claimed_by = claimed_task.assigned_user_id.unwrap();

    let next_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_a,
        operator.id,
        location_a,
        Some(90),
        None,
        None,
        None,
        Some("second tenant A count".to_owned()),
    )
    .await
    .unwrap();
    assert!(
        !repo::tasks::start_task(&fixture.db, tenant_a, next_task, claimed_by)
            .await
            .unwrap()
    );

    sqlx::query(
        "UPDATE work_tasks SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_a.get())
    .bind(task_a)
    .execute(&fixture.db)
    .await
    .unwrap();
    let first_db = fixture.db.clone();
    let second_db = fixture.db.clone();
    let (first_release, second_release) = tokio::join!(
        repo::tasks::release_expired_tasks(&first_db, tenant_a),
        repo::tasks::release_expired_tasks(&second_db, tenant_a),
    );
    assert_eq!(first_release.unwrap() + second_release.unwrap(), 1);
    let release: (i64, i64) = sqlx::query_as(
        r#"
        SELECT task.release_count, COUNT(progress.id)
        FROM work_tasks task
        LEFT JOIN work_task_progress progress
          ON progress.tenant_id = task.tenant_id
         AND progress.task_id = task.id
         AND progress.action = 'expired'
        WHERE task.tenant_id = $1 AND task.id = $2
        GROUP BY task.release_count
        "#,
    )
    .bind(tenant_a.get())
    .bind(task_a)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(release, (1, 1));

    assert!(
        repo::tasks::cancel_task(&fixture.db, tenant_a, next_task, operator.id)
            .await
            .unwrap()
    );
    let cancellation_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_task_progress WHERE tenant_id = $1 AND task_id = $2 AND action = 'cancelled'",
    )
    .bind(tenant_a.get())
    .bind(next_task)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(cancellation_events, 1);

    let tenant_b_permission =
        repo::permissions::add_permission(&fixture.db, tenant_b, "wms", Some("WMS"))
            .await
            .unwrap();
    let tenant_b_role = repo::roles::add_role(
        &fixture.db,
        tenant_b,
        "task-scope-operator@test.com-wms",
        Some("WMS worker"),
    )
    .await
    .unwrap();
    repo::roles::add_role_permission(&fixture.db, tenant_b, tenant_b_role, tenant_b_permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&fixture.db, tenant_b, operator.id, tenant_b_role)
        .await
        .unwrap();

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let missing_tenant = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/tasks")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_tenant.status(), StatusCode::BAD_REQUEST);

    let selected_tenant = app
        .oneshot(
            Request::builder()
                .uri("/api/tasks")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_b.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_tenant.status(), StatusCode::OK);
    let body = to_bytes(selected_tenant.into_body(), usize::MAX)
        .await
        .unwrap();
    let tasks = serde_json::from_slice::<Vec<WorkTask>>(&body).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task_b);
    assert_eq!(tasks[0].tenant_id, tenant_b);
}
