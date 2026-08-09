use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::replenishment::{
    ClaimNextReplenishmentWorkCommand, ClaimReplenishmentWorkByIdCommand,
    HeartbeatReplenishmentClaimCommand, ReleaseReplenishmentClaimCommand, ReplenishmentClaim,
    ReplenishmentClaimHeartbeatResult, ReplenishmentClaimReleaseResult,
    ReplenishmentLocationReadModel, CLAIM_NEXT_REPLENISHMENT_WORK_OPERATION,
    CLAIM_REPLENISHMENT_WORK_BY_ID_OPERATION, HEARTBEAT_REPLENISHMENT_CLAIM_OPERATION,
    RELEASE_REPLENISHMENT_CLAIM_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId, ItemBatchId, LocationId,
    ReplenishmentClaimReleaseReason, ReplenishmentMoveQuantity, ReplenishmentPlanId,
    ReplenishmentPolicyId, ReplenishmentPolicyRevision, ReplenishmentScanValue, ReplenishmentUom,
    ReplenishmentWorkId, ReplenishmentWorkStatus, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::tasks::{
    insert_progress_tx, release_expired_tasks_tx, release_inaccessible_active_tasks_tx,
};

use super::{require_scope, require_stored_work_visible_before_replay_tx};

pub async fn claim_next(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    _command: ClaimNextReplenishmentWorkCommand,
) -> AppResult<Option<ReplenishmentClaim>> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLAIM_NEXT_REPLENISHMENT_WORK_OPERATION, &())?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(claim) = prepared
        .replayed::<Option<ReplenishmentClaim>>(&mut tx)
        .await?
    {
        if let Some(claim) = claim.as_ref() {
            require_visible_tx(&mut tx, access.tenant_id, claim.work_id, &scope).await?;
        }
        tx.commit().await?;
        return Ok(claim);
    }
    release_stale_work_tx(&mut tx, access, context.actor_id.get(), &scope).await?;
    ensure_no_other_active_work_tx(&mut tx, access, None).await?;
    let claimed_at = now_iso();
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"
        WITH candidate AS (
          SELECT work.id
          FROM work_tasks work
          JOIN replenishment_tasks detail ON detail.tenant_id=work.tenant_id
            AND detail.task_id=work.id AND detail.closed_at IS NULL
          WHERE work.tenant_id=$1 AND work.task_type='replenishment'
            AND work.required_permission='wms' AND work.deleted IS NULL
            AND (work.scheduled_for IS NULL OR work.scheduled_for <= $2)
            AND ((work.status='assigned' AND work.assigned_user_id=$3)
              OR (work.status='open' AND work.assigned_user_id IS NULL))
            AND ($4 OR work.facility_id=ANY($5))
            AND ($6 OR work.inventory_owner_id=ANY($7))
          ORDER BY CASE WHEN work.status='assigned' THEN 0 ELSE 1 END,
            work.priority DESC, work.due_at ASC NULLS LAST,
            COALESCE(work.scheduled_for,work.created), work.created, work.id
          FOR UPDATE OF work SKIP LOCKED LIMIT 1
        )
        UPDATE work_tasks work SET status='in_progress', assigned_user_id=$3,
          started_at=COALESCE(work.started_at,$2),
          lease_expires_at=$2+make_interval(secs=>work.task_timeout_seconds::int), modified=$2
        FROM candidate WHERE work.tenant_id=$1 AND work.id=candidate.id RETURNING work.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(claimed_at)
    .bind(context.actor_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let claim = match task_id {
        Some(task_id) => {
            insert_progress_tx(
                &mut tx,
                access.tenant_id,
                task_id,
                None,
                Some(context.actor_id.get()),
                "started",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            Some(load_claim_tx(&mut tx, access, task_id, context.actor_id.get()).await?)
        }
        None => None,
    };
    Ok(prepared.commit(tx, claim).await?)
}

pub async fn claim_by_id(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ClaimReplenishmentWorkByIdCommand,
) -> AppResult<ReplenishmentClaim> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        CLAIM_REPLENISHMENT_WORK_BY_ID_OPERATION,
        &command.work_id,
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(claim) = prepared.replayed::<ReplenishmentClaim>(&mut tx).await? {
        require_visible_tx(&mut tx, access.tenant_id, command.work_id, &scope).await?;
        tx.commit().await?;
        return Ok(claim);
    }
    release_stale_work_tx(&mut tx, access, context.actor_id.get(), &scope).await?;
    let row = lock_claim_target_tx(&mut tx, access.tenant_id, command.work_id, &scope).await?;
    let status: String = row.try_get("status")?;
    let assigned: Option<i64> = row.try_get("assigned_user_id")?;
    if status == "in_progress"
        && assigned == Some(context.actor_id.get())
        && row.try_get::<Option<bool>, _>("lease_current")? == Some(true)
    {
        let claim = load_claim_tx(
            &mut tx,
            access,
            command.work_id.get(),
            context.actor_id.get(),
        )
        .await?;
        return Ok(prepared.commit(tx, claim).await?);
    }
    if !matches!(status.as_str(), "open" | "assigned")
        || assigned.is_some_and(|id| id != context.actor_id.get())
    {
        return Err(AppError::conflict("replenishment work cannot be claimed"));
    }
    if row
        .try_get::<Option<Timestamp>, _>("scheduled_for")?
        .is_some_and(|scheduled| scheduled > now_iso())
    {
        return Err(AppError::conflict(
            "replenishment work is not scheduled yet",
        ));
    }
    ensure_no_other_active_work_tx(&mut tx, access, Some(command.work_id)).await?;
    let claimed_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE work_tasks SET status='in_progress',assigned_user_id=$1,
          started_at=COALESCE(started_at,$2),
          lease_expires_at=$2+make_interval(secs=>task_timeout_seconds::int),modified=$2
        WHERE tenant_id=$3 AND id=$4 AND task_type='replenishment' AND deleted IS NULL
          AND status IN ('open','assigned') AND (assigned_user_id IS NULL OR assigned_user_id=$1)
        "#,
    )
    .bind(context.actor_id.get())
    .bind(claimed_at)
    .bind(access.tenant_id.get())
    .bind(command.work_id.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("replenishment work cannot be claimed"));
    }
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        command.work_id.get(),
        None,
        Some(context.actor_id.get()),
        "started",
        None,
        None,
        None,
        None,
        None,
    )
    .await?;
    let claim = load_claim_tx(
        &mut tx,
        access,
        command.work_id.get(),
        context.actor_id.get(),
    )
    .await?;
    Ok(prepared.commit(tx, claim).await?)
}

pub async fn current_claim(
    db: &Db,
    access: &TenantAccess,
) -> AppResult<Option<ReplenishmentClaim>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    release_stale_work_tx(&mut tx, access, access.user_id.get(), &scope).await?;
    let row = sqlx::query(
        r#"SELECT id,task_type,status,facility_id,inventory_owner_id,
             lease_expires_at>statement_timestamp() AS lease_current
           FROM work_tasks WHERE tenant_id=$1 AND assigned_user_id=$2 AND deleted IS NULL
             AND status IN ('assigned','in_progress') LIMIT 1"#,
    )
    .bind(access.tenant_id.get())
    .bind(access.user_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    require_scope(
        &scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )?;
    if row.try_get::<String, _>("task_type")? != "replenishment" {
        return Err(AppError::conflict("active task is not replenishment work"));
    }
    if row.try_get::<String, _>("status")? != "in_progress"
        || row.try_get::<Option<bool>, _>("lease_current")? != Some(true)
    {
        tx.commit().await?;
        return Ok(None);
    }
    let claim = load_claim_tx(&mut tx, access, row.try_get("id")?, access.user_id.get()).await?;
    tx.commit().await?;
    Ok(Some(claim))
}

pub async fn heartbeat_claim(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: HeartbeatReplenishmentClaimCommand,
) -> AppResult<ReplenishmentClaimHeartbeatResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        HEARTBEAT_REPLENISHMENT_CLAIM_OPERATION,
        &command.work_id,
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ReplenishmentClaimHeartbeatResult>(&mut tx)
        .await?
    {
        require_visible_tx(&mut tx, access.tenant_id, command.work_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let row = lock_active_claim_tx(&mut tx, access, command.work_id, &scope).await?;
    let heartbeat_at = now_iso();
    let lease_expires_at: Timestamp = sqlx::query_scalar(
        r#"UPDATE work_tasks SET lease_expires_at=$1+make_interval(secs=>task_timeout_seconds::int),modified=$1
           WHERE tenant_id=$2 AND id=$3 AND task_type='replenishment' AND status='in_progress'
             AND assigned_user_id=$4 AND lease_expires_at>$1 RETURNING lease_expires_at"#,
    )
    .bind(heartbeat_at)
    .bind(access.tenant_id.get())
    .bind(command.work_id.get())
    .bind(context.actor_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("replenishment claim is no longer active"))?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        command.work_id.get(),
        None,
        Some(context.actor_id.get()),
        "replenishment_heartbeat",
        None,
        None,
        None,
        None,
        Some(
            &serde_json::json!({"previous_lease_expires_at": row.try_get::<Timestamp,_>("lease_expires_at")?, "lease_expires_at": lease_expires_at}).to_string(),
        ),
    )
    .await?;
    let result = ReplenishmentClaimHeartbeatResult {
        work_id: command.work_id,
        heartbeat_at,
        lease_expires_at,
    };
    Ok(prepared.commit(tx, result).await?)
}

pub async fn release_claim(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ReleaseReplenishmentClaimCommand,
) -> AppResult<ReplenishmentClaimReleaseResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_release(&command)?;
    let prepared = PreparedCommand::new_v1(
        context,
        RELEASE_REPLENISHMENT_CLAIM_OPERATION,
        &(command.work_id, command.reason, &command.note),
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ReplenishmentClaimReleaseResult>(&mut tx)
        .await?
    {
        require_visible_tx(&mut tx, access.tenant_id, command.work_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_active_claim_tx(&mut tx, access, command.work_id, &scope).await?;
    let released_at = now_iso();
    let release_count: i64 = sqlx::query_scalar(
        r#"UPDATE work_tasks SET status='open',assigned_user_id=NULL,started_at=NULL,
             lease_expires_at=NULL,last_released_at=$1,release_count=release_count+1,modified=$1
           WHERE tenant_id=$2 AND id=$3 AND task_type='replenishment' AND status='in_progress'
             AND assigned_user_id=$4 AND lease_expires_at>$1 AND completed_at IS NULL
           RETURNING release_count"#,
    )
    .bind(released_at)
    .bind(access.tenant_id.get())
    .bind(command.work_id.get())
    .bind(context.actor_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("replenishment claim is no longer active"))?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        command.work_id.get(),
        None,
        Some(context.actor_id.get()),
        "replenishment_released",
        None,
        None,
        None,
        command.note.as_deref(),
        Some(&serde_json::json!({"release_count": release_count, "reason": reason_text(command.reason)}).to_string()),
    )
    .await?;
    let result = ReplenishmentClaimReleaseResult {
        work_id: command.work_id,
        status: ReplenishmentWorkStatus::Pending,
        released_at,
        release_count,
        reason: command.reason,
        note: command.note,
    };
    Ok(prepared.commit(tx, result).await?)
}

async fn load_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_id: i64,
) -> AppResult<ReplenishmentClaim> {
    let row = sqlx::query(
        r#"
        SELECT work.priority,work.instructions,work.due_at,work.lease_expires_at,
          detail.plan_run_id,detail.policy_id,detail.policy_revision,detail.inventory_owner_id,
          detail.facility_id,detail.travel_sequence,detail.source_inventory_balance_id,
          detail.item_batch_id,detail.item_id,detail.uom,detail.planned_qty,
          detail.source_location_id,source.barcode AS source_barcode,source.name AS source_name,
          detail.destination_location_id,destination.barcode AS destination_barcode,
          destination.name AS destination_name,detail.source_lot AS lot,
          detail.source_serial AS serial,detail.source_expiration AS expiration,
          item.description AS item_description,
          ARRAY(SELECT barcode.name FROM barcodes barcode WHERE barcode.tenant_id=detail.tenant_id
            AND barcode.item_id=detail.item_id AND barcode.deleted IS NULL ORDER BY barcode.id) AS item_barcodes,
          balance.qty_on_hand-balance.qty_reserved-balance.qty_held AS source_free,
          balance.location_id AS current_source_location,balance.item_batch_id AS current_batch,
          balance.item_id AS current_item,balance.uom AS current_uom,balance.status AS current_status,
          balance.license_plate_id,balance.deleted AS balance_deleted,
          source.active AS source_active,source.pickable AS source_pickable,source.receivable AS source_receivable,
          destination.active AS destination_active,destination.pickable AS destination_pickable,
          destination.receivable AS destination_receivable
        FROM work_tasks work JOIN replenishment_tasks detail ON detail.tenant_id=work.tenant_id
          AND detail.task_id=work.id AND detail.closed_at IS NULL
        JOIN inventory_balances balance ON balance.tenant_id=detail.tenant_id
          AND balance.inventory_owner_id=detail.inventory_owner_id AND balance.facility_id=detail.facility_id
          AND balance.id=detail.source_inventory_balance_id
        JOIN item_batches batch ON batch.tenant_id=detail.tenant_id
          AND batch.inventory_owner_id=detail.inventory_owner_id AND batch.id=detail.item_batch_id
          AND batch.deleted IS NULL
        JOIN items item ON item.tenant_id=detail.tenant_id AND item.id=detail.item_id AND item.deleted IS NULL
        JOIN locations source ON source.tenant_id=detail.tenant_id AND source.facility_id=detail.facility_id
          AND source.id=detail.source_location_id AND source.deleted IS NULL
        JOIN locations destination ON destination.tenant_id=detail.tenant_id AND destination.facility_id=detail.facility_id
          AND destination.id=detail.destination_location_id AND destination.deleted IS NULL
        WHERE work.tenant_id=$1 AND work.id=$2 AND work.task_type='replenishment'
          AND work.status='in_progress' AND work.assigned_user_id=$3
          AND work.lease_expires_at>statement_timestamp() AND work.deleted IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(actor_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("replenishment claim is no longer executable"))?;
    let item_barcodes: Vec<String> = row.try_get("item_barcodes")?;
    let source_barcode: String = row.try_get("source_barcode")?;
    let destination_barcode: String = row.try_get("destination_barcode")?;
    let planned: i64 = row.try_get("planned_qty")?;
    if item_barcodes.is_empty()
        || row.try_get::<i64, _>("source_free")? < planned
        || row.try_get::<i64, _>("current_source_location")?
            != row.try_get::<i64, _>("source_location_id")?
        || row.try_get::<i64, _>("current_batch")? != row.try_get::<i64, _>("item_batch_id")?
        || row.try_get::<i64, _>("current_item")? != row.try_get::<i64, _>("item_id")?
        || row.try_get::<String, _>("current_uom")? != row.try_get::<String, _>("uom")?
        || row.try_get::<String, _>("current_status")? != "available"
        || row.try_get::<Option<i64>, _>("license_plate_id")?.is_some()
        || row
            .try_get::<Option<Timestamp>, _>("balance_deleted")?
            .is_some()
        || !row.try_get::<bool, _>("source_active")?
        || row.try_get::<bool, _>("source_pickable")?
        || row.try_get::<bool, _>("source_receivable")?
        || !row.try_get::<bool, _>("destination_active")?
        || !row.try_get::<bool, _>("destination_pickable")?
        || row.try_get::<bool, _>("destination_receivable")?
    {
        return Err(AppError::conflict(
            "replenishment work inventory or location snapshot is stale",
        ));
    }
    Ok(ReplenishmentClaim {
        work_id: ReplenishmentWorkId::new(task_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        plan_id: ReplenishmentPlanId::new(row.try_get("plan_run_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        policy_id: ReplenishmentPolicyId::new(row.try_get("policy_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        policy_revision: ReplenishmentPolicyRevision::new(row.try_get("policy_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        sequence: u32::try_from(row.try_get::<i64, _>("travel_sequence")?)
            .map_err(|_| AppError::internal("replenishment sequence overflow"))?,
        priority: row.try_get("priority")?,
        instructions: row.try_get("instructions")?,
        due_at: row.try_get("due_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        source_inventory_balance_id: InventoryBalanceId::new(
            row.try_get("source_inventory_balance_id")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_id: CatalogItemId::new(row.try_get("item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_description: row.try_get("item_description")?,
        item_barcodes: item_barcodes
            .into_iter()
            .map(|value| {
                ReplenishmentScanValue::new(value)
                    .map_err(|error| AppError::internal(error.to_string()))
            })
            .collect::<AppResult<Vec<_>>>()?,
        uom: ReplenishmentUom::new(row.try_get::<String, _>("uom")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        quantity: ReplenishmentMoveQuantity::new(planned)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location: ReplenishmentLocationReadModel {
            location_id: LocationId::new(row.try_get("source_location_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            barcode: ReplenishmentScanValue::new(source_barcode)
                .map_err(|error| AppError::internal(error.to_string()))?,
            name: row.try_get("source_name")?,
        },
        destination_pick_face: ReplenishmentLocationReadModel {
            location_id: LocationId::new(row.try_get("destination_location_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            barcode: ReplenishmentScanValue::new(destination_barcode)
                .map_err(|error| AppError::internal(error.to_string()))?,
            name: row.try_get("destination_name")?,
        },
    })
}

async fn release_stale_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    release_expired_tasks_tx(tx, access.tenant_id, Some(user_id), scope).await?;
    release_inaccessible_active_tasks_tx(tx, access.tenant_id, user_id, scope).await
}

async fn ensure_no_other_active_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    allowed: Option<ReplenishmentWorkId>,
) -> AppResult<()> {
    let active: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM work_tasks WHERE tenant_id=$1 AND assigned_user_id=$2 AND deleted IS NULL AND status IN ('assigned','in_progress') AND ($3::bigint IS NULL OR id<>$3) LIMIT 1 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(access.user_id.get())
    .bind(allowed.map(|id| id.get()))
    .fetch_optional(&mut **tx)
    .await?;
    if active.is_some() {
        Err(AppError::conflict(
            "user already has active work; resume or release it first",
        ))
    } else {
        Ok(())
    }
}

async fn lock_claim_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ReplenishmentWorkId,
    scope: &ScopeBindings,
) -> AppResult<sqlx::postgres::PgRow> {
    let row = sqlx::query(
        r#"SELECT work.status,work.assigned_user_id,work.scheduled_for,work.lease_expires_at,
             work.lease_expires_at>statement_timestamp() AS lease_current,
             work.facility_id,work.inventory_owner_id
           FROM work_tasks work JOIN replenishment_tasks detail ON detail.tenant_id=work.tenant_id
             AND detail.task_id=work.id
           WHERE work.tenant_id=$1 AND work.id=$2 AND work.task_type='replenishment'
             AND work.deleted IS NULL FOR UPDATE OF work"#,
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment work"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )?;
    Ok(row)
}

async fn lock_active_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    work_id: ReplenishmentWorkId,
    scope: &ScopeBindings,
) -> AppResult<sqlx::postgres::PgRow> {
    let row = lock_claim_target_tx(tx, access.tenant_id, work_id, scope).await?;
    if row.try_get::<String, _>("status")? != "in_progress"
        || row.try_get::<Option<i64>, _>("assigned_user_id")? != Some(access.user_id.get())
        || row.try_get::<Option<bool>, _>("lease_current")? != Some(true)
    {
        return Err(AppError::conflict(
            "replenishment claim is no longer active",
        ));
    }
    Ok(row)
}

async fn require_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ReplenishmentWorkId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM work_tasks WHERE tenant_id=$1 AND id=$2 AND task_type='replenishment' AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment work"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}

fn validate_release(command: &ReleaseReplenishmentClaimCommand) -> AppResult<()> {
    if let Some(note) = command.note.as_deref() {
        if note.is_empty() || note.trim() != note || note.chars().count() > 500 {
            return Err(AppError::bad_request(
                "replenishment release note is invalid",
            ));
        }
    }
    if command.reason == ReplenishmentClaimReleaseReason::Other && command.note.is_none() {
        return Err(AppError::bad_request(
            "release note is required when reason is other",
        ));
    }
    Ok(())
}

fn reason_text(reason: ReplenishmentClaimReleaseReason) -> &'static str {
    match reason {
        ReplenishmentClaimReleaseReason::WorkInterrupted => "work_interrupted",
        ReplenishmentClaimReleaseReason::EquipmentUnavailable => "equipment_unavailable",
        ReplenishmentClaimReleaseReason::SourceBlocked => "source_blocked",
        ReplenishmentClaimReleaseReason::DestinationBlocked => "destination_blocked",
        ReplenishmentClaimReleaseReason::InventoryMismatch => "inventory_mismatch",
        ReplenishmentClaimReleaseReason::SafetyIssue => "safety_issue",
        ReplenishmentClaimReleaseReason::Other => "other",
    }
}
