//! Atomic order streaming from allocation readiness into executable RF work.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{FacilityId, LocationId, OrderId, OrderRevision};

use crate::order_allocation::{AllocationPolicyExpectation, PlanOrderAllocationResult};
use crate::order_release::ReleaseOrderResult;

pub const ORDER_STREAM_OPERATION: &str = "order.stream.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOrderCommand {
    pub order_id: OrderId,
    pub facility_id: FacilityId,
    pub destination_location_id: LocationId,
    pub expected_revision: OrderRevision,
    pub expected_allocation_policy: AllocationPolicyExpectation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOrderResult {
    pub allocation: PlanOrderAllocationResult,
    pub release: ReleaseOrderResult,
}

impl StreamOrderResult {
    pub fn is_consistent(&self) -> bool {
        self.allocation.quantities_are_consistent()
            && self.allocation.shortage_quantity == 0
            && self.allocation.order_id == self.release.order_id
            && self.allocation.inventory_owner_id == self.release.inventory_owner_id
            && self.allocation.facility_id == self.release.facility_id
            && self
                .allocation
                .revision
                .checked_next()
                .is_some_and(|revision| revision == self.release.revision)
            && self.release.is_consistent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_allocation::{
        AllocationPolicyReadModel, OrderAllocationDetail, OrderAllocationLineState,
        PlanOrderAllocationResult,
    };
    use crate::order_release::ReleaseOrderResult;
    use wareboxes_domain::{
        AllocationOutcome, AllocationQuantity, AllocationRunId, AllocationStrategy,
        InventoryAllocationId, InventoryBalanceId, InventoryOwnerId, InventoryReservationId,
        ItemBatchId, OrderLineId, OrderReleaseId, OrderStatus,
    };

    #[test]
    fn streamed_result_requires_consecutive_allocation_and_release_evidence() {
        let allocation = PlanOrderAllocationResult {
            allocation_run_id: AllocationRunId::new(1).unwrap(),
            order_id: OrderId::new(2).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(3).unwrap(),
            facility_id: FacilityId::new(4).unwrap(),
            policy: AllocationPolicyReadModel::product_default(),
            strategy: AllocationStrategy::Fefo,
            outcome: AllocationOutcome::FullyAllocated,
            revision: OrderRevision::new(2).unwrap(),
            newly_allocated_quantity: 5,
            original_demand_quantity: 5,
            backordered_quantity: 0,
            demand_quantity: 5,
            allocated_quantity: 5,
            shortage_quantity: 0,
            lines: vec![OrderAllocationLineState {
                order_line_id: OrderLineId::new(7).unwrap(),
                line_key: "line-1".into(),
                item_id: 8,
                item_description: None,
                uom: "each".into(),
                original_demand_quantity: 5,
                backordered_quantity: 0,
                demand_quantity: AllocationQuantity::new(5).unwrap(),
                reservation_id: Some(InventoryReservationId::new(9).unwrap()),
                reserved_quantity: 5,
                allocated_quantity: 5,
                shortage_quantity: 0,
                shortage_reason: None,
                allocations: vec![OrderAllocationDetail {
                    allocation_id: InventoryAllocationId::new(10).unwrap(),
                    reservation_id: InventoryReservationId::new(9).unwrap(),
                    inventory_balance_id: InventoryBalanceId::new(11).unwrap(),
                    item_batch_id: ItemBatchId::new(12).unwrap(),
                    location_id: LocationId::new(13).unwrap(),
                    location_name: None,
                    location_barcode: None,
                    license_plate_id: None,
                    license_plate_barcode: None,
                    lot: None,
                    serial: None,
                    expiration: None,
                    quantity: AllocationQuantity::new(5).unwrap(),
                }],
            }],
        };
        let release = ReleaseOrderResult {
            release_id: OrderReleaseId::new(5).unwrap(),
            order_id: allocation.order_id,
            inventory_owner_id: allocation.inventory_owner_id,
            facility_id: allocation.facility_id,
            destination_location_id: LocationId::new(6).unwrap(),
            status: OrderStatus::Processing,
            revision: OrderRevision::new(3).unwrap(),
            allocation_count: 1,
            pick_task_count: 1,
            released_quantity: 5,
            released_at: "2026-08-16T12:00:00Z".parse().unwrap(),
        };
        let result = StreamOrderResult {
            allocation,
            release,
        };
        assert!(result.is_consistent());

        let mut stale = result;
        stale.release.revision = OrderRevision::new(4).unwrap();
        assert!(!stale.is_consistent());
    }
}
