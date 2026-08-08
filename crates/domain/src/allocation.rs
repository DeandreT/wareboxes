use std::cmp::Ordering;
use std::collections::HashSet;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use crate::{InventoryBalanceId, ItemBatchId, LicensePlateId, LocationId, OrderStatus, Timestamp};

/// Allocation policy selected for one planning run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationStrategy {
    /// Allocate the earliest expiration first, then the oldest received stock.
    Fefo,
}

impl AllocationStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fefo => "fefo",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fefo" => Some(Self::Fefo),
            _ => None,
        }
    }
}

/// Positive quantity accepted by the allocation planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct AllocationQuantity(i64);

impl AllocationQuantity {
    pub const fn new(value: i64) -> Result<Self, AllocationPlanError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(AllocationPlanError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for AllocationQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Positive order revision used for optimistic allocation commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OrderRevision(i64);

impl OrderRevision {
    pub const fn new(value: i64) -> Result<Self, AllocationPlanError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(AllocationPlanError::InvalidRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl<'de> Deserialize<'de> for OrderRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// One eligible inventory balance supplied to the pure allocation policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocationCandidate {
    inventory_balance_id: InventoryBalanceId,
    item_batch_id: ItemBatchId,
    location_id: LocationId,
    license_plate_id: Option<LicensePlateId>,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
    received_at: Timestamp,
    available_quantity: AllocationQuantity,
}

impl AllocationCandidate {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inventory_balance_id: InventoryBalanceId,
        item_batch_id: ItemBatchId,
        location_id: LocationId,
        license_plate_id: Option<LicensePlateId>,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<Timestamp>,
        received_at: Timestamp,
        available_quantity: AllocationQuantity,
    ) -> Self {
        Self {
            inventory_balance_id,
            item_batch_id,
            location_id,
            license_plate_id,
            lot,
            serial,
            expiration,
            received_at,
            available_quantity,
        }
    }

    pub const fn inventory_balance_id(&self) -> InventoryBalanceId {
        self.inventory_balance_id
    }

    pub const fn item_batch_id(&self) -> ItemBatchId {
        self.item_batch_id
    }

    pub const fn location_id(&self) -> LocationId {
        self.location_id
    }

    pub const fn license_plate_id(&self) -> Option<LicensePlateId> {
        self.license_plate_id
    }

    pub fn lot(&self) -> Option<&str> {
        self.lot.as_deref()
    }

    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    pub const fn expiration(&self) -> Option<&Timestamp> {
        self.expiration.as_ref()
    }

    pub const fn received_at(&self) -> &Timestamp {
        &self.received_at
    }

    pub const fn available_quantity(&self) -> AllocationQuantity {
        self.available_quantity
    }
}

/// Concrete inventory selected by the FEFO policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAllocation {
    inventory_balance_id: InventoryBalanceId,
    item_batch_id: ItemBatchId,
    location_id: LocationId,
    license_plate_id: Option<LicensePlateId>,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
    quantity: AllocationQuantity,
}

impl PlannedAllocation {
    pub const fn inventory_balance_id(&self) -> InventoryBalanceId {
        self.inventory_balance_id
    }

    pub const fn item_batch_id(&self) -> ItemBatchId {
        self.item_batch_id
    }

    pub const fn location_id(&self) -> LocationId {
        self.location_id
    }

    pub const fn license_plate_id(&self) -> Option<LicensePlateId> {
        self.license_plate_id
    }

    pub fn lot(&self) -> Option<&str> {
        self.lot.as_deref()
    }

    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    pub const fn expiration(&self) -> Option<&Timestamp> {
        self.expiration.as_ref()
    }

    pub const fn quantity(&self) -> AllocationQuantity {
        self.quantity
    }
}

/// Aggregate result of planning one demand quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationOutcome {
    FullyAllocated,
    PartiallyAllocated,
    NotAllocated,
}

impl AllocationOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullyAllocated => "fully_allocated",
            Self::PartiallyAllocated => "partially_allocated",
            Self::NotAllocated => "not_allocated",
        }
    }
}

/// Why positive allocation demand remains unsatisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationShortageReason {
    NoEligibleInventory,
    InsufficientEligibleInventory,
}

impl AllocationShortageReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoEligibleInventory => "no_eligible_inventory",
            Self::InsufficientEligibleInventory => "insufficient_eligible_inventory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocationPlan {
    demand_quantity: AllocationQuantity,
    allocated_quantity: i64,
    shortage_quantity: i64,
    outcome: AllocationOutcome,
    shortage_reason: Option<AllocationShortageReason>,
    allocations: Vec<PlannedAllocation>,
}

impl AllocationPlan {
    pub const fn demand_quantity(&self) -> AllocationQuantity {
        self.demand_quantity
    }

    pub const fn allocated_quantity(&self) -> i64 {
        self.allocated_quantity
    }

    pub const fn shortage_quantity(&self) -> i64 {
        self.shortage_quantity
    }

    pub const fn outcome(&self) -> AllocationOutcome {
        self.outcome
    }

    pub const fn shortage_reason(&self) -> Option<AllocationShortageReason> {
        self.shortage_reason
    }

    pub fn allocations(&self) -> &[PlannedAllocation] {
        &self.allocations
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AllocationPlanError {
    #[error("allocation quantity must be a positive integer, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("order revision must be a positive integer, got {value}")]
    InvalidRevision { value: i64 },
    #[error("inventory balance {inventory_balance_id} appears more than once")]
    DuplicateInventoryBalance { inventory_balance_id: i64 },
}

/// Order-level eligibility independent of persistence and authorization concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderAllocationReadiness {
    Ready,
    AlreadyFullyAllocated,
    Blocked(OrderAllocationBlockReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderAllocationBlockReason {
    ActiveHold,
    OrderStatusNotAllocatable,
}

/// Determines whether another allocation run may execute for an order aggregate.
pub const fn assess_order_allocation_readiness(
    status: OrderStatus,
    active_hold_count: u64,
    remaining_quantity: u64,
) -> OrderAllocationReadiness {
    if active_hold_count > 0 || matches!(status, OrderStatus::Held) {
        return OrderAllocationReadiness::Blocked(OrderAllocationBlockReason::ActiveHold);
    }

    match status {
        OrderStatus::Open | OrderStatus::Processing => {
            if remaining_quantity == 0 {
                OrderAllocationReadiness::AlreadyFullyAllocated
            } else {
                OrderAllocationReadiness::Ready
            }
        }
        OrderStatus::AwaitingShipment if remaining_quantity == 0 => {
            OrderAllocationReadiness::AlreadyFullyAllocated
        }
        _ => {
            OrderAllocationReadiness::Blocked(OrderAllocationBlockReason::OrderStatusNotAllocatable)
        }
    }
}

/// Selects concrete stock in deterministic FEFO order.
///
/// The repository must pre-filter candidates to the command's tenant, owner,
/// facility, item, UOM, and allocatable inventory status.
pub fn plan_fefo_allocation(
    demand_quantity: AllocationQuantity,
    mut candidates: Vec<AllocationCandidate>,
) -> Result<AllocationPlan, AllocationPlanError> {
    let mut balance_ids = HashSet::with_capacity(candidates.len());
    for candidate in &candidates {
        if !balance_ids.insert(candidate.inventory_balance_id) {
            return Err(AllocationPlanError::DuplicateInventoryBalance {
                inventory_balance_id: candidate.inventory_balance_id.get(),
            });
        }
    }

    candidates.sort_by(compare_fefo_candidates);
    let mut remaining = demand_quantity.get();
    let mut allocations = Vec::new();

    for candidate in candidates {
        if remaining == 0 {
            break;
        }
        let selected = remaining.min(candidate.available_quantity.get());
        remaining -= selected;
        allocations.push(PlannedAllocation {
            inventory_balance_id: candidate.inventory_balance_id,
            item_batch_id: candidate.item_batch_id,
            location_id: candidate.location_id,
            license_plate_id: candidate.license_plate_id,
            lot: candidate.lot,
            serial: candidate.serial,
            expiration: candidate.expiration,
            quantity: AllocationQuantity(selected),
        });
    }

    let allocated_quantity = demand_quantity.get() - remaining;
    let (outcome, shortage_reason) = if remaining == 0 {
        (AllocationOutcome::FullyAllocated, None)
    } else if allocated_quantity == 0 {
        (
            AllocationOutcome::NotAllocated,
            Some(AllocationShortageReason::NoEligibleInventory),
        )
    } else {
        (
            AllocationOutcome::PartiallyAllocated,
            Some(AllocationShortageReason::InsufficientEligibleInventory),
        )
    };

    Ok(AllocationPlan {
        demand_quantity,
        allocated_quantity,
        shortage_quantity: remaining,
        outcome,
        shortage_reason,
        allocations,
    })
}

fn compare_fefo_candidates(left: &AllocationCandidate, right: &AllocationCandidate) -> Ordering {
    compare_expiration(left.expiration.as_ref(), right.expiration.as_ref())
        .then_with(|| left.received_at.cmp(&right.received_at))
        .then_with(|| {
            left.inventory_balance_id
                .get()
                .cmp(&right.inventory_balance_id.get())
        })
}

fn compare_expiration(left: Option<&Timestamp>, right: Option<&Timestamp>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn timestamp(day: u32) -> Timestamp {
        Utc.with_ymd_and_hms(2027, 8, day, 12, 0, 0)
            .single()
            .unwrap()
    }

    fn candidate(
        balance_id: i64,
        expiration_day: Option<u32>,
        received_day: u32,
        quantity: i64,
    ) -> AllocationCandidate {
        AllocationCandidate::new(
            InventoryBalanceId::new(balance_id).unwrap(),
            ItemBatchId::new(balance_id + 100).unwrap(),
            LocationId::new(balance_id + 200).unwrap(),
            None,
            Some(format!("LOT-{balance_id}")),
            None,
            expiration_day.map(timestamp),
            timestamp(received_day),
            AllocationQuantity::new(quantity).unwrap(),
        )
    }

    #[test]
    fn fefo_plans_expiring_stock_before_fifo_and_non_expiring_stock() {
        let plan = plan_fefo_allocation(
            AllocationQuantity::new(9).unwrap(),
            vec![
                candidate(3, None, 1, 5),
                candidate(1, Some(20), 3, 5),
                candidate(2, Some(10), 4, 5),
            ],
        )
        .unwrap();

        assert_eq!(plan.outcome(), AllocationOutcome::FullyAllocated);
        assert_eq!(plan.allocated_quantity(), 9);
        assert_eq!(plan.shortage_quantity(), 0);
        assert_eq!(plan.shortage_reason(), None);
        assert_eq!(plan.allocations().len(), 2);
        assert_eq!(plan.allocations()[0].inventory_balance_id().get(), 2);
        assert_eq!(plan.allocations()[0].quantity().get(), 5);
        assert_eq!(plan.allocations()[1].inventory_balance_id().get(), 1);
        assert_eq!(plan.allocations()[1].quantity().get(), 4);
    }

    #[test]
    fn fefo_uses_received_time_then_balance_id_as_stable_tie_breakers() {
        let same_expiration = Some(10);
        let plan = plan_fefo_allocation(
            AllocationQuantity::new(3).unwrap(),
            vec![
                candidate(9, same_expiration, 3, 1),
                candidate(8, same_expiration, 2, 1),
                candidate(7, same_expiration, 2, 1),
            ],
        )
        .unwrap();

        let selected: Vec<_> = plan
            .allocations()
            .iter()
            .map(|allocation| allocation.inventory_balance_id().get())
            .collect();
        assert_eq!(selected, vec![7, 8, 9]);
    }

    #[test]
    fn shortage_is_a_typed_success_and_preserves_conservation() {
        let partial = plan_fefo_allocation(
            AllocationQuantity::new(8).unwrap(),
            vec![candidate(1, Some(10), 2, 3)],
        )
        .unwrap();
        assert_eq!(partial.outcome(), AllocationOutcome::PartiallyAllocated);
        assert_eq!(partial.allocated_quantity(), 3);
        assert_eq!(partial.shortage_quantity(), 5);
        assert_eq!(
            partial.shortage_reason(),
            Some(AllocationShortageReason::InsufficientEligibleInventory)
        );
        assert_eq!(
            partial.demand_quantity().get(),
            partial.allocated_quantity() + partial.shortage_quantity()
        );

        let none = plan_fefo_allocation(AllocationQuantity::new(8).unwrap(), Vec::new()).unwrap();
        assert_eq!(none.outcome(), AllocationOutcome::NotAllocated);
        assert_eq!(none.allocated_quantity(), 0);
        assert_eq!(none.shortage_quantity(), 8);
        assert_eq!(
            none.shortage_reason(),
            Some(AllocationShortageReason::NoEligibleInventory)
        );
    }

    #[test]
    fn planner_rejects_duplicate_balances_and_invalid_primitives() {
        let duplicate = candidate(1, Some(10), 2, 3);
        assert_eq!(
            plan_fefo_allocation(
                AllocationQuantity::new(4).unwrap(),
                vec![duplicate.clone(), duplicate]
            ),
            Err(AllocationPlanError::DuplicateInventoryBalance {
                inventory_balance_id: 1
            })
        );
        assert_eq!(
            AllocationQuantity::new(0),
            Err(AllocationPlanError::InvalidQuantity { value: 0 })
        );
        assert_eq!(
            OrderRevision::new(-1),
            Err(AllocationPlanError::InvalidRevision { value: -1 })
        );
    }

    #[test]
    fn readiness_allows_partial_retries_but_blocks_holds_and_terminal_orders() {
        assert_eq!(
            assess_order_allocation_readiness(OrderStatus::Open, 0, 4),
            OrderAllocationReadiness::Ready
        );
        assert_eq!(
            assess_order_allocation_readiness(OrderStatus::Processing, 0, 2),
            OrderAllocationReadiness::Ready
        );
        assert_eq!(
            assess_order_allocation_readiness(OrderStatus::Processing, 0, 0),
            OrderAllocationReadiness::AlreadyFullyAllocated
        );
        assert_eq!(
            assess_order_allocation_readiness(OrderStatus::Held, 1, 4),
            OrderAllocationReadiness::Blocked(OrderAllocationBlockReason::ActiveHold)
        );
        assert_eq!(
            assess_order_allocation_readiness(OrderStatus::Cancelled, 0, 4),
            OrderAllocationReadiness::Blocked(
                OrderAllocationBlockReason::OrderStatusNotAllocatable
            )
        );
    }
}
