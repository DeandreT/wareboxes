use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::replenishment::{
    ConfirmReplenishmentWorkCommand, ConfirmReplenishmentWorkResult,
    CONFIRM_REPLENISHMENT_WORK_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId, ItemBatchId, LocationId,
    ReplenishmentConfirmationId, ReplenishmentMoveQuantity, ReplenishmentPlanId,
    ReplenishmentPolicyId, ReplenishmentUom, ReplenishmentWorkId, ReplenishmentWorkStatus,
    Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory;
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::tasks::insert_progress_tx;

use super::{enqueue_event_tx, require_scope, require_stored_work_visible_before_replay_tx};

#[derive(Debug)]
struct Target {
    work_id: ReplenishmentWorkId,
    plan_id: ReplenishmentPlanId,
    policy_id: ReplenishmentPolicyId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    source_balance_id: InventoryBalanceId,
    source_location_id: LocationId,
    destination_location_id: LocationId,
    item_batch_id: ItemBatchId,
    item_id: CatalogItemId,
    uom: ReplenishmentUom,
    quantity: ReplenishmentMoveQuantity,
    source_barcode: String,
    destination_barcode: String,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
}

pub async fn confirm_work(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfirmReplenishmentWorkCommand,
) -> AppResult<ConfirmReplenishmentWorkResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        CONFIRM_REPLENISHMENT_WORK_OPERATION,
        &(
            command.work_id,
            &command.source_location_barcode,
            &command.item_barcode,
            &command.lot_scan,
            &command.serial_scan,
            &command.destination_pick_face_barcode,
        ),
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ConfirmReplenishmentWorkResult>(&mut tx)
        .await?
    {
        require_replayed_confirmation_visible_tx(
            &mut tx,
            access.tenant_id,
            result.confirmation_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let target = lock_target_tx(
        &mut tx,
        access,
        command.work_id,
        context.actor_id.get(),
        &scope,
    )
    .await?;
    validate_scans_tx(&mut tx, access.tenant_id, &target, command).await?;
    inventory::ensure_location_accepts_batch_tx(
        &mut tx,
        access.tenant_id,
        target.owner_id.get(),
        target.destination_location_id.get(),
        target.item_batch_id.get(),
    )
    .await?;
    lock_balances_tx(&mut tx, access.tenant_id, &target).await?;
    let confirmed_at = now_iso();
    let transaction_id = inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility: inventory_journal::owner_facility_scope(
                target.owner_id.get(),
                target.facility_id.get(),
            )?,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("scanner-confirmed pick-face replenishment"),
            reference_type: Some("replenishment_task"),
            reference_id: Some(target.work_id.get()),
            correlation_id: Some(&context.request_id),
            operation: CONFIRM_REPLENISHMENT_WORK_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;
    decrement_source_tx(&mut tx, access.tenant_id, &target, confirmed_at).await?;
    let destination_balance_id =
        increment_destination_tx(&mut tx, access.tenant_id, &target, confirmed_at).await?;
    append_entries_tx(&mut tx, access.tenant_id, &target, transaction_id).await?;
    complete_work_tx(
        &mut tx,
        access.tenant_id,
        &target,
        context.actor_id.get(),
        confirmed_at,
    )
    .await?;
    let confirmation_id = insert_confirmation_tx(
        &mut tx,
        access.tenant_id,
        &target,
        destination_balance_id,
        transaction_id,
        context.actor_id.get(),
        confirmed_at,
    )
    .await?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        target.work_id.get(),
        None,
        Some(context.actor_id.get()),
        "replenishment_confirmed",
        Some(target.quantity.get()),
        Some(target.source_location_id.get()),
        Some(target.destination_location_id.get()),
        None,
        None,
    )
    .await?;
    let result = ConfirmReplenishmentWorkResult {
        confirmation_id,
        work_id: target.work_id,
        plan_id: target.plan_id,
        policy_id: target.policy_id,
        inventory_transaction_id: transaction_id,
        source_inventory_balance_id: target.source_balance_id,
        destination_inventory_balance_id: destination_balance_id,
        item_batch_id: target.item_batch_id,
        item_id: target.item_id,
        uom: target.uom.clone(),
        lot: target.lot.clone(),
        serial: target.serial.clone(),
        source_location_id: target.source_location_id,
        destination_pick_face_location_id: target.destination_location_id,
        quantity: target.quantity,
        work_status: ReplenishmentWorkStatus::Completed,
        confirmed_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        confirmed_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        target.owner_id,
        target.facility_id,
        context.actor_id.get(),
        "replenishment_task",
        target.work_id.get(),
        "inventory.replenishment.confirmed",
        &format!("confirmed:{}", confirmation_id.get()),
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
        confirmed_at,
    )
    .await?;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await?)
}

async fn lock_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    work_id: ReplenishmentWorkId,
    actor_id: i64,
    scope: &ScopeBindings,
) -> AppResult<Target> {
    let row = sqlx::query(
        r#"
        SELECT work.status,work.assigned_user_id,
          work.lease_expires_at>statement_timestamp() AS lease_current,
          detail.plan_run_id,detail.policy_id,detail.inventory_owner_id,detail.facility_id,
          detail.source_inventory_balance_id,detail.source_location_id,detail.destination_location_id,
          detail.item_batch_id,detail.item_id,detail.uom,detail.planned_qty,detail.closed_at,
          source.barcode AS source_barcode,source.active AS source_active,
          source.pickable AS source_pickable,source.receivable AS source_receivable,
          destination.barcode AS destination_barcode,destination.active AS destination_active,
          destination.pickable AS destination_pickable,destination.receivable AS destination_receivable,
          detail.source_lot AS lot,detail.source_serial AS serial,
          detail.source_expiration AS expiration
        FROM work_tasks work
        JOIN replenishment_tasks detail ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id
        JOIN locations source ON source.tenant_id=detail.tenant_id AND source.facility_id=detail.facility_id
          AND source.id=detail.source_location_id AND source.deleted IS NULL
        JOIN locations destination ON destination.tenant_id=detail.tenant_id AND destination.facility_id=detail.facility_id
          AND destination.id=detail.destination_location_id AND destination.deleted IS NULL
        JOIN item_batches batch ON batch.tenant_id=detail.tenant_id
          AND batch.inventory_owner_id=detail.inventory_owner_id AND batch.id=detail.item_batch_id
          AND batch.deleted IS NULL
        WHERE work.tenant_id=$1 AND work.id=$2 AND work.task_type='replenishment' AND work.deleted IS NULL
        FOR UPDATE OF work
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(work_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment work"))?;
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    require_scope(scope, owner_id, facility_id)?;
    if row.try_get::<String, _>("status")? != "in_progress"
        || row.try_get::<Option<i64>, _>("assigned_user_id")? != Some(actor_id)
        || row.try_get::<Option<bool>, _>("lease_current")? != Some(true)
        || row.try_get::<Option<Timestamp>, _>("closed_at")?.is_some()
    {
        return Err(AppError::conflict(
            "replenishment work does not have an active claim for this operator",
        ));
    }
    if !row.try_get::<bool, _>("source_active")?
        || row.try_get::<bool, _>("source_pickable")?
        || row.try_get::<bool, _>("source_receivable")?
        || !row.try_get::<bool, _>("destination_active")?
        || !row.try_get::<bool, _>("destination_pickable")?
        || row.try_get::<bool, _>("destination_receivable")?
    {
        return Err(AppError::conflict(
            "replenishment source or destination is no longer executable",
        ));
    }
    Ok(Target {
        work_id,
        plan_id: ReplenishmentPlanId::new(row.try_get("plan_run_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        policy_id: ReplenishmentPolicyId::new(row.try_get("policy_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        owner_id: InventoryOwnerId::new(owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(facility_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_balance_id: InventoryBalanceId::new(row.try_get("source_inventory_balance_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_id: LocationId::new(row.try_get("source_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_id: CatalogItemId::new(row.try_get("item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        uom: ReplenishmentUom::new(row.try_get::<String, _>("uom")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        quantity: ReplenishmentMoveQuantity::new(row.try_get("planned_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_barcode: row.try_get("source_barcode")?,
        destination_barcode: row.try_get("destination_barcode")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
    })
}

async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &Target,
    command: &ConfirmReplenishmentWorkCommand,
) -> AppResult<()> {
    if command.source_location_barcode.as_str() != target.source_barcode {
        return Err(AppError::bad_request(
            "scanned source location does not match replenishment work",
        ));
    }
    if command.destination_pick_face_barcode.as_str() != target.destination_barcode {
        return Err(AppError::bad_request(
            "scanned destination does not match replenishment work",
        ));
    }
    let item_matches: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM barcodes WHERE tenant_id=$1 AND item_id=$2 AND deleted IS NULL AND lower(name)=lower($3))",
    )
    .bind(tenant_id.get())
    .bind(target.item_id.get())
    .bind(command.item_barcode.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if !item_matches {
        return Err(AppError::bad_request(
            "scanned item does not match replenishment work",
        ));
    }
    if command.lot_scan.as_ref().map(|value| value.as_str()) != target.lot.as_deref() {
        return Err(AppError::bad_request(
            "scanned lot does not match replenishment work",
        ));
    }
    if command.serial_scan.as_ref().map(|value| value.as_str()) != target.serial.as_deref() {
        return Err(AppError::bad_request(
            "scanned serial does not match replenishment work",
        ));
    }
    Ok(())
}

async fn lock_balances_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &Target,
) -> AppResult<()> {
    let destination_id: Option<i64> = sqlx::query_scalar(
        r#"SELECT id FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND facility_id=$3 AND location_id=$4 AND license_plate_id IS NULL
             AND item_batch_id=$5 AND uom=$6 AND status='available'"#,
    )
    .bind(tenant_id.get())
    .bind(target.owner_id.get())
    .bind(target.facility_id.get())
    .bind(target.destination_location_id.get())
    .bind(target.item_batch_id.get())
    .bind(target.uom.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    let mut ids = vec![target.source_balance_id.get()];
    if let Some(id) = destination_id {
        ids.push(id);
    }
    ids.sort_unstable();
    ids.dedup();
    let rows = sqlx::query(
        r#"SELECT id,location_id,item_batch_id,item_id,uom,status,license_plate_id,
             qty_on_hand,qty_reserved,qty_held,deleted
           FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND facility_id=$3 AND id=ANY($4) ORDER BY id FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(target.owner_id.get())
    .bind(target.facility_id.get())
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let source = rows
        .iter()
        .find(|row| row.try_get::<i64, _>("id").ok() == Some(target.source_balance_id.get()))
        .ok_or_else(|| AppError::conflict("replenishment source balance is no longer active"))?;
    let free = source
        .try_get::<i64, _>("qty_on_hand")?
        .checked_sub(source.try_get("qty_reserved")?)
        .and_then(|value| value.checked_sub(source.try_get::<i64, _>("qty_held").ok()?))
        .ok_or_else(|| AppError::internal("source free quantity overflow"))?;
    if free < target.quantity.get()
        || source.try_get::<i64, _>("location_id")? != target.source_location_id.get()
        || source.try_get::<i64, _>("item_batch_id")? != target.item_batch_id.get()
        || source.try_get::<i64, _>("item_id")? != target.item_id.get()
        || source.try_get::<String, _>("uom")? != target.uom.as_str()
        || source.try_get::<String, _>("status")? != "available"
        || source
            .try_get::<Option<i64>, _>("license_plate_id")?
            .is_some()
        || source.try_get::<Option<Timestamp>, _>("deleted")?.is_some()
    {
        return Err(AppError::conflict(
            "replenishment source inventory changed after planning",
        ));
    }
    Ok(())
}

async fn decrement_source_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &Target,
    confirmed_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE inventory_balances SET qty_on_hand=qty_on_hand-$1,modified=$2
           WHERE tenant_id=$3 AND inventory_owner_id=$4 AND facility_id=$5 AND id=$6
             AND location_id=$7 AND item_batch_id=$8 AND item_id=$9 AND uom=$10
             AND status='available' AND license_plate_id IS NULL AND deleted IS NULL
             AND qty_on_hand-qty_reserved-qty_held >= $1"#,
    )
    .bind(target.quantity.get())
    .bind(confirmed_at)
    .bind(tenant_id.get())
    .bind(target.owner_id.get())
    .bind(target.facility_id.get())
    .bind(target.source_balance_id.get())
    .bind(target.source_location_id.get())
    .bind(target.item_batch_id.get())
    .bind(target.item_id.get())
    .bind(target.uom.as_str())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "replenishment source inventory changed during confirmation",
        ));
    }
    Ok(())
}

async fn increment_destination_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &Target,
    confirmed_at: Timestamp,
) -> AppResult<InventoryBalanceId> {
    let id: i64 = sqlx::query_scalar(
        r#"INSERT INTO inventory_balances (tenant_id,inventory_owner_id,created,modified,facility_id,
             location_id,license_plate_id,item_batch_id,item_id,uom,status,qty_on_hand,qty_reserved)
           VALUES ($1,$2,$3,$3,$4,$5,NULL,$6,$7,$8,'available',$9,0)
           ON CONFLICT (tenant_id,inventory_owner_id,location_id,item_batch_id,uom,status)
             WHERE license_plate_id IS NULL
           DO UPDATE SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
             modified=excluded.modified,deleted=NULL RETURNING id"#,
    )
    .bind(tenant_id.get()).bind(target.owner_id.get()).bind(confirmed_at)
    .bind(target.facility_id.get()).bind(target.destination_location_id.get())
    .bind(target.item_batch_id.get()).bind(target.item_id.get()).bind(target.uom.as_str())
    .bind(target.quantity.get()).fetch_one(&mut **tx).await?;
    InventoryBalanceId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn append_entries_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &Target,
    transaction_id: i64,
) -> AppResult<()> {
    let owner_facility =
        inventory_journal::owner_facility_scope(target.owner_id.get(), target.facility_id.get())?;
    for (location_id, quantity_delta) in [
        (target.source_location_id.get(), -target.quantity.get()),
        (target.destination_location_id.get(), target.quantity.get()),
    ] {
        inventory_journal::append_entry(
            tx,
            tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id,
                license_plate_id: None,
                item_batch_id: target.item_batch_id.get(),
                status: InventoryStatus::Available,
                quantity_delta,
            },
        )
        .await?;
    }
    Ok(())
}

async fn complete_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &Target,
    actor_id: i64,
    confirmed_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE work_tasks SET status='completed',completed_by=$1,completed_at=$2,
             lease_expires_at=NULL,modified=$2 WHERE tenant_id=$3 AND id=$4
             AND task_type='replenishment' AND deleted IS NULL AND status='in_progress'
             AND assigned_user_id=$1 AND lease_expires_at>statement_timestamp()"#,
    )
    .bind(actor_id)
    .bind(confirmed_at)
    .bind(tenant_id.get())
    .bind(target.work_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "replenishment claim expired during confirmation",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_confirmation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &Target,
    destination_balance_id: InventoryBalanceId,
    transaction_id: i64,
    actor_id: i64,
    confirmed_at: Timestamp,
) -> AppResult<ReplenishmentConfirmationId> {
    let id: i64 = sqlx::query_scalar(
        r#"INSERT INTO replenishment_confirmations (
             tenant_id,task_id,plan_run_id,policy_id,policy_revision,inventory_owner_id,facility_id,
             inventory_transaction_id,source_inventory_balance_id,destination_inventory_balance_id,
             source_location_id,destination_location_id,item_batch_id,item_id,uom,lot,expiration,serial,
             inventory_status,quantity,confirmed_by_user_id,confirmed_at)
           SELECT $1,detail.task_id,detail.plan_run_id,detail.policy_id,detail.policy_revision,
             detail.inventory_owner_id,detail.facility_id,$2,detail.source_inventory_balance_id,$3,
             detail.source_location_id,detail.destination_location_id,detail.item_batch_id,detail.item_id,
             detail.uom,$4,$5,$6,detail.inventory_status,detail.planned_qty,$7,$8
           FROM replenishment_tasks detail WHERE detail.tenant_id=$1 AND detail.task_id=$9
           RETURNING id"#,
    ).bind(tenant_id.get()).bind(transaction_id).bind(destination_balance_id.get())
      .bind(&target.lot).bind(target.expiration).bind(&target.serial).bind(actor_id)
      .bind(confirmed_at).bind(target.work_id.get()).fetch_one(&mut **tx).await?;
    ReplenishmentConfirmationId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn require_replayed_confirmation_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    confirmation_id: ReplenishmentConfirmationId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM replenishment_confirmations WHERE tenant_id=$1 AND id=$2",
    ).bind(tenant_id.get()).bind(confirmation_id.get()).fetch_optional(&mut **tx).await?
      .ok_or_else(|| AppError::not_found("replenishment confirmation"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}
