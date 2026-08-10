use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_INBOUND_INSPECTION_NOTE_LENGTH: usize = 500;

/// Terminal disposition for one exact quarantined receipt hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundInspectionOutcome {
    Approved,
    Damaged,
}

impl InboundInspectionOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Damaged => "damaged",
        }
    }

    pub const fn target_status(self) -> InboundInspectionTargetStatus {
        match self {
            Self::Approved => InboundInspectionTargetStatus::Available,
            Self::Damaged => InboundInspectionTargetStatus::Damaged,
        }
    }
}

/// Inventory disposition produced by an inspection outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundInspectionTargetStatus {
    Available,
    Damaged,
}

impl InboundInspectionTargetStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Damaged => "damaged",
        }
    }
}

/// Trimmed operator evidence retained with the immutable disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct InboundInspectionNote(String);

impl InboundInspectionNote {
    pub fn new(value: impl Into<String>) -> Result<Self, InboundInspectionError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_INBOUND_INSPECTION_NOTE_LENGTH
        {
            return Err(InboundInspectionError::InvalidNote);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for InboundInspectionNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundInspectionError {
    InvalidNote,
    InvalidQuantity,
    HoldNotActive,
    StockNotQuarantined,
}

impl fmt::Display for InboundInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidNote => {
                "inspection note must be trimmed, nonempty, and at most 500 characters"
            }
            Self::InvalidQuantity => "inspection quantity must be positive",
            Self::HoldNotActive => "inbound inspection hold is not active",
            Self::StockNotQuarantined => "inbound inspection stock is not quarantined",
        })
    }
}

impl std::error::Error for InboundInspectionError {}

/// Pure eligibility and target-status decision for a whole receipt hold.
pub fn decide_inbound_inspection(
    hold_is_active: bool,
    source_is_quarantined: bool,
    quantity: i64,
    outcome: InboundInspectionOutcome,
) -> Result<InboundInspectionTargetStatus, InboundInspectionError> {
    if !hold_is_active {
        return Err(InboundInspectionError::HoldNotActive);
    }
    if !source_is_quarantined {
        return Err(InboundInspectionError::StockNotQuarantined);
    }
    if quantity <= 0 {
        return Err(InboundInspectionError::InvalidQuantity);
    }
    Ok(outcome.target_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_hold_decision_is_exact() {
        assert_eq!(
            decide_inbound_inspection(true, true, 3, InboundInspectionOutcome::Approved),
            Ok(InboundInspectionTargetStatus::Available)
        );
        assert_eq!(
            decide_inbound_inspection(true, true, 3, InboundInspectionOutcome::Damaged),
            Ok(InboundInspectionTargetStatus::Damaged)
        );
        assert_eq!(
            decide_inbound_inspection(false, true, 3, InboundInspectionOutcome::Approved),
            Err(InboundInspectionError::HoldNotActive)
        );
        assert_eq!(
            decide_inbound_inspection(true, false, 3, InboundInspectionOutcome::Approved),
            Err(InboundInspectionError::StockNotQuarantined)
        );
        assert_eq!(
            decide_inbound_inspection(true, true, 0, InboundInspectionOutcome::Approved),
            Err(InboundInspectionError::InvalidQuantity)
        );
    }

    #[test]
    fn inspection_note_is_bounded_and_canonical() {
        assert_eq!(
            InboundInspectionNote::new("Checked seals")
                .unwrap()
                .as_str(),
            "Checked seals"
        );
        assert!(InboundInspectionNote::new("").is_err());
        assert!(InboundInspectionNote::new(" padded ").is_err());
        assert!(InboundInspectionNote::new("x".repeat(501)).is_err());
    }
}
