use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision, StorageZonePurpose};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStoragePolicyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureItemStoragePolicyRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub allowed_zone_purposes: Vec<StorageZonePurpose>,
    pub max_quantity_per_location: Option<i64>,
    pub expected_revision: Option<Revision>,
}

impl<'de> Deserialize<'de> for ConfigureItemStoragePolicyRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            inventory_owner_id: i64,
            facility_id: i64,
            item_id: i64,
            uom: String,
            allowed_zone_purposes: Vec<StorageZonePurpose>,
            #[serde(default)]
            max_quantity_per_location: Option<i64>,
            #[serde(default)]
            expected_revision: Option<Revision>,
        }

        let mut raw = Raw::deserialize(deserializer)?;
        if raw.inventory_owner_id <= 0 || raw.facility_id <= 0 || raw.item_id <= 0 {
            return Err(D::Error::custom(
                "inventory_owner_id, facility_id, and item_id must be positive",
            ));
        }
        if raw.uom.is_empty()
            || raw.uom.trim() != raw.uom
            || raw.uom.chars().count() > 32
            || raw.uom.chars().any(char::is_control)
        {
            return Err(D::Error::custom(
                "uom must be trimmed, nonempty, and at most 32 characters",
            ));
        }
        raw.allowed_zone_purposes.sort_unstable();
        raw.allowed_zone_purposes.dedup();
        if raw.allowed_zone_purposes.is_empty() {
            return Err(D::Error::custom("allowed_zone_purposes must not be empty"));
        }
        if raw
            .max_quantity_per_location
            .is_some_and(|value| value <= 0)
        {
            return Err(D::Error::custom(
                "max_quantity_per_location must be positive",
            ));
        }
        Ok(Self {
            inventory_owner_id: raw.inventory_owner_id,
            facility_id: raw.facility_id,
            item_id: raw.item_id,
            uom: raw.uom,
            allowed_zone_purposes: raw.allowed_zone_purposes,
            max_quantity_per_location: raw.max_quantity_per_location,
            expected_revision: raw.expected_revision,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireItemStoragePolicyRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemStoragePolicyResponse {
    pub item_storage_policy_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub allowed_zone_purposes: Vec<StorageZonePurpose>,
    pub max_quantity_per_location: Option<i64>,
    pub status: ItemStoragePolicyStatus,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
    pub retired_by: Option<i64>,
    pub retired_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ItemStoragePolicyPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<StorageZonePurpose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ItemStoragePolicyStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type ItemStoragePolicyPage = CursorPage<ItemStoragePolicyResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_is_strict_and_canonicalizes_purposes() {
        let request: ConfigureItemStoragePolicyRequest =
            serde_json::from_value(serde_json::json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "item_id": 4,
                "uom": "case",
                "allowed_zone_purposes": ["pick", "reserve", "pick"],
                "max_quantity_per_location": 40
            }))
            .unwrap();
        assert_eq!(
            request.allowed_zone_purposes,
            vec![StorageZonePurpose::Reserve, StorageZonePurpose::Pick]
        );
        assert!(
            serde_json::from_value::<ConfigureItemStoragePolicyRequest>(serde_json::json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "item_id": 4,
                "uom": "case",
                "allowed_zone_purposes": [],
                "max_quantity_per_location": 0
            }))
            .is_err()
        );
    }
}
