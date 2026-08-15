use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct CustomerPortalWorkspaceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default)]
    pub include_history: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CustomerPortalOrderStatus {
    Open,
    Held,
    Processing,
    AwaitingPacking,
    Packing,
    AwaitingShipment,
    Shipped,
    Cancelled,
    Void,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CustomerPortalShipmentStatus {
    AwaitingManifest,
    Manifested,
    PartiallyDeparted,
    Departed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CustomerPortalDocumentType {
    PackingSlip,
    CartonLabelSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerPortalInventoryResponse {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub lot: Option<String>,
    pub expiration: Option<String>,
    pub uom: String,
    pub status: String,
    pub on_hand: i64,
    pub reserved: i64,
    pub held: i64,
    pub available: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerPortalOrderResponse {
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: Option<i64>,
    pub facility_name: Option<String>,
    pub status: CustomerPortalOrderStatus,
    pub rush: bool,
    pub ordered_quantity: i64,
    pub ship_by: Option<String>,
    pub created_at: String,
    pub destination_company: Option<String>,
    pub destination_city: Option<String>,
    pub destination_region: Option<String>,
    pub destination_country: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerPortalShipmentResponse {
    pub shipment_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub status: CustomerPortalShipmentStatus,
    pub carton_count: i64,
    pub shipped_quantity: i64,
    pub carrier: Option<String>,
    pub service: Option<String>,
    pub tracking_numbers: Vec<String>,
    pub created_at: String,
    pub manifested_at: Option<String>,
    pub departed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerPortalDocumentResponse {
    pub document_id: i64,
    pub shipment_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub document_type: CustomerPortalDocumentType,
    pub file_name: String,
    pub media_type: String,
    pub content_length: i64,
    pub content_sha256: String,
    pub generated_at: String,
    pub download_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerPortalWorkspaceResponse {
    pub generated_at: String,
    pub inventory: Vec<CustomerPortalInventoryResponse>,
    pub orders: Vec<CustomerPortalOrderResponse>,
    pub shipments: Vec<CustomerPortalShipmentResponse>,
    pub documents: Vec<CustomerPortalDocumentResponse>,
    pub inventory_report_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_contract_does_not_expose_internal_execution_fields() {
        let fields = serde_json::to_value(CustomerPortalInventoryResponse {
            inventory_owner_id: 1,
            inventory_owner_name: "Client".into(),
            facility_id: 2,
            facility_name: "West".into(),
            item_id: 3,
            item_description: Some("Widget".into()),
            primary_sku: Some("W-1".into()),
            lot: None,
            expiration: None,
            uom: "each".into(),
            status: "available".into(),
            on_hand: 10,
            reserved: 2,
            held: 1,
            available: 7,
        })
        .unwrap();
        assert!(fields.get("location_id").is_none());
        assert!(fields.get("license_plate_id").is_none());
        assert!(fields.get("tenant_id").is_none());
    }
}
