//! Server-selected release of allocation-ready orders into executable pick work.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    DynamicReleaseRunId, FacilityId, InventoryOwnerId, LocationId, OrderId, OrderRevision,
    Timestamp, UserId,
};

use crate::pick_wave::PickWaveReadModel;
use crate::wave_policy::{WavePolicyExpectation, WavePolicyReadModel};

pub const DYNAMIC_RELEASE_OPERATION: &str = "outbound.dynamic_release.run.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicReleaseCommand {
    pub facility_id: FacilityId,
    pub inventory_owner_id: InventoryOwnerId,
    pub destination_location_id: LocationId,
    pub expected_policy: WavePolicyExpectation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicReleaseReadinessQuery {
    pub facility_id: FacilityId,
    pub inventory_owner_id: InventoryOwnerId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicReleaseCandidateReadModel {
    pub order_id: OrderId,
    pub order_key: String,
    pub revision: OrderRevision,
    pub rank: u32,
    pub rush: bool,
    pub ship_by: Option<Timestamp>,
    pub order_created_at: Timestamp,
    pub demand_quantity: i64,
    pub allocated_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicReleaseReadinessReadModel {
    pub facility_id: FacilityId,
    pub inventory_owner_id: InventoryOwnerId,
    pub input_snapshot_at: Timestamp,
    pub policy: WavePolicyReadModel,
    pub eligible_order_count: i64,
    pub selected_order_count: i64,
    pub deferred_order_count: i64,
    pub selected_orders: Vec<DynamicReleaseCandidateReadModel>,
}

impl DynamicReleaseReadinessReadModel {
    pub fn is_consistent(&self) -> bool {
        self.eligible_order_count >= 0
            && self.selected_order_count >= 0
            && self.deferred_order_count >= 0
            && self.eligible_order_count == self.selected_order_count + self.deferred_order_count
            && usize::try_from(self.selected_order_count) == Ok(self.selected_orders.len())
            && self
                .selected_orders
                .iter()
                .enumerate()
                .all(|(index, order)| {
                    u32::try_from(index + 1) == Ok(order.rank)
                        && order.demand_quantity > 0
                        && order.demand_quantity == order.allocated_quantity
                })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicReleaseRunReadModel {
    pub run_id: DynamicReleaseRunId,
    pub facility_id: FacilityId,
    pub inventory_owner_id: InventoryOwnerId,
    pub destination_location_id: LocationId,
    pub input_snapshot_at: Timestamp,
    pub policy: WavePolicyReadModel,
    pub eligible_order_count: i64,
    pub selected_order_count: i64,
    pub deferred_order_count: i64,
    pub selected_orders: Vec<DynamicReleaseCandidateReadModel>,
    pub wave: Option<PickWaveReadModel>,
    pub released_by: UserId,
    pub released_at: Timestamp,
}

impl DynamicReleaseRunReadModel {
    pub fn is_consistent(&self) -> bool {
        let readiness = DynamicReleaseReadinessReadModel {
            facility_id: self.facility_id,
            inventory_owner_id: self.inventory_owner_id,
            input_snapshot_at: self.input_snapshot_at,
            policy: self.policy.clone(),
            eligible_order_count: self.eligible_order_count,
            selected_order_count: self.selected_order_count,
            deferred_order_count: self.deferred_order_count,
            selected_orders: self.selected_orders.clone(),
        };
        readiness.is_consistent()
            && match &self.wave {
                Some(wave) => {
                    self.selected_order_count > 0
                        && wave.is_consistent()
                        && wave.facility_id == self.facility_id
                        && wave.destination_location_id == self.destination_location_id
                        && wave.orders.len() == self.selected_orders.len()
                        && wave.orders.iter().zip(&self.selected_orders).all(
                            |(member, candidate)| {
                                member.order_id == candidate.order_id
                                    && member.inventory_owner_id == self.inventory_owner_id
                                    && member.sequence == candidate.rank
                            },
                        )
                }
                None => self.selected_order_count == 0,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_dense_fully_allocated_selection() {
        let readiness = DynamicReleaseReadinessReadModel {
            facility_id: FacilityId::new(1).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(2).unwrap(),
            input_snapshot_at: "2026-08-16T12:00:00Z".parse().unwrap(),
            policy: WavePolicyReadModel::product_default(),
            eligible_order_count: 2,
            selected_order_count: 1,
            deferred_order_count: 1,
            selected_orders: vec![DynamicReleaseCandidateReadModel {
                order_id: OrderId::new(3).unwrap(),
                order_key: "ORDER-1".into(),
                revision: OrderRevision::new(4).unwrap(),
                rank: 1,
                rush: true,
                ship_by: None,
                order_created_at: "2026-08-16T10:00:00Z".parse().unwrap(),
                demand_quantity: 5,
                allocated_quantity: 5,
            }],
        };
        assert!(readiness.is_consistent());

        let mut sparse = readiness;
        sparse.selected_orders[0].rank = 2;
        assert!(!sparse.is_consistent());
    }
}
