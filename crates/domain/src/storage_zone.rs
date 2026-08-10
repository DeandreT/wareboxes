//! Versioned facility storage-zone definitions and invariants.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{FacilityId, LocationId, TenantId};

pub const MAX_STORAGE_ZONE_CODE_LENGTH: usize = 32;
pub const MAX_STORAGE_ZONE_NAME_LENGTH: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct StorageZoneCode(String);

impl StorageZoneCode {
    pub fn new(value: impl Into<String>) -> Result<Self, StorageZoneError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_STORAGE_ZONE_CODE_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(StorageZoneError::InvalidCode);
        }
        let normalized = value.to_ascii_uppercase();
        if !normalized
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        {
            return Err(StorageZoneError::InvalidCode);
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for StorageZoneCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct StorageZoneName(String);

impl StorageZoneName {
    pub fn new(value: impl Into<String>) -> Result<Self, StorageZoneError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_STORAGE_ZONE_NAME_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(StorageZoneError::InvalidName);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for StorageZoneName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageZonePurpose {
    Receiving,
    Reserve,
    Pick,
    Staging,
    Packing,
    Shipping,
    Quarantine,
    Damage,
}

impl StorageZonePurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Receiving => "receiving",
            Self::Reserve => "reserve",
            Self::Pick => "pick",
            Self::Staging => "staging",
            Self::Packing => "packing",
            Self::Shipping => "shipping",
            Self::Quarantine => "quarantine",
            Self::Damage => "damage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "receiving" => Some(Self::Receiving),
            "reserve" => Some(Self::Reserve),
            "pick" => Some(Self::Pick),
            "staging" => Some(Self::Staging),
            "packing" => Some(Self::Packing),
            "shipping" => Some(Self::Shipping),
            "quarantine" => Some(Self::Quarantine),
            "damage" => Some(Self::Damage),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageZoneStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct StorageZoneRevision(i64);

impl StorageZoneRevision {
    pub const fn new(value: i64) -> Result<Self, StorageZoneError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(StorageZoneError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for StorageZoneRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct StorageZoneTravelSequence(u32);

impl StorageZoneTravelSequence {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for StorageZoneTravelSequence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::new(u32::deserialize(deserializer)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct StorageZoneLocationIds(Vec<LocationId>);

impl StorageZoneLocationIds {
    pub fn new(mut location_ids: Vec<LocationId>) -> Result<Self, StorageZoneError> {
        location_ids.sort_unstable_by_key(|location_id| location_id.get());
        location_ids.dedup();
        if location_ids.is_empty() {
            return Err(StorageZoneError::EmptyLocationSet);
        }
        Ok(Self(location_ids))
    }

    pub fn as_slice(&self) -> &[LocationId] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for StorageZoneLocationIds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<LocationId>::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageZoneDefinition {
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub code: StorageZoneCode,
    pub name: StorageZoneName,
    pub purpose: StorageZonePurpose,
    pub travel_sequence: StorageZoneTravelSequence,
    pub location_ids: StorageZoneLocationIds,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageZoneError {
    #[error("storage zone code must be trimmed, nonempty, alphanumeric with '-' or '_', and at most {MAX_STORAGE_ZONE_CODE_LENGTH} characters")]
    InvalidCode,
    #[error("storage zone name must be trimmed, nonempty, and at most {MAX_STORAGE_ZONE_NAME_LENGTH} characters")]
    InvalidName,
    #[error("storage zone revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("storage zone must contain at least one location")]
    EmptyLocationSet,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_normalizes_code_and_location_set() {
        let definition = StorageZoneDefinition {
            tenant_id: TenantId::new(1).unwrap(),
            facility_id: FacilityId::new(2).unwrap(),
            code: StorageZoneCode::new("pick-a").unwrap(),
            name: StorageZoneName::new("Fast pick").unwrap(),
            purpose: StorageZonePurpose::Pick,
            travel_sequence: StorageZoneTravelSequence::new(10),
            location_ids: StorageZoneLocationIds::new(vec![
                LocationId::new(4).unwrap(),
                LocationId::new(3).unwrap(),
                LocationId::new(4).unwrap(),
            ])
            .unwrap(),
        };
        assert_eq!(definition.code.as_str(), "PICK-A");
        assert_eq!(
            definition
                .location_ids
                .as_slice()
                .iter()
                .map(|id| id.get())
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
    }

    #[test]
    fn invalid_text_and_empty_membership_fail_closed() {
        assert_eq!(
            StorageZoneCode::new("pick aisle"),
            Err(StorageZoneError::InvalidCode)
        );
        assert_eq!(
            StorageZoneLocationIds::new(Vec::new()),
            Err(StorageZoneError::EmptyLocationSet)
        );
    }
}
