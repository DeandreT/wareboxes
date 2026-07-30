use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit};

/// Public inventory disposition for a balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryBalanceStatus {
    Available,
    Hold,
    Damaged,
    Quarantine,
}

/// Quantities projected for one inventory balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryQuantity {
    pub on_hand: i64,
    pub reserved: i64,
    pub held: i64,
    pub available: i64,
}

/// Version 1 inventory-balance response without persistence metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryBalanceResponse {
    pub id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub uom: String,
    pub status: InventoryBalanceStatus,
    pub quantity: InventoryQuantity,
}

/// Cursor query for the version 1 inventory-balance collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InventoryBalancePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

/// Cursor page returned by the version 1 inventory-balance collection.
pub type InventoryBalancePage = CursorPage<InventoryBalanceResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_balance_response_excludes_persistence_metadata() {
        let response = InventoryBalanceResponse {
            id: 11,
            inventory_owner_id: 22,
            inventory_owner_name: "Acme".into(),
            facility_id: 33,
            facility_name: Some("North DC".into()),
            location_id: 44,
            location_name: Some("Reserve 01".into()),
            location_barcode: Some("R-01".into()),
            license_plate_id: None,
            license_plate_barcode: None,
            item_batch_id: 55,
            item_id: 66,
            item_description: Some("Widget".into()),
            primary_sku: Some("WIDGET-EA".into()),
            lot: Some("LOT-1".into()),
            serial: None,
            uom: "each".into(),
            status: InventoryBalanceStatus::Available,
            quantity: InventoryQuantity {
                on_hand: 12,
                reserved: 2,
                held: 1,
                available: 9,
            },
        };

        let value = serde_json::to_value(response).unwrap();
        assert!(value.get("tenant_id").is_none());
        assert!(value.get("deleted").is_none());
        assert!(value.get("created").is_none());
        assert!(value.get("modified").is_none());
        assert_eq!(value["status"], "available");
        assert_eq!(value["quantity"]["available"], 9);
    }

    #[test]
    fn inventory_balance_page_request_uses_validated_defaults() {
        let request = serde_json::from_str::<InventoryBalancePageRequest>("{}").unwrap();
        assert!(request.cursor.is_none());
        assert_eq!(request.limit, PageLimit::default());
        assert!(serde_json::from_str::<InventoryBalancePageRequest>(r#"{"limit":1001}"#).is_err());
    }
}
