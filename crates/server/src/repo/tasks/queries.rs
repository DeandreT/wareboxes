use sqlx::Row;
use wareboxes_core::models::{
    TenantAccess, UnpackCancelledOrderTaskLine, WorkTask, WorkTaskStatus, WorkTaskType,
};
use wareboxes_domain::TenantId;

use crate::db::{bind_tenant_context, Db};
use crate::error::{AppError, AppResult};

use super::execution::task_is_accessible;
use super::ScopeBindings;

#[derive(Debug, Clone, Default)]
pub struct WorkTaskFilters {
    pub show_deleted: bool,
    pub status: Option<WorkTaskStatus>,
    pub task_type: Option<WorkTaskType>,
    pub assigned_user_id: Option<i64>,
    pub location_id: Option<i64>,
    pub order_id: Option<i64>,
}

pub(super) fn map_task(row: &sqlx::postgres::PgRow) -> AppResult<WorkTask> {
    let task_type: String = row.try_get("task_type")?;
    let status: String = row.try_get("status")?;
    Ok(WorkTask {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
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
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        task_id: row.try_get("task_id")?,
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
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
    SELECT id, tenant_id, facility_id, inventory_owner_id, created, modified, deleted, task_type,
           status, required_permission, priority, title, instructions, assigned_user_id, created_by,
           completed_by, scheduled_for, due_at, started_at, lease_expires_at, task_timeout_seconds,
           last_released_at, release_count, completed_at, metadata_json
    FROM work_tasks
"#;

pub async fn get_tasks(
    db: &Db,
    tenant_id: TenantId,
    filters: WorkTaskFilters,
) -> AppResult<Vec<WorkTask>> {
    get_tasks_with_scope(db, tenant_id, &ScopeBindings::unrestricted(), filters).await
}

pub async fn get_tasks_in_scope(
    db: &Db,
    access: &TenantAccess,
    filters: WorkTaskFilters,
) -> AppResult<Vec<WorkTask>> {
    get_tasks_with_scope(
        db,
        access.tenant_id,
        &ScopeBindings::for_access(access),
        filters,
    )
    .await
}

async fn get_tasks_with_scope(
    db: &Db,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    filters: WorkTaskFilters,
) -> AppResult<Vec<WorkTask>> {
    let sql = format!(
        r#"
        {TASK_SELECT}
        WHERE tenant_id = $1
          AND ($2 OR deleted IS NULL)
          AND ($3::TEXT IS NULL OR status = $3)
          AND ($4::TEXT IS NULL OR task_type = $4)
          AND ($5::BIGINT IS NULL OR assigned_user_id = $5)
          AND (
              $6::BIGINT IS NULL
              OR EXISTS (SELECT 1 FROM cycle_count_item_location_tasks d WHERE d.tenant_id = work_tasks.tenant_id AND d.task_id = work_tasks.id AND d.location_id = $6)
              OR EXISTS (SELECT 1 FROM cycle_count_location_tasks d WHERE d.tenant_id = work_tasks.tenant_id AND d.task_id = work_tasks.id AND d.location_id = $6)
              OR EXISTS (SELECT 1 FROM break_master_pack_tasks d WHERE d.tenant_id = work_tasks.tenant_id AND d.task_id = work_tasks.id AND d.location_id = $6)
          )
          AND (
              $7::BIGINT IS NULL
              OR EXISTS (SELECT 1 FROM unpack_cancelled_order_tasks d WHERE d.tenant_id = work_tasks.tenant_id AND d.task_id = work_tasks.id AND d.order_id = $7)
              OR EXISTS (SELECT 1 FROM cycle_count_item_location_tasks d WHERE d.tenant_id = work_tasks.tenant_id AND d.task_id = work_tasks.id AND d.order_id = $7)
          )
          AND ($8 OR facility_id = ANY($9))
          AND ($10 OR inventory_owner_id = ANY($11))
        ORDER BY COALESCE(scheduled_for, created), priority DESC, created, id
        "#
    );

    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(&sql)
        .bind(tenant_id.get())
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
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_all(&mut *tx)
        .await?;
    let tasks = rows.iter().map(map_task).collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(tasks)
}

pub async fn get_unpack_cancelled_order_task_lines(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
) -> AppResult<Vec<UnpackCancelledOrderTaskLine>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, task_id, facility_id, inventory_owner_id, order_item_id, item_id,
               item_batch_id, inventory_balance_id, license_plate_id, source_location_id,
               destination_location_id, expected_qty, unpacked_qty, missing_qty, damaged_qty, status
        FROM unpack_cancelled_order_task_lines
        WHERE tenant_id = $1 AND task_id = $2
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(map_unpack_cancelled_order_task_line)
        .collect()
}

pub async fn get_unpack_cancelled_order_task_lines_in_scope(
    db: &Db,
    access: &TenantAccess,
    task_id: i64,
) -> AppResult<Vec<UnpackCancelledOrderTaskLine>> {
    if !task_is_accessible(db, access, task_id).await? {
        return Ok(Vec::new());
    }
    get_unpack_cancelled_order_task_lines(db, access.tenant_id, task_id).await
}
