use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickCartStatus {
    Active,
    OutOfService,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickClusterStatus {
    Planned,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickExecutionMethod {
    Discrete,
    Case,
    ClusterCart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickExecutionResponse {
    pub method: PickExecutionMethod,
    pub cluster_id: Option<i64>,
    pub cart_barcode: Option<String>,
    pub slot_code: Option<String>,
    pub sequence: Option<i64>,
    pub task_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePickCartRequest {
    pub facility_id: i64,
    pub barcode: String,
    pub name: String,
    pub slot_codes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePickCartStatusRequest {
    pub expected_revision: i64,
    pub status: PickCartStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClusterTaskAssignmentRequest {
    pub task_id: i64,
    pub slot_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPickClusterRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub cart_id: i64,
    pub assignments: Vec<PickClusterTaskAssignmentRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextClusterPickRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelPickClusterRequest {
    pub expected_revision: i64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickCartSlotResponse {
    pub slot_id: i64,
    pub code: String,
    pub sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickCartResponse {
    pub cart_id: i64,
    pub facility_id: i64,
    pub barcode: String,
    pub name: String,
    pub status: PickCartStatus,
    pub revision: i64,
    pub slots: Vec<PickCartSlotResponse>,
    pub created_by: i64,
    pub created_at: String,
    pub status_changed_by: Option<i64>,
    pub status_changed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClusterCandidateResponse {
    pub task_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub source_travel_sequence: i64,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub planned_quantity: i64,
    pub priority: i64,
    pub ship_by: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClusterMemberResponse {
    pub member_id: i64,
    pub sequence: i64,
    pub task_id: i64,
    pub task_status: String,
    pub order_id: i64,
    pub order_key: String,
    pub slot_id: i64,
    pub slot_code: String,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub planned_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClusterResponse {
    pub cluster_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub cart_id: i64,
    pub cart_barcode: String,
    pub cart_name: String,
    pub status: PickClusterStatus,
    pub revision: i64,
    pub task_count: i64,
    pub order_count: i64,
    pub completed_task_count: i64,
    pub assigned_user_id: Option<i64>,
    pub planned_by: i64,
    pub planned_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub cancelled_by: Option<i64>,
    pub cancelled_at: Option<String>,
    pub cancellation_note: Option<String>,
    pub members: Vec<PickClusterMemberResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClusterWorkspaceResponse {
    pub carts: Vec<PickCartResponse>,
    pub candidates: Vec<PickClusterCandidateResponse>,
    pub clusters: Vec<PickClusterResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClusterWorkspaceRequest {
    pub facility_id: i64,
    pub inventory_owner_id: i64,
    #[serde(default)]
    pub include_history: bool,
}
