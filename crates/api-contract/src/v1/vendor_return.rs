use serde::{Deserialize, Serialize};

use super::{OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VendorReturnReason {
    Damaged,
    Defective,
    Expired,
    Recall,
    Overstock,
    VendorRequest,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VendorReturnStatus {
    Draft,
    Released,
    Shipped,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateVendorReturnLineRequest {
    pub inventory_balance_id: i64,
    pub quantity: i64,
    pub reason: VendorReturnReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateVendorReturnRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub number: String,
    pub vendor_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub lines: Vec<CreateVendorReturnLineRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorReturnLifecycleRequest {
    pub expected_revision: Revision,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorReturnPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<VendorReturnStatus>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorReturnLineResponse {
    pub line_id: i64,
    pub inventory_balance_id: i64,
    pub location_id: i64,
    pub location_code: String,
    pub license_plate_id: Option<i64>,
    pub license_plate_number: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub inventory_status: String,
    pub quantity: i64,
    pub reason: VendorReturnReason,
    pub note: Option<String>,
    pub hold_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorReturnEventResponse {
    pub event_id: i64,
    pub from_status: Option<VendorReturnStatus>,
    pub to_status: VendorReturnStatus,
    pub note: Option<String>,
    pub resulting_revision: Revision,
    pub actor_id: i64,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorReturnResponse {
    pub vendor_return_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub number: String,
    pub vendor_name: String,
    pub vendor_reference: Option<String>,
    pub status: VendorReturnStatus,
    pub revision: Revision,
    pub note: Option<String>,
    pub lines: Vec<VendorReturnLineResponse>,
    pub shipment_inventory_transaction_id: Option<i64>,
    pub billable_event_id: Option<i64>,
    pub created_by: i64,
    pub created_at: String,
    pub released_by: Option<i64>,
    pub released_at: Option<String>,
    pub shipped_by: Option<i64>,
    pub shipped_at: Option<String>,
    pub cancelled_by: Option<i64>,
    pub cancelled_at: Option<String>,
    pub events: Vec<VendorReturnEventResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorReturnPageResponse {
    pub items: Vec<VendorReturnResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<OpaqueCursor>,
}
