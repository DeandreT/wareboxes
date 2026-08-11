use serde::{Deserialize, Serialize};

use super::{
    CursorPage, InventoryBalanceSearchQuery, InventoryBalanceStatus, InventorySortDirection,
    OpaqueCursor, PageLimit,
};

/// Typed reason for restricting a quantity of inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryHoldReason {
    QualityInspection,
    DamageSuspected,
    InventoryDiscrepancy,
    Regulatory,
    CustomerRequest,
    Other,
}

/// Lifecycle state of an inventory quantity hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryHoldStatus {
    Active,
    Released,
}

/// Cursor query for the version 1 inventory-hold collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryHoldPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<InventoryHoldStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<InventoryBalanceSearchQuery>,
    #[serde(default)]
    pub sort: InventoryHoldSort,
    #[serde(default = "default_hold_direction")]
    pub direction: InventorySortDirection,
}

impl Default for InventoryHoldPageRequest {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: PageLimit::default(),
            status: None,
            query: None,
            sort: InventoryHoldSort::Created,
            direction: default_hold_direction(),
        }
    }
}

const fn default_hold_direction() -> InventorySortDirection {
    InventorySortDirection::Descending
}

/// Stable server-side ordering for the inventory-hold collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventoryHoldSort {
    Id,
    Item,
    Client,
    Position,
    Reason,
    #[default]
    Created,
    Quantity,
}

/// One quantity hold with the display context required by an operations workbench.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryHoldResponse {
    pub id: i64,
    pub created_at: String,
    pub created_by_user_id: i64,
    pub released_at: Option<String>,
    pub released_by_user_id: Option<i64>,
    pub inventory_balance_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_barcode: Option<String>,
    pub location_name: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub inventory_status: InventoryBalanceStatus,
    pub quantity: i64,
    pub reason: InventoryHoldReason,
    pub note: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub status: InventoryHoldStatus,
}

/// Cursor page returned by the version 1 inventory-hold collection.
pub type InventoryHoldPage = CursorPage<InventoryHoldResponse>;

/// Places a quantity hold against one inventory balance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceInventoryHoldRequest {
    pub inventory_balance_id: i64,
    pub quantity: i64,
    pub reason: InventoryHoldReason,
    pub note: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
}

/// Result of placing an inventory quantity hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceInventoryHoldResponse {
    pub hold_id: i64,
}

/// Explicit release command. The persisted hold already records its releasing actor and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInventoryHoldRequest {}

/// Result of releasing an inventory quantity hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInventoryHoldResponse {
    pub hold_id: i64,
    pub released_quantity: i64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn response() -> InventoryHoldResponse {
        InventoryHoldResponse {
            id: 11,
            created_at: "2026-07-29T18:00:00+00:00".into(),
            created_by_user_id: 12,
            released_at: None,
            released_by_user_id: None,
            inventory_balance_id: 13,
            inventory_owner_id: 14,
            inventory_owner_name: "Acme Retail".into(),
            facility_id: 15,
            facility_name: Some("Reno DC".into()),
            location_id: 16,
            location_barcode: Some("A-01-02".into()),
            location_name: Some("Aisle A / Bay 1 / Level 2".into()),
            license_plate_id: Some(17),
            license_plate_barcode: Some("LPN-00017".into()),
            item_batch_id: 18,
            lot: Some("LOT-7".into()),
            serial: None,
            expiration: Some("2027-07-29T00:00:00+00:00".into()),
            item_id: 19,
            item_description: Some("Widget".into()),
            uom: "case".into(),
            inventory_status: InventoryBalanceStatus::Available,
            quantity: 4,
            reason: InventoryHoldReason::QualityInspection,
            note: Some("Awaiting QA".into()),
            reference_type: Some("receipt".into()),
            reference_id: Some(20),
            status: InventoryHoldStatus::Active,
        }
    }

    #[test]
    fn hold_response_has_an_exact_operations_contract() {
        let value = serde_json::to_value(response()).unwrap();

        assert_eq!(
            value,
            json!({
                "id": 11,
                "created_at": "2026-07-29T18:00:00+00:00",
                "created_by_user_id": 12,
                "released_at": null,
                "released_by_user_id": null,
                "inventory_balance_id": 13,
                "inventory_owner_id": 14,
                "inventory_owner_name": "Acme Retail",
                "facility_id": 15,
                "facility_name": "Reno DC",
                "location_id": 16,
                "location_barcode": "A-01-02",
                "location_name": "Aisle A / Bay 1 / Level 2",
                "license_plate_id": 17,
                "license_plate_barcode": "LPN-00017",
                "item_batch_id": 18,
                "lot": "LOT-7",
                "serial": null,
                "expiration": "2027-07-29T00:00:00+00:00",
                "item_id": 19,
                "item_description": "Widget",
                "uom": "case",
                "inventory_status": "available",
                "quantity": 4,
                "reason": "quality_inspection",
                "note": "Awaiting QA",
                "reference_type": "receipt",
                "reference_id": 20,
                "status": "active"
            })
        );
        for persistence_field in ["tenant_id", "modified", "deleted"] {
            assert!(value.get(persistence_field).is_none());
        }
    }

    #[test]
    fn hold_requests_are_bounded_and_reject_transport_ambiguity() {
        let page = serde_json::from_value::<InventoryHoldPageRequest>(json!({
            "limit": 25,
            "status": "active"
        }))
        .unwrap();
        assert_eq!(page.limit.get(), 25);
        assert_eq!(page.status, Some(InventoryHoldStatus::Active));
        assert_eq!(page.sort, InventoryHoldSort::Created);
        assert_eq!(page.direction, InventorySortDirection::Descending);
        let sorted = serde_json::from_value::<InventoryHoldPageRequest>(json!({
            "limit": 25,
            "status": "released",
            "query": "damaged",
            "sort": "quantity",
            "direction": "ascending"
        }))
        .unwrap();
        assert_eq!(sorted.sort, InventoryHoldSort::Quantity);
        assert_eq!(sorted.direction, InventorySortDirection::Ascending);
        assert!(serde_json::from_value::<InventoryHoldPageRequest>(json!({
            "limit": 25,
            "offset": 25
        }))
        .is_err());
        assert!(serde_json::from_value::<PlaceInventoryHoldRequest>(json!({
            "inventory_balance_id": 13,
            "quantity": 4,
            "reason": "quality_inspection",
            "note": null,
            "reference_type": null,
            "reference_id": null,
            "idempotency_key": "must-be-a-header"
        }))
        .is_err());
        assert!(
            serde_json::from_value::<ReleaseInventoryHoldRequest>(json!({"force": true})).is_err()
        );
        assert_eq!(
            serde_json::to_value(ReleaseInventoryHoldRequest::default()).unwrap(),
            json!({})
        );
    }
}
