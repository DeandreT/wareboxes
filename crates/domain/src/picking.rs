use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_PICK_SCAN_VALUE_LENGTH: usize = 200;
pub const MAX_PICK_SHORTAGE_NOTE_LENGTH: usize = 500;

/// Completion state of one immutable piece of directed pick work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickContentState {
    Pending,
    Completed,
    Shorted,
}

impl PickContentState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Shorted => "shorted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "completed" => Some(Self::Completed),
            "shorted" => Some(Self::Shorted),
            _ => None,
        }
    }

    pub const fn complete(self) -> Result<Self, PickingError> {
        match self {
            Self::Pending => Ok(Self::Completed),
            Self::Completed => Err(PickingError::ContentAlreadyCompleted),
            Self::Shorted => Err(PickingError::ContentAlreadyShorted),
        }
    }

    pub const fn short(self) -> Result<Self, PickingError> {
        match self {
            Self::Pending => Ok(Self::Shorted),
            Self::Completed => Err(PickingError::ContentAlreadyCompleted),
            Self::Shorted => Err(PickingError::ContentAlreadyShorted),
        }
    }
}

/// Physical reason an operator could not complete the directed quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageReason {
    InventoryMissing,
    InsufficientQuantity,
    DamagedInventory,
    WrongInventory,
    LotOrSerialMismatch,
    Other,
}

impl PickShortageReason {
    pub const ALL: [Self; 6] = [
        Self::InventoryMissing,
        Self::InsufficientQuantity,
        Self::DamagedInventory,
        Self::WrongInventory,
        Self::LotOrSerialMismatch,
        Self::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InventoryMissing => "inventory_missing",
            Self::InsufficientQuantity => "insufficient_quantity",
            Self::DamagedInventory => "damaged_inventory",
            Self::WrongInventory => "wrong_inventory",
            Self::LotOrSerialMismatch => "lot_or_serial_mismatch",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inventory_missing" => Some(Self::InventoryMissing),
            "insufficient_quantity" => Some(Self::InsufficientQuantity),
            "damaged_inventory" => Some(Self::DamagedInventory),
            "wrong_inventory" => Some(Self::WrongInventory),
            "lot_or_serial_mismatch" => Some(Self::LotOrSerialMismatch),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub const fn requires_note(self) -> bool {
        matches!(self, Self::Other)
    }
}

/// Lifecycle state of a reported pick shortage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageStatus {
    AwaitingInventory,
    RecoveryInProgress,
    Resolved,
}

impl PickShortageStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingInventory => "awaiting_inventory",
            Self::RecoveryInProgress => "recovery_in_progress",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "awaiting_inventory" => Some(Self::AwaitingInventory),
            "recovery_in_progress" => Some(Self::RecoveryInProgress),
            "resolved" => Some(Self::Resolved),
            _ => None,
        }
    }
}

/// Positive optimistic revision of one pick-shortage exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PickShortageRevision(i64);

impl PickShortageRevision {
    pub const fn new(value: i64) -> Result<Self, PickingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PickingError::InvalidShortageRevision { value })
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

impl<'de> Deserialize<'de> for PickShortageRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Actual quantity physically found by the picker, including zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ActualPickQuantity(i64);

impl ActualPickQuantity {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: i64) -> Result<Self, PickingError> {
        if value >= 0 {
            Ok(Self(value))
        } else {
            Err(PickingError::InvalidActualQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl From<PickQuantity> for ActualPickQuantity {
    fn from(value: PickQuantity) -> Self {
        Self(value.get())
    }
}

impl<'de> Deserialize<'de> for ActualPickQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Conserved planned, picked, and short quantities for one exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PickShortageQuantities {
    planned: PickQuantity,
    picked: ActualPickQuantity,
    short: PickQuantity,
}

impl PickShortageQuantities {
    pub const fn new(
        planned: PickQuantity,
        picked: ActualPickQuantity,
    ) -> Result<Self, PickingError> {
        if picked.get() >= planned.get() {
            return Err(PickingError::PickIsNotShort {
                planned: planned.get(),
                picked: picked.get(),
            });
        }
        let short_value = planned.get() - picked.get();
        Ok(Self {
            planned,
            picked,
            short: PickQuantity(short_value),
        })
    }

    pub const fn planned(self) -> PickQuantity {
        self.planned
    }

    pub const fn picked(self) -> ActualPickQuantity {
        self.picked
    }

    pub const fn short(self) -> PickQuantity {
        self.short
    }
}

impl<'de> Deserialize<'de> for PickShortageQuantities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawQuantities {
            planned: PickQuantity,
            picked: ActualPickQuantity,
            short: PickQuantity,
        }

        let raw = RawQuantities::deserialize(deserializer)?;
        let quantities = Self::new(raw.planned, raw.picked).map_err(D::Error::custom)?;
        if quantities.short != raw.short {
            return Err(D::Error::custom("pick shortage quantity does not conserve"));
        }
        Ok(quantities)
    }
}

/// Trimmed, nonblank operator context for a pick shortage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PickShortageNote(String);

impl PickShortageNote {
    pub fn new(value: impl Into<String>) -> Result<Self, PickingError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value {
            return Err(PickingError::InvalidShortageNote);
        }
        if value.chars().count() > MAX_PICK_SHORTAGE_NOTE_LENGTH {
            return Err(PickingError::ShortageNoteTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PickShortageNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Validated operator reason and optional context for a pick shortage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PickShortageDetails {
    reason: PickShortageReason,
    note: Option<PickShortageNote>,
}

impl PickShortageDetails {
    pub fn new(
        reason: PickShortageReason,
        note: Option<PickShortageNote>,
    ) -> Result<Self, PickingError> {
        if reason.requires_note() && note.is_none() {
            return Err(PickingError::ShortageNoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> PickShortageReason {
        self.reason
    }

    pub fn note(&self) -> Option<&PickShortageNote> {
        self.note.as_ref()
    }
}

impl<'de> Deserialize<'de> for PickShortageDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawDetails {
            reason: PickShortageReason,
            note: Option<PickShortageNote>,
        }

        let raw = RawDetails::deserialize(deserializer)?;
        Self::new(raw.reason, raw.note).map_err(D::Error::custom)
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
    #[error("pick content is already shorted")]
    ContentAlreadyShorted,
    #[error("actual pick quantity cannot be negative, got {value}")]
    InvalidActualQuantity { value: i64 },
    #[error("pick shortage revision must be positive, got {value}")]
    InvalidShortageRevision { value: i64 },
    #[error("actual pick quantity {picked} must be less than planned quantity {planned}")]
    PickIsNotShort { planned: i64, picked: i64 },
    #[error("pick shortage note must be trimmed and nonblank")]
    InvalidShortageNote,
    #[error("pick shortage note cannot exceed {MAX_PICK_SHORTAGE_NOTE_LENGTH} characters")]
    ShortageNoteTooLong,
    #[error("pick shortage reason other requires a note")]
    ShortageNoteRequired,
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

    #[test]
    fn shorting_is_a_terminal_content_transition() {
        assert_eq!(
            PickContentState::Pending.short(),
            Ok(PickContentState::Shorted)
        );
        assert_eq!(
            PickContentState::Shorted.complete(),
            Err(PickingError::ContentAlreadyShorted)
        );
        assert_eq!(
            PickContentState::parse("shorted"),
            Some(PickContentState::Shorted)
        );
        assert_eq!(PickContentState::Shorted.as_str(), "shorted");
    }

    #[test]
    fn shortage_quantities_are_nonnegative_short_and_conserved() {
        let quantities = PickShortageQuantities::new(
            PickQuantity::new(7).unwrap(),
            ActualPickQuantity::new(2).unwrap(),
        )
        .unwrap();
        assert_eq!(quantities.planned().get(), 7);
        assert_eq!(quantities.picked().get(), 2);
        assert_eq!(quantities.short().get(), 5);
        assert_eq!(
            ActualPickQuantity::new(-1),
            Err(PickingError::InvalidActualQuantity { value: -1 })
        );
        assert_eq!(
            PickShortageQuantities::new(
                PickQuantity::new(7).unwrap(),
                ActualPickQuantity::new(7).unwrap(),
            ),
            Err(PickingError::PickIsNotShort {
                planned: 7,
                picked: 7,
            })
        );
        assert!(serde_json::from_str::<PickShortageQuantities>(
            r#"{"planned":7,"picked":2,"short":4}"#
        )
        .is_err());
    }

    #[test]
    fn shortage_reasons_and_statuses_have_stable_wire_values() {
        for reason in PickShortageReason::ALL {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(json, format!("\"{}\"", reason.as_str()));
            assert_eq!(PickShortageReason::parse(reason.as_str()), Some(reason));
        }
        assert_eq!(
            serde_json::to_string(&PickShortageStatus::RecoveryInProgress).unwrap(),
            r#""recovery_in_progress""#
        );
        assert_eq!(
            PickShortageStatus::parse("awaiting_inventory"),
            Some(PickShortageStatus::AwaitingInventory)
        );
    }

    #[test]
    fn other_shortage_reason_requires_bounded_trimmed_context() {
        assert_eq!(
            PickShortageDetails::new(PickShortageReason::Other, None),
            Err(PickingError::ShortageNoteRequired)
        );
        assert_eq!(
            PickShortageNote::new(" padded "),
            Err(PickingError::InvalidShortageNote)
        );
        assert_eq!(
            PickShortageNote::new("x".repeat(MAX_PICK_SHORTAGE_NOTE_LENGTH + 1)),
            Err(PickingError::ShortageNoteTooLong)
        );

        let details = PickShortageDetails::new(
            PickShortageReason::Other,
            Some(PickShortageNote::new("Cycle count requested").unwrap()),
        )
        .unwrap();
        assert_eq!(details.reason(), PickShortageReason::Other);
        assert_eq!(
            details.note().map(PickShortageNote::as_str),
            Some("Cycle count requested")
        );
    }
}
