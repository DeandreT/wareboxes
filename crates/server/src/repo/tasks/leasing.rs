use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{CommandContext, TenantId};

use crate::db::{now_iso, Db};
use crate::error::AppResult;
use crate::repo::access::{current_scope_tx, lock_current_scope_tx, lock_user_tx};
use crate::repo::idempotency::PreparedCommand;

use super::execution::{
    insert_progress_tx, task_assignment_requirements_tx, user_can_execute_task_tx,
};
use super::{require_command_context, ScopeBindings};

pub async fn assign_task(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    assigned_user_id: i64,
) -> AppResult<bool> {
    assign_task_with_scope(db, tenant_id, task_id, assigned_user_id, None, None).await
}

async fn assign_task_with_scope(
    db: &Db,
    tenant_id: TenantId,
    task_id: i64,
    assigned_user_id: i64,
    scope_user_id: Option<i64>,
    command: Option<&CommandContext>,
) -> AppResult<bool> {
    let prepared = command
        .map(|command| {
            PreparedCommand::new(command, "task.assign.v1", &(task_id, assigned_user_id))
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    let mut lock_user_ids = vec![assigned_user_id];
    if let Some(scope_user_id) = scope_user_id {
        lock_user_ids.push(scope_user_id);
    }
    lock_user_ids.sort_unstable();
    lock_user_ids.dedup();
    for lock_user_id in lock_user_ids {
        lock_user_tx(&mut tx, tenant_id, lock_user_id).await?;
    }
    let current_scope = match scope_user_id {
        Some(scope_user_id) => Some(current_scope_tx(&mut tx, tenant_id, scope_user_id).await?),
        None => None,
    };
    let Some((dimensions, required_permission)) =
        task_assignment_requirements_tx(&mut tx, tenant_id, task_id).await?
    else {
        return Ok(false);
    };
    if current_scope
        .as_ref()
        .is_some_and(|scope| !dimensions.is_allowed_by(scope))
    {
        return Ok(false);
    }
    if let Some(prepared) = prepared.as_ref() {
        if let Some(assigned) = prepared.replayed::<bool>(&mut tx).await? {
            tx.commit().await?;
            return Ok(assigned);
        }
    }
    if !user_can_execute_task_tx(
        &mut tx,
        tenant_id,
        assigned_user_id,
        dimensions,
        &required_permission,
    )
    .await?
    {
        return Ok(false);
    }
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
    .bind(assigned_user_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    if active.is_some() {
        return Ok(false);
    }
    let res = sqlx::query(
        r#"
        UPDATE work_tasks
        SET assigned_user_id = $1, status = 'assigned', modified = $2
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND status = 'open'
        "#,
    )
    .bind(assigned_user_id)
    .bind(now_iso())
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    let assigned = res.rows_affected() > 0;
    if !assigned {
        return Ok(false);
    }
    match prepared {
        Some(prepared) => prepared.commit(tx, assigned).await,
        None => {
            tx.commit().await?;
            Ok(assigned)
        }
    }
}

pub async fn assign_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    assigned_user_id: i64,
) -> AppResult<bool> {
    require_command_context(access, command)?;
    assign_task_with_scope(
        db,
        access.tenant_id,
        task_id,
        assigned_user_id,
        Some(access.user_id.get()),
        Some(command),
    )
    .await
}

pub async fn release_expired_tasks(db: &Db, tenant_id: TenantId) -> AppResult<u64> {
    release_expired_tasks_with_scope(db, tenant_id, None, &ScopeBindings::unrestricted(), None)
        .await
}

pub async fn release_expired_tasks_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
) -> AppResult<u64> {
    require_command_context(access, command)?;
    release_expired_tasks_with_scope(
        db,
        access.tenant_id,
        Some(access.user_id.get()),
        &ScopeBindings::for_access(access),
        Some(command),
    )
    .await
}

pub(super) async fn release_expired_tasks_with_scope(
    db: &Db,
    tenant_id: TenantId,
    scope_user_id: Option<i64>,
    scope: &ScopeBindings,
    command: Option<&CommandContext>,
) -> AppResult<u64> {
    let prepared = command
        .map(|command| PreparedCommand::new(command, "task.release_expired.v1", &()))
        .transpose()?;
    let mut tx = db.begin().await?;
    let current_scope = match scope_user_id {
        Some(scope_user_id) => {
            Some(lock_current_scope_tx(&mut tx, tenant_id, scope_user_id).await?)
        }
        None => None,
    };
    let scope = current_scope.as_ref().unwrap_or(scope);
    if let Some(prepared) = prepared.as_ref() {
        if let Some(released) = prepared.replayed::<u64>(&mut tx).await? {
            tx.commit().await?;
            return Ok(released);
        }
    }
    let released = release_expired_tasks_tx(&mut tx, tenant_id, None, scope).await?;
    let count = released.len() as u64;
    match prepared {
        Some(prepared) => {
            prepared.commit(tx, count).await?;
        }
        None => tx.commit().await?,
    }
    Ok(count)
}

pub(super) async fn release_expired_tasks_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    assigned_user_id: Option<i64>,
    scope: &ScopeBindings,
) -> AppResult<Vec<i64>> {
    let now = now_iso();
    let task_ids = sqlx::query_scalar::<_, i64>(
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
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
          AND lease_expires_at IS NOT NULL
          AND lease_expires_at <= $1
          AND completed_at IS NULL
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
          AND ($7::BIGINT IS NULL OR assigned_user_id = $7)
        RETURNING id
        "#,
    )
    .bind(now)
    .bind(tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(assigned_user_id)
    .fetch_all(&mut **tx)
    .await?;
    for task_id in &task_ids {
        insert_progress_tx(
            tx, tenant_id, *task_id, None, None, "expired", None, None, None, None, None,
        )
        .await?;
    }
    Ok(task_ids)
}

pub(super) async fn release_inaccessible_active_tasks_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let now = now_iso();
    let task_ids = sqlx::query_scalar::<_, i64>(
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
          AND assigned_user_id = $3
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
          AND completed_at IS NULL
          AND NOT COALESCE(
              ($4 OR facility_id = ANY($5))
              AND ($6 OR inventory_owner_id = ANY($7)),
              FALSE
          )
        RETURNING id
        "#,
    )
    .bind(now)
    .bind(tenant_id.get())
    .bind(user_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut **tx)
    .await?;
    for task_id in task_ids {
        insert_progress_tx(
            tx,
            tenant_id,
            task_id,
            None,
            Some(user_id),
            "scope_revoked",
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    }
    Ok(())
}
