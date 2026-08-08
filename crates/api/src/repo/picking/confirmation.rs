use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::picking::{ConfirmPickContentCommand, ConfirmPickContentResult};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    InventoryAllocationId, InventoryBalanceId, InventoryOwnerId, LicensePlateId, LocationId,
    OrderId, OrderRevision, OrderStatus, PickContentState, PickQuantity, PickTaskId, TenantId,
    Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::orders::insert_order_activity_tx;

use super::CONFIRM_OPERATION;

#[derive(Debug)]
struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    status: OrderStatus,
    revision: OrderRevision,
}

#[derive(Debug)]
struct PickTarget {
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
    quantity: PickQuantity,
    destination_location_id: LocationId,
}

#[derive(Debug)]
struct DestinationPlate {
    id: LicensePlateId,
}

pub async fn confirm_content(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ConfirmPickContentCommand,
) -> AppResult<ConfirmPickContentResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let fingerprint = serde_json::json!({
        "task_id": command.task_id,
        "content_id": command.content_id,
        "source_location_barcode": command.source_location_barcode,
        "item_barcode": command.item_barcode,
        "source_license_plate_barcode": command.source_license_plate_barcode,
        "destination_license_plate_barcode": command.destination_license_plate_barcode,
    });
    let prepared = PreparedCommand::new_v1(context, CONFIRM_OPERATION, &fingerprint)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    if let Some(result) = prepared
        .replayed::<ConfirmPickContentResult>(&mut tx)
        .await?
    {
        require_replayed_confirmation_visible_tx(
            &mut tx,
            access.tenant_id,
            result.result_id,
            result.task_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id = task_order_hint_tx(&mut tx, access.tenant_id, command.task_id).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    if order.status != OrderStatus::Processing {
        return Err(AppError::conflict("order is not in picking execution"));
    }
    let target = lock_pick_target_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &command,
        &scope,
    )
    .await?;
    if target.order_id != order_id || target.inventory_owner_id != order.inventory_owner_id {
        return Err(AppError::internal(
            "pick task does not match its order scope",
        ));
    }
    validate_scans_tx(&mut tx, access.tenant_id, &target, &command).await?;
    let destination_plate = lock_destination_plate_tx(
        &mut tx,
        access.tenant_id,
        &target,
        command.destination_license_plate_barcode.as_str(),
    )
    .await?;

    let owner_facility = inventory_journal::owner_facility_scope(
        target.inventory_owner_id.get(),
        target.facility_id,
    )?;
    let transaction_id = inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("pick confirmation"),
            reference_type: Some("pick_task_content"),
            reference_id: Some(command.content_id.get()),
            correlation_id: Some(&context.request_id),
            operation: CONFIRM_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;
    let confirmed_at = now_iso();
    fulfill_source_allocation_tx(&mut tx, access.tenant_id, &target, confirmed_at).await?;
    decrement_source_balance_tx(&mut tx, access.tenant_id, &target, confirmed_at).await?;
    let destination_balance_id = upsert_destination_balance_tx(
        &mut tx,
        access.tenant_id,
        &target,
        destination_plate.id,
        confirmed_at,
    )
    .await?;
    let destination_allocation_id = create_destination_allocation_tx(
        &mut tx,
        access.tenant_id,
        &target,
        destination_plate.id,
        destination_balance_id,
        context.actor_id.get(),
        confirmed_at,
    )
    .await?;

    for (location_id, license_plate_id, quantity_delta) in [
        (
            target.source_location_id,
            target.source_license_plate_id,
            -target.quantity.get(),
        ),
        (
            target.destination_location_id,
            Some(destination_plate.id),
            target.quantity.get(),
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

    let result_id = insert_confirmation_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &command,
        &target,
        destination_plate.id,
        destination_allocation_id,
        destination_balance_id,
        transaction_id,
        confirmed_at,
    )
    .await?;
    complete_pick_rows_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &command,
        confirmed_at,
    )
    .await?;
    let remaining_pick_count = remaining_pick_count_tx(
        &mut tx,
        access.tenant_id,
        target.inventory_owner_id,
        target.order_id,
    )
    .await?;
    let order_ready_to_pack = remaining_pick_count == 0;
    let (order_status, order_revision) = if order_ready_to_pack {
        advance_order_to_awaiting_shipment_tx(&mut tx, access.tenant_id, &order, target.order_id)
            .await?
    } else {
        (order.status, order.revision)
    };

    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        target.inventory_owner_id,
        target.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "confirmed pick task {} ({} units)",
            command.task_id,
            target.quantity.get()
        ),
    )
    .await?;

    let result = ConfirmPickContentResult {
        result_id,
        content_id: command.content_id,
        task_id: command.task_id,
        order_id: target.order_id,
        inventory_transaction_id: transaction_id,
        source_inventory_allocation_id: target.source_allocation_id,
        destination_inventory_allocation_id: destination_allocation_id,
        source_inventory_balance_id: target.source_balance_id,
        destination_inventory_balance_id: destination_balance_id,
        source_location_id: target.source_location_id,
        destination_location_id: target.destination_location_id,
        source_license_plate_id: target.source_license_plate_id,
        destination_license_plate_id: destination_plate.id,
        picked_quantity: target.quantity,
        confirmed_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        confirmed_at,
        content_state: PickContentState::Completed,
        task_completed: true,
        order_ready_to_pack,
        order_status,
        order_revision,
    };
    enqueue_confirmation_event_tx(
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

async fn task_order_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: PickTaskId,
) -> AppResult<OrderId> {
    let id: i64 =
        sqlx::query_scalar("SELECT order_id FROM pick_tasks WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id.get())
            .bind(task_id.get())
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

async fn lock_pick_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ConfirmPickContentCommand,
    scope: &ScopeBindings,
) -> AppResult<PickTarget> {
    let row = sqlx::query(
        r#"
        SELECT task.order_id, task.order_release_id, task.inventory_owner_id,
               task.facility_id, task.destination_location_id,
               task.lease_expires_at,
               content.order_item_id, content.reservation_id,
               content.source_allocation_id, content.source_inventory_balance_id,
               content.source_location_id, content.source_license_plate_id,
               content.item_batch_id, content.item_id, content.uom,
               content.inventory_status, content.planned_qty, content.state,
               allocation.inventory_balance_id AS allocation_balance_id,
               allocation.location_id AS allocation_location_id,
               allocation.license_plate_id AS allocation_license_plate_id,
               allocation.item_batch_id AS allocation_batch_id,
               allocation.item_id AS allocation_item_id,
               allocation.uom AS allocation_uom,
               allocation.inventory_status AS allocation_status,
               allocation.qty AS allocation_qty,
               allocation.status AS allocation_lifecycle,
               allocation.deleted AS allocation_deleted,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_license_plate_id,
               balance.item_batch_id AS balance_batch_id,
               balance.item_id AS balance_item_id,
               balance.uom AS balance_uom, balance.status AS balance_status,
               balance.qty_on_hand, balance.qty_reserved, balance.deleted AS balance_deleted
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
        WHERE task.tenant_id = $1 AND task.id = $2
          AND content.id = $3
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
    let now = now_iso();
    if row.try_get::<Timestamp, _>("lease_expires_at")? <= now {
        return Err(AppError::conflict("pick claim has expired"));
    }
    let target = PickTarget {
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
        quantity: PickQuantity::new(row.try_get("planned_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    };
    validate_target_row(&row, &target)?;
    Ok(target)
}

fn validate_target_row(row: &sqlx::postgres::PgRow, target: &PickTarget) -> AppResult<()> {
    let quantity = target.quantity.get();
    let valid = row.try_get::<String, _>("state")? == "pending"
        && row.try_get::<i64, _>("allocation_balance_id")? == target.source_balance_id.get()
        && row.try_get::<i64, _>("allocation_location_id")? == target.source_location_id.get()
        && row.try_get::<Option<i64>, _>("allocation_license_plate_id")?
            == target.source_license_plate_id.map(|id| id.get())
        && row.try_get::<i64, _>("allocation_batch_id")? == target.item_batch_id
        && row.try_get::<i64, _>("allocation_item_id")? == target.item_id
        && row.try_get::<String, _>("allocation_uom")? == target.uom
        && row.try_get::<String, _>("allocation_status")? == target.inventory_status.as_str()
        && row.try_get::<i64, _>("allocation_qty")? == quantity
        && row.try_get::<String, _>("allocation_lifecycle")? == "allocated"
        && row
            .try_get::<Option<Timestamp>, _>("allocation_deleted")?
            .is_none()
        && row.try_get::<i64, _>("balance_location_id")? == target.source_location_id.get()
        && row.try_get::<Option<i64>, _>("balance_license_plate_id")?
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
            "allocated source stock changed before pick confirmation",
        ));
    }
    Ok(())
}

async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    command: &ConfirmPickContentCommand,
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
    let item_matches: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM barcodes
            WHERE tenant_id = $1 AND item_id = $2 AND deleted IS NULL AND name = $3
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
            "scanned item does not match the directed pick",
        ));
    }
    match (
        target.source_license_plate_id,
        command.source_license_plate_barcode.as_ref(),
    ) {
        (None, None) => {}
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
            let barcode = barcode.ok_or_else(|| {
                AppError::conflict("directed source license plate is no longer available")
            })?;
            if barcode != scanned.as_str() {
                return Err(AppError::bad_request(
                    "scanned source license plate does not match the directed pick",
                ));
            }
        }
        _ => {
            return Err(AppError::bad_request(
                "source license plate scan does not match the directed pick",
            ));
        }
    }
    Ok(())
}

async fn lock_destination_plate_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    barcode: &str,
) -> AppResult<DestinationPlate> {
    let row = sqlx::query(
        r#"
        SELECT plate.id, plate.location_id,
               location.active, location.pickable, location.barcode, location.type
        FROM license_plates plate
        INNER JOIN locations location
          ON location.tenant_id = plate.tenant_id
         AND location.facility_id = plate.facility_id
         AND location.id = plate.location_id AND location.deleted IS NULL
        WHERE plate.tenant_id = $1 AND plate.inventory_owner_id = $2
          AND plate.facility_id = $3 AND plate.barcode = $4
          AND plate.deleted IS NULL
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
    if location_id != target.destination_location_id.get()
        || !row.try_get::<bool, _>("active")?
        || row.try_get::<bool, _>("pickable")?
        || !matches!(
            row.try_get::<String, _>("type")?
                .to_ascii_lowercase()
                .as_str(),
            "staging" | "packing"
        )
        || row
            .try_get::<Option<String>, _>("barcode")?
            .is_none_or(|value| value.trim().is_empty())
    {
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
    Ok(DestinationPlate { id })
}

async fn fulfill_source_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_allocations
        SET status = 'fulfilled', modified = $1, deleted = $1
        WHERE tenant_id = $2 AND inventory_owner_id = $3 AND id = $4
          AND status = 'allocated' AND deleted IS NULL AND qty = $5
        "#,
    )
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.source_allocation_id.get())
    .bind(target.quantity.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("source allocation changed during pick"));
    }
    Ok(())
}

async fn decrement_source_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    occurred_at: Timestamp,
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
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.source_balance_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("source inventory changed during pick"));
    }
    Ok(())
}

async fn upsert_destination_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    destination_plate_id: LicensePlateId,
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
    .bind(destination_plate_id.get())
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(target.quantity.get())
    .fetch_one(&mut **tx)
    .await?;
    InventoryBalanceId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn create_destination_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    destination_plate_id: LicensePlateId,
    destination_balance_id: InventoryBalanceId,
    actor_user_id: i64,
    occurred_at: Timestamp,
) -> AppResult<InventoryAllocationId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_allocations (
            tenant_id, inventory_owner_id, created, created_by,
            reservation_id, inventory_balance_id, facility_id, location_id,
            license_plate_id, item_batch_id, item_id, uom, inventory_status,
            allocation_run_id, qty, status
        )
        SELECT tenant_id, inventory_owner_id, $1, $2, reservation_id, $3,
               facility_id, $4, $5, item_batch_id, item_id, uom,
               inventory_status, allocation_run_id, qty, 'allocated'
        FROM inventory_allocations
        WHERE tenant_id = $6 AND inventory_owner_id = $7 AND id = $8
          AND status = 'fulfilled' AND deleted = $1
        RETURNING id
        "#,
    )
    .bind(occurred_at)
    .bind(actor_user_id)
    .bind(destination_balance_id.get())
    .bind(target.destination_location_id.get())
    .bind(destination_plate_id.get())
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.source_allocation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("source allocation could not be transferred"))?;
    InventoryAllocationId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn complete_pick_rows_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ConfirmPickContentCommand,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let content = sqlx::query(
        r#"
        UPDATE pick_task_contents SET state = 'completed', completed_at = $1
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
        return Err(AppError::conflict(
            "pick content changed during confirmation",
        ));
    }
    let task = sqlx::query(
        r#"
        UPDATE pick_tasks SET status = 'completed', completed_at = $1,
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
        return Err(AppError::conflict("pick task changed during confirmation"));
    }
    Ok(())
}

async fn remaining_pick_count_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM pick_tasks
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND order_id = $3 AND status <> 'completed'
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_one(&mut **tx)
    .await?)
}

async fn advance_order_to_awaiting_shipment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order: &LockedOrder,
    order_id: OrderId,
) -> AppResult<(OrderStatus, OrderRevision)> {
    let revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let updated = sqlx::query(
        r#"
        UPDATE orders SET status = 'awaiting shipment', revision = $1
        WHERE tenant_id = $2 AND id = $3 AND status = 'processing' AND revision = $4
        "#,
    )
    .bind(revision.get())
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(order.revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed during pick confirmation"));
    }
    Ok((OrderStatus::AwaitingShipment, revision))
}

#[allow(clippy::too_many_arguments)]
async fn insert_confirmation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ConfirmPickContentCommand,
    target: &PickTarget,
    destination_plate_id: LicensePlateId,
    destination_allocation_id: InventoryAllocationId,
    destination_balance_id: InventoryBalanceId,
    transaction_id: i64,
    confirmed_at: Timestamp,
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
    .bind(target.quantity.get())
    .bind(actor_user_id)
    .bind(confirmed_at)
    .fetch_one(&mut **tx)
    .await?)
}

async fn require_replayed_confirmation_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result_id: i64,
    task_id: PickTaskId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id FROM pick_confirmations
        WHERE tenant_id = $1 AND id = $2 AND task_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(result_id)
    .bind(task_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick confirmation"))?;
    if !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
        || !scope.includes_facility(row.try_get("facility_id")?)
    {
        return Err(AppError::not_found("pick confirmation"));
    }
    Ok(())
}

async fn enqueue_confirmation_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    result: &ConfirmPickContentResult,
) -> AppResult<()> {
    let facility_id = wareboxes_domain::FacilityId::new(facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("pick-confirmation:{}", result.result_id);
    let aggregate_id = result.task_id.get().to_string();
    let ordering_key = format!("order:{}", result.order_id.get());
    let payload = serde_json::json!({
        "pick_confirmation_id": result.result_id,
        "pick_task_id": result.task_id,
        "pick_content_id": result.content_id,
        "order_id": result.order_id,
        "inventory_transaction_id": result.inventory_transaction_id,
        "source_inventory_allocation_id": result.source_inventory_allocation_id,
        "destination_inventory_allocation_id": result.destination_inventory_allocation_id,
        "source_inventory_balance_id": result.source_inventory_balance_id,
        "destination_inventory_balance_id": result.destination_inventory_balance_id,
        "picked_quantity": result.picked_quantity,
        "order_ready_to_pack": result.order_ready_to_pack,
        "order_status": result.order_status,
        "order_revision": result.order_revision,
    });
    let aggregate_sequence =
        crate::repo::orders::next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(result.confirmed_by.get()),
            event_key: &event_key,
            aggregate_type: "pick_task",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: "outbound.pick.confirmed",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.confirmed_at,
        },
    )
    .await?;
    Ok(())
}
