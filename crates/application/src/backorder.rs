//! Application contracts for owner/facility backorder policy and pre-release demand splitting.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    BackorderDetails, BackorderPolicyId, BackorderPolicyMode, BackorderPolicyRevision,
    BackorderSplitId, FacilityId, InventoryOwnerId, OrderId, OrderLineId, OrderRevision,
    OrderStatus, Timestamp, UserId,
};

pub const CONFIGURE_BACKORDER_POLICY_OPERATION: &str = "outbound.backorder.policy.configure.v1";
pub const SPLIT_ORDER_BACKORDER_OPERATION: &str = "outbound.backorder.split.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureBackorderPolicyCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub mode: BackorderPolicyMode,
    pub expected_revision: Option<BackorderPolicyRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackorderPolicyReadModel {
    pub policy_id: BackorderPolicyId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub mode: BackorderPolicyMode,
    pub revision: BackorderPolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

pub type ConfigureBackorderPolicyResult = BackorderPolicyReadModel;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitOrderBackorderCommand {
    pub order_id: OrderId,
    pub facility_id: FacilityId,
    pub expected_order_revision: OrderRevision,
    pub expected_policy_revision: BackorderPolicyRevision,
    pub details: BackorderDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackorderSplitLineReadModel {
    pub parent_order_line_id: OrderLineId,
    pub child_order_line_id: OrderLineId,
    pub line_key: String,
    pub item_id: i64,
    pub uom: String,
    pub original_quantity: i64,
    pub allocated_quantity: i64,
    pub previously_backordered_quantity: i64,
    pub newly_backordered_quantity: i64,
    pub resulting_parent_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitOrderBackorderResult {
    pub split_id: BackorderSplitId,
    pub policy_id: BackorderPolicyId,
    pub policy_revision: BackorderPolicyRevision,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub parent_order_id: OrderId,
    pub parent_order_key: String,
    pub parent_status: OrderStatus,
    pub parent_revision: OrderRevision,
    pub child_order_id: OrderId,
    pub child_order_key: String,
    pub child_status: OrderStatus,
    pub child_revision: OrderRevision,
    pub original_quantity: i64,
    pub allocated_quantity: i64,
    pub previously_backordered_quantity: i64,
    pub newly_backordered_quantity: i64,
    pub parent_effective_quantity: i64,
    pub lines: Vec<BackorderSplitLineReadModel>,
    pub details: BackorderDetails,
    pub split_by: UserId,
    pub split_at: Timestamp,
}

impl SplitOrderBackorderResult {
    pub fn quantities_are_consistent(&self) -> bool {
        if self.original_quantity <= 0
            || self.allocated_quantity <= 0
            || self.previously_backordered_quantity < 0
            || self.newly_backordered_quantity <= 0
            || self.parent_effective_quantity < self.allocated_quantity
            || self.lines.is_empty()
        {
            return false;
        }
        self.lines.iter().try_fold(
            (0_i64, 0_i64, 0_i64, 0_i64),
            |(original, allocated, previous, new), line| {
                if line.original_quantity <= 0
                    || line.allocated_quantity < 0
                    || line.previously_backordered_quantity < 0
                    || line.newly_backordered_quantity < 0
                    || line.resulting_parent_quantity != line.allocated_quantity
                    || line.original_quantity
                        != line.allocated_quantity
                            + line.previously_backordered_quantity
                            + line.newly_backordered_quantity
                {
                    return None;
                }
                Some((
                    original.checked_add(line.original_quantity)?,
                    allocated.checked_add(line.allocated_quantity)?,
                    previous.checked_add(line.previously_backordered_quantity)?,
                    new.checked_add(line.newly_backordered_quantity)?,
                ))
            },
        ) == Some((
            self.original_quantity,
            self.allocated_quantity,
            self.previously_backordered_quantity,
            self.newly_backordered_quantity,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{BackorderNote, BackorderReason};

    #[test]
    fn split_result_conserves_parent_and_child_demand() {
        let result = SplitOrderBackorderResult {
            split_id: BackorderSplitId::new(1).unwrap(),
            policy_id: BackorderPolicyId::new(2).unwrap(),
            policy_revision: BackorderPolicyRevision::new(1).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(3).unwrap(),
            facility_id: FacilityId::new(4).unwrap(),
            parent_order_id: OrderId::new(5).unwrap(),
            parent_order_key: "SO-1".into(),
            parent_status: OrderStatus::Open,
            parent_revision: OrderRevision::new(3).unwrap(),
            child_order_id: OrderId::new(6).unwrap(),
            child_order_key: "SO-1-B001".into(),
            child_status: OrderStatus::Open,
            child_revision: OrderRevision::new(1).unwrap(),
            original_quantity: 10,
            allocated_quantity: 6,
            previously_backordered_quantity: 0,
            newly_backordered_quantity: 4,
            parent_effective_quantity: 6,
            lines: vec![BackorderSplitLineReadModel {
                parent_order_line_id: OrderLineId::new(7).unwrap(),
                child_order_line_id: OrderLineId::new(8).unwrap(),
                line_key: "1".into(),
                item_id: 9,
                uom: "case".into(),
                original_quantity: 10,
                allocated_quantity: 6,
                previously_backordered_quantity: 0,
                newly_backordered_quantity: 4,
                resulting_parent_quantity: 6,
            }],
            details: BackorderDetails::new(
                BackorderReason::Other,
                Some(BackorderNote::new("Client approved").unwrap()),
            )
            .unwrap(),
            split_by: UserId::new(10).unwrap(),
            split_at: "2026-08-09T20:00:00Z".parse().unwrap(),
        };
        assert!(result.quantities_are_consistent());
    }
}
