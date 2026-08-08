//! Application contracts for optimistic, replay-safe waveless order release.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, LocationId, OrderId, OrderReleaseId, OrderRevision, OrderStatus,
    Timestamp,
};

/// Stable idempotency operation for the first waveless order-release schema.
pub const ORDER_RELEASE_OPERATION: &str = "order.release.v1";

/// Releases one fully allocated order at the revision observed by the operator.
///
/// The inventory owner is deliberately absent. It is derived from the scoped,
/// locked order aggregate inside the command transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseOrderCommand {
    pub order_id: OrderId,
    pub facility_id: FacilityId,
    pub destination_location_id: LocationId,
    pub expected_revision: OrderRevision,
}

/// Replay-stable result of creating waveless pick work for one order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseOrderResult {
    pub release_id: OrderReleaseId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub destination_location_id: LocationId,
    pub status: OrderStatus,
    pub revision: OrderRevision,
    pub allocation_count: i64,
    pub pick_task_count: i64,
    pub released_quantity: i64,
    pub released_at: Timestamp,
}

impl ReleaseOrderResult {
    /// Checks invariants expected of the durable release projection.
    pub const fn is_consistent(&self) -> bool {
        matches!(self.status, OrderStatus::Processing)
            && self.allocation_count > 0
            && self.pick_task_count > 0
            && self.allocation_count == self.pick_task_count
            && self.released_quantity > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_result_requires_processing_work_and_conserved_contents() {
        let released_at = "2026-08-08T20:00:00Z".parse::<Timestamp>().unwrap();
        let result = ReleaseOrderResult {
            release_id: OrderReleaseId::new(11).unwrap(),
            order_id: OrderId::new(12).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(13).unwrap(),
            facility_id: FacilityId::new(14).unwrap(),
            destination_location_id: LocationId::new(15).unwrap(),
            status: OrderStatus::Processing,
            revision: OrderRevision::new(3).unwrap(),
            allocation_count: 2,
            pick_task_count: 2,
            released_quantity: 8,
            released_at,
        };

        assert!(result.is_consistent());

        let mut inconsistent = result;
        inconsistent.pick_task_count = 1;
        assert!(!inconsistent.is_consistent());
    }
}
