use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

pub const MAX_PICK_SCAN_VALUE_LENGTH: usize = 200;

/// Completion state of one immutable piece of directed pick work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickContentState {
    Pending,
    Completed,
}

impl PickContentState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }

    pub const fn complete(self) -> Result<Self, PickingError> {
        match self {
            Self::Pending => Ok(Self::Completed),
            Self::Completed => Err(PickingError::ContentAlreadyCompleted),
        }
    }
}

/// Operator reason for returning an active pick claim to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SourceBlocked,
    InventoryDiscrepancy,
    SafetyIssue,
    Other,
}

/// Positive quantity for planned and confirmed pick work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PickQuantity(i64);

impl PickQuantity {
    pub const fn new(value: i64) -> Result<Self, PickingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PickingError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for PickQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Exact scannable value supplied by an RF operator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PickScanValue(String);

impl PickScanValue {
    pub fn new(value: impl Into<String>) -> Result<Self, PickingError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PickingError::EmptyScanValue);
        }
        if value.trim() != value {
            return Err(PickingError::UntrimmedScanValue);
        }
        if value.chars().count() > MAX_PICK_SCAN_VALUE_LENGTH {
            return Err(PickingError::ScanValueTooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(PickingError::InvalidScanCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for PickScanValue {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for PickScanValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PickScanValue {
    type Err = PickingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for PickScanValue {
    type Error = PickingError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for PickScanValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PickingError {
    #[error("pick quantity must be a positive integer, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("pick scan value cannot be empty")]
    EmptyScanValue,
    #[error("pick scan value must be trimmed")]
    UntrimmedScanValue,
    #[error("pick scan value cannot exceed {MAX_PICK_SCAN_VALUE_LENGTH} characters")]
    ScanValueTooLong,
    #[error("pick scan value cannot contain control characters")]
    InvalidScanCharacter,
    #[error("pick content is already completed")]
    ContentAlreadyCompleted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_are_positive_and_strict_when_deserialized() {
        assert_eq!(PickQuantity::new(4).map(PickQuantity::get), Ok(4));
        assert_eq!(
            PickQuantity::new(0),
            Err(PickingError::InvalidQuantity { value: 0 })
        );
    }

    #[test]
    fn scans_are_exact_printable_values() {
        let scan = PickScanValue::new("A-01-01").unwrap();
        assert_eq!(scan.as_str(), "A-01-01");

        assert_eq!(PickScanValue::new(""), Err(PickingError::EmptyScanValue));
        assert_eq!(
            PickScanValue::new(" A-01"),
            Err(PickingError::UntrimmedScanValue)
        );
        assert_eq!(
            PickScanValue::new("A\n01"),
            Err(PickingError::InvalidScanCharacter)
        );
        assert_eq!(
            PickScanValue::new("x".repeat(MAX_PICK_SCAN_VALUE_LENGTH + 1)),
            Err(PickingError::ScanValueTooLong)
        );
    }

    #[test]
    fn pick_content_can_only_complete_once() {
        assert_eq!(
            PickContentState::Pending.complete(),
            Ok(PickContentState::Completed)
        );
        assert_eq!(
            PickContentState::Completed.complete(),
            Err(PickingError::ContentAlreadyCompleted)
        );
    }
}
