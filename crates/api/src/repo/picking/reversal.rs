use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::picking::{
    ReversePickConfirmationCommand, ReversePickConfirmationResult,
    REVERSE_PICK_CONFIRMATION_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    reverse_pick_before_packing, InventoryAllocationId, InventoryBalanceId, InventoryOwnerId,
    LicensePlateId, LocationId, OrderId, OrderRevision, OrderStatus, PickConfirmationId,
    PickContentId, PickContentState, PickQuantity, PickReversalId, PickTaskId, TenantId, Timestamp,
    UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::inventory_locking;
use crate::repo::orders::insert_order_activity_tx;

mod history;

pub use history::list_confirmation_history;

#[derive(Debug)]
struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    status: OrderStatus,
    revision: OrderRevision,
}

#[derive(Debug)]
struct ReversalTarget {
    confirmation_id: PickConfirmationId,
    task_id: PickTaskId,
    content_id: PickContentId,
    order_id: OrderId,
    order_release_id: i64,
    order_item_id: i64,
    reservation_id: i64,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    source_allocation_id: InventoryAllocationId,
    staged_allocation_id: InventoryAllocationId,
    source_balance_id: InventoryBalanceId,
    staged_balance_id: InventoryBalanceId,
    source_location_id: LocationId,
    staged_location_id: LocationId,
    source_license_plate_id: Option<LicensePlateId>,
    staged_license_plate_id: LicensePlateId,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    inventory_status: InventoryStatus,
    quantity: PickQuantity,
    lot: Option<String>,
    serial: Option<String>,
    source_location_barcode: String,
    staged_location_barcode: String,
    source_license_plate_barcode: Option<String>,
    staged_license_plate_barcode: String,
}

pub async fn reverse_confirmation(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReversePickConfirmationCommand,
) -> AppResult<ReversePickConfirmationResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    command
        .validate_details()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let prepared = PreparedCommand::new_v1(context, REVERSE_PICK_CONFIRMATION_OPERATION, command)?;
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

    require_stored_reversal_visible_before_replay_tx(
        &mut tx,
        access.tenant_id,
        prepared.idempotency_key(),
        &scope,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<ReversePickConfirmationResult>(&mut tx)
        .await?
    {
        require_reversal_visible_tx(&mut tx, access.tenant_id, result.reversal_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id =
        confirmation_order_hint_tx(&mut tx, access.tenant_id, command.confirmation_id, &scope)
            .await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    if order.revision != command.expected_order_revision {
        return Err(AppError::conflict("order revision is stale"));
    }
    let has_downstream_execution = has_packing_execution_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        order_id,
    )
    .await?;
    let shortage_backed = is_shortage_backed_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.confirmation_id,
    )
    .await?;
    reverse_pick_before_packing(order.status, has_downstream_execution, shortage_backed)
        .map_err(|error| AppError::conflict(error.to_string()))?;

    let reservation_id = confirmation_reservation_hint_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.confirmation_id,
    )
    .await?;
    lock_reservation_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        order_id,
        reservation_id,
    )
    .await?;
    lock_confirmation_plates_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.confirmation_id,
        &scope,
    )
    .await?;
    let target = lock_target_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.confirmation_id,
        &scope,
    )
    .await?;
    if target.order_id != order_id || target.reservation_id != reservation_id {
        return Err(AppError::internal(
            "pick confirmation does not match its locked order and reservation",
        ));
    }
    validate_scans_tx(&mut tx, access.tenant_id, &target, command).await?;

    let reversed_at = now_iso();
    let resulting_revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let owner_facility = inventory_journal::owner_facility_scope(
        target.inventory_owner_id.get(),
        target.facility_id,
    )?;
    let transaction_id = inventory_journal::begin_batched_transaction_at(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("pick confirmation reversal"),
            reference_type: Some("pick_confirmation"),
            reference_id: Some(target.confirmation_id.get()),
            correlation_id: Some(&context.request_id),
            operation: REVERSE_PICK_CONFIRMATION_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
        reversed_at,
    )
    .await?;
    let reversal_id = insert_reversal_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        &target,
        transaction_id,
        order.revision,
        resulting_revision,
        reversed_at,
    )
    .await?;

    release_staged_allocation_tx(&mut tx, access.tenant_id, &target, reversed_at).await?;
    decrement_staged_balance_tx(&mut tx, access.tenant_id, &target, reversed_at).await?;
    if target.source_license_plate_id == Some(target.staged_license_plate_id) {
        return_full_pallet_header_tx(&mut tx, access.tenant_id, &target).await?;
    }
    increment_source_balance_tx(&mut tx, access.tenant_id, &target, reversed_at).await?;
    reactivate_source_allocation_tx(&mut tx, access.tenant_id, &target).await?;

    for (location_id, license_plate_id, quantity_delta) in [
        (
            target.source_location_id,
            target.source_license_plate_id,
            target.quantity.get(),
        ),
        (
            target.staged_location_id,
            Some(target.staged_license_plate_id),
            -target.quantity.get(),
        ),
    ] {
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: location_id.get(),
                license_plate_id: license_plate_id.map(|id| id.get()),
                item_batch_id: target.item_batch_id,
                status: target.inventory_status,
                quantity_delta,
            },
        )
        .await?;
    }

    reopen_pick_work_tx(&mut tx, access.tenant_id, &target).await?;
    regress_order_to_processing_tx(
        &mut tx,
        access.tenant_id,
        order_id,
        order.status,
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
            "reversed pick confirmation {} ({} units; {})",
            target.confirmation_id,
            target.quantity.get(),
            command.reason.as_str()
        ),
    )
    .await?;

    let result = ReversePickConfirmationResult {
        reversal_id,
        confirmation_id: target.confirmation_id,
        task_id: target.task_id,
        content_id: target.content_id,
        order_id,
        inventory_transaction_id: transaction_id,
        source_inventory_allocation_id: target.source_allocation_id,
        staged_inventory_allocation_id: target.staged_allocation_id,
        source_inventory_balance_id: target.source_balance_id,
        staged_inventory_balance_id: target.staged_balance_id,
        source_location_id: target.source_location_id,
        staged_location_id: target.staged_location_id,
        source_license_plate_id: target.source_license_plate_id,
        staged_license_plate_id: target.staged_license_plate_id,
        reversed_quantity: target.quantity,
        content_state: PickContentState::Pending,
        order_status: OrderStatus::Processing,
        order_revision: resulting_revision,
        reason: command.reason,
        note: command.note.clone(),
        reversed_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        reversed_at,
    };
    enqueue_reversal_event_tx(
        &mut tx,
        access.tenant_id,
        target.inventory_owner_id,
        target.facility_id,
        &result,
    )
    .await?;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await?)
}

async fn confirmation_order_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    confirmation_id: PickConfirmationId,
    scope: &ScopeBindings,
) -> AppResult<OrderId> {
    let order_id: i64 = sqlx::query_scalar(
        r#"
        SELECT order_id FROM pick_confirmations
        WHERE tenant_id = $1 AND id = $2
          AND ($3 OR inventory_owner_id = ANY($4))
          AND ($5 OR facility_id = ANY($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(confirmation_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick confirmation"))?;
    OrderId::new(order_id).map_err(|error| AppError::internal(error.to_string()))
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
    Ok(LockedOrder {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: OrderStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or_else(|| AppError::internal("order has invalid status"))?,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn has_packing_execution_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM packing_sessions
            WHERE tenant_id = $1 AND inventory_owner_id = $2 AND order_id = $3
              AND state <> 'abandoned'
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(order_id.get())
    .fetch_one(&mut **tx)
    .await?)
}

async fn is_shortage_backed_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    confirmation_id: PickConfirmationId,
) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM pick_shortages shortage
            INNER JOIN pick_confirmations confirmation
              ON confirmation.tenant_id = shortage.tenant_id
             AND confirmation.inventory_owner_id = shortage.inventory_owner_id
             AND confirmation.id = $3
            WHERE shortage.tenant_id = $1
              AND shortage.inventory_owner_id = $2
              AND (
                  shortage.pick_confirmation_id = confirmation.id
                  OR shortage.pick_task_content_id = confirmation.pick_task_content_id
              )
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(confirmation_id.get())
    .fetch_one(&mut **tx)
    .await?)
}

async fn confirmation_reservation_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    confirmation_id: PickConfirmationId,
) -> AppResult<i64> {
    sqlx::query_scalar(
        r#"
        SELECT reservation_id FROM pick_confirmations
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(confirmation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick confirmation"))
}

async fn lock_reservation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    order_id: OrderId,
    reservation_id: i64,
) -> AppResult<()> {
    let locked: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM inventory_reservations
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND order_id = $3 AND id = $4
          AND status = 'active' AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(order_id.get())
    .bind(reservation_id)
    .fetch_optional(&mut **tx)
    .await?;
    locked
        .map(|_| ())
        .ok_or_else(|| AppError::conflict("pick reservation is no longer active"))
}

async fn lock_confirmation_plates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    confirmation_id: PickConfirmationId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT facility_id, source_license_plate_id,
               destination_license_plate_id
        FROM pick_confirmations
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(confirmation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick confirmation"))?;
    if !scope.includes_facility(row.try_get("facility_id")?) {
        return Err(AppError::not_found("pick confirmation"));
    }
    let mut ids = Vec::with_capacity(2);
    if let Some(id) = row.try_get::<Option<i64>, _>("source_license_plate_id")? {
        ids.push(id);
    }
    ids.push(row.try_get("destination_license_plate_id")?);
    inventory_locking::lock_license_plates(tx, tenant_id, ids).await
}

async fn lock_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    confirmation_id: PickConfirmationId,
    scope: &ScopeBindings,
) -> AppResult<ReversalTarget> {
    let row = sqlx::query(
        r#"
        SELECT confirmation.task_id, confirmation.pick_task_content_id,
               confirmation.order_release_id, confirmation.order_id,
               confirmation.order_item_id, confirmation.reservation_id,
               confirmation.inventory_owner_id, confirmation.facility_id,
               confirmation.source_inventory_allocation_id,
               staged_allocation.id AS destination_inventory_allocation_id,
               confirmation.source_inventory_balance_id,
               staged_balance.id AS destination_inventory_balance_id,
               confirmation.source_location_id,
               staged_allocation.location_id AS destination_location_id,
               confirmation.source_license_plate_id,
               staged_allocation.license_plate_id AS destination_license_plate_id,
               confirmation.item_batch_id, confirmation.item_id,
               confirmation.uom, confirmation.inventory_status,
               confirmation.picked_qty, confirmation.confirmed_at,
               task.status AS task_status, task.completed_at AS task_completed_at,
               content.state AS content_state,
               content.completed_at AS content_completed_at,
               content.planned_qty,
               source_allocation.status AS source_allocation_status,
               source_allocation.deleted AS source_allocation_deleted,
               source_allocation.execution_stage AS source_execution_stage,
               staged_allocation.status AS staged_allocation_status,
               staged_allocation.deleted AS staged_allocation_deleted,
               staged_allocation.execution_stage AS staged_execution_stage,
               staged_balance.qty_on_hand AS staged_on_hand,
               staged_balance.qty_reserved AS staged_reserved,
               batch.lot, batch.serial,
               source_location.barcode AS source_location_barcode,
               staged_location.barcode AS staged_location_barcode,
               source_plate.barcode AS source_license_plate_barcode,
               staged_plate.barcode AS staged_license_plate_barcode
        FROM pick_confirmations confirmation
        INNER JOIN pick_tasks task
          ON task.tenant_id = confirmation.tenant_id
         AND task.inventory_owner_id = confirmation.inventory_owner_id
         AND task.facility_id = confirmation.facility_id
         AND task.id = confirmation.task_id
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id
         AND content.inventory_owner_id = task.inventory_owner_id
         AND content.facility_id = task.facility_id
         AND content.task_id = task.id
         AND content.id = confirmation.pick_task_content_id
        INNER JOIN inventory_allocations source_allocation
          ON source_allocation.tenant_id = confirmation.tenant_id
         AND source_allocation.inventory_owner_id = confirmation.inventory_owner_id
         AND source_allocation.id = confirmation.source_inventory_allocation_id
        LEFT JOIN packing_session_allocations pack_snapshot
          ON pack_snapshot.tenant_id = confirmation.tenant_id
         AND pack_snapshot.inventory_owner_id = confirmation.inventory_owner_id
         AND pack_snapshot.facility_id = confirmation.facility_id
         AND pack_snapshot.pick_confirmation_id = confirmation.id
        LEFT JOIN packing_sessions pack_session
          ON pack_session.tenant_id = pack_snapshot.tenant_id
         AND pack_session.inventory_owner_id = pack_snapshot.inventory_owner_id
         AND pack_session.facility_id = pack_snapshot.facility_id
         AND pack_session.id = pack_snapshot.packing_session_id
         AND pack_session.state = 'abandoned'
        LEFT JOIN packing_allocation_positions pack_position
          ON pack_position.tenant_id = pack_snapshot.tenant_id
         AND pack_position.inventory_owner_id = pack_snapshot.inventory_owner_id
         AND pack_position.facility_id = pack_snapshot.facility_id
         AND pack_position.packing_session_id = pack_snapshot.packing_session_id
         AND pack_position.packing_session_allocation_id = pack_snapshot.id
         AND pack_position.state = 'available'
         AND pack_session.id IS NOT NULL
        INNER JOIN inventory_allocations staged_allocation
          ON staged_allocation.tenant_id = confirmation.tenant_id
         AND staged_allocation.inventory_owner_id = confirmation.inventory_owner_id
         AND staged_allocation.id = COALESCE(
             pack_position.current_inventory_allocation_id,
             confirmation.destination_inventory_allocation_id
         )
        INNER JOIN inventory_balances source_balance
          ON source_balance.tenant_id = confirmation.tenant_id
         AND source_balance.inventory_owner_id = confirmation.inventory_owner_id
         AND source_balance.id = confirmation.source_inventory_balance_id
        INNER JOIN inventory_balances staged_balance
          ON staged_balance.tenant_id = confirmation.tenant_id
         AND staged_balance.inventory_owner_id = confirmation.inventory_owner_id
         AND staged_balance.id = staged_allocation.inventory_balance_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = confirmation.tenant_id
         AND batch.inventory_owner_id = confirmation.inventory_owner_id
         AND batch.id = confirmation.item_batch_id
        INNER JOIN locations source_location
          ON source_location.tenant_id = confirmation.tenant_id
         AND source_location.facility_id = confirmation.facility_id
         AND source_location.id = confirmation.source_location_id
        INNER JOIN locations staged_location
          ON staged_location.tenant_id = confirmation.tenant_id
         AND staged_location.facility_id = confirmation.facility_id
         AND staged_location.id = staged_allocation.location_id
        LEFT JOIN license_plates source_plate
          ON source_plate.tenant_id = confirmation.tenant_id
         AND source_plate.inventory_owner_id = confirmation.inventory_owner_id
         AND source_plate.facility_id = confirmation.facility_id
         AND source_plate.id = confirmation.source_license_plate_id
        INNER JOIN license_plates staged_plate
          ON staged_plate.tenant_id = confirmation.tenant_id
         AND staged_plate.inventory_owner_id = confirmation.inventory_owner_id
         AND staged_plate.facility_id = confirmation.facility_id
         AND staged_plate.id = staged_allocation.license_plate_id
        WHERE confirmation.tenant_id = $1
          AND confirmation.inventory_owner_id = $2
          AND confirmation.id = $3
        FOR UPDATE OF task, content, source_allocation, staged_allocation,
                      source_balance, staged_balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(confirmation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick confirmation"))?;
    let facility_id: i64 = row.try_get("facility_id")?;
    if !scope.includes_inventory_owner(owner_id.get()) || !scope.includes_facility(facility_id) {
        return Err(AppError::not_found("pick confirmation"));
    }
    if row.try_get::<String, _>("task_status")? != "completed"
        || row.try_get::<String, _>("content_state")? != "completed"
        || row.try_get::<Timestamp, _>("task_completed_at")?
            != row.try_get::<Timestamp, _>("confirmed_at")?
        || row.try_get::<Timestamp, _>("content_completed_at")?
            != row.try_get::<Timestamp, _>("confirmed_at")?
        || row.try_get::<i64, _>("planned_qty")? != row.try_get::<i64, _>("picked_qty")?
        || row.try_get::<String, _>("source_allocation_status")? != "fulfilled"
        || row.try_get::<String, _>("source_execution_stage")? != "pick_source"
        || row.try_get::<Option<Timestamp>, _>("source_allocation_deleted")?
            != Some(row.try_get("confirmed_at")?)
        || row.try_get::<String, _>("staged_allocation_status")? != "allocated"
        || row.try_get::<String, _>("staged_execution_stage")? != "staged"
        || row
            .try_get::<Option<Timestamp>, _>("staged_allocation_deleted")?
            .is_some()
        || row.try_get::<i64, _>("staged_on_hand")? < row.try_get("picked_qty")?
        || row.try_get::<i64, _>("staged_reserved")? < row.try_get("picked_qty")?
    {
        return Err(AppError::conflict(
            "pick confirmation is not reversible in its current execution state",
        ));
    }
    Ok(ReversalTarget {
        confirmation_id,
        task_id: PickTaskId::new(row.try_get("task_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        content_id: PickContentId::new(row.try_get("pick_task_content_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_release_id: row.try_get("order_release_id")?,
        order_item_id: row.try_get("order_item_id")?,
        reservation_id: row.try_get("reservation_id")?,
        inventory_owner_id: owner_id,
        facility_id,
        source_allocation_id: InventoryAllocationId::new(
            row.try_get("source_inventory_allocation_id")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        staged_allocation_id: InventoryAllocationId::new(
            row.try_get("destination_inventory_allocation_id")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        source_balance_id: InventoryBalanceId::new(row.try_get("source_inventory_balance_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        staged_balance_id: InventoryBalanceId::new(
            row.try_get("destination_inventory_balance_id")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_id: LocationId::new(row.try_get("source_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        staged_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_license_plate_id: row
            .try_get::<Option<i64>, _>("source_license_plate_id")?
            .map(LicensePlateId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        staged_license_plate_id: LicensePlateId::new(row.try_get("destination_license_plate_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        inventory_status: InventoryStatus::parse(&row.try_get::<String, _>("inventory_status")?)
            .ok_or_else(|| AppError::internal("pick confirmation has invalid inventory status"))?,
        quantity: PickQuantity::new(row.try_get("picked_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        source_location_barcode: row
            .try_get::<Option<String>, _>("source_location_barcode")?
            .ok_or_else(|| AppError::conflict("original source location is not scannable"))?,
        staged_location_barcode: row
            .try_get::<Option<String>, _>("staged_location_barcode")?
            .ok_or_else(|| AppError::conflict("staged location is not scannable"))?,
        source_license_plate_barcode: row.try_get("source_license_plate_barcode")?,
        staged_license_plate_barcode: row.try_get("staged_license_plate_barcode")?,
    })
}

async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ReversalTarget,
    command: &ReversePickConfirmationCommand,
) -> AppResult<()> {
    if target.staged_location_barcode != command.staged_location_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned staged location does not match the pick confirmation",
        ));
    }
    if target.staged_license_plate_barcode != command.staged_license_plate_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned staged license plate does not match the pick confirmation",
        ));
    }
    if target.source_location_barcode != command.return_location_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned return location does not match the original pick source",
        ));
    }
    match (
        target.source_license_plate_barcode.as_deref(),
        command.return_license_plate_barcode.as_ref(),
    ) {
        (None, None) => {}
        (Some(expected), Some(scanned)) if expected == scanned.as_str() => {}
        (Some(_), None) => {
            return Err(AppError::bad_request(
                "return license plate scan is required for the original pick source",
            ));
        }
        _ => {
            return Err(AppError::bad_request(
                "scanned return license plate does not match the original pick source",
            ));
        }
    }
    validate_identity_scan("lot", target.lot.as_deref(), command.lot_scan.as_ref())?;
    validate_identity_scan(
        "serial",
        target.serial.as_deref(),
        command.serial_scan.as_ref(),
    )?;
    let item_matches: bool = sqlx::query_scalar(
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
    .bind(command.item_barcode.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if !item_matches {
        return Err(AppError::bad_request(
            "scanned item does not match the pick confirmation",
        ));
    }
    Ok(())
}

fn validate_identity_scan(
    label: &str,
    expected: Option<&str>,
    scanned: Option<&wareboxes_domain::PickScanValue>,
) -> AppResult<()> {
    match (expected, scanned) {
        (None, None) => Ok(()),
        (Some(expected), Some(scanned)) if expected == scanned.as_str() => Ok(()),
        (Some(_), None) => Err(AppError::bad_request(format!(
            "{label} scan is required for this pick reversal"
        ))),
        _ => Err(AppError::bad_request(format!(
            "scanned {label} does not match the pick confirmation"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_reversal_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ReversePickConfirmationCommand,
    target: &ReversalTarget,
    transaction_id: i64,
    expected_revision: OrderRevision,
    resulting_revision: OrderRevision,
    reversed_at: Timestamp,
) -> AppResult<PickReversalId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO pick_reversals (
            tenant_id, inventory_owner_id, facility_id, pick_confirmation_id,
            task_id, pick_task_content_id, order_release_id, order_id,
            order_item_id, reservation_id, source_inventory_allocation_id,
            staged_inventory_allocation_id, source_inventory_balance_id,
            staged_inventory_balance_id, source_location_id, staged_location_id,
            source_license_plate_id, staged_license_plate_id, item_batch_id,
            item_id, uom, inventory_status, inventory_transaction_id,
            reversed_qty, expected_order_revision, resulting_order_revision,
            reason, note, reversed_by_user_id, reversed_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
            $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
            $21, $22, $23, $24, $25, $26, $27, $28, $29, $30
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.confirmation_id.get())
    .bind(target.task_id.get())
    .bind(target.content_id.get())
    .bind(target.order_release_id)
    .bind(target.order_id.get())
    .bind(target.order_item_id)
    .bind(target.reservation_id)
    .bind(target.source_allocation_id.get())
    .bind(target.staged_allocation_id.get())
    .bind(target.source_balance_id.get())
    .bind(target.staged_balance_id.get())
    .bind(target.source_location_id.get())
    .bind(target.staged_location_id.get())
    .bind(target.source_license_plate_id.map(|id| id.get()))
    .bind(target.staged_license_plate_id.get())
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(transaction_id)
    .bind(target.quantity.get())
    .bind(expected_revision.get())
    .bind(resulting_revision.get())
    .bind(command.reason.as_str())
    .bind(command.note.as_ref().map(|note| note.as_str()))
    .bind(actor_user_id)
    .bind(reversed_at)
    .fetch_one(&mut **tx)
    .await?;
    PickReversalId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn release_staged_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ReversalTarget,
    reversed_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_allocations
        SET status = 'released', modified = $1, deleted = $1
        WHERE tenant_id = $2 AND inventory_owner_id = $3 AND id = $4
          AND status = 'allocated' AND deleted IS NULL
          AND execution_stage = 'staged' AND qty = $5
        "#,
    )
    .bind(reversed_at)
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.staged_allocation_id.get())
    .bind(target.quantity.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "staged allocation changed during pick reversal",
        ));
    }
    Ok(())
}

async fn decrement_staged_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ReversalTarget,
    reversed_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1, modified = $2
        WHERE tenant_id = $3 AND inventory_owner_id = $4
          AND facility_id = $5 AND id = $6 AND deleted IS NULL
          AND qty_on_hand >= $1
        "#,
    )
    .bind(target.quantity.get())
    .bind(reversed_at)
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.staged_balance_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "staged inventory changed during pick reversal",
        ));
    }
    Ok(())
}

async fn increment_source_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ReversalTarget,
    reversed_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand + $1, modified = $2, deleted = NULL
        WHERE tenant_id = $3 AND inventory_owner_id = $4
          AND facility_id = $5 AND id = $6
        "#,
    )
    .bind(target.quantity.get())
    .bind(reversed_at)
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.source_balance_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "source inventory changed during pick reversal",
        ));
    }
    Ok(())
}

async fn return_full_pallet_header_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ReversalTarget,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE license_plates SET location_id=$1
        WHERE tenant_id=$2 AND inventory_owner_id=$3 AND facility_id=$4
          AND id=$5 AND location_id=$6 AND parent_license_plate_id IS NULL
          AND deleted IS NULL
          AND NOT EXISTS(SELECT 1 FROM license_plates child
            WHERE child.tenant_id=license_plates.tenant_id
              AND child.inventory_owner_id=license_plates.inventory_owner_id
              AND child.facility_id=license_plates.facility_id
              AND child.parent_license_plate_id=license_plates.id
              AND child.deleted IS NULL)
          AND NOT EXISTS(SELECT 1 FROM inventory_balances balance
            WHERE balance.tenant_id=license_plates.tenant_id
              AND balance.inventory_owner_id=license_plates.inventory_owner_id
              AND balance.facility_id=license_plates.facility_id
              AND balance.license_plate_id=license_plates.id
              AND balance.deleted IS NULL AND balance.qty_on_hand>0)"#,
    )
    .bind(target.source_location_id.get())
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.staged_license_plate_id.get())
    .bind(target.staged_location_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "full pallet changed after pick confirmation",
        ));
    }
    Ok(())
}

async fn reactivate_source_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ReversalTarget,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_allocations
        SET status = 'allocated', modified = NULL, deleted = NULL
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND id = $3
          AND status = 'fulfilled' AND deleted IS NOT NULL
          AND execution_stage = 'pick_source' AND qty = $4
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.source_allocation_id.get())
    .bind(target.quantity.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "source allocation changed during pick reversal",
        ));
    }
    Ok(())
}

async fn reopen_pick_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ReversalTarget,
) -> AppResult<()> {
    let content = sqlx::query(
        r#"
        UPDATE pick_task_contents
        SET state = 'pending', completed_at = NULL
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND id = $3 AND task_id = $4 AND state = 'completed'
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.content_id.get())
    .bind(target.task_id.get())
    .execute(&mut **tx)
    .await?;
    if content.rows_affected() != 1 {
        return Err(AppError::conflict(
            "pick content changed during pick reversal",
        ));
    }
    let task = sqlx::query(
        r#"
        UPDATE pick_tasks
        SET status = 'open', assigned_user_id = NULL, claimed_at = NULL,
            lease_expires_at = NULL, completed_at = NULL
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND id = $3 AND status = 'completed'
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.task_id.get())
    .execute(&mut **tx)
    .await?;
    if task.rows_affected() != 1 {
        return Err(AppError::conflict("pick task changed during pick reversal"));
    }
    Ok(())
}

async fn regress_order_to_processing_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    prior_status: OrderStatus,
    expected_revision: OrderRevision,
    resulting_revision: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders SET status = 'processing', revision = $1
        WHERE tenant_id = $2 AND id = $3 AND status = $4 AND revision = $5
        "#,
    )
    .bind(resulting_revision.get())
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(prior_status.as_str())
    .bind(expected_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed during pick reversal"));
    }
    Ok(())
}

async fn require_stored_reversal_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    idempotency_key: &str,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let reversal_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT (result_json->>'reversal_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = $2 AND idempotency_key = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(REVERSE_PICK_CONFIRMATION_OPERATION)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(reversal_id) = reversal_id {
        require_reversal_visible_tx(
            tx,
            tenant_id,
            PickReversalId::new(reversal_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            scope,
        )
        .await?;
    }
    Ok(())
}

async fn require_reversal_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    reversal_id: PickReversalId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id
        FROM pick_reversals WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(reversal_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick reversal"))?;
    if !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
        || !scope.includes_facility(row.try_get("facility_id")?)
    {
        return Err(AppError::not_found("pick reversal"));
    }
    Ok(())
}

async fn enqueue_reversal_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    result: &ReversePickConfirmationResult,
) -> AppResult<()> {
    let facility_id = wareboxes_domain::FacilityId::new(facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("pick-reversal:{}", result.reversal_id);
    let aggregate_id = result.task_id.get().to_string();
    let ordering_key = format!("order:{}", result.order_id.get());
    let aggregate_sequence =
        crate::repo::orders::next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let payload = serde_json::json!({
        "pick_reversal_id": result.reversal_id,
        "pick_confirmation_id": result.confirmation_id,
        "pick_task_id": result.task_id,
        "pick_content_id": result.content_id,
        "order_id": result.order_id,
        "inventory_transaction_id": result.inventory_transaction_id,
        "source_inventory_allocation_id": result.source_inventory_allocation_id,
        "staged_inventory_allocation_id": result.staged_inventory_allocation_id,
        "source_inventory_balance_id": result.source_inventory_balance_id,
        "staged_inventory_balance_id": result.staged_inventory_balance_id,
        "reversed_quantity": result.reversed_quantity,
        "order_status": result.order_status,
        "order_revision": result.order_revision,
        "reason": result.reason,
        "note": result.note,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(result.reversed_by.get()),
            event_key: &event_key,
            aggregate_type: "pick_task",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: "outbound.pick.reversed",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.reversed_at,
        },
    )
    .await?;
    Ok(())
}
