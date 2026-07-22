mod common;

use common::*;

#[tokio::test]
async fn work_tasks_are_precise_and_deduplicate_generated_tasks() {
    let db = setup().await;

    let user = auth::register_user(&db, "tasks@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let assignee = auth::register_user(&db, "task-worker@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let wms_perm = repo::permissions::add_permission(&db, "wms", Some("WMS"))
        .await
        .unwrap();
    let wms_role = repo::roles::add_role(&db, "task-wms", Some("task worker"))
        .await
        .unwrap();
    repo::roles::add_role_permission(&db, wms_role, wms_perm)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, assignee.id, wms_role)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let warehouse = repo::warehouses::add_warehouse(&db, tenant_id, "Task DC")
        .await
        .unwrap();
    let freezer = repo::locations::add_location(
        &db,
        warehouse,
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
        warehouse,
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
    let batch = repo::inventory::add_item_batch(
        &db,
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
    let balance = repo::inventory::get_balances(&db, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.item_batch_id == batch && balance.location_id == freezer)
        .unwrap();

    let cycle_a = repo::tasks::create_item_location_cycle_count_task(
        &db,
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

    let pack_link =
        repo::items::add_item_pack_link(&db, master_item, single_item, 12, Some("12 pack"))
            .await
            .unwrap();
    assert_eq!(
        repo::items::get_item_pack_links(&db, false).await.unwrap()[0].id,
        pack_link
    );
    let break_task = repo::tasks::create_break_master_pack_task(
        &db,
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

    let started = repo::tasks::start_next_task(&db, assignee.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started.id, cycle_a);
    assert_eq!(started.assigned_user_id, Some(assignee.id));
    assert_eq!(started.status, WorkTaskStatus::InProgress);
    assert!(started.lease_expires_at.is_some());
    assert!(repo::tasks::complete_task(&db, cycle_a, assignee.id, None)
        .await
        .unwrap());

    let started = repo::tasks::start_next_task(&db, assignee.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started.id, break_task);
    assert!(!repo::tasks::record_task_progress(
        &db,
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
    assert!(repo::tasks::abort_task(&db, break_task, assignee.id)
        .await
        .unwrap());
    let aborted = repo::tasks::get_tasks(
        &db,
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

    let restarted = repo::tasks::start_next_task(&db, assignee.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restarted.id, break_task);
    assert!(
        repo::tasks::complete_task(&db, break_task, assignee.id, Some(1))
            .await
            .unwrap()
    );

    let tasks = repo::tasks::get_tasks(
        &db,
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
    let account = fixture.account(tenant_id, "Cancel Task Account").await;
    let item = fixture.item("Cancelled Order Item", "each").await;
    let order_id = fixture.order("CANCEL-TASK-1", Some(account)).await;
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
        account_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    assert!(repo::orders::update_order_by_user(db, &update, user.id)
        .await
        .unwrap());
    assert!(repo::orders::update_order_by_user(db, &update, user.id)
        .await
        .unwrap());

    let tasks = repo::tasks::get_tasks(
        db,
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

    assert!(repo::tasks::start_task(db, tasks[0].id, user.id)
        .await
        .unwrap());
    assert!(repo::tasks::record_task_progress(
        db,
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
    assert!(!repo::tasks::complete_task(db, tasks[0].id, user.id, None)
        .await
        .unwrap());
    assert!(repo::tasks::record_task_progress(
        db,
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
    assert!(repo::tasks::complete_task(db, tasks[0].id, user.id, None)
        .await
        .unwrap());
}
