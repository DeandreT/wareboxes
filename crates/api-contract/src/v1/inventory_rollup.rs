use serde::{Deserialize, Serialize};

use super::{CursorPage, InventoryQuantity, InventorySortDirection, OpaqueCursor, PageLimit};

/// Sortable dimensions shared by inventory summary views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRollupSort {
    #[default]
    Client,
    Item,
    Scope,
    Balances,
    Batches,
    Locations,
}

/// Cursor query shared by the inventory rollup collections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRollupPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default)]
    pub sort: InventoryRollupSort,
    #[serde(default = "default_rollup_direction")]
    pub direction: InventorySortDirection,
}

impl Default for InventoryRollupPageRequest {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: PageLimit::default(),
            query: None,
            sort: InventoryRollupSort::Client,
            direction: default_rollup_direction(),
        }
    }
}

const fn default_rollup_direction() -> InventorySortDirection {
    InventorySortDirection::Ascending
}

/// Quantities for one unit of measure inside an inventory rollup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRollupQuantity {
    pub uom: String,
    pub quantity: InventoryQuantity,
}

/// Inventory grouped by client, item, and location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryLocationRollupResponse {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub quantities: Vec<InventoryRollupQuantity>,
    pub balance_count: i64,
    pub batch_count: i64,
}

/// Inventory grouped by client, item, and facility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryFacilityRollupResponse {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub quantities: Vec<InventoryRollupQuantity>,
    pub balance_count: i64,
    pub batch_count: i64,
    pub location_count: i64,
}

/// Inventory grouped by client and item across authorized facilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryItemRollupResponse {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub quantities: Vec<InventoryRollupQuantity>,
    pub balance_count: i64,
    pub batch_count: i64,
    pub location_count: i64,
    pub facility_count: i64,
}

pub type InventoryLocationRollupPage = CursorPage<InventoryLocationRollupResponse>;
pub type InventoryFacilityRollupPage = CursorPage<InventoryFacilityRollupResponse>;
pub type InventoryItemRollupPage = CursorPage<InventoryItemRollupResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollups_keep_incompatible_units_separate() {
        let response = InventoryItemRollupResponse {
            inventory_owner_id: 12,
            inventory_owner_name: "Acme".to_owned(),
            item_id: 34,
            item_description: Some("Widget".to_owned()),
            primary_sku: Some("WIDGET".to_owned()),
            quantities: vec![
                InventoryRollupQuantity {
                    uom: "case".to_owned(),
                    quantity: InventoryQuantity {
                        on_hand: 4,
                        reserved: 0,
                        held: 0,
                        available: 4,
                    },
                },
                InventoryRollupQuantity {
                    uom: "each".to_owned(),
                    quantity: InventoryQuantity {
                        on_hand: 24,
                        reserved: 3,
                        held: 2,
                        available: 19,
                    },
                },
            ],
            balance_count: 3,
            batch_count: 2,
            location_count: 2,
            facility_count: 1,
        };

        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["quantities"][0]["uom"], "case");
        assert_eq!(value["quantities"][1]["quantity"]["available"], 19);
        assert!(value.get("tenant_id").is_none());
    }

    #[test]
    fn page_request_uses_validated_defaults() {
        let request = serde_json::from_str::<InventoryRollupPageRequest>("{}").unwrap();
        assert!(request.cursor.is_none());
        assert_eq!(request.limit, PageLimit::default());
        assert_eq!(request.sort, InventoryRollupSort::Client);
        assert_eq!(request.direction, InventorySortDirection::Ascending);
        assert!(serde_json::from_str::<InventoryRollupPageRequest>(r#"{"limit":0}"#).is_err());
        assert!(
            serde_json::from_str::<InventoryRollupPageRequest>(r#"{"sort":"unknown"}"#).is_err()
        );
    }
}
