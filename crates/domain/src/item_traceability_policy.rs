//! Owner/facility item identity and shelf-life policy invariants.

use std::fmt;

use chrono::Duration;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{CatalogItemId, FacilityId, InventoryOwnerId, TenantId, Timestamp};

pub const MAX_ITEM_TRACEABILITY_POLICY_UOM_LENGTH: usize = 32;
pub const MAX_MINIMUM_SHELF_LIFE_DAYS: u32 = 36_500;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ItemTraceabilityPolicyUom(String);

impl ItemTraceabilityPolicyUom {
    pub fn new(value: impl Into<String>) -> Result<Self, ItemTraceabilityPolicyError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_ITEM_TRACEABILITY_POLICY_UOM_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(ItemTraceabilityPolicyError::InvalidUom);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ItemTraceabilityPolicyUom {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ItemTraceabilityPolicyUom {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceabilityRequirement {
    NotTracked,
    Required,
}

impl TraceabilityRequirement {
    pub const fn requires_value(self) -> bool {
        matches!(self, Self::Required)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemTraceabilityPolicyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ItemTraceabilityPolicyRevision(i64);

impl ItemTraceabilityPolicyRevision {
    pub const fn new(value: i64) -> Result<Self, ItemTraceabilityPolicyError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ItemTraceabilityPolicyError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for ItemTraceabilityPolicyRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct MinimumShelfLifeDays(u32);

impl MinimumShelfLifeDays {
    pub const fn new(value: u32) -> Result<Self, ItemTraceabilityPolicyError> {
        if value <= MAX_MINIMUM_SHELF_LIFE_DAYS {
            Ok(Self(value))
        } else {
            Err(ItemTraceabilityPolicyError::InvalidMinimumShelfLife { value })
        }
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for MinimumShelfLifeDays {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(u32::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemTraceabilityPolicyDefinition {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub item_id: CatalogItemId,
    pub uom: ItemTraceabilityPolicyUom,
    pub lot: TraceabilityRequirement,
    pub serial: TraceabilityRequirement,
    pub expiration: TraceabilityRequirement,
    pub minimum_shelf_life_days: Option<MinimumShelfLifeDays>,
}

impl ItemTraceabilityPolicyDefinition {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tenant_id: TenantId,
        inventory_owner_id: InventoryOwnerId,
        facility_id: FacilityId,
        item_id: CatalogItemId,
        uom: ItemTraceabilityPolicyUom,
        lot: TraceabilityRequirement,
        serial: TraceabilityRequirement,
        expiration: TraceabilityRequirement,
        minimum_shelf_life_days: Option<MinimumShelfLifeDays>,
    ) -> Result<Self, ItemTraceabilityPolicyError> {
        if expiration == TraceabilityRequirement::NotTracked && minimum_shelf_life_days.is_some() {
            return Err(ItemTraceabilityPolicyError::ShelfLifeRequiresExpiration);
        }
        Ok(Self {
            tenant_id,
            inventory_owner_id,
            facility_id,
            item_id,
            uom,
            lot,
            serial,
            expiration,
            minimum_shelf_life_days,
        })
    }

    pub fn validate_batch(
        &self,
        lot: Option<&str>,
        serial: Option<&str>,
        expiration: Option<Timestamp>,
        received_at: Timestamp,
    ) -> Result<(), ItemTraceabilityPolicyError> {
        validate_identity("lot", self.lot, lot)?;
        validate_identity("serial", self.serial, serial)?;
        match (self.expiration, expiration) {
            (TraceabilityRequirement::Required, None) => {
                return Err(ItemTraceabilityPolicyError::RequiredIdentityMissing {
                    identity: "expiration",
                });
            }
            (TraceabilityRequirement::NotTracked, Some(_)) => {
                return Err(ItemTraceabilityPolicyError::UntrackedIdentityPresent {
                    identity: "expiration",
                });
            }
            _ => {}
        }
        if let (Some(days), Some(expiration)) = (self.minimum_shelf_life_days, expiration) {
            let minimum = received_at
                .checked_add_signed(Duration::days(i64::from(days.get())))
                .ok_or(ItemTraceabilityPolicyError::ShelfLifeOverflow)?;
            if expiration < minimum {
                return Err(ItemTraceabilityPolicyError::InsufficientShelfLife);
            }
        }
        Ok(())
    }
}

fn validate_identity(
    identity: &'static str,
    requirement: TraceabilityRequirement,
    value: Option<&str>,
) -> Result<(), ItemTraceabilityPolicyError> {
    match (requirement, value) {
        (TraceabilityRequirement::Required, None) => {
            Err(ItemTraceabilityPolicyError::RequiredIdentityMissing { identity })
        }
        (TraceabilityRequirement::NotTracked, Some(_)) => {
            Err(ItemTraceabilityPolicyError::UntrackedIdentityPresent { identity })
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ItemTraceabilityPolicyError {
    #[error("item traceability policy revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("item traceability policy UOM must be trimmed, nonempty, and at most {MAX_ITEM_TRACEABILITY_POLICY_UOM_LENGTH} characters")]
    InvalidUom,
    #[error("minimum shelf life requires expiration tracking")]
    ShelfLifeRequiresExpiration,
    #[error("minimum shelf life must not exceed {MAX_MINIMUM_SHELF_LIFE_DAYS} days, got {value}")]
    InvalidMinimumShelfLife { value: u32 },
    #[error("required {identity} identity is missing")]
    RequiredIdentityMissing { identity: &'static str },
    #[error("untracked {identity} identity must not be supplied")]
    UntrackedIdentityPresent { identity: &'static str },
    #[error("batch expiration does not satisfy minimum shelf life")]
    InsufficientShelfLife,
    #[error("minimum shelf-life calculation overflowed")]
    ShelfLifeOverflow,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn definition() -> ItemTraceabilityPolicyDefinition {
        ItemTraceabilityPolicyDefinition::new(
            TenantId::new(1).unwrap(),
            InventoryOwnerId::new(2).unwrap(),
            FacilityId::new(3).unwrap(),
            CatalogItemId::new(4).unwrap(),
            ItemTraceabilityPolicyUom::new("case").unwrap(),
            TraceabilityRequirement::Required,
            TraceabilityRequirement::NotTracked,
            TraceabilityRequirement::Required,
            Some(MinimumShelfLifeDays::new(30).unwrap()),
        )
        .unwrap()
    }

    #[test]
    fn required_identity_and_shelf_life_are_exact() {
        let policy = definition();
        let received = Utc.with_ymd_and_hms(2026, 8, 10, 12, 0, 0).unwrap();
        assert!(policy
            .validate_batch(
                Some("LOT-1"),
                None,
                Some(Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap()),
                received,
            )
            .is_ok());
        assert_eq!(
            policy.validate_batch(None, None, Some(received), received),
            Err(ItemTraceabilityPolicyError::RequiredIdentityMissing { identity: "lot" })
        );
        assert_eq!(
            policy.validate_batch(Some("LOT-1"), Some("SER-1"), Some(received), received),
            Err(ItemTraceabilityPolicyError::UntrackedIdentityPresent { identity: "serial" })
        );
    }

    #[test]
    fn shelf_life_without_expiration_is_rejected() {
        assert_eq!(
            ItemTraceabilityPolicyDefinition::new(
                TenantId::new(1).unwrap(),
                InventoryOwnerId::new(2).unwrap(),
                FacilityId::new(3).unwrap(),
                CatalogItemId::new(4).unwrap(),
                ItemTraceabilityPolicyUom::new("each").unwrap(),
                TraceabilityRequirement::NotTracked,
                TraceabilityRequirement::NotTracked,
                TraceabilityRequirement::NotTracked,
                Some(MinimumShelfLifeDays::new(1).unwrap()),
            ),
            Err(ItemTraceabilityPolicyError::ShelfLifeRequiresExpiration)
        );
    }
}
