use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::picking::{
    PickShortageHoldResult, PickShortageMovementResult, ReportPickShortageCommand,
    ReportPickShortageOutcome, ReportPickShortageResult, REPORT_PICK_SHORTAGE_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, TenantAccess};
use wareboxes_domain::{
    ActualPickQuantity, AllocationExecutionStage, InventoryAllocationId, InventoryBalanceId,
    InventoryHoldId, InventoryOwnerId, LicensePlateId, LocationId, OrderId, OrderRevision,
    OrderStatus, PickContentId, PickQuantity, PickShortageId, PickShortageQuantities,
    PickShortageReason, PickShortageRevision, PickShortageStatus, PickTaskId, TenantId, Timestamp,
    UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory_locking;
use crate::repo::orders::insert_order_activity_tx;

mod events;
mod movement;

pub(super) use events::enqueue_parent_shortage_transition_event_tx;
use events::{enqueue_shortage_event_tx, ParentShortageTransition};
use movement::{execute_partial_move_tx, short_source_allocation_tx};

#[derive(Debug)]
struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    status: OrderStatus,
    revision: OrderRevision,
}

#[derive(Debug)]
struct ShortageTarget {
    order_id: OrderId,
    order_item_id: i64,
    release_id: i64,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    reservation_id: i64,
    source_allocation_id: InventoryAllocationId,
    source_balance_id: InventoryBalanceId,
    source_location_id: LocationId,
    source_license_plate_id: Option<LicensePlateId>,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    inventory_status: InventoryStatus,
    planned_quantity: PickQuantity,
    destination_location_id: LocationId,
    lot: Option<String>,
    serial: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct PartialMovement {
    transaction_id: i64,
    confirmation_id: i64,
    destination_allocation_id: InventoryAllocationId,
    destination_balance_id: InventoryBalanceId,
    destination_plate_id: LicensePlateId,
    picked_quantity: PickQuantity,
}

pub async fn report_shortage(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReportPickShortageCommand,
) -> AppResult<ReportPickShortageResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, REPORT_PICK_SHORTAGE_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    require_stored_shortage_visible_before_replay_tx(
        &mut tx,
        access.tenant_id,
        prepared.idempotency_key(),
        &scope,
    )
    .await?;

    if let Some(result) = prepared
        .replayed::<ReportPickShortageResult>(&mut tx)
        .await?
    {
        require_replayed_shortage_visible_tx(&mut tx, access.tenant_id, result.shortage_id, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id = task_order_hint_tx(&mut tx, access.tenant_id, command.task_id, &scope).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    if order.status != OrderStatus::Processing {
        return Err(AppError::conflict("order is not in picking execution"));
    }
    lock_reservation_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        order_id,
        command.task_id,
        command.content_id,
    )
    .await?;
    lock_relevant_license_plates_tx(&mut tx, access.tenant_id, command, &scope).await?;
    let target = lock_target_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        &scope,
    )
    .await?;
    if target.order_id != order_id || target.inventory_owner_id != order.inventory_owner_id {
        return Err(AppError::internal(
            "pick shortage target does not match its order",
        ));
    }
    validate_report_evidence_tx(&mut tx, access.tenant_id, &target, command).await?;

    let actual_quantity = command.outcome.actual_quantity();
    let quantities = PickShortageQuantities::new(target.planned_quantity, actual_quantity)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let reported_at = now_iso();

    let movement = match &command.outcome {
        ReportPickShortageOutcome::NoPick => {
            short_source_allocation_tx(&mut tx, access.tenant_id, &target, reported_at).await?;
            None
        }
        ReportPickShortageOutcome::Partial {
            picked_quantity,
            destination_license_plate_barcode,
        } => {
            let movement = execute_partial_move_tx(
                &mut tx,
                access.tenant_id,
                context,
                &prepared,
                command,
                &target,
                *picked_quantity,
                destination_license_plate_barcode.as_str(),
                reported_at,
            )
            .await?;
            Some(movement)
        }
    };

    let hold_id = insert_discrepancy_hold_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        &target,
        quantities.short(),
        reported_at,
    )
    .await?;
    let resulting_revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let shortage_id = insert_shortage_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        &target,
        quantities,
        hold_id,
        movement,
        order.revision,
        resulting_revision,
        reported_at,
    )
    .await?;
    terminalize_pick_work_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        reported_at,
    )
    .await?;
    super::cluster::enqueue_terminal_event_for_task_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        command.task_id,
    )
    .await?;
    let parent_shortage_transition = advance_parent_shortage_for_terminal_work_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command.task_id,
        target.source_allocation_id,
        target.planned_quantity,
        reported_at,
    )
    .await?;
    advance_order_revision_tx(
        &mut tx,
        access.tenant_id,
        order_id,
        order.revision,
        resulting_revision,
    )
    .await?;

    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        target.inventory_owner_id,
        order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "reported pick shortage on task {} ({} picked, {} short)",
            command.task_id,
            quantities.picked().get(),
            quantities.short().get()
        ),
    )
    .await?;

    let result = ReportPickShortageResult {
        shortage_id,
        shortage_revision: PickShortageRevision::new(1)
            .map_err(|error| AppError::internal(error.to_string()))?,
        shortage_status: PickShortageStatus::AwaitingInventory,
        task_id: command.task_id,
        content_id: command.content_id,
        order_id,
        order_revision: resulting_revision,
        quantities,
        details: command.details.clone(),
        reallocated_quantity: ActualPickQuantity::ZERO,
        recovery_terminal_quantity: ActualPickQuantity::ZERO,
        remaining_to_allocate_quantity: ActualPickQuantity::new(quantities.short().get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        observed_item_barcode: command.observed_item_barcode.clone(),
        observed_lot: command.observed_lot.clone(),
        observed_serial: command.observed_serial.clone(),
        hold: PickShortageHoldResult {
            hold_id,
            inventory_balance_id: target.source_balance_id,
            held_quantity: quantities.short(),
        },
        movement: movement.map(|movement| PickShortageMovementResult {
            inventory_transaction_id: movement.transaction_id,
            source_inventory_allocation_id: target.source_allocation_id,
            destination_inventory_allocation_id: movement.destination_allocation_id,
            source_inventory_balance_id: target.source_balance_id,
            destination_inventory_balance_id: movement.destination_balance_id,
            source_location_id: target.source_location_id,
            destination_location_id: target.destination_location_id,
            source_license_plate_id: target.source_license_plate_id,
            destination_license_plate_id: movement.destination_plate_id,
            picked_quantity: movement.picked_quantity,
            destination_stage: AllocationExecutionStage::Staged,
        }),
        reported_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        reported_at,
    };
    enqueue_shortage_event_tx(
        &mut tx,
        access.tenant_id,
        target.inventory_owner_id,
        target.facility_id,
        &result,
    )
    .await?;
    if let Some(transition) = parent_shortage_transition.as_ref() {
        enqueue_parent_shortage_transition_event_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            resulting_revision,
            transition,
        )
        .await?;
    }
    Ok(prepared
        .commit_with_inventory_transaction(
            tx,
            result,
            movement.map(|movement| movement.transaction_id),
        )
        .await?)
}

async fn require_stored_shortage_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    idempotency_key: &str,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let shortage_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT (result_json->>'shortage_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = $2 AND idempotency_key = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(REPORT_PICK_SHORTAGE_OPERATION)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(shortage_id) = shortage_id {
        require_replayed_shortage_visible_tx(
            tx,
            tenant_id,
            PickShortageId::new(shortage_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            scope,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn advance_parent_shortage_for_terminal_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    task_id: PickTaskId,
    source_allocation_id: InventoryAllocationId,
    terminal_quantity: PickQuantity,
    occurred_at: Timestamp,
) -> AppResult<Option<ParentShortageTransition>> {
    let updated = sqlx::query(
        r#"
        UPDATE pick_shortages shortage
        SET recovery_terminal_qty = shortage.recovery_terminal_qty + $1,
            status = CASE
                WHEN shortage.recovery_terminal_qty + $1 = shortage.short_qty
                 AND shortage.reallocated_qty = shortage.short_qty
                    THEN 'resolved'
                WHEN shortage.recovery_terminal_qty + $1 = shortage.reallocated_qty
                 AND shortage.reallocated_qty < shortage.short_qty
                    THEN 'awaiting_inventory'
                ELSE 'recovery_in_progress'
            END,
            revision = shortage.revision + 1,
            modified_at = $2,
            resolved_by_user_id = CASE
                WHEN shortage.recovery_terminal_qty + $1 = shortage.short_qty
                 AND shortage.reallocated_qty = shortage.short_qty
                    THEN $3
                ELSE NULL
            END,
            resolved_at = CASE
                WHEN shortage.recovery_terminal_qty + $1 = shortage.short_qty
                 AND shortage.reallocated_qty = shortage.short_qty
                    THEN $2
                ELSE NULL
            END
        FROM order_release_allocations snapshot
        WHERE snapshot.tenant_id = $4
          AND snapshot.allocation_id = $5
          AND snapshot.source_kind = 'shortage_recovery'
          AND snapshot.pick_shortage_id = shortage.id
          AND shortage.tenant_id = snapshot.tenant_id
          AND shortage.inventory_owner_id = snapshot.inventory_owner_id
          AND shortage.facility_id = snapshot.facility_id
          AND shortage.order_release_id = snapshot.order_release_id
          AND shortage.order_id = snapshot.order_id
          AND shortage.order_item_id = snapshot.order_item_id
          AND shortage.reservation_id = snapshot.reservation_id
          AND shortage.status <> 'resolved'
          AND EXISTS (
              SELECT 1 FROM pick_tasks task
              WHERE task.tenant_id = snapshot.tenant_id
                AND task.id = $6
                AND task.source_allocation_id = snapshot.allocation_id
                AND task.status IN ('completed', 'shorted')
          )
        RETURNING shortage.id, shortage.inventory_owner_id, shortage.facility_id,
                  shortage.order_id, shortage.revision, shortage.status,
                  shortage.reallocated_qty, shortage.recovery_terminal_qty,
                  shortage.remaining_to_allocate_qty
        "#,
    )
    .bind(terminal_quantity.get())
    .bind(occurred_at)
    .bind(actor_user_id)
    .bind(tenant_id.get())
    .bind(source_allocation_id.get())
    .bind(task_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if updated.len() > 1 {
        return Err(AppError::internal(
            "recovery pick work matched multiple parent shortages",
        ));
    }
    updated
        .into_iter()
        .next()
        .map(|row| {
            let status = PickShortageStatus::parse(&row.try_get::<String, _>("status")?)
                .ok_or_else(|| AppError::internal("parent pick shortage has invalid status"))?;
            Ok(ParentShortageTransition {
                shortage_id: PickShortageId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                facility_id: row.try_get("facility_id")?,
                order_id: OrderId::new(row.try_get("order_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                revision: PickShortageRevision::new(row.try_get("revision")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                status,
                reallocated_quantity: ActualPickQuantity::new(row.try_get("reallocated_qty")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                recovery_terminal_quantity: ActualPickQuantity::new(
                    row.try_get("recovery_terminal_qty")?,
                )
                .map_err(|error| AppError::internal(error.to_string()))?,
                remaining_to_allocate_quantity: ActualPickQuantity::new(
                    row.try_get("remaining_to_allocate_qty")?,
                )
                .map_err(|error| AppError::internal(error.to_string()))?,
                trigger_task_id: task_id,
                trigger_source_allocation_id: source_allocation_id,
                terminal_quantity,
                occurred_at,
            })
        })
        .transpose()
}

async fn task_order_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: PickTaskId,
    scope: &ScopeBindings,
) -> AppResult<OrderId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        SELECT order_id
        FROM pick_tasks
        WHERE tenant_id = $1 AND id = $2
          AND ($3 OR inventory_owner_id = ANY($4))
          AND ($5 OR facility_id = ANY($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick task"))?;
    OrderId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, status, revision
        FROM orders
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
          AND ($3 OR inventory_owner_id = ANY($4))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    let status: String = row.try_get("status")?;
    Ok(LockedOrder {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: OrderStatus::parse(&status)
            .ok_or_else(|| AppError::internal("order has an invalid status"))?,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn lock_reservation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
    task_id: PickTaskId,
    content_id: PickContentId,
) -> AppResult<()> {
    let reservation_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT reservation.id
        FROM pick_tasks task
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        INNER JOIN inventory_reservations reservation
          ON reservation.tenant_id = content.tenant_id
         AND reservation.inventory_owner_id = content.inventory_owner_id
         AND reservation.id = content.reservation_id
        WHERE task.tenant_id = $1 AND task.inventory_owner_id = $2
          AND task.order_id = $3 AND task.id = $4 AND content.id = $5
          AND reservation.status = 'active' AND reservation.deleted IS NULL
        FOR UPDATE OF reservation
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .bind(task_id.get())
    .bind(content_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    reservation_id
        .map(|_| ())
        .ok_or_else(|| AppError::conflict("pick reservation is no longer active"))
}

async fn lock_relevant_license_plates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &ReportPickShortageCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let destination_barcode = match &command.outcome {
        ReportPickShortageOutcome::NoPick => None,
        ReportPickShortageOutcome::Partial {
            destination_license_plate_barcode,
            ..
        } => Some(destination_license_plate_barcode.as_str()),
    };
    let row = sqlx::query(
        r#"
        SELECT content.source_license_plate_id,
               (
                   SELECT plate.id FROM license_plates plate
                   WHERE plate.tenant_id = task.tenant_id
                     AND plate.inventory_owner_id = task.inventory_owner_id
                     AND plate.facility_id = task.facility_id
                     AND plate.barcode = $4 AND plate.deleted IS NULL
               ) AS destination_license_plate_id
        FROM pick_tasks task
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        WHERE task.tenant_id = $1 AND task.id = $2 AND content.id = $3
          AND ($5 OR task.facility_id = ANY($6))
          AND ($7 OR task.inventory_owner_id = ANY($8))
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.task_id.get())
    .bind(command.content_id.get())
    .bind(destination_barcode)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick task"))?;
    let mut ids = Vec::with_capacity(2);
    if let Some(id) = row.try_get::<Option<i64>, _>("source_license_plate_id")? {
        ids.push(id);
    }
    if let Some(id) = row.try_get::<Option<i64>, _>("destination_license_plate_id")? {
        ids.push(id);
    }
    inventory_locking::lock_license_plates(tx, tenant_id, ids).await
}

async fn lock_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ReportPickShortageCommand,
    scope: &ScopeBindings,
) -> AppResult<ShortageTarget> {
    let row = sqlx::query(
        r#"
        SELECT task.order_id, task.order_release_id, task.inventory_owner_id,
               task.facility_id, task.destination_location_id, task.lease_expires_at,
               content.order_item_id, content.reservation_id,
               content.source_allocation_id, content.source_inventory_balance_id,
               content.source_location_id, content.source_license_plate_id,
               content.item_batch_id, content.item_id, content.uom,
               content.inventory_status, content.planned_qty, content.state,
               allocation.inventory_balance_id AS allocation_balance_id,
               allocation.location_id AS allocation_location_id,
               allocation.license_plate_id AS allocation_plate_id,
               allocation.item_batch_id AS allocation_batch_id,
               allocation.item_id AS allocation_item_id,
               allocation.uom AS allocation_uom,
               allocation.inventory_status AS allocation_status,
               allocation.qty AS allocation_qty,
               allocation.status AS allocation_lifecycle,
               allocation.execution_stage,
               allocation.deleted AS allocation_deleted,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_plate_id,
               balance.item_batch_id AS balance_batch_id,
               balance.item_id AS balance_item_id, balance.uom AS balance_uom,
               balance.status AS balance_status, balance.qty_on_hand,
               balance.qty_reserved, balance.deleted AS balance_deleted,
               batch.lot, batch.serial
        FROM pick_tasks task
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id = content.tenant_id
         AND allocation.inventory_owner_id = content.inventory_owner_id
         AND allocation.id = content.source_allocation_id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = content.tenant_id
         AND balance.inventory_owner_id = content.inventory_owner_id
         AND balance.facility_id = content.facility_id
         AND balance.id = content.source_inventory_balance_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = content.tenant_id
         AND batch.inventory_owner_id = content.inventory_owner_id
         AND batch.id = content.item_batch_id
        WHERE task.tenant_id = $1 AND task.id = $2 AND content.id = $3
          AND task.status = 'in_progress' AND task.assigned_user_id = $4
        FOR UPDATE OF task, content, allocation, balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.task_id.get())
    .bind(command.content_id.get())
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("pick claim is not active for this content"))?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    if !scope.includes_facility(facility_id) || !scope.includes_inventory_owner(owner_id) {
        return Err(AppError::not_found("pick task"));
    }
    if row.try_get::<Timestamp, _>("lease_expires_at")? <= now_iso() {
        return Err(AppError::conflict("pick claim has expired"));
    }
    let target = ShortageTarget {
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_item_id: row.try_get("order_item_id")?,
        release_id: row.try_get("order_release_id")?,
        inventory_owner_id: InventoryOwnerId::new(owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id,
        reservation_id: row.try_get("reservation_id")?,
        source_allocation_id: InventoryAllocationId::new(row.try_get("source_allocation_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_balance_id: InventoryBalanceId::new(row.try_get("source_inventory_balance_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_id: LocationId::new(row.try_get("source_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_license_plate_id: row
            .try_get::<Option<i64>, _>("source_license_plate_id")?
            .map(LicensePlateId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        inventory_status: InventoryStatus::parse(&row.try_get::<String, _>("inventory_status")?)
            .ok_or_else(|| AppError::internal("pick content has invalid inventory status"))?,
        planned_quantity: PickQuantity::new(row.try_get("planned_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
    };
    let quantity = target.planned_quantity.get();
    let valid = row.try_get::<String, _>("state")? == "pending"
        && row.try_get::<i64, _>("allocation_balance_id")? == target.source_balance_id.get()
        && row.try_get::<i64, _>("allocation_location_id")? == target.source_location_id.get()
        && row.try_get::<Option<i64>, _>("allocation_plate_id")?
            == target.source_license_plate_id.map(|id| id.get())
        && row.try_get::<i64, _>("allocation_batch_id")? == target.item_batch_id
        && row.try_get::<i64, _>("allocation_item_id")? == target.item_id
        && row.try_get::<String, _>("allocation_uom")? == target.uom
        && row.try_get::<String, _>("allocation_status")? == target.inventory_status.as_str()
        && row.try_get::<i64, _>("allocation_qty")? == quantity
        && row.try_get::<String, _>("allocation_lifecycle")? == "allocated"
        && row.try_get::<String, _>("execution_stage")? == "pick_source"
        && row
            .try_get::<Option<Timestamp>, _>("allocation_deleted")?
            .is_none()
        && row.try_get::<i64, _>("balance_location_id")? == target.source_location_id.get()
        && row.try_get::<Option<i64>, _>("balance_plate_id")?
            == target.source_license_plate_id.map(|id| id.get())
        && row.try_get::<i64, _>("balance_batch_id")? == target.item_batch_id
        && row.try_get::<i64, _>("balance_item_id")? == target.item_id
        && row.try_get::<String, _>("balance_uom")? == target.uom
        && row.try_get::<String, _>("balance_status")? == target.inventory_status.as_str()
        && row.try_get::<i64, _>("qty_on_hand")? >= quantity
        && row.try_get::<i64, _>("qty_reserved")? >= quantity
        && row
            .try_get::<Option<Timestamp>, _>("balance_deleted")?
            .is_none();
    if !valid {
        return Err(AppError::conflict(
            "allocated source stock changed before shortage report",
        ));
    }
    Ok(target)
}

async fn validate_report_evidence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ShortageTarget,
    command: &ReportPickShortageCommand,
) -> AppResult<()> {
    let source_barcode: Option<String> = sqlx::query_scalar(
        r#"
        SELECT barcode FROM locations
        WHERE tenant_id = $1 AND facility_id = $2 AND id = $3
          AND deleted IS NULL AND active AND pickable
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.facility_id)
    .bind(target.source_location_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let source_barcode = source_barcode.ok_or_else(|| {
        AppError::conflict("directed source location is no longer available for picking")
    })?;
    if source_barcode != command.source_location_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned source location does not match the directed pick",
        ));
    }
    validate_source_plate_tx(tx, tenant_id, target, command).await?;

    let observed_item_matches = match command.observed_item_barcode.as_ref() {
        Some(barcode) => {
            sqlx::query_scalar(
                r#"
            SELECT EXISTS (
                SELECT 1 FROM barcodes
                WHERE tenant_id = $1 AND item_id = $2
                  AND deleted IS NULL AND name = $3
            )
            "#,
            )
            .bind(tenant_id.get())
            .bind(target.item_id)
            .bind(barcode.as_str())
            .fetch_one(&mut **tx)
            .await?
        }
        None => false,
    };
    let lot_matches = command
        .observed_lot
        .as_ref()
        .map(|scan| Some(scan.as_str()) == target.lot.as_deref())
        .unwrap_or(target.lot.is_none());
    let serial_matches = command
        .observed_serial
        .as_ref()
        .map(|scan| Some(scan.as_str()) == target.serial.as_deref())
        .unwrap_or(target.serial.is_none());

    match command.details.reason() {
        PickShortageReason::InventoryMissing => {
            if command.observed_item_barcode.is_some()
                || command.observed_lot.is_some()
                || command.observed_serial.is_some()
            {
                return Err(AppError::bad_request(
                    "missing inventory cannot include observed stock identity",
                ));
            }
        }
        PickShortageReason::WrongInventory => {
            if command.observed_item_barcode.is_none() || observed_item_matches {
                return Err(AppError::bad_request(
                    "wrong inventory requires a scanned item that differs from the directed item",
                ));
            }
        }
        PickShortageReason::LotOrSerialMismatch => {
            if !observed_item_matches
                || (target.lot.is_none() && target.serial.is_none())
                || (lot_matches && serial_matches)
            {
                return Err(AppError::bad_request(
                    "lot or serial mismatch requires matching item evidence and a mismatched controlled identity",
                ));
            }
        }
        PickShortageReason::InsufficientQuantity | PickShortageReason::DamagedInventory => {
            if !observed_item_matches || !lot_matches || !serial_matches {
                return Err(AppError::bad_request(
                    "shortage evidence does not match the directed inventory",
                ));
            }
        }
        PickShortageReason::Other => {}
    }

    if matches!(command.outcome, ReportPickShortageOutcome::Partial { .. })
        && (!matches!(
            command.details.reason(),
            PickShortageReason::InsufficientQuantity
                | PickShortageReason::DamagedInventory
                | PickShortageReason::Other
        ) || !observed_item_matches
            || !lot_matches
            || !serial_matches)
    {
        return Err(AppError::bad_request(
            "partial pick requires matching stock identity and a quantity or damage reason",
        ));
    }
    Ok(())
}

async fn validate_source_plate_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ShortageTarget,
    command: &ReportPickShortageCommand,
) -> AppResult<()> {
    match (
        target.source_license_plate_id,
        command.source_license_plate_barcode.as_ref(),
    ) {
        (None, None) => Ok(()),
        (Some(plate_id), Some(scanned)) => {
            let barcode: Option<String> = sqlx::query_scalar(
                r#"
                SELECT barcode FROM license_plates
                WHERE tenant_id = $1 AND inventory_owner_id = $2 AND facility_id = $3
                  AND id = $4 AND location_id = $5 AND deleted IS NULL
                FOR UPDATE
                "#,
            )
            .bind(tenant_id.get())
            .bind(target.inventory_owner_id.get())
            .bind(target.facility_id)
            .bind(plate_id.get())
            .bind(target.source_location_id.get())
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
            match barcode {
                Some(barcode) if barcode == scanned.as_str() => Ok(()),
                Some(_) => Err(AppError::bad_request(
                    "scanned source license plate does not match the directed pick",
                )),
                None => Err(AppError::conflict(
                    "directed source license plate is no longer available",
                )),
            }
        }
        _ => Err(AppError::bad_request(
            "source license plate scan does not match the directed pick",
        )),
    }
}

async fn insert_discrepancy_hold_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ReportPickShortageCommand,
    target: &ShortageTarget,
    short_quantity: PickQuantity,
    occurred_at: Timestamp,
) -> AppResult<InventoryHoldId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_holds (
            tenant_id, inventory_owner_id, created, modified, created_by,
            inventory_balance_id, facility_id, location_id, license_plate_id,
            item_batch_id, item_id, uom, inventory_status, qty, reason_code,
            note, reference_type, reference_id, status
        ) VALUES (
            $1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            $13, 'inventory_discrepancy', $14, 'pick_shortage_source', $15, 'active'
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(occurred_at)
    .bind(actor_user_id)
    .bind(target.source_balance_id.get())
    .bind(target.facility_id)
    .bind(target.source_location_id.get())
    .bind(target.source_license_plate_id.map(|id| id.get()))
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(short_quantity.get())
    .bind(command.details.note().map(|note| note.as_str()))
    .bind(command.content_id.get())
    .fetch_one(&mut **tx)
    .await?;
    InventoryHoldId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_shortage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ReportPickShortageCommand,
    target: &ShortageTarget,
    quantities: PickShortageQuantities,
    hold_id: InventoryHoldId,
    movement: Option<PartialMovement>,
    previous_revision: OrderRevision,
    resulting_revision: OrderRevision,
    occurred_at: Timestamp,
) -> AppResult<PickShortageId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO pick_shortages (
            tenant_id, inventory_owner_id, facility_id, order_release_id,
            order_id, order_item_id, reservation_id, task_id,
            pick_task_content_id, source_inventory_allocation_id,
            source_inventory_balance_id, source_location_id,
            source_license_plate_id, destination_location_id, item_batch_id,
            item_id, uom, inventory_status, planned_qty, picked_qty, short_qty,
            reason_code, note, observed_item_barcode, observed_lot,
            observed_serial, inventory_hold_id, pick_confirmation_id,
            inventory_transaction_id, destination_inventory_allocation_id,
            destination_inventory_balance_id, destination_license_plate_id,
            reported_by_user_id, reported_at, report_previous_order_revision,
            report_resulting_order_revision, modified_at, revision, status,
            reallocated_qty, recovery_terminal_qty, remaining_to_allocate_qty
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
            $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25,
            $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $34,
            1, 'awaiting_inventory', 0, 0, $21
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.release_id)
    .bind(target.order_id.get())
    .bind(target.order_item_id)
    .bind(target.reservation_id)
    .bind(command.task_id.get())
    .bind(command.content_id.get())
    .bind(target.source_allocation_id.get())
    .bind(target.source_balance_id.get())
    .bind(target.source_location_id.get())
    .bind(target.source_license_plate_id.map(|id| id.get()))
    .bind(target.destination_location_id.get())
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(quantities.planned().get())
    .bind(quantities.picked().get())
    .bind(quantities.short().get())
    .bind(command.details.reason().as_str())
    .bind(command.details.note().map(|note| note.as_str()))
    .bind(
        command
            .observed_item_barcode
            .as_ref()
            .map(|value| value.as_str()),
    )
    .bind(command.observed_lot.as_ref().map(|value| value.as_str()))
    .bind(command.observed_serial.as_ref().map(|value| value.as_str()))
    .bind(hold_id.get())
    .bind(movement.map(|value| value.confirmation_id))
    .bind(movement.map(|value| value.transaction_id))
    .bind(movement.map(|value| value.destination_allocation_id.get()))
    .bind(movement.map(|value| value.destination_balance_id.get()))
    .bind(movement.map(|value| value.destination_plate_id.get()))
    .bind(actor_user_id)
    .bind(occurred_at)
    .bind(previous_revision.get())
    .bind(resulting_revision.get())
    .fetch_one(&mut **tx)
    .await?;
    PickShortageId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn terminalize_pick_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ReportPickShortageCommand,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let content = sqlx::query(
        r#"
        UPDATE pick_task_contents SET state = 'shorted', completed_at = $1
        WHERE tenant_id = $2 AND id = $3 AND task_id = $4 AND state = 'pending'
        "#,
    )
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(command.content_id.get())
    .bind(command.task_id.get())
    .execute(&mut **tx)
    .await?;
    if content.rows_affected() != 1 {
        return Err(AppError::conflict("pick content changed during short pick"));
    }
    let task = sqlx::query(
        r#"
        UPDATE pick_tasks SET status = 'shorted', completed_at = $1,
            lease_expires_at = NULL
        WHERE tenant_id = $2 AND id = $3 AND status = 'in_progress'
          AND assigned_user_id = $4
        "#,
    )
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(command.task_id.get())
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    if task.rows_affected() != 1 {
        return Err(AppError::conflict("pick task changed during short pick"));
    }
    Ok(())
}

async fn advance_order_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    expected: OrderRevision,
    resulting: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders SET revision = $1
        WHERE tenant_id = $2 AND id = $3
          AND status = 'processing' AND revision = $4 AND deleted IS NULL
        "#,
    )
    .bind(resulting.get())
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(expected.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed during short pick"));
    }
    Ok(())
}

async fn require_replayed_shortage_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage_id: PickShortageId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT inventory_owner_id, facility_id FROM pick_shortages WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(shortage_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick shortage"))?;
    if !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
        || !scope.includes_facility(row.try_get("facility_id")?)
    {
        return Err(AppError::not_found("pick shortage"));
    }
    Ok(())
}
