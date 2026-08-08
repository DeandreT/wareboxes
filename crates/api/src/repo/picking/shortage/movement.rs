use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::picking::{ReportPickShortageCommand, REPORT_PICK_SHORTAGE_OPERATION};
use wareboxes_application::CommandContext;
use wareboxes_core::models::InventoryTransactionType;
use wareboxes_domain::{
    InventoryAllocationId, InventoryBalanceId, LicensePlateId, PickQuantity, TenantId, Timestamp,
};

use crate::error::{AppError, AppResult};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};

use super::{PartialMovement, ShortageTarget};

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_partial_move_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    context: &CommandContext,
    prepared: &PreparedCommand,
    command: &ReportPickShortageCommand,
    target: &ShortageTarget,
    picked_quantity: PickQuantity,
    destination_barcode: &str,
    reported_at: Timestamp,
) -> AppResult<PartialMovement> {
    if picked_quantity.get() >= target.planned_quantity.get() {
        return Err(AppError::bad_request(
            "short pick quantity must be less than the directed quantity",
        ));
    }
    let destination_plate_id =
        lock_destination_plate_tx(tx, tenant_id, target, destination_barcode).await?;
    bind_outbound_container_tx(
        tx,
        tenant_id,
        context.actor_id.get(),
        target,
        destination_plate_id,
        reported_at,
    )
    .await?;
    let owner_facility = inventory_journal::owner_facility_scope(
        target.inventory_owner_id.get(),
        target.facility_id,
    )?;
    let transaction_id = inventory_journal::begin_transaction(
        tx,
        &JournalCommand {
            tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("partial pick shortage"),
            reference_type: Some("pick_task_content"),
            reference_id: Some(command.content_id.get()),
            correlation_id: Some(&context.request_id),
            operation: REPORT_PICK_SHORTAGE_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;
    short_source_allocation_tx(tx, tenant_id, target, reported_at).await?;
    decrement_source_balance_tx(tx, tenant_id, target, picked_quantity, reported_at).await?;
    let destination_balance_id = upsert_destination_balance_tx(
        tx,
        tenant_id,
        target,
        destination_plate_id,
        picked_quantity,
        reported_at,
    )
    .await?;
    let destination_allocation_id = create_staged_allocation_tx(
        tx,
        tenant_id,
        target,
        destination_plate_id,
        destination_balance_id,
        picked_quantity,
        context.actor_id.get(),
        reported_at,
    )
    .await?;
    for (location_id, license_plate_id, delta) in [
        (
            target.source_location_id,
            target.source_license_plate_id,
            -picked_quantity.get(),
        ),
        (
            target.destination_location_id,
            Some(destination_plate_id),
            picked_quantity.get(),
        ),
    ] {
        inventory_journal::append_entry(
            tx,
            tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: location_id.get(),
                license_plate_id: license_plate_id.map(|id| id.get()),
                item_batch_id: target.item_batch_id,
                status: target.inventory_status,
                quantity_delta: delta,
            },
        )
        .await?;
    }
    let confirmation_id = insert_partial_confirmation_tx(
        tx,
        tenant_id,
        context.actor_id.get(),
        command,
        target,
        destination_plate_id,
        destination_allocation_id,
        destination_balance_id,
        transaction_id,
        picked_quantity,
        reported_at,
    )
    .await?;
    Ok(PartialMovement {
        transaction_id,
        confirmation_id,
        destination_allocation_id,
        destination_balance_id,
        destination_plate_id,
        picked_quantity,
    })
}

async fn lock_destination_plate_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ShortageTarget,
    barcode: &str,
) -> AppResult<LicensePlateId> {
    let row = sqlx::query(
        r#"
        SELECT plate.id, plate.location_id, location.active,
               location.pickable, location.barcode, location.type
        FROM license_plates plate
        INNER JOIN locations location
          ON location.tenant_id = plate.tenant_id
         AND location.facility_id = plate.facility_id
         AND location.id = plate.location_id AND location.deleted IS NULL
        WHERE plate.tenant_id = $1 AND plate.inventory_owner_id = $2
          AND plate.facility_id = $3 AND plate.barcode = $4 AND plate.deleted IS NULL
        FOR UPDATE OF plate
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(barcode)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        AppError::bad_request("scanned destination license plate is not available in this facility")
    })?;
    let location_id: i64 = row.try_get("location_id")?;
    let valid = location_id == target.destination_location_id.get()
        && row.try_get::<bool, _>("active")?
        && !row.try_get::<bool, _>("pickable")?
        && matches!(
            row.try_get::<String, _>("type")?
                .to_ascii_lowercase()
                .as_str(),
            "staging" | "packing"
        )
        && row
            .try_get::<Option<String>, _>("barcode")?
            .is_some_and(|value| !value.trim().is_empty());
    if !valid {
        return Err(AppError::conflict(
            "destination license plate is not at the directed staging location",
        ));
    }
    let id = LicensePlateId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    if Some(id) == target.source_license_plate_id {
        return Err(AppError::conflict(
            "source and destination license plates must differ",
        ));
    }
    Ok(id)
}

async fn bind_outbound_container_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    target: &ShortageTarget,
    plate_id: LicensePlateId,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let existing = sqlx::query(
        r#"
        SELECT order_release_id, order_id, destination_location_id
        FROM outbound_order_containers
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND facility_id = $3 AND license_plate_id = $4
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(plate_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(existing) = existing {
        let matches = existing.try_get::<i64, _>("order_release_id")? == target.release_id
            && existing.try_get::<i64, _>("order_id")? == target.order_id.get()
            && existing.try_get::<i64, _>("destination_location_id")?
                == target.destination_location_id.get();
        return matches.then_some(()).ok_or_else(|| {
            AppError::conflict("destination license plate is assigned to another outbound order")
        });
    }
    let occupied: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM inventory_balances
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND facility_id = $3 AND license_plate_id = $4
          AND deleted IS NULL AND qty_on_hand > 0
        ORDER BY id LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(plate_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    if occupied.is_some() {
        return Err(AppError::conflict(
            "unassigned destination license plate is not empty",
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO outbound_order_containers (
            tenant_id, inventory_owner_id, facility_id, order_release_id,
            order_id, destination_location_id, license_plate_id,
            created_by_user_id, created_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.release_id)
    .bind(target.order_id.get())
    .bind(target.destination_location_id.get())
    .bind(plate_id.get())
    .bind(actor_user_id)
    .bind(occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
pub(super) async fn short_source_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ShortageTarget,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_allocations
        SET status = 'shorted', modified = $1, deleted = $1
        WHERE tenant_id = $2 AND inventory_owner_id = $3 AND id = $4
          AND status = 'allocated' AND deleted IS NULL
          AND execution_stage = 'pick_source' AND qty = $5
        "#,
    )
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.source_allocation_id.get())
    .bind(target.planned_quantity.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "source allocation changed during short pick",
        ));
    }
    Ok(())
}

async fn decrement_source_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ShortageTarget,
    picked_quantity: PickQuantity,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_balances SET qty_on_hand = qty_on_hand - $1, modified = $2
        WHERE tenant_id = $3 AND inventory_owner_id = $4
          AND facility_id = $5 AND id = $6 AND deleted IS NULL
          AND qty_on_hand >= $1
        "#,
    )
    .bind(picked_quantity.get())
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.source_balance_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "source inventory changed during short pick",
        ));
    }
    Ok(())
}

async fn upsert_destination_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ShortageTarget,
    plate_id: LicensePlateId,
    quantity: PickQuantity,
    occurred_at: Timestamp,
) -> AppResult<InventoryBalanceId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_balances (
            tenant_id, inventory_owner_id, created, modified, facility_id,
            location_id, license_plate_id, item_batch_id, item_id, uom,
            status, qty_on_hand, qty_reserved, qty_held
        ) VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, 0, 0)
        ON CONFLICT (
            tenant_id, inventory_owner_id, location_id, license_plate_id,
            item_batch_id, uom, status
        ) WHERE license_plate_id IS NOT NULL
        DO UPDATE SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                      modified = excluded.modified, deleted = NULL
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(occurred_at)
    .bind(target.facility_id)
    .bind(target.destination_location_id.get())
    .bind(plate_id.get())
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(quantity.get())
    .fetch_one(&mut **tx)
    .await?;
    InventoryBalanceId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn create_staged_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &ShortageTarget,
    plate_id: LicensePlateId,
    balance_id: InventoryBalanceId,
    quantity: PickQuantity,
    actor_user_id: i64,
    occurred_at: Timestamp,
) -> AppResult<InventoryAllocationId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_allocations (
            tenant_id, inventory_owner_id, created, created_by,
            reservation_id, inventory_balance_id, facility_id, location_id,
            license_plate_id, item_batch_id, item_id, uom, inventory_status,
            allocation_run_id, qty, status, execution_stage
        )
        SELECT tenant_id, inventory_owner_id, $1, $2, reservation_id, $3,
               facility_id, $4, $5, item_batch_id, item_id, uom,
               inventory_status, allocation_run_id, $6, 'allocated', 'staged'
        FROM inventory_allocations
        WHERE tenant_id = $7 AND inventory_owner_id = $8 AND id = $9
          AND status = 'shorted' AND deleted = $1 AND execution_stage = 'pick_source'
        RETURNING id
        "#,
    )
    .bind(occurred_at)
    .bind(actor_user_id)
    .bind(balance_id.get())
    .bind(target.destination_location_id.get())
    .bind(plate_id.get())
    .bind(quantity.get())
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.source_allocation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("source allocation could not be partially transferred"))?;
    InventoryAllocationId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_partial_confirmation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ReportPickShortageCommand,
    target: &ShortageTarget,
    destination_plate_id: LicensePlateId,
    destination_allocation_id: InventoryAllocationId,
    destination_balance_id: InventoryBalanceId,
    transaction_id: i64,
    quantity: PickQuantity,
    occurred_at: Timestamp,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        r#"
        INSERT INTO pick_confirmations (
            tenant_id, inventory_owner_id, facility_id, task_id,
            pick_task_content_id, order_release_id, order_id, order_item_id,
            reservation_id, source_inventory_allocation_id,
            destination_inventory_allocation_id, source_inventory_balance_id,
            destination_inventory_balance_id, source_location_id,
            destination_location_id, source_license_plate_id,
            destination_license_plate_id, item_batch_id, item_id, uom,
            inventory_status, inventory_transaction_id, picked_qty,
            confirmed_by_user_id, confirmed_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23,
            $24, $25
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(command.task_id.get())
    .bind(command.content_id.get())
    .bind(target.release_id)
    .bind(target.order_id.get())
    .bind(target.order_item_id)
    .bind(target.reservation_id)
    .bind(target.source_allocation_id.get())
    .bind(destination_allocation_id.get())
    .bind(target.source_balance_id.get())
    .bind(destination_balance_id.get())
    .bind(target.source_location_id.get())
    .bind(target.destination_location_id.get())
    .bind(target.source_license_plate_id.map(|id| id.get()))
    .bind(destination_plate_id.get())
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(transaction_id)
    .bind(quantity.get())
    .bind(actor_user_id)
    .bind(occurred_at)
    .fetch_one(&mut **tx)
    .await?)
}
