use std::collections::{BTreeMap, BTreeSet};

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbound_load::OutboundLoadShipmentDepartureResult;
use wareboxes_application::shipping::{
    ConfirmShipmentDepartureCommand, ConfirmShipmentDepartureResult,
    CONFIRM_SHIPMENT_DEPARTURE_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    confirm_shipment_departure as validate_departure, CartonId, OrderId, OrderRevision,
    ShipmentCartonIdentity, ShipmentId, ShipmentRevision, ShipmentScanValue, ShipmentStatus,
    ShippingError, TenantId, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::inventory_locking;
use crate::repo::orders::insert_order_activity_tx;

use super::{
    enqueue_order_event_tx, lock_order_tx, lock_shipment_tx, order_hint_for_shipment_tx, positive,
    require_replayed_shipment_id_visible_tx,
};

#[derive(Debug)]
struct DepartureHint {
    allocation_id: i64,
    balance_id: i64,
    reservation_id: i64,
    license_plate_id: i64,
}

#[derive(Debug, Clone)]
struct DepartureCarton {
    shipment_carton_id: i64,
    carton_id: CartonId,
    carton_barcode: ShipmentScanValue,
    license_plate_id: i64,
    packed_qty: i64,
}

#[derive(Debug, Clone)]
struct BalanceDeparture {
    balance_id: i64,
    location_id: i64,
    license_plate_id: i64,
    item_batch_id: i64,
    status: InventoryStatus,
    quantity: i64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct OutboundLoadShipmentTarget {
    pub shipment_id: ShipmentId,
    pub order_id: OrderId,
    pub expected_shipment_revision: ShipmentRevision,
    pub expected_order_revision: OrderRevision,
}

pub async fn confirm_departure(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfirmShipmentDepartureCommand,
) -> AppResult<ConfirmShipmentDepartureResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CONFIRM_SHIPMENT_DEPARTURE_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<ConfirmShipmentDepartureResult>(&mut tx)
        .await?
    {
        require_replayed_shipment_id_visible_tx(
            &mut tx,
            access.tenant_id,
            result.shipment_id,
            result.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id =
        order_hint_for_shipment_tx(&mut tx, access.tenant_id, command.shipment_id).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    let shipment = lock_shipment_tx(&mut tx, access.tenant_id, command.shipment_id, &scope).await?;
    if shipment.order_id != order.id || shipment.inventory_owner_id != order.inventory_owner_id {
        return Err(AppError::not_found("shipment"));
    }
    if shipment.revision != command.expected_shipment_revision
        || order.revision != command.expected_order_revision
    {
        return Err(AppError::conflict("shipment departure revision is stale"));
    }
    let remaining_cartons =
        remaining_departure_cartons_tx(&mut tx, access.tenant_id, shipment.id).await?;
    let carton_identities = remaining_cartons
        .iter()
        .map(|carton| ShipmentCartonIdentity::new(carton.carton_id, carton.carton_barcode.clone()))
        .collect::<Vec<_>>();
    let transition = validate_departure(
        shipment.status,
        order.status,
        &carton_identities,
        &command.scanned_carton_barcodes,
    )
    .map_err(departure_validation_error)?;

    let selected_by_barcode = remaining_cartons
        .iter()
        .map(|carton| (carton.carton_barcode.as_str(), carton))
        .collect::<BTreeMap<_, _>>();
    let selected_cartons = command
        .scanned_carton_barcodes
        .iter()
        .map(|barcode| {
            selected_by_barcode
                .get(barcode.as_str())
                .copied()
                .ok_or_else(|| AppError::bad_request("departure carton scan is not remaining"))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let selected_carton_ids = selected_cartons
        .iter()
        .map(|carton| carton.carton_id)
        .collect::<Vec<_>>();

    let hints = departure_hints_tx(
        &mut tx,
        access.tenant_id,
        shipment.id,
        "packed",
        &selected_carton_ids,
    )
    .await?;
    lock_departure_inventory_tx(&mut tx, access.tenant_id, shipment.order_id.get(), &hints).await?;
    let balances = validate_departure_inventory_tx(
        &mut tx,
        access.tenant_id,
        shipment.id,
        "packed",
        &selected_carton_ids,
        &hints,
    )
    .await?;
    let shipped_qty = balances.values().try_fold(0_i64, |total, balance| {
        total
            .checked_add(balance.quantity)
            .ok_or_else(|| AppError::internal("departure quantity exceeds i64"))
    })?;
    let selected_snapshot_qty = selected_cartons.iter().try_fold(0_i64, |total, carton| {
        total
            .checked_add(carton.packed_qty)
            .ok_or_else(|| AppError::internal("departure quantity exceeds i64"))
    })?;
    if shipped_qty != selected_snapshot_qty {
        return Err(AppError::conflict(
            "packed inventory changed before shipment departure",
        ));
    }
    let manifest_id: i64 = sqlx::query_scalar(
        "SELECT id FROM shipment_manifests WHERE tenant_id = $1 AND shipment_id = $2",
    )
    .bind(access.tenant_id.get())
    .bind(shipment.id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("shipment has no carrier manifest"))?;

    let owner_facility = inventory_journal::owner_facility_scope(
        shipment.inventory_owner_id.get(),
        shipment.facility_id.get(),
    )?;
    let transaction_id = inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Ship,
            reason: Some("confirm shipment departure"),
            reference_type: Some("shipment"),
            reference_id: Some(shipment.id.get()),
            correlation_id: Some(&context.request_id),
            operation: CONFIRM_SHIPMENT_DEPARTURE_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;
    let departed_at = now_iso();
    fulfill_allocations_tx(&mut tx, access.tenant_id, &hints, departed_at).await?;
    consume_balances_tx(
        &mut tx,
        access.tenant_id,
        owner_facility,
        transaction_id,
        &balances,
        departed_at,
    )
    .await?;
    if matches!(transition.shipment_status, ShipmentStatus::Departed) {
        fulfill_reservations_tx(&mut tx, access.tenant_id, shipment.order_id, departed_at).await?;
    }
    depart_license_plates_tx(&mut tx, access.tenant_id, &hints, departed_at).await?;
    depart_packed_positions_tx(
        &mut tx,
        access.tenant_id,
        shipment.id,
        &selected_carton_ids,
        transaction_id,
        departed_at,
    )
    .await?;

    let next_shipment_revision = shipment
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("shipment revision overflow"))?;
    let next_order_revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let scanned_carton_count = i64::try_from(selected_cartons.len())
        .map_err(|_| AppError::internal("departure carton count exceeds i64"))?;
    let cumulative_departed_carton_count = shipment
        .departed_carton_count
        .checked_add(scanned_carton_count)
        .ok_or_else(|| AppError::internal("departed carton count exceeds i64"))?;
    let cumulative_departed_quantity = shipment
        .departed_qty
        .checked_add(shipped_qty)
        .ok_or_else(|| AppError::internal("departed quantity exceeds i64"))?;
    let terminal_departed_at =
        matches!(transition.shipment_status, ShipmentStatus::Departed).then_some(departed_at);
    let shipment_updated = sqlx::query(
        r#"
        UPDATE shipments
        SET state = $1, revision = $2, departed_at = $3,
            departed_carton_count = $4, departed_qty = $5
        WHERE tenant_id = $6 AND id = $7 AND state = $8 AND revision = $9
        "#,
    )
    .bind(transition.shipment_status.as_str())
    .bind(next_shipment_revision.get())
    .bind(terminal_departed_at)
    .bind(cumulative_departed_carton_count)
    .bind(cumulative_departed_quantity)
    .bind(access.tenant_id.get())
    .bind(shipment.id.get())
    .bind(shipment.status.as_str())
    .bind(shipment.revision.get())
    .execute(&mut *tx)
    .await?;
    if shipment_updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "shipment changed during departure confirmation",
        ));
    }
    let order_updated = sqlx::query(
        r#"
        UPDATE orders SET status = $1, revision = $2
        WHERE tenant_id = $3 AND id = $4 AND status = $5 AND revision = $6
        "#,
    )
    .bind(transition.order_status.as_str())
    .bind(next_order_revision.get())
    .bind(access.tenant_id.get())
    .bind(order.id.get())
    .bind(order.status.as_str())
    .bind(order.revision.get())
    .execute(&mut *tx)
    .await?;
    if order_updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "order changed during departure confirmation",
        ));
    }
    let confirmation_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO shipment_confirmations (
            tenant_id, inventory_owner_id, facility_id, shipment_id,
            manifest_id, packing_session_id, order_release_id, order_id,
            inventory_transaction_id, expected_shipment_revision,
            resulting_shipment_revision, expected_order_revision,
            resulting_order_revision, resulting_shipment_state,
            carton_count, shipped_qty,
            confirmed_by_user_id, confirmed_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17, $18
        )
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment.inventory_owner_id.get())
    .bind(shipment.facility_id.get())
    .bind(shipment.id.get())
    .bind(manifest_id)
    .bind(shipment.packing_session_id.get())
    .bind(shipment.order_release_id)
    .bind(shipment.order_id.get())
    .bind(transaction_id)
    .bind(shipment.revision.get())
    .bind(next_shipment_revision.get())
    .bind(order.revision.get())
    .bind(next_order_revision.get())
    .bind(transition.shipment_status.as_str())
    .bind(scanned_carton_count)
    .bind(shipped_qty)
    .bind(context.actor_id.get())
    .bind(departed_at)
    .fetch_one(&mut *tx)
    .await?;
    insert_confirmation_cartons_tx(
        &mut tx,
        access.tenant_id,
        &shipment,
        confirmation_id,
        &selected_cartons,
        departed_at,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "departed {} carton(s) from shipment {} ({} remaining)",
            scanned_carton_count,
            shipment.id,
            shipment.carton_count - cumulative_departed_carton_count
        ),
    )
    .await?;
    let departure_event_key = if matches!(transition.shipment_status, ShipmentStatus::Departed) {
        format!("shipment:{}:departed", shipment.id.get())
    } else {
        format!(
            "shipment:{}:departure:{}",
            shipment.id.get(),
            next_shipment_revision.get()
        )
    };
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
        context.actor_id.get(),
        shipment.order_id,
        if matches!(transition.shipment_status, ShipmentStatus::Departed) {
            "shipping.shipment_departed"
        } else {
            "shipping.shipment_partially_departed"
        },
        &departure_event_key,
        serde_json::json!({
            "shipment_id": shipment.id,
            "order_id": shipment.order_id,
            "inventory_transaction_id": transaction_id,
            "confirmation_id": confirmation_id,
            "carton_count": scanned_carton_count,
            "departure_quantity": shipped_qty,
            "cumulative_departed_carton_count": cumulative_departed_carton_count,
            "cumulative_departed_quantity": cumulative_departed_quantity,
            "remaining_carton_count": shipment.carton_count - cumulative_departed_carton_count,
            "remaining_quantity": shipment.shipped_qty - cumulative_departed_quantity,
            "ordered_quantity": shipment.demand.ordered(),
            "shipped_quantity": shipment.shipped_qty,
            "accepted_short_quantity": shipment.demand.accepted_short(),
            "shipment_revision": next_shipment_revision,
            "order_revision": next_order_revision,
            "departed_at": departed_at,
        }),
        departed_at,
    )
    .await?;
    let result = ConfirmShipmentDepartureResult {
        shipment_id: shipment.id,
        order_id: shipment.order_id,
        shipment_status: transition.shipment_status,
        shipment_revision: next_shipment_revision,
        order_status: transition.order_status,
        order_revision: next_order_revision,
        scanned_carton_count,
        departure_quantity: shipped_qty,
        cumulative_departed_quantity,
        remaining_quantity: shipment.shipped_qty - cumulative_departed_quantity,
        remaining_carton_count: shipment.carton_count - cumulative_departed_carton_count,
        demand: shipment.demand,
        departed_by: positive(context.actor_id.get(), UserId::new)?,
        departed_at,
    };
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await?)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn depart_for_outbound_load_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    scope: &ScopeBindings,
    context: &CommandContext,
    prepared: &PreparedCommand,
    target: OutboundLoadShipmentTarget,
    departed_at: wareboxes_domain::Timestamp,
) -> AppResult<OutboundLoadShipmentDepartureResult> {
    let order = lock_order_tx(tx, access.tenant_id, target.order_id, scope).await?;
    let shipment = lock_shipment_tx(tx, access.tenant_id, target.shipment_id, scope).await?;
    if shipment.order_id != order.id
        || shipment.inventory_owner_id != order.inventory_owner_id
        || shipment.status != ShipmentStatus::Manifested
        || order.status != wareboxes_domain::OrderStatus::AwaitingShipment
    {
        return Err(AppError::conflict(
            "outbound load shipment is no longer ready to depart",
        ));
    }
    if shipment.revision != target.expected_shipment_revision
        || order.revision != target.expected_order_revision
    {
        return Err(AppError::conflict(
            "outbound load shipment departure revision is stale",
        ));
    }
    let remaining_cartons =
        remaining_departure_cartons_tx(tx, access.tenant_id, shipment.id).await?;
    let carton_ids = remaining_cartons
        .iter()
        .map(|carton| carton.carton_id)
        .collect::<Vec<_>>();
    let hints =
        departure_hints_tx(tx, access.tenant_id, shipment.id, "loaded", &carton_ids).await?;
    lock_departure_inventory_tx(tx, access.tenant_id, shipment.order_id.get(), &hints).await?;
    let balances = validate_departure_inventory_tx(
        tx,
        access.tenant_id,
        shipment.id,
        "loaded",
        &carton_ids,
        &hints,
    )
    .await?;
    let shipped_qty = balances.values().try_fold(0_i64, |total, balance| {
        total
            .checked_add(balance.quantity)
            .ok_or_else(|| AppError::internal("departure quantity exceeds i64"))
    })?;
    if shipped_qty != shipment.shipped_qty {
        return Err(AppError::conflict(
            "loaded inventory does not match shipment quantity",
        ));
    }
    let manifest_id: i64 = sqlx::query_scalar(
        "SELECT id FROM shipment_manifests WHERE tenant_id=$1 AND shipment_id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(shipment.id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("shipment has no carrier manifest"))?;
    let owner_facility = inventory_journal::owner_facility_scope(
        shipment.inventory_owner_id.get(),
        shipment.facility_id.get(),
    )?;
    let transaction_id = inventory_journal::begin_batched_transaction_at(
        tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Ship,
            reason: Some("confirm outbound load shipment departure"),
            reference_type: Some("shipment"),
            reference_id: Some(shipment.id.get()),
            correlation_id: Some(&context.request_id),
            operation: CONFIRM_SHIPMENT_DEPARTURE_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
        departed_at,
    )
    .await?;
    fulfill_allocations_tx(tx, access.tenant_id, &hints, departed_at).await?;
    consume_balances_tx(
        tx,
        access.tenant_id,
        owner_facility,
        transaction_id,
        &balances,
        departed_at,
    )
    .await?;
    fulfill_reservations_tx(tx, access.tenant_id, shipment.order_id, departed_at).await?;
    depart_license_plates_tx(tx, access.tenant_id, &hints, departed_at).await?;
    depart_packed_positions_tx(
        tx,
        access.tenant_id,
        shipment.id,
        &carton_ids,
        transaction_id,
        departed_at,
    )
    .await?;
    let next_shipment_revision = shipment
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("shipment revision overflow"))?;
    let next_order_revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    require_updated(
        sqlx::query(
            "UPDATE shipments SET state='departed',revision=$3,departed_at=$4,departed_carton_count=carton_count,departed_qty=shipped_qty WHERE tenant_id=$1 AND id=$2 AND state='manifested' AND revision=$5",
        )
        .bind(access.tenant_id.get())
        .bind(shipment.id.get())
        .bind(next_shipment_revision.get())
        .bind(departed_at)
        .bind(shipment.revision.get())
        .execute(&mut **tx)
        .await?
        .rows_affected(),
        "shipment changed during outbound load departure",
    )?;
    require_updated(
        sqlx::query(
            "UPDATE orders SET status='shipped',revision=$3 WHERE tenant_id=$1 AND id=$2 AND status='awaiting shipment' AND revision=$4",
        )
        .bind(access.tenant_id.get())
        .bind(order.id.get())
        .bind(next_order_revision.get())
        .bind(order.revision.get())
        .execute(&mut **tx)
        .await?
        .rows_affected(),
        "order changed during outbound load departure",
    )?;
    let confirmation_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO shipment_confirmations (
            tenant_id,inventory_owner_id,facility_id,shipment_id,manifest_id,
            packing_session_id,order_release_id,order_id,inventory_transaction_id,
            expected_shipment_revision,resulting_shipment_revision,
            expected_order_revision,resulting_order_revision,resulting_shipment_state,
            carton_count,shipped_qty,
            confirmed_by_user_id,confirmed_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment.inventory_owner_id.get())
    .bind(shipment.facility_id.get())
    .bind(shipment.id.get())
    .bind(manifest_id)
    .bind(shipment.packing_session_id.get())
    .bind(shipment.order_release_id)
    .bind(shipment.order_id.get())
    .bind(transaction_id)
    .bind(shipment.revision.get())
    .bind(next_shipment_revision.get())
    .bind(order.revision.get())
    .bind(next_order_revision.get())
    .bind(ShipmentStatus::Departed.as_str())
    .bind(shipment.carton_count)
    .bind(shipment.shipped_qty)
    .bind(context.actor_id.get())
    .bind(departed_at)
    .fetch_one(&mut **tx)
    .await?;
    let confirmation_cartons = remaining_cartons.iter().collect::<Vec<_>>();
    insert_confirmation_cartons_tx(
        tx,
        access.tenant_id,
        &shipment,
        confirmation_id,
        &confirmation_cartons,
        departed_at,
    )
    .await?;
    insert_order_activity_tx(
        tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "departed shipment {} on outbound load with {} carton(s)",
            shipment.id, shipment.carton_count
        ),
    )
    .await?;
    enqueue_order_event_tx(
        tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
        context.actor_id.get(),
        shipment.order_id,
        "shipping.shipment_departed",
        &format!("shipment:{}:departed", shipment.id.get()),
        serde_json::json!({
            "shipment_id": shipment.id,
            "order_id": shipment.order_id,
            "inventory_transaction_id": transaction_id,
            "carton_count": shipment.carton_count,
            "ordered_quantity": shipment.demand.ordered(),
            "shipped_quantity": shipment.shipped_qty,
            "accepted_short_quantity": shipment.demand.accepted_short(),
            "shipment_revision": next_shipment_revision,
            "order_revision": next_order_revision,
            "departed_at": departed_at,
        }),
        departed_at,
    )
    .await?;
    Ok(OutboundLoadShipmentDepartureResult {
        shipment_id: shipment.id,
        order_id: shipment.order_id,
        inventory_owner_id: shipment.inventory_owner_id,
        inventory_transaction_id: transaction_id,
        shipment_status: ShipmentStatus::Departed,
        shipment_revision: next_shipment_revision,
        order_status: wareboxes_domain::OrderStatus::Shipped,
        order_revision: next_order_revision,
        demand: shipment.demand,
    })
}

fn require_updated(rows: u64, message: &'static str) -> AppResult<()> {
    if rows == 1 {
        Ok(())
    } else {
        Err(AppError::conflict(message))
    }
}

fn departure_validation_error(error: ShippingError) -> AppError {
    match error {
        ShippingError::DepartureCartonSetMismatch => AppError::bad_request(error.to_string()),
        _ => AppError::conflict(error.to_string()),
    }
}

async fn remaining_departure_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: wareboxes_domain::ShipmentId,
) -> AppResult<Vec<DepartureCarton>> {
    let rows = sqlx::query(
        r#"
        SELECT shipment_carton.id AS shipment_carton_id,
               shipment_carton.carton_id, shipment_carton.carton_barcode,
               shipment_carton.license_plate_id, shipment_carton.packed_qty
        FROM shipment_cartons shipment_carton
        WHERE shipment_carton.tenant_id = $1
          AND shipment_carton.shipment_id = $2
          AND NOT EXISTS (
              SELECT 1
              FROM shipment_confirmation_cartons departed
              WHERE departed.tenant_id = shipment_carton.tenant_id
                AND departed.inventory_owner_id = shipment_carton.inventory_owner_id
                AND departed.shipment_id = shipment_carton.shipment_id
                AND departed.shipment_carton_id = shipment_carton.id
          )
        ORDER BY shipment_carton.sequence, shipment_carton.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(DepartureCarton {
                shipment_carton_id: row.try_get("shipment_carton_id")?,
                carton_id: positive(row.try_get("carton_id")?, CartonId::new)?,
                carton_barcode: ShipmentScanValue::new(row.try_get::<String, _>("carton_barcode")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                license_plate_id: row.try_get("license_plate_id")?,
                packed_qty: row.try_get("packed_qty")?,
            })
        })
        .collect()
}

async fn insert_confirmation_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment: &super::LockedShipment,
    confirmation_id: i64,
    cartons: &[&DepartureCarton],
    departed_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    for (index, carton) in cartons.iter().enumerate() {
        let sequence = i64::try_from(index + 1)
            .map_err(|_| AppError::internal("departure carton sequence exceeds i64"))?;
        sqlx::query(
            r#"
            INSERT INTO shipment_confirmation_cartons (
                tenant_id, inventory_owner_id, facility_id, shipment_id,
                confirmation_id, shipment_carton_id, carton_id,
                license_plate_id, sequence, packed_qty, departed_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(tenant_id.get())
        .bind(shipment.inventory_owner_id.get())
        .bind(shipment.facility_id.get())
        .bind(shipment.id.get())
        .bind(confirmation_id)
        .bind(carton.shipment_carton_id)
        .bind(carton.carton_id.get())
        .bind(carton.license_plate_id)
        .bind(sequence)
        .bind(carton.packed_qty)
        .bind(departed_at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn departure_hints_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: wareboxes_domain::ShipmentId,
    required_state: &str,
    carton_ids: &[CartonId],
) -> AppResult<Vec<DepartureHint>> {
    let rows = sqlx::query(
        r#"
        SELECT position.current_inventory_allocation_id AS allocation_id,
               position.current_inventory_balance_id AS balance_id,
               position.reservation_id,
               position.current_license_plate_id AS license_plate_id
        FROM shipment_cartons shipment_carton
        JOIN packed_inventory_positions position
          ON position.tenant_id=shipment_carton.tenant_id
         AND position.inventory_owner_id=shipment_carton.inventory_owner_id
         AND position.facility_id=shipment_carton.facility_id
         AND position.carton_id=shipment_carton.carton_id
        WHERE shipment_carton.tenant_id=$1 AND shipment_carton.shipment_id=$2
          AND position.state=$3
          AND shipment_carton.carton_id = ANY($4)
        ORDER BY position.current_inventory_allocation_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(required_state)
    .bind(carton_ids.iter().map(|id| id.get()).collect::<Vec<_>>())
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Err(AppError::conflict("shipment has no packed inventory"));
    }
    rows.into_iter()
        .map(|row| {
            Ok(DepartureHint {
                allocation_id: row.try_get("allocation_id")?,
                balance_id: row.try_get("balance_id")?,
                reservation_id: row.try_get("reservation_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
            })
        })
        .collect()
}

async fn lock_departure_inventory_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
    hints: &[DepartureHint],
) -> AppResult<()> {
    inventory_locking::lock_license_plates(
        tx,
        tenant_id,
        hints.iter().map(|hint| hint.license_plate_id).collect(),
    )
    .await?;
    lock_exact_ids_tx(
        tx,
        tenant_id,
        "inventory_allocations",
        hints.iter().map(|hint| hint.allocation_id).collect(),
    )
    .await?;
    lock_exact_ids_tx(
        tx,
        tenant_id,
        "inventory_balances",
        hints.iter().map(|hint| hint.balance_id).collect(),
    )
    .await?;
    let reservations: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM inventory_reservations
        WHERE tenant_id = $1 AND order_id = $2
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    if reservations.is_empty()
        || hints
            .iter()
            .any(|hint| !reservations.contains(&hint.reservation_id))
    {
        return Err(AppError::conflict(
            "shipment departure inventory does not match the order reservations",
        ));
    }
    Ok(())
}

async fn lock_exact_ids_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    table: &str,
    mut expected_ids: Vec<i64>,
) -> AppResult<()> {
    expected_ids.sort_unstable();
    expected_ids.dedup();
    let sql = match table {
        "inventory_allocations" => {
            "SELECT id FROM inventory_allocations WHERE tenant_id = $1 AND id = ANY($2) ORDER BY id FOR UPDATE"
        }
        "inventory_balances" => {
            "SELECT id FROM inventory_balances WHERE tenant_id = $1 AND id = ANY($2) ORDER BY id FOR UPDATE"
        }
        _ => return Err(AppError::internal("unsupported departure lock table")),
    };
    let locked: Vec<i64> = sqlx::query_scalar(sql)
        .bind(tenant_id.get())
        .bind(&expected_ids)
        .fetch_all(&mut **tx)
        .await?;
    if locked != expected_ids {
        return Err(AppError::conflict(
            "shipment inventory changed before departure",
        ));
    }
    Ok(())
}

async fn validate_departure_inventory_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: wareboxes_domain::ShipmentId,
    required_state: &str,
    carton_ids: &[CartonId],
    hints: &[DepartureHint],
) -> AppResult<BTreeMap<i64, BalanceDeparture>> {
    let rows = sqlx::query(
        r#"
        SELECT position.current_inventory_allocation_id AS allocation_id,
               position.current_inventory_balance_id AS balance_id,
               position.current_location_id AS location_id,
               position.current_license_plate_id AS license_plate_id,
               position.item_batch_id, position.inventory_status, position.packed_qty,
               allocation.status AS allocation_status,
               allocation.execution_stage AS allocation_execution_stage,
               allocation.deleted AS allocation_deleted,
               allocation.qty AS allocation_qty,
               balance.qty_on_hand, balance.qty_reserved, balance.qty_held,
               balance.deleted AS balance_deleted,
               plate.location_id AS plate_location_id, plate.deleted AS plate_deleted,
               reservation.status AS reservation_status,
               reservation.deleted AS reservation_deleted
        FROM shipment_cartons shipment_carton
        INNER JOIN packed_inventory_positions position
          ON position.tenant_id=shipment_carton.tenant_id
         AND position.inventory_owner_id=shipment_carton.inventory_owner_id
         AND position.facility_id=shipment_carton.facility_id
         AND position.carton_id=shipment_carton.carton_id
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id = position.tenant_id
         AND allocation.inventory_owner_id = position.inventory_owner_id
         AND allocation.facility_id = position.facility_id
         AND allocation.id = position.current_inventory_allocation_id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = position.tenant_id
         AND balance.inventory_owner_id = position.inventory_owner_id
         AND balance.facility_id = position.facility_id
         AND balance.id = position.current_inventory_balance_id
        INNER JOIN license_plates plate
          ON plate.tenant_id = position.tenant_id
         AND plate.inventory_owner_id = position.inventory_owner_id
         AND plate.facility_id = position.facility_id
         AND plate.id = position.current_license_plate_id
        INNER JOIN inventory_reservations reservation
          ON reservation.tenant_id = position.tenant_id
         AND reservation.inventory_owner_id = position.inventory_owner_id
         AND reservation.facility_id = position.facility_id
         AND reservation.id = position.reservation_id
        WHERE shipment_carton.tenant_id=$1 AND shipment_carton.shipment_id=$2
          AND position.state=$3
          AND shipment_carton.carton_id = ANY($4)
        ORDER BY position.current_inventory_allocation_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(required_state)
    .bind(carton_ids.iter().map(|id| id.get()).collect::<Vec<_>>())
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != hints.len() {
        return Err(AppError::conflict(
            "shipment inventory changed before departure",
        ));
    }
    let mut balances = BTreeMap::<i64, BalanceDeparture>::new();
    for row in rows {
        let quantity: i64 = row.try_get("packed_qty")?;
        let allocation_id: i64 = row.try_get("allocation_id")?;
        let balance_id: i64 = row.try_get("balance_id")?;
        let location_id: i64 = row.try_get("location_id")?;
        let license_plate_id: i64 = row.try_get("license_plate_id")?;
        let item_batch_id: i64 = row.try_get("item_batch_id")?;
        let status_text: String = row.try_get("inventory_status")?;
        let status = InventoryStatus::parse(&status_text)
            .ok_or_else(|| AppError::internal("packed inventory has an invalid status"))?;
        let valid = quantity > 0
            && row.try_get::<String, _>("allocation_status")? == "allocated"
            && row.try_get::<String, _>("allocation_execution_stage")? == "packed"
            && row
                .try_get::<Option<wareboxes_domain::Timestamp>, _>("allocation_deleted")?
                .is_none()
            && row.try_get::<i64, _>("allocation_qty")? == quantity
            && row
                .try_get::<Option<wareboxes_domain::Timestamp>, _>("balance_deleted")?
                .is_none()
            && row.try_get::<Option<i64>, _>("plate_location_id")? == Some(location_id)
            && row
                .try_get::<Option<wareboxes_domain::Timestamp>, _>("plate_deleted")?
                .is_none()
            && row.try_get::<String, _>("reservation_status")? == "active"
            && row
                .try_get::<Option<wareboxes_domain::Timestamp>, _>("reservation_deleted")?
                .is_none();
        if !valid {
            return Err(AppError::conflict(
                "packed allocation changed before shipment departure",
            ));
        }
        let entry = balances.entry(balance_id).or_insert(BalanceDeparture {
            balance_id,
            location_id,
            license_plate_id,
            item_batch_id,
            status,
            quantity: 0,
        });
        if entry.location_id != location_id
            || entry.license_plate_id != license_plate_id
            || entry.item_batch_id != item_batch_id
            || entry.status != status
        {
            return Err(AppError::internal(
                "packed contents disagree with their inventory balance",
            ));
        }
        entry.quantity = entry
            .quantity
            .checked_add(quantity)
            .ok_or_else(|| AppError::internal("packed balance quantity exceeds i64"))?;
        if allocation_id <= 0 {
            return Err(AppError::internal("packed allocation ID is invalid"));
        }
        let qty_on_hand: i64 = row.try_get("qty_on_hand")?;
        let qty_reserved: i64 = row.try_get("qty_reserved")?;
        let qty_held: i64 = row.try_get("qty_held")?;
        if qty_on_hand <= 0 || qty_reserved <= 0 || qty_held != 0 {
            return Err(AppError::conflict(
                "packed balance is not fully available for shipment",
            ));
        }
    }
    for balance in balances.values() {
        let row = sqlx::query(
            "SELECT qty_on_hand, qty_reserved FROM inventory_balances WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id.get())
        .bind(balance.balance_id)
        .fetch_one(&mut **tx)
        .await?;
        if row.try_get::<i64, _>("qty_on_hand")? != balance.quantity
            || row.try_get::<i64, _>("qty_reserved")? != balance.quantity
        {
            return Err(AppError::conflict(
                "packed balance contains inventory outside this shipment",
            ));
        }
    }
    Ok(balances)
}

async fn fulfill_allocations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    hints: &[DepartureHint],
    departed_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    for allocation_id in hints
        .iter()
        .map(|hint| hint.allocation_id)
        .collect::<BTreeSet<_>>()
    {
        let updated = sqlx::query(
            r#"
            UPDATE inventory_allocations
            SET status = 'fulfilled', modified = $1, deleted = $1
            WHERE tenant_id = $2 AND id = $3
              AND status = 'allocated' AND deleted IS NULL
            "#,
        )
        .bind(departed_at)
        .bind(tenant_id.get())
        .bind(allocation_id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "packed allocation changed during shipment departure",
            ));
        }
    }
    Ok(())
}

async fn consume_balances_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_facility: wareboxes_domain::OwnerFacilityScope,
    transaction_id: i64,
    balances: &BTreeMap<i64, BalanceDeparture>,
    departed_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    for balance in balances.values() {
        let updated = sqlx::query(
            r#"
            UPDATE inventory_balances
            SET qty_on_hand = 0, modified = $1, deleted = $1
            WHERE tenant_id = $2 AND id = $3 AND deleted IS NULL
              AND qty_on_hand = $4 AND qty_reserved = 0 AND qty_held = 0
            "#,
        )
        .bind(departed_at)
        .bind(tenant_id.get())
        .bind(balance.balance_id)
        .bind(balance.quantity)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "packed balance changed during shipment departure",
            ));
        }
        inventory_journal::append_entry(
            tx,
            tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: balance.location_id,
                license_plate_id: Some(balance.license_plate_id),
                item_batch_id: balance.item_batch_id,
                status: balance.status,
                quantity_delta: -balance.quantity,
            },
        )
        .await?;
    }
    Ok(())
}

async fn fulfill_reservations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    departed_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_reservations WHERE tenant_id=$1 AND order_id=$2 AND status='active' AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE inventory_reservations
        SET status = 'fulfilled', modified = $1, deleted = $1
        WHERE tenant_id = $2 AND order_id = $3
          AND status = 'active' AND deleted IS NULL
        "#,
    )
    .bind(departed_at)
    .bind(tenant_id.get())
    .bind(order_id.get())
    .execute(&mut **tx)
    .await?;
    if active_count <= 0 || updated.rows_affected() != active_count as u64 {
        return Err(AppError::conflict(
            "inventory reservation changed during shipment departure",
        ));
    }
    Ok(())
}

async fn depart_license_plates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    hints: &[DepartureHint],
    departed_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    for license_plate_id in hints
        .iter()
        .map(|hint| hint.license_plate_id)
        .collect::<BTreeSet<_>>()
    {
        let updated = sqlx::query(
            r#"
            UPDATE license_plates SET location_id = NULL, deleted = $1
            WHERE tenant_id = $2 AND id = $3 AND location_id IS NOT NULL AND deleted IS NULL
            "#,
        )
        .bind(departed_at)
        .bind(tenant_id.get())
        .bind(license_plate_id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "shipment carton changed during departure",
            ));
        }
    }
    Ok(())
}

async fn depart_packed_positions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: wareboxes_domain::ShipmentId,
    carton_ids: &[CartonId],
    inventory_transaction_id: i64,
    departed_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE packed_inventory_positions position
        SET state='departed',current_inventory_allocation_id=NULL,
            current_inventory_balance_id=NULL,current_location_id=NULL,
            current_license_plate_id=NULL,revision=position.revision+1,
            positioned_at=$3,departure_inventory_transaction_id=$4,departed_at=$3
        FROM shipment_cartons shipment_carton
        WHERE shipment_carton.tenant_id=$1 AND shipment_carton.shipment_id=$2
          AND shipment_carton.inventory_owner_id=position.inventory_owner_id
          AND shipment_carton.facility_id=position.facility_id
          AND shipment_carton.carton_id=position.carton_id
          AND position.tenant_id=shipment_carton.tenant_id
          AND position.state IN ('packed','loaded')
          AND shipment_carton.carton_id = ANY($5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(departed_at)
    .bind(inventory_transaction_id)
    .bind(carton_ids.iter().map(|id| id.get()).collect::<Vec<_>>())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::conflict(
            "shipment packed positions changed during departure",
        ));
    }
    Ok(())
}
