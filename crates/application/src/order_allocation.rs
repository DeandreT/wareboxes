//! Application contracts for order-level reservation and concrete stock planning.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    AllocationOutcome, AllocationQuantity, AllocationRunId, AllocationShortageReason,
    AllocationStrategy, FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryOwnerId,
    InventoryReservationId, ItemBatchId, LicensePlateId, LocationId, OrderId, OrderLineId,
    OrderRevision, Timestamp,
};

use crate::backorder::BackorderPolicyReadModel;

/// Stable idempotency operation for the first order allocation command schema.
pub const ORDER_ALLOCATION_OPERATION: &str = "order.allocate.v1";

/// Plans all remaining demand on one order against one facility.
///
/// The inventory owner is deliberately absent. It is derived from the scoped,
/// locked order aggregate inside the command transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanOrderAllocationCommand {
    pub order_id: OrderId,
    pub facility_id: FacilityId,
    pub expected_revision: OrderRevision,
    pub strategy: AllocationStrategy,
}

/// One active concrete allocation in the post-command order state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAllocationDetail {
    pub allocation_id: InventoryAllocationId,
    pub reservation_id: InventoryReservationId,
    pub inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub location_id: LocationId,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<LicensePlateId>,
    pub license_plate_barcode: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantity: AllocationQuantity,
}

/// Cumulative reservation and allocation state for one order demand line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAllocationLineState {
    pub order_line_id: OrderLineId,
    pub line_key: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub original_demand_quantity: i64,
    pub backordered_quantity: i64,
    pub demand_quantity: AllocationQuantity,
    pub reservation_id: Option<InventoryReservationId>,
    pub reserved_quantity: i64,
    pub allocated_quantity: i64,
    pub shortage_quantity: i64,
    pub shortage_reason: Option<AllocationShortageReason>,
    pub allocations: Vec<OrderAllocationDetail>,
}

impl OrderAllocationLineState {
    /// Checks the quantity conservation required of repository projections.
    pub fn quantities_are_consistent(&self) -> bool {
        self.original_demand_quantity > 0
            && self.backordered_quantity >= 0
            && self.original_demand_quantity
                == self.demand_quantity.get() + self.backordered_quantity
            && self.reserved_quantity >= 0
            && self.reserved_quantity <= self.demand_quantity.get()
            && self.allocated_quantity >= 0
            && self.allocated_quantity <= self.reserved_quantity
            && self.shortage_quantity == self.demand_quantity.get() - self.allocated_quantity
            && (self.shortage_quantity == 0) == self.shortage_reason.is_none()
            && self
                .allocations
                .iter()
                .try_fold(0_i64, |total, allocation| {
                    total.checked_add(allocation.quantity.get())
                })
                == Some(self.allocated_quantity)
    }
}

/// Replay-stable result of one committed allocation planning run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanOrderAllocationResult {
    pub allocation_run_id: AllocationRunId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub strategy: AllocationStrategy,
    pub outcome: AllocationOutcome,
    pub revision: OrderRevision,
    pub newly_allocated_quantity: i64,
    pub original_demand_quantity: i64,
    pub backordered_quantity: i64,
    pub demand_quantity: i64,
    pub allocated_quantity: i64,
    pub shortage_quantity: i64,
    pub lines: Vec<OrderAllocationLineState>,
}

impl PlanOrderAllocationResult {
    pub fn quantities_are_consistent(&self) -> bool {
        if self.newly_allocated_quantity < 0
            || self.original_demand_quantity <= 0
            || self.backordered_quantity < 0
            || self.original_demand_quantity != self.demand_quantity + self.backordered_quantity
            || self.demand_quantity <= 0
            || self.allocated_quantity < 0
            || self.shortage_quantity < 0
            || self.demand_quantity != self.allocated_quantity + self.shortage_quantity
            || !self
                .lines
                .iter()
                .all(OrderAllocationLineState::quantities_are_consistent)
        {
            return false;
        }

        let totals = self.lines.iter().try_fold(
            (0_i64, 0_i64, 0_i64, 0_i64, 0_i64),
            |(original, backordered, demand, allocated, shortage), line| {
                Some((
                    original.checked_add(line.original_demand_quantity)?,
                    backordered.checked_add(line.backordered_quantity)?,
                    demand.checked_add(line.demand_quantity.get())?,
                    allocated.checked_add(line.allocated_quantity)?,
                    shortage.checked_add(line.shortage_quantity)?,
                ))
            },
        );
        totals
            == Some((
                self.original_demand_quantity,
                self.backordered_quantity,
                self.demand_quantity,
                self.allocated_quantity,
                self.shortage_quantity,
            ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderAllocationReadinessStatus {
    Ready,
    AlreadyFullyAllocated,
    Blocked,
}

/// Typed operator-facing reason that another allocation run cannot execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderAllocationReadinessBlocker {
    ActiveHold,
    CrossDockInProgress,
    OrderStatusNotAllocatable,
    OwnerFacilityUnavailable,
}

/// Active owner-assigned facility visible through the actor's site scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderAllocationFacilityReadModel {
    pub facility_id: FacilityId,
    pub facility_name: String,
}

/// Current allocation state used to render and gate the planning workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderAllocationReadinessReadModel {
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub order_key: String,
    pub facility_id: FacilityId,
    pub eligible_facilities: Vec<OrderAllocationFacilityReadModel>,
    pub backorder_policy: Option<BackorderPolicyReadModel>,
    pub revision: OrderRevision,
    pub status: OrderAllocationReadinessStatus,
    pub blocking_reasons: Vec<OrderAllocationReadinessBlocker>,
    pub strategy: AllocationStrategy,
    pub outcome: AllocationOutcome,
    pub original_demand_quantity: i64,
    pub backordered_quantity: i64,
    pub demand_quantity: i64,
    pub reserved_quantity: i64,
    pub allocated_quantity: i64,
    pub shortage_quantity: i64,
    pub lines: Vec<OrderAllocationLineState>,
}

impl OrderAllocationReadinessReadModel {
    pub fn quantities_are_consistent(&self) -> bool {
        self.original_demand_quantity > 0
            && self.backordered_quantity >= 0
            && self.original_demand_quantity == self.demand_quantity + self.backordered_quantity
            && self.demand_quantity > 0
            && self.reserved_quantity >= 0
            && self.reserved_quantity <= self.demand_quantity
            && self.allocated_quantity >= 0
            && self.allocated_quantity <= self.reserved_quantity
            && self.shortage_quantity == self.demand_quantity - self.allocated_quantity
            && self
                .lines
                .iter()
                .all(OrderAllocationLineState::quantities_are_consistent)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn line_state() -> OrderAllocationLineState {
        OrderAllocationLineState {
            order_line_id: OrderLineId::new(12).unwrap(),
            line_key: "1".into(),
            item_id: 41,
            item_description: Some("Case-picked item".into()),
            uom: "case".into(),
            original_demand_quantity: 8,
            backordered_quantity: 0,
            demand_quantity: AllocationQuantity::new(8).unwrap(),
            reservation_id: Some(InventoryReservationId::new(22).unwrap()),
            reserved_quantity: 8,
            allocated_quantity: 5,
            shortage_quantity: 3,
            shortage_reason: Some(AllocationShortageReason::InsufficientEligibleInventory),
            allocations: vec![OrderAllocationDetail {
                allocation_id: InventoryAllocationId::new(31).unwrap(),
                reservation_id: InventoryReservationId::new(22).unwrap(),
                inventory_balance_id: InventoryBalanceId::new(42).unwrap(),
                item_batch_id: ItemBatchId::new(52).unwrap(),
                location_id: LocationId::new(62).unwrap(),
                location_name: Some("Forward pick A-01".into()),
                location_barcode: Some("A-01".into()),
                license_plate_id: Some(LicensePlateId::new(72).unwrap()),
                license_plate_barcode: Some("LP-00072".into()),
                lot: Some("LOT-7".into()),
                serial: None,
                expiration: Some("2027-08-10T00:00:00Z".parse::<Timestamp>().unwrap()),
                quantity: AllocationQuantity::new(5).unwrap(),
            }],
        }
    }

    #[test]
    fn command_identity_never_accepts_a_caller_supplied_owner() {
        let command = PlanOrderAllocationCommand {
            order_id: OrderId::new(7).unwrap(),
            facility_id: FacilityId::new(8).unwrap(),
            expected_revision: OrderRevision::new(3).unwrap(),
            strategy: AllocationStrategy::Fefo,
        };

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "order_id": 7,
                "facility_id": 8,
                "expected_revision": 3,
                "strategy": "fefo"
            })
        );
    }

    #[test]
    fn cumulative_line_and_result_quantities_must_conserve_demand() {
        let line = line_state();
        assert!(line.quantities_are_consistent());

        let result = PlanOrderAllocationResult {
            allocation_run_id: AllocationRunId::new(81).unwrap(),
            order_id: OrderId::new(7).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(9).unwrap(),
            facility_id: FacilityId::new(8).unwrap(),
            strategy: AllocationStrategy::Fefo,
            outcome: AllocationOutcome::PartiallyAllocated,
            revision: OrderRevision::new(4).unwrap(),
            newly_allocated_quantity: 5,
            original_demand_quantity: 8,
            backordered_quantity: 0,
            demand_quantity: 8,
            allocated_quantity: 5,
            shortage_quantity: 3,
            lines: vec![line],
        };
        assert!(result.quantities_are_consistent());

        let mut invalid = result;
        invalid.shortage_quantity = 4;
        assert!(!invalid.quantities_are_consistent());
    }
}
