//! Physical dispatch and receipt execution for an interfacility transfer.

mod readiness;

pub use readiness::execution_readiness;

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::transfer_order::{
    DispatchTransferOrderCommand, DispatchTransferOrderResult, ReceiveTransferOrderCommand,
    ReceiveTransferOrderResult, TransferDispatchLineResult, TransferReceiptLineResult,
    DISPATCH_TRANSFER_ORDER_OPERATION, RECEIVE_TRANSFER_ORDER_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    dispatch_transfer_order, receive_transfer_order, CatalogItemId, FacilityId, InventoryBalanceId,
    InventoryOwnerId, ItemBatchId, LocationId, Timestamp, TransferOrderDispatchId,
    TransferOrderDispatchLineId, TransferOrderId, TransferOrderLineId, TransferOrderReceiptId,
    TransferOrderReceiptLineId, TransferOrderStatus,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};

use super::{
    enqueue_event, internal, lock_scope, lock_visible_order, parse_status,
    require_stored_visible_before_replay, revision,
};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::inventory;
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};

#[derive(Debug)]
struct OrderScope {
    owner_id: InventoryOwnerId,
    source_facility_id: FacilityId,
    destination_facility_id: FacilityId,
    status: TransferOrderStatus,
    revision: wareboxes_domain::TransferOrderRevision,
}

#[derive(Debug, Clone)]
struct StockTarget {
    transfer_order_line_id: TransferOrderLineId,
    source_balance_id: InventoryBalanceId,
    source_location_id: LocationId,
    source_location_barcode: String,
    item_batch_id: ItemBatchId,
    item_id: CatalogItemId,
    uom: String,
    lot: Option<String>,
    expiration: Option<Timestamp>,
    serial: Option<String>,
    status: InventoryStatus,
    quantity: i64,
}

#[derive(Debug, Clone)]
struct DispatchedTarget {
    dispatch_line_id: TransferOrderDispatchLineId,
    transfer_order_line_id: TransferOrderLineId,
    transit_balance_id: InventoryBalanceId,
    transit_location_id: LocationId,
    item_batch_id: ItemBatchId,
    item_id: CatalogItemId,
    uom: String,
    lot: Option<String>,
    expiration: Option<Timestamp>,
    serial: Option<String>,
    status: InventoryStatus,
    quantity: i64,
}

pub async fn dispatch(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &DispatchTransferOrderCommand,
) -> AppResult<DispatchTransferOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, DISPATCH_TRANSFER_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<DispatchTransferOrderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = lock_visible_order(&mut tx, access, &scope, command.transfer_order_id).await?;
    let order = order_scope(&row)?;
    if order.revision != command.expected_revision {
        return Err(AppError::conflict(
            "transfer order changed; refresh before dispatching",
        ));
    }
    let resulting_revision = dispatch_transfer_order(order.status, order.revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    lock_scope(
        &mut tx,
        access,
        order.owner_id.get(),
        order.source_facility_id.get(),
        order.destination_facility_id.get(),
    )
    .await?;
    let transit_barcode = command
        .execution
        .observed_transit_location_barcode()
        .as_str();
    lock_execution_location(
        &mut tx,
        access.tenant_id.get(),
        command.execution.transit_location_id().get(),
        order.source_facility_id.get(),
        transit_barcode,
        ExecutionLocationKind::Transit,
    )
    .await?;
    let targets = lock_dispatch_targets(&mut tx, access, &order, command).await?;
    let dispatched_at = now_iso();
    let transaction_id = begin_journal(
        &mut tx,
        access,
        context,
        &prepared,
        &order,
        JournalDescriptor {
            transfer_order_id: command.transfer_order_id,
            transaction_type: InventoryTransactionType::Move,
            operation: DISPATCH_TRANSFER_ORDER_OPERATION,
            reason: "transfer order dispatched to in-transit staging",
        },
    )
    .await?;

    let mut moved = Vec::with_capacity(targets.len());
    for target in &targets {
        decrement_balance(
            &mut tx,
            access.tenant_id.get(),
            order.owner_id.get(),
            order.source_facility_id.get(),
            target.source_balance_id.get(),
            target.quantity,
            dispatched_at,
        )
        .await?;
        let transit_balance_id = increment_loose_balance(
            &mut tx,
            access.tenant_id.get(),
            order.owner_id.get(),
            order.source_facility_id.get(),
            command.execution.transit_location_id().get(),
            target,
            dispatched_at,
        )
        .await?;
        append_move_entries(
            &mut tx,
            access,
            order.owner_id,
            order.source_facility_id,
            transaction_id,
            target.source_location_id,
            command.execution.transit_location_id(),
            target.item_batch_id,
            target.status,
            target.quantity,
        )
        .await?;
        moved.push((target.clone(), transit_balance_id));
    }

    let total_dispatched_quantity = sum_quantities(targets.iter().map(|target| target.quantity))?;
    let selection_count = i64::try_from(targets.len())
        .map_err(|_| AppError::bad_request("transfer dispatch selection count exceeds i64"))?;
    let dispatch_id = TransferOrderDispatchId::new(
        sqlx::query_scalar(
            r#"INSERT INTO transfer_order_dispatches
               (tenant_id,inventory_owner_id,source_facility_id,destination_facility_id,
                transfer_order_id,expected_revision,resulting_revision,transit_location_id,
                transit_location_barcode,inventory_transaction_id,selection_count,
                total_dispatched_quantity,dispatched_by_user_id,dispatched_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
               RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(order.owner_id.get())
        .bind(order.source_facility_id.get())
        .bind(order.destination_facility_id.get())
        .bind(command.transfer_order_id.get())
        .bind(order.revision.get())
        .bind(resulting_revision.get())
        .bind(command.execution.transit_location_id().get())
        .bind(transit_barcode)
        .bind(transaction_id)
        .bind(selection_count)
        .bind(total_dispatched_quantity)
        .bind(context.actor_id.get())
        .bind(dispatched_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    let mut result_lines = Vec::with_capacity(moved.len());
    for (target, transit_balance_id) in moved {
        let dispatch_line_id = TransferOrderDispatchLineId::new(
            sqlx::query_scalar(
                r#"INSERT INTO transfer_order_dispatch_lines
                   (tenant_id,inventory_owner_id,source_facility_id,destination_facility_id,
                    transfer_order_id,transfer_order_dispatch_id,transfer_order_line_id,
                    source_inventory_balance_id,source_location_id,transit_inventory_balance_id,
                    transit_location_id,item_batch_id,item_id,uom,lot,expiration,serial,
                    inventory_status,quantity,observed_source_location_barcode)
                   VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,
                           $17,$18,$19,$20) RETURNING id"#,
            )
            .bind(access.tenant_id.get())
            .bind(order.owner_id.get())
            .bind(order.source_facility_id.get())
            .bind(order.destination_facility_id.get())
            .bind(command.transfer_order_id.get())
            .bind(dispatch_id.get())
            .bind(target.transfer_order_line_id.get())
            .bind(target.source_balance_id.get())
            .bind(target.source_location_id.get())
            .bind(transit_balance_id.get())
            .bind(command.execution.transit_location_id().get())
            .bind(target.item_batch_id.get())
            .bind(target.item_id.get())
            .bind(&target.uom)
            .bind(&target.lot)
            .bind(target.expiration)
            .bind(&target.serial)
            .bind(target.status.as_str())
            .bind(target.quantity)
            .bind(&target.source_location_barcode)
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(internal)?;
        result_lines.push(TransferDispatchLineResult {
            dispatch_line_id,
            transfer_order_line_id: target.transfer_order_line_id,
            source_inventory_balance_id: target.source_balance_id,
            source_location_id: target.source_location_id,
            transit_inventory_balance_id: transit_balance_id,
            item_batch_id: target.item_batch_id,
            item_id: target.item_id,
            uom: target.uom,
            lot: target.lot,
            expiration: target.expiration,
            serial: target.serial,
            inventory_status: target.status.as_str().to_owned(),
            quantity: target.quantity,
        });
    }
    sqlx::query(
        r#"UPDATE transfer_orders
           SET status='in_transit',revision=$3,dispatched_by_user_id=$4,dispatched_at=$5
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.transfer_order_id.get())
    .bind(resulting_revision.get())
    .bind(context.actor_id.get())
    .bind(dispatched_at)
    .execute(&mut *tx)
    .await?;
    let result = DispatchTransferOrderResult {
        dispatch_id,
        transfer_order_id: command.transfer_order_id,
        previous_status: order.status,
        status: TransferOrderStatus::InTransit,
        revision: resulting_revision,
        transit_location_id: command.execution.transit_location_id(),
        transit_location_barcode: transit_barcode.to_owned(),
        inventory_transaction_id: transaction_id,
        lines: result_lines,
        total_dispatched_quantity,
        dispatched_by: context.actor_id,
        dispatched_at,
    };
    enqueue_event(
        &mut tx,
        access,
        context,
        order.owner_id,
        order.source_facility_id,
        &result.transfer_order_id,
        result.revision,
        "dispatched",
        "inventory.transfer_order.dispatched",
        serde_json::json!({
            "dispatch_id": result.dispatch_id.get(),
            "transfer_order_id": result.transfer_order_id.get(),
            "inventory_transaction_id": transaction_id,
            "transit_location_id": result.transit_location_id.get(),
            "destination_facility_id": order.destination_facility_id.get(),
            "status": result.status.as_str(), "revision": result.revision.get(),
            "selection_count": result.lines.len(),
            "total_dispatched_quantity": result.total_dispatched_quantity,
            "dispatched_by": result.dispatched_by.get(), "dispatched_at": result.dispatched_at,
        }),
        result.dispatched_at,
    )
    .await?;
    insert_result(
        &mut tx,
        &prepared.completed_result(&result, Some(transaction_id))?,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn receive(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReceiveTransferOrderCommand,
) -> AppResult<ReceiveTransferOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RECEIVE_TRANSFER_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ReceiveTransferOrderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = lock_visible_order(&mut tx, access, &scope, command.transfer_order_id).await?;
    let order = order_scope(&row)?;
    if order.revision != command.expected_revision {
        return Err(AppError::conflict(
            "transfer order changed; refresh before receiving",
        ));
    }
    let resulting_revision = receive_transfer_order(order.status, order.revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    lock_scope(
        &mut tx,
        access,
        order.owner_id.get(),
        order.source_facility_id.get(),
        order.destination_facility_id.get(),
    )
    .await?;
    lock_execution_location(
        &mut tx,
        access.tenant_id.get(),
        command.destination_location_id.get(),
        order.destination_facility_id.get(),
        command.observed_destination_location_barcode.as_str(),
        ExecutionLocationKind::Receiving,
    )
    .await?;
    let (dispatch_id, targets) = lock_receipt_targets(&mut tx, access, &order, command).await?;
    for target in &targets {
        inventory::ensure_location_accepts_batch_tx(
            &mut tx,
            access.tenant_id,
            order.owner_id.get(),
            command.destination_location_id.get(),
            target.item_batch_id.get(),
        )
        .await?;
    }
    let received_at = now_iso();
    let transaction_id = begin_journal(
        &mut tx,
        access,
        context,
        &prepared,
        &order,
        JournalDescriptor {
            transfer_order_id: command.transfer_order_id,
            transaction_type: InventoryTransactionType::Transfer,
            operation: RECEIVE_TRANSFER_ORDER_OPERATION,
            reason: "transfer order received at destination facility",
        },
    )
    .await?;
    let total_received_quantity = sum_quantities(targets.iter().map(|target| target.quantity))?;
    let line_count = i64::try_from(targets.len())
        .map_err(|_| AppError::bad_request("transfer receipt line count exceeds i64"))?;
    let receipt_id = TransferOrderReceiptId::new(
        sqlx::query_scalar(
            r#"INSERT INTO transfer_order_receipts
               (tenant_id,inventory_owner_id,source_facility_id,destination_facility_id,
                transfer_order_id,transfer_order_dispatch_id,expected_revision,
                resulting_revision,destination_location_id,destination_location_barcode,
                inventory_transaction_id,line_count,total_received_quantity,
                received_by_user_id,received_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
               RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(order.owner_id.get())
        .bind(order.source_facility_id.get())
        .bind(order.destination_facility_id.get())
        .bind(command.transfer_order_id.get())
        .bind(dispatch_id.get())
        .bind(order.revision.get())
        .bind(resulting_revision.get())
        .bind(command.destination_location_id.get())
        .bind(command.observed_destination_location_barcode.as_str())
        .bind(transaction_id)
        .bind(line_count)
        .bind(total_received_quantity)
        .bind(context.actor_id.get())
        .bind(received_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    let mut result_lines = Vec::with_capacity(targets.len());
    for target in targets {
        decrement_balance(
            &mut tx,
            access.tenant_id.get(),
            order.owner_id.get(),
            order.source_facility_id.get(),
            target.transit_balance_id.get(),
            target.quantity,
            received_at,
        )
        .await?;
        let destination_balance_id = increment_received_balance(
            &mut tx,
            access.tenant_id.get(),
            order.owner_id.get(),
            order.destination_facility_id.get(),
            command.destination_location_id.get(),
            &target,
            received_at,
        )
        .await?;
        append_receipt_entries(
            &mut tx,
            access,
            &order,
            transaction_id,
            &target,
            command.destination_location_id,
        )
        .await?;
        let receipt_line_id = TransferOrderReceiptLineId::new(
            sqlx::query_scalar(
                r#"INSERT INTO transfer_order_receipt_lines
                   (tenant_id,inventory_owner_id,source_facility_id,destination_facility_id,
                    transfer_order_id,transfer_order_receipt_id,transfer_order_dispatch_line_id,
                    transfer_order_line_id,transit_inventory_balance_id,transit_location_id,
                    destination_inventory_balance_id,destination_location_id,item_batch_id,
                    item_id,uom,lot,expiration,serial,inventory_status,quantity)
                   VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,
                           $17,$18,$19,$20) RETURNING id"#,
            )
            .bind(access.tenant_id.get())
            .bind(order.owner_id.get())
            .bind(order.source_facility_id.get())
            .bind(order.destination_facility_id.get())
            .bind(command.transfer_order_id.get())
            .bind(receipt_id.get())
            .bind(target.dispatch_line_id.get())
            .bind(target.transfer_order_line_id.get())
            .bind(target.transit_balance_id.get())
            .bind(target.transit_location_id.get())
            .bind(destination_balance_id.get())
            .bind(command.destination_location_id.get())
            .bind(target.item_batch_id.get())
            .bind(target.item_id.get())
            .bind(&target.uom)
            .bind(&target.lot)
            .bind(target.expiration)
            .bind(&target.serial)
            .bind(target.status.as_str())
            .bind(target.quantity)
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(internal)?;
        result_lines.push(TransferReceiptLineResult {
            receipt_line_id,
            dispatch_line_id: target.dispatch_line_id,
            transfer_order_line_id: target.transfer_order_line_id,
            transit_inventory_balance_id: target.transit_balance_id,
            destination_inventory_balance_id: destination_balance_id,
            item_batch_id: target.item_batch_id,
            item_id: target.item_id,
            uom: target.uom,
            lot: target.lot,
            expiration: target.expiration,
            serial: target.serial,
            inventory_status: target.status.as_str().to_owned(),
            quantity: target.quantity,
        });
    }
    sqlx::query(
        r#"UPDATE transfer_orders
           SET status='received',revision=$3,received_by_user_id=$4,received_at=$5
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.transfer_order_id.get())
    .bind(resulting_revision.get())
    .bind(context.actor_id.get())
    .bind(received_at)
    .execute(&mut *tx)
    .await?;
    let result = ReceiveTransferOrderResult {
        receipt_id,
        transfer_order_id: command.transfer_order_id,
        previous_status: order.status,
        status: TransferOrderStatus::Received,
        revision: resulting_revision,
        destination_location_id: command.destination_location_id,
        destination_location_barcode: command
            .observed_destination_location_barcode
            .as_str()
            .to_owned(),
        inventory_transaction_id: transaction_id,
        lines: result_lines,
        total_received_quantity,
        received_by: context.actor_id,
        received_at,
    };
    enqueue_event(
        &mut tx,
        access,
        context,
        order.owner_id,
        order.destination_facility_id,
        &result.transfer_order_id,
        result.revision,
        "received",
        "inventory.transfer_order.received",
        serde_json::json!({
            "receipt_id": result.receipt_id.get(),
            "transfer_order_id": result.transfer_order_id.get(),
            "inventory_transaction_id": transaction_id,
            "source_facility_id": order.source_facility_id.get(),
            "destination_location_id": result.destination_location_id.get(),
            "status": result.status.as_str(), "revision": result.revision.get(),
            "line_count": result.lines.len(),
            "total_received_quantity": result.total_received_quantity,
            "received_by": result.received_by.get(), "received_at": result.received_at,
        }),
        result.received_at,
    )
    .await?;
    insert_result(
        &mut tx,
        &prepared.completed_result(&result, Some(transaction_id))?,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

fn order_scope(row: &sqlx::postgres::PgRow) -> AppResult<OrderScope> {
    Ok(OrderScope {
        owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?,
        source_facility_id: FacilityId::new(row.try_get("source_facility_id")?)
            .map_err(internal)?,
        destination_facility_id: FacilityId::new(row.try_get("destination_facility_id")?)
            .map_err(internal)?,
        status: parse_status(row.try_get::<String, _>("status")?.as_str())?,
        revision: revision(row.try_get("revision")?)?,
    })
}

#[derive(Debug, Clone, Copy)]
enum ExecutionLocationKind {
    Transit,
    Receiving,
}

async fn lock_execution_location(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    location_id: i64,
    facility_id: i64,
    observed_barcode: &str,
    kind: ExecutionLocationKind,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"SELECT facility_id,barcode,active,deleted,pickable,receivable,lower(type) AS type
           FROM locations WHERE tenant_id=$1 AND id=$2 FOR SHARE"#,
    )
    .bind(tenant_id)
    .bind(location_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::bad_request("scanned transfer location was not found"))?;
    let barcode: Option<String> = row.try_get("barcode")?;
    let common_valid = row.try_get::<i64, _>("facility_id")? == facility_id
        && barcode.as_deref() == Some(observed_barcode)
        && row.try_get::<bool, _>("active")?
        && row.try_get::<Option<Timestamp>, _>("deleted")?.is_none();
    let kind_valid = match kind {
        ExecutionLocationKind::Transit => {
            !row.try_get::<bool, _>("pickable")?
                && !row.try_get::<bool, _>("receivable")?
                && row.try_get::<String, _>("type")? == "transfer_in_transit"
        }
        ExecutionLocationKind::Receiving => row.try_get::<bool, _>("receivable")?,
    };
    if common_valid && kind_valid {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "scanned transfer location does not match the executable facility location",
        ))
    }
}

async fn lock_dispatch_targets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    order: &OrderScope,
    command: &DispatchTransferOrderCommand,
) -> AppResult<Vec<StockTarget>> {
    let source_ids = command
        .execution
        .selections()
        .iter()
        .map(|selection| selection.source_inventory_balance_id().get())
        .collect::<Vec<_>>();
    let line_rows = sqlx::query(
        r#"SELECT id,item_id,uom,requested_quantity FROM transfer_order_lines
           WHERE tenant_id=$1 AND transfer_order_id=$2 ORDER BY id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.transfer_order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let lines = line_rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get::<i64, _>("id")?,
                (
                    row.try_get::<i64, _>("item_id")?,
                    row.try_get::<String, _>("uom")?,
                    row.try_get::<i64, _>("requested_quantity")?,
                ),
            ))
        })
        .collect::<AppResult<HashMap<_, _>>>()?;
    let existing_destination_ids: Vec<i64> = sqlx::query_scalar(
        r#"SELECT destination.id FROM inventory_balances source
           JOIN inventory_balances destination
             ON destination.tenant_id=source.tenant_id
            AND destination.inventory_owner_id=source.inventory_owner_id
            AND destination.facility_id=source.facility_id
            AND destination.location_id=$4 AND destination.license_plate_id IS NULL
            AND destination.item_batch_id=source.item_batch_id
            AND destination.uom=source.uom AND destination.status=source.status
           WHERE source.tenant_id=$1 AND source.inventory_owner_id=$2
             AND source.facility_id=$3 AND source.id=ANY($5)"#,
    )
    .bind(access.tenant_id.get())
    .bind(order.owner_id.get())
    .bind(order.source_facility_id.get())
    .bind(command.execution.transit_location_id().get())
    .bind(&source_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut lock_ids = source_ids.clone();
    lock_ids.extend(existing_destination_ids);
    lock_ids.sort_unstable();
    lock_ids.dedup();
    let locked_rows = sqlx::query(
        r#"SELECT balance.id,balance.location_id,location.barcode AS location_barcode,
                  balance.item_batch_id,balance.item_id,balance.uom,balance.status,
                  balance.license_plate_id,balance.qty_on_hand,balance.qty_reserved,
                  balance.qty_held,balance.deleted,batch.lot,batch.expiration,batch.serial,
                  batch.deleted AS batch_deleted,location.active AS location_active,
                  location.deleted AS location_deleted,location.pickable AS location_pickable
           FROM inventory_balances balance
           JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
             AND batch.id=balance.item_batch_id
           JOIN locations location ON location.tenant_id=balance.tenant_id
             AND location.id=balance.location_id
           WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
             AND balance.facility_id=$3 AND balance.id=ANY($4)
           ORDER BY balance.id FOR UPDATE OF balance"#,
    )
    .bind(access.tenant_id.get())
    .bind(order.owner_id.get())
    .bind(order.source_facility_id.get())
    .bind(&lock_ids)
    .fetch_all(&mut **tx)
    .await?;
    let rows = locked_rows
        .iter()
        .map(|row| Ok((row.try_get::<i64, _>("id")?, row)))
        .collect::<AppResult<HashMap<_, _>>>()?;
    let mut line_totals = HashMap::<i64, i64>::new();
    let mut line_batches = HashMap::<i64, i64>::new();
    let mut targets = Vec::with_capacity(command.execution.selections().len());
    for selection in command.execution.selections() {
        let line_id = selection.transfer_order_line_id().get();
        let (line_item_id, line_uom, _) = lines
            .get(&line_id)
            .ok_or_else(|| AppError::bad_request("dispatch line is outside the transfer order"))?;
        let row = rows
            .get(&selection.source_inventory_balance_id().get())
            .ok_or_else(|| AppError::conflict("dispatch source balance is no longer available"))?;
        let free = row
            .try_get::<i64, _>("qty_on_hand")?
            .checked_sub(row.try_get("qty_reserved")?)
            .and_then(|value| value.checked_sub(row.try_get::<i64, _>("qty_held").ok()?))
            .ok_or_else(|| AppError::internal("dispatch source free quantity overflow"))?;
        let item_id: i64 = row.try_get("item_id")?;
        let item_batch_id: i64 = row.try_get("item_batch_id")?;
        let uom: String = row.try_get("uom")?;
        let status = parse_inventory_status(row.try_get::<String, _>("status")?.as_str())?;
        if item_id != *line_item_id
            || uom != *line_uom
            || status != InventoryStatus::Available
            || free < selection.quantity().get()
            || row.try_get::<Option<i64>, _>("license_plate_id")?.is_some()
            || row.try_get::<Option<Timestamp>, _>("deleted")?.is_some()
            || row
                .try_get::<Option<Timestamp>, _>("batch_deleted")?
                .is_some()
            || row
                .try_get::<Option<Timestamp>, _>("location_deleted")?
                .is_some()
            || !row.try_get::<bool, _>("location_active")?
            || !row.try_get::<bool, _>("location_pickable")?
            || row
                .try_get::<Option<String>, _>("location_barcode")?
                .as_deref()
                != Some(selection.observed_source_location_barcode().as_str())
            || row
                .try_get::<Option<Timestamp>, _>("expiration")?
                .is_some_and(|expiration| expiration <= now_iso())
        {
            return Err(AppError::conflict(
                "dispatch source stock or scanned location no longer matches the transfer",
            ));
        }
        if line_batches
            .insert(line_id, item_batch_id)
            .is_some_and(|existing| existing != item_batch_id)
        {
            return Err(AppError::bad_request(
                "each transfer line must dispatch one lot and serial identity",
            ));
        }
        let total = line_totals.entry(line_id).or_default();
        *total = total
            .checked_add(selection.quantity().get())
            .ok_or_else(|| AppError::bad_request("dispatch line quantity exceeds i64"))?;
        targets.push(StockTarget {
            transfer_order_line_id: selection.transfer_order_line_id(),
            source_balance_id: selection.source_inventory_balance_id(),
            source_location_id: LocationId::new(row.try_get("location_id")?).map_err(internal)?,
            source_location_barcode: selection
                .observed_source_location_barcode()
                .as_str()
                .to_owned(),
            item_batch_id: ItemBatchId::new(item_batch_id).map_err(internal)?,
            item_id: CatalogItemId::new(item_id).map_err(internal)?,
            uom,
            lot: row.try_get("lot")?,
            expiration: row.try_get("expiration")?,
            serial: row.try_get("serial")?,
            status,
            quantity: selection.quantity().get(),
        });
    }
    if lines.iter().any(|(line_id, (_, _, requested))| {
        line_totals.get(line_id).copied().unwrap_or_default() != *requested
    }) {
        return Err(AppError::bad_request(
            "dispatch selections must exactly fulfill every transfer order line",
        ));
    }
    Ok(targets)
}

async fn lock_receipt_targets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    order: &OrderScope,
    command: &ReceiveTransferOrderCommand,
) -> AppResult<(TransferOrderDispatchId, Vec<DispatchedTarget>)> {
    let dispatch_id = TransferOrderDispatchId::new(
        sqlx::query_scalar(
            "SELECT id FROM transfer_order_dispatches WHERE tenant_id=$1 AND transfer_order_id=$2",
        )
        .bind(access.tenant_id.get())
        .bind(command.transfer_order_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::conflict("transfer dispatch evidence is missing"))?,
    )
    .map_err(internal)?;
    let rows = sqlx::query(
        r#"SELECT id,transfer_order_line_id,transit_inventory_balance_id,transit_location_id,
                  item_batch_id,item_id,uom,lot,expiration,serial,inventory_status,quantity
           FROM transfer_order_dispatch_lines
           WHERE tenant_id=$1 AND transfer_order_dispatch_id=$2 ORDER BY id"#,
    )
    .bind(access.tenant_id.get())
    .bind(dispatch_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Err(AppError::conflict(
            "transfer dispatch line evidence is missing",
        ));
    }
    let targets = rows
        .iter()
        .map(|row| {
            Ok(DispatchedTarget {
                dispatch_line_id: TransferOrderDispatchLineId::new(row.try_get("id")?)
                    .map_err(internal)?,
                transfer_order_line_id: TransferOrderLineId::new(
                    row.try_get("transfer_order_line_id")?,
                )
                .map_err(internal)?,
                transit_balance_id: InventoryBalanceId::new(
                    row.try_get("transit_inventory_balance_id")?,
                )
                .map_err(internal)?,
                transit_location_id: LocationId::new(row.try_get("transit_location_id")?)
                    .map_err(internal)?,
                item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?).map_err(internal)?,
                item_id: CatalogItemId::new(row.try_get("item_id")?).map_err(internal)?,
                uom: row.try_get("uom")?,
                lot: row.try_get("lot")?,
                expiration: row.try_get("expiration")?,
                serial: row.try_get("serial")?,
                status: parse_inventory_status(
                    row.try_get::<String, _>("inventory_status")?.as_str(),
                )?,
                quantity: row.try_get("quantity")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let transit_ids = targets
        .iter()
        .map(|target| target.transit_balance_id.get())
        .collect::<Vec<_>>();
    let existing_destination_ids: Vec<i64> = sqlx::query_scalar(
        r#"SELECT destination.id FROM inventory_balances transit
           JOIN inventory_balances destination
             ON destination.tenant_id=transit.tenant_id
            AND destination.inventory_owner_id=transit.inventory_owner_id
            AND destination.facility_id=$4 AND destination.location_id=$5
            AND destination.license_plate_id IS NULL
            AND destination.item_batch_id=transit.item_batch_id
            AND destination.uom=transit.uom AND destination.status=transit.status
           WHERE transit.tenant_id=$1 AND transit.inventory_owner_id=$2
             AND transit.facility_id=$3 AND transit.id=ANY($6)"#,
    )
    .bind(access.tenant_id.get())
    .bind(order.owner_id.get())
    .bind(order.source_facility_id.get())
    .bind(order.destination_facility_id.get())
    .bind(command.destination_location_id.get())
    .bind(&transit_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut source_lock_ids = transit_ids.clone();
    source_lock_ids.sort_unstable();
    source_lock_ids.dedup();
    sqlx::query(
        r#"SELECT id FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND facility_id=$3 AND id=ANY($4) ORDER BY id FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(order.owner_id.get())
    .bind(order.source_facility_id.get())
    .bind(&source_lock_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut destination_lock_ids = existing_destination_ids;
    destination_lock_ids.sort_unstable();
    destination_lock_ids.dedup();
    if !destination_lock_ids.is_empty() {
        sqlx::query(
            r#"SELECT id FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
                 AND facility_id=$3 AND id=ANY($4) ORDER BY id FOR UPDATE"#,
        )
        .bind(access.tenant_id.get())
        .bind(order.owner_id.get())
        .bind(order.destination_facility_id.get())
        .bind(&destination_lock_ids)
        .fetch_all(&mut **tx)
        .await?;
    }
    let current_rows = sqlx::query(
        r#"SELECT id,location_id,item_batch_id,item_id,uom,status,license_plate_id,
                  qty_on_hand,qty_reserved,qty_held,deleted
           FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND facility_id=$3 AND id=ANY($4)"#,
    )
    .bind(access.tenant_id.get())
    .bind(order.owner_id.get())
    .bind(order.source_facility_id.get())
    .bind(&source_lock_ids)
    .fetch_all(&mut **tx)
    .await?;
    let current = current_rows
        .iter()
        .map(|row| Ok((row.try_get::<i64, _>("id")?, row)))
        .collect::<AppResult<HashMap<_, _>>>()?;
    for target in &targets {
        let row = current
            .get(&target.transit_balance_id.get())
            .ok_or_else(|| AppError::conflict("in-transit stock is no longer available"))?;
        let free = row
            .try_get::<i64, _>("qty_on_hand")?
            .checked_sub(row.try_get("qty_reserved")?)
            .and_then(|value| value.checked_sub(row.try_get::<i64, _>("qty_held").ok()?))
            .ok_or_else(|| AppError::internal("in-transit free quantity overflow"))?;
        if free < target.quantity
            || row.try_get::<i64, _>("location_id")? != target.transit_location_id.get()
            || row.try_get::<i64, _>("item_batch_id")? != target.item_batch_id.get()
            || row.try_get::<i64, _>("item_id")? != target.item_id.get()
            || row.try_get::<String, _>("uom")? != target.uom
            || row.try_get::<String, _>("status")? != target.status.as_str()
            || row.try_get::<Option<i64>, _>("license_plate_id")?.is_some()
            || row.try_get::<Option<Timestamp>, _>("deleted")?.is_some()
        {
            return Err(AppError::conflict(
                "in-transit stock changed after transfer dispatch",
            ));
        }
    }
    Ok((dispatch_id, targets))
}

struct JournalDescriptor<'a> {
    transfer_order_id: TransferOrderId,
    transaction_type: InventoryTransactionType,
    operation: &'a str,
    reason: &'a str,
}

async fn begin_journal(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    prepared: &PreparedCommand,
    order: &OrderScope,
    descriptor: JournalDescriptor<'_>,
) -> AppResult<i64> {
    inventory_journal::begin_transaction(
        tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility: inventory_journal::owner_facility_scope(
                order.owner_id.get(),
                order.source_facility_id.get(),
            )?,
            actor_user_id: context.actor_id.get(),
            transaction_type: descriptor.transaction_type,
            reason: Some(descriptor.reason),
            reference_type: Some("transfer_order"),
            reference_id: Some(descriptor.transfer_order_id.get()),
            correlation_id: Some(&context.request_id),
            operation: descriptor.operation,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await
}

async fn decrement_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    owner_id: i64,
    facility_id: i64,
    balance_id: i64,
    quantity: i64,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let result = sqlx::query(
        r#"UPDATE inventory_balances SET qty_on_hand=qty_on_hand-$1,modified=$2
           WHERE tenant_id=$3 AND inventory_owner_id=$4 AND facility_id=$5 AND id=$6
             AND license_plate_id IS NULL AND deleted IS NULL
             AND qty_on_hand-qty_reserved-qty_held >= $1"#,
    )
    .bind(quantity)
    .bind(occurred_at)
    .bind(tenant_id)
    .bind(owner_id)
    .bind(facility_id)
    .bind(balance_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict(
            "transfer source inventory changed during execution",
        ))
    }
}

async fn increment_loose_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    owner_id: i64,
    facility_id: i64,
    location_id: i64,
    target: &StockTarget,
    occurred_at: Timestamp,
) -> AppResult<InventoryBalanceId> {
    increment_balance(
        tx,
        tenant_id,
        owner_id,
        facility_id,
        location_id,
        target.item_batch_id,
        target.item_id,
        &target.uom,
        target.status,
        target.quantity,
        occurred_at,
    )
    .await
}

async fn increment_received_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    owner_id: i64,
    facility_id: i64,
    location_id: i64,
    target: &DispatchedTarget,
    occurred_at: Timestamp,
) -> AppResult<InventoryBalanceId> {
    increment_balance(
        tx,
        tenant_id,
        owner_id,
        facility_id,
        location_id,
        target.item_batch_id,
        target.item_id,
        &target.uom,
        target.status,
        target.quantity,
        occurred_at,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn increment_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    owner_id: i64,
    facility_id: i64,
    location_id: i64,
    item_batch_id: ItemBatchId,
    item_id: CatalogItemId,
    uom: &str,
    status: InventoryStatus,
    quantity: i64,
    occurred_at: Timestamp,
) -> AppResult<InventoryBalanceId> {
    let id = sqlx::query_scalar(
        r#"INSERT INTO inventory_balances
             (tenant_id,inventory_owner_id,created,modified,facility_id,location_id,
              license_plate_id,item_batch_id,item_id,uom,status,qty_on_hand,qty_reserved)
           VALUES ($1,$2,$3,$3,$4,$5,NULL,$6,$7,$8,$9,$10,0)
           ON CONFLICT (tenant_id,inventory_owner_id,location_id,item_batch_id,uom,status)
             WHERE license_plate_id IS NULL
           DO UPDATE SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
             modified=excluded.modified,deleted=NULL RETURNING id"#,
    )
    .bind(tenant_id)
    .bind(owner_id)
    .bind(occurred_at)
    .bind(facility_id)
    .bind(location_id)
    .bind(item_batch_id.get())
    .bind(item_id.get())
    .bind(uom)
    .bind(status.as_str())
    .bind(quantity)
    .fetch_one(&mut **tx)
    .await?;
    InventoryBalanceId::new(id).map_err(internal)
}

#[allow(clippy::too_many_arguments)]
async fn append_move_entries(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    transaction_id: i64,
    source_location_id: LocationId,
    destination_location_id: LocationId,
    item_batch_id: ItemBatchId,
    status: InventoryStatus,
    quantity: i64,
) -> AppResult<()> {
    let owner_facility =
        inventory_journal::owner_facility_scope(owner_id.get(), facility_id.get())?;
    for (location_id, quantity_delta) in [
        (source_location_id.get(), -quantity),
        (destination_location_id.get(), quantity),
    ] {
        inventory_journal::append_entry(
            tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id,
                license_plate_id: None,
                item_batch_id: item_batch_id.get(),
                status,
                quantity_delta,
            },
        )
        .await?;
    }
    Ok(())
}

async fn append_receipt_entries(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    order: &OrderScope,
    transaction_id: i64,
    target: &DispatchedTarget,
    destination_location_id: LocationId,
) -> AppResult<()> {
    for (facility_id, location_id, quantity_delta) in [
        (
            order.source_facility_id,
            target.transit_location_id,
            -target.quantity,
        ),
        (
            order.destination_facility_id,
            destination_location_id,
            target.quantity,
        ),
    ] {
        inventory_journal::append_entry(
            tx,
            access.tenant_id,
            inventory_journal::owner_facility_scope(order.owner_id.get(), facility_id.get())?,
            transaction_id,
            &JournalEntry {
                location_id: location_id.get(),
                license_plate_id: None,
                item_batch_id: target.item_batch_id.get(),
                status: target.status,
                quantity_delta,
            },
        )
        .await?;
    }
    Ok(())
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal("stored inventory status is invalid"))
}

fn sum_quantities(mut quantities: impl Iterator<Item = i64>) -> AppResult<i64> {
    quantities.try_fold(0_i64, |total, quantity| {
        total
            .checked_add(quantity)
            .ok_or_else(|| AppError::bad_request("transfer execution quantity exceeds i64"))
    })
}
