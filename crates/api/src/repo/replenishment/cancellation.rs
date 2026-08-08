use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::replenishment::{
    CancelReplenishmentWorkCommand, CancelReplenishmentWorkResult,
    CANCEL_REPLENISHMENT_WORK_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId, ItemBatchId,
    ReplenishmentCancellationId, ReplenishmentMoveQuantity, ReplenishmentPlanId,
    ReplenishmentPolicyId, ReplenishmentPolicyRevision, ReplenishmentPolicyScope, ReplenishmentUom,
    ReplenishmentWorkId, ReplenishmentWorkStatus, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::tasks::insert_progress_tx;

use super::policy::lock_natural_key_tx;
use super::{enqueue_event_tx, require_scope, require_stored_work_visible_before_replay_tx};

#[derive(Debug)]
struct CancellationTarget {
    work_id: ReplenishmentWorkId,
    plan_id: ReplenishmentPlanId,
    policy_id: ReplenishmentPolicyId,
    policy_revision: ReplenishmentPolicyRevision,
    scope: ReplenishmentPolicyScope,
    source_balance_id: InventoryBalanceId,
    item_batch_id: ItemBatchId,
    quantity: ReplenishmentMoveQuantity,
    previous_status: ReplenishmentWorkStatus,
    previous_status_text: String,
    previous_assigned_user_id: Option<UserId>,
}

pub async fn cancel_work(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: CancelReplenishmentWorkCommand,
) -> AppResult<CancelReplenishmentWorkResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        CANCEL_REPLENISHMENT_WORK_OPERATION,
        &(command.work_id(), command.reason(), command.note()),
    )?;
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
        .replayed::<CancelReplenishmentWorkResult>(&mut tx)
        .await?
    {
        require_replayed_cancellation_visible_tx(
            &mut tx,
            access.tenant_id,
            result.cancellation_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let policy_scope = load_scope_hint_tx(&mut tx, access.tenant_id, command.work_id()).await?;
    require_scope(
        &scope,
        policy_scope.inventory_owner_id.get(),
        policy_scope.facility_id.get(),
    )?;
    lock_natural_key_tx(&mut tx, &policy_scope).await?;
    lock_policy_tx(&mut tx, access.tenant_id, &policy_scope).await?;
    let target = lock_target_tx(&mut tx, access.tenant_id, command.work_id(), &scope).await?;
    let cancelled_at = now_iso();
    let cancelled_by = UserId::new(context.actor_id.get())
        .map_err(|error| AppError::internal(error.to_string()))?;
    let cancellation_id = insert_evidence_tx(
        &mut tx,
        access.tenant_id,
        &target,
        &command,
        cancelled_by,
        cancelled_at,
    )
    .await?;
    cancel_task_tx(
        &mut tx,
        access.tenant_id,
        &target,
        cancelled_by,
        cancelled_at,
    )
    .await?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        target.work_id.get(),
        None,
        Some(cancelled_by.get()),
        "replenishment_cancelled",
        Some(target.quantity.get()),
        None,
        Some(target.scope.pick_face_location_id.get()),
        command.note().map(|note| note.as_str()),
        Some(
            &serde_json::json!({
                "cancellation_id": cancellation_id.get(),
                "reason": reason_text(command.reason()),
                "previous_work_status": target.previous_status_text,
                "previous_assigned_user_id": target.previous_assigned_user_id.map(UserId::get),
            })
            .to_string(),
        ),
    )
    .await?;
    let result = CancelReplenishmentWorkResult {
        cancellation_id,
        work_id: target.work_id,
        plan_id: target.plan_id,
        policy_id: target.policy_id,
        policy_revision: target.policy_revision,
        scope: target.scope,
        source_inventory_balance_id: target.source_balance_id,
        item_batch_id: target.item_batch_id,
        quantity: target.quantity,
        previous_status: target.previous_status,
        previous_assigned_user_id: target.previous_assigned_user_id,
        status: ReplenishmentWorkStatus::Cancelled,
        reason: command.reason(),
        note: command.note().cloned(),
        cancelled_by,
        cancelled_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        result.scope.inventory_owner_id,
        result.scope.facility_id,
        cancelled_by.get(),
        "replenishment_task",
        result.work_id.get(),
        "inventory.replenishment.cancelled",
        &format!("cancelled:{}", cancellation_id.get()),
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
        cancelled_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn load_scope_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ReplenishmentWorkId,
) -> AppResult<ReplenishmentPolicyScope> {
    let row = sqlx::query(
        r#"
        SELECT detail.inventory_owner_id,detail.facility_id,detail.item_id,detail.uom,
               policy.pick_face_location_id
        FROM replenishment_tasks detail
        JOIN replenishment_policies policy ON policy.tenant_id=detail.tenant_id
          AND policy.inventory_owner_id=detail.inventory_owner_id
          AND policy.facility_id=detail.facility_id AND policy.id=detail.policy_id
        WHERE detail.tenant_id=$1 AND detail.task_id=$2
        "#,
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment work"))?;
    scope_from_row(tenant_id, &row)
}

async fn lock_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: &ReplenishmentPolicyScope,
) -> AppResult<()> {
    sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id FROM replenishment_policies
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND pick_face_location_id=$4 AND item_id=$5 AND uom=$6
        ORDER BY revision DESC LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.pick_face_location_id.get())
    .bind(scope.item_id.get())
    .bind(scope.uom.as_str())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment work"))?;
    Ok(())
}

async fn lock_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ReplenishmentWorkId,
    scope: &ScopeBindings,
) -> AppResult<CancellationTarget> {
    let row = sqlx::query(
        r#"
        SELECT work.status,work.assigned_user_id,work.completed_at,detail.closed_at,
          detail.plan_run_id,detail.policy_id,detail.policy_revision,
          detail.inventory_owner_id,detail.facility_id,detail.source_inventory_balance_id,
          detail.item_batch_id,detail.item_id,detail.uom,detail.planned_qty,
          policy.pick_face_location_id
        FROM work_tasks work
        JOIN replenishment_tasks detail ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id
        JOIN replenishment_policies policy ON policy.tenant_id=detail.tenant_id
          AND policy.inventory_owner_id=detail.inventory_owner_id
          AND policy.facility_id=detail.facility_id AND policy.id=detail.policy_id
        WHERE work.tenant_id=$1 AND work.id=$2 AND work.task_type='replenishment'
          AND work.deleted IS NULL
        FOR UPDATE OF work
        "#,
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment work"))?;
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    require_scope(scope, owner_id, facility_id)?;
    let status: String = row.try_get("status")?;
    if !matches!(status.as_str(), "open" | "assigned")
        || row
            .try_get::<Option<Timestamp>, _>("completed_at")?
            .is_some()
        || row.try_get::<Option<Timestamp>, _>("closed_at")?.is_some()
    {
        let message = if status == "in_progress" {
            "active replenishment claim must be released before cancellation"
        } else {
            "replenishment work cannot be cancelled from its current state"
        };
        return Err(AppError::conflict(message));
    }
    let assigned_id: Option<i64> = row.try_get("assigned_user_id")?;
    let assignment_is_consistent = (status == "open" && assigned_id.is_none())
        || (status == "assigned" && assigned_id.is_some());
    if !assignment_is_consistent {
        return Err(AppError::conflict(
            "replenishment work assignment is inconsistent",
        ));
    }
    Ok(CancellationTarget {
        work_id,
        plan_id: id(row.try_get("plan_run_id")?, ReplenishmentPlanId::new)?,
        policy_id: id(row.try_get("policy_id")?, ReplenishmentPolicyId::new)?,
        policy_revision: ReplenishmentPolicyRevision::new(row.try_get("policy_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        scope: scope_from_row(tenant_id, &row)?,
        source_balance_id: id(
            row.try_get("source_inventory_balance_id")?,
            InventoryBalanceId::new,
        )?,
        item_batch_id: id(row.try_get("item_batch_id")?, ItemBatchId::new)?,
        quantity: ReplenishmentMoveQuantity::new(row.try_get("planned_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        previous_status: ReplenishmentWorkStatus::Pending,
        previous_status_text: status,
        previous_assigned_user_id: assigned_id
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn insert_evidence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &CancellationTarget,
    command: &CancelReplenishmentWorkCommand,
    actor_id: UserId,
    cancelled_at: Timestamp,
) -> AppResult<ReplenishmentCancellationId> {
    let value: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO replenishment_cancellations (
          tenant_id,task_id,plan_run_id,policy_id,policy_revision,inventory_owner_id,
          facility_id,source_inventory_balance_id,item_batch_id,item_id,uom,planned_qty,
          previous_work_status,previous_assigned_user_id,reason_code,note,
          cancelled_by_user_id,cancelled_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.work_id.get())
    .bind(target.plan_id.get())
    .bind(target.policy_id.get())
    .bind(target.policy_revision.get())
    .bind(target.scope.inventory_owner_id.get())
    .bind(target.scope.facility_id.get())
    .bind(target.source_balance_id.get())
    .bind(target.item_batch_id.get())
    .bind(target.scope.item_id.get())
    .bind(target.scope.uom.as_str())
    .bind(target.quantity.get())
    .bind(&target.previous_status_text)
    .bind(target.previous_assigned_user_id.map(UserId::get))
    .bind(reason_text(command.reason()))
    .bind(command.note().map(|note| note.as_str()))
    .bind(actor_id.get())
    .bind(cancelled_at)
    .fetch_one(&mut **tx)
    .await?;
    id(value, ReplenishmentCancellationId::new)
}

async fn cancel_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &CancellationTarget,
    actor_id: UserId,
    cancelled_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE work_tasks SET status='cancelled',assigned_user_id=NULL,started_at=NULL,
          lease_expires_at=NULL,completed_by=$1,completed_at=$2,modified=$2
        WHERE tenant_id=$3 AND id=$4 AND task_type='replenishment' AND deleted IS NULL
          AND status=$5 AND assigned_user_id IS NOT DISTINCT FROM $6 AND completed_at IS NULL
        "#,
    )
    .bind(actor_id.get())
    .bind(cancelled_at)
    .bind(tenant_id.get())
    .bind(target.work_id.get())
    .bind(&target.previous_status_text)
    .bind(target.previous_assigned_user_id.map(UserId::get))
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "replenishment work changed during cancellation",
        ));
    }
    Ok(())
}

async fn require_replayed_cancellation_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    cancellation_id: ReplenishmentCancellationId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM replenishment_cancellations WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(cancellation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment work"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}

fn scope_from_row(
    tenant_id: TenantId,
    row: &sqlx::postgres::PgRow,
) -> AppResult<ReplenishmentPolicyScope> {
    Ok(ReplenishmentPolicyScope {
        tenant_id,
        inventory_owner_id: id(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        facility_id: id(row.try_get("facility_id")?, FacilityId::new)?,
        item_id: CatalogItemId::new(row.try_get("item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        uom: ReplenishmentUom::new(row.try_get::<String, _>("uom")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        pick_face_location_id: id(
            row.try_get("pick_face_location_id")?,
            wareboxes_domain::LocationId::new,
        )?,
    })
}

fn id<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>) -> AppResult<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(|error| AppError::internal(error.to_string()))
}

const fn reason_text(
    reason: wareboxes_domain::ReplenishmentWorkCancellationReason,
) -> &'static str {
    use wareboxes_domain::ReplenishmentWorkCancellationReason as Reason;
    match reason {
        Reason::DemandRemoved => "demand_removed",
        Reason::PolicyReconfigured => "policy_reconfigured",
        Reason::SourceUnavailable => "source_unavailable",
        Reason::DestinationUnavailable => "destination_unavailable",
        Reason::PlanningError => "planning_error",
        Reason::Other => "other",
    }
}
