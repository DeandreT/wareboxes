use std::collections::HashSet;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{OrderLineId, OrderRevision, OrderStatus};

pub const MAX_BACKORDER_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackorderPolicyMode {
    Block,
    SplitShortage,
}

impl BackorderPolicyMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::SplitShortage => "split_shortage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "block" => Some(Self::Block),
            "split_shortage" => Some(Self::SplitShortage),
            _ => None,
        }
    }
}

impl fmt::Display for BackorderPolicyMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackorderReason {
    InventoryUnavailable,
    ClientRequested,
    ServiceLevel,
    Other,
}

impl BackorderReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InventoryUnavailable => "inventory_unavailable",
            Self::ClientRequested => "client_requested",
            Self::ServiceLevel => "service_level",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inventory_unavailable" => Some(Self::InventoryUnavailable),
            "client_requested" => Some(Self::ClientRequested),
            "service_level" => Some(Self::ServiceLevel),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BackorderNote(String);

impl BackorderNote {
    pub fn new(value: impl Into<String>) -> Result<Self, BackorderError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_BACKORDER_NOTE_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(BackorderError::InvalidNote);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BackorderNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackorderDetails {
    pub reason: BackorderReason,
    pub note: Option<BackorderNote>,
}

impl BackorderDetails {
    pub fn new(
        reason: BackorderReason,
        note: Option<BackorderNote>,
    ) -> Result<Self, BackorderError> {
        if reason == BackorderReason::Other && note.is_none() {
            return Err(BackorderError::OtherRequiresNote);
        }
        Ok(Self { reason, note })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct BackorderPolicyRevision(i64);

impl BackorderPolicyRevision {
    pub const fn new(value: i64) -> Result<Self, BackorderError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(BackorderError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for BackorderPolicyRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Server-observed quantity state for one parent demand line before a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackorderLineSnapshot {
    pub order_line_id: OrderLineId,
    pub original_quantity: i64,
    pub previously_backordered_quantity: i64,
    pub effective_quantity: i64,
    pub allocated_quantity: i64,
}

impl BackorderLineSnapshot {
    pub const fn shortage_quantity(self) -> Option<i64> {
        self.effective_quantity.checked_sub(self.allocated_quantity)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackorderSplitLineTransition {
    pub order_line_id: OrderLineId,
    pub original_quantity: i64,
    pub allocated_quantity: i64,
    pub previously_backordered_quantity: i64,
    pub newly_backordered_quantity: i64,
    pub resulting_backordered_quantity: i64,
    pub resulting_effective_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackorderSplitTransition {
    pub parent_revision: OrderRevision,
    pub child_revision: OrderRevision,
    pub original_quantity: i64,
    pub allocated_quantity: i64,
    pub previously_backordered_quantity: i64,
    pub newly_backordered_quantity: i64,
    pub parent_effective_quantity: i64,
    pub lines: Vec<BackorderSplitLineTransition>,
}

pub fn split_current_allocation_shortage(
    status: OrderStatus,
    revision: OrderRevision,
    policy: BackorderPolicyMode,
    lines: &[BackorderLineSnapshot],
) -> Result<BackorderSplitTransition, BackorderError> {
    if status != OrderStatus::Open {
        return Err(BackorderError::OrderNotOpen { status });
    }
    if policy != BackorderPolicyMode::SplitShortage {
        return Err(BackorderError::PolicyBlocksSplit);
    }
    if lines.is_empty() {
        return Err(BackorderError::MissingLines);
    }

    let mut seen = HashSet::with_capacity(lines.len());
    let mut transitions = Vec::with_capacity(lines.len());
    let mut original_total = 0_i64;
    let mut allocated_total = 0_i64;
    let mut parent_effective_total = 0_i64;
    let mut previous_backorder_total = 0_i64;
    let mut new_backorder_total = 0_i64;

    for line in lines {
        if !seen.insert(line.order_line_id)
            || line.original_quantity <= 0
            || line.previously_backordered_quantity < 0
            || line.effective_quantity <= 0
            || line.allocated_quantity < 0
            || line.previously_backordered_quantity + line.effective_quantity
                != line.original_quantity
            || line.allocated_quantity > line.effective_quantity
        {
            return Err(BackorderError::InvalidLineState {
                order_line_id: line.order_line_id,
            });
        }
        let shortage = line
            .shortage_quantity()
            .ok_or(BackorderError::QuantityOverflow)?;
        parent_effective_total = parent_effective_total
            .checked_add(line.allocated_quantity)
            .ok_or(BackorderError::QuantityOverflow)?;
        if shortage == 0 {
            continue;
        }
        let resulting_backordered = line
            .previously_backordered_quantity
            .checked_add(shortage)
            .ok_or(BackorderError::QuantityOverflow)?;
        original_total = original_total
            .checked_add(line.original_quantity)
            .ok_or(BackorderError::QuantityOverflow)?;
        allocated_total = allocated_total
            .checked_add(line.allocated_quantity)
            .ok_or(BackorderError::QuantityOverflow)?;
        previous_backorder_total = previous_backorder_total
            .checked_add(line.previously_backordered_quantity)
            .ok_or(BackorderError::QuantityOverflow)?;
        new_backorder_total = new_backorder_total
            .checked_add(shortage)
            .ok_or(BackorderError::QuantityOverflow)?;
        transitions.push(BackorderSplitLineTransition {
            order_line_id: line.order_line_id,
            original_quantity: line.original_quantity,
            allocated_quantity: line.allocated_quantity,
            previously_backordered_quantity: line.previously_backordered_quantity,
            newly_backordered_quantity: shortage,
            resulting_backordered_quantity: resulting_backordered,
            resulting_effective_quantity: line.allocated_quantity,
        });
    }
    if new_backorder_total <= 0 {
        return Err(BackorderError::NoShortage);
    }
    if parent_effective_total <= 0 {
        return Err(BackorderError::ZeroEffectiveDemand);
    }
    let parent_revision = revision
        .checked_next()
        .ok_or(BackorderError::RevisionOverflow)?;
    Ok(BackorderSplitTransition {
        parent_revision,
        child_revision: OrderRevision::new(1).map_err(|_| BackorderError::RevisionOverflow)?,
        original_quantity: original_total,
        allocated_quantity: allocated_total,
        previously_backordered_quantity: previous_backorder_total,
        newly_backordered_quantity: new_backorder_total,
        parent_effective_quantity: parent_effective_total,
        lines: transitions,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BackorderError {
    #[error("backorder policy revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("backorder note must be trimmed, nonblank, printable, and at most 500 characters")]
    InvalidNote,
    #[error("backorder reason Other requires a note")]
    OtherRequiresNote,
    #[error("order status {status} does not allow a pre-release backorder split")]
    OrderNotOpen { status: OrderStatus },
    #[error("the active backorder policy blocks shortage splitting")]
    PolicyBlocksSplit,
    #[error("backorder split requires at least one demand line")]
    MissingLines,
    #[error("order line {order_line_id} has inconsistent backorder quantities")]
    InvalidLineState { order_line_id: OrderLineId },
    #[error("the order has no current allocation shortage")]
    NoShortage,
    #[error("a backorder split cannot reduce the parent order to zero demand")]
    ZeroEffectiveDemand,
    #[error("backorder quantity exceeds supported range")]
    QuantityOverflow,
    #[error("backorder revision exceeds supported range")]
    RevisionOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_derives_only_current_shortage_and_preserves_allocated_parent() {
        let transition = split_current_allocation_shortage(
            OrderStatus::Open,
            OrderRevision::new(4).unwrap(),
            BackorderPolicyMode::SplitShortage,
            &[
                BackorderLineSnapshot {
                    order_line_id: OrderLineId::new(1).unwrap(),
                    original_quantity: 10,
                    previously_backordered_quantity: 0,
                    effective_quantity: 10,
                    allocated_quantity: 6,
                },
                BackorderLineSnapshot {
                    order_line_id: OrderLineId::new(2).unwrap(),
                    original_quantity: 5,
                    previously_backordered_quantity: 1,
                    effective_quantity: 4,
                    allocated_quantity: 4,
                },
            ],
        )
        .unwrap();
        assert_eq!(transition.parent_revision.get(), 5);
        assert_eq!(transition.newly_backordered_quantity, 4);
        assert_eq!(transition.previously_backordered_quantity, 0);
        assert_eq!(transition.parent_effective_quantity, 10);
        assert_eq!(transition.lines[0].resulting_effective_quantity, 6);
        assert_eq!(transition.lines.len(), 1);
    }

    #[test]
    fn split_rejects_block_policy_no_shortage_and_zero_parent() {
        let line = BackorderLineSnapshot {
            order_line_id: OrderLineId::new(1).unwrap(),
            original_quantity: 4,
            previously_backordered_quantity: 0,
            effective_quantity: 4,
            allocated_quantity: 0,
        };
        assert_eq!(
            split_current_allocation_shortage(
                OrderStatus::Open,
                OrderRevision::new(1).unwrap(),
                BackorderPolicyMode::Block,
                &[line]
            ),
            Err(BackorderError::PolicyBlocksSplit)
        );
        assert_eq!(
            split_current_allocation_shortage(
                OrderStatus::Open,
                OrderRevision::new(1).unwrap(),
                BackorderPolicyMode::SplitShortage,
                &[line]
            ),
            Err(BackorderError::ZeroEffectiveDemand)
        );
        let full = BackorderLineSnapshot {
            allocated_quantity: 4,
            ..line
        };
        assert_eq!(
            split_current_allocation_shortage(
                OrderStatus::Open,
                OrderRevision::new(1).unwrap(),
                BackorderPolicyMode::SplitShortage,
                &[full]
            ),
            Err(BackorderError::NoShortage)
        );
    }

    #[test]
    fn other_reason_requires_a_bounded_note() {
        assert_eq!(
            BackorderDetails::new(BackorderReason::Other, None),
            Err(BackorderError::OtherRequiresNote)
        );
        assert!(BackorderDetails::new(
            BackorderReason::Other,
            Some(BackorderNote::new("Client approved split").unwrap())
        )
        .is_ok());
        assert!(BackorderNote::new(" ").is_err());
    }
}
