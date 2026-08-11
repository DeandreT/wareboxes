use sqlx::Row;
use wareboxes_application::cross_dock::{
    PlanCrossDockWorkCommand, PlanCrossDockWorkResult, PLAN_CROSS_DOCK_WORK_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::OrderStatus;
use wareboxes_core::models::{TenantAccess, WorkTaskType};
use wareboxes_domain::{
    plan_cross_dock, CatalogItemId, CrossDockPlanId, CrossDockPlanningSnapshot, CrossDockUom,
    CrossDockWorkId, CrossDockWorkStatus, FacilityId, InboundLoadId, InventoryBalanceId,
    InventoryOwnerId, ItemBatchId, LocationId, OrderRevision, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::orders::insert_order_activity_tx;
use crate::repo::tasks::{insert_task_tx, task_permission, task_timeout_seconds, NewWorkTask};

use super::{enqueue_event_tx, require_scope, require_stored_work_visible_before_replay_tx};

struct SourceHint {
    owner_id: i64,
    facility_id: i64,
    inbound_load_id: i64,
    balance_id: i64,
    location_id: i64,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<wareboxes_domain::Timestamp>,
    receipt_quantity: i64,
}

pub async fn plan_work(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanCrossDockWorkCommand,
) -> AppResult<PlanCrossDockWorkResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, PLAN_CROSS_DOCK_WORK_OPERATION, command)?;
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
        .replayed::<PlanCrossDockWorkResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let source = source_hint_tx(
        &mut tx,
        access.tenant_id.get(),
        command.source_receipt_inventory_transaction_id,
    )
    .await?;
    require_scope(&scope, source.owner_id, source.facility_id)?;
    let row = sqlx::query(
        r#"SELECT orders.status,orders.revision,orders.order_key,line.line_key,
                  reservation.id AS reservation_id,reservation.qty AS reservation_quantity
           FROM orders
           JOIN order_items line ON line.tenant_id=orders.tenant_id
             AND line.inventory_owner_id=orders.inventory_owner_id AND line.order_id=orders.id
             AND line.id=$4 AND line.deleted IS NULL
           JOIN inventory_reservations reservation ON reservation.tenant_id=orders.tenant_id
             AND reservation.inventory_owner_id=orders.inventory_owner_id
             AND reservation.order_id=orders.id AND reservation.order_item_id=line.id
             AND reservation.facility_id=$5 AND reservation.status='active'
             AND reservation.deleted IS NULL
           WHERE orders.tenant_id=$1 AND orders.inventory_owner_id=$2
             AND orders.id=$3 AND orders.deleted IS NULL
             AND line.item_id=$6 AND line.uom=$7
           FOR UPDATE OF orders,reservation"#,
    )
    .bind(access.tenant_id.get())
    .bind(source.owner_id)
    .bind(command.order_id.get())
    .bind(command.order_line_id.get())
    .bind(source.facility_id)
    .bind(source.item_id)
    .bind(&source.uom)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("cross-dock order demand"))?;
    let revision: i64 = row.try_get("revision")?;
    if revision != command.expected_order_revision.get() {
        return Err(AppError::conflict(
            "order revision does not match expected revision",
        ));
    }
    if row.try_get::<String, _>("status")? != "open" {
        return Err(AppError::conflict("cross-dock work requires an open order"));
    }
    let reservation_id: i64 = row.try_get("reservation_id")?;
    let reservation_quantity: i64 = row.try_get("reservation_quantity")?;
    let active_allocation_rows = sqlx::query(
        r#"SELECT id,qty FROM inventory_allocations
           WHERE tenant_id=$1 AND inventory_owner_id=$2 AND reservation_id=$3
             AND status='allocated' AND deleted IS NULL ORDER BY id FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(source.owner_id)
    .bind(reservation_id)
    .fetch_all(&mut *tx)
    .await?;
    let active_allocation_quantity =
        active_allocation_rows.iter().try_fold(0_i64, |sum, row| {
            sum.checked_add(row.try_get::<i64, _>("qty")?)
                .ok_or_else(|| AppError::internal("active allocation quantity overflow"))
        })?;
    let prior_receipt_rows = sqlx::query(
        r#"SELECT detail.task_id,plan.planned_quantity
           FROM cross_dock_plan_runs plan JOIN cross_dock_tasks detail
             ON detail.tenant_id=plan.tenant_id AND detail.plan_run_id=plan.id
           JOIN work_tasks work ON work.tenant_id=detail.tenant_id AND work.id=detail.task_id
           WHERE plan.tenant_id=$1 AND plan.inventory_owner_id=$2
             AND plan.source_receipt_inventory_transaction_id=$3 AND work.status<>'cancelled'
           ORDER BY detail.task_id FOR UPDATE OF work,detail"#,
    )
    .bind(access.tenant_id.get())
    .bind(source.owner_id)
    .bind(command.source_receipt_inventory_transaction_id)
    .fetch_all(&mut *tx)
    .await?;
    let prior_receipt_cross_dock_quantity =
        prior_receipt_rows.iter().try_fold(0_i64, |sum, row| {
            sum.checked_add(row.try_get::<i64, _>("planned_quantity")?)
                .ok_or_else(|| AppError::internal("receipt cross-dock quantity overflow"))
        })?;
    let active_cross_dock_rows = sqlx::query(
        r#"SELECT detail.task_id,plan.planned_quantity
           FROM cross_dock_plan_runs plan JOIN cross_dock_tasks detail
             ON detail.tenant_id=plan.tenant_id AND detail.plan_run_id=plan.id
           WHERE plan.tenant_id=$1 AND plan.inventory_owner_id=$2
             AND plan.reservation_id=$3 AND detail.closed_at IS NULL
           ORDER BY detail.task_id FOR UPDATE OF detail"#,
    )
    .bind(access.tenant_id.get())
    .bind(source.owner_id)
    .bind(reservation_id)
    .fetch_all(&mut *tx)
    .await?;
    let active_cross_dock_quantity =
        active_cross_dock_rows.iter().try_fold(0_i64, |sum, row| {
            sum.checked_add(row.try_get::<i64, _>("planned_quantity")?)
                .ok_or_else(|| AppError::internal("active cross-dock quantity overflow"))
        })?;
    let balance = sqlx::query(
        r#"SELECT qty_on_hand-qty_reserved-qty_held AS free_quantity,location_id,
                  item_batch_id,item_id,uom,status,license_plate_id,deleted
           FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND facility_id=$3 AND id=$4 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(source.owner_id)
    .bind(source.facility_id)
    .bind(source.balance_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("received inventory is no longer available"))?;
    if balance.try_get::<String, _>("status")? != "available"
        || balance
            .try_get::<Option<i64>, _>("license_plate_id")?
            .is_some()
        || balance
            .try_get::<Option<wareboxes_domain::Timestamp>, _>("deleted")?
            .is_some()
    {
        return Err(AppError::conflict(
            "cross-dock source must be active loose available inventory",
        ));
    }
    let source_free_quantity: i64 = balance.try_get("free_quantity")?;
    let source_is_claimed: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM loose_inventory_movement_claims
             WHERE tenant_id=$1 AND inventory_owner_id=$2
               AND source_inventory_balance_id=$3 AND released_at IS NULL)"#,
    )
    .bind(access.tenant_id.get())
    .bind(source.owner_id)
    .bind(source.balance_id)
    .fetch_one(&mut *tx)
    .await?;
    if source_is_claimed {
        return Err(AppError::conflict(
            "received inventory already has active movement work",
        ));
    }
    require_destination_tx(
        &mut tx,
        access.tenant_id.get(),
        source.facility_id,
        command.destination_pick_face_location_id.get(),
    )
    .await?;
    let receipt_remaining = source
        .receipt_quantity
        .checked_sub(prior_receipt_cross_dock_quantity)
        .ok_or_else(|| AppError::conflict("received quantity is already committed"))?;
    let decision = plan_cross_dock(
        command.quantity,
        CrossDockPlanningSnapshot {
            order_status: OrderStatus::Open,
            reservation_quantity,
            allocated_quantity: active_allocation_quantity,
            active_cross_dock_quantity,
            source_free_quantity: source_free_quantity.min(receipt_remaining),
        },
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    let planned_at = now_iso();
    let resulting_revision = revision + 1;
    sqlx::query("UPDATE orders SET revision=$1 WHERE tenant_id=$2 AND inventory_owner_id=$3 AND id=$4 AND revision=$5")
        .bind(resulting_revision).bind(access.tenant_id.get()).bind(source.owner_id)
        .bind(command.order_id.get()).bind(revision).execute(&mut *tx).await?;
    let plan_id = CrossDockPlanId::new(sqlx::query_scalar(
        r#"INSERT INTO cross_dock_plan_runs
           (tenant_id,inventory_owner_id,facility_id,order_id,order_item_id,reservation_id,
            source_receipt_inventory_transaction_id,inbound_load_id,source_inventory_balance_id,
            source_location_id,destination_location_id,item_batch_id,item_id,uom,lot,serial,expiration,
            receipt_quantity,prior_receipt_cross_dock_quantity,active_cross_dock_quantity,
            source_free_quantity,reservation_quantity,active_allocation_quantity,
            unallocated_demand_quantity,planned_quantity,
            expected_order_revision,resulting_order_revision,planned_by_user_id,planned_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
             $18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29) RETURNING id"#,
    ).bind(access.tenant_id.get()).bind(source.owner_id).bind(source.facility_id)
      .bind(command.order_id.get()).bind(command.order_line_id.get()).bind(reservation_id)
      .bind(command.source_receipt_inventory_transaction_id).bind(source.inbound_load_id)
      .bind(source.balance_id).bind(source.location_id).bind(command.destination_pick_face_location_id.get())
      .bind(source.item_batch_id).bind(source.item_id).bind(&source.uom).bind(&source.lot)
      .bind(&source.serial).bind(source.expiration).bind(source.receipt_quantity)
      .bind(prior_receipt_cross_dock_quantity).bind(active_cross_dock_quantity)
      .bind(source_free_quantity).bind(reservation_quantity)
      .bind(active_allocation_quantity)
      .bind(reservation_quantity-active_allocation_quantity-active_cross_dock_quantity)
      .bind(command.quantity.get()).bind(revision).bind(resulting_revision)
      .bind(context.actor_id.get()).bind(planned_at).fetch_one(&mut *tx).await?)
      .map_err(|error| AppError::internal(error.to_string()))?;
    let work_task_id = insert_task_tx(
        &mut tx,
        access.tenant_id,
        NewWorkTask {
            facility_id: Some(source.facility_id),
            inventory_owner_id: Some(source.owner_id),
            task_type: WorkTaskType::CrossDock,
            title: format!("Cross-dock {}", row.try_get::<String, _>("order_key")?),
            instructions: command.instructions.clone(),
            required_permission: task_permission(WorkTaskType::CrossDock).to_owned(),
            priority: command.priority,
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::CrossDock),
            assigned_user_id: command.assigned_user_id.map(|id| id.get()),
            created_by: Some(context.actor_id.get()),
            scheduled_for: None,
            due_at: command.due_at,
            metadata_json: Some(
                serde_json::json!({"plan_id":plan_id.get(),"order_id":command.order_id.get()})
                    .to_string(),
            ),
        },
    )
    .await?;
    sqlx::query(
        r#"INSERT INTO cross_dock_tasks
           (tenant_id,task_id,plan_run_id,inventory_owner_id,facility_id,order_id,order_item_id,
            reservation_id,source_receipt_inventory_transaction_id,inbound_load_id,
            source_inventory_balance_id,source_location_id,destination_location_id,item_batch_id,
            item_id,uom,lot,serial,expiration,planned_quantity)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)"#,
    )
    .bind(access.tenant_id.get())
    .bind(work_task_id)
    .bind(plan_id.get())
    .bind(source.owner_id)
    .bind(source.facility_id)
    .bind(command.order_id.get())
    .bind(command.order_line_id.get())
    .bind(reservation_id)
    .bind(command.source_receipt_inventory_transaction_id)
    .bind(source.inbound_load_id)
    .bind(source.balance_id)
    .bind(source.location_id)
    .bind(command.destination_pick_face_location_id.get())
    .bind(source.item_batch_id)
    .bind(source.item_id)
    .bind(&source.uom)
    .bind(&source.lot)
    .bind(&source.serial)
    .bind(source.expiration)
    .bind(command.quantity.get())
    .execute(&mut *tx)
    .await?;
    let result = PlanCrossDockWorkResult {
        plan_id,
        work_id: CrossDockWorkId::new(work_task_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        order_id: command.order_id,
        order_line_id: command.order_line_id,
        reservation_id,
        previous_order_revision: OrderRevision::new(revision)
            .map_err(|e| AppError::internal(e.to_string()))?,
        order_revision: OrderRevision::new(resulting_revision)
            .map_err(|e| AppError::internal(e.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(source.owner_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        facility_id: FacilityId::new(source.facility_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        inbound_load_id: InboundLoadId::new(source.inbound_load_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        source_receipt_inventory_transaction_id: command.source_receipt_inventory_transaction_id,
        source_inventory_balance_id: InventoryBalanceId::new(source.balance_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        source_location_id: LocationId::new(source.location_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        destination_pick_face_location_id: command.destination_pick_face_location_id,
        item_batch_id: ItemBatchId::new(source.item_batch_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        item_id: CatalogItemId::new(source.item_id)
            .map_err(|e| AppError::internal(e.to_string()))?,
        uom: CrossDockUom::new(source.uom).map_err(|e| AppError::internal(e.to_string()))?,
        lot: source.lot,
        serial: source.serial,
        expiration: source.expiration,
        quantity: command.quantity,
        remaining_unallocated_quantity: decision.remaining_unallocated_quantity,
        status: CrossDockWorkStatus::Pending,
        planned_by: UserId::new(context.actor_id.get())
            .map_err(|e| AppError::internal(e.to_string()))?,
        planned_at,
    };
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        InventoryOwnerId::new(source.owner_id).map_err(|e| AppError::internal(e.to_string()))?,
        command.order_id.get(),
        Some(context.actor_id.get()),
        "cross_dock_planned",
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        result.inventory_owner_id,
        result.facility_id,
        context.actor_id.get(),
        command.order_id.get(),
        "inbound.cross_dock.planned",
        &format!("cross-dock-plan:{}", plan_id.get()),
        &serde_json::to_value(&result).map_err(|e| AppError::internal(e.to_string()))?,
        planned_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn source_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    transaction_id: i64,
) -> AppResult<SourceHint> {
    let row = sqlx::query(
        r#"SELECT transaction.inventory_owner_id,entry.facility_id,batch.load_id,
                  balance.id AS balance_id,entry.location_id,entry.item_batch_id,entry.item_id,
                  entry.uom,entry.lot,entry.serial,entry.expiration,entry.quantity_delta
           FROM inventory_transactions transaction
           JOIN inventory_entries entry ON entry.tenant_id=transaction.tenant_id
             AND entry.inventory_owner_id=transaction.inventory_owner_id
             AND entry.transaction_id=transaction.id AND entry.quantity_delta>0
           JOIN item_batches batch ON batch.tenant_id=entry.tenant_id
             AND batch.inventory_owner_id=entry.inventory_owner_id AND batch.id=entry.item_batch_id
           JOIN inventory_balances balance ON balance.tenant_id=entry.tenant_id
             AND balance.inventory_owner_id=entry.inventory_owner_id AND balance.facility_id=entry.facility_id
             AND balance.location_id=entry.location_id AND balance.item_batch_id=entry.item_batch_id
             AND balance.uom=entry.uom AND balance.status=entry.status AND balance.license_plate_id IS NULL
           WHERE transaction.tenant_id=$1 AND transaction.id=$2
             AND transaction.transaction_type='receive'
             AND transaction.operation='inbound.receive_expected_inventory.v1'
             AND transaction.reference_type='load_line'
             AND entry.status='available' AND batch.load_id IS NOT NULL
             AND (SELECT COUNT(*) FROM inventory_entries counted
                  WHERE counted.tenant_id=transaction.tenant_id
                    AND counted.inventory_owner_id=transaction.inventory_owner_id
                    AND counted.transaction_id=transaction.id)=1"#,
    ).bind(tenant_id).bind(transaction_id).fetch_optional(&mut **tx).await?
      .ok_or_else(|| AppError::not_found("eligible expected receipt inventory"))?;
    Ok(SourceHint {
        owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        inbound_load_id: row.try_get("load_id")?,
        balance_id: row.try_get("balance_id")?,
        location_id: row.try_get("location_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        receipt_quantity: row.try_get("quantity_delta")?,
    })
}

async fn require_destination_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    facility_id: i64,
    location_id: i64,
) -> AppResult<()> {
    let found: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM locations WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
             AND deleted IS NULL AND active AND pickable AND NOT receivable
             AND NULLIF(btrim(barcode),'') IS NOT NULL FOR SHARE)"#,
    )
    .bind(tenant_id)
    .bind(facility_id)
    .bind(location_id)
    .fetch_one(&mut **tx)
    .await?;
    if found {
        Ok(())
    } else {
        Err(AppError::conflict(
            "destination pick face is not executable",
        ))
    }
}
