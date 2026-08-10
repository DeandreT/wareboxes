use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationOrderItemMappingStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationOrderOwnerMappingStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureIntegrationOrderOwnerMappingRequest {
    pub source_key: String,
    pub external_inventory_owner_key: String,
    pub inventory_owner_id: i64,
    #[serde(default)]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireIntegrationOrderOwnerMappingRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderOwnerMappingResponse {
    pub mapping_id: i64,
    pub source_key: String,
    pub external_inventory_owner_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub status: IntegrationOrderOwnerMappingStatus,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
    pub retired_by: Option<i64>,
    pub retired_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderOwnerMappingPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<IntegrationOrderOwnerMappingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type IntegrationOrderOwnerMappingPage = CursorPage<IntegrationOrderOwnerMappingResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureIntegrationOrderItemMappingRequest {
    pub inventory_owner_id: i64,
    pub source_key: String,
    pub external_item_key: String,
    pub external_uom: String,
    pub item_id: i64,
    pub requested_uom: String,
    #[serde(default)]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireIntegrationOrderItemMappingRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderItemMappingResponse {
    pub mapping_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub source_key: String,
    pub external_item_key: String,
    pub external_uom: String,
    pub item_id: i64,
    pub item_description: String,
    pub requested_uom: String,
    pub status: IntegrationOrderItemMappingStatus,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
    pub retired_by: Option<i64>,
    pub retired_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderItemMappingPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<IntegrationOrderItemMappingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type IntegrationOrderItemMappingPage = CursorPage<IntegrationOrderItemMappingResponse>;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn mapping_configuration_is_strict_and_revision_bound() {
        let value = json!({
            "inventory_owner_id": 7,
            "source_key": "acme-edi",
            "external_item_key": "ACME-SKU-10",
            "external_uom": "CS",
            "item_id": 11,
            "requested_uom": "case",
            "expected_revision": 2
        });
        assert!(
            serde_json::from_value::<ConfigureIntegrationOrderItemMappingRequest>(value.clone())
                .is_ok()
        );
        let mut unknown = value;
        unknown["force"] = json!(true);
        assert!(
            serde_json::from_value::<ConfigureIntegrationOrderItemMappingRequest>(unknown).is_err()
        );
    }

    #[test]
    fn owner_mapping_configuration_uses_external_identity() {
        let value = json!({
            "source_key": "acme-edi",
            "external_inventory_owner_key": "northstar",
            "inventory_owner_id": 7,
            "expected_revision": 2
        });
        assert!(
            serde_json::from_value::<ConfigureIntegrationOrderOwnerMappingRequest>(value.clone())
                .is_ok()
        );
        let mut unknown = value;
        unknown["tenant_id"] = json!(1);
        assert!(
            serde_json::from_value::<ConfigureIntegrationOrderOwnerMappingRequest>(unknown)
                .is_err()
        );
    }
}
