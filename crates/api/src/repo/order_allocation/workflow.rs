use std::collections::{HashMap, HashSet};

use super::{
    ActiveReservation, BalanceHint, LockedCandidate, LockedOrder, LockedOrderLine, PlannedLine,
};
use crate::error::{AppError, AppResult};
use sqlx::Row;
use wareboxes_application::order_allocation::{
    AllocationPolicyReadModel, AllocationPolicySource, PlanOrderAllocationCommand,
};
use wareboxes_domain::{
    plan_allocation, AllocationOutcome, AllocationQuantity, AllocationRunId,
    AllocationShortageReason, ConfigurationScope, FacilityId, InventoryAllocationId,
    InventoryBalanceId, InventoryOwnerId, InventoryReservationId, ItemBatchId, LicensePlateId,
    LocationId, OrderId, OrderLineId, OrderRevision, OrderStatus, TenantId, Timestamp,
};

mod events;

pub(super) use events::enqueue_order_allocation_event_tx;
use events::{enqueue_allocation_created_event_tx, enqueue_reservation_created_event_tx};

pub(super) async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, order_key, status, revision
        FROM orders
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    map_locked_order(&row, order_id)
}

pub(super) async fn read_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, order_key, status, revision
        FROM orders
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    map_locked_order(&row, order_id)
}

fn map_locked_order(row: &sqlx::postgres::PgRow, order_id: OrderId) -> AppResult<LockedOrder> {
    let status_value: String = row.try_get("status")?;
    let status = OrderStatus::parse(&status_value).ok_or_else(|| {
        AppError::internal(format!(
            "order {} has invalid status {status_value:?}",
            order_id.get()
        ))
    })?;
    Ok(LockedOrder {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_key: row.try_get("order_key")?,
        status,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

pub(super) async fn lock_active_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let assignment: Option<i64> = sqlx::query_scalar(
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
    .bind(facility_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    assignment
        .map(|_| ())
        .ok_or_else(|| AppError::conflict("facility is not active for the order client"))
}

pub(super) async fn lock_order_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<Vec<LockedOrderLine>> {
    let rows = sqlx::query(
        r#"
        SELECT line.id, line.line_key, line.item_id, line.uom,
               line.qty AS original_qty, demand.backordered_qty, demand.effective_qty
        FROM order_items line
        INNER JOIN outbound_effective_demand demand
          ON demand.tenant_id=line.tenant_id
         AND demand.inventory_owner_id=line.inventory_owner_id
         AND demand.order_id=line.order_id AND demand.order_item_id=line.id
        WHERE line.tenant_id = $1
          AND line.inventory_owner_id = $2
          AND line.order_id = $3
          AND line.deleted IS NULL
        ORDER BY line.line_number, line.id
        FOR UPDATE OF line
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.iter().map(map_order_line).collect()
}

fn map_order_line(row: &sqlx::postgres::PgRow) -> AppResult<LockedOrderLine> {
    Ok(LockedOrderLine {
        id: OrderLineId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        line_key: row.try_get("line_key")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        quantity: AllocationQuantity::new(row.try_get("effective_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

pub(super) async fn lock_active_order_holds_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<i64> {
    let rows = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id
        FROM order_holds
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND order_id = $3
          AND released_at IS NULL
        ORDER BY id
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    i64::try_from(rows.len()).map_err(|_| AppError::internal("active order hold count exceeds i64"))
}

pub(super) async fn lock_active_cross_dock_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<i64> {
    let rows = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT detail.task_id
        FROM cross_dock_tasks detail
        INNER JOIN work_tasks work
          ON work.tenant_id=detail.tenant_id AND work.id=detail.task_id
        WHERE detail.tenant_id=$1
          AND detail.inventory_owner_id=$2
          AND detail.order_id=$3
          AND detail.closed_at IS NULL
          AND work.status IN ('open','assigned','in_progress')
        ORDER BY detail.task_id
        FOR UPDATE OF detail,work
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    i64::try_from(rows.len()).map_err(|_| AppError::internal("active cross-dock count exceeds i64"))
}

pub(super) async fn lock_active_reservations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<HashMap<i64, ActiveReservation>> {
    let rows = sqlx::query(
        r#"
        SELECT reservation.id, reservation.order_item_id, reservation.facility_id,
               reservation.qty - COALESCE(backorder.qty, 0) AS effective_qty
        FROM inventory_reservations reservation
        LEFT JOIN LATERAL (
          SELECT SUM(split_line.newly_backordered_qty)::bigint AS qty
          FROM order_backorder_split_lines split_line
          WHERE split_line.tenant_id=reservation.tenant_id
            AND split_line.inventory_owner_id=reservation.inventory_owner_id
            AND split_line.parent_order_id=reservation.order_id
            AND split_line.parent_order_item_id=reservation.order_item_id
        ) backorder ON true
        WHERE reservation.tenant_id = $1
          AND reservation.inventory_owner_id = $2
          AND reservation.order_id = $3
          AND reservation.deleted IS NULL
          AND reservation.status = 'active'
        ORDER BY reservation.order_item_id, reservation.facility_id, reservation.id
        FOR UPDATE OF reservation
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let mut reservations = HashMap::with_capacity(rows.len());
    for row in &rows {
        let reservation = ActiveReservation {
            id: InventoryReservationId::new(row.try_get("id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            order_line_id: OrderLineId::new(row.try_get("order_item_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            facility_id: FacilityId::new(row.try_get("facility_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            quantity: AllocationQuantity::new(row.try_get("effective_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            allocated_quantity: 0,
        };
        if reservations
            .insert(reservation.order_line_id.get(), reservation)
            .is_some()
        {
            return Err(AppError::conflict(
                "order line has active reservations in multiple facilities",
            ));
        }
    }
    Ok(reservations)
}

pub(super) fn validate_existing_reservations(
    lines: &[LockedOrderLine],
    reservations: &HashMap<i64, ActiveReservation>,
    facility_id: FacilityId,
) -> AppResult<()> {
    let active_line_ids = lines
        .iter()
        .map(|line| line.id.get())
        .collect::<HashSet<_>>();
    if reservations
        .keys()
        .any(|order_line_id| !active_line_ids.contains(order_line_id))
    {
        return Err(AppError::conflict(
            "order has an active reservation for an inactive demand line",
        ));
    }
    for line in lines {
        let Some(reservation) = reservations.get(&line.id.get()) else {
            continue;
        };
        if reservation.facility_id != facility_id {
            return Err(AppError::conflict(
                "order has active reservations in another facility",
            ));
        }
        if reservation.quantity != line.quantity {
            return Err(AppError::conflict(
                "active reservation does not cover full order-line demand",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn create_missing_full_demand_reservations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    command: &PlanOrderAllocationCommand,
    actor_user_id: i64,
    occurred_at: Timestamp,
    lines: &[LockedOrderLine],
    reservations: &mut HashMap<i64, ActiveReservation>,
) -> AppResult<()> {
    for line in lines {
        if reservations.contains_key(&line.id.get()) {
            continue;
        }
        let reservation_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO inventory_reservations
                (tenant_id, inventory_owner_id, created, modified, created_by,
                 order_id, order_item_id, facility_id, item_id, uom, qty, status)
            VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(occurred_at)
        .bind(actor_user_id)
        .bind(command.order_id.get())
        .bind(line.id.get())
        .bind(command.facility_id.get())
        .bind(line.item_id)
        .bind(&line.uom)
        .bind(line.quantity.get())
        .fetch_one(&mut **tx)
        .await?;
        let reservation_id = InventoryReservationId::new(reservation_id)
            .map_err(|error| AppError::internal(error.to_string()))?;
        enqueue_reservation_created_event_tx(
            tx,
            tenant_id,
            inventory_owner_id,
            command,
            actor_user_id,
            line,
            reservation_id,
            occurred_at,
        )
        .await?;
        reservations.insert(
            line.id.get(),
            ActiveReservation {
                id: reservation_id,
                order_line_id: line.id,
                facility_id: command.facility_id,
                quantity: line.quantity,
                allocated_quantity: 0,
            },
        );
    }
    Ok(())
}

pub(super) async fn load_allocated_quantities_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    reservations: &mut HashMap<i64, ActiveReservation>,
) -> AppResult<()> {
    let reservation_ids = reservations
        .values()
        .map(|reservation| reservation.id.get())
        .collect::<Vec<_>>();
    if reservation_ids.is_empty() {
        return Ok(());
    }
    let rows = sqlx::query(
        r#"
        SELECT reservation_id, COALESCE(SUM(qty), 0)::BIGINT AS allocated_qty
        FROM inventory_allocations
        WHERE tenant_id = $1
          AND reservation_id = ANY($2)
          AND deleted IS NULL
          AND status = 'allocated'
        GROUP BY reservation_id
        ORDER BY reservation_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(&reservation_ids)
    .fetch_all(&mut **tx)
    .await?;
    let allocated_by_reservation = rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get("reservation_id")?,
                row.try_get("allocated_qty")?,
            ))
        })
        .collect::<AppResult<HashMap<i64, i64>>>()?;
    for reservation in reservations.values_mut() {
        reservation.allocated_quantity = allocated_by_reservation
            .get(&reservation.id.get())
            .copied()
            .unwrap_or(0);
        if reservation.allocated_quantity < 0
            || reservation.allocated_quantity > reservation.quantity.get()
        {
            return Err(AppError::internal(
                "active allocations do not conserve reservation demand",
            ));
        }
    }
    Ok(())
}

pub(super) fn sum_line_demand(lines: &[LockedOrderLine]) -> AppResult<i64> {
    lines
        .iter()
        .try_fold(0_i64, |total, line| total.checked_add(line.quantity.get()))
        .ok_or_else(|| AppError::internal("order demand quantity exceeds i64"))
}

pub(super) async fn lock_candidate_inventory_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    lines: &[LockedOrderLine],
    occurred_at: Timestamp,
) -> AppResult<HashMap<i64, LockedCandidate>> {
    let item_ids = lines.iter().map(|line| line.item_id).collect::<Vec<_>>();
    let uoms = lines
        .iter()
        .map(|line| line.uom.as_str())
        .collect::<Vec<_>>();
    let hint_rows = sqlx::query(
        r#"
        SELECT balance.id, balance.item_batch_id, balance.location_id,
               balance.license_plate_id
        FROM inventory_balances balance
        INNER JOIN item_batches batch
            ON batch.tenant_id = balance.tenant_id
           AND batch.inventory_owner_id = balance.inventory_owner_id
           AND batch.id = balance.item_batch_id
           AND batch.deleted IS NULL
           AND (batch.expiration IS NULL OR batch.expiration > $6)
        INNER JOIN locations location
            ON location.tenant_id = balance.tenant_id
           AND location.facility_id = balance.facility_id
           AND location.id = balance.location_id
           AND location.deleted IS NULL
           AND location.active
           AND location.pickable
        LEFT JOIN license_plates plate
            ON plate.tenant_id = balance.tenant_id
           AND plate.inventory_owner_id = balance.inventory_owner_id
           AND plate.facility_id = balance.facility_id
           AND plate.id = balance.license_plate_id
        WHERE balance.tenant_id = $1
          AND balance.inventory_owner_id = $2
          AND balance.facility_id = $3
          AND balance.deleted IS NULL
          AND balance.status = 'available'
          AND balance.qty_on_hand - balance.qty_reserved - balance.qty_held > 0
          AND (
              balance.license_plate_id IS NULL
              OR (plate.id IS NOT NULL AND plate.deleted IS NULL)
          )
          AND EXISTS (
              SELECT 1
              FROM UNNEST($4::BIGINT[], $5::TEXT[]) demand(item_id, uom)
              WHERE demand.item_id = balance.item_id
                AND demand.uom = balance.uom
          )
        ORDER BY balance.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .bind(&item_ids)
    .bind(&uoms)
    .bind(occurred_at)
    .fetch_all(&mut **tx)
    .await?;
    let hints = hint_rows
        .iter()
        .map(|row| {
            Ok(BalanceHint {
                inventory_balance_id: InventoryBalanceId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                location_id: LocationId::new(row.try_get("location_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                license_plate_id: row
                    .try_get::<Option<i64>, _>("license_plate_id")?
                    .map(LicensePlateId::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    if hints.is_empty() {
        return Ok(HashMap::new());
    }

    let mut item_batch_ids = hints
        .iter()
        .map(|hint| hint.item_batch_id.get())
        .collect::<Vec<_>>();
    item_batch_ids.sort_unstable();
    item_batch_ids.dedup();
    sqlx::query(
        r#"
        SELECT id
        FROM item_batches
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND id = ANY($3)
        ORDER BY id
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(&item_batch_ids)
    .fetch_all(&mut **tx)
    .await?;

    let mut location_ids = hints
        .iter()
        .map(|hint| hint.location_id.get())
        .collect::<Vec<_>>();
    location_ids.sort_unstable();
    location_ids.dedup();
    sqlx::query(
        r#"
        SELECT id
        FROM locations
        WHERE tenant_id = $1
          AND facility_id = $2
          AND id = ANY($3)
        ORDER BY id
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(&location_ids)
    .fetch_all(&mut **tx)
    .await?;

    let mut license_plate_ids = hints
        .iter()
        .filter_map(|hint| hint.license_plate_id.map(LicensePlateId::get))
        .collect::<Vec<_>>();
    license_plate_ids.sort_unstable();
    license_plate_ids.dedup();
    if !license_plate_ids.is_empty() {
        sqlx::query(
            r#"
            SELECT id
            FROM license_plates
            WHERE tenant_id = $1 AND id = ANY($2)
            ORDER BY id
            FOR UPDATE
            "#,
        )
        .bind(tenant_id.get())
        .bind(&license_plate_ids)
        .fetch_all(&mut **tx)
        .await?;
    }

    let balance_ids = hints
        .iter()
        .map(|hint| hint.inventory_balance_id.get())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT balance.id, balance.license_plate_id, balance.item_batch_id,
               balance.location_id, balance.item_id, balance.uom, balance.status,
               balance.deleted AS balance_deleted, balance.qty_on_hand,
               balance.qty_reserved, balance.qty_held,
               batch.created AS batch_created, batch.deleted AS batch_deleted,
               batch.lot, batch.serial, batch.expiration,
               location.deleted AS location_deleted, location.active AS location_active,
               location.pickable AS location_pickable,
               plate.deleted AS license_plate_deleted
        FROM inventory_balances balance
        INNER JOIN item_batches batch
            ON batch.tenant_id = balance.tenant_id
           AND batch.inventory_owner_id = balance.inventory_owner_id
           AND batch.id = balance.item_batch_id
        INNER JOIN locations location
            ON location.tenant_id = balance.tenant_id
           AND location.facility_id = balance.facility_id
           AND location.id = balance.location_id
        LEFT JOIN license_plates plate
            ON plate.tenant_id = balance.tenant_id
           AND plate.inventory_owner_id = balance.inventory_owner_id
           AND plate.facility_id = balance.facility_id
           AND plate.id = balance.license_plate_id
        WHERE balance.tenant_id = $1 AND balance.id = ANY($2)
        ORDER BY balance.id
        FOR UPDATE OF balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(&balance_ids)
    .fetch_all(&mut **tx)
    .await?;
    let hints_by_balance = hints
        .into_iter()
        .map(|hint| (hint.inventory_balance_id.get(), hint))
        .collect::<HashMap<_, _>>();
    let mut candidates = HashMap::with_capacity(rows.len());
    for row in &rows {
        let candidate = map_locked_candidate(row)?;
        let hint = hints_by_balance
            .get(&candidate.inventory_balance_id.get())
            .ok_or_else(|| AppError::internal("locked an unexpected inventory balance"))?;
        if hint.item_batch_id != candidate.item_batch_id
            || hint.location_id != candidate.location_id
            || hint.license_plate_id != candidate.license_plate_id
        {
            return Err(AppError::conflict(
                "inventory balance dimensions changed while acquiring locks",
            ));
        }
        if candidate.available_quantity(occurred_at)?.is_some() {
            candidates.insert(candidate.inventory_balance_id.get(), candidate);
        }
    }
    Ok(candidates)
}

fn map_locked_candidate(row: &sqlx::postgres::PgRow) -> AppResult<LockedCandidate> {
    Ok(LockedCandidate {
        inventory_balance_id: InventoryBalanceId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        location_id: LocationId::new(row.try_get("location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        license_plate_id: row
            .try_get::<Option<i64>, _>("license_plate_id")?
            .map(LicensePlateId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        received_at: row.try_get("batch_created")?,
        location_deleted: row.try_get("location_deleted")?,
        location_active: row.try_get("location_active")?,
        location_pickable: row.try_get("location_pickable")?,
        batch_deleted: row.try_get("batch_deleted")?,
        license_plate_deleted: row.try_get("license_plate_deleted")?,
        status: row.try_get("status")?,
        balance_deleted: row.try_get("balance_deleted")?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
        qty_held: row.try_get("qty_held")?,
    })
}

pub(super) fn plan_lines(
    lines: &[LockedOrderLine],
    reservations: &HashMap<i64, ActiveReservation>,
    candidates: &HashMap<i64, LockedCandidate>,
    policy: &AllocationPolicyReadModel,
    occurred_at: Timestamp,
) -> AppResult<Vec<PlannedLine>> {
    if policy.allow_partial && policy.require_complete_line {
        return Err(AppError::internal(
            "allocation policy has conflicting partial-line rules",
        ));
    }
    let mut available_by_balance = candidates
        .values()
        .map(|candidate| {
            candidate
                .available_quantity(occurred_at)
                .map(|available| (candidate.inventory_balance_id.get(), available.unwrap_or(0)))
        })
        .collect::<AppResult<HashMap<_, _>>>()?;
    let mut planned_lines = Vec::with_capacity(lines.len());
    for line in lines {
        let reservation = reservations.get(&line.id.get()).ok_or_else(|| {
            AppError::internal("allocation planner did not create an order-line reservation")
        })?;
        let remaining = line
            .quantity
            .get()
            .checked_sub(reservation.allocated_quantity)
            .ok_or_else(|| AppError::internal("allocated quantity exceeds order-line demand"))?;
        let allocations = if remaining == 0 {
            Vec::new()
        } else {
            let demand = AllocationQuantity::new(remaining)
                .map_err(|error| AppError::internal(error.to_string()))?;
            let line_candidates = candidates
                .values()
                .filter(|candidate| candidate.item_id == line.item_id && candidate.uom == line.uom)
                .filter_map(|candidate| {
                    let available = available_by_balance
                        .get(&candidate.inventory_balance_id.get())
                        .copied()
                        .unwrap_or(0);
                    (available > 0).then_some((candidate, available))
                })
                .map(|(candidate, available)| candidate.domain_candidate(available))
                .collect::<AppResult<Vec<_>>>()?;
            let plan = plan_allocation(policy.strategy, demand, line_candidates)
                .map_err(|error| AppError::internal(error.to_string()))?;
            let accepted = !policy.require_complete_line || plan.shortage_quantity() == 0;
            let allocations = if accepted {
                plan.allocations().to_vec()
            } else {
                Vec::new()
            };
            for allocation in &allocations {
                let available = available_by_balance
                    .get_mut(&allocation.inventory_balance_id().get())
                    .ok_or_else(|| {
                        AppError::internal("allocation plan references an unknown balance")
                    })?;
                *available = available
                    .checked_sub(allocation.quantity().get())
                    .ok_or_else(|| {
                        AppError::internal("allocation exceeds shared stock capacity")
                    })?;
            }
            allocations
        };
        let newly_allocated_quantity = allocations
            .iter()
            .try_fold(0_i64, |total, allocation| {
                total.checked_add(allocation.quantity().get())
            })
            .ok_or_else(|| AppError::internal("line allocation quantity exceeds i64"))?;
        let total_allocated_quantity = reservation
            .allocated_quantity
            .checked_add(newly_allocated_quantity)
            .ok_or_else(|| AppError::internal("line allocation quantity exceeds i64"))?;
        let shortage_quantity = line
            .quantity
            .get()
            .checked_sub(total_allocated_quantity)
            .ok_or_else(|| AppError::internal("allocated quantity exceeds order-line demand"))?;
        planned_lines.push(PlannedLine {
            line: line.clone(),
            reservation_id: reservation.id,
            previously_allocated_quantity: reservation.allocated_quantity,
            newly_allocated_quantity,
            total_allocated_quantity,
            shortage_quantity,
            shortage_reason: cumulative_shortage_reason(
                total_allocated_quantity,
                shortage_quantity,
            ),
            allocations,
        });
    }
    if !policy.allow_partial
        && !policy.require_complete_line
        && planned_lines.iter().any(|line| line.shortage_quantity > 0)
    {
        for line in &mut planned_lines {
            line.allocations.clear();
            line.newly_allocated_quantity = 0;
            line.total_allocated_quantity = line.previously_allocated_quantity;
            line.shortage_quantity = line
                .line
                .quantity
                .get()
                .checked_sub(line.previously_allocated_quantity)
                .ok_or_else(|| AppError::internal("allocated quantity exceeds line demand"))?;
            line.shortage_reason =
                cumulative_shortage_reason(line.total_allocated_quantity, line.shortage_quantity);
        }
    }
    Ok(planned_lines)
}

pub(super) fn cumulative_shortage_reason(
    allocated_quantity: i64,
    shortage_quantity: i64,
) -> Option<AllocationShortageReason> {
    if shortage_quantity == 0 {
        None
    } else if allocated_quantity == 0 {
        Some(AllocationShortageReason::NoEligibleInventory)
    } else {
        Some(AllocationShortageReason::InsufficientEligibleInventory)
    }
}

pub(super) fn cumulative_outcome(
    demand_quantity: i64,
    allocated_quantity: i64,
) -> AppResult<AllocationOutcome> {
    if demand_quantity <= 0 || allocated_quantity < 0 || allocated_quantity > demand_quantity {
        return Err(AppError::internal("allocation totals are invalid"));
    }
    Ok(if allocated_quantity == demand_quantity {
        AllocationOutcome::FullyAllocated
    } else if allocated_quantity == 0 {
        AllocationOutcome::NotAllocated
    } else {
        AllocationOutcome::PartiallyAllocated
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_allocation_run_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    actor_user_id: i64,
    command: &PlanOrderAllocationCommand,
    policy: &AllocationPolicyReadModel,
    outcome: AllocationOutcome,
    demand_quantity: i64,
    allocated_quantity: i64,
    shortage_quantity: i64,
    resulting_revision: OrderRevision,
    occurred_at: Timestamp,
) -> AppResult<AllocationRunId> {
    let (scope_level, policy_owner_id, policy_facility_id) = policy_scope_values(policy);
    let policy_definition = serde_json::json!({
        "kind": "allocation",
        "rotation": policy.strategy.as_str(),
        "allow_partial": policy.allow_partial,
        "require_complete_line": policy.require_complete_line,
    });
    let allocation_run_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO order_allocation_runs
            (tenant_id, inventory_owner_id, order_id, facility_id, created,
             created_by_user_id,strategy,policy_source,policy_configuration_id,
             policy_configuration_revision,policy_scope_level,
             policy_inventory_owner_id,policy_facility_id,policy_definition,policy_hash,
             outcome,requested_qty,allocated_qty,short_qty,expected_revision,resulting_revision)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(command.order_id.get())
    .bind(command.facility_id.get())
    .bind(occurred_at)
    .bind(actor_user_id)
    .bind(policy.strategy.as_str())
    .bind(match policy.source {
        AllocationPolicySource::ProductDefault => "product_default",
        AllocationPolicySource::Configuration => "configuration",
    })
    .bind(policy.configuration_id.map(|id| id.get()))
    .bind(policy.configuration_revision)
    .bind(scope_level)
    .bind(policy_owner_id)
    .bind(policy_facility_id)
    .bind(policy_definition)
    .bind(&policy.policy_hash)
    .bind(outcome.as_str())
    .bind(demand_quantity)
    .bind(allocated_quantity)
    .bind(shortage_quantity)
    .bind(command.expected_revision.get())
    .bind(resulting_revision.get())
    .fetch_one(&mut **tx)
    .await?;
    AllocationRunId::new(allocation_run_id).map_err(|error| AppError::internal(error.to_string()))
}

fn policy_scope_values(
    policy: &AllocationPolicyReadModel,
) -> (Option<&'static str>, Option<i64>, Option<i64>) {
    match policy.configuration_scope {
        None => (None, None, None),
        Some(ConfigurationScope::Tenant) => (Some("tenant"), None, None),
        Some(ConfigurationScope::InventoryOwner { inventory_owner_id }) => (
            Some("inventory_owner"),
            Some(inventory_owner_id.get()),
            None,
        ),
        Some(ConfigurationScope::Facility { facility_id }) => {
            (Some("facility"), None, Some(facility_id.get()))
        }
        Some(ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        }) => (
            Some("owner_facility"),
            Some(inventory_owner_id.get()),
            Some(facility_id.get()),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn persist_planned_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    actor_user_id: i64,
    command: &PlanOrderAllocationCommand,
    policy: &AllocationPolicyReadModel,
    allocation_run_id: AllocationRunId,
    planned_lines: &[PlannedLine],
    candidates: &HashMap<i64, LockedCandidate>,
    occurred_at: Timestamp,
) -> AppResult<()> {
    for planned_line in planned_lines {
        sqlx::query(
            r#"
            INSERT INTO order_allocation_run_lines
                (tenant_id, inventory_owner_id, allocation_run_id, order_id,
                 order_item_id, reservation_id, requested_qty,
                 previously_allocated_qty, newly_allocated_qty,
                 total_allocated_qty, short_qty, shortage_reason)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(allocation_run_id.get())
        .bind(command.order_id.get())
        .bind(planned_line.line.id.get())
        .bind(planned_line.reservation_id.get())
        .bind(planned_line.line.quantity.get())
        .bind(planned_line.previously_allocated_quantity)
        .bind(planned_line.newly_allocated_quantity)
        .bind(planned_line.total_allocated_quantity)
        .bind(planned_line.shortage_quantity)
        .bind(
            planned_line
                .shortage_reason
                .map(AllocationShortageReason::as_str),
        )
        .execute(&mut **tx)
        .await?;

        for planned in &planned_line.allocations {
            let candidate = candidates
                .get(&planned.inventory_balance_id().get())
                .ok_or_else(|| AppError::internal("planned inventory balance is unavailable"))?;
            let allocation_id: i64 = sqlx::query_scalar(
                r#"
                INSERT INTO inventory_allocations
                    (tenant_id, inventory_owner_id, created, modified, created_by,
                     reservation_id, inventory_balance_id, facility_id, location_id,
                     license_plate_id, item_batch_id, item_id, uom,
                     inventory_status, allocation_run_id, qty, status,
                     execution_stage)
                VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                        $12, 'available', $13, $14, 'allocated', 'pick_source')
                RETURNING id
                "#,
            )
            .bind(tenant_id.get())
            .bind(inventory_owner_id.get())
            .bind(occurred_at)
            .bind(actor_user_id)
            .bind(planned_line.reservation_id.get())
            .bind(planned.inventory_balance_id().get())
            .bind(command.facility_id.get())
            .bind(planned.location_id().get())
            .bind(planned.license_plate_id().map(LicensePlateId::get))
            .bind(planned.item_batch_id().get())
            .bind(planned_line.line.item_id)
            .bind(&planned_line.line.uom)
            .bind(allocation_run_id.get())
            .bind(planned.quantity().get())
            .fetch_one(&mut **tx)
            .await?;
            let allocation_id = InventoryAllocationId::new(allocation_id)
                .map_err(|error| AppError::internal(error.to_string()))?;
            enqueue_allocation_created_event_tx(
                tx,
                tenant_id,
                inventory_owner_id,
                actor_user_id,
                command,
                policy,
                allocation_run_id,
                planned_line,
                candidate,
                allocation_id,
                planned.quantity(),
                occurred_at,
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn update_order_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    expected_revision: OrderRevision,
    resulting_revision: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders
        SET revision = $1
        WHERE tenant_id = $2 AND id = $3 AND revision = $4 AND deleted IS NULL
        "#,
    )
    .bind(resulting_revision.get())
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(expected_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::internal(
            "locked order revision was not updated atomically",
        ));
    }
    Ok(())
}
