mod common;

use common::*;

#[derive(Clone, Copy)]
struct DetailRow {
    table_name: &'static str,
    key_name: &'static str,
    key: i64,
}

#[derive(Clone, Copy)]
struct CountTarget {
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    item_batch_id: i64,
    item_id: i64,
    inventory_balance_id: i64,
}

#[tokio::test]
async fn typed_task_details_require_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("task-detail-rls-a@test.com").await;
    let user_b = fixture.user("task-detail-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let facility_a = fixture.facility(tenant_a, "Task Detail RLS A").await;
    let facility_b = fixture.facility(tenant_b, "Task Detail RLS B").await;
    let location_a = fixture
        .location(tenant_a, facility_a, "TASK-DETAIL-RLS-A")
        .await;
    let location_b = fixture
        .location(tenant_b, facility_b, "TASK-DETAIL-RLS-B")
        .await;
    let single_a = fixture.item(tenant_a, "Task Detail Single A", "each").await;
    let master_a = fixture.item(tenant_a, "Task Detail Master A", "case").await;
    let single_b = fixture.item(tenant_b, "Task Detail Single B", "each").await;
    let master_b = fixture.item(tenant_b, "Task Detail Master B", "case").await;
    let owner_a = fixture
        .inventory_owner(tenant_a, "Task Detail Owner A")
        .await;
    let owner_b = fixture
        .inventory_owner(tenant_b, "Task Detail Owner B")
        .await;
    let order_a = fixture
        .order(tenant_a, "TASK-DETAIL-ORDER-A", owner_a)
        .await;
    let carrier_order_a = fixture
        .order(tenant_a, "TASK-DETAIL-CARRIER-ORDER-A", owner_a)
        .await;
    let order_b = fixture
        .order(tenant_b, "TASK-DETAIL-ORDER-B", owner_b)
        .await;
    let item_batch_a = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_a,
        owner_a,
        single_a,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let item_batch_b = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_b,
        owner_b,
        single_b,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    insert_owner_facility(&mut tenant_b_tx, tenant_b, owner_b, facility_b)
        .await
        .unwrap();
    let balance_b = insert_balance(
        &mut tenant_b_tx,
        tenant_b,
        owner_b,
        facility_b,
        location_b,
        item_batch_b,
        single_b,
    )
    .await
    .unwrap();
    tenant_b_tx.commit().await.unwrap();
    let target_b = CountTarget {
        tenant_id: tenant_b,
        inventory_owner_id: owner_b,
        facility_id: facility_b,
        location_id: location_b,
        item_batch_id: item_batch_b,
        item_id: single_b,
        inventory_balance_id: balance_b,
    };

    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    insert_owner_facility(&mut tx, tenant_a, owner_a, facility_a)
        .await
        .unwrap();
    let balance_a = insert_balance(
        &mut tx,
        tenant_a,
        owner_a,
        facility_a,
        location_a,
        item_batch_a,
        single_a,
    )
    .await
    .unwrap();
    let target_a = CountTarget {
        tenant_id: tenant_a,
        inventory_owner_id: owner_a,
        facility_id: facility_a,
        location_id: location_a,
        item_batch_id: item_batch_a,
        item_id: single_a,
        inventory_balance_id: balance_a,
    };
    let item_task = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        Some(owner_a),
        "cycle_count_item_location",
        "Task Detail Item A",
    )
    .await;
    let location_task = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        None,
        "cycle_count_location",
        "Task Detail Location A",
    )
    .await;
    let break_task = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        None,
        "break_master_pack",
        "Task Detail Break A",
    )
    .await;
    let unpack_task = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        Some(owner_a),
        "unpack_cancelled_order",
        "Task Detail Unpack A",
    )
    .await;
    let item_carrier = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        Some(owner_a),
        "cycle_count_item_location",
        "Task Detail Item Carrier",
    )
    .await;
    let location_carrier = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        None,
        "cycle_count_location",
        "Task Detail Location Carrier",
    )
    .await;
    let break_carrier = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        None,
        "break_master_pack",
        "Task Detail Break Carrier",
    )
    .await;
    let unpack_carrier = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        Some(owner_a),
        "unpack_cancelled_order",
        "Task Detail Unpack Carrier",
    )
    .await;
    let result_carrier = insert_task(
        &mut tx,
        tenant_a,
        user_a.id,
        facility_a,
        Some(owner_a),
        "cycle_count_item_location",
        "Task Detail Result Carrier",
    )
    .await;
    insert_item_detail(&mut tx, item_task, target_a)
        .await
        .unwrap();
    insert_location_detail(&mut tx, tenant_a, location_task, facility_a, location_a)
        .await
        .unwrap();
    insert_break_detail(
        &mut tx, tenant_a, break_task, facility_a, location_a, master_a, single_a,
    )
    .await
    .unwrap();
    insert_unpack_detail(&mut tx, tenant_a, unpack_task, facility_a, owner_a, order_a)
        .await
        .unwrap();
    let unpack_line = insert_unpack_line(
        &mut tx,
        tenant_a,
        unpack_task,
        facility_a,
        owner_a,
        single_a,
    )
    .await
    .unwrap();
    activate_task(&mut tx, tenant_a, item_task, user_a.id)
        .await
        .unwrap();
    insert_count_result(&mut tx, item_task, target_a, user_a.id)
        .await
        .unwrap();
    complete_count_task(&mut tx, tenant_a, item_task)
        .await
        .unwrap();
    insert_item_detail(&mut tx, result_carrier, target_a)
        .await
        .unwrap();
    activate_task(&mut tx, tenant_a, result_carrier, user_a.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let detail_rows = [
        DetailRow {
            table_name: "cycle_count_item_location_tasks",
            key_name: "task_id",
            key: item_task,
        },
        DetailRow {
            table_name: "cycle_count_location_tasks",
            key_name: "task_id",
            key: location_task,
        },
        DetailRow {
            table_name: "break_master_pack_tasks",
            key_name: "task_id",
            key: break_task,
        },
        DetailRow {
            table_name: "unpack_cancelled_order_tasks",
            key_name: "task_id",
            key: unpack_task,
        },
        DetailRow {
            table_name: "unpack_cancelled_order_task_lines",
            key_name: "id",
            key: unpack_line,
        },
    ];
    let source_rows = snapshots(&fixture.db, tenant_a, &detail_rows).await;
    let source_result = result_snapshot(&fixture.db, tenant_a, item_task).await;

    for row in detail_rows {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}", row.table_name))
            .fetch_one(&fixture.db)
            .await
            .unwrap();
        assert_eq!(count, 0, "unbound visibility for {}", row.table_name);
        let updates = sqlx::query(&format!(
            "UPDATE {} SET tenant_id = tenant_id WHERE {} = $1",
            row.table_name, row.key_name
        ))
        .bind(row.key)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(updates, 0, "unbound update for {}", row.table_name);
        let deletes = sqlx::query(&format!(
            "DELETE FROM {} WHERE {} = $1",
            row.table_name, row.key_name
        ))
        .bind(row.key)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(deletes, 0, "unbound delete for {}", row.table_name);
    }

    let mut tx = fixture.db.begin().await.unwrap();
    assert!(insert_item_detail(&mut tx, item_carrier, target_a)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(
        insert_location_detail(&mut tx, tenant_a, location_carrier, facility_a, location_a,)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(insert_break_detail(
        &mut tx,
        tenant_a,
        break_carrier,
        facility_a,
        location_a,
        master_a,
        single_a,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(insert_unpack_detail(
        &mut tx,
        tenant_a,
        unpack_carrier,
        facility_a,
        owner_a,
        carrier_order_a,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(insert_unpack_line(
        &mut tx,
        tenant_a,
        unpack_task,
        facility_a,
        owner_a,
        single_a,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();

    let result_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cycle_count_item_location_results")
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(result_count, 0, "unbound result visibility");
    assert!(sqlx::query(
        "UPDATE cycle_count_item_location_results SET note = 'changed' WHERE task_id = $1",
    )
    .bind(item_task)
    .execute(&fixture.db)
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM cycle_count_item_location_results WHERE task_id = $1")
            .bind(item_task)
            .execute(&fixture.db)
            .await
            .is_err()
    );
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(
        insert_count_result(&mut tx, result_carrier, target_a, user_a.id)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    for row in detail_rows {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {} WHERE {} = $1",
            row.table_name, row.key_name
        ))
        .bind(row.key)
        .fetch_one(&mut *tenant_b_tx)
        .await
        .unwrap();
        assert_eq!(count, 0, "cross-tenant read for {}", row.table_name);
        let updates = sqlx::query(&format!(
            "UPDATE {} SET tenant_id = tenant_id WHERE {} = $1",
            row.table_name, row.key_name
        ))
        .bind(row.key)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(updates, 0, "cross-tenant update for {}", row.table_name);
        let deletes = sqlx::query(&format!(
            "DELETE FROM {} WHERE {} = $1",
            row.table_name, row.key_name
        ))
        .bind(row.key)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(deletes, 0, "cross-tenant delete for {}", row.table_name);
    }
    let result_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cycle_count_item_location_results WHERE task_id = $1",
    )
    .bind(item_task)
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(result_count, 0, "cross-tenant result read");
    tenant_b_tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_item_detail(&mut tx, item_task, target_b)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(
        insert_location_detail(&mut tx, tenant_b, location_task, facility_b, location_b,)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_break_detail(
        &mut tx, tenant_b, break_task, facility_b, location_b, master_b, single_b,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(
        insert_unpack_detail(&mut tx, tenant_b, unpack_task, facility_b, owner_b, order_b,)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_unpack_line(
        &mut tx,
        tenant_b,
        unpack_task,
        facility_b,
        owner_b,
        single_b,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_item_detail(&mut tx, item_carrier, target_a)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(
        insert_location_detail(&mut tx, tenant_a, location_carrier, facility_a, location_a,)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_break_detail(
        &mut tx,
        tenant_a,
        break_carrier,
        facility_a,
        location_a,
        master_a,
        single_a,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_unpack_detail(
        &mut tx,
        tenant_a,
        unpack_carrier,
        facility_a,
        owner_a,
        carrier_order_a,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_unpack_line(
        &mut tx,
        tenant_a,
        unpack_task,
        facility_a,
        owner_a,
        single_a,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(
        insert_count_result(&mut tx, result_carrier, target_a, user_a.id)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    assert!(sqlx::query(
        "UPDATE cycle_count_item_location_results SET note = 'changed' WHERE task_id = $1",
    )
    .bind(item_task)
    .execute(&mut *tx)
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    assert!(
        sqlx::query("DELETE FROM cycle_count_item_location_results WHERE task_id = $1")
            .bind(item_task)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let admin_db = admin_db_for(&fixture.db).await;
    assert!(sqlx::query(
        "UPDATE cycle_count_item_location_results SET note = 'changed' WHERE tenant_id = $1 AND task_id = $2",
    )
    .bind(tenant_a.get())
    .bind(item_task)
    .execute(&admin_db)
    .await
    .is_err());
    assert!(sqlx::query(
        "DELETE FROM cycle_count_item_location_results WHERE tenant_id = $1 AND task_id = $2",
    )
    .bind(tenant_a.get())
    .bind(item_task)
    .execute(&admin_db)
    .await
    .is_err());
    admin_db.close().await;

    assert_eq!(
        snapshots(&fixture.db, tenant_a, &detail_rows).await,
        source_rows
    );
    assert_eq!(
        result_snapshot(&fixture.db, tenant_a, item_task).await,
        source_result
    );
}

async fn snapshots(db: &db::Db, tenant_id: TenantId, rows: &[DetailRow]) -> Vec<String> {
    let mut tx = tenant_tx(db, tenant_id).await;
    let mut snapshots = Vec::with_capacity(rows.len());
    for row in rows {
        let snapshot: String = sqlx::query_scalar(&format!(
            "SELECT row_to_json(detail_row)::TEXT FROM {} detail_row WHERE {} = $1",
            row.table_name, row.key_name
        ))
        .bind(row.key)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        snapshots.push(snapshot);
    }
    tx.rollback().await.unwrap();
    snapshots
}

async fn result_snapshot(db: &db::Db, tenant_id: TenantId, task_id: i64) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot = sqlx::query_scalar(
        r#"
        SELECT row_to_json(result_row)::TEXT
        FROM cycle_count_item_location_results AS result_row
        WHERE task_id = $1
        "#,
    )
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    snapshot
}

#[allow(clippy::too_many_arguments)]
async fn insert_task(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    facility_id: i64,
    inventory_owner_id: Option<i64>,
    task_type: &str,
    title: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO work_tasks (
            tenant_id, facility_id, inventory_owner_id, created, task_type, title, created_by
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(task_type)
    .bind(title)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

async fn insert_item_detail(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    task_id: i64,
    target: CountTarget,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO cycle_count_item_location_tasks (
            tenant_id, task_id, facility_id, inventory_owner_id, location_id,
            item_id, inventory_balance_id, source
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'rls_test')
        "#,
    )
    .bind(target.tenant_id.get())
    .bind(task_id)
    .bind(target.facility_id)
    .bind(target.inventory_owner_id)
    .bind(target.location_id)
    .bind(target.item_id)
    .bind(target.inventory_balance_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn insert_owner_facility(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_facilities (
            tenant_id, created, inventory_owner_id, facility_id
        )
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
async fn insert_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    item_batch_id: i64,
    item_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO inventory_balances (
            tenant_id, inventory_owner_id, created, facility_id, location_id,
            item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'each', 'available', 5, 0)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(facility_id)
    .bind(location_id)
    .bind(item_batch_id)
    .bind(item_id)
    .fetch_one(&mut **tx)
    .await
}

async fn activate_task(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'in_progress',
            assigned_user_id = $3,
            started_at = statement_timestamp(),
            lease_expires_at = statement_timestamp() + INTERVAL '1 hour'
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn insert_count_result(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    task_id: i64,
    target: CountTarget,
    user_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO cycle_count_item_location_results (
            tenant_id, task_id, inventory_owner_id, facility_id, location_id,
            item_id, inventory_balance_id, item_batch_id, license_plate_id,
            uom, lot, expiration, serial, status, system_qty_on_hand,
            system_qty_reserved, counted_qty, variance_qty,
            inventory_transaction_id, confirmed_by, confirmed_at, note
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, NULL, 'each', NULL, NULL, NULL,
            'available', 5, 0, 5, 0, NULL, $9, statement_timestamp(),
            'RLS fixture'
        )
        "#,
    )
    .bind(target.tenant_id.get())
    .bind(task_id)
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(target.location_id)
    .bind(target.item_id)
    .bind(target.inventory_balance_id)
    .bind(target.item_batch_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn complete_count_task(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE work_tasks task
        SET status = 'completed',
            assigned_user_id = NULL,
            lease_expires_at = NULL,
            completed_by = result.confirmed_by,
            completed_at = result.confirmed_at
        FROM cycle_count_item_location_results result
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND result.tenant_id = task.tenant_id
          AND result.task_id = task.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn insert_location_detail(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
    facility_id: i64,
    location_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO cycle_count_location_tasks (
            tenant_id, task_id, facility_id, location_id
        )
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(facility_id)
    .bind(location_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
async fn insert_break_detail(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
    facility_id: i64,
    location_id: i64,
    master_item_id: i64,
    single_item_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO break_master_pack_tasks (
            tenant_id, task_id, facility_id, location_id, master_item_id,
            single_item_id, master_qty, inner_qty_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, 1, 2)
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(facility_id)
    .bind(location_id)
    .bind(master_item_id)
    .bind(single_item_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn insert_unpack_detail(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    order_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO unpack_cancelled_order_tasks (
            tenant_id, facility_id, inventory_owner_id, task_id, order_id
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(inventory_owner_id)
    .bind(task_id)
    .bind(order_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn insert_unpack_line(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    item_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO unpack_cancelled_order_task_lines (
            tenant_id, facility_id, inventory_owner_id, task_id, item_id, expected_qty
        )
        VALUES ($1, $2, $3, $4, $5, 1)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(inventory_owner_id)
    .bind(task_id)
    .bind(item_id)
    .fetch_one(&mut **tx)
    .await
}
