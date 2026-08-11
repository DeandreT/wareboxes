use sqlx::Row;
use wareboxes_application::cross_dock::{
    ConfirmCrossDockWorkCommand, ConfirmCrossDockWorkResult, CONFIRM_CROSS_DOCK_WORK_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    CatalogItemId, CrossDockConfirmationId, CrossDockPlanId, CrossDockQuantity, CrossDockUom,
    CrossDockWorkId, CrossDockWorkStatus, FacilityId, InventoryBalanceId, InventoryOwnerId,
    ItemBatchId, LocationId, OrderId, OrderLineId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory;
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::orders::insert_order_activity_tx;
use crate::repo::tasks::insert_progress_tx;

use super::{enqueue_event_tx, require_scope, require_stored_work_visible_before_replay_tx};

struct Target {
    work_id: CrossDockWorkId,
    plan_id: CrossDockPlanId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    order_id: OrderId,
    order_line_id: OrderLineId,
    reservation_id: i64,
    source_balance_id: InventoryBalanceId,
    source_location_id: LocationId,
    destination_location_id: LocationId,
    item_batch_id: ItemBatchId,
    item_id: CatalogItemId,
    uom: CrossDockUom,
    quantity: CrossDockQuantity,
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
    command: &ConfirmCrossDockWorkCommand,
) -> AppResult<ConfirmCrossDockWorkResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CONFIRM_CROSS_DOCK_WORK_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_work_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ConfirmCrossDockWorkResult>(&mut tx)
        .await?
    {
        require_confirmation_visible_tx(
            &mut tx,
            access.tenant_id.get(),
            result.confirmation_id.get(),
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
    validate_scans_tx(&mut tx, access.tenant_id.get(), &target, command).await?;
    inventory::ensure_location_accepts_batch_tx(
        &mut tx,
        access.tenant_id,
        target.owner_id.get(),
        target.destination_location_id.get(),
        target.item_batch_id.get(),
    )
    .await?;
    let destination_existing: Option<i64> = sqlx::query_scalar(
        r#"SELECT id FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND facility_id=$3 AND location_id=$4 AND license_plate_id IS NULL
             AND item_batch_id=$5 AND uom=$6 AND status='available'"#,
    )
    .bind(access.tenant_id.get())
    .bind(target.owner_id.get())
    .bind(target.facility_id.get())
    .bind(target.destination_location_id.get())
    .bind(target.item_batch_id.get())
    .bind(target.uom.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let mut ids = vec![target.source_balance_id.get()];
    if let Some(id) = destination_existing {
        ids.push(id)
    }
    ids.sort_unstable();
    ids.dedup();
    let rows = sqlx::query(
        r#"SELECT id,location_id,item_batch_id,item_id,uom,status,license_plate_id,
                  qty_on_hand,qty_reserved,qty_held,deleted
           FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND facility_id=$3 AND id=ANY($4) ORDER BY id FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(target.owner_id.get())
    .bind(target.facility_id.get())
    .bind(&ids)
    .fetch_all(&mut *tx)
    .await?;
    let source = rows
        .iter()
        .find(|row| row.try_get::<i64, _>("id").ok() == Some(target.source_balance_id.get()))
        .ok_or_else(|| AppError::conflict("cross-dock source inventory is no longer active"))?;
    let free = source.try_get::<i64, _>("qty_on_hand")?
        - source.try_get::<i64, _>("qty_reserved")?
        - source.try_get::<i64, _>("qty_held")?;
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
            "cross-dock source inventory changed after planning",
        ));
    }
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
            reason: Some("scanner-confirmed inbound cross-dock"),
            reference_type: Some("cross_dock_task"),
            reference_id: Some(target.work_id.get()),
            correlation_id: Some(&context.request_id),
            operation: CONFIRM_CROSS_DOCK_WORK_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;
    let changed = sqlx::query(
        r#"UPDATE inventory_balances SET qty_on_hand=qty_on_hand-$1,modified=$2
         WHERE tenant_id=$3 AND inventory_owner_id=$4 AND facility_id=$5 AND id=$6
           AND deleted IS NULL AND qty_on_hand-qty_reserved-qty_held >= $1"#,
    )
    .bind(target.quantity.get())
    .bind(confirmed_at)
    .bind(access.tenant_id.get())
    .bind(target.owner_id.get())
    .bind(target.facility_id.get())
    .bind(target.source_balance_id.get())
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::conflict(
            "cross-dock source changed during confirmation",
        ));
    }
    let destination_balance_id = InventoryBalanceId::new(
        sqlx::query_scalar(
            r#"INSERT INTO inventory_balances
         (tenant_id,inventory_owner_id,created,modified,facility_id,location_id,license_plate_id,
          item_batch_id,item_id,uom,status,qty_on_hand,qty_reserved)
         VALUES ($1,$2,$3,$3,$4,$5,NULL,$6,$7,$8,'available',$9,0)
         ON CONFLICT (tenant_id,inventory_owner_id,location_id,item_batch_id,uom,status)
           WHERE license_plate_id IS NULL
         DO UPDATE SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
           modified=excluded.modified,deleted=NULL RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(target.owner_id.get())
        .bind(confirmed_at)
        .bind(target.facility_id.get())
        .bind(target.destination_location_id.get())
        .bind(target.item_batch_id.get())
        .bind(target.item_id.get())
        .bind(target.uom.as_str())
        .bind(target.quantity.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|e| AppError::internal(e.to_string()))?;
    for (location, delta) in [
        (target.source_location_id.get(), -target.quantity.get()),
        (target.destination_location_id.get(), target.quantity.get()),
    ] {
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            inventory_journal::owner_facility_scope(
                target.owner_id.get(),
                target.facility_id.get(),
            )?,
            transaction_id,
            &JournalEntry {
                location_id: location,
                license_plate_id: None,
                item_batch_id: target.item_batch_id.get(),
                status: InventoryStatus::Available,
                quantity_delta: delta,
            },
        )
        .await?;
    }
    let allocation_id:i64=sqlx::query_scalar(
      r#"INSERT INTO inventory_allocations
         (tenant_id,inventory_owner_id,created,modified,created_by,reservation_id,
          inventory_balance_id,facility_id,location_id,license_plate_id,item_batch_id,item_id,uom,
          inventory_status,allocation_run_id,qty,status,execution_stage)
         VALUES ($1,$2,$3,$3,$4,$5,$6,$7,$8,NULL,$9,$10,$11,'available',NULL,$12,'allocated','pick_source')
         RETURNING id"#,
    ).bind(access.tenant_id.get()).bind(target.owner_id.get()).bind(confirmed_at)
      .bind(context.actor_id.get()).bind(target.reservation_id).bind(destination_balance_id.get())
      .bind(target.facility_id.get()).bind(target.destination_location_id.get())
      .bind(target.item_batch_id.get()).bind(target.item_id.get()).bind(target.uom.as_str())
      .bind(target.quantity.get()).fetch_one(&mut *tx).await?;
    let completed=sqlx::query(
      r#"UPDATE work_tasks SET status='completed',completed_by=$1,completed_at=$2,
           lease_expires_at=NULL,modified=$2 WHERE tenant_id=$3 AND id=$4 AND task_type='cross_dock'
           AND status='in_progress' AND assigned_user_id=$1 AND lease_expires_at>statement_timestamp()"#,
    ).bind(context.actor_id.get()).bind(confirmed_at).bind(access.tenant_id.get()).bind(target.work_id.get())
      .execute(&mut *tx).await?;
    if completed.rows_affected() != 1 {
        return Err(AppError::conflict(
            "cross-dock claim expired during confirmation",
        ));
    }
    let confirmation_id=CrossDockConfirmationId::new(sqlx::query_scalar(
      r#"INSERT INTO cross_dock_confirmations
         (tenant_id,task_id,plan_run_id,inventory_owner_id,facility_id,order_id,order_item_id,
          reservation_id,inventory_transaction_id,inventory_allocation_id,source_inventory_balance_id,
          destination_inventory_balance_id,source_location_id,destination_location_id,item_batch_id,
          item_id,uom,lot,serial,expiration,quantity,confirmed_by_user_id,confirmed_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23)
         RETURNING id"#,
    ).bind(access.tenant_id.get()).bind(target.work_id.get()).bind(target.plan_id.get())
      .bind(target.owner_id.get()).bind(target.facility_id.get()).bind(target.order_id.get())
      .bind(target.order_line_id.get()).bind(target.reservation_id).bind(transaction_id).bind(allocation_id)
      .bind(target.source_balance_id.get()).bind(destination_balance_id.get())
      .bind(target.source_location_id.get()).bind(target.destination_location_id.get())
      .bind(target.item_batch_id.get()).bind(target.item_id.get()).bind(target.uom.as_str())
      .bind(&target.lot).bind(&target.serial).bind(target.expiration).bind(target.quantity.get())
      .bind(context.actor_id.get()).bind(confirmed_at).fetch_one(&mut *tx).await?)
      .map_err(|e|AppError::internal(e.to_string()))?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        target.work_id.get(),
        None,
        Some(context.actor_id.get()),
        "cross_dock_confirmed",
        Some(target.quantity.get()),
        Some(target.source_location_id.get()),
        Some(target.destination_location_id.get()),
        None,
        None,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        target.owner_id,
        target.order_id.get(),
        Some(context.actor_id.get()),
        "cross_dock_confirmed",
    )
    .await?;
    let result = ConfirmCrossDockWorkResult {
        confirmation_id,
        work_id: target.work_id,
        plan_id: target.plan_id,
        order_id: target.order_id,
        order_line_id: target.order_line_id,
        reservation_id: target.reservation_id,
        inventory_transaction_id: transaction_id,
        inventory_allocation_id: allocation_id,
        source_inventory_balance_id: target.source_balance_id,
        destination_inventory_balance_id: destination_balance_id,
        source_location_id: target.source_location_id,
        destination_pick_face_location_id: target.destination_location_id,
        item_batch_id: target.item_batch_id,
        item_id: target.item_id,
        uom: target.uom.clone(),
        lot: target.lot.clone(),
        serial: target.serial.clone(),
        quantity: target.quantity,
        work_status: CrossDockWorkStatus::Completed,
        confirmed_by: UserId::new(context.actor_id.get())
            .map_err(|e| AppError::internal(e.to_string()))?,
        confirmed_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        target.owner_id,
        target.facility_id,
        context.actor_id.get(),
        target.order_id.get(),
        "inbound.cross_dock.confirmed",
        &format!("cross-dock-confirmation:{}", confirmation_id.get()),
        &serde_json::to_value(&result).map_err(|e| AppError::internal(e.to_string()))?,
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
    work_id: CrossDockWorkId,
    actor_id: i64,
    scope: &ScopeBindings,
) -> AppResult<Target> {
    let hint=sqlx::query("SELECT order_id,inventory_owner_id,facility_id FROM cross_dock_tasks WHERE tenant_id=$1 AND task_id=$2")
      .bind(access.tenant_id.get()).bind(work_id.get()).fetch_optional(&mut **tx).await?
      .ok_or_else(||AppError::not_found("cross-dock work"))?;
    require_scope(
        scope,
        hint.try_get("inventory_owner_id")?,
        hint.try_get("facility_id")?,
    )?;
    let order_id: i64 = hint.try_get("order_id")?;
    sqlx::query("SELECT id FROM orders WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(access.tenant_id.get())
        .bind(order_id)
        .fetch_one(&mut **tx)
        .await?;
    let row=sqlx::query(
      r#"SELECT detail.*,work.status,work.assigned_user_id,
                work.lease_expires_at>statement_timestamp() AS lease_current,
                orders.status AS order_status,reservation.status AS reservation_status,
                source.barcode AS source_barcode,source.active AS source_active,source.receivable AS source_receivable,
                destination.barcode AS destination_barcode,destination.active AS destination_active,
                destination.pickable AS destination_pickable,destination.receivable AS destination_receivable
         FROM work_tasks work JOIN cross_dock_tasks detail ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id
         JOIN orders ON orders.tenant_id=detail.tenant_id AND orders.inventory_owner_id=detail.inventory_owner_id AND orders.id=detail.order_id
         JOIN inventory_reservations reservation ON reservation.tenant_id=detail.tenant_id
           AND reservation.inventory_owner_id=detail.inventory_owner_id AND reservation.id=detail.reservation_id
         JOIN locations source ON source.tenant_id=detail.tenant_id AND source.facility_id=detail.facility_id
           AND source.id=detail.source_location_id AND source.deleted IS NULL
         JOIN locations destination ON destination.tenant_id=detail.tenant_id AND destination.facility_id=detail.facility_id
           AND destination.id=detail.destination_location_id AND destination.deleted IS NULL
         WHERE work.tenant_id=$1 AND work.id=$2 AND work.task_type='cross_dock' AND work.deleted IS NULL
         FOR UPDATE OF work,reservation"#,
    ).bind(access.tenant_id.get()).bind(work_id.get()).fetch_optional(&mut **tx).await?
      .ok_or_else(||AppError::not_found("cross-dock work"))?;
    if row.try_get::<String, _>("status")? != "in_progress"
        || row.try_get::<Option<i64>, _>("assigned_user_id")? != Some(actor_id)
        || row.try_get::<Option<bool>, _>("lease_current")? != Some(true)
        || row.try_get::<Option<Timestamp>, _>("closed_at")?.is_some()
        || row.try_get::<String, _>("order_status")? != "open"
        || row.try_get::<String, _>("reservation_status")? != "active"
    {
        return Err(AppError::conflict(
            "cross-dock work no longer has executable demand or claim",
        ));
    }
    if !row.try_get::<bool, _>("source_active")?
        || !row.try_get::<bool, _>("source_receivable")?
        || !row.try_get::<bool, _>("destination_active")?
        || !row.try_get::<bool, _>("destination_pickable")?
        || row.try_get::<bool, _>("destination_receivable")?
    {
        return Err(AppError::conflict(
            "cross-dock source or destination is no longer executable",
        ));
    }
    Ok(Target {
        work_id,
        plan_id: id(&row, "plan_run_id", CrossDockPlanId::new)?,
        owner_id: id(&row, "inventory_owner_id", InventoryOwnerId::new)?,
        facility_id: id(&row, "facility_id", FacilityId::new)?,
        order_id: id(&row, "order_id", OrderId::new)?,
        order_line_id: id(&row, "order_item_id", OrderLineId::new)?,
        reservation_id: row.try_get("reservation_id")?,
        source_balance_id: id(&row, "source_inventory_balance_id", InventoryBalanceId::new)?,
        source_location_id: id(&row, "source_location_id", LocationId::new)?,
        destination_location_id: id(&row, "destination_location_id", LocationId::new)?,
        item_batch_id: id(&row, "item_batch_id", ItemBatchId::new)?,
        item_id: id(&row, "item_id", CatalogItemId::new)?,
        uom: CrossDockUom::new(row.try_get::<String, _>("uom")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        quantity: CrossDockQuantity::new(row.try_get("planned_quantity")?)
            .map_err(|e| AppError::internal(e.to_string()))?,
        source_barcode: row.try_get("source_barcode")?,
        destination_barcode: row.try_get("destination_barcode")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
    })
}

async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    target: &Target,
    command: &ConfirmCrossDockWorkCommand,
) -> AppResult<()> {
    if command.source_receiving_location_barcode.as_str() != target.source_barcode {
        return Err(AppError::bad_request(
            "scanned source location does not match cross-dock work",
        ));
    }
    if command.destination_pick_face_barcode.as_str() != target.destination_barcode {
        return Err(AppError::bad_request(
            "scanned destination does not match cross-dock work",
        ));
    }
    let item_matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM barcodes WHERE tenant_id=$1 AND item_id=$2 AND deleted IS NULL AND lower(name)=lower($3))")
      .bind(tenant_id).bind(target.item_id.get()).bind(command.item_barcode.as_str()).fetch_one(&mut **tx).await?;
    if !item_matches {
        return Err(AppError::bad_request(
            "scanned item does not match cross-dock work",
        ));
    }
    if command.lot_scan.as_ref().map(|v| v.as_str()) != target.lot.as_deref() {
        return Err(AppError::bad_request(
            "scanned lot does not match cross-dock work",
        ));
    }
    if command.serial_scan.as_ref().map(|v| v.as_str()) != target.serial.as_deref() {
        return Err(AppError::bad_request(
            "scanned serial does not match cross-dock work",
        ));
    }
    Ok(())
}

async fn require_confirmation_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row=sqlx::query("SELECT inventory_owner_id,facility_id FROM cross_dock_confirmations WHERE tenant_id=$1 AND id=$2")
      .bind(tenant_id).bind(id).fetch_optional(&mut **tx).await?.ok_or_else(||AppError::not_found("cross-dock confirmation"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}

fn id<T, E>(
    row: &sqlx::postgres::PgRow,
    column: &str,
    ctor: fn(i64) -> Result<T, E>,
) -> AppResult<T>
where
    E: std::fmt::Display,
{
    ctor(row.try_get(column)?).map_err(|e| AppError::internal(e.to_string()))
}
