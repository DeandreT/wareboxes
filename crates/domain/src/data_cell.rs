use serde::{Deserialize, Serialize};

pub const MAX_DATA_CELL_KEY_LENGTH: usize = 63;
pub const MAX_DATA_CELL_NAME_LENGTH: usize = 200;
pub const MAX_DATA_CELL_REGION_LENGTH: usize = 32;
pub const MAX_DATA_RESIDENCY_CODE_LENGTH: usize = 16;
pub const MAX_DATA_CELL_REASON_LENGTH: usize = 500;
pub const MAX_DATA_CELL_TENANTS: u32 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DataCellError {
    #[error("{field} must not be blank or padded")]
    InvalidText { field: &'static str },
    #[error("{field} must not contain control characters")]
    ControlCharacter { field: &'static str },
    #[error("{field} exceeds {max} characters")]
    TooLong { field: &'static str, max: usize },
    #[error("data-cell key must be 3 through 63 lowercase letters, digits, or hyphens and cannot start or end with a hyphen")]
    InvalidKey,
    #[error("data-cell region must be lowercase letters, digits, or hyphens")]
    InvalidRegion,
    #[error("data-residency code must be uppercase letters, digits, or hyphens")]
    InvalidResidency,
    #[error("data-cell tenant capacity must be between 1 and {MAX_DATA_CELL_TENANTS}")]
    InvalidCapacity,
    #[error("a dedicated data cell must have capacity for exactly one tenant")]
    InvalidDedicatedCapacity,
    #[error("data-cell revision must be a positive integer")]
    InvalidRevision,
    #[error("data cell cannot transition from {from} to {to}")]
    InvalidTransition {
        from: DataCellStatus,
        to: DataCellStatus,
    },
}

fn text(value: String, field: &'static str, max: usize) -> Result<String, DataCellError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(DataCellError::InvalidText { field });
    }
    if value.chars().any(char::is_control) {
        return Err(DataCellError::ControlCharacter { field });
    }
    if value.chars().count() > max {
        return Err(DataCellError::TooLong { field, max });
    }
    Ok(value)
}

macro_rules! text_value {
    ($name:ident, $field:literal, $max:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, DataCellError> {
                text(value.into(), $field, $max).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

text_value!(DataCellName, "data-cell name", MAX_DATA_CELL_NAME_LENGTH);
text_value!(
    DataCellReason,
    "data-cell reason",
    MAX_DATA_CELL_REASON_LENGTH
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataCellKey(String);

impl DataCellKey {
    pub fn new(value: impl Into<String>) -> Result<Self, DataCellError> {
        let value = value.into();
        if !(3..=MAX_DATA_CELL_KEY_LENGTH).contains(&value.len())
            || value.starts_with('-')
            || value.ends_with('-')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(DataCellError::InvalidKey);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataCellRegion(String);

impl DataCellRegion {
    pub fn new(value: impl Into<String>) -> Result<Self, DataCellError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_DATA_CELL_REGION_LENGTH
            || value.starts_with('-')
            || value.ends_with('-')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(DataCellError::InvalidRegion);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataResidencyCode(String);

impl DataResidencyCode {
    pub fn new(value: impl Into<String>) -> Result<Self, DataCellError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_DATA_RESIDENCY_CODE_LENGTH
            || value.starts_with('-')
            || value.ends_with('-')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(DataCellError::InvalidResidency);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn allows(&self, actual: &Self) -> bool {
        self.0 == "GLOBAL" || self == actual
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataCellCapacity(u32);

impl DataCellCapacity {
    pub fn new(value: u32) -> Result<Self, DataCellError> {
        if (1..=MAX_DATA_CELL_TENANTS).contains(&value) {
            Ok(Self(value))
        } else {
            Err(DataCellError::InvalidCapacity)
        }
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataCellRevision(i64);

impl DataCellRevision {
    pub fn new(value: i64) -> Result<Self, DataCellError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(DataCellError::InvalidRevision)
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub fn checked_next(self) -> Option<Self> {
        self.0
            .checked_add(1)
            .and_then(|value| Self::new(value).ok())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataCellPlacementRevision(i64);

impl DataCellPlacementRevision {
    pub fn new(value: i64) -> Result<Self, DataCellError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(DataCellError::InvalidRevision)
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub fn checked_next(self) -> Option<Self> {
        self.0
            .checked_add(1)
            .and_then(|value| Self::new(value).ok())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataCellMode {
    Shared,
    Dedicated,
}

impl DataCellMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Dedicated => "dedicated",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "shared" => Some(Self::Shared),
            "dedicated" => Some(Self::Dedicated),
            _ => None,
        }
    }

    pub fn validate_capacity(self, capacity: DataCellCapacity) -> Result<(), DataCellError> {
        if self == Self::Dedicated && capacity.get() != 1 {
            Err(DataCellError::InvalidDedicatedCapacity)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataCellStatus {
    Provisioning,
    Active,
    Draining,
    Retired,
}

impl DataCellStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Retired => "retired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "provisioning" => Some(Self::Provisioning),
            "active" => Some(Self::Active),
            "draining" => Some(Self::Draining),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }

    pub fn require_transition(self, next: Self) -> Result<(), DataCellError> {
        if matches!(
            (self, next),
            (Self::Provisioning, Self::Active)
                | (Self::Active, Self::Draining)
                | (Self::Draining, Self::Active)
                | (Self::Draining, Self::Retired)
        ) {
            Ok(())
        } else {
            Err(DataCellError::InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

impl std::fmt::Display for DataCellStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residency_and_lifecycle_are_explicit() {
        let global = DataResidencyCode::new("GLOBAL").unwrap();
        let us = DataResidencyCode::new("US").unwrap();
        let eu = DataResidencyCode::new("EU").unwrap();
        assert!(global.allows(&us));
        assert!(us.allows(&us));
        assert!(!us.allows(&eu));
        assert!(DataCellStatus::Provisioning
            .require_transition(DataCellStatus::Active)
            .is_ok());
        assert!(DataCellStatus::Active
            .require_transition(DataCellStatus::Retired)
            .is_err());
    }

    #[test]
    fn cell_identifiers_and_capacity_are_bounded() {
        assert!(DataCellKey::new("us-west-2-a").is_ok());
        assert!(DataCellKey::new("US-West").is_err());
        assert!(DataCellRegion::new("us-west-2").is_ok());
        assert!(DataResidencyCode::new("US").is_ok());
        assert!(DataResidencyCode::new("us").is_err());
        assert!(DataCellCapacity::new(0).is_err());
        assert!(DataCellCapacity::new(100).is_ok());
        assert!(DataCellMode::Shared
            .validate_capacity(DataCellCapacity::new(100).unwrap())
            .is_ok());
        assert!(DataCellMode::Dedicated
            .validate_capacity(DataCellCapacity::new(2).unwrap())
            .is_err());
    }
}
