use sqlx::Row;
use wareboxes_core::models::{TenantAccess, Timestamp, WorkTaskStatus, WorkTaskType};
use wareboxes_core::CoreError;
use wareboxes_domain::{CommandContext, TenantId};

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::idempotency::{require_command_context, PreparedCommand};

use super::access::{current_scope_tx, lock_user_tx, ScopeBindings};
use execution::user_can_execute_task_tx;
use queries::map_task;
use references::{
    lock_active_location_tx, lock_item_cycle_references_tx, lock_unpack_order_lines_tx,
};

mod execution;
mod leasing;
mod queries;
mod references;

pub use execution::*;
pub use leasing::*;
pub use queries::*;

pub(crate) async fn release_tasks_outside_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    leasing::release_inaccessible_active_tasks_tx(tx, tenant_id, user_id, scope).await
}

struct NewWorkTask {
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskDimensions {
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
}

impl TaskDimensions {
    fn is_allowed_by(self, scope: &ScopeBindings) -> bool {
        (scope.all_facilities
            || self
                .facility_id
                .is_some_and(|id| scope.facility_ids.contains(&id)))
            && (scope.all_inventory_owners
                || self
                    .inventory_owner_id
                    .is_some_and(|id| scope.inventory_owner_ids.contains(&id)))
    }
}

fn concealed_task_reference(error: AppError) -> AppError {
    match error {
        AppError::Core(
            CoreError::BadRequest(_) | CoreError::NotFound(_) | CoreError::Forbidden,
        ) => AppError::not_found("task references"),
        other => other,
    }
}

pub(super) async fn require_replayed_task_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id
        FROM work_tasks
        WHERE tenant_id = $1 AND id = $2
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("task"))?;
    let dimensions = TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    };
    if dimensions.is_allowed_by(scope) {
        Ok(())
    } else {
        Err(AppError::not_found("task"))
    }
}

async fn lock_current_task_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope_user_id: i64,
    assigned_user_id: Option<i64>,
) -> AppResult<ScopeBindings> {
    let mut user_ids = vec![scope_user_id];
    if let Some(assigned_user_id) = assigned_user_id {
        user_ids.push(assigned_user_id);
    }
    user_ids.sort_unstable();
    user_ids.dedup();
    for user_id in user_ids {
        lock_user_tx(tx, tenant_id, user_id).await?;
    }
    current_scope_tx(tx, tenant_id, scope_user_id).await
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

async fn insert_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task: NewWorkTask,
) -> AppResult<i64> {
    if let Some(assigned_user_id) = task.assigned_user_id {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))",
        )
        .bind(tenant_id.get())
        .bind(assigned_user_id)
        .execute(&mut **tx)
        .await?;
        let dimensions = TaskDimensions {
            facility_id: task.facility_id,
            inventory_owner_id: task.inventory_owner_id,
        };
        if !user_can_execute_task_tx(
            tx,
            tenant_id,
            assigned_user_id,
            dimensions,
            &task.required_permission,
        )
        .await?
        {
            return Err(AppError::bad_request(
                "assigned user cannot access the task facility or inventory owner",
            ));
        }
    }
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO work_tasks (
            tenant_id, facility_id, inventory_owner_id, created, modified, task_type, status,
            required_permission, priority, title, instructions, assigned_user_id, created_by,
            scheduled_for, due_at, task_timeout_seconds, metadata_json
        )
        VALUES ($1, $2, $3, $4, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(task.facility_id)
    .bind(task.inventory_owner_id)
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
    tenant_id: TenantId,
    user_id: i64,
    location_id: i64,
    item_id: i64,
    source: Option<&str>,
    order_id: Option<i64>,
    order_item_id: Option<i64>,
    inventory_balance_id: Option<i64>,
    note: Option<&str>,
) -> AppResult<i64> {
    create_item_location_cycle_count_task_with_scope(
        db,
        tenant_id,
        user_id,
        None,
        location_id,
        item_id,
        source,
        order_id,
        order_item_id,
        inventory_balance_id,
        note,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_item_location_cycle_count_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    scope_user_id: Option<i64>,
    location_id: i64,
    item_id: i64,
    source: Option<&str>,
    order_id: Option<i64>,
    order_item_id: Option<i64>,
    inventory_balance_id: Option<i64>,
    note: Option<&str>,
    command: Option<&CommandContext>,
) -> AppResult<i64> {
    let prepared = command
        .map(|command| {
            PreparedCommand::new(
                command,
                "task.create_item_location_cycle_count.v1",
                &(
                    location_id,
                    item_id,
                    source,
                    order_id,
                    order_item_id,
                    inventory_balance_id,
                    note,
                ),
            )
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    let current_scope = match scope_user_id {
        Some(scope_user_id) => {
            Some(lock_current_task_scope_tx(&mut tx, tenant_id, scope_user_id, None).await?)
        }
        None => None,
    };
    if let Some(prepared) = prepared.as_ref() {
        if let Some(task_id) = prepared.replayed::<i64>(&mut tx).await? {
            let scope = current_scope
                .as_ref()
                .ok_or_else(|| AppError::internal("scoped task command is missing its scope"))?;
            require_replayed_task_visible_tx(&mut tx, tenant_id, task_id, scope).await?;
            tx.commit().await?;
            return Ok(task_id);
        }
    }
    let references = lock_item_cycle_references_tx(
        &mut tx,
        tenant_id,
        location_id,
        item_id,
        order_id,
        order_item_id,
        inventory_balance_id,
    )
    .await;
    if let Err(error) = references {
        return Err(if current_scope.is_some() {
            concealed_task_reference(error)
        } else {
            error
        });
    }
    let dimensions = item_location_cycle_count_dimensions_tx(
        &mut tx,
        tenant_id,
        location_id,
        item_id,
        order_id,
        order_item_id,
        inventory_balance_id,
    )
    .await
    .map_err(|error| {
        if current_scope.is_some() {
            concealed_task_reference(error)
        } else {
            error
        }
    })?;
    if current_scope
        .as_ref()
        .is_some_and(|scope| !dimensions.is_allowed_by(scope))
    {
        return Err(AppError::not_found("task references"));
    }
    let facility_id = dimensions
        .facility_id
        .ok_or_else(|| AppError::internal("cycle count task is missing a facility"))?;
    ensure_active_item_tx(&mut tx, tenant_id, item_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT || ':' || $3::TEXT, 0))")
        .bind(tenant_id.get())
        .bind(location_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;

    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT task.id
        FROM work_tasks task
        INNER JOIN cycle_count_item_location_tasks detail ON detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND detail.tenant_id = task.tenant_id
          AND task.deleted IS NULL
          AND task.task_type = 'cycle_count_item_location'
          AND task.status IN ('open', 'assigned', 'in_progress')
          AND task.facility_id = $4
          AND task.inventory_owner_id IS NOT DISTINCT FROM $5
          AND detail.location_id = $2
          AND detail.item_id = $3
        LIMIT 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(location_id)
    .bind(item_id)
    .bind(dimensions.facility_id)
    .bind(dimensions.inventory_owner_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(existing) = existing {
        sqlx::query("UPDATE work_tasks SET modified = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(now_iso())
            .bind(tenant_id.get())
            .bind(existing)
            .execute(&mut *tx)
            .await?;
        return match prepared {
            Some(prepared) => prepared.commit(tx, existing).await,
            None => {
                tx.commit().await?;
                Ok(existing)
            }
        };
    }

    let task_id = insert_task_tx(
        &mut tx,
        tenant_id,
        NewWorkTask {
            facility_id: dimensions.facility_id,
            inventory_owner_id: dimensions.inventory_owner_id,
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
            tenant_id, task_id, facility_id, inventory_owner_id, location_id, item_id,
            inventory_balance_id, order_id, order_item_id, source, note
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(facility_id)
    .bind(dimensions.inventory_owner_id)
    .bind(location_id)
    .bind(item_id)
    .bind(inventory_balance_id)
    .bind(order_id)
    .bind(order_item_id)
    .bind(source)
    .bind(note)
    .execute(&mut *tx)
    .await?;
    match prepared {
        Some(prepared) => prepared.commit(tx, task_id).await,
        None => {
            tx.commit().await?;
            Ok(task_id)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_item_location_cycle_count_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    location_id: i64,
    item_id: i64,
    source: Option<&str>,
    order_id: Option<i64>,
    order_item_id: Option<i64>,
    inventory_balance_id: Option<i64>,
    note: Option<&str>,
) -> AppResult<i64> {
    require_command_context(access, command)?;
    create_item_location_cycle_count_task_with_scope(
        db,
        access.tenant_id,
        command.actor_id.get(),
        Some(command.actor_id.get()),
        location_id,
        item_id,
        source,
        order_id,
        order_item_id,
        inventory_balance_id,
        note,
        Some(command),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_location_cycle_count_task(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    location_id: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
) -> AppResult<i64> {
    create_location_cycle_count_task_with_scope(
        db,
        tenant_id,
        user_id,
        None,
        location_id,
        priority,
        assigned_user_id,
        scheduled_for,
        due_at,
        instructions,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_location_cycle_count_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    scope_user_id: Option<i64>,
    location_id: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
    command: Option<&CommandContext>,
) -> AppResult<i64> {
    let prepared = command
        .map(|command| {
            PreparedCommand::new(
                command,
                "task.create_location_cycle_count.v1",
                &(
                    location_id,
                    priority,
                    assigned_user_id,
                    scheduled_for,
                    due_at,
                    &instructions,
                ),
            )
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    let current_scope = match scope_user_id {
        Some(scope_user_id) => Some(
            lock_current_task_scope_tx(&mut tx, tenant_id, scope_user_id, assigned_user_id).await?,
        ),
        None => None,
    };
    if let Some(prepared) = prepared.as_ref() {
        if let Some(task_id) = prepared.replayed::<i64>(&mut tx).await? {
            let scope = current_scope
                .as_ref()
                .ok_or_else(|| AppError::internal("scoped task command is missing its scope"))?;
            require_replayed_task_visible_tx(&mut tx, tenant_id, task_id, scope).await?;
            tx.commit().await?;
            return Ok(task_id);
        }
    }
    let facility_id = lock_active_location_tx(&mut tx, tenant_id, location_id)
        .await
        .map_err(|error| {
            if current_scope.is_some() {
                concealed_task_reference(error)
            } else {
                error
            }
        })?;
    if current_scope.as_ref().is_some_and(|scope| {
        !(TaskDimensions {
            facility_id: Some(facility_id),
            inventory_owner_id: None,
        })
        .is_allowed_by(scope)
    }) {
        return Err(AppError::not_found("task references"));
    }
    let task_id = insert_task_tx(
        &mut tx,
        tenant_id,
        NewWorkTask {
            facility_id: Some(facility_id),
            inventory_owner_id: None,
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
        "INSERT INTO cycle_count_location_tasks (tenant_id, task_id, facility_id, location_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(facility_id)
    .bind(location_id)
    .execute(&mut *tx)
    .await?;
    match prepared {
        Some(prepared) => prepared.commit(tx, task_id).await,
        None => {
            tx.commit().await?;
            Ok(task_id)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_location_cycle_count_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    location_id: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
) -> AppResult<i64> {
    require_command_context(access, command)?;
    create_location_cycle_count_task_with_scope(
        db,
        access.tenant_id,
        command.actor_id.get(),
        Some(command.actor_id.get()),
        location_id,
        priority,
        assigned_user_id,
        scheduled_for,
        due_at,
        instructions,
        Some(command),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_break_master_pack_task(
    db: &Db,
    tenant_id: TenantId,
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
    create_break_master_pack_task_with_scope(
        db,
        tenant_id,
        user_id,
        None,
        master_item_id,
        single_item_id,
        location_id,
        qty,
        priority,
        assigned_user_id,
        scheduled_for,
        due_at,
        instructions,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_break_master_pack_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    scope_user_id: Option<i64>,
    master_item_id: i64,
    single_item_id: i64,
    location_id: i64,
    qty: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
    command: Option<&CommandContext>,
) -> AppResult<i64> {
    if qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }
    let prepared = command
        .map(|command| {
            PreparedCommand::new(
                command,
                "task.create_break_master_pack.v1",
                &(
                    master_item_id,
                    single_item_id,
                    location_id,
                    qty,
                    priority,
                    assigned_user_id,
                    scheduled_for,
                    due_at,
                    &instructions,
                ),
            )
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    let current_scope = match scope_user_id {
        Some(scope_user_id) => Some(
            lock_current_task_scope_tx(&mut tx, tenant_id, scope_user_id, assigned_user_id).await?,
        ),
        None => None,
    };
    if let Some(prepared) = prepared.as_ref() {
        if let Some(task_id) = prepared.replayed::<i64>(&mut tx).await? {
            let scope = current_scope
                .as_ref()
                .ok_or_else(|| AppError::internal("scoped task command is missing its scope"))?;
            require_replayed_task_visible_tx(&mut tx, tenant_id, task_id, scope).await?;
            tx.commit().await?;
            return Ok(task_id);
        }
    }
    let pack: Option<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT pack.inner_qty, location.facility_id
        FROM item_pack_links pack
        INNER JOIN items master
            ON master.tenant_id = pack.tenant_id
           AND master.id = pack.master_item_id
           AND master.deleted IS NULL
        INNER JOIN items single
            ON single.tenant_id = pack.tenant_id
           AND single.id = pack.single_item_id
           AND single.deleted IS NULL
        INNER JOIN locations location
            ON location.tenant_id = pack.tenant_id
           AND location.id = $4
           AND location.deleted IS NULL
           AND location.active
        WHERE pack.tenant_id = $1
          AND pack.master_item_id = $2
          AND pack.single_item_id = $3
          AND pack.deleted IS NULL
        ORDER BY pack.inner_qty DESC
        LIMIT 1
        FOR SHARE OF pack, master, single, location
        "#,
    )
    .bind(tenant_id.get())
    .bind(master_item_id)
    .bind(single_item_id)
    .bind(location_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((inner_qty, facility_id)) = pack else {
        return Err(if current_scope.is_some() {
            AppError::not_found("task references")
        } else {
            AppError::bad_request(
                "break master pack tasks require active items, location, and pack link",
            )
        });
    };
    if let Some(current_scope) = current_scope {
        if !(TaskDimensions {
            facility_id: Some(facility_id),
            inventory_owner_id: None,
        })
        .is_allowed_by(&current_scope)
        {
            return Err(AppError::not_found("task references"));
        }
    }
    let task_id = insert_task_tx(
        &mut tx,
        tenant_id,
        NewWorkTask {
            facility_id: Some(facility_id),
            inventory_owner_id: None,
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
            tenant_id, task_id, facility_id, location_id, master_item_id, single_item_id,
            master_qty, inner_qty_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(facility_id)
    .bind(location_id)
    .bind(master_item_id)
    .bind(single_item_id)
    .bind(qty)
    .bind(inner_qty)
    .execute(&mut *tx)
    .await?;
    match prepared {
        Some(prepared) => prepared.commit(tx, task_id).await,
        None => {
            tx.commit().await?;
            Ok(task_id)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_break_master_pack_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
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
    require_command_context(access, command)?;
    create_break_master_pack_task_with_scope(
        db,
        access.tenant_id,
        command.actor_id.get(),
        Some(command.actor_id.get()),
        master_item_id,
        single_item_id,
        location_id,
        qty,
        priority,
        assigned_user_id,
        scheduled_for,
        due_at,
        instructions,
        Some(command),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_unpack_cancelled_order_task(
    db: &Db,
    tenant_id: TenantId,
    user_id: Option<i64>,
    order_id: i64,
    facility_id: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
) -> AppResult<i64> {
    let mut tx = db.begin().await?;
    if let Some(assigned_user_id) = assigned_user_id {
        lock_user_tx(&mut tx, tenant_id, assigned_user_id).await?;
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(tenant_id.get())
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    let task_id = create_unpack_cancelled_order_task_tx(
        &mut tx,
        tenant_id,
        user_id,
        order_id,
        facility_id,
        priority,
        assigned_user_id,
        scheduled_for,
        due_at,
        instructions,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(task_id)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_unpack_cancelled_order_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: Option<i64>,
    order_id: i64,
    facility_id: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
    scope: Option<&ScopeBindings>,
) -> AppResult<i64> {
    let order: Option<(String, i64)> = sqlx::query_as(
        r#"
        SELECT orders.status, orders.inventory_owner_id
        FROM orders
        INNER JOIN inventory_owner_facilities assignment
            ON assignment.tenant_id = orders.tenant_id
           AND assignment.inventory_owner_id = orders.inventory_owner_id
           AND assignment.facility_id = $3
           AND assignment.deleted IS NULL
        INNER JOIN facilities facility
            ON facility.tenant_id = assignment.tenant_id
           AND facility.id = assignment.facility_id
           AND facility.deleted IS NULL
        INNER JOIN inventory_owners inventory_owner
            ON inventory_owner.tenant_id = orders.tenant_id
           AND inventory_owner.id = orders.inventory_owner_id
           AND inventory_owner.deleted IS NULL
        WHERE orders.tenant_id = $1 AND orders.id = $2 AND orders.deleted IS NULL
        FOR UPDATE OF orders
        FOR SHARE OF assignment, facility, inventory_owner
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(facility_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((status, inventory_owner_id)) = order else {
        return Err(if scope.is_some() {
            AppError::not_found("task references")
        } else {
            AppError::bad_request("order not found")
        });
    };
    if scope.is_some_and(|scope| {
        !(TaskDimensions {
            facility_id: Some(facility_id),
            inventory_owner_id: Some(inventory_owner_id),
        })
        .is_allowed_by(scope)
    }) {
        return Err(AppError::not_found("task references"));
    }
    if status != "cancelled" {
        return Err(AppError::bad_request(
            "unpack cancelled order tasks require a cancelled order",
        ));
    }
    lock_unpack_order_lines_tx(tx, tenant_id, inventory_owner_id, order_id).await?;

    let existing: Option<(i64, i64, String, Option<Timestamp>)> = sqlx::query_as(
        r#"
        SELECT task.id, task.facility_id, task.status, task.deleted
        FROM work_tasks task
        INNER JOIN unpack_cancelled_order_tasks detail
            ON detail.tenant_id = task.tenant_id AND detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND task.task_type = 'unpack_cancelled_order'
          AND detail.order_id = $2
        LIMIT 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((existing, existing_facility_id, status, deleted)) = existing {
        if existing_facility_id != facility_id {
            return Err(AppError::conflict(
                "cancelled order already has an unpack task in another facility",
            ));
        }
        if deleted.is_some() || !matches!(status.as_str(), "open" | "assigned" | "in_progress") {
            return Err(AppError::conflict(
                "cancelled order already has terminal unpack work",
            ));
        }
        sqlx::query("UPDATE work_tasks SET modified = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(now_iso())
            .bind(tenant_id.get())
            .bind(existing)
            .execute(&mut **tx)
            .await?;
        return Ok(existing);
    }

    let task_id = insert_task_tx(
        tx,
        tenant_id,
        NewWorkTask {
            facility_id: Some(facility_id),
            inventory_owner_id: Some(inventory_owner_id),
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
    sqlx::query(
        "INSERT INTO unpack_cancelled_order_tasks (tenant_id, facility_id, inventory_owner_id, task_id, order_id) VALUES ($1, $2, $3, $4, $5)",
    )
        .bind(tenant_id.get())
        .bind(facility_id)
        .bind(inventory_owner_id)
        .bind(task_id)
        .bind(order_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO unpack_cancelled_order_task_lines (
            tenant_id, facility_id, inventory_owner_id, task_id, order_item_id, item_id,
            item_batch_id, expected_qty
        )
        SELECT $1, $2, $3, $4, oi.id, oi.item_id, oi.item_batch_id, oi.qty
        FROM order_items oi
        WHERE oi.tenant_id = $1
          AND oi.inventory_owner_id = $3
          AND oi.order_id = $5
          AND oi.deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(inventory_owner_id)
    .bind(task_id)
    .bind(order_id)
    .execute(&mut **tx)
    .await?;
    Ok(task_id)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_unpack_cancelled_order_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    order_id: i64,
    facility_id: i64,
    priority: Option<i64>,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<String>,
) -> AppResult<i64> {
    require_command_context(access, command)?;
    let prepared = PreparedCommand::new(
        command,
        "task.create_unpack_cancelled_order.v1",
        &(
            order_id,
            facility_id,
            priority,
            assigned_user_id,
            scheduled_for,
            due_at,
            &instructions,
        ),
    )?;
    let mut tx = db.begin().await?;
    let current_scope = lock_current_task_scope_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        assigned_user_id,
    )
    .await?;
    if let Some(task_id) = prepared.replayed::<i64>(&mut tx).await? {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &current_scope)
            .await?;
        tx.commit().await?;
        return Ok(task_id);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(access.tenant_id.get())
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    let task_id = create_unpack_cancelled_order_task_tx(
        &mut tx,
        access.tenant_id,
        Some(command.actor_id.get()),
        order_id,
        facility_id,
        priority,
        assigned_user_id,
        scheduled_for,
        due_at,
        instructions,
        Some(&current_scope),
    )
    .await?;
    prepared.commit(tx, task_id).await
}

#[allow(clippy::too_many_arguments)]
async fn item_location_cycle_count_dimensions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    location_id: i64,
    item_id: i64,
    order_id: Option<i64>,
    order_item_id: Option<i64>,
    inventory_balance_id: Option<i64>,
) -> AppResult<TaskDimensions> {
    let facility_id: i64 = sqlx::query_scalar(
        "SELECT facility_id FROM locations WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL AND active",
    )
    .bind(tenant_id.get())
    .bind(location_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::bad_request("location not found"))?;
    let mut inventory_owner_id = None;

    if let Some(inventory_balance_id) = inventory_balance_id {
        let balance: Option<(i64, i64, i64, i64)> = sqlx::query_as(
            r#"
            SELECT inventory_owner_id, facility_id, location_id, item_id
            FROM inventory_balances
            WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_balance_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((owner_id, balance_facility_id, balance_location_id, balance_item_id)) = balance
        else {
            return Err(AppError::bad_request("inventory balance not found"));
        };
        if balance_facility_id != facility_id
            || balance_location_id != location_id
            || balance_item_id != item_id
        {
            return Err(AppError::bad_request(
                "inventory balance does not match the task location and item",
            ));
        }
        inventory_owner_id = Some(owner_id);
    }

    if let Some(order_id) = order_id {
        let owner_id: Option<i64> = sqlx::query_scalar(
            "SELECT inventory_owner_id FROM orders WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL",
        )
        .bind(tenant_id.get())
        .bind(order_id)
        .fetch_optional(&mut **tx)
        .await?;
        merge_task_owner(&mut inventory_owner_id, owner_id, "order")?;
    }

    if let Some(order_item_id) = order_item_id {
        let order_item: Option<(i64, i64, i64)> = sqlx::query_as(
            r#"
            SELECT inventory_owner_id, order_id, item_id
            FROM order_items
            WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
            "#,
        )
        .bind(tenant_id.get())
        .bind(order_item_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((owner_id, item_order_id, order_item_item_id)) = order_item else {
            return Err(AppError::bad_request("order item not found"));
        };
        if order_id.is_some_and(|order_id| order_id != item_order_id)
            || order_item_item_id != item_id
        {
            return Err(AppError::bad_request(
                "order item does not match the task order and item",
            ));
        }
        merge_task_owner(&mut inventory_owner_id, Some(owner_id), "order item")?;
    }

    Ok(TaskDimensions {
        facility_id: Some(facility_id),
        inventory_owner_id,
    })
}

fn merge_task_owner(
    inventory_owner_id: &mut Option<i64>,
    candidate: Option<i64>,
    reference: &str,
) -> AppResult<()> {
    let candidate =
        candidate.ok_or_else(|| AppError::bad_request(format!("{reference} not found")))?;
    if inventory_owner_id.is_some_and(|owner_id| owner_id != candidate) {
        return Err(AppError::bad_request(
            "task references must belong to the same inventory owner",
        ));
    }
    *inventory_owner_id = Some(candidate);
    Ok(())
}

async fn ensure_active_item_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    item_id: i64,
) -> AppResult<()> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM items WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(item_id)
    .fetch_optional(&mut **tx)
    .await?;
    if found.is_none() {
        return Err(AppError::bad_request("item not found"));
    }
    Ok(())
}
