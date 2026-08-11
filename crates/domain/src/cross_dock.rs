use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::OrderStatus;

pub const MAX_CROSS_DOCK_UOM_LENGTH: usize = 32;
pub const MAX_CROSS_DOCK_SCAN_VALUE_LENGTH: usize = 200;
pub const MAX_CROSS_DOCK_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CrossDockUom(String);

impl CrossDockUom {
    pub fn new(value: impl Into<String>) -> Result<Self, CrossDockError> {
        let value = value.into();
        validate_text(&value, MAX_CROSS_DOCK_UOM_LENGTH)
            .map_err(|()| CrossDockError::InvalidUom)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CrossDockUom {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CrossDockQuantity(i64);

impl CrossDockQuantity {
    pub const fn new(value: i64) -> Result<Self, CrossDockError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(CrossDockError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for CrossDockQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CrossDockScanValue(String);

impl CrossDockScanValue {
    pub fn new(value: impl Into<String>) -> Result<Self, CrossDockError> {
        let value = value.into();
        validate_text(&value, MAX_CROSS_DOCK_SCAN_VALUE_LENGTH)
            .map_err(|()| CrossDockError::InvalidScanValue)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CrossDockScanValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CrossDockNote(String);

impl CrossDockNote {
    pub fn new(value: impl Into<String>) -> Result<Self, CrossDockError> {
        let value = value.into();
        validate_text(&value, MAX_CROSS_DOCK_NOTE_LENGTH)
            .map_err(|()| CrossDockError::InvalidNote)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CrossDockNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossDockWorkStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl CrossDockWorkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "open" | "assigned" | "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossDockClaimReleaseReason {
    WorkInterrupted,
    EndOfShift,
    EquipmentIssue,
    Other,
}

impl CrossDockClaimReleaseReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkInterrupted => "work_interrupted",
            Self::EndOfShift => "end_of_shift",
            Self::EquipmentIssue => "equipment_issue",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossDockCancellationReason {
    DemandChanged,
    ReceiptReassigned,
    OperationalChange,
    Other,
}

impl CrossDockCancellationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DemandChanged => "demand_changed",
            Self::ReceiptReassigned => "receipt_reassigned",
            Self::OperationalChange => "operational_change",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockCancellationDetails {
    pub reason: CrossDockCancellationReason,
    pub note: Option<CrossDockNote>,
}

impl CrossDockCancellationDetails {
    pub fn new(
        reason: CrossDockCancellationReason,
        note: Option<CrossDockNote>,
    ) -> Result<Self, CrossDockError> {
        if reason == CrossDockCancellationReason::Other && note.is_none() {
            return Err(CrossDockError::OtherCancellationRequiresNote);
        }
        Ok(Self { reason, note })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockPlanningSnapshot {
    pub order_status: OrderStatus,
    pub reservation_quantity: i64,
    pub allocated_quantity: i64,
    pub active_cross_dock_quantity: i64,
    pub source_free_quantity: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockPlanDecision {
    pub planned_quantity: CrossDockQuantity,
    pub remaining_unallocated_quantity: i64,
    pub remaining_source_free_quantity: i64,
}

pub fn plan_cross_dock(
    requested: CrossDockQuantity,
    snapshot: CrossDockPlanningSnapshot,
) -> Result<CrossDockPlanDecision, CrossDockError> {
    if snapshot.order_status != OrderStatus::Open {
        return Err(CrossDockError::OrderNotOpen {
            status: snapshot.order_status,
        });
    }
    let snapshot_is_invalid = snapshot.reservation_quantity <= 0
        || snapshot.allocated_quantity < 0
        || snapshot.active_cross_dock_quantity < 0
        || snapshot.source_free_quantity < 0
        || snapshot
            .allocated_quantity
            .checked_add(snapshot.active_cross_dock_quantity)
            .is_none_or(|committed| committed > snapshot.reservation_quantity);
    if snapshot_is_invalid {
        return Err(CrossDockError::InvalidPlanningSnapshot);
    }
    let remaining_demand = snapshot.reservation_quantity
        - snapshot.allocated_quantity
        - snapshot.active_cross_dock_quantity;
    if requested.get() > remaining_demand {
        return Err(CrossDockError::ExceedsUnallocatedDemand {
            requested: requested.get(),
            remaining: remaining_demand,
        });
    }
    if requested.get() > snapshot.source_free_quantity {
        return Err(CrossDockError::ExceedsSourceFreeQuantity {
            requested: requested.get(),
            available: snapshot.source_free_quantity,
        });
    }
    Ok(CrossDockPlanDecision {
        planned_quantity: requested,
        remaining_unallocated_quantity: remaining_demand - requested.get(),
        remaining_source_free_quantity: snapshot.source_free_quantity - requested.get(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CrossDockError {
    #[error("cross-dock UOM is invalid")]
    InvalidUom,
    #[error("cross-dock scan value is invalid")]
    InvalidScanValue,
    #[error("cross-dock note is invalid")]
    InvalidNote,
    #[error("cross-dock quantity must be positive, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("cross-dock work requires an open order, got {status}")]
    OrderNotOpen { status: OrderStatus },
    #[error("cross-dock planning snapshot is inconsistent")]
    InvalidPlanningSnapshot,
    #[error("requested quantity {requested} exceeds unallocated demand {remaining}")]
    ExceedsUnallocatedDemand { requested: i64, remaining: i64 },
    #[error("requested quantity {requested} exceeds source free quantity {available}")]
    ExceedsSourceFreeQuantity { requested: i64, available: i64 },
    #[error("other cross-dock cancellation requires a note")]
    OtherCancellationRequiresNote,
}

fn validate_text(value: &str, max_chars: usize) -> Result<(), ()> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > max_chars
        || value.chars().any(char::is_control)
    {
        Err(())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_conserves_demand_and_received_stock() {
        let decision = plan_cross_dock(
            CrossDockQuantity::new(5).unwrap(),
            CrossDockPlanningSnapshot {
                order_status: OrderStatus::Open,
                reservation_quantity: 12,
                allocated_quantity: 3,
                active_cross_dock_quantity: 2,
                source_free_quantity: 8,
            },
        )
        .unwrap();
        assert_eq!(decision.remaining_unallocated_quantity, 2);
        assert_eq!(decision.remaining_source_free_quantity, 3);
    }

    #[test]
    fn planning_rejects_overcommit_and_non_open_orders() {
        let base = CrossDockPlanningSnapshot {
            order_status: OrderStatus::Open,
            reservation_quantity: 6,
            allocated_quantity: 3,
            active_cross_dock_quantity: 2,
            source_free_quantity: 4,
        };
        assert!(matches!(
            plan_cross_dock(CrossDockQuantity::new(2).unwrap(), base),
            Err(CrossDockError::ExceedsUnallocatedDemand { .. })
        ));
        assert!(matches!(
            plan_cross_dock(
                CrossDockQuantity::new(1).unwrap(),
                CrossDockPlanningSnapshot {
                    order_status: OrderStatus::Held,
                    ..base
                }
            ),
            Err(CrossDockError::OrderNotOpen { .. })
        ));
    }

    #[test]
    fn cancellation_requires_bounded_other_note() {
        assert!(
            CrossDockCancellationDetails::new(CrossDockCancellationReason::Other, None).is_err()
        );
        assert!(CrossDockCancellationDetails::new(
            CrossDockCancellationReason::Other,
            Some(CrossDockNote::new("Customer rerouted the order").unwrap())
        )
        .is_ok());
    }
}
