use serde::{Deserialize, Serialize};

use super::{OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueAddedWorkKind {
    Relabel,
    Refurbishment,
    Kit,
    Dekit,
    Assembly,
    ValueAddedService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueAddedWorkStatus {
    Draft,
    Released,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueAddedInventoryStatus {
    Available,
    Hold,
    Damaged,
    Quarantine,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateValueAddedWorkInputRequest {
    pub inventory_balance_id: i64,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateValueAddedWorkOutputRequest {
    pub location_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub inventory_status: ValueAddedInventoryStatus,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateValueAddedWorkRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub number: String,
    pub kind: ValueAddedWorkKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub inputs: Vec<CreateValueAddedWorkInputRequest>,
    pub outputs: Vec<CreateValueAddedWorkOutputRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueAddedWorkLifecycleRequest {
    pub expected_revision: Revision,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueAddedWorkPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ValueAddedWorkStatus>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueAddedWorkInputResponse {
    pub input_id: i64,
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
    pub inventory_status: ValueAddedInventoryStatus,
    pub quantity: i64,
    pub hold_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueAddedWorkOutputResponse {
    pub output_id: i64,
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
    pub inventory_status: ValueAddedInventoryStatus,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueAddedWorkEventResponse {
    pub event_id: i64,
    pub from_status: Option<ValueAddedWorkStatus>,
    pub to_status: ValueAddedWorkStatus,
    pub note: Option<String>,
    pub resulting_revision: Revision,
    pub actor_id: i64,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueAddedWorkResponse {
    pub work_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub number: String,
    pub kind: ValueAddedWorkKind,
    pub status: ValueAddedWorkStatus,
    pub revision: Revision,
    pub note: Option<String>,
    pub inputs: Vec<ValueAddedWorkInputResponse>,
    pub outputs: Vec<ValueAddedWorkOutputResponse>,
    pub completion_inventory_transaction_id: Option<i64>,
    pub billable_event_id: Option<i64>,
    pub created_by: i64,
    pub created_at: String,
    pub released_by: Option<i64>,
    pub released_at: Option<String>,
    pub completed_by: Option<i64>,
    pub completed_at: Option<String>,
    pub cancelled_by: Option<i64>,
    pub cancelled_at: Option<String>,
    pub events: Vec<ValueAddedWorkEventResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueAddedWorkPageResponse {
    pub items: Vec<ValueAddedWorkResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<OpaqueCursor>,
}
