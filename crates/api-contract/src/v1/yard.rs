use serde::{Deserialize, Serialize};

use super::{OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardAssetKind {
    Trailer,
    Container,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardLocationKind {
    Gate,
    Parking,
    DockDoor,
    Inspection,
    Staging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardAppointmentStatus {
    Scheduled,
    CheckedIn,
    Completed,
    Cancelled,
    NoShow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardVisitStatus {
    GatedIn,
    InYard,
    AtDoor,
    Loading,
    Unloading,
    ReadyToDepart,
    Rejected,
    GatedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardOperation {
    Loading,
    Unloading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardVisitEventKind {
    GatedIn,
    Spotted,
    DoorAssigned,
    OperationStarted,
    OperationCompleted,
    Rejected,
    GatedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureYardLocationRequest {
    pub facility_id: i64,
    pub code: String,
    pub name: String,
    pub kind: YardLocationKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterYardAssetRequest {
    pub kind: YardAssetKind,
    pub asset_number: String,
    pub carrier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateYardAppointmentRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub direction: YardDirection,
    pub appointment_number: String,
    pub scheduled_from: String,
    pub scheduled_until: String,
    pub carrier: String,
    pub expected_asset_kind: YardAssetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_asset_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound_load_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_load_id: Option<i64>,
    pub free_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardLifecycleRequest {
    pub expected_revision: Revision,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateInYardVisitRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appointment_id: Option<i64>,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub direction: YardDirection,
    pub asset_id: i64,
    pub driver_name: String,
    pub gate_location_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveYardVisitRequest {
    pub expected_revision: Revision,
    pub destination_location_id: i64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignYardVisitDoorRequest {
    pub expected_revision: Revision,
    pub door_location_id: i64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardDockOperationRequest {
    pub expected_revision: Revision,
    pub operation: YardOperation,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardLocationResponse {
    pub location_id: i64,
    pub facility_id: i64,
    pub facility_name: String,
    pub code: String,
    pub name: String,
    pub kind: YardLocationKind,
    pub active: bool,
    pub revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardAssetResponse {
    pub asset_id: i64,
    pub kind: YardAssetKind,
    pub asset_number: String,
    pub carrier: String,
    pub active: bool,
    pub revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardAppointmentResponse {
    pub appointment_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub direction: YardDirection,
    pub appointment_number: String,
    pub scheduled_from: String,
    pub scheduled_until: String,
    pub carrier: String,
    pub expected_asset_kind: YardAssetKind,
    pub expected_asset_number: Option<String>,
    pub inbound_load_id: Option<i64>,
    pub outbound_load_id: Option<i64>,
    pub free_minutes: u32,
    pub status: YardAppointmentStatus,
    pub revision: Revision,
    pub note: Option<String>,
    pub visit_id: Option<i64>,
    pub created_by: i64,
    pub created_at: String,
    pub updated_by: Option<i64>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardVisitEventResponse {
    pub event_id: i64,
    pub kind: YardVisitEventKind,
    pub from_status: Option<YardVisitStatus>,
    pub to_status: YardVisitStatus,
    pub from_location_id: Option<i64>,
    pub to_location_id: Option<i64>,
    pub operation: Option<YardOperation>,
    pub note: Option<String>,
    pub resulting_revision: Revision,
    pub actor_id: i64,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardDetentionResponse {
    pub detention_id: i64,
    pub total_minutes: u64,
    pub free_minutes: u32,
    pub detention_minutes: u64,
    pub billable_hours: u64,
    pub billable_event_id: Option<i64>,
    pub calculated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardVisitResponse {
    pub visit_id: i64,
    pub appointment_id: Option<i64>,
    pub appointment_number: Option<String>,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub direction: YardDirection,
    pub asset_id: i64,
    pub asset_kind: YardAssetKind,
    pub asset_number: String,
    pub carrier: String,
    pub driver_name: String,
    pub status: YardVisitStatus,
    pub revision: Revision,
    pub current_location_id: Option<i64>,
    pub current_location_code: Option<String>,
    pub dock_door_location_id: Option<i64>,
    pub dock_door_code: Option<String>,
    pub inbound_load_id: Option<i64>,
    pub outbound_load_id: Option<i64>,
    pub gated_in_at: String,
    pub operation_started_at: Option<String>,
    pub operation_completed_at: Option<String>,
    pub gated_out_at: Option<String>,
    pub rejected_at: Option<String>,
    pub detention: Option<YardDetentionResponse>,
    pub events: Vec<YardVisitEventResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct YardWorkspaceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default)]
    pub include_completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YardWorkspaceResponse {
    pub locations: Vec<YardLocationResponse>,
    pub assets: Vec<YardAssetResponse>,
    pub appointments: Vec<YardAppointmentResponse>,
    pub visits: Vec<YardVisitResponse>,
    pub next_cursor: Option<OpaqueCursor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_appointment_rejects_unknown_fields() {
        let value = serde_json::json!({
            "inventory_owner_id": 1,
            "facility_id": 2,
            "direction": "inbound",
            "appointment_number": "APT-1",
            "scheduled_from": "2026-08-01T08:00:00Z",
            "scheduled_until": "2026-08-01T09:00:00Z",
            "carrier": "Carrier",
            "expected_asset_kind": "trailer",
            "free_minutes": 120,
            "private_extension": true
        });
        assert!(serde_json::from_value::<CreateYardAppointmentRequest>(value).is_err());
    }
}
