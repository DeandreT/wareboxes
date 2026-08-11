use sqlx::Row;
use wareboxes_application::cross_dock::{
    ClaimCrossDockWorkByIdCommand, ClaimNextCrossDockWorkCommand, CrossDockClaim,
    CrossDockClaimHeartbeatResult, CrossDockClaimReleaseResult, CrossDockLocationReadModel,
    HeartbeatCrossDockClaimCommand, ReleaseCrossDockClaimCommand,
    CLAIM_CROSS_DOCK_WORK_BY_ID_OPERATION, CLAIM_NEXT_CROSS_DOCK_WORK_OPERATION,
    HEARTBEAT_CROSS_DOCK_CLAIM_OPERATION, RELEASE_CROSS_DOCK_CLAIM_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, CrossDockPlanId, CrossDockQuantity, CrossDockScanValue, CrossDockUom,
    CrossDockWorkId, CrossDockWorkStatus, FacilityId, InventoryBalanceId, InventoryOwnerId,
    ItemBatchId, LocationId, OrderId, OrderLineId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::tasks::{
    insert_progress_tx, release_expired_tasks_tx, release_inaccessible_active_tasks_tx,
};

use super::{require_scope, require_stored_work_visible_before_replay_tx, require_work_visible_tx};

pub async fn claim_next(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    _command: ClaimNextCrossDockWorkCommand,
) -> AppResult<Option<CrossDockClaim>> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLAIM_NEXT_CROSS_DOCK_WORK_OPERATION, &())?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<Option<CrossDockClaim>>(&mut tx).await? {
        if let Some(claim) = result.as_ref() {
            require_work_visible_tx(&mut tx, access.tenant_id, claim.work_id.get(), &scope).await?;
        }
        tx.commit().await?;
        return Ok(result);
    }
    release_stale_tx(&mut tx, access, context.actor_id.get(), &scope).await?;
    require_no_other_active_tx(&mut tx, access, None).await?;
    let now = now_iso();
    let work_id: Option<i64> = sqlx::query_scalar(
        r#"WITH candidate AS (
             SELECT work.id FROM work_tasks work
             JOIN cross_dock_tasks detail ON detail.tenant_id=work.tenant_id
               AND detail.task_id=work.id AND detail.closed_at IS NULL
             WHERE work.tenant_id=$1 AND work.task_type='cross_dock' AND work.deleted IS NULL
               AND (work.scheduled_for IS NULL OR work.scheduled_for<=$2)
               AND ((work.status='assigned' AND work.assigned_user_id=$3)
                 OR (work.status='open' AND work.assigned_user_id IS NULL))
               AND ($4 OR work.facility_id=ANY($5))
               AND ($6 OR work.inventory_owner_id=ANY($7))
             ORDER BY CASE WHEN work.status='assigned' THEN 0 ELSE 1 END,
               work.priority DESC,work.due_at ASC NULLS LAST,work.created,work.id
             FOR UPDATE OF work SKIP LOCKED LIMIT 1)
           UPDATE work_tasks work SET status='in_progress',assigned_user_id=$3,
             started_at=COALESCE(started_at,$2),
             lease_expires_at=$2+make_interval(secs=>work.task_timeout_seconds::int),modified=$2
           FROM candidate WHERE work.tenant_id=$1 AND work.id=candidate.id RETURNING work.id"#,
    )
    .bind(access.tenant_id.get())
    .bind(now)
    .bind(context.actor_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let result = if let Some(work_id) = work_id {
        insert_progress_tx(
            &mut tx,
            access.tenant_id,
            work_id,
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
        Some(load_claim_tx(&mut tx, access, work_id, context.actor_id.get()).await?)
    } else {
        None
    };
    Ok(prepared.commit(tx, result).await?)
}

pub async fn claim_by_id(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ClaimCrossDockWorkByIdCommand,
) -> AppResult<CrossDockClaim> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        CLAIM_CROSS_DOCK_WORK_BY_ID_OPERATION,
        &command.work_id,
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<CrossDockClaim>(&mut tx).await? {
        require_work_visible_tx(&mut tx, access.tenant_id, command.work_id.get(), &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    release_stale_tx(&mut tx, access, context.actor_id.get(), &scope).await?;
    let row = sqlx::query(
        r#"SELECT work.status,work.assigned_user_id,work.scheduled_for,
                  work.lease_expires_at>statement_timestamp() AS lease_current,
                  work.inventory_owner_id,work.facility_id
           FROM work_tasks work JOIN cross_dock_tasks detail
             ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id AND detail.closed_at IS NULL
           WHERE work.tenant_id=$1 AND work.id=$2 AND work.task_type='cross_dock'
             AND work.deleted IS NULL FOR UPDATE OF work"#,
    ).bind(access.tenant_id.get()).bind(command.work_id.get()).fetch_optional(&mut *tx).await?
      .ok_or_else(|| AppError::not_found("cross-dock work"))?;
    require_scope(
        &scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )?;
    let status: String = row.try_get("status")?;
    let assigned: Option<i64> = row.try_get("assigned_user_id")?;
    if status == "in_progress"
        && assigned == Some(context.actor_id.get())
        && row.try_get::<Option<bool>, _>("lease_current")? == Some(true)
    {
        let result = load_claim_tx(
            &mut tx,
            access,
            command.work_id.get(),
            context.actor_id.get(),
        )
        .await?;
        return Ok(prepared.commit(tx, result).await?);
    }
    if !matches!(status.as_str(), "open" | "assigned")
        || assigned.is_some_and(|id| id != context.actor_id.get())
        || row
            .try_get::<Option<Timestamp>, _>("scheduled_for")?
            .is_some_and(|at| at > now_iso())
    {
        return Err(AppError::conflict("cross-dock work cannot be claimed"));
    }
    require_no_other_active_tx(&mut tx, access, Some(command.work_id)).await?;
    let now = now_iso();
    let changed=sqlx::query(
        r#"UPDATE work_tasks SET status='in_progress',assigned_user_id=$1,
             started_at=COALESCE(started_at,$2),lease_expires_at=$2+make_interval(secs=>task_timeout_seconds::int),modified=$2
           WHERE tenant_id=$3 AND id=$4 AND task_type='cross_dock' AND deleted IS NULL
             AND status IN ('open','assigned') AND (assigned_user_id IS NULL OR assigned_user_id=$1)"#,
    ).bind(context.actor_id.get()).bind(now).bind(access.tenant_id.get()).bind(command.work_id.get())
      .execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::conflict("cross-dock work cannot be claimed"));
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
    let result = load_claim_tx(
        &mut tx,
        access,
        command.work_id.get(),
        context.actor_id.get(),
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn current_claim(db: &Db, access: &TenantAccess) -> AppResult<Option<CrossDockClaim>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    release_stale_tx(&mut tx, access, access.user_id.get(), &scope).await?;
    let row = sqlx::query(
        r#"SELECT id,task_type,status,facility_id,inventory_owner_id,
                  lease_expires_at>statement_timestamp() AS lease_current
           FROM work_tasks WHERE tenant_id=$1 AND assigned_user_id=$2 AND deleted IS NULL
             AND status IN ('assigned','in_progress') ORDER BY id LIMIT 1"#,
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
    if row.try_get::<String, _>("task_type")? != "cross_dock" {
        return Err(AppError::conflict("active task is not cross-dock work"));
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
    command: HeartbeatCrossDockClaimCommand,
) -> AppResult<CrossDockClaimHeartbeatResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        HEARTBEAT_CROSS_DOCK_CLAIM_OPERATION,
        &command.work_id,
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CrossDockClaimHeartbeatResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    require_work_visible_tx(&mut tx, access.tenant_id, command.work_id.get(), &scope).await?;
    let now = now_iso();
    let lease:Option<Timestamp>=sqlx::query_scalar(
        r#"UPDATE work_tasks SET lease_expires_at=$1+make_interval(secs=>task_timeout_seconds::int),modified=$1
           WHERE tenant_id=$2 AND id=$3 AND task_type='cross_dock' AND status='in_progress'
             AND assigned_user_id=$4 AND lease_expires_at>statement_timestamp()
           RETURNING lease_expires_at"#,
    ).bind(now).bind(access.tenant_id.get()).bind(command.work_id.get()).bind(context.actor_id.get())
      .fetch_optional(&mut *tx).await?;
    let lease = lease.ok_or_else(|| AppError::conflict("cross-dock claim is no longer active"))?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        command.work_id.get(),
        None,
        Some(context.actor_id.get()),
        "cross_dock_heartbeat",
        None,
        None,
        None,
        None,
        None,
    )
    .await?;
    Ok(prepared
        .commit(
            tx,
            CrossDockClaimHeartbeatResult {
                work_id: command.work_id,
                heartbeat_at: now,
                lease_expires_at: lease,
            },
        )
        .await?)
}

pub async fn release_claim(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReleaseCrossDockClaimCommand,
) -> AppResult<CrossDockClaimReleaseResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RELEASE_CROSS_DOCK_CLAIM_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CrossDockClaimReleaseResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    require_work_visible_tx(&mut tx, access.tenant_id, command.work_id.get(), &scope).await?;
    let now = now_iso();
    let row = sqlx::query(
        r#"UPDATE work_tasks SET status='open',assigned_user_id=NULL,lease_expires_at=NULL,
             last_released_at=$1,release_count=release_count+1,modified=$1
           WHERE tenant_id=$2 AND id=$3 AND task_type='cross_dock' AND status='in_progress'
             AND assigned_user_id=$4 AND lease_expires_at>statement_timestamp()
           RETURNING release_count"#,
    )
    .bind(now)
    .bind(access.tenant_id.get())
    .bind(command.work_id.get())
    .bind(context.actor_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("cross-dock claim is no longer active"))?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        command.work_id.get(),
        None,
        Some(context.actor_id.get()),
        "cross_dock_released",
        None,
        None,
        None,
        command.note.as_deref(),
        Some(&serde_json::json!({"reason":command.reason.as_str()}).to_string()),
    )
    .await?;
    Ok(prepared
        .commit(
            tx,
            CrossDockClaimReleaseResult {
                work_id: command.work_id,
                status: CrossDockWorkStatus::Pending,
                released_at: now,
                release_count: row.try_get("release_count")?,
                reason: command.reason,
                note: command.note.clone(),
            },
        )
        .await?)
}

async fn release_stale_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    release_expired_tasks_tx(tx, access.tenant_id, Some(user_id), scope).await?;
    release_inaccessible_active_tasks_tx(tx, access.tenant_id, user_id, scope).await?;
    Ok(())
}

async fn require_no_other_active_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    except: Option<CrossDockWorkId>,
) -> AppResult<()> {
    let row = sqlx::query_scalar::<_, i64>(
        r#"SELECT id FROM work_tasks WHERE tenant_id=$1 AND assigned_user_id=$2
             AND deleted IS NULL AND status IN ('assigned','in_progress')
             AND ($3::BIGINT IS NULL OR id<>$3) ORDER BY id LIMIT 1 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(access.user_id.get())
    .bind(except.map(|id| id.get()))
    .fetch_optional(&mut **tx)
    .await?;
    if row.is_some() {
        Err(AppError::conflict("operator already has active work"))
    } else {
        Ok(())
    }
}

async fn load_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    work_id: i64,
    actor_id: i64,
) -> AppResult<CrossDockClaim> {
    let row=sqlx::query(
        r#"SELECT detail.*,work.priority,work.instructions,work.due_at,work.lease_expires_at,
                  orders.order_key,line.line_key,item.description AS item_description,
                  source.barcode AS source_barcode,source.name AS source_name,
                  destination.barcode AS destination_barcode,destination.name AS destination_name
           FROM work_tasks work JOIN cross_dock_tasks detail
             ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id
           JOIN orders ON orders.tenant_id=detail.tenant_id AND orders.inventory_owner_id=detail.inventory_owner_id AND orders.id=detail.order_id
           JOIN order_items line ON line.tenant_id=detail.tenant_id AND line.inventory_owner_id=detail.inventory_owner_id AND line.order_id=detail.order_id AND line.id=detail.order_item_id
           JOIN items item ON item.tenant_id=detail.tenant_id AND item.id=detail.item_id
           JOIN locations source ON source.tenant_id=detail.tenant_id AND source.id=detail.source_location_id
           JOIN locations destination ON destination.tenant_id=detail.tenant_id AND destination.id=detail.destination_location_id
           WHERE work.tenant_id=$1 AND work.id=$2 AND work.task_type='cross_dock'
             AND work.status='in_progress' AND work.assigned_user_id=$3
             AND work.lease_expires_at>statement_timestamp() AND detail.closed_at IS NULL"#,
    ).bind(access.tenant_id.get()).bind(work_id).bind(actor_id).fetch_optional(&mut **tx).await?
      .ok_or_else(||AppError::conflict("cross-dock claim is no longer active"))?;
    let item_id: i64 = row.try_get("item_id")?;
    let barcodes=sqlx::query_scalar::<_,String>("SELECT name FROM barcodes WHERE tenant_id=$1 AND item_id=$2 AND deleted IS NULL ORDER BY id")
      .bind(access.tenant_id.get()).bind(item_id).fetch_all(&mut **tx).await?
      .into_iter().map(CrossDockScanValue::new).collect::<Result<Vec<_>,_>>()
      .map_err(|e|AppError::internal(e.to_string()))?;
    let loc =
        |id: i64, barcode: String, name: Option<String>| -> AppResult<CrossDockLocationReadModel> {
            Ok(CrossDockLocationReadModel {
                location_id: LocationId::new(id).map_err(|e| AppError::internal(e.to_string()))?,
                barcode: CrossDockScanValue::new(barcode)
                    .map_err(|e| AppError::internal(e.to_string()))?,
                name,
            })
        };
    Ok(CrossDockClaim {
        work_id: CrossDockWorkId::new(work_id).map_err(|e| AppError::internal(e.to_string()))?,
        plan_id: CrossDockPlanId::new(row.try_get("plan_run_id")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        order_key: row.try_get("order_key")?,
        order_line_id: OrderLineId::new(row.try_get("order_item_id")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        order_line_key: row.try_get("line_key")?,
        reservation_id: row.try_get("reservation_id")?,
        priority: row.try_get("priority")?,
        instructions: row.try_get("instructions")?,
        due_at: row.try_get("due_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        source_receipt_inventory_transaction_id: row
            .try_get("source_receipt_inventory_transaction_id")?,
        source_inventory_balance_id: InventoryBalanceId::new(
            row.try_get("source_inventory_balance_id")?,
        )
        .map_err(|e| AppError::internal(e.to_string()))?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        item_id: CatalogItemId::new(item_id).map_err(|e| AppError::internal(e.to_string()))?,
        item_description: row.try_get("item_description")?,
        item_barcodes: barcodes,
        uom: CrossDockUom::new(row.try_get::<String, _>("uom")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        quantity: CrossDockQuantity::new(row.try_get("planned_quantity")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        source_receiving_location: loc(
            row.try_get("source_location_id")?,
            row.try_get("source_barcode")?,
            row.try_get("source_name")?,
        )?,
        destination_pick_face: loc(
            row.try_get("destination_location_id")?,
            row.try_get("destination_barcode")?,
            row.try_get("destination_name")?,
        )?,
    })
}
