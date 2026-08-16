use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::work_orchestration::{
    ActivateWorkOrchestrationDispatchCommand, ActivateWorkOrchestrationDispatchResult,
    CancelWorkOrchestrationDispatchCommand, CancelWorkOrchestrationDispatchResult,
    ACTIVATE_WORK_ORCHESTRATION_DISPATCH_OPERATION, CANCEL_WORK_ORCHESTRATION_DISPATCH_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, WorkOrchestrationDispatchId, WorkOrchestrationDispatchStatus,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::query;
use super::scope::{bind_actor_tx, require_command_scope};
use super::SUPERVISOR_PERMISSION;
use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

fn validate_cancellation_note(command: &CancelWorkOrchestrationDispatchCommand) -> AppResult<()> {
    if command.reason.as_str() == "other" && command.note.is_none() {
        return Err(AppError::bad_request(
            "a note is required for the other cancellation reason",
        ));
    }
    if let Some(note) = command.note.as_deref() {
        if note.trim() != note
            || note.is_empty()
            || note.chars().count() > 500
            || note.chars().any(char::is_control)
        {
            return Err(AppError::bad_request(
                "dispatch cancellation note must be trimmed, printable, and at most 500 characters",
            ));
        }
    }
    Ok(())
}

pub async fn activate_dispatch(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ActivateWorkOrchestrationDispatchCommand,
) -> AppResult<ActivateWorkOrchestrationDispatchResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.tenant_id != access.tenant_id {
        return Err(AppError::not_found("work orchestration plan"));
    }
    let prepared = PreparedCommand::new_v1(
        context,
        ACTIVATE_WORK_ORCHESTRATION_DISPATCH_OPERATION,
        command,
    )?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    bind_actor_tx(&mut tx, context.actor_id).await?;
    if let Some(result) = prepared
        .replayed::<ActivateWorkOrchestrationDispatchResult>(&mut tx)
        .await?
    {
        require_command_scope(
            &scope,
            result.facility_id,
            result.inventory_owner_id,
            "work orchestration dispatch",
        )?;
        tx.commit().await?;
        return Ok(result);
    }

    let plan = sqlx::query(
        r#"SELECT facility_id,requested_inventory_owner_id,generated_for_user_id,item_count
        FROM work_orchestration_plans WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.plan_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("work orchestration plan"))?;
    let facility_id = FacilityId::new(plan.try_get("facility_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let inventory_owner_id = plan
        .try_get::<Option<i64>, _>("requested_inventory_owner_id")?
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    require_command_scope(
        &scope,
        facility_id,
        inventory_owner_id,
        "work orchestration dispatch",
    )?;
    let worker_user_id: i64 = plan
        .try_get::<Option<i64>, _>("generated_for_user_id")?
        .ok_or_else(|| {
            AppError::conflict("generate the plan for an eligible worker before dispatching it")
        })?;
    if plan.try_get::<i64, _>("item_count")? <= 0 {
        return Err(AppError::conflict("an empty plan cannot be dispatched"));
    }

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "work-orchestration-dispatch:{}:{worker_user_id}",
            access.tenant_id.get()
        ))
        .execute(&mut *tx)
        .await?;
    let task_rows = sqlx::query(
        r#"SELECT task.id,task.status,task.assigned_user_id
        FROM work_orchestration_plan_items item
        JOIN work_tasks task ON task.tenant_id=item.tenant_id AND task.id=item.work_task_id
        WHERE item.tenant_id=$1 AND item.plan_id=$2
        ORDER BY task.id FOR UPDATE OF task"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.plan_id.get())
    .fetch_all(&mut *tx)
    .await?;
    if task_rows.iter().any(|row| {
        row.try_get::<String, _>("status").ok().as_deref() != Some("open")
            || row
                .try_get::<Option<i64>, _>("assigned_user_id")
                .ok()
                .flatten()
                .is_some()
    }) {
        return Err(AppError::conflict(
            "the plan is stale because one or more tasks are no longer open",
        ));
    }
    let activated_at = now_iso();
    let dispatch_id = WorkOrchestrationDispatchId::new(
        sqlx::query_scalar(
            r#"INSERT INTO work_orchestration_dispatches (
              tenant_id,facility_id,inventory_owner_id,plan_id,worker_user_id,
              activated_by_user_id,activated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(facility_id.get())
        .bind(inventory_owner_id.map(InventoryOwnerId::get))
        .bind(command.plan_id.get())
        .bind(worker_user_id)
        .bind(context.actor_id.get())
        .bind(activated_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    sqlx::query(
        r#"INSERT INTO work_orchestration_dispatch_items (
          tenant_id,dispatch_id,plan_item_id,sequence,work_task_id)
        SELECT tenant_id,$3,id,sequence,work_task_id
        FROM work_orchestration_plan_items
        WHERE tenant_id=$1 AND plan_id=$2 ORDER BY sequence"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.plan_id.get())
    .bind(dispatch_id.get())
    .execute(&mut *tx)
    .await?;
    let assigned = sqlx::query(
        r#"UPDATE work_tasks task SET status='assigned',assigned_user_id=$1,modified=$2
        FROM work_orchestration_dispatch_items item
        WHERE item.tenant_id=$3 AND item.dispatch_id=$4 AND item.sequence=1
          AND task.tenant_id=item.tenant_id AND task.id=item.work_task_id
          AND task.status='open' AND task.assigned_user_id IS NULL AND task.deleted IS NULL"#,
    )
    .bind(worker_user_id)
    .bind(activated_at)
    .bind(access.tenant_id.get())
    .bind(dispatch_id.get())
    .execute(&mut *tx)
    .await?;
    if assigned.rows_affected() != 1 {
        return Err(AppError::conflict(
            "the first dispatched task is no longer available",
        ));
    }
    sqlx::query("SET CONSTRAINTS work_orchestration_dispatch_require_items IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    let result = query::read_dispatch_tx(&mut tx, access.tenant_id, dispatch_id).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn cancel_dispatch(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelWorkOrchestrationDispatchCommand,
) -> AppResult<CancelWorkOrchestrationDispatchResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.tenant_id != access.tenant_id {
        return Err(AppError::not_found("work orchestration dispatch"));
    }
    validate_cancellation_note(command)?;
    let prepared = PreparedCommand::new_v1(
        context,
        CANCEL_WORK_ORCHESTRATION_DISPATCH_OPERATION,
        command,
    )?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    bind_actor_tx(&mut tx, context.actor_id).await?;
    if let Some(result) = prepared
        .replayed::<CancelWorkOrchestrationDispatchResult>(&mut tx)
        .await?
    {
        require_command_scope(
            &scope,
            result.facility_id,
            result.inventory_owner_id,
            "work orchestration dispatch",
        )?;
        tx.commit().await?;
        return Ok(result);
    }
    let target = sqlx::query(
        r#"SELECT facility_id,inventory_owner_id,status,revision
        FROM work_orchestration_dispatches WHERE tenant_id=$1 AND id=$2 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.dispatch_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("work orchestration dispatch"))?;
    let facility_id = FacilityId::new(target.try_get("facility_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let inventory_owner_id = target
        .try_get::<Option<i64>, _>("inventory_owner_id")?
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    require_command_scope(
        &scope,
        facility_id,
        inventory_owner_id,
        "work orchestration dispatch",
    )?;
    if target.try_get::<String, _>("status")? != WorkOrchestrationDispatchStatus::Active.as_str()
        || target.try_get::<i64, _>("revision")? != command.expected_revision.get()
    {
        return Err(AppError::conflict(
            "work orchestration dispatch revision or status changed",
        ));
    }
    let ended_at = now_iso();
    let updated = sqlx::query(
        r#"UPDATE work_orchestration_dispatches SET status='cancelled',revision=revision+1,
          ended_by_user_id=$1,ended_at=$2,cancellation_reason=$3,cancellation_note=$4
        WHERE tenant_id=$5 AND id=$6 AND status='active' AND revision=$7"#,
    )
    .bind(context.actor_id.get())
    .bind(ended_at)
    .bind(command.reason.as_str())
    .bind(command.note.as_deref())
    .bind(access.tenant_id.get())
    .bind(command.dispatch_id.get())
    .bind(command.expected_revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "work orchestration dispatch revision or status changed",
        ));
    }
    let result = query::read_dispatch_tx(&mut tx, access.tenant_id, command.dispatch_id).await?;
    Ok(prepared.commit(tx, result).await?)
}
