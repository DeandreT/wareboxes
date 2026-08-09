//! Atomic waveless release of fully allocated orders into typed RF pick work.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::order_release::{
    ReleaseOrderCommand, ReleaseOrderResult, ORDER_RELEASE_OPERATION,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    release_order as transition_order, InventoryOwnerId, OrderId, OrderReleaseId, OrderRevision,
    OrderStatus, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::{insert_order_activity_tx, next_outbox_sequence_tx};

const PICK_LEASE_SECONDS: i64 = 30 * 60;

#[derive(Debug)]
struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    status: OrderStatus,
    revision: OrderRevision,
    rush: bool,
    ship_by: Option<Timestamp>,
}

#[derive(Debug)]
struct DemandLine {
    id: i64,
    quantity: i64,
    item_id: i64,
    uom: String,
}

#[derive(Debug)]
struct ReleaseAllocation {
    order_item_id: i64,
    reservation_id: i64,
    allocation_id: i64,
    inventory_balance_id: i64,
    source_location_id: i64,
    source_license_plate_id: Option<i64>,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    inventory_status: String,
    quantity: i64,
}

pub async fn release_order(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReleaseOrderCommand,
) -> AppResult<ReleaseOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, ORDER_RELEASE_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "orders").await?;

    if let Some(result) = prepared.replayed::<ReleaseOrderResult>(&mut tx).await? {
        require_replayed_release_visible_tx(
            &mut tx,
            access.tenant_id,
            result.release_id,
            result.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    if !scope.includes_facility(command.facility_id.get()) {
        return Err(AppError::not_found("order release"));
    }
    let order = lock_order_tx(&mut tx, access.tenant_id, command.order_id, &scope).await?;
    if order.revision != command.expected_revision {
        return Err(AppError::conflict(
            "order revision does not match expected revision",
        ));
    }
    let resulting_status =
        transition_order(order.status).map_err(|error| AppError::conflict(error.to_string()))?;
    lock_active_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.facility_id.get(),
    )
    .await?;
    lock_destination_location_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id.get(),
        command.destination_location_id.get(),
    )
    .await?;
    require_no_active_holds_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id,
    )
    .await?;

    let lines = lock_demand_lines_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id,
    )
    .await?;
    let allocations = lock_release_allocations_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id,
        command.facility_id.get(),
    )
    .await?;
    validate_complete_allocation(&lines, &allocations)?;

    let released_quantity = allocations
        .iter()
        .try_fold(0_i64, |total, allocation| {
            total.checked_add(allocation.quantity)
        })
        .ok_or_else(|| AppError::internal("released order quantity exceeds i64"))?;
    let allocation_count = i64::try_from(allocations.len())
        .map_err(|_| AppError::internal("release allocation count exceeds i64"))?;
    let released_at = now_iso();
    let resulting_revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let release_id = insert_release_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        context.actor_id.get(),
        command,
        resulting_revision,
        allocation_count,
        released_quantity,
        released_at,
    )
    .await?;
    insert_pick_work_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        context.actor_id.get(),
        command,
        &order,
        release_id,
        &allocations,
        released_at,
    )
    .await?;
    update_order_tx(
        &mut tx,
        access.tenant_id,
        command.order_id,
        order.status,
        order.revision,
        resulting_status,
        resulting_revision,
        released_at,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id.get(),
        Some(context.actor_id.get()),
        &format!("released order to {} pick task(s)", allocation_count),
    )
    .await?;

    let result = ReleaseOrderResult {
        release_id,
        order_id: command.order_id,
        inventory_owner_id: order.inventory_owner_id,
        facility_id: command.facility_id,
        destination_location_id: command.destination_location_id,
        status: resulting_status,
        revision: resulting_revision,
        allocation_count,
        pick_task_count: allocation_count,
        released_quantity,
        released_at,
    };
    enqueue_release_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command.expected_revision,
        &result,
    )
    .await?;

    Ok(prepared.commit(tx, result).await?)
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, status, revision, rush, ship_by
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
        rush: row.try_get("rush")?,
        ship_by: row.try_get("ship_by")?,
    })
}

async fn lock_active_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
) -> AppResult<()> {
    let exists: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT assignment.id
        FROM inventory_owner_facilities assignment
        INNER JOIN inventory_owners owner
          ON owner.tenant_id = assignment.tenant_id
         AND owner.id = assignment.inventory_owner_id
         AND owner.deleted IS NULL
        INNER JOIN facilities facility
          ON facility.tenant_id = assignment.tenant_id
         AND facility.id = assignment.facility_id
         AND facility.deleted IS NULL
        WHERE assignment.tenant_id = $1
          AND assignment.inventory_owner_id = $2
          AND assignment.facility_id = $3
          AND assignment.deleted IS NULL
        FOR SHARE OF assignment, owner
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id)
    .fetch_optional(&mut **tx)
    .await?;
    if exists.is_none() {
        return Err(AppError::conflict(
            "inventory owner is not active at the selected facility",
        ));
    }
    Ok(())
}

async fn lock_destination_location_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: i64,
    location_id: i64,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT barcode, active, pickable, type
        FROM locations
        WHERE tenant_id = $1 AND facility_id = $2 AND id = $3 AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(location_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick destination location"))?;
    let barcode: Option<String> = row.try_get("barcode")?;
    if !row.try_get::<bool, _>("active")?
        || row.try_get::<bool, _>("pickable")?
        || !matches!(
            row.try_get::<String, _>("type")?
                .to_ascii_lowercase()
                .as_str(),
            "staging" | "packing"
        )
        || barcode.is_none_or(|barcode| barcode.trim().is_empty())
    {
        return Err(AppError::conflict(
            "pick destination must be an active, scannable, non-pickable location",
        ));
    }
    Ok(())
}

async fn require_no_active_holds_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<()> {
    let rows = sqlx::query(
        r#"
        SELECT id FROM order_holds
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND order_id = $3 AND released_at IS NULL
        ORDER BY id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if !rows.is_empty() {
        return Err(AppError::conflict("order has an active hold"));
    }
    Ok(())
}

async fn lock_demand_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<Vec<DemandLine>> {
    let rows = sqlx::query(
        r#"
        SELECT line.id, demand.effective_qty AS qty, line.item_id, line.uom
        FROM order_items line
        INNER JOIN outbound_effective_demand demand
          ON demand.tenant_id=line.tenant_id
         AND demand.inventory_owner_id=line.inventory_owner_id
         AND demand.order_id=line.order_id AND demand.order_item_id=line.id
        WHERE line.tenant_id = $1 AND line.inventory_owner_id = $2
          AND line.order_id = $3 AND line.deleted IS NULL
        ORDER BY line.line_number, line.id FOR SHARE OF line
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Err(AppError::internal("order has no active demand lines"));
    }
    rows.iter()
        .map(|row| {
            Ok(DemandLine {
                id: row.try_get("id")?,
                quantity: row.try_get("qty")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
            })
        })
        .collect()
}

async fn lock_release_allocations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
    facility_id: i64,
) -> AppResult<Vec<ReleaseAllocation>> {
    let rows = sqlx::query(
        r#"
        SELECT reservation.order_item_id, reservation.id AS reservation_id,
               allocation.id AS allocation_id,
               allocation.inventory_balance_id, allocation.location_id,
               allocation.license_plate_id, allocation.item_batch_id,
               allocation.item_id, allocation.uom, allocation.inventory_status,
               allocation.qty,
               balance.facility_id AS balance_facility_id,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_license_plate_id,
               balance.item_batch_id AS balance_item_batch_id,
               balance.item_id AS balance_item_id, balance.uom AS balance_uom,
               balance.status AS balance_status, balance.qty_on_hand,
               balance.qty_reserved, balance.deleted AS balance_deleted,
               location.barcode AS location_barcode,
               location.active AS location_active,
               location.pickable AS location_pickable,
               plate.barcode AS license_plate_barcode,
               plate.deleted AS license_plate_deleted,
               batch.deleted AS batch_deleted,
               item.deleted AS item_deleted,
               EXISTS (
                   SELECT 1 FROM barcodes barcode
                   WHERE barcode.tenant_id = allocation.tenant_id
                     AND barcode.item_id = allocation.item_id
                     AND barcode.deleted IS NULL
                     AND btrim(barcode.name) <> ''
               ) AS item_has_barcode
        FROM inventory_reservations reservation
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id = reservation.tenant_id
         AND allocation.inventory_owner_id = reservation.inventory_owner_id
         AND allocation.reservation_id = reservation.id
         AND allocation.status = 'allocated' AND allocation.deleted IS NULL
         AND allocation.execution_stage = 'pick_source'
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = allocation.tenant_id
         AND balance.inventory_owner_id = allocation.inventory_owner_id
         AND balance.facility_id = allocation.facility_id
         AND balance.id = allocation.inventory_balance_id
        INNER JOIN locations location
          ON location.tenant_id = allocation.tenant_id
         AND location.facility_id = allocation.facility_id
         AND location.id = allocation.location_id
         AND location.deleted IS NULL
        INNER JOIN item_batches batch
          ON batch.tenant_id = allocation.tenant_id
         AND batch.inventory_owner_id = allocation.inventory_owner_id
         AND batch.id = allocation.item_batch_id
        INNER JOIN items item
          ON item.tenant_id = allocation.tenant_id AND item.id = allocation.item_id
        LEFT JOIN license_plates plate
          ON plate.tenant_id = allocation.tenant_id
         AND plate.inventory_owner_id = allocation.inventory_owner_id
         AND plate.facility_id = allocation.facility_id
         AND plate.id = allocation.license_plate_id
        WHERE reservation.tenant_id = $1
          AND reservation.inventory_owner_id = $2
          AND reservation.order_id = $3
          AND reservation.facility_id = $4
          AND reservation.status = 'active' AND reservation.deleted IS NULL
        ORDER BY allocation.location_id, allocation.id
        FOR SHARE OF reservation, allocation, balance, location, batch, item
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .bind(facility_id)
    .fetch_all(&mut **tx)
    .await?;

    let allocations = rows
        .iter()
        .map(|row| {
            let allocation = ReleaseAllocation {
                order_item_id: row.try_get("order_item_id")?,
                reservation_id: row.try_get("reservation_id")?,
                allocation_id: row.try_get("allocation_id")?,
                inventory_balance_id: row.try_get("inventory_balance_id")?,
                source_location_id: row.try_get("location_id")?,
                source_license_plate_id: row.try_get("license_plate_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                inventory_status: row.try_get("inventory_status")?,
                quantity: row.try_get("qty")?,
            };
            let location_barcode: Option<String> = row.try_get("location_barcode")?;
            let license_plate_barcode: Option<String> = row.try_get("license_plate_barcode")?;
            let source_is_valid = row.try_get::<i64, _>("balance_facility_id")? == facility_id
                && row.try_get::<i64, _>("balance_location_id")? == allocation.source_location_id
                && row.try_get::<Option<i64>, _>("balance_license_plate_id")?
                    == allocation.source_license_plate_id
                && row.try_get::<i64, _>("balance_item_batch_id")? == allocation.item_batch_id
                && row.try_get::<i64, _>("balance_item_id")? == allocation.item_id
                && row.try_get::<String, _>("balance_uom")? == allocation.uom
                && row.try_get::<String, _>("balance_status")? == allocation.inventory_status
                && allocation.inventory_status == "available"
                && row
                    .try_get::<Option<Timestamp>, _>("balance_deleted")?
                    .is_none()
                && row.try_get::<i64, _>("qty_on_hand")? >= allocation.quantity
                && row.try_get::<i64, _>("qty_reserved")? >= allocation.quantity
                && row.try_get::<bool, _>("location_active")?
                && row.try_get::<bool, _>("location_pickable")?
                && location_barcode.is_some_and(|value| !value.trim().is_empty())
                && row
                    .try_get::<Option<Timestamp>, _>("batch_deleted")?
                    .is_none()
                && row
                    .try_get::<Option<Timestamp>, _>("item_deleted")?
                    .is_none()
                && row.try_get::<bool, _>("item_has_barcode")?
                && match allocation.source_license_plate_id {
                    Some(_) => {
                        row.try_get::<Option<Timestamp>, _>("license_plate_deleted")?
                            .is_none()
                            && license_plate_barcode.is_some_and(|value| !value.trim().is_empty())
                    }
                    None => true,
                };
            if !source_is_valid {
                return Err(AppError::conflict(
                    "allocated stock is no longer scanner-ready for picking",
                ));
            }
            Ok(allocation)
        })
        .collect::<AppResult<Vec<_>>>()?;
    lock_source_license_plates_tx(tx, tenant_id, inventory_owner_id, facility_id, &allocations)
        .await?;
    Ok(allocations)
}

async fn lock_source_license_plates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    allocations: &[ReleaseAllocation],
) -> AppResult<()> {
    let mut expected = allocations
        .iter()
        .filter_map(|allocation| {
            allocation
                .source_license_plate_id
                .map(|id| (id, allocation.source_location_id))
        })
        .collect::<Vec<_>>();
    expected.sort_unstable();
    expected.dedup();
    if expected.is_empty() {
        return Ok(());
    }
    let ids = expected.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT id, location_id, barcode, deleted
        FROM license_plates
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND facility_id = $3 AND id = ANY($4)
        ORDER BY id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id)
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != expected.len()
        || rows.iter().any(|row| {
            let id = row.try_get::<i64, _>("id").ok();
            let location_id = row.try_get::<i64, _>("location_id").ok();
            let expected_location = id.and_then(|id| {
                expected
                    .iter()
                    .find_map(|(expected_id, location)| (*expected_id == id).then_some(*location))
            });
            location_id != expected_location
                || row
                    .try_get::<Option<Timestamp>, _>("deleted")
                    .ok()
                    .flatten()
                    .is_some()
                || row
                    .try_get::<Option<String>, _>("barcode")
                    .ok()
                    .flatten()
                    .is_none_or(|barcode| barcode.trim().is_empty())
        })
    {
        return Err(AppError::conflict(
            "allocated license plate is no longer scanner-ready for picking",
        ));
    }
    Ok(())
}

fn validate_complete_allocation(
    lines: &[DemandLine],
    allocations: &[ReleaseAllocation],
) -> AppResult<()> {
    if allocations.is_empty() {
        return Err(AppError::conflict("order is not fully allocated"));
    }
    let mut totals = BTreeMap::<i64, i64>::new();
    let mut reservation_ids = BTreeMap::<i64, i64>::new();
    let mut allocation_ids = BTreeSet::new();
    for allocation in allocations {
        let total = totals.entry(allocation.order_item_id).or_default();
        *total = total
            .checked_add(allocation.quantity)
            .ok_or_else(|| AppError::internal("allocated line quantity exceeds i64"))?;
        if reservation_ids
            .insert(allocation.order_item_id, allocation.reservation_id)
            .is_some_and(|existing| existing != allocation.reservation_id)
            || !allocation_ids.insert(allocation.allocation_id)
        {
            return Err(AppError::conflict(
                "order allocation does not have one reservation per demand line",
            ));
        }
    }
    if lines.iter().any(|line| {
        totals.get(&line.id).copied() != Some(line.quantity)
            || allocations.iter().any(|allocation| {
                allocation.order_item_id == line.id
                    && (allocation.item_id != line.item_id || allocation.uom != line.uom)
            })
    }) || totals.len() != lines.len()
    {
        return Err(AppError::conflict("order is not fully allocated"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_release_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    actor_user_id: i64,
    command: &ReleaseOrderCommand,
    resulting_revision: OrderRevision,
    allocation_count: i64,
    released_quantity: i64,
    released_at: Timestamp,
) -> AppResult<OrderReleaseId> {
    let id = sqlx::query_scalar(
        r#"
        INSERT INTO order_releases (
            tenant_id, inventory_owner_id, facility_id, order_id,
            destination_location_id, released_by_user_id, released_at,
            release_mode, expected_revision, resulting_revision,
            allocation_count, released_qty, pick_task_count
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'waveless', $8, $9, $10, $11, $10)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.order_id.get())
    .bind(command.destination_location_id.get())
    .bind(actor_user_id)
    .bind(released_at)
    .bind(command.expected_revision.get())
    .bind(resulting_revision.get())
    .bind(allocation_count)
    .bind(released_quantity)
    .fetch_one(&mut **tx)
    .await?;
    OrderReleaseId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_pick_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    _actor_user_id: i64,
    command: &ReleaseOrderCommand,
    order: &LockedOrder,
    release_id: OrderReleaseId,
    allocations: &[ReleaseAllocation],
    released_at: Timestamp,
) -> AppResult<()> {
    let priority = if order.rush { 100_i64 } else { 0_i64 };
    for (index, allocation) in allocations.iter().enumerate() {
        let travel_sequence = i64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| AppError::internal("pick travel sequence exceeds i64"))?;
        sqlx::query(
            r#"
            INSERT INTO order_release_allocations (
                tenant_id, inventory_owner_id, facility_id, order_release_id,
                order_id, order_item_id, reservation_id, allocation_id,
                inventory_balance_id, source_location_id, source_license_plate_id,
                item_batch_id, item_id, uom, inventory_status, planned_qty,
                travel_sequence
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, $16, $17
            )
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(release_id.get())
        .bind(command.order_id.get())
        .bind(allocation.order_item_id)
        .bind(allocation.reservation_id)
        .bind(allocation.allocation_id)
        .bind(allocation.inventory_balance_id)
        .bind(allocation.source_location_id)
        .bind(allocation.source_license_plate_id)
        .bind(allocation.item_batch_id)
        .bind(allocation.item_id)
        .bind(&allocation.uom)
        .bind(&allocation.inventory_status)
        .bind(allocation.quantity)
        .bind(travel_sequence)
        .execute(&mut **tx)
        .await?;
        let task_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO pick_tasks (
                tenant_id, inventory_owner_id, facility_id, order_release_id,
                order_id, order_item_id, reservation_id, source_allocation_id,
                destination_location_id, created_at,
                status, priority, ship_by, task_timeout_seconds
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                'open', $11, $12, $13
            )
            RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(release_id.get())
        .bind(command.order_id.get())
        .bind(allocation.order_item_id)
        .bind(allocation.reservation_id)
        .bind(allocation.allocation_id)
        .bind(command.destination_location_id.get())
        .bind(released_at)
        .bind(priority)
        .bind(order.ship_by)
        .bind(PICK_LEASE_SECONDS)
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO pick_task_contents (
                tenant_id, inventory_owner_id, facility_id, task_id,
                order_release_id, order_id, order_item_id, reservation_id,
                source_allocation_id, source_inventory_balance_id,
                source_location_id, source_license_plate_id, item_batch_id,
                item_id, uom, inventory_status, planned_qty, travel_sequence, state
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, $16, $17, $18, 'pending'
            )
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(task_id)
        .bind(release_id.get())
        .bind(command.order_id.get())
        .bind(allocation.order_item_id)
        .bind(allocation.reservation_id)
        .bind(allocation.allocation_id)
        .bind(allocation.inventory_balance_id)
        .bind(allocation.source_location_id)
        .bind(allocation.source_license_plate_id)
        .bind(allocation.item_batch_id)
        .bind(allocation.item_id)
        .bind(&allocation.uom)
        .bind(&allocation.inventory_status)
        .bind(allocation.quantity)
        .bind(travel_sequence)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn update_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    expected_status: OrderStatus,
    expected_revision: OrderRevision,
    status: OrderStatus,
    revision: OrderRevision,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders
        SET status = $1, revision = $2, confirmed = COALESCE(confirmed, $3)
        WHERE tenant_id = $4 AND id = $5 AND deleted IS NULL
          AND status = $6 AND revision = $7
        "#,
    )
    .bind(status.as_str())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(expected_status.as_str())
    .bind(expected_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed during release"));
    }
    Ok(())
}

async fn require_replayed_release_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    release_id: OrderReleaseId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id
        FROM order_releases
        WHERE tenant_id = $1 AND id = $2 AND order_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(release_id.get())
    .bind(order_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order release"))?;
    if !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
        || !scope.includes_facility(row.try_get("facility_id")?)
    {
        return Err(AppError::not_found("order release"));
    }
    Ok(())
}

async fn enqueue_release_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    expected_revision: OrderRevision,
    result: &ReleaseOrderResult,
) -> AppResult<()> {
    let event_key = format!("order-release:{}", result.release_id.get());
    let aggregate_id = result.order_id.get().to_string();
    let payload = serde_json::json!({
        "release_id": result.release_id,
        "order_id": result.order_id,
        "inventory_owner_id": result.inventory_owner_id,
        "facility_id": result.facility_id,
        "destination_location_id": result.destination_location_id,
        "release_mode": "waveless",
        "expected_revision": expected_revision,
        "revision": result.revision,
        "allocation_count": result.allocation_count,
        "pick_task_count": result.pick_task_count,
        "released_quantity": result.released_quantity,
        "released_at": result.released_at,
    });
    let ordering_key = format!("order:{}", result.order_id.get());
    let aggregate_sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(result.inventory_owner_id),
            facility_id: Some(result.facility_id),
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: "order.released",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.released_at,
        },
    )
    .await?;
    Ok(())
}
