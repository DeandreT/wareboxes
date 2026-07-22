use sqlx::Row;
use wareboxes_core::models::{
    Timestamp, UnpackCancelledOrderTaskLine, WorkTask, WorkTaskProgressAction, WorkTaskStatus,
    WorkTaskType,
};
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::permissions;

#[derive(Debug, Clone, Default)]
pub struct WorkTaskFilters {
    pub show_deleted: bool,
    pub status: Option<WorkTaskStatus>,
    pub task_type: Option<WorkTaskType>,
    pub assigned_user_id: Option<i64>,
    pub location_id: Option<i64>,
    pub order_id: Option<i64>,
}

struct NewWorkTask {
    task_type: WorkTaskType,
    title: String,
    instructions: Option<String>,
    required_permission: String,
    priority: i64,
    task_timeout_seconds: i64,
    assigned_user_id: Option<i64>,
    created_by: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    metadata_json: Option<String>,
}

fn task_permission(_task_type: WorkTaskType) -> &'static str {
    "wms"
}

fn task_timeout_seconds(task_type: WorkTaskType) -> i64 {
    match task_type {
        WorkTaskType::CycleCountItemLocation => 30 * 60,
        WorkTaskType::CycleCountLocation => 60 * 60,
        WorkTaskType::BreakMasterPack => 45 * 60,
        WorkTaskType::UnpackCancelledOrder => 30 * 60,
    }
}

fn map_task(row: &sqlx::postgres::PgRow) -> AppResult<WorkTask> {
    let task_type: String = row.try_get("task_type")?;
    let status: String = row.try_get("status")?;
    Ok(WorkTask {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        modified: row.try_get("modified")?,
        deleted: row.try_get("deleted")?,
        task_type: WorkTaskType::parse(&task_type).ok_or_else(|| {
            AppError::internal(format!("invalid work task type in database: {task_type}"))
        })?,
        status: WorkTaskStatus::parse(&status).ok_or_else(|| {
            AppError::internal(format!("invalid work task status in database: {status}"))
        })?,
        required_permission: row.try_get("required_permission")?,
        priority: row.try_get("priority")?,
        title: row.try_get("title")?,
        instructions: row.try_get("instructions")?,
        assigned_user_id: row.try_get("assigned_user_id")?,
        created_by: row.try_get("created_by")?,
        completed_by: row.try_get("completed_by")?,
        scheduled_for: row.try_get("scheduled_for")?,
        due_at: row.try_get("due_at")?,
        started_at: row.try_get("started_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        task_timeout_seconds: row.try_get("task_timeout_seconds")?,
        last_released_at: row.try_get("last_released_at")?,
        release_count: row.try_get("release_count")?,
        completed_at: row.try_get("completed_at")?,
        metadata_json: row.try_get("metadata_json")?,
    })
}

fn map_unpack_cancelled_order_task_line(
    row: &sqlx::postgres::PgRow,
) -> AppResult<UnpackCancelledOrderTaskLine> {
    Ok(UnpackCancelledOrderTaskLine {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        order_item_id: row.try_get("order_item_id")?,
        item_id: row.try_get("item_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        source_location_id: row.try_get("source_location_id")?,
        destination_location_id: row.try_get("destination_location_id")?,
        expected_qty: row.try_get("expected_qty")?,
        unpacked_qty: row.try_get("unpacked_qty")?,
        missing_qty: row.try_get("missing_qty")?,
        damaged_qty: row.try_get("damaged_qty")?,
        status: row.try_get("status")?,
    })
}

const TASK_SELECT: &str = r#"
    SELECT id, created, modified, deleted, task_type, status, required_permission, priority,
           title, instructions, assigned_user_id, created_by, completed_by, scheduled_for,
           due_at, started_at, lease_expires_at, task_timeout_seconds, last_released_at,
           release_count, completed_at, metadata_json
    FROM work_tasks
"#;

pub async fn get_tasks(db: &Db, filters: WorkTaskFilters) -> AppResult<Vec<WorkTask>> {
    release_expired_tasks(db).await?;
    let sql = format!(
        r#"
        {TASK_SELECT}
        WHERE ($1 OR deleted IS NULL)
          AND ($2::TEXT IS NULL OR status = $2)
          AND ($3::TEXT IS NULL OR task_type = $3)
          AND ($4::BIGINT IS NULL OR assigned_user_id = $4)
          AND (
              $5::BIGINT IS NULL
              OR EXISTS (SELECT 1 FROM cycle_count_item_location_tasks d WHERE d.task_id = work_tasks.id AND d.location_id = $5)
              OR EXISTS (SELECT 1 FROM cycle_count_location_tasks d WHERE d.task_id = work_tasks.id AND d.location_id = $5)
              OR EXISTS (SELECT 1 FROM break_master_pack_tasks d WHERE d.task_id = work_tasks.id AND d.location_id = $5)
          )
          AND (
              $6::BIGINT IS NULL
              OR EXISTS (SELECT 1 FROM unpack_cancelled_order_tasks d WHERE d.task_id = work_tasks.id AND d.order_id = $6)
              OR EXISTS (SELECT 1 FROM cycle_count_item_location_tasks d WHERE d.task_id = work_tasks.id AND d.order_id = $6)
          )
        ORDER BY COALESCE(scheduled_for, created), priority DESC, created, id
        "#
    );

    let rows = sqlx::query(&sql)
        .bind(filters.show_deleted)
        .bind(filters.status.map(|status| status.as_str().to_owned()))
        .bind(
            filters
                .task_type
                .map(|task_type| task_type.as_str().to_owned()),
        )
        .bind(filters.assigned_user_id)
        .bind(filters.location_id)
        .bind(filters.order_id)
        .fetch_all(db)
        .await?;
    rows.iter().map(map_task).collect()
}

pub async fn get_unpack_cancelled_order_task_lines(
    db: &Db,
    task_id: i64,
) -> AppResult<Vec<UnpackCancelledOrderTaskLine>> {
    let rows = sqlx::query(
        r#"
        SELECT id, task_id, order_item_id, item_id, item_batch_id, inventory_balance_id,
               license_plate_id, source_location_id, destination_location_id, expected_qty,
               unpacked_qty, missing_qty, damaged_qty, status
        FROM unpack_cancelled_order_task_lines
        WHERE task_id = $1
        ORDER BY id
        "#,
    )
    .bind(task_id)
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(map_unpack_cancelled_order_task_line)
        .collect()
}

async fn insert_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    task: NewWorkTask,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO work_tasks (
            created, modified, task_type, status, required_permission, priority, title,
            instructions, assigned_user_id, created_by, scheduled_for, due_at,
            task_timeout_seconds, metadata_json
        )
        VALUES ($1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        RETURNING id
        "#,
    )
    .bind(now_iso())
    .bind(task.task_type.as_str())
    .bind(
        if task.assigned_user_id.is_some() {
            WorkTaskStatus::Assigned
        } else {
            WorkTaskStatus::Open
        }
        .as_str(),
    )
    .bind(task.required_permission)
    .bind(task.priority)
    .bind(task.title)
    .bind(task.instructions)
    .bind(task.assigned_user_id)
    .bind(task.created_by)
    .bind(task.scheduled_for)
    .bind(task.due_at)
    .bind(task.task_timeout_seconds)
    .bind(task.metadata_json)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_item_location_cycle_count_task(
    db: &Db,
    user_id: i64,
    location_id: i64,
    item_id: i64,
    source: Option<&str>,
    order_id: Option<i64>,
    order_item_id: Option<i64>,
    inventory_balance_id: Option<i64>,
    note: Option<&str>,
) -> AppResult<i64> {
    let facility_id = facility_for_location(db, location_id).await?;
    ensure_active_item(db, item_id).await?;
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(location_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;

    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT task.id
        FROM work_tasks task
        INNER JOIN cycle_count_item_location_tasks detail ON detail.task_id = task.id
        WHERE task.deleted IS NULL
          AND task.task_type = 'cycle_count_item_location'
          AND task.status IN ('open', 'assigned', 'in_progress')
          AND detail.location_id = $1
          AND detail.item_id = $2
        LIMIT 1
        "#,
    )
    .bind(location_id)
    .bind(item_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(existing) = existing {
        sqlx::query("UPDATE work_tasks SET modified = $1 WHERE id = $2")
            .bind(now_iso())
            .bind(existing)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(existing);
    }

    let task_id = insert_task_tx(
        &mut tx,
        NewWorkTask {
            task_type: WorkTaskType::CycleCountItemLocation,
            title: "Cycle count item location".to_owned(),
            instructions: note.map(str::to_owned),
            required_permission: task_permission(WorkTaskType::CycleCountItemLocation).to_owned(),
            priority: 90,
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::CycleCountItemLocation),
            assigned_user_id: None,
            created_by: Some(user_id),
            scheduled_for: None,
            due_at: None,
            metadata_json: None,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO cycle_count_item_location_tasks (
            task_id, facility_id, location_id, item_id, inventory_balance_id,
            order_id, order_item_id, source, note
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(task_id)
    .bind(facility_id)
    .bind(location_id)
    .bind(item_id)
    .bind(inventory_balance_id)
    .bind(order_id)
    .bind(order_item_id)
    .bind(source)
    .bind(note)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(task_id)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_location_cycle_count_task(
    db: &Db,
    user_id: i64,
    location_id: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
) -> AppResult<i64> {
    let facility_id = facility_for_location(db, location_id).await?;
    let mut tx = db.begin().await?;
    let task_id = insert_task_tx(
        &mut tx,
        NewWorkTask {
            task_type: WorkTaskType::CycleCountLocation,
            title: "Cycle count location".to_owned(),
            instructions,
            required_permission: task_permission(WorkTaskType::CycleCountLocation).to_owned(),
            priority: priority.unwrap_or(30),
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::CycleCountLocation),
            assigned_user_id,
            created_by: Some(user_id),
            scheduled_for,
            due_at,
            metadata_json: None,
        },
    )
    .await?;
    sqlx::query(
        "INSERT INTO cycle_count_location_tasks (task_id, facility_id, location_id) VALUES ($1, $2, $3)",
    )
    .bind(task_id)
    .bind(facility_id)
    .bind(location_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(task_id)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_break_master_pack_task(
    db: &Db,
    user_id: i64,
    master_item_id: i64,
    single_item_id: i64,
    location_id: i64,
    qty: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
) -> AppResult<i64> {
    if qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }
    let inner_qty: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT inner_qty
        FROM item_pack_links
        WHERE master_item_id = $1
          AND single_item_id = $2
          AND deleted IS NULL
        ORDER BY inner_qty DESC
        LIMIT 1
        "#,
    )
    .bind(master_item_id)
    .bind(single_item_id)
    .fetch_optional(db)
    .await?;
    let Some(inner_qty) = inner_qty else {
        return Err(AppError::bad_request(
            "break master pack tasks require a master-to-single item pack link",
        ));
    };
    let facility_id = facility_for_location(db, location_id).await?;
    let mut tx = db.begin().await?;
    let task_id = insert_task_tx(
        &mut tx,
        NewWorkTask {
            task_type: WorkTaskType::BreakMasterPack,
            title: format!("Break {qty} master packs into {} singles", qty * inner_qty),
            instructions,
            required_permission: task_permission(WorkTaskType::BreakMasterPack).to_owned(),
            priority: priority.unwrap_or(40),
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::BreakMasterPack),
            assigned_user_id,
            created_by: Some(user_id),
            scheduled_for,
            due_at,
            metadata_json: None,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO break_master_pack_tasks (
            task_id, facility_id, location_id, master_item_id, single_item_id,
            master_qty, inner_qty_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(task_id)
    .bind(facility_id)
    .bind(location_id)
    .bind(master_item_id)
    .bind(single_item_id)
    .bind(qty)
    .bind(inner_qty)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(task_id)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_unpack_cancelled_order_task(
    db: &Db,
    tenant_id: TenantId,
    user_id: Option<i64>,
    order_id: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
) -> AppResult<i64> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM orders WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_optional(db)
    .await?;
    if status.as_deref() != Some("cancelled") {
        return Err(AppError::bad_request(
            "unpack cancelled order tasks require a cancelled order",
        ));
    }

    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT task.id
        FROM work_tasks task
        INNER JOIN unpack_cancelled_order_tasks detail ON detail.task_id = task.id
        WHERE task.deleted IS NULL
          AND task.task_type = 'unpack_cancelled_order'
          AND task.status IN ('open', 'assigned', 'in_progress')
          AND detail.order_id = $1
        LIMIT 1
        "#,
    )
    .bind(order_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(existing) = existing {
        sqlx::query("UPDATE work_tasks SET modified = $1 WHERE id = $2")
            .bind(now_iso())
            .bind(existing)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(existing);
    }

    let task_id = insert_task_tx(
        &mut tx,
        NewWorkTask {
            task_type: WorkTaskType::UnpackCancelledOrder,
            title: "Unpack cancelled order".to_owned(),
            instructions,
            required_permission: task_permission(WorkTaskType::UnpackCancelledOrder).to_owned(),
            priority: priority.unwrap_or(70),
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::UnpackCancelledOrder),
            assigned_user_id,
            created_by: user_id,
            scheduled_for,
            due_at,
            metadata_json: None,
        },
    )
    .await?;
    sqlx::query("INSERT INTO unpack_cancelled_order_tasks (task_id, order_id) VALUES ($1, $2)")
        .bind(task_id)
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO unpack_cancelled_order_task_lines (
            task_id, order_item_id, item_id, item_batch_id, expected_qty
        )
        SELECT $1, oi.id, oi.item_id, oi.item_batch_id, oi.qty
        FROM order_items oi
        WHERE oi.tenant_id = $2 AND oi.order_id = $3 AND oi.deleted IS NULL
        "#,
    )
    .bind(task_id)
    .bind(tenant_id.get())
    .bind(order_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(task_id)
}

pub async fn assign_task(db: &Db, task_id: i64, assigned_user_id: i64) -> AppResult<bool> {
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET assigned_user_id = $1, status = 'assigned', modified = $2
        WHERE id = $3
          AND deleted IS NULL
          AND status = 'open'
        "#,
    )
    .bind(assigned_user_id)
    .bind(now_iso())
    .bind(task_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn release_expired_tasks(db: &Db) -> AppResult<u64> {
    let now = now_iso();
    let task_ids = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id
        FROM work_tasks
        WHERE deleted IS NULL
          AND status IN ('assigned', 'in_progress')
          AND lease_expires_at IS NOT NULL
          AND lease_expires_at <= $1
          AND completed_at IS NULL
        "#,
    )
    .bind(now)
    .fetch_all(db)
    .await?;
    if task_ids.is_empty() {
        return Ok(0);
    }
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'open',
            assigned_user_id = NULL,
            started_at = NULL,
            lease_expires_at = NULL,
            last_released_at = $1,
            release_count = release_count + 1,
            modified = $1
        WHERE id = ANY($2)
        "#,
    )
    .bind(now)
    .bind(&task_ids)
    .execute(db)
    .await?;
    for task_id in task_ids {
        insert_progress(
            db, task_id, None, None, "expired", None, None, None, None, None,
        )
        .await?;
    }
    Ok(res.rows_affected())
}

pub async fn start_next_task(
    db: &Db,
    user_id: i64,
    task_type: Option<WorkTaskType>,
) -> AppResult<Option<WorkTask>> {
    release_expired_tasks(db).await?;

    let permissions = permissions::get_user_permissions(db, user_id).await?;
    let is_admin = permissions
        .iter()
        .any(|permission| permission.name.eq_ignore_ascii_case("admin"));
    let allowed_permissions = permissions
        .iter()
        .map(|permission| permission.name.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if !is_admin && allowed_permissions.is_empty() {
        return Ok(None);
    }

    let active: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM work_tasks
        WHERE deleted IS NULL
          AND assigned_user_id = $1
          AND status IN ('assigned', 'in_progress')
          AND (lease_expires_at IS NULL OR lease_expires_at > $2)
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(now_iso())
    .fetch_optional(db)
    .await?;
    if active.is_some() {
        return Err(AppError::conflict(
            "user already has an active task; abort or complete it first",
        ));
    }

    let now = now_iso();
    let row = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT id
            FROM work_tasks
            WHERE deleted IS NULL
              AND status = 'open'
              AND (scheduled_for IS NULL OR scheduled_for <= $1)
              AND ($2 OR LOWER(required_permission) = ANY($3))
              AND ($4::TEXT IS NULL OR task_type = $4)
            ORDER BY priority DESC, due_at ASC NULLS LAST, COALESCE(scheduled_for, created), created, id
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE work_tasks AS task
        SET status = 'in_progress',
            assigned_user_id = $5,
            started_at = COALESCE(task.started_at, $1),
            lease_expires_at = $1 + make_interval(secs => task.task_timeout_seconds::INT),
            modified = $1
        FROM candidate
        WHERE task.id = candidate.id
        RETURNING task.id, task.created, task.modified, task.deleted, task.task_type, task.status,
                  task.required_permission, task.priority, task.title, task.instructions,
                  task.assigned_user_id, task.created_by, task.completed_by, task.scheduled_for,
                  task.due_at, task.started_at, task.lease_expires_at, task.task_timeout_seconds,
                  task.last_released_at, task.release_count, task.completed_at, task.metadata_json
        "#,
    )
    .bind(now)
    .bind(is_admin)
    .bind(&allowed_permissions)
    .bind(task_type.map(|task_type| task_type.as_str().to_owned()))
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    if let Some(row) = row.as_ref() {
        let task_id: i64 = row.try_get("id")?;
        insert_progress(
            db,
            task_id,
            None,
            Some(user_id),
            "started",
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    }
    row.as_ref().map(map_task).transpose()
}

pub async fn start_task(db: &Db, task_id: i64, user_id: i64) -> AppResult<bool> {
    release_expired_tasks(db).await?;
    let now = now_iso();
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET assigned_user_id = COALESCE(assigned_user_id, $1),
            status = 'in_progress',
            started_at = COALESCE(started_at, $2),
            lease_expires_at = $2 + make_interval(secs => task_timeout_seconds::INT),
            modified = $2
        WHERE id = $3
          AND deleted IS NULL
          AND status IN ('open', 'assigned')
          AND (assigned_user_id IS NULL OR assigned_user_id = $1)
        "#,
    )
    .bind(user_id)
    .bind(now)
    .bind(task_id)
    .execute(db)
    .await?;
    if res.rows_affected() > 0 {
        insert_progress(
            db,
            task_id,
            None,
            Some(user_id),
            "started",
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    }
    Ok(res.rows_affected() > 0)
}

#[allow(clippy::too_many_arguments)]
pub async fn record_task_progress(
    db: &Db,
    user_id: i64,
    task_id: i64,
    task_line_id: Option<i64>,
    action: WorkTaskProgressAction,
    qty_completed: i64,
    from_location_id: Option<i64>,
    to_location_id: Option<i64>,
    note: Option<&str>,
) -> AppResult<bool> {
    if qty_completed <= 0 {
        return Err(AppError::bad_request("completed quantity must be positive"));
    }
    let task_type = task_type(db, task_id).await?;
    let updated = match task_type {
        WorkTaskType::BreakMasterPack => {
            if action != WorkTaskProgressAction::Progress || task_line_id.is_some() {
                return Err(AppError::bad_request(
                    "break master pack tasks only accept task progress",
                ));
            }
            let res = sqlx::query(
                r#"
                UPDATE break_master_pack_tasks detail
                SET master_qty_completed = master_qty_completed + $1
                FROM work_tasks task
                WHERE detail.task_id = task.id
                  AND detail.task_id = $2
                  AND task.deleted IS NULL
                  AND task.status IN ('assigned', 'in_progress')
                  AND task.assigned_user_id = $3
                  AND detail.master_qty_completed + $1 <= detail.master_qty
                "#,
            )
            .bind(qty_completed)
            .bind(task_id)
            .bind(user_id)
            .execute(db)
            .await?;
            res.rows_affected() > 0
        }
        WorkTaskType::UnpackCancelledOrder => {
            let task_line_id = task_line_id.ok_or_else(|| {
                AppError::bad_request("unpack cancelled order progress requires a task line")
            })?;
            if action == WorkTaskProgressAction::Progress {
                return Err(AppError::bad_request(
                    "unpack cancelled order tasks require unpacked, missing, or damaged progress",
                ));
            }
            let action = action.as_str();
            let res = sqlx::query(
                r#"
                UPDATE unpack_cancelled_order_task_lines line
                SET unpacked_qty = line.unpacked_qty + CASE WHEN $1 = 'unpacked' THEN $2 ELSE 0 END,
                    missing_qty = line.missing_qty + CASE WHEN $1 = 'missing' THEN $2 ELSE 0 END,
                    damaged_qty = line.damaged_qty + CASE WHEN $1 = 'damaged' THEN $2 ELSE 0 END,
                    source_location_id = COALESCE(line.source_location_id, $5),
                    destination_location_id = COALESCE($6, line.destination_location_id),
                    status = CASE
                        WHEN line.unpacked_qty + line.missing_qty + line.damaged_qty + $2 < line.expected_qty THEN 'partial'
                        WHEN line.missing_qty + line.damaged_qty
                             + CASE WHEN $1 IN ('missing', 'damaged') THEN $2 ELSE 0 END > 0 THEN 'exception'
                        ELSE 'completed'
                    END
                FROM work_tasks task
                WHERE line.task_id = task.id
                  AND line.id = $3
                  AND line.task_id = $4
                  AND task.deleted IS NULL
                  AND task.status IN ('assigned', 'in_progress')
                  AND task.assigned_user_id = $7
                  AND line.status IN ('open', 'partial')
                  AND line.unpacked_qty + line.missing_qty + line.damaged_qty + $2 <= line.expected_qty
                "#,
            )
            .bind(action)
            .bind(qty_completed)
            .bind(task_line_id)
            .bind(task_id)
            .bind(from_location_id)
            .bind(to_location_id)
            .bind(user_id)
            .execute(db)
            .await?;
            res.rows_affected() > 0
        }
        WorkTaskType::CycleCountItemLocation | WorkTaskType::CycleCountLocation => false,
    };
    if updated {
        sqlx::query("UPDATE work_tasks SET modified = $1 WHERE id = $2")
            .bind(now_iso())
            .bind(task_id)
            .execute(db)
            .await?;
        insert_progress(
            db,
            task_id,
            task_line_id,
            Some(user_id),
            action.as_str(),
            Some(qty_completed),
            from_location_id,
            to_location_id,
            note,
            None,
        )
        .await?;
    }
    Ok(updated)
}

pub async fn complete_task(
    db: &Db,
    task_id: i64,
    user_id: i64,
    qty_completed: Option<i64>,
) -> AppResult<bool> {
    if let Some(qty_completed) = qty_completed {
        if !record_task_progress(
            db,
            user_id,
            task_id,
            None,
            WorkTaskProgressAction::Progress,
            qty_completed,
            None,
            None,
            None,
        )
        .await?
        {
            return Ok(false);
        }
    }
    let task_type = task_type(db, task_id).await?;
    let detail_complete = match task_type {
        WorkTaskType::BreakMasterPack => {
            let complete: bool = sqlx::query_scalar(
                "SELECT master_qty_completed >= master_qty FROM break_master_pack_tasks WHERE task_id = $1",
            )
            .bind(task_id)
            .fetch_optional(db)
            .await?
            .unwrap_or(false);
            complete
        }
        WorkTaskType::UnpackCancelledOrder => {
            let open_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM unpack_cancelled_order_task_lines WHERE task_id = $1 AND status IN ('open', 'partial')",
            )
            .bind(task_id)
            .fetch_one(db)
            .await?;
            open_count == 0
        }
        _ => true,
    };
    if !detail_complete {
        return Ok(false);
    }

    let now = now_iso();
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'completed',
            completed_by = $1,
            completed_at = $2,
            lease_expires_at = NULL,
            modified = $2
        WHERE id = $3
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
          AND (assigned_user_id IS NULL OR assigned_user_id = $1)
        "#,
    )
    .bind(user_id)
    .bind(now)
    .bind(task_id)
    .execute(db)
    .await?;
    if res.rows_affected() > 0 {
        insert_progress(
            db,
            task_id,
            None,
            Some(user_id),
            "completed",
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    }
    Ok(res.rows_affected() > 0)
}

pub async fn abort_task(db: &Db, task_id: i64, user_id: i64) -> AppResult<bool> {
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'open',
            assigned_user_id = NULL,
            started_at = NULL,
            lease_expires_at = NULL,
            last_released_at = $1,
            release_count = release_count + 1,
            modified = $1
        WHERE id = $2
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
          AND assigned_user_id = $3
          AND completed_at IS NULL
        "#,
    )
    .bind(now_iso())
    .bind(task_id)
    .bind(user_id)
    .execute(db)
    .await?;
    if res.rows_affected() > 0 {
        insert_progress(
            db,
            task_id,
            None,
            Some(user_id),
            "aborted",
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    }
    Ok(res.rows_affected() > 0)
}

pub async fn cancel_task(db: &Db, task_id: i64, user_id: i64) -> AppResult<bool> {
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'cancelled',
            completed_by = $1,
            completed_at = $2,
            lease_expires_at = NULL,
            modified = $2
        WHERE id = $3
          AND deleted IS NULL
          AND status IN ('open', 'assigned', 'in_progress')
        "#,
    )
    .bind(user_id)
    .bind(now_iso())
    .bind(task_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

async fn task_type(db: &Db, task_id: i64) -> AppResult<WorkTaskType> {
    let value: String = sqlx::query_scalar("SELECT task_type FROM work_tasks WHERE id = $1")
        .bind(task_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError::bad_request("task not found"))?;
    WorkTaskType::parse(&value)
        .ok_or_else(|| AppError::internal(format!("invalid work task type in database: {value}")))
}

#[allow(clippy::too_many_arguments)]
async fn insert_progress(
    db: &Db,
    task_id: i64,
    task_line_id: Option<i64>,
    user_id: Option<i64>,
    action: &str,
    qty_delta: Option<i64>,
    from_location_id: Option<i64>,
    to_location_id: Option<i64>,
    note: Option<&str>,
    metadata_json: Option<&str>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO work_task_progress (
            created, task_id, task_line_id, user_id, action, qty_delta,
            from_location_id, to_location_id, note, metadata_json
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id
        "#,
    )
    .bind(now_iso())
    .bind(task_id)
    .bind(task_line_id)
    .bind(user_id)
    .bind(action)
    .bind(qty_delta)
    .bind(from_location_id)
    .bind(to_location_id)
    .bind(note)
    .bind(metadata_json)
    .fetch_one(db)
    .await?;
    Ok(id)
}

async fn facility_for_location(db: &Db, location_id: i64) -> AppResult<i64> {
    sqlx::query_scalar(
        "SELECT facility_id FROM locations WHERE id = $1 AND deleted IS NULL AND active",
    )
    .bind(location_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| AppError::bad_request("location not found"))
}

async fn ensure_active_item(db: &Db, item_id: i64) -> AppResult<()> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT id FROM items WHERE id = $1 AND deleted IS NULL")
            .bind(item_id)
            .fetch_optional(db)
            .await?;
    if found.is_none() {
        return Err(AppError::bad_request("item not found"));
    }
    Ok(())
}
