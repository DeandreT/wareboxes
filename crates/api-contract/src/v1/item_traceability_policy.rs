use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceabilityRequirement {
    NotTracked,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemTraceabilityPolicyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureItemTraceabilityPolicyRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: TraceabilityRequirement,
    pub serial: TraceabilityRequirement,
    pub expiration: TraceabilityRequirement,
    pub minimum_shelf_life_days: Option<u32>,
    pub expected_revision: Option<Revision>,
}

impl<'de> Deserialize<'de> for ConfigureItemTraceabilityPolicyRequest {
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
            lot: TraceabilityRequirement,
            serial: TraceabilityRequirement,
            expiration: TraceabilityRequirement,
            #[serde(default)]
            minimum_shelf_life_days: Option<u32>,
            #[serde(default)]
            expected_revision: Option<Revision>,
        }

        let raw = Raw::deserialize(deserializer)?;
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
        if raw.expiration == TraceabilityRequirement::NotTracked
            && raw.minimum_shelf_life_days.is_some()
        {
            return Err(D::Error::custom(
                "minimum_shelf_life_days requires expiration tracking",
            ));
        }
        if raw
            .minimum_shelf_life_days
            .is_some_and(|days| days > 36_500)
        {
            return Err(D::Error::custom(
                "minimum_shelf_life_days must not exceed 36500",
            ));
        }
        Ok(Self {
            inventory_owner_id: raw.inventory_owner_id,
            facility_id: raw.facility_id,
            item_id: raw.item_id,
            uom: raw.uom,
            lot: raw.lot,
            serial: raw.serial,
            expiration: raw.expiration,
            minimum_shelf_life_days: raw.minimum_shelf_life_days,
            expected_revision: raw.expected_revision,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireItemTraceabilityPolicyRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemTraceabilityPolicyResponse {
    pub item_traceability_policy_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub lot: TraceabilityRequirement,
    pub serial: TraceabilityRequirement,
    pub expiration: TraceabilityRequirement,
    pub minimum_shelf_life_days: Option<u32>,
    pub status: ItemTraceabilityPolicyStatus,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
    pub retired_by: Option<i64>,
    pub retired_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ItemTraceabilityPolicyPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lot: Option<TraceabilityRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<TraceabilityRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiration: Option<TraceabilityRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ItemTraceabilityPolicyStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type ItemTraceabilityPolicyPage = CursorPage<ItemTraceabilityPolicyResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_is_strict_and_shelf_life_requires_expiration() {
        let request: ConfigureItemTraceabilityPolicyRequest =
            serde_json::from_value(serde_json::json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "item_id": 4,
                "uom": "case",
                "lot": "required",
                "serial": "not_tracked",
                "expiration": "required",
                "minimum_shelf_life_days": 30
            }))
            .unwrap();
        assert_eq!(request.minimum_shelf_life_days, Some(30));
        assert!(
            serde_json::from_value::<ConfigureItemTraceabilityPolicyRequest>(serde_json::json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "item_id": 4,
                "uom": "case",
                "lot": "not_tracked",
                "serial": "not_tracked",
                "expiration": "not_tracked",
                "minimum_shelf_life_days": 1
            }))
            .is_err()
        );
    }
}
