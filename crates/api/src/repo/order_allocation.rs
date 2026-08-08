//! Atomic order-level reservation and deterministic concrete stock planning.

use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::order_allocation::{
    OrderAllocationReadinessBlocker, OrderAllocationReadinessReadModel,
    OrderAllocationReadinessStatus, PlanOrderAllocationCommand, PlanOrderAllocationResult,
    ORDER_ALLOCATION_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    assess_order_allocation_readiness, AllocationCandidate, AllocationOutcome, AllocationQuantity,
    AllocationShortageReason, AllocationStrategy, FacilityId, InventoryBalanceId, InventoryOwnerId,
    InventoryReservationId, ItemBatchId, LicensePlateId, LocationId, OrderAllocationBlockReason,
    OrderAllocationReadiness, OrderId, OrderLineId, OrderRevision, OrderStatus, PlannedAllocation,
    Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::orders::{insert_order_activity_tx, require_replayed_order_visible_tx};

mod read_model;
mod workflow;

use read_model::{eligible_facilities_tx, line_state_totals, load_line_states_tx};
use workflow::{
    create_missing_full_demand_reservations_tx, cumulative_outcome,
    enqueue_order_allocation_event_tx, insert_allocation_run_tx, load_allocated_quantities_tx,
    lock_active_order_holds_tx, lock_active_owner_facility_tx, lock_active_reservations_tx,
    lock_candidate_inventory_tx, lock_order_lines_tx, lock_order_tx, persist_planned_lines_tx,
    plan_lines, read_order_tx, sum_line_demand, update_order_revision_tx,
    validate_existing_reservations,
};

#[derive(Debug)]
struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    order_key: String,
    status: OrderStatus,
    revision: OrderRevision,
}

#[derive(Debug, Clone)]
struct LockedOrderLine {
    id: OrderLineId,
    line_key: String,
    item_id: i64,
    uom: String,
    quantity: AllocationQuantity,
}

#[derive(Debug, Clone, Copy)]
struct ActiveReservation {
    id: InventoryReservationId,
    order_line_id: OrderLineId,
    facility_id: FacilityId,
    quantity: AllocationQuantity,
    allocated_quantity: i64,
}

#[derive(Debug, Clone, Copy)]
struct BalanceHint {
    inventory_balance_id: InventoryBalanceId,
    item_batch_id: ItemBatchId,
    location_id: LocationId,
    license_plate_id: Option<LicensePlateId>,
}

#[derive(Debug, Clone)]
struct LockedCandidate {
    inventory_balance_id: InventoryBalanceId,
    item_batch_id: ItemBatchId,
    location_id: LocationId,
    license_plate_id: Option<LicensePlateId>,
    item_id: i64,
    uom: String,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
    received_at: Timestamp,
    location_deleted: Option<Timestamp>,
    location_active: bool,
    location_pickable: bool,
    batch_deleted: Option<Timestamp>,
    license_plate_deleted: Option<Timestamp>,
    status: String,
    balance_deleted: Option<Timestamp>,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
}

impl LockedCandidate {
    fn available_quantity(&self, occurred_at: Timestamp) -> AppResult<Option<i64>> {
        let unexpired = self
            .expiration
            .is_none_or(|expiration| expiration > occurred_at);
        let license_plate_active =
            self.license_plate_id.is_none() || self.license_plate_deleted.is_none();
        if self.balance_deleted.is_some()
            || self.status != "available"
            || self.location_deleted.is_some()
            || !self.location_active
            || !self.location_pickable
            || self.batch_deleted.is_some()
            || !unexpired
            || !license_plate_active
        {
            return Ok(None);
        }

        let committed = self
            .qty_reserved
            .checked_add(self.qty_held)
            .ok_or_else(|| AppError::internal("inventory commitments exceed i64"))?;
        let available = self
            .qty_on_hand
            .checked_sub(committed)
            .ok_or_else(|| AppError::internal("inventory commitments exceed on-hand quantity"))?;
        Ok((available > 0).then_some(available))
    }

    fn domain_candidate(&self, available: i64) -> AppResult<AllocationCandidate> {
        let quantity = AllocationQuantity::new(available)
            .map_err(|error| AppError::internal(error.to_string()))?;
        Ok(AllocationCandidate::new(
            self.inventory_balance_id,
            self.item_batch_id,
            self.location_id,
            self.license_plate_id,
            self.lot.clone(),
            self.serial.clone(),
            self.expiration,
            self.received_at,
            quantity,
        ))
    }
}

#[derive(Debug)]
struct PlannedLine {
    line: LockedOrderLine,
    reservation_id: InventoryReservationId,
    previously_allocated_quantity: i64,
    newly_allocated_quantity: i64,
    total_allocated_quantity: i64,
    shortage_quantity: i64,
    shortage_reason: Option<AllocationShortageReason>,
    allocations: Vec<PlannedAllocation>,
}

#[derive(Debug, Clone, Copy)]
struct AllocationTotals {
    demand_quantity: i64,
    reserved_quantity: i64,
    allocated_quantity: i64,
    shortage_quantity: i64,
    outcome: AllocationOutcome,
}

pub async fn plan_order_allocation(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanOrderAllocationCommand,
) -> AppResult<PlanOrderAllocationResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, ORDER_ALLOCATION_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "orders").await?;

    if let Some(result) = prepared
        .replayed::<PlanOrderAllocationResult>(&mut tx)
        .await?
    {
        require_replayed_order_visible_tx(&mut tx, access.tenant_id, result.order_id.get(), &scope)
            .await?;
        if !scope.includes_facility(result.facility_id.get()) {
            return Err(AppError::not_found("order allocation"));
        }
        tx.commit().await?;
        return Ok(result);
    }

    if !scope.includes_facility(command.facility_id.get()) {
        return Err(AppError::forbidden());
    }
    let order = lock_order_tx(&mut tx, access.tenant_id, command.order_id).await?;
    if !scope.includes_inventory_owner(order.inventory_owner_id.get()) {
        return Err(AppError::not_found("order"));
    }
    if order.revision != command.expected_revision {
        return Err(AppError::conflict(
            "order revision does not match expected revision",
        ));
    }
    lock_active_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.facility_id,
    )
    .await?;

    let lines = lock_order_lines_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id,
    )
    .await?;
    if lines.is_empty() {
        return Err(AppError::internal("order has no active demand lines"));
    }
    let active_hold_count = lock_active_order_holds_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id,
    )
    .await?;
    let mut reservations = lock_active_reservations_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id,
    )
    .await?;
    validate_existing_reservations(&lines, &reservations, command.facility_id)?;

    let occurred_at = now_iso();
    create_missing_full_demand_reservations_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command,
        context.actor_id.get(),
        occurred_at,
        &lines,
        &mut reservations,
    )
    .await?;
    load_allocated_quantities_tx(&mut tx, access.tenant_id, &mut reservations).await?;

    let demand_quantity = sum_line_demand(&lines)?;
    let previously_allocated_quantity = reservations
        .values()
        .try_fold(0_i64, |total, reservation| {
            total.checked_add(reservation.allocated_quantity)
        })
        .ok_or_else(|| AppError::internal("allocated order quantity exceeds i64"))?;
    let remaining_quantity = demand_quantity
        .checked_sub(previously_allocated_quantity)
        .ok_or_else(|| AppError::internal("allocated quantity exceeds order demand"))?;
    let remaining_quantity_u64 = u64::try_from(remaining_quantity)
        .map_err(|_| AppError::internal("remaining order quantity is invalid"))?;
    let active_hold_count_u64 = u64::try_from(active_hold_count)
        .map_err(|_| AppError::internal("active order hold count is invalid"))?;
    match assess_order_allocation_readiness(
        order.status,
        active_hold_count_u64,
        remaining_quantity_u64,
    ) {
        OrderAllocationReadiness::Ready => {}
        OrderAllocationReadiness::AlreadyFullyAllocated => {
            return Err(AppError::conflict("order is already fully allocated"));
        }
        OrderAllocationReadiness::Blocked(OrderAllocationBlockReason::ActiveHold) => {
            return Err(AppError::conflict("order has an active hold"));
        }
        OrderAllocationReadiness::Blocked(
            OrderAllocationBlockReason::OrderStatusNotAllocatable,
        ) => {
            return Err(AppError::conflict("order status does not allow allocation"));
        }
    }

    let candidates = lock_candidate_inventory_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.facility_id,
        &lines,
        occurred_at,
    )
    .await?;
    let planned_lines = plan_lines(&lines, &reservations, &candidates, occurred_at)?;
    let newly_allocated_quantity = planned_lines
        .iter()
        .try_fold(0_i64, |total, line| {
            total.checked_add(line.newly_allocated_quantity)
        })
        .ok_or_else(|| AppError::internal("new allocation quantity exceeds i64"))?;
    let allocated_quantity = planned_lines
        .iter()
        .try_fold(0_i64, |total, line| {
            total.checked_add(line.total_allocated_quantity)
        })
        .ok_or_else(|| AppError::internal("allocated order quantity exceeds i64"))?;
    let shortage_quantity = demand_quantity
        .checked_sub(allocated_quantity)
        .ok_or_else(|| AppError::internal("allocated quantity exceeds order demand"))?;
    let outcome = cumulative_outcome(demand_quantity, allocated_quantity)?;
    let resulting_revision = command
        .expected_revision
        .checked_next()
        .ok_or_else(|| AppError::conflict("order revision cannot be incremented"))?;
    let allocation_run_id = insert_allocation_run_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        context.actor_id.get(),
        command,
        outcome,
        demand_quantity,
        allocated_quantity,
        shortage_quantity,
        resulting_revision,
        occurred_at,
    )
    .await?;

    persist_planned_lines_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        context.actor_id.get(),
        command,
        allocation_run_id,
        &planned_lines,
        &candidates,
        occurred_at,
    )
    .await?;
    update_order_revision_tx(
        &mut tx,
        access.tenant_id,
        command.order_id,
        command.expected_revision,
        resulting_revision,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id.get(),
        Some(context.actor_id.get()),
        "planned order allocation",
    )
    .await?;
    enqueue_order_allocation_event_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        context.actor_id.get(),
        command,
        allocation_run_id,
        outcome,
        resulting_revision,
        demand_quantity,
        newly_allocated_quantity,
        allocated_quantity,
        shortage_quantity,
        occurred_at,
    )
    .await?;

    let lines = load_line_states_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id,
        command.facility_id,
    )
    .await?;
    let result = PlanOrderAllocationResult {
        allocation_run_id,
        order_id: command.order_id,
        inventory_owner_id: order.inventory_owner_id,
        facility_id: command.facility_id,
        strategy: command.strategy,
        outcome,
        revision: resulting_revision,
        newly_allocated_quantity,
        demand_quantity,
        allocated_quantity,
        shortage_quantity,
        lines,
    };
    if !result.quantities_are_consistent() {
        return Err(AppError::internal(
            "committed allocation result does not conserve order demand",
        ));
    }
    Ok(prepared.commit(tx, result).await?)
}

pub async fn order_allocation_readiness(
    db: &Db,
    access: &TenantAccess,
    order_id: OrderId,
    facility_id: FacilityId,
) -> AppResult<OrderAllocationReadinessReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "orders").await?;
    if !scope.includes_facility(facility_id.get()) {
        return Err(AppError::forbidden());
    }

    let order = read_order_tx(&mut tx, access.tenant_id, order_id).await?;
    if !scope.includes_inventory_owner(order.inventory_owner_id.get()) {
        return Err(AppError::not_found("order"));
    }
    let eligible_facilities =
        eligible_facilities_tx(&mut tx, access.tenant_id, order.inventory_owner_id, &scope).await?;
    let selected_facility_is_eligible = eligible_facilities
        .iter()
        .any(|facility| facility.facility_id == facility_id);
    let active_hold_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM order_holds
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND order_id = $3
          AND released_at IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order.inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let lines = load_line_states_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        order_id,
        facility_id,
    )
    .await?;
    if lines.is_empty() {
        return Err(AppError::internal("order has no active demand lines"));
    }
    let totals = line_state_totals(&lines)?;
    let remaining_quantity_u64 = u64::try_from(totals.shortage_quantity)
        .map_err(|_| AppError::internal("remaining order quantity is invalid"))?;
    let active_hold_count_u64 = u64::try_from(active_hold_count)
        .map_err(|_| AppError::internal("active order hold count is invalid"))?;
    let assessment = assess_order_allocation_readiness(
        order.status,
        active_hold_count_u64,
        remaining_quantity_u64,
    );
    let mut blocking_reasons = Vec::new();
    match assessment {
        OrderAllocationReadiness::Ready | OrderAllocationReadiness::AlreadyFullyAllocated => {}
        OrderAllocationReadiness::Blocked(OrderAllocationBlockReason::ActiveHold) => {
            blocking_reasons.push(OrderAllocationReadinessBlocker::ActiveHold);
        }
        OrderAllocationReadiness::Blocked(
            OrderAllocationBlockReason::OrderStatusNotAllocatable,
        ) => {
            blocking_reasons.push(OrderAllocationReadinessBlocker::OrderStatusNotAllocatable);
        }
    }
    if !selected_facility_is_eligible {
        blocking_reasons.push(OrderAllocationReadinessBlocker::OwnerFacilityUnavailable);
    }
    let status = if !blocking_reasons.is_empty() {
        OrderAllocationReadinessStatus::Blocked
    } else {
        match assessment {
            OrderAllocationReadiness::Ready => OrderAllocationReadinessStatus::Ready,
            OrderAllocationReadiness::AlreadyFullyAllocated => {
                OrderAllocationReadinessStatus::AlreadyFullyAllocated
            }
            OrderAllocationReadiness::Blocked(_) => OrderAllocationReadinessStatus::Blocked,
        }
    };
    let result = OrderAllocationReadinessReadModel {
        order_id,
        inventory_owner_id: order.inventory_owner_id,
        order_key: order.order_key,
        facility_id,
        eligible_facilities,
        revision: order.revision,
        status,
        blocking_reasons,
        strategy: AllocationStrategy::Fefo,
        outcome: totals.outcome,
        demand_quantity: totals.demand_quantity,
        reserved_quantity: totals.reserved_quantity,
        allocated_quantity: totals.allocated_quantity,
        shortage_quantity: totals.shortage_quantity,
        lines,
    };
    if !result.quantities_are_consistent() {
        return Err(AppError::internal(
            "allocation readiness does not conserve order demand",
        ));
    }
    tx.commit().await?;
    Ok(result)
}
