//! Facility-scoped inventory recall lifecycle invariants.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_INVENTORY_RECALL_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRecallStatus {
    Active,
    Released,
}

impl InventoryRecallStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRecallReason {
    Regulatory,
    SupplierNotice,
    CustomerRequest,
    QualityConcern,
    Other,
}

impl InventoryRecallReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Regulatory => "regulatory",
            Self::SupplierNotice => "supplier_notice",
            Self::CustomerRequest => "customer_request",
            Self::QualityConcern => "quality_concern",
            Self::Other => "other",
        }
    }

    pub const fn requires_note(self) -> bool {
        matches!(self, Self::Other)
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "regulatory" => Some(Self::Regulatory),
            "supplier_notice" => Some(Self::SupplierNotice),
            "customer_request" => Some(Self::CustomerRequest),
            "quality_concern" => Some(Self::QualityConcern),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct InventoryRecallNote(String);

impl InventoryRecallNote {
    pub fn new(value: impl Into<String>) -> Result<Self, InventoryRecallError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value {
            return Err(InventoryRecallError::InvalidNote);
        }
        if value.chars().count() > MAX_INVENTORY_RECALL_NOTE_LENGTH {
            return Err(InventoryRecallError::NoteTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for InventoryRecallNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InventoryRecallDetails {
    reason: InventoryRecallReason,
    note: Option<InventoryRecallNote>,
}

impl InventoryRecallDetails {
    pub fn new(
        reason: InventoryRecallReason,
        note: Option<InventoryRecallNote>,
    ) -> Result<Self, InventoryRecallError> {
        if reason.requires_note() && note.is_none() {
            return Err(InventoryRecallError::NoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> InventoryRecallReason {
        self.reason
    }

    pub fn note(&self) -> Option<&InventoryRecallNote> {
        self.note.as_ref()
    }
}

impl<'de> Deserialize<'de> for InventoryRecallDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            reason: InventoryRecallReason,
            note: Option<InventoryRecallNote>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::new(raw.reason, raw.note).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct InventoryRecallRevision(i64);

impl InventoryRecallRevision {
    pub const fn new(value: i64) -> Result<Self, InventoryRecallError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(InventoryRecallError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for InventoryRecallRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

pub fn release_inventory_recall(
    status: InventoryRecallStatus,
    revision: InventoryRecallRevision,
) -> Result<InventoryRecallRevision, InventoryRecallError> {
    if status != InventoryRecallStatus::Active {
        return Err(InventoryRecallError::NotActive { status });
    }
    revision
        .checked_next()
        .ok_or(InventoryRecallError::RevisionOverflow)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InventoryRecallError {
    #[error("inventory recall revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("inventory recall note must be trimmed and nonempty")]
    InvalidNote,
    #[error("inventory recall note cannot exceed {MAX_INVENTORY_RECALL_NOTE_LENGTH} characters")]
    NoteTooLong,
    #[error("inventory recall note is required when reason is other")]
    NoteRequired,
    #[error("inventory recall must be active to release, got {status:?}")]
    NotActive { status: InventoryRecallStatus },
    #[error("inventory recall revision overflow")]
    RevisionOverflow,
}

impl fmt::Display for InventoryRecallStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_reason_requires_bounded_context() {
        assert_eq!(
            InventoryRecallDetails::new(InventoryRecallReason::Other, None),
            Err(InventoryRecallError::NoteRequired)
        );
        assert!(InventoryRecallDetails::new(
            InventoryRecallReason::Other,
            Some(InventoryRecallNote::new("Supplier escalation 42").unwrap())
        )
        .is_ok());
    }

    #[test]
    fn release_is_an_active_single_revision_transition() {
        assert_eq!(
            release_inventory_recall(
                InventoryRecallStatus::Active,
                InventoryRecallRevision::new(4).unwrap()
            )
            .map(InventoryRecallRevision::get),
            Ok(5)
        );
        assert!(release_inventory_recall(
            InventoryRecallStatus::Released,
            InventoryRecallRevision::new(5).unwrap()
        )
        .is_err());
    }
}
