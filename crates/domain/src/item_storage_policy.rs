//! Owner/facility item storage compatibility and location-capacity invariants.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{CatalogItemId, FacilityId, InventoryOwnerId, StorageZonePurpose, TenantId};

pub const MAX_ITEM_STORAGE_POLICY_UOM_LENGTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ItemStoragePolicyUom(String);

impl ItemStoragePolicyUom {
    pub fn new(value: impl Into<String>) -> Result<Self, ItemStoragePolicyError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_ITEM_STORAGE_POLICY_UOM_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(ItemStoragePolicyError::InvalidUom);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ItemStoragePolicyUom {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ItemStoragePolicyUom {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStoragePolicyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ItemStoragePolicyRevision(i64);

impl ItemStoragePolicyRevision {
    pub const fn new(value: i64) -> Result<Self, ItemStoragePolicyError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ItemStoragePolicyError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for ItemStoragePolicyRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ItemStorageLocationCapacity(i64);

impl ItemStorageLocationCapacity {
    pub const fn new(value: i64) -> Result<Self, ItemStoragePolicyError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ItemStoragePolicyError::InvalidCapacity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ItemStorageLocationCapacity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AllowedStorageZonePurposes(Vec<StorageZonePurpose>);

impl AllowedStorageZonePurposes {
    pub fn new(mut purposes: Vec<StorageZonePurpose>) -> Result<Self, ItemStoragePolicyError> {
        purposes.sort_unstable();
        purposes.dedup();
        if purposes.is_empty() {
            return Err(ItemStoragePolicyError::EmptyPurposeSet);
        }
        Ok(Self(purposes))
    }

    pub fn as_slice(&self) -> &[StorageZonePurpose] {
        &self.0
    }

    pub fn contains(&self, purpose: StorageZonePurpose) -> bool {
        self.0.contains(&purpose)
    }
}

impl<'de> Deserialize<'de> for AllowedStorageZonePurposes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<StorageZonePurpose>::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStoragePolicyDefinition {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub item_id: CatalogItemId,
    pub uom: ItemStoragePolicyUom,
    pub allowed_zone_purposes: AllowedStorageZonePurposes,
    pub max_quantity_per_location: Option<ItemStorageLocationCapacity>,
}

impl ItemStoragePolicyDefinition {
    pub fn permits(
        &self,
        purpose: StorageZonePurpose,
        resulting_on_hand: i64,
    ) -> Result<(), ItemStoragePolicyError> {
        if resulting_on_hand < 0 {
            return Err(ItemStoragePolicyError::InvalidResultingQuantity {
                value: resulting_on_hand,
            });
        }
        if !self.allowed_zone_purposes.contains(purpose) {
            return Err(ItemStoragePolicyError::ZonePurposeNotAllowed { purpose });
        }
        if self
            .max_quantity_per_location
            .is_some_and(|capacity| resulting_on_hand > capacity.get())
        {
            return Err(ItemStoragePolicyError::LocationCapacityExceeded {
                resulting: resulting_on_hand,
                capacity: self
                    .max_quantity_per_location
                    .map_or(0, ItemStorageLocationCapacity::get),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ItemStoragePolicyError {
    #[error("item storage policy revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("item storage policy UOM must be trimmed, nonempty, and at most {MAX_ITEM_STORAGE_POLICY_UOM_LENGTH} characters")]
    InvalidUom,
    #[error("item storage location capacity must be positive, got {value}")]
    InvalidCapacity { value: i64 },
    #[error("item storage policy must allow at least one storage-zone purpose")]
    EmptyPurposeSet,
    #[error("resulting on-hand quantity must be nonnegative, got {value}")]
    InvalidResultingQuantity { value: i64 },
    #[error("storage-zone purpose {purpose:?} is not allowed for the item")]
    ZonePurposeNotAllowed { purpose: StorageZonePurpose },
    #[error("resulting on-hand quantity {resulting} exceeds location capacity {capacity}")]
    LocationCapacityExceeded { resulting: i64, capacity: i64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition() -> ItemStoragePolicyDefinition {
        ItemStoragePolicyDefinition {
            tenant_id: TenantId::new(1).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(2).unwrap(),
            facility_id: FacilityId::new(3).unwrap(),
            item_id: CatalogItemId::new(4).unwrap(),
            uom: ItemStoragePolicyUom::new("case").unwrap(),
            allowed_zone_purposes: AllowedStorageZonePurposes::new(vec![
                StorageZonePurpose::Pick,
                StorageZonePurpose::Reserve,
                StorageZonePurpose::Pick,
            ])
            .unwrap(),
            max_quantity_per_location: Some(ItemStorageLocationCapacity::new(20).unwrap()),
        }
    }

    #[test]
    fn purpose_set_is_canonical_and_capacity_is_exact() {
        let definition = definition();
        assert_eq!(
            definition.allowed_zone_purposes.as_slice(),
            &[StorageZonePurpose::Reserve, StorageZonePurpose::Pick]
        );
        assert_eq!(definition.permits(StorageZonePurpose::Pick, 20), Ok(()));
        assert!(matches!(
            definition.permits(StorageZonePurpose::Pick, 21),
            Err(ItemStoragePolicyError::LocationCapacityExceeded { .. })
        ));
        assert!(matches!(
            definition.permits(StorageZonePurpose::Packing, 1),
            Err(ItemStoragePolicyError::ZonePurposeNotAllowed { .. })
        ));
    }

    #[test]
    fn empty_purpose_set_and_nonpositive_capacity_fail_closed() {
        assert_eq!(
            AllowedStorageZonePurposes::new(Vec::new()),
            Err(ItemStoragePolicyError::EmptyPurposeSet)
        );
        assert_eq!(
            ItemStorageLocationCapacity::new(0),
            Err(ItemStoragePolicyError::InvalidCapacity { value: 0 })
        );
    }
}
