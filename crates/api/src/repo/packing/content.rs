use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::packing::{
    PackPickedAllocationCommand, PackPickedAllocationResult, PACK_PICKED_ALLOCATION_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    CartonContentId, InventoryAllocationId, InventoryBalanceId, ItemBatchId, LicensePlateId,
    LocationId, OrderLineId, PackQuantity, PackingProgress, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::inventory_locking;
use crate::repo::orders::insert_order_activity_tx;

use super::carton::update_order_tx;
use super::{
    enqueue_order_event_tx, lock_order_tx, lock_session_tx, require_replayed_ids_visible_tx,
    require_revision, session_order_hint_tx,
};

#[derive(Debug)]
struct LockedCarton {
    license_plate_id: LicensePlateId,
    barcode: String,
    state: String,
}

#[derive(Debug)]
struct PackTarget {
    snapshot_id: i64,
    order_item_id: i64,
    reservation_id: i64,
    outbound_order_container_id: i64,
    pick_confirmation_id: i64,
    source_allocation_id: InventoryAllocationId,
    source_balance_id: InventoryBalanceId,
    source_location_id: LocationId,
    source_license_plate_id: LicensePlateId,
    source_license_plate_barcode: String,
    item_batch_id: ItemBatchId,
    item_id: i64,
    uom: String,
    lot: Option<String>,
    serial: Option<String>,
    inventory_status: InventoryStatus,
    quantity: PackQuantity,
}

pub async fn pack_picked_allocation(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PackPickedAllocationCommand,
) -> AppResult<PackPickedAllocationResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let fingerprint = serde_json::json!({
        "session_id": command.session_id,
        "carton_id": command.carton_id,
        "inventory_allocation_id": command.inventory_allocation_id,
        "item_barcode": command.item_barcode,
        "lot_scan": command.lot_scan,
        "serial_scan": command.serial_scan,
        "source_license_plate_barcode": command.source_license_plate_barcode,
        "carton_barcode": command.carton_barcode,
        "expected_revision": command.expected_revision,
    });
    let prepared =
        PreparedCommand::new_v1(context, PACK_PICKED_ALLOCATION_OPERATION, &fingerprint)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<PackPickedAllocationResult>(&mut tx)
        .await?
    {
        require_replayed_ids_visible_tx(
            &mut tx,
            access.tenant_id,
            result.session_id,
            result.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id = session_order_hint_tx(&mut tx, access.tenant_id, command.session_id).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    wareboxes_domain::continue_packing(order.status)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let session = lock_session_tx(&mut tx, access.tenant_id, command.session_id, &scope).await?;
    if session.order_id != order_id || session.state != "open" {
        return Err(AppError::conflict("packing session is not open"));
    }
    let revision = require_revision(&order, Some(&session), command.expected_revision)?;
    let carton = lock_carton_tx(&mut tx, access.tenant_id, command).await?;
    if carton.state != "open" {
        return Err(AppError::conflict("carton is already closed"));
    }
    if carton.barcode != command.carton_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned carton does not match the open carton",
        ));
    }
    let source_plate_id = source_plate_hint_tx(&mut tx, access.tenant_id, command).await?;
    inventory_locking::lock_license_plates(
        &mut tx,
        access.tenant_id,
        vec![source_plate_id, carton.license_plate_id.get()],
    )
    .await?;
    let target = lock_target_tx(&mut tx, access.tenant_id, command).await?;
    validate_scans_tx(&mut tx, access.tenant_id, command, &target).await?;
    require_carton_plate_at_station_tx(
        &mut tx,
        access.tenant_id,
        &session,
        carton.license_plate_id,
    )
    .await?;

    let owner_facility = inventory_journal::owner_facility_scope(
        session.inventory_owner_id.get(),
        session.facility_id,
    )?;
    let transaction_id = inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("pack picked allocation"),
            reference_type: Some("packing_session_allocation"),
            reference_id: Some(target.snapshot_id),
            correlation_id: Some(&context.request_id),
            operation: PACK_PICKED_ALLOCATION_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;
    let packed_at = now_iso();
    fulfill_source_allocation_tx(&mut tx, access.tenant_id, &target, packed_at).await?;
    decrement_source_balance_tx(&mut tx, access.tenant_id, &target, packed_at).await?;
    let destination_balance_id = upsert_carton_balance_tx(
        &mut tx,
        access.tenant_id,
        &session,
        &target,
        carton.license_plate_id,
        packed_at,
    )
    .await?;
    let destination_allocation_id = create_carton_allocation_tx(
        &mut tx,
        access.tenant_id,
        &session,
        &target,
        destination_balance_id,
        carton.license_plate_id,
        context.actor_id.get(),
        packed_at,
    )
    .await?;
    for (location_id, license_plate_id, quantity_delta) in [
        (
            target.source_location_id,
            target.source_license_plate_id,
            -target.quantity.get(),
        ),
        (
            LocationId::new(session.packing_location_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            carton.license_plate_id,
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
                license_plate_id: Some(license_plate_id.get()),
                item_batch_id: target.item_batch_id.get(),
                status: target.inventory_status,
                quantity_delta,
            },
        )
        .await?;
    }
    let content_id = insert_content_tx(
        &mut tx,
        access.tenant_id,
        &session,
        command,
        &target,
        carton.license_plate_id,
        destination_allocation_id,
        destination_balance_id,
        transaction_id,
        context.actor_id.get(),
        packed_at,
    )
    .await?;
    let progress = update_progress_tx(
        &mut tx,
        access.tenant_id,
        &session,
        revision,
        target.quantity.get(),
    )
    .await?;
    update_order_tx(
        &mut tx,
        access.tenant_id,
        order_id,
        order.status,
        order.revision,
        order.status,
        revision,
    )
    .await?;

    let result = PackPickedAllocationResult {
        content_id,
        session_id: session.id,
        carton_id: command.carton_id,
        order_id,
        order_line_id: OrderLineId::new(target.order_item_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_allocation_id: command.inventory_allocation_id,
        inventory_transaction_id: transaction_id,
        source_inventory_allocation_id: target.source_allocation_id,
        destination_inventory_allocation_id: destination_allocation_id,
        source_inventory_balance_id: target.source_balance_id,
        destination_inventory_balance_id: destination_balance_id,
        source_location_id: target.source_location_id,
        destination_location_id: LocationId::new(session.packing_location_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_license_plate_id: target.source_license_plate_id,
        destination_license_plate_id: carton.license_plate_id,
        item_batch_id: target.item_batch_id,
        item_id: target.item_id,
        quantity: target.quantity,
        uom: target.uom.clone(),
        packed_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        packed_at,
        revision,
        progress,
    };
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "packed {} {} into carton {}",
            target.quantity.get(),
            target.uom,
            command.carton_id
        ),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        session.facility_id,
        context.actor_id.get(),
        order_id,
        "packing.content_confirmed",
        &format!("carton-content:{}:confirmed", content_id.get()),
        serde_json::json!({
            "packing_session_id": session.id,
            "carton_id": command.carton_id,
            "content_id": content_id,
            "order_id": order_id,
            "source_inventory_allocation_id": target.source_allocation_id,
            "destination_inventory_allocation_id": destination_allocation_id,
            "inventory_transaction_id": transaction_id,
            "quantity": target.quantity,
            "uom": target.uom,
            "revision": revision,
            "packed_at": packed_at,
        }),
        packed_at,
    )
    .await?;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await?)
}

async fn lock_carton_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &PackPickedAllocationCommand,
) -> AppResult<LockedCarton> {
    let row = sqlx::query(
        r#"
        SELECT carton.license_plate_id, plate.barcode, carton.state
        FROM cartons carton
        INNER JOIN license_plates plate
          ON plate.tenant_id = carton.tenant_id
         AND plate.inventory_owner_id = carton.inventory_owner_id
         AND plate.facility_id = carton.facility_id
         AND plate.id = carton.license_plate_id
        WHERE carton.tenant_id = $1 AND carton.packing_session_id = $2
          AND carton.id = $3
        FOR UPDATE OF carton
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.session_id.get())
    .bind(command.carton_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("carton"))?;
    Ok(LockedCarton {
        license_plate_id: LicensePlateId::new(row.try_get("license_plate_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        barcode: row.try_get("barcode")?,
        state: row.try_get("state")?,
    })
}

async fn source_plate_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &PackPickedAllocationCommand,
) -> AppResult<i64> {
    sqlx::query_scalar(
        r#"
        SELECT source_license_plate_id
        FROM packing_session_allocations
        WHERE tenant_id = $1 AND packing_session_id = $2
          AND source_inventory_allocation_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.session_id.get())
    .bind(command.inventory_allocation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packable allocation"))
}

async fn lock_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &PackPickedAllocationCommand,
) -> AppResult<PackTarget> {
    let row = sqlx::query(
        r#"
        SELECT snapshot.id, snapshot.order_item_id, snapshot.reservation_id,
               snapshot.outbound_order_container_id, snapshot.pick_confirmation_id,
               snapshot.source_inventory_allocation_id,
               snapshot.source_inventory_balance_id, snapshot.source_location_id,
               snapshot.source_license_plate_id, plate.barcode,
               plate.location_id AS plate_location_id, plate.deleted AS plate_deleted,
               snapshot.item_batch_id, snapshot.item_id, snapshot.uom,
               snapshot.inventory_status, snapshot.planned_qty,
               batch.lot, batch.serial,
               allocation.reservation_id AS allocation_reservation_id,
               allocation.inventory_balance_id AS allocation_balance_id,
               allocation.location_id AS allocation_location_id,
               allocation.license_plate_id AS allocation_plate_id,
               allocation.item_batch_id AS allocation_batch_id,
               allocation.item_id AS allocation_item_id,
               allocation.uom AS allocation_uom,
               allocation.inventory_status AS allocation_status,
               allocation.qty AS allocation_qty,
               allocation.status AS allocation_lifecycle,
               allocation.execution_stage AS allocation_execution_stage,
               allocation.deleted AS allocation_deleted,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_plate_id,
               balance.item_batch_id AS balance_batch_id,
               balance.item_id AS balance_item_id, balance.uom AS balance_uom,
               balance.status AS balance_status, balance.qty_on_hand,
               balance.qty_reserved, balance.deleted AS balance_deleted,
               content.id AS existing_content_id
        FROM packing_session_allocations snapshot
        INNER JOIN license_plates plate
          ON plate.tenant_id = snapshot.tenant_id
         AND plate.inventory_owner_id = snapshot.inventory_owner_id
         AND plate.facility_id = snapshot.facility_id
         AND plate.id = snapshot.source_license_plate_id
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id = snapshot.tenant_id
         AND allocation.inventory_owner_id = snapshot.inventory_owner_id
         AND allocation.id = snapshot.source_inventory_allocation_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = snapshot.tenant_id
         AND batch.inventory_owner_id = snapshot.inventory_owner_id
         AND batch.id = snapshot.item_batch_id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = snapshot.tenant_id
         AND balance.inventory_owner_id = snapshot.inventory_owner_id
         AND balance.facility_id = snapshot.facility_id
         AND balance.id = snapshot.source_inventory_balance_id
        LEFT JOIN carton_contents content
          ON content.tenant_id = snapshot.tenant_id
         AND content.packing_session_allocation_id = snapshot.id
        WHERE snapshot.tenant_id = $1 AND snapshot.packing_session_id = $2
          AND snapshot.source_inventory_allocation_id = $3
        FOR UPDATE OF allocation, balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.session_id.get())
    .bind(command.inventory_allocation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packable allocation"))?;
    if row
        .try_get::<Option<i64>, _>("existing_content_id")?
        .is_some()
    {
        return Err(AppError::conflict("picked allocation is already packed"));
    }
    let target = PackTarget {
        snapshot_id: row.try_get("id")?,
        order_item_id: row.try_get("order_item_id")?,
        reservation_id: row.try_get("reservation_id")?,
        outbound_order_container_id: row.try_get("outbound_order_container_id")?,
        pick_confirmation_id: row.try_get("pick_confirmation_id")?,
        source_allocation_id: InventoryAllocationId::new(
            row.try_get("source_inventory_allocation_id")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        source_balance_id: InventoryBalanceId::new(row.try_get("source_inventory_balance_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_id: LocationId::new(row.try_get("source_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_license_plate_id: LicensePlateId::new(row.try_get("source_license_plate_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_license_plate_barcode: row.try_get("barcode")?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        inventory_status: InventoryStatus::parse(&row.try_get::<String, _>("inventory_status")?)
            .ok_or_else(|| AppError::internal("pack snapshot has invalid inventory status"))?,
        quantity: PackQuantity::new(row.try_get("planned_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    };
    let valid = row.try_get::<i64, _>("allocation_reservation_id")? == target.reservation_id
        && row.try_get::<Option<i64>, _>("plate_location_id")?
            == Some(target.source_location_id.get())
        && row
            .try_get::<Option<Timestamp>, _>("plate_deleted")?
            .is_none()
        && row.try_get::<i64, _>("allocation_balance_id")? == target.source_balance_id.get()
        && row.try_get::<i64, _>("allocation_location_id")? == target.source_location_id.get()
        && row.try_get::<Option<i64>, _>("allocation_plate_id")?
            == Some(target.source_license_plate_id.get())
        && row.try_get::<i64, _>("allocation_batch_id")? == target.item_batch_id.get()
        && row.try_get::<i64, _>("allocation_item_id")? == target.item_id
        && row.try_get::<String, _>("allocation_uom")? == target.uom
        && row.try_get::<String, _>("allocation_status")? == target.inventory_status.as_str()
        && row.try_get::<i64, _>("allocation_qty")? == target.quantity.get()
        && row.try_get::<String, _>("allocation_lifecycle")? == "allocated"
        && row.try_get::<String, _>("allocation_execution_stage")? == "staged"
        && row
            .try_get::<Option<Timestamp>, _>("allocation_deleted")?
            .is_none()
        && row.try_get::<i64, _>("balance_location_id")? == target.source_location_id.get()
        && row.try_get::<Option<i64>, _>("balance_plate_id")?
            == Some(target.source_license_plate_id.get())
        && row.try_get::<i64, _>("balance_batch_id")? == target.item_batch_id.get()
        && row.try_get::<i64, _>("balance_item_id")? == target.item_id
        && row.try_get::<String, _>("balance_uom")? == target.uom
        && row.try_get::<String, _>("balance_status")? == target.inventory_status.as_str()
        && row.try_get::<i64, _>("qty_on_hand")? >= target.quantity.get()
        && row.try_get::<i64, _>("qty_reserved")? >= target.quantity.get()
        && row
            .try_get::<Option<Timestamp>, _>("balance_deleted")?
            .is_none();
    if !valid {
        return Err(AppError::conflict(
            "picked allocation or source tote changed before packing confirmation",
        ));
    }
    Ok(target)
}

async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &PackPickedAllocationCommand,
    target: &PackTarget,
) -> AppResult<()> {
    if target.source_license_plate_barcode != command.source_license_plate_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned source tote does not match the picked allocation",
        ));
    }
    validate_stock_identity_scan("lot", target.lot.as_deref(), command.lot_scan.as_ref())?;
    validate_stock_identity_scan(
        "serial",
        target.serial.as_deref(),
        command.serial_scan.as_ref(),
    )?;
    let item_matches: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM barcodes
            WHERE tenant_id = $1 AND item_id = $2
              AND name = $3 AND deleted IS NULL
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
            "scanned item does not match the picked allocation",
        ));
    }
    Ok(())
}

fn validate_stock_identity_scan(
    label: &str,
    expected: Option<&str>,
    scanned: Option<&wareboxes_domain::PackScanValue>,
) -> AppResult<()> {
    match (expected, scanned) {
        (None, None) => Ok(()),
        (Some(expected), Some(scanned)) if expected == scanned.as_str() => Ok(()),
        (Some(_), None) => Err(AppError::bad_request(format!(
            "{label} scan is required for this picked allocation"
        ))),
        _ => Err(AppError::bad_request(format!(
            "scanned {label} does not match the picked allocation"
        ))),
    }
}

async fn require_carton_plate_at_station_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    plate_id: LicensePlateId,
) -> AppResult<()> {
    let matches: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM license_plates
            WHERE tenant_id = $1 AND inventory_owner_id = $2
              AND facility_id = $3 AND id = $4 AND location_id = $5
              AND deleted IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(session.facility_id)
    .bind(plate_id.get())
    .bind(session.packing_location_id)
    .fetch_one(&mut **tx)
    .await?;
    if matches {
        Ok(())
    } else {
        Err(AppError::conflict(
            "carton is no longer at the packing station",
        ))
    }
}

async fn fulfill_source_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PackTarget,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_allocations
        SET status = 'fulfilled', modified = $1, deleted = $1
        WHERE tenant_id = $2 AND id = $3 AND status = 'allocated'
          AND deleted IS NULL AND qty = $4
        "#,
    )
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(target.source_allocation_id.get())
    .bind(target.quantity.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "source allocation changed during packing",
        ));
    }
    Ok(())
}

async fn decrement_source_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PackTarget,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1, modified = $2
        WHERE tenant_id = $3 AND id = $4 AND deleted IS NULL
          AND qty_on_hand >= $1
        "#,
    )
    .bind(target.quantity.get())
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(target.source_balance_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "source inventory changed during packing",
        ));
    }
    Ok(())
}

async fn upsert_carton_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    target: &PackTarget,
    carton_plate_id: LicensePlateId,
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
    .bind(session.inventory_owner_id.get())
    .bind(occurred_at)
    .bind(session.facility_id)
    .bind(session.packing_location_id)
    .bind(carton_plate_id.get())
    .bind(target.item_batch_id.get())
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(target.quantity.get())
    .fetch_one(&mut **tx)
    .await?;
    InventoryBalanceId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn create_carton_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    target: &PackTarget,
    balance_id: InventoryBalanceId,
    carton_plate_id: LicensePlateId,
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
               inventory_status, allocation_run_id, qty, 'allocated', 'packed'
        FROM inventory_allocations
        WHERE tenant_id = $6 AND inventory_owner_id = $7 AND id = $8
          AND status = 'fulfilled' AND deleted = $1
        RETURNING id
        "#,
    )
    .bind(occurred_at)
    .bind(actor_user_id)
    .bind(balance_id.get())
    .bind(session.packing_location_id)
    .bind(carton_plate_id.get())
    .bind(tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(target.source_allocation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("source allocation could not be packed"))?;
    InventoryAllocationId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_content_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    command: &PackPickedAllocationCommand,
    target: &PackTarget,
    carton_plate_id: LicensePlateId,
    destination_allocation_id: InventoryAllocationId,
    destination_balance_id: InventoryBalanceId,
    transaction_id: i64,
    actor_user_id: i64,
    packed_at: Timestamp,
) -> AppResult<CartonContentId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO carton_contents (
            tenant_id, inventory_owner_id, facility_id, packing_session_id,
            carton_id, order_release_id, order_id, order_item_id,
            reservation_id, packing_session_allocation_id,
            outbound_order_container_id, pick_confirmation_id,
            source_inventory_allocation_id, destination_inventory_allocation_id,
            source_inventory_balance_id, destination_inventory_balance_id,
            source_location_id, destination_location_id,
            source_license_plate_id, destination_license_plate_id,
            item_batch_id, item_id, uom, inventory_status,
            inventory_transaction_id, packed_qty, packed_by_user_id, packed_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17, $18, $19, $20, $21, $22,
            $23, $24, $25, $26, $27, $28
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(session.facility_id)
    .bind(session.id.get())
    .bind(command.carton_id.get())
    .bind(session.order_release_id)
    .bind(session.order_id.get())
    .bind(target.order_item_id)
    .bind(target.reservation_id)
    .bind(target.snapshot_id)
    .bind(target.outbound_order_container_id)
    .bind(target.pick_confirmation_id)
    .bind(target.source_allocation_id.get())
    .bind(destination_allocation_id.get())
    .bind(target.source_balance_id.get())
    .bind(destination_balance_id.get())
    .bind(target.source_location_id.get())
    .bind(session.packing_location_id)
    .bind(target.source_license_plate_id.get())
    .bind(carton_plate_id.get())
    .bind(target.item_batch_id.get())
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(transaction_id)
    .bind(target.quantity.get())
    .bind(actor_user_id)
    .bind(packed_at)
    .fetch_one(&mut **tx)
    .await?;
    CartonContentId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn update_progress_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    revision: wareboxes_domain::OrderRevision,
    packed_qty: i64,
) -> AppResult<PackingProgress> {
    let packed_allocation_count = session
        .packed_allocation_count
        .checked_add(1)
        .ok_or_else(|| AppError::internal("packed allocation count overflow"))?;
    let total_packed_qty = session
        .packed_qty
        .checked_add(packed_qty)
        .ok_or_else(|| AppError::internal("packed quantity overflow"))?;
    let progress = PackingProgress::new(
        session.expected_allocation_count,
        packed_allocation_count,
        session.expected_qty,
        total_packed_qty,
        session.open_carton_count,
        session.closed_carton_count,
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    let updated = sqlx::query(
        r#"
        UPDATE packing_sessions
        SET revision = $1, packed_allocation_count = $2, packed_qty = $3
        WHERE tenant_id = $4 AND id = $5 AND state = 'open' AND revision = $6
        "#,
    )
    .bind(revision.get())
    .bind(packed_allocation_count)
    .bind(total_packed_qty)
    .bind(tenant_id.get())
    .bind(session.id.get())
    .bind(session.revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("packing session changed"));
    }
    Ok(progress)
}
