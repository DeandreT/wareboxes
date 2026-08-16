use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{InventoryBalanceId, ItemBatchId, LocationId, OrderId, PickCartSlotId, PickTaskId};

pub const MAX_PICK_CART_BARCODE_LENGTH: usize = 80;
pub const MAX_PICK_CART_NAME_LENGTH: usize = 120;
pub const MAX_PICK_CART_SLOT_CODE_LENGTH: usize = 40;
pub const MAX_PICK_CART_SLOTS: usize = 48;
pub const MAX_PICK_CLUSTER_TASKS: usize = 200;
pub const MAX_PICK_CLUSTER_CANCEL_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PickCartBarcode(String);

impl PickCartBarcode {
    pub fn new(value: String) -> Result<Self, PickClusterError> {
        let value = value.trim().to_owned();
        if value.is_empty() || value.len() > MAX_PICK_CART_BARCODE_LENGTH {
            return Err(PickClusterError::InvalidCartBarcode);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PickCartName(String);

impl PickCartName {
    pub fn new(value: String) -> Result<Self, PickClusterError> {
        let value = value.trim().to_owned();
        if value.is_empty() || value.len() > MAX_PICK_CART_NAME_LENGTH {
            return Err(PickClusterError::InvalidCartName);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PickCartSlotCode(String);

impl PickCartSlotCode {
    pub fn new(value: String) -> Result<Self, PickClusterError> {
        let value = value.trim().to_ascii_uppercase();
        if value.is_empty() || value.len() > MAX_PICK_CART_SLOT_CODE_LENGTH {
            return Err(PickClusterError::InvalidSlotCode);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickExecutionMethod {
    Discrete,
    Case,
    Pallet,
    ClusterCart,
    BatchCart,
}

impl PickExecutionMethod {
    pub fn for_unclustered_uom(uom: &str) -> Self {
        if uom.eq_ignore_ascii_case("case") {
            Self::Case
        } else {
            Self::Discrete
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickRouteMode {
    ClusterCart,
    BatchCart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickBatchPlanLine {
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub item_batch_id: ItemBatchId,
    pub uom: String,
    pub inventory_status: String,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickBatchEvidence {
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub item_batch_id: ItemBatchId,
    pub uom: String,
    pub inventory_status: String,
    pub total_quantity: i64,
}

pub fn derive_pick_batch_evidence(
    lines: &[PickBatchPlanLine],
) -> Result<Option<PickBatchEvidence>, PickClusterError> {
    let Some(first) = lines.first() else {
        return Ok(None);
    };
    if !lines.iter().all(|line| {
        line.source_inventory_balance_id == first.source_inventory_balance_id
            && line.source_location_id == first.source_location_id
            && line.item_batch_id == first.item_batch_id
            && line.uom == first.uom
            && line.inventory_status == first.inventory_status
    }) {
        return Ok(None);
    }
    let total_quantity = lines.iter().try_fold(0_i64, |total, line| {
        if line.quantity <= 0 {
            return Err(PickClusterError::InvalidBatchQuantity);
        }
        total
            .checked_add(line.quantity)
            .ok_or(PickClusterError::BatchQuantityOverflow)
    })?;
    Ok(Some(PickBatchEvidence {
        source_inventory_balance_id: first.source_inventory_balance_id,
        source_location_id: first.source_location_id,
        item_batch_id: first.item_batch_id,
        uom: first.uom.clone(),
        inventory_status: first.inventory_status.clone(),
        total_quantity,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickCartStatus {
    Active,
    OutOfService,
    Retired,
}

impl PickCartStatus {
    pub const fn transition(self, next: Self) -> Result<Self, PickClusterError> {
        match (self, next) {
            (Self::Active, Self::OutOfService)
            | (Self::OutOfService, Self::Active)
            | (Self::Active, Self::Retired)
            | (Self::OutOfService, Self::Retired) => Ok(next),
            _ => Err(PickClusterError::InvalidCartTransition),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickClusterStatus {
    Planned,
    InProgress,
    Completed,
    Cancelled,
}

impl PickClusterStatus {
    pub const fn start(self) -> Result<Self, PickClusterError> {
        match self {
            Self::Planned => Ok(Self::InProgress),
            _ => Err(PickClusterError::InvalidClusterTransition),
        }
    }

    pub const fn complete(self) -> Result<Self, PickClusterError> {
        match self {
            Self::InProgress => Ok(Self::Completed),
            _ => Err(PickClusterError::InvalidClusterTransition),
        }
    }

    pub const fn cancel(self) -> Result<Self, PickClusterError> {
        match self {
            Self::Planned | Self::InProgress => Ok(Self::Cancelled),
            _ => Err(PickClusterError::InvalidClusterTransition),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickClusterPlanLine {
    pub task_id: PickTaskId,
    pub order_id: OrderId,
    pub slot_id: PickCartSlotId,
}

pub fn validate_pick_cluster_plan(lines: &[PickClusterPlanLine]) -> Result<(), PickClusterError> {
    if lines.len() < 2 || lines.len() > MAX_PICK_CLUSTER_TASKS {
        return Err(PickClusterError::InvalidTaskCount);
    }
    let mut task_ids = BTreeSet::new();
    let mut order_to_slot = BTreeMap::new();
    let mut slot_to_order = BTreeMap::new();
    for line in lines {
        if !task_ids.insert(line.task_id.get()) {
            return Err(PickClusterError::DuplicateTask);
        }
        if order_to_slot
            .insert(line.order_id.get(), line.slot_id.get())
            .is_some_and(|slot_id| slot_id != line.slot_id.get())
        {
            return Err(PickClusterError::OrderUsesMultipleSlots);
        }
        if slot_to_order
            .insert(line.slot_id.get(), line.order_id.get())
            .is_some_and(|order_id| order_id != line.order_id.get())
        {
            return Err(PickClusterError::SlotUsesMultipleOrders);
        }
    }
    if order_to_slot.len() < 2 {
        return Err(PickClusterError::MultipleOrdersRequired);
    }
    Ok(())
}

pub fn validate_pick_cart_slot_count(count: usize) -> Result<(), PickClusterError> {
    if (2..=MAX_PICK_CART_SLOTS).contains(&count) {
        Ok(())
    } else {
        Err(PickClusterError::InvalidSlotCount)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PickClusterError {
    #[error("pick cart barcode must be non-blank and no longer than 80 characters")]
    InvalidCartBarcode,
    #[error("pick cart name must be non-blank and no longer than 120 characters")]
    InvalidCartName,
    #[error("pick cart slot code must be non-blank and no longer than 40 characters")]
    InvalidSlotCode,
    #[error("a pick cart must have between 2 and 48 slots")]
    InvalidSlotCount,
    #[error("a pick cluster must contain between 2 and 200 tasks")]
    InvalidTaskCount,
    #[error("a pick cluster cannot contain the same task twice")]
    DuplicateTask,
    #[error("a pick cluster must contain at least two orders")]
    MultipleOrdersRequired,
    #[error("all tasks for one order must use the same cart slot")]
    OrderUsesMultipleSlots,
    #[error("one cart slot cannot hold more than one order in a cluster")]
    SlotUsesMultipleOrders,
    #[error("pick cart status transition is invalid")]
    InvalidCartTransition,
    #[error("pick cluster status transition is invalid")]
    InvalidClusterTransition,
    #[error("batch-cart quantities must be positive")]
    InvalidBatchQuantity,
    #[error("batch-cart total quantity exceeds the supported range")]
    BatchQuantityOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(task: i64, order: i64, slot: i64) -> PickClusterPlanLine {
        PickClusterPlanLine {
            task_id: PickTaskId::new(task).unwrap(),
            order_id: OrderId::new(order).unwrap(),
            slot_id: PickCartSlotId::new(slot).unwrap(),
        }
    }

    #[test]
    fn cluster_requires_distinct_orders_with_stable_slots() {
        assert!(validate_pick_cluster_plan(&[line(1, 10, 100), line(2, 20, 200)]).is_ok());
        assert_eq!(
            validate_pick_cluster_plan(&[line(1, 10, 100), line(2, 10, 200)]),
            Err(PickClusterError::OrderUsesMultipleSlots)
        );
        assert_eq!(
            validate_pick_cluster_plan(&[line(1, 10, 100), line(2, 20, 100)]),
            Err(PickClusterError::SlotUsesMultipleOrders)
        );
    }

    #[test]
    fn batch_evidence_requires_one_exact_inventory_identity() {
        let line = |balance, location, batch, quantity| PickBatchPlanLine {
            source_inventory_balance_id: InventoryBalanceId::new(balance).unwrap(),
            source_location_id: LocationId::new(location).unwrap(),
            item_batch_id: ItemBatchId::new(batch).unwrap(),
            uom: "each".into(),
            inventory_status: "available".into(),
            quantity,
        };
        let evidence = derive_pick_batch_evidence(&[line(9, 1, 2, 3), line(9, 1, 2, 4)])
            .unwrap()
            .unwrap();
        assert_eq!(evidence.total_quantity, 7);
        assert_eq!(
            derive_pick_batch_evidence(&[line(9, 1, 2, 3), line(9, 1, 3, 4)]).unwrap(),
            None
        );
        assert_eq!(
            derive_pick_batch_evidence(&[line(9, 1, 2, 0)]),
            Err(PickClusterError::InvalidBatchQuantity)
        );
    }

    #[test]
    fn case_uom_selects_explicit_case_execution() {
        assert_eq!(
            PickExecutionMethod::for_unclustered_uom("case"),
            PickExecutionMethod::Case
        );
        assert_eq!(
            PickExecutionMethod::for_unclustered_uom("CASE"),
            PickExecutionMethod::Case
        );
        assert_eq!(
            PickExecutionMethod::for_unclustered_uom("each"),
            PickExecutionMethod::Discrete
        );
    }

    #[test]
    fn cart_and_cluster_lifecycles_are_explicit() {
        assert_eq!(
            PickCartStatus::Active.transition(PickCartStatus::OutOfService),
            Ok(PickCartStatus::OutOfService)
        );
        assert!(PickCartStatus::Retired
            .transition(PickCartStatus::Active)
            .is_err());
        assert_eq!(
            PickClusterStatus::Planned.start(),
            Ok(PickClusterStatus::InProgress)
        );
        assert!(PickClusterStatus::Completed.cancel().is_err());
    }
}
