use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::packing::{
    RemovePackedContentCommand, RemovePackedContentResult, REMOVE_PACKED_CONTENT_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    remove_packed_content as validate_removal, CartonContentRemovalId, CartonStatus,
    InventoryAllocationId, InventoryBalanceId, ItemBatchId, LicensePlateId, LocationId,
    OrderLineId, PackContentRemovalReason, PackQuantity, PackingProgress, TenantId, Timestamp,
    UserId,
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
struct RemovalHint {
    carton_plate_id: i64,
    return_plate_id: i64,
}

#[derive(Debug)]
struct RemovalTarget {
    snapshot_id: i64,
    order_item_id: i64,
    reservation_id: i64,
    source_allocation_id: InventoryAllocationId,
    source_balance_id: InventoryBalanceId,
    source_location_id: LocationId,
    source_plate_id: LicensePlateId,
    source_plate_barcode: String,
    destination_location_id: LocationId,
    destination_plate_id: LicensePlateId,
    destination_plate_barcode: String,
    item_batch_id: ItemBatchId,
    item_id: i64,
    uom: String,
    lot: Option<String>,
    serial: Option<String>,
    inventory_status: InventoryStatus,
    quantity: PackQuantity,
    allocation_position_revision: i64,
    packed_position_revision: i64,
    active_carton_content_count: i64,
}

pub async fn remove_packed_content(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RemovePackedContentCommand,
) -> AppResult<RemovePackedContentResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let fingerprint = serde_json::json!({
        "session_id": command.session_id,
        "carton_id": command.carton_id,
        "content_id": command.content_id,
        "carton_barcode": command.carton_barcode,
        "item_barcode": command.item_barcode,
        "lot_scan": command.lot_scan,
        "serial_scan": command.serial_scan,
        "destination_license_plate_barcode": command.destination_license_plate_barcode,
        "details": command.details,
        "expected_revision": command.expected_revision,
    });
    let prepared = PreparedCommand::new_v1(context, REMOVE_PACKED_CONTENT_OPERATION, &fingerprint)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<RemovePackedContentResult>(&mut tx)
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
    let hint = removal_hint_tx(&mut tx, access.tenant_id, command).await?;
    inventory_locking::lock_license_plates(
        &mut tx,
        access.tenant_id,
        vec![hint.carton_plate_id, hint.return_plate_id],
    )
    .await?;
    let target = lock_removal_target_tx(&mut tx, access.tenant_id, command).await?;
    validate_scans_tx(&mut tx, access.tenant_id, command, &target).await?;
    validate_removal(CartonStatus::Open, target.active_carton_content_count)
        .map_err(|error| AppError::conflict(error.to_string()))?;

    let removal_id: i64 =
        sqlx::query_scalar("SELECT nextval('public.carton_content_removals_id_seq'::regclass)")
            .fetch_one(&mut *tx)
            .await?;
    let owner_facility = inventory_journal::owner_facility_scope(
        session.inventory_owner_id.get(),
        session.facility_id,
    )?;
    let inventory_transaction_id = inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("remove packed content"),
            reference_type: Some("carton_content"),
            reference_id: Some(command.content_id.get()),
            correlation_id: Some(&context.request_id),
            operation: REMOVE_PACKED_CONTENT_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;
    let removed_at = now_iso();
    fulfill_packed_allocation_tx(&mut tx, access.tenant_id, &target, removed_at).await?;
    decrement_packed_balance_tx(&mut tx, access.tenant_id, &target, removed_at).await?;
    let destination_balance_id =
        upsert_return_balance_tx(&mut tx, access.tenant_id, &session, &target, removed_at).await?;
    let destination_allocation_id = create_return_allocation_tx(
        &mut tx,
        access.tenant_id,
        &session,
        &target,
        destination_balance_id,
        context.actor_id.get(),
        removed_at,
    )
    .await?;
    for (location_id, plate_id, delta) in [
        (
            target.source_location_id,
            target.source_plate_id,
            -target.quantity.get(),
        ),
        (
            target.destination_location_id,
            target.destination_plate_id,
            target.quantity.get(),
        ),
    ] {
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            inventory_transaction_id,
            &JournalEntry {
                location_id: location_id.get(),
                license_plate_id: Some(plate_id.get()),
                item_batch_id: target.item_batch_id.get(),
                status: target.inventory_status,
                quantity_delta: delta,
            },
        )
        .await?;
    }
    update_packed_position_tx(
        &mut tx,
        access.tenant_id,
        command.content_id.get(),
        removal_id,
        target.packed_position_revision,
        removed_at,
    )
    .await?;
    update_allocation_position_tx(
        &mut tx,
        access.tenant_id,
        target.snapshot_id,
        destination_allocation_id,
        destination_balance_id,
        &target,
        target.allocation_position_revision,
        removed_at,
    )
    .await?;
    insert_removal_tx(
        &mut tx,
        access.tenant_id,
        &session,
        command,
        &target,
        removal_id,
        inventory_transaction_id,
        destination_allocation_id,
        destination_balance_id,
        revision.get(),
        context.actor_id.get(),
        removed_at,
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

    let removal_id = CartonContentRemovalId::new(removal_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let result = RemovePackedContentResult {
        removal_id,
        content_id: command.content_id,
        session_id: session.id,
        carton_id: command.carton_id,
        order_id,
        order_line_id: OrderLineId::new(target.order_item_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_transaction_id,
        source_inventory_allocation_id: target.source_allocation_id,
        destination_inventory_allocation_id: destination_allocation_id,
        source_inventory_balance_id: target.source_balance_id,
        destination_inventory_balance_id: destination_balance_id,
        source_location_id: target.source_location_id,
        destination_location_id: target.destination_location_id,
        source_license_plate_id: target.source_plate_id,
        destination_license_plate_id: target.destination_plate_id,
        item_batch_id: target.item_batch_id,
        item_id: target.item_id,
        quantity: target.quantity,
        uom: target.uom.clone(),
        details: command.details.clone(),
        removed_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        removed_at,
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
            "removed {} {} from carton {} to tote {}",
            target.quantity.get(),
            target.uom,
            command.carton_id,
            target.destination_plate_barcode
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
        "packing.content_removed",
        &format!(
            "carton-content:{}:removed:{}",
            command.content_id, removal_id
        ),
        serde_json::json!({
            "packing_session_id": session.id,
            "carton_id": command.carton_id,
            "content_id": command.content_id,
            "removal_id": removal_id,
            "order_id": order_id,
            "source_inventory_allocation_id": target.source_allocation_id,
            "destination_inventory_allocation_id": destination_allocation_id,
            "inventory_transaction_id": inventory_transaction_id,
            "quantity": target.quantity,
            "uom": target.uom,
            "reason": reason_code(command.details.reason()),
            "revision": revision,
            "removed_at": removed_at,
        }),
        removed_at,
    )
    .await?;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(inventory_transaction_id))
        .await?)
}

async fn removal_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &RemovePackedContentCommand,
) -> AppResult<RemovalHint> {
    let row = sqlx::query(
        r#"
        SELECT carton.license_plate_id AS carton_plate_id,
               snapshot.source_license_plate_id AS return_plate_id
        FROM carton_contents content
        JOIN cartons carton ON carton.tenant_id=content.tenant_id AND carton.id=content.carton_id
        JOIN packing_session_allocations snapshot
          ON snapshot.tenant_id=content.tenant_id AND snapshot.id=content.packing_session_allocation_id
        JOIN packing_allocation_positions position
          ON position.tenant_id=content.tenant_id
         AND position.packing_session_allocation_id=content.packing_session_allocation_id
         AND position.current_carton_content_id=content.id
        WHERE content.tenant_id=$1 AND content.packing_session_id=$2
          AND content.carton_id=$3 AND content.id=$4
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.session_id.get())
    .bind(command.carton_id.get())
    .bind(command.content_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("active carton content"))?;
    Ok(RemovalHint {
        carton_plate_id: row.try_get("carton_plate_id")?,
        return_plate_id: row.try_get("return_plate_id")?,
    })
}

async fn lock_removal_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &RemovePackedContentCommand,
) -> AppResult<RemovalTarget> {
    let row = sqlx::query(
        r#"
        SELECT snapshot.id AS snapshot_id, snapshot.order_item_id, snapshot.reservation_id,
               snapshot.source_location_id AS return_location_id,
               snapshot.source_license_plate_id AS return_plate_id,
               return_plate.barcode AS return_plate_barcode,
               return_plate.location_id AS return_plate_location_id,
               return_plate.deleted AS return_plate_deleted,
               carton.license_plate_id AS carton_plate_id, carton_plate.barcode AS carton_barcode,
               carton_plate.location_id AS carton_plate_location_id,
               carton_plate.deleted AS carton_plate_deleted, carton.state AS carton_state,
               position.revision AS allocation_position_revision,
               packed.revision AS packed_position_revision, packed.state AS packed_position_state,
               position.current_inventory_allocation_id AS source_allocation_id,
               position.current_inventory_balance_id AS source_balance_id,
               position.current_location_id AS source_location_id,
               position.current_license_plate_id AS source_plate_id,
               content.item_batch_id, content.item_id, content.uom, content.inventory_status,
               content.packed_qty, batch.lot, batch.serial,
               allocation.reservation_id AS allocation_reservation_id,
               allocation.inventory_balance_id AS allocation_balance_id,
               allocation.location_id AS allocation_location_id,
               allocation.license_plate_id AS allocation_plate_id,
               allocation.item_batch_id AS allocation_batch_id,
               allocation.item_id AS allocation_item_id, allocation.uom AS allocation_uom,
               allocation.inventory_status AS allocation_status, allocation.qty AS allocation_qty,
               allocation.status AS allocation_lifecycle,
               allocation.execution_stage AS allocation_execution_stage,
               allocation.deleted AS allocation_deleted,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_plate_id,
               balance.item_batch_id AS balance_batch_id, balance.item_id AS balance_item_id,
               balance.uom AS balance_uom, balance.status AS balance_status,
               balance.qty_on_hand, balance.qty_reserved, balance.deleted AS balance_deleted,
               (SELECT COUNT(*) FROM packing_allocation_positions active
                JOIN carton_contents active_content
                  ON active_content.tenant_id=active.tenant_id
                 AND active_content.id=active.current_carton_content_id
                WHERE active.tenant_id=content.tenant_id
                  AND active_content.carton_id=content.carton_id
                  AND active.state='packed') AS active_carton_content_count
        FROM carton_contents content
        JOIN cartons carton ON carton.tenant_id=content.tenant_id AND carton.id=content.carton_id
        JOIN license_plates carton_plate ON carton_plate.tenant_id=carton.tenant_id
                                        AND carton_plate.id=carton.license_plate_id
        JOIN packing_session_allocations snapshot
          ON snapshot.tenant_id=content.tenant_id AND snapshot.id=content.packing_session_allocation_id
        JOIN license_plates return_plate ON return_plate.tenant_id=snapshot.tenant_id
                                        AND return_plate.id=snapshot.source_license_plate_id
        JOIN packing_allocation_positions position
          ON position.tenant_id=content.tenant_id
         AND position.packing_session_allocation_id=content.packing_session_allocation_id
         AND position.state='packed' AND position.current_carton_content_id=content.id
        JOIN packed_inventory_positions packed
          ON packed.tenant_id=content.tenant_id AND packed.carton_content_id=content.id
        JOIN inventory_allocations allocation
          ON allocation.tenant_id=content.tenant_id
         AND allocation.id=position.current_inventory_allocation_id
        JOIN inventory_balances balance
          ON balance.tenant_id=content.tenant_id
         AND balance.id=position.current_inventory_balance_id
        JOIN item_batches batch ON batch.tenant_id=content.tenant_id
                               AND batch.id=content.item_batch_id
        WHERE content.tenant_id=$1 AND content.packing_session_id=$2
          AND content.carton_id=$3 AND content.id=$4
        FOR UPDATE OF carton, position, packed, allocation, balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.session_id.get())
    .bind(command.carton_id.get())
    .bind(command.content_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("active carton content"))?;
    if row.try_get::<String, _>("carton_state")? != "open" {
        return Err(AppError::conflict("carton is not open"));
    }
    if row.try_get::<String, _>("carton_barcode")? != command.carton_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned carton does not match the content",
        ));
    }
    let target = RemovalTarget {
        snapshot_id: row.try_get("snapshot_id")?,
        order_item_id: row.try_get("order_item_id")?,
        reservation_id: row.try_get("reservation_id")?,
        source_allocation_id: InventoryAllocationId::new(row.try_get("source_allocation_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_balance_id: InventoryBalanceId::new(row.try_get("source_balance_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_id: LocationId::new(row.try_get("source_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_plate_id: LicensePlateId::new(row.try_get("source_plate_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_plate_barcode: row.try_get("carton_barcode")?,
        destination_location_id: LocationId::new(row.try_get("return_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        destination_plate_id: LicensePlateId::new(row.try_get("return_plate_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        destination_plate_barcode: row.try_get("return_plate_barcode")?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        inventory_status: InventoryStatus::parse(&row.try_get::<String, _>("inventory_status")?)
            .ok_or_else(|| AppError::internal("carton content has invalid inventory status"))?,
        quantity: PackQuantity::new(row.try_get("packed_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        allocation_position_revision: row.try_get("allocation_position_revision")?,
        packed_position_revision: row.try_get("packed_position_revision")?,
        active_carton_content_count: row.try_get("active_carton_content_count")?,
    };
    let valid = row.try_get::<Option<i64>, _>("return_plate_location_id")?
        == Some(target.destination_location_id.get())
        && row
            .try_get::<Option<Timestamp>, _>("return_plate_deleted")?
            .is_none()
        && row.try_get::<Option<i64>, _>("carton_plate_location_id")?
            == Some(target.source_location_id.get())
        && row
            .try_get::<Option<Timestamp>, _>("carton_plate_deleted")?
            .is_none()
        && row.try_get::<String, _>("packed_position_state")? == "packed"
        && row.try_get::<i64, _>("allocation_reservation_id")? == target.reservation_id
        && row.try_get::<i64, _>("allocation_balance_id")? == target.source_balance_id.get()
        && row.try_get::<i64, _>("allocation_location_id")? == target.source_location_id.get()
        && row.try_get::<Option<i64>, _>("allocation_plate_id")?
            == Some(target.source_plate_id.get())
        && row.try_get::<i64, _>("allocation_batch_id")? == target.item_batch_id.get()
        && row.try_get::<i64, _>("allocation_item_id")? == target.item_id
        && row.try_get::<String, _>("allocation_uom")? == target.uom
        && row.try_get::<String, _>("allocation_status")? == target.inventory_status.as_str()
        && row.try_get::<i64, _>("allocation_qty")? == target.quantity.get()
        && row.try_get::<String, _>("allocation_lifecycle")? == "allocated"
        && row.try_get::<String, _>("allocation_execution_stage")? == "packed"
        && row
            .try_get::<Option<Timestamp>, _>("allocation_deleted")?
            .is_none()
        && row.try_get::<i64, _>("balance_location_id")? == target.source_location_id.get()
        && row.try_get::<Option<i64>, _>("balance_plate_id")? == Some(target.source_plate_id.get())
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
            "packed content position changed before removal",
        ));
    }
    Ok(target)
}

async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &RemovePackedContentCommand,
    target: &RemovalTarget,
) -> AppResult<()> {
    if target.source_plate_barcode != command.carton_barcode.as_str()
        || target.destination_plate_barcode != command.destination_license_plate_barcode.as_str()
    {
        return Err(AppError::bad_request(
            "scanned carton or destination tote does not match the active content",
        ));
    }
    validate_identity_scan("lot", target.lot.as_deref(), command.lot_scan.as_ref())?;
    validate_identity_scan(
        "serial",
        target.serial.as_deref(),
        command.serial_scan.as_ref(),
    )?;
    let item_matches: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
            SELECT 1 FROM barcodes
            WHERE tenant_id=$1 AND item_id=$2 AND name=$3 AND deleted IS NULL
        )"#,
    )
    .bind(tenant_id.get())
    .bind(target.item_id)
    .bind(command.item_barcode.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if item_matches {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "scanned item does not match the active content",
        ))
    }
}

fn validate_identity_scan(
    label: &str,
    expected: Option<&str>,
    scanned: Option<&wareboxes_domain::PackScanValue>,
) -> AppResult<()> {
    match (expected, scanned) {
        (None, None) => Ok(()),
        (Some(expected), Some(scanned)) if expected == scanned.as_str() => Ok(()),
        (Some(_), None) => Err(AppError::bad_request(format!(
            "{label} scan is required for this content"
        ))),
        _ => Err(AppError::bad_request(format!(
            "scanned {label} does not match the active content"
        ))),
    }
}

async fn fulfill_packed_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &RemovalTarget,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE inventory_allocations
           SET status='fulfilled',modified=$1,deleted=$1
           WHERE tenant_id=$2 AND id=$3 AND status='allocated'
             AND execution_stage='packed' AND deleted IS NULL AND qty=$4"#,
    )
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(target.source_allocation_id.get())
    .bind(target.quantity.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict(
            "packed allocation changed during removal",
        ))
    }
}

async fn decrement_packed_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &RemovalTarget,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE inventory_balances
           SET qty_on_hand=qty_on_hand-$1,modified=$2
           WHERE tenant_id=$3 AND id=$4 AND deleted IS NULL AND qty_on_hand>=$1"#,
    )
    .bind(target.quantity.get())
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(target.source_balance_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict(
            "packed inventory changed during removal",
        ))
    }
}

async fn upsert_return_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    target: &RemovalTarget,
    occurred_at: Timestamp,
) -> AppResult<InventoryBalanceId> {
    let id: i64 = sqlx::query_scalar(
        r#"INSERT INTO inventory_balances (
             tenant_id,inventory_owner_id,created,modified,facility_id,location_id,
             license_plate_id,item_batch_id,item_id,uom,status,qty_on_hand,qty_reserved,qty_held
           ) VALUES ($1,$2,$3,$3,$4,$5,$6,$7,$8,$9,$10,$11,0,0)
           ON CONFLICT (tenant_id,inventory_owner_id,location_id,license_plate_id,
                        item_batch_id,uom,status) WHERE license_plate_id IS NOT NULL
           DO UPDATE SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
                         modified=excluded.modified,deleted=NULL
           RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(occurred_at)
    .bind(session.facility_id)
    .bind(target.destination_location_id.get())
    .bind(target.destination_plate_id.get())
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
async fn create_return_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    target: &RemovalTarget,
    balance_id: InventoryBalanceId,
    actor_user_id: i64,
    occurred_at: Timestamp,
) -> AppResult<InventoryAllocationId> {
    let id: i64 = sqlx::query_scalar(
        r#"INSERT INTO inventory_allocations (
             tenant_id,inventory_owner_id,created,created_by,reservation_id,
             inventory_balance_id,facility_id,location_id,license_plate_id,item_batch_id,
             item_id,uom,inventory_status,allocation_run_id,qty,status,execution_stage
           )
           SELECT tenant_id,inventory_owner_id,$1,$2,reservation_id,$3,facility_id,$4,$5,
                  item_batch_id,item_id,uom,inventory_status,allocation_run_id,qty,
                  'allocated','staged'
           FROM inventory_allocations
           WHERE tenant_id=$6 AND inventory_owner_id=$7 AND id=$8
             AND status='fulfilled' AND deleted=$1
           RETURNING id"#,
    )
    .bind(occurred_at)
    .bind(actor_user_id)
    .bind(balance_id.get())
    .bind(target.destination_location_id.get())
    .bind(target.destination_plate_id.get())
    .bind(tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(target.source_allocation_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("packed allocation could not be returned"))?;
    InventoryAllocationId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn update_packed_position_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    content_id: i64,
    removal_id: i64,
    expected_revision: i64,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE packed_inventory_positions
           SET state='unpacked',current_inventory_allocation_id=NULL,
               current_inventory_balance_id=NULL,current_location_id=NULL,
               current_license_plate_id=NULL,revision=revision+1,positioned_at=$1,
               carton_content_removal_id=$2,unpacked_at=$1
           WHERE tenant_id=$3 AND carton_content_id=$4 AND state='packed'
             AND revision=$5 AND outbound_load_id IS NULL"#,
    )
    .bind(occurred_at)
    .bind(removal_id)
    .bind(tenant_id.get())
    .bind(content_id)
    .bind(expected_revision)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict("packed position changed during removal"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn update_allocation_position_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    snapshot_id: i64,
    allocation_id: InventoryAllocationId,
    balance_id: InventoryBalanceId,
    target: &RemovalTarget,
    expected_revision: i64,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE packing_allocation_positions
           SET state='available',current_carton_content_id=NULL,
               current_inventory_allocation_id=$1,current_inventory_balance_id=$2,
               current_location_id=$3,current_license_plate_id=$4,
               revision=revision+1,positioned_at=$5
           WHERE tenant_id=$6 AND packing_session_allocation_id=$7
             AND state='packed' AND current_carton_content_id IS NOT NULL
             AND revision=$8"#,
    )
    .bind(allocation_id.get())
    .bind(balance_id.get())
    .bind(target.destination_location_id.get())
    .bind(target.destination_plate_id.get())
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(snapshot_id)
    .bind(expected_revision)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict(
            "packing allocation position changed during removal",
        ))
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_removal_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    command: &RemovePackedContentCommand,
    target: &RemovalTarget,
    removal_id: i64,
    transaction_id: i64,
    destination_allocation_id: InventoryAllocationId,
    destination_balance_id: InventoryBalanceId,
    resulting_order_revision: i64,
    actor_user_id: i64,
    occurred_at: Timestamp,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO carton_content_removals (
             id,tenant_id,inventory_owner_id,facility_id,packing_session_id,carton_id,
             carton_content_id,packing_session_allocation_id,order_id,order_item_id,
             reservation_id,inventory_transaction_id,source_inventory_allocation_id,
             destination_inventory_allocation_id,source_inventory_balance_id,
             destination_inventory_balance_id,source_location_id,destination_location_id,
             source_license_plate_id,destination_license_plate_id,item_batch_id,item_id,uom,
             inventory_status,removed_qty,reason_code,note,expected_position_revision,
             resulting_position_revision,expected_packed_position_revision,
             resulting_packed_position_revision,expected_order_revision,
             resulting_order_revision,removed_by_user_id,removed_at
           ) OVERRIDING SYSTEM VALUE VALUES (
             $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,
             $19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35
           )"#,
    )
    .bind(removal_id)
    .bind(tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(session.facility_id)
    .bind(session.id.get())
    .bind(command.carton_id.get())
    .bind(command.content_id.get())
    .bind(target.snapshot_id)
    .bind(session.order_id.get())
    .bind(target.order_item_id)
    .bind(target.reservation_id)
    .bind(transaction_id)
    .bind(target.source_allocation_id.get())
    .bind(destination_allocation_id.get())
    .bind(target.source_balance_id.get())
    .bind(destination_balance_id.get())
    .bind(target.source_location_id.get())
    .bind(target.destination_location_id.get())
    .bind(target.source_plate_id.get())
    .bind(target.destination_plate_id.get())
    .bind(target.item_batch_id.get())
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.inventory_status.as_str())
    .bind(target.quantity.get())
    .bind(reason_code(command.details.reason()))
    .bind(command.details.note().map(|note| note.as_str()))
    .bind(target.allocation_position_revision)
    .bind(target.allocation_position_revision + 1)
    .bind(target.packed_position_revision)
    .bind(target.packed_position_revision + 1)
    .bind(command.expected_revision.get())
    .bind(resulting_order_revision)
    .bind(actor_user_id)
    .bind(occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn update_progress_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    revision: wareboxes_domain::OrderRevision,
    removed_qty: i64,
) -> AppResult<PackingProgress> {
    let packed_count = session
        .packed_allocation_count
        .checked_sub(1)
        .ok_or_else(|| AppError::internal("packed allocation count underflow"))?;
    let packed_qty = session
        .packed_qty
        .checked_sub(removed_qty)
        .ok_or_else(|| AppError::internal("packed quantity underflow"))?;
    let progress = PackingProgress::new(
        session.expected_allocation_count,
        packed_count,
        session.expected_qty,
        packed_qty,
        session.open_carton_count,
        session.closed_carton_count,
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    let updated = sqlx::query(
        r#"UPDATE packing_sessions
           SET revision=$1,packed_allocation_count=$2,packed_qty=$3
           WHERE tenant_id=$4 AND id=$5 AND state='open' AND revision=$6"#,
    )
    .bind(revision.get())
    .bind(packed_count)
    .bind(packed_qty)
    .bind(tenant_id.get())
    .bind(session.id.get())
    .bind(session.revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(progress)
    } else {
        Err(AppError::conflict("packing session changed"))
    }
}

const fn reason_code(reason: PackContentRemovalReason) -> &'static str {
    match reason {
        PackContentRemovalReason::WrongCarton => "wrong_carton",
        PackContentRemovalReason::WrongItem => "wrong_item",
        PackContentRemovalReason::QualityIssue => "quality_issue",
        PackContentRemovalReason::DamagedCarton => "damaged_carton",
        PackContentRemovalReason::Other => "other",
    }
}
