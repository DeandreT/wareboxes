use sqlx::Row;
use wareboxes_application::cross_dock::{
    CancelCrossDockWorkCommand, CancelCrossDockWorkResult, CANCEL_CROSS_DOCK_WORK_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CrossDockCancellationId, CrossDockPlanId, CrossDockQuantity, CrossDockWorkStatus,
    InventoryOwnerId, OrderId, OrderLineId, OrderRevision, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::orders::insert_order_activity_tx;
use crate::repo::tasks::insert_progress_tx;

use super::{enqueue_event_tx, require_scope, require_stored_work_visible_before_replay_tx};

pub async fn cancel_work(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelCrossDockWorkCommand,
) -> AppResult<CancelCrossDockWorkResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_CROSS_DOCK_WORK_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        "wms_supervisor",
    )
    .await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CancelCrossDockWorkResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let hint=sqlx::query("SELECT order_id,inventory_owner_id,facility_id FROM cross_dock_tasks WHERE tenant_id=$1 AND task_id=$2")
      .bind(access.tenant_id.get()).bind(command.work_id.get()).fetch_optional(&mut *tx).await?
      .ok_or_else(||AppError::not_found("cross-dock work"))?;
    let owner_id: i64 = hint.try_get("inventory_owner_id")?;
    let facility_id: i64 = hint.try_get("facility_id")?;
    require_scope(&scope, owner_id, facility_id)?;
    let order_id: i64 = hint.try_get("order_id")?;
    let order=sqlx::query("SELECT status,revision FROM orders WHERE tenant_id=$1 AND inventory_owner_id=$2 AND id=$3 FOR UPDATE")
      .bind(access.tenant_id.get()).bind(owner_id).bind(order_id).fetch_optional(&mut *tx).await?
      .ok_or_else(||AppError::not_found("cross-dock order"))?;
    let revision: i64 = order.try_get("revision")?;
    if order.try_get::<String, _>("status")? != "open"
        || revision != command.expected_order_revision.get()
    {
        return Err(AppError::conflict(
            "cross-dock cancellation order state or revision changed",
        ));
    }
    let row=sqlx::query(
      r#"SELECT detail.plan_run_id,detail.order_item_id,detail.reservation_id,detail.planned_quantity,
                work.status,work.assigned_user_id,work.started_at,detail.closed_at
         FROM work_tasks work JOIN cross_dock_tasks detail ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id
         WHERE work.tenant_id=$1 AND work.id=$2 AND work.task_type='cross_dock' AND work.deleted IS NULL
         FOR UPDATE OF work"#,
    ).bind(access.tenant_id.get()).bind(command.work_id.get()).fetch_optional(&mut *tx).await?
      .ok_or_else(||AppError::not_found("cross-dock work"))?;
    let previous_status: String = row.try_get("status")?;
    if !matches!(previous_status.as_str(), "open" | "assigned")
        || row.try_get::<Option<Timestamp>, _>("started_at")?.is_some()
        || row.try_get::<Option<Timestamp>, _>("closed_at")?.is_some()
    {
        return Err(AppError::conflict(
            "only pending cross-dock work can be cancelled",
        ));
    }
    let cancelled_at = now_iso();
    let resulting_revision = revision + 1;
    sqlx::query("UPDATE orders SET revision=$1 WHERE tenant_id=$2 AND inventory_owner_id=$3 AND id=$4 AND revision=$5")
      .bind(resulting_revision).bind(access.tenant_id.get()).bind(owner_id).bind(order_id).bind(revision).execute(&mut *tx).await?;
    sqlx::query("UPDATE work_tasks SET status='cancelled',completed_by=$1,completed_at=$2,lease_expires_at=NULL,modified=$2 WHERE tenant_id=$3 AND id=$4")
      .bind(context.actor_id.get()).bind(cancelled_at).bind(access.tenant_id.get()).bind(command.work_id.get()).execute(&mut *tx).await?;
    let cancellation_id=CrossDockCancellationId::new(sqlx::query_scalar(
      r#"INSERT INTO cross_dock_cancellations
         (tenant_id,task_id,plan_run_id,inventory_owner_id,facility_id,order_id,order_item_id,
          reservation_id,planned_quantity,expected_order_revision,resulting_order_revision,
          previous_work_status,previous_assigned_user_id,reason_code,note,cancelled_by_user_id,cancelled_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) RETURNING id"#,
    ).bind(access.tenant_id.get()).bind(command.work_id.get()).bind(row.try_get::<i64,_>("plan_run_id")?)
      .bind(owner_id).bind(facility_id).bind(order_id).bind(row.try_get::<i64,_>("order_item_id")?)
      .bind(row.try_get::<i64,_>("reservation_id")?).bind(row.try_get::<i64,_>("planned_quantity")?)
      .bind(revision).bind(resulting_revision).bind(&previous_status)
      .bind(row.try_get::<Option<i64>,_>("assigned_user_id")?).bind(command.details.reason.as_str())
      .bind(command.details.note.as_ref().map(|note|note.as_str())).bind(context.actor_id.get()).bind(cancelled_at)
      .fetch_one(&mut *tx).await?).map_err(|e|AppError::internal(e.to_string()))?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        command.work_id.get(),
        None,
        Some(context.actor_id.get()),
        "cross_dock_cancelled",
        None,
        None,
        None,
        command.details.note.as_ref().map(|note| note.as_str()),
        Some(&serde_json::json!({"reason":command.details.reason.as_str()}).to_string()),
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        InventoryOwnerId::new(owner_id).map_err(|e| AppError::internal(e.to_string()))?,
        order_id,
        Some(context.actor_id.get()),
        "cross_dock_cancelled",
    )
    .await?;
    let result = CancelCrossDockWorkResult {
        cancellation_id,
        work_id: command.work_id,
        plan_id: CrossDockPlanId::new(row.try_get("plan_run_id")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        order_id: OrderId::new(order_id).map_err(|e| AppError::internal(e.to_string()))?,
        order_line_id: OrderLineId::new(row.try_get("order_item_id")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        previous_order_revision: OrderRevision::new(revision)
            .map_err(|e| AppError::internal(e.to_string()))?,
        order_revision: OrderRevision::new(resulting_revision)
            .map_err(|e| AppError::internal(e.to_string()))?,
        quantity: CrossDockQuantity::new(row.try_get("planned_quantity")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        previous_status: CrossDockWorkStatus::Pending,
        status: CrossDockWorkStatus::Cancelled,
        details: command.details.clone(),
        cancelled_by: UserId::new(context.actor_id.get())
            .map_err(|e| AppError::internal(e.to_string()))?,
        cancelled_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        InventoryOwnerId::new(owner_id).map_err(|e| AppError::internal(e.to_string()))?,
        wareboxes_domain::FacilityId::new(facility_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        context.actor_id.get(),
        order_id,
        "inbound.cross_dock.cancelled",
        &format!("cross-dock-cancellation:{}", cancellation_id.get()),
        &serde_json::to_value(&result).map_err(|e| AppError::internal(e.to_string()))?,
        cancelled_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
