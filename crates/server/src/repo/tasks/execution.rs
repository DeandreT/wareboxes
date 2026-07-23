use sqlx::Row;
use wareboxes_core::models::{TenantAccess, WorkTask, WorkTaskProgressAction, WorkTaskType};
use wareboxes_domain::{CommandContext, TenantId};

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::permissions;
use crate::repo::idempotency::PreparedCommand;

use super::leasing::{release_expired_tasks_tx, release_inaccessible_active_tasks_tx};
use super::{map_task, require_command_context, ScopeBindings, TaskDimensions};
use crate::repo::access::lock_current_scope_tx;

pub async fn start_next_task(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    task_type: Option<WorkTaskType>,
) -> AppResult<Option<WorkTask>> {
    start_next_task_with_scope(
        db,
        tenant_id,
        user_id,
        task_type,
        None,
        &ScopeBindings::unrestricted(),
        None,
    )
    .await
}

pub async fn start_next_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_type: Option<WorkTaskType>,
) -> AppResult<Option<WorkTask>> {
    require_command_context(access, command)?;
    start_next_task_with_scope(
        db,
        access.tenant_id,
        command.actor_id.get(),
        task_type,
        Some(command.actor_id.get()),
        &ScopeBindings::for_access(access),
        Some(command),
    )
    .await
}

async fn start_next_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    task_type: Option<WorkTaskType>,
    scope_user_id: Option<i64>,
    scope: &ScopeBindings,
    command: Option<&CommandContext>,
) -> AppResult<Option<WorkTask>> {
    let prepared = command
        .map(|command| PreparedCommand::new(command, "task.start_next.v1", &task_type))
        .transpose()?;
    let permissions = permissions::get_user_permissions(db, tenant_id, user_id).await?;
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

    let mut tx = db.begin().await?;
    let current_scope = match scope_user_id {
        Some(scope_user_id) => {
            Some(lock_current_scope_tx(&mut tx, tenant_id, scope_user_id).await?)
        }
        None => None,
    };
    if scope_user_id.is_none() {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))",
        )
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }
    let scope = current_scope.as_ref().unwrap_or(scope);
    if let Some(prepared) = prepared.as_ref() {
        if let Some(task) = prepared.replayed::<Option<WorkTask>>(&mut tx).await? {
            if let Some(task) = task.as_ref() {
                require_current_task_replay_tx(&mut tx, tenant_id, user_id, task.id, scope).await?;
            }
            tx.commit().await?;
            return Ok(task);
        }
    }
    release_expired_tasks_tx(&mut tx, tenant_id, Some(user_id), scope).await?;
    release_inaccessible_active_tasks_tx(&mut tx, tenant_id, user_id, scope).await?;
    let active: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM work_tasks
        WHERE tenant_id = $1
          AND deleted IS NULL
          AND assigned_user_id = $2
          AND status IN ('assigned', 'in_progress')
        LIMIT 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .fetch_optional(&mut *tx)
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
            WHERE tenant_id = $6
              AND deleted IS NULL
              AND status = 'open'
              AND (scheduled_for IS NULL OR scheduled_for <= $1)
              AND ($2 OR LOWER(required_permission) = ANY($3))
              AND ($4::TEXT IS NULL OR task_type = $4)
              AND ($7 OR facility_id = ANY($8))
              AND ($9 OR inventory_owner_id = ANY($10))
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
        WHERE task.tenant_id = $6 AND task.id = candidate.id
        RETURNING task.id, task.tenant_id, task.facility_id, task.inventory_owner_id, task.created,
                  task.modified, task.deleted, task.task_type, task.status, task.required_permission,
                  task.priority, task.title, task.instructions,
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
    .bind(tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(row) = row.as_ref() {
        let task_id: i64 = row.try_get("id")?;
        insert_progress_tx(
            &mut tx,
            tenant_id,
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
    let task = row.as_ref().map(map_task).transpose()?;
    match prepared {
        Some(prepared) => prepared.commit(tx, task).await,
        None => {
            tx.commit().await?;
            Ok(task)
        }
    }
}

async fn require_current_task_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    task_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id, assigned_user_id, status,
               lease_expires_at > statement_timestamp() AS lease_is_current
        FROM work_tasks
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Err(AppError::conflict("task claim is no longer active"));
    };
    let dimensions = TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    };
    if !dimensions.is_allowed_by(scope) {
        return Err(AppError::not_found("task"));
    }
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    let status: String = row.try_get("status")?;
    let lease_is_current: Option<bool> = row.try_get("lease_is_current")?;
    if assigned_user_id != Some(user_id)
        || !matches!(status.as_str(), "assigned" | "in_progress")
        || lease_is_current == Some(false)
    {
        return Err(AppError::conflict("task claim is no longer active"));
    }
    Ok(())
}

pub async fn start_task(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
) -> AppResult<bool> {
    start_task_with_scope(
        db,
        tenant_id,
        task_id,
        user_id,
        None,
        &ScopeBindings::unrestricted(),
        None,
    )
    .await
}

pub async fn start_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
) -> AppResult<bool> {
    require_command_context(access, command)?;
    start_task_with_scope(
        db,
        access.tenant_id,
        task_id,
        command.actor_id.get(),
        Some(command.actor_id.get()),
        &ScopeBindings::for_access(access),
        Some(command),
    )
    .await
}

async fn start_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
    scope_user_id: Option<i64>,
    scope: &ScopeBindings,
    command: Option<&CommandContext>,
) -> AppResult<bool> {
    let prepared = command
        .map(|command| PreparedCommand::new(command, "task.start.v1", &task_id))
        .transpose()?;
    let now = now_iso();
    let mut tx = db.begin().await?;
    let current_scope = match scope_user_id {
        Some(scope_user_id) => {
            let Some(scope) =
                lock_task_in_current_scope_tx(&mut tx, tenant_id, scope_user_id, task_id).await?
            else {
                return Err(AppError::not_found("task"));
            };
            Some(scope)
        }
        None => None,
    };
    if scope_user_id.is_none() {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))",
        )
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }
    let scope = current_scope.as_ref().unwrap_or(scope);
    if let Some(prepared) = prepared.as_ref() {
        if let Some(started) = prepared.replayed::<bool>(&mut tx).await? {
            tx.commit().await?;
            return Ok(started);
        }
    }
    release_expired_tasks_tx(&mut tx, tenant_id, Some(user_id), scope).await?;
    release_inaccessible_active_tasks_tx(&mut tx, tenant_id, user_id, scope).await?;
    let active: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM work_tasks
        WHERE tenant_id = $1
          AND assigned_user_id = $2
          AND id <> $3
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
        LIMIT 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    if active.is_some() {
        return Ok(false);
    }
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET assigned_user_id = COALESCE(assigned_user_id, $1),
            status = 'in_progress',
            started_at = COALESCE(started_at, $2),
            lease_expires_at = $2 + make_interval(secs => task_timeout_seconds::INT),
            modified = $2
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND status IN ('open', 'assigned')
          AND (assigned_user_id IS NULL OR assigned_user_id = $1)
          AND ($5 OR facility_id = ANY($6))
          AND ($7 OR inventory_owner_id = ANY($8))
        "#,
    )
    .bind(user_id)
    .bind(now)
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() > 0 {
        insert_progress_tx(
            &mut tx,
            tenant_id,
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
    let started = res.rows_affected() > 0;
    if !started {
        return Ok(false);
    }
    match prepared {
        Some(prepared) => prepared.commit(tx, started).await,
        None => {
            tx.commit().await?;
            Ok(started)
        }
    }
}

async fn lock_task_in_current_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope_user_id: i64,
    task_id: i64,
) -> AppResult<Option<ScopeBindings>> {
    let scope = lock_current_scope_tx(tx, tenant_id, scope_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id
        FROM work_tasks
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let dimensions = TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    };
    Ok(dimensions.is_allowed_by(&scope).then_some(scope))
}

async fn progress_locations_match_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
    scope: &ScopeBindings,
    from_location_id: Option<i64>,
    to_location_id: Option<i64>,
) -> AppResult<bool> {
    let mut location_ids = [from_location_id, to_location_id]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    location_ids.sort_unstable();
    location_ids.dedup();
    if location_ids.is_empty() {
        return Ok(true);
    }
    let rows = sqlx::query(
        r#"
        SELECT location.id
        FROM locations location
        INNER JOIN work_tasks task
            ON task.tenant_id = location.tenant_id
           AND task.id = $2
        WHERE location.tenant_id = $1
          AND location.id = ANY($3)
          AND location.deleted IS NULL
          AND location.active
          AND location.facility_id = task.facility_id
          AND ($4 OR location.facility_id = ANY($5))
        FOR SHARE OF location
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(&location_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.len() == location_ids.len())
}

#[allow(clippy::too_many_arguments)]
pub async fn record_task_progress(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    task_id: i64,
    task_line_id: Option<i64>,
    action: WorkTaskProgressAction,
    qty_completed: i64,
    from_location_id: Option<i64>,
    to_location_id: Option<i64>,
    note: Option<&str>,
) -> AppResult<bool> {
    record_task_progress_with_scope(
        db,
        tenant_id,
        user_id,
        None,
        task_id,
        task_line_id,
        action,
        qty_completed,
        from_location_id,
        to_location_id,
        note,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn record_task_progress_with_scope(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    scope_user_id: Option<i64>,
    task_id: i64,
    task_line_id: Option<i64>,
    action: WorkTaskProgressAction,
    qty_completed: i64,
    from_location_id: Option<i64>,
    to_location_id: Option<i64>,
    note: Option<&str>,
    command: Option<&CommandContext>,
) -> AppResult<bool> {
    if qty_completed <= 0 {
        return Err(AppError::bad_request("completed quantity must be positive"));
    }
    let prepared = command
        .map(|command| {
            PreparedCommand::new(
                command,
                "task.progress.v1",
                &(
                    task_id,
                    task_line_id,
                    action,
                    qty_completed,
                    from_location_id,
                    to_location_id,
                    note,
                ),
            )
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    if let Some(scope_user_id) = scope_user_id {
        let Some(scope) =
            lock_task_in_current_scope_tx(&mut tx, tenant_id, scope_user_id, task_id).await?
        else {
            return Ok(false);
        };
        if let Some(prepared) = prepared.as_ref() {
            if let Some(updated) = prepared.replayed::<bool>(&mut tx).await? {
                tx.commit().await?;
                return Ok(updated);
            }
        }
        if !progress_locations_match_task_tx(
            &mut tx,
            tenant_id,
            task_id,
            &scope,
            from_location_id,
            to_location_id,
        )
        .await?
        {
            return Ok(false);
        }
    }
    if scope_user_id.is_none() {
        if let Some(prepared) = prepared.as_ref() {
            if let Some(updated) = prepared.replayed::<bool>(&mut tx).await? {
                tx.commit().await?;
                return Ok(updated);
            }
        }
    }
    let updated = record_task_progress_tx(
        &mut tx,
        tenant_id,
        user_id,
        task_id,
        task_line_id,
        action,
        qty_completed,
        from_location_id,
        to_location_id,
        note,
    )
    .await?;
    if !updated {
        return Ok(false);
    }
    match prepared {
        Some(prepared) => prepared.commit(tx, updated).await,
        None => {
            tx.commit().await?;
            Ok(updated)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn record_task_progress_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    task_line_id: Option<i64>,
    action: WorkTaskProgressAction,
    qty_completed: i64,
    from_location_id: Option<i64>,
    to_location_id: Option<i64>,
    note: Option<&str>,
) -> AppResult<bool> {
    require_command_context(access, command)?;
    record_task_progress_with_scope(
        db,
        access.tenant_id,
        command.actor_id.get(),
        Some(command.actor_id.get()),
        task_id,
        task_line_id,
        action,
        qty_completed,
        from_location_id,
        to_location_id,
        note,
        Some(command),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn record_task_progress_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    task_id: i64,
    task_line_id: Option<i64>,
    action: WorkTaskProgressAction,
    qty_completed: i64,
    from_location_id: Option<i64>,
    to_location_id: Option<i64>,
    note: Option<&str>,
) -> AppResult<bool> {
    let task_type = task_type_tx(tx, tenant_id, task_id).await?;
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
                WHERE detail.tenant_id = task.tenant_id
                  AND detail.task_id = task.id
                  AND task.tenant_id = $2
                  AND detail.task_id = $3
                  AND task.deleted IS NULL
                  AND task.status IN ('assigned', 'in_progress')
                  AND task.assigned_user_id = $4
                  AND detail.master_qty_completed + $1 <= detail.master_qty
                "#,
            )
            .bind(qty_completed)
            .bind(tenant_id.get())
            .bind(task_id)
            .bind(user_id)
            .execute(&mut **tx)
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
                WHERE line.tenant_id = task.tenant_id
                  AND line.task_id = task.id
                  AND task.tenant_id = $8
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
            .bind(tenant_id.get())
            .execute(&mut **tx)
            .await?;
            res.rows_affected() > 0
        }
        WorkTaskType::CycleCountItemLocation | WorkTaskType::CycleCountLocation => false,
    };
    if updated {
        sqlx::query("UPDATE work_tasks SET modified = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(now_iso())
            .bind(tenant_id.get())
            .bind(task_id)
            .execute(&mut **tx)
            .await?;
        insert_progress_tx(
            tx,
            tenant_id,
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
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
    qty_completed: Option<i64>,
) -> AppResult<bool> {
    complete_task_with_scope(db, tenant_id, task_id, user_id, qty_completed, None, None).await
}

async fn complete_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
    qty_completed: Option<i64>,
    scope_user_id: Option<i64>,
    command: Option<&CommandContext>,
) -> AppResult<bool> {
    if qty_completed.is_some_and(|quantity| quantity <= 0) {
        return Err(AppError::bad_request("completed quantity must be positive"));
    }
    let prepared = command
        .map(|command| PreparedCommand::new(command, "task.complete.v1", &(task_id, qty_completed)))
        .transpose()?;
    let mut tx = db.begin().await?;
    if let Some(scope_user_id) = scope_user_id {
        if lock_task_in_current_scope_tx(&mut tx, tenant_id, scope_user_id, task_id)
            .await?
            .is_none()
        {
            return Ok(false);
        }
    }
    if let Some(prepared) = prepared.as_ref() {
        if let Some(completed) = prepared.replayed::<bool>(&mut tx).await? {
            tx.commit().await?;
            return Ok(completed);
        }
    }
    if let Some(qty_completed) = qty_completed {
        if !record_task_progress_tx(
            &mut tx,
            tenant_id,
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
    let locked: Option<i64> =
        sqlx::query_scalar("SELECT id FROM work_tasks WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
            .bind(tenant_id.get())
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?;
    if locked.is_none() {
        return Ok(false);
    }
    let task_type = task_type_tx(&mut tx, tenant_id, task_id).await?;
    let detail_complete = match task_type {
        WorkTaskType::BreakMasterPack => {
            let complete: bool = sqlx::query_scalar(
                "SELECT master_qty_completed >= master_qty FROM break_master_pack_tasks WHERE tenant_id = $1 AND task_id = $2",
            )
            .bind(tenant_id.get())
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or(false);
            complete
        }
        WorkTaskType::UnpackCancelledOrder => {
            let open_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM unpack_cancelled_order_task_lines WHERE tenant_id = $1 AND task_id = $2 AND status IN ('open', 'partial')",
            )
            .bind(tenant_id.get())
            .bind(task_id)
            .fetch_one(&mut *tx)
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
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
          AND (assigned_user_id IS NULL OR assigned_user_id = $1)
        "#,
    )
    .bind(user_id)
    .bind(now)
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() > 0 {
        insert_progress_tx(
            &mut tx,
            tenant_id,
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
    let completed = res.rows_affected() > 0;
    if !completed {
        return Ok(false);
    }
    match prepared {
        Some(prepared) => prepared.commit(tx, completed).await,
        None => {
            tx.commit().await?;
            Ok(completed)
        }
    }
}

pub async fn complete_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    qty_completed: Option<i64>,
) -> AppResult<bool> {
    require_command_context(access, command)?;
    complete_task_with_scope(
        db,
        access.tenant_id,
        task_id,
        command.actor_id.get(),
        qty_completed,
        Some(command.actor_id.get()),
        Some(command),
    )
    .await
}

pub async fn abort_task(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
) -> AppResult<bool> {
    abort_task_with_scope(db, tenant_id, task_id, user_id, None, None).await
}

async fn abort_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
    scope_user_id: Option<i64>,
    command: Option<&CommandContext>,
) -> AppResult<bool> {
    let prepared = command
        .map(|command| PreparedCommand::new(command, "task.abort.v1", &task_id))
        .transpose()?;
    let mut tx = db.begin().await?;
    if let Some(scope_user_id) = scope_user_id {
        if lock_task_in_current_scope_tx(&mut tx, tenant_id, scope_user_id, task_id)
            .await?
            .is_none()
        {
            return Ok(false);
        }
    }
    if let Some(prepared) = prepared.as_ref() {
        if let Some(aborted) = prepared.replayed::<bool>(&mut tx).await? {
            tx.commit().await?;
            return Ok(aborted);
        }
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
        WHERE tenant_id = $2
          AND id = $3
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
          AND assigned_user_id = $4
          AND completed_at IS NULL
        "#,
    )
    .bind(now_iso())
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() > 0 {
        insert_progress_tx(
            &mut tx,
            tenant_id,
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
    let aborted = res.rows_affected() > 0;
    if !aborted {
        return Ok(false);
    }
    match prepared {
        Some(prepared) => prepared.commit(tx, aborted).await,
        None => {
            tx.commit().await?;
            Ok(aborted)
        }
    }
}

pub async fn abort_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
) -> AppResult<bool> {
    require_command_context(access, command)?;
    abort_task_with_scope(
        db,
        access.tenant_id,
        task_id,
        command.actor_id.get(),
        Some(command.actor_id.get()),
        Some(command),
    )
    .await
}

pub async fn cancel_task(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
) -> AppResult<bool> {
    cancel_task_with_scope(db, tenant_id, task_id, user_id, None, None).await
}

async fn cancel_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    user_id: i64,
    scope_user_id: Option<i64>,
    command: Option<&CommandContext>,
) -> AppResult<bool> {
    let prepared = command
        .map(|command| PreparedCommand::new(command, "task.cancel.v1", &task_id))
        .transpose()?;
    let mut tx = db.begin().await?;
    if let Some(scope_user_id) = scope_user_id {
        if lock_task_in_current_scope_tx(&mut tx, tenant_id, scope_user_id, task_id)
            .await?
            .is_none()
        {
            return Ok(false);
        }
    }
    if let Some(prepared) = prepared.as_ref() {
        if let Some(cancelled) = prepared.replayed::<bool>(&mut tx).await? {
            tx.commit().await?;
            return Ok(cancelled);
        }
    }
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'cancelled',
            completed_by = $1,
            completed_at = $2,
            lease_expires_at = NULL,
            modified = $2
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND status IN ('open', 'assigned', 'in_progress')
        "#,
    )
    .bind(user_id)
    .bind(now_iso())
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() > 0 {
        insert_progress_tx(
            &mut tx,
            tenant_id,
            task_id,
            None,
            Some(user_id),
            "cancelled",
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    }
    let cancelled = res.rows_affected() > 0;
    if !cancelled {
        return Ok(false);
    }
    match prepared {
        Some(prepared) => prepared.commit(tx, cancelled).await,
        None => {
            tx.commit().await?;
            Ok(cancelled)
        }
    }
}

pub async fn cancel_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
) -> AppResult<bool> {
    require_command_context(access, command)?;
    cancel_task_with_scope(
        db,
        access.tenant_id,
        task_id,
        command.actor_id.get(),
        Some(command.actor_id.get()),
        Some(command),
    )
    .await
}

async fn task_type_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
) -> AppResult<WorkTaskType> {
    let value: String = sqlx::query_scalar(
        "SELECT task_type FROM work_tasks WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::bad_request("task not found"))?;
    WorkTaskType::parse(&value)
        .ok_or_else(|| AppError::internal(format!("invalid work task type in database: {value}")))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_progress_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
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
            tenant_id, created, task_id, facility_id, inventory_owner_id, task_line_id,
            user_id, action, qty_delta, from_location_id, to_location_id, note, metadata_json
        )
        SELECT task.tenant_id, $2, task.id, task.facility_id, task.inventory_owner_id,
               $4, $5, $6, $7, $8, $9, $10, $11
        FROM work_tasks task
        WHERE task.tenant_id = $1 AND task.id = $3
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
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
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

pub(super) async fn task_is_accessible(
    db: &Db,
    access: &TenantAccess,
    task_id: i64,
) -> AppResult<bool> {
    let scope = ScopeBindings::for_access(access);
    sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM work_tasks
            WHERE tenant_id = $1
              AND id = $2
              AND deleted IS NULL
              AND ($3 OR facility_id = ANY($4))
              AND ($5 OR inventory_owner_id = ANY($6))
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(db)
    .await
    .map_err(AppError::from)
}

pub(super) async fn task_assignment_requirements_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
) -> AppResult<Option<(TaskDimensions, String)>> {
    let row = sqlx::query(
        "SELECT facility_id, inventory_owner_id, required_permission FROM work_tasks WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref()
        .map(|row| {
            Ok((
                TaskDimensions {
                    facility_id: row.try_get("facility_id")?,
                    inventory_owner_id: row.try_get("inventory_owner_id")?,
                },
                row.try_get("required_permission")?,
            ))
        })
        .transpose()
}

pub(super) async fn user_can_execute_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    dimensions: TaskDimensions,
    required_permission: &str,
) -> AppResult<bool> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM tenant_memberships membership
            WHERE membership.tenant_id = $1
              AND membership.user_id = $2
              AND membership.deleted IS NULL
              AND (
                  ($3::BIGINT IS NULL AND membership.all_facilities)
                  OR (
                      $3::BIGINT IS NOT NULL
                      AND (
                          membership.all_facilities
                          OR EXISTS (
                              SELECT 1
                              FROM user_facilities user_facility
                              WHERE user_facility.tenant_id = membership.tenant_id
                                AND user_facility.user_id = membership.user_id
                                AND user_facility.facility_id = $3
                                AND user_facility.deleted IS NULL
                          )
                      )
                  )
              )
              AND (
                  ($4::BIGINT IS NULL AND membership.all_inventory_owners)
                  OR (
                      $4::BIGINT IS NOT NULL
                      AND (
                          membership.all_inventory_owners
                          OR EXISTS (
                              SELECT 1
                              FROM user_inventory_owners user_owner
                              WHERE user_owner.tenant_id = membership.tenant_id
                                AND user_owner.user_id = membership.user_id
                                AND user_owner.inventory_owner_id = $4
                                AND user_owner.deleted IS NULL
                          )
                      )
                  )
              )
              AND EXISTS (
                  SELECT 1
                  FROM user_roles user_role
                  INNER JOIN roles role
                     ON role.tenant_id = user_role.tenant_id
                    AND role.id = user_role.role_id
                    AND role.deleted IS NULL
                  INNER JOIN role_permissions role_permission
                     ON role_permission.tenant_id = role.tenant_id
                    AND role_permission.role_id = role.id
                    AND role_permission.deleted IS NULL
                  INNER JOIN permissions permission
                     ON permission.tenant_id = role_permission.tenant_id
                    AND permission.id = role_permission.permission_id
                    AND permission.deleted IS NULL
                  WHERE user_role.tenant_id = membership.tenant_id
                    AND user_role.user_id = membership.user_id
                    AND user_role.deleted IS NULL
                    AND LOWER(permission.name) IN ('admin', LOWER($5))
              )
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .bind(dimensions.facility_id)
    .bind(dimensions.inventory_owner_id)
    .bind(required_permission)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)
}
