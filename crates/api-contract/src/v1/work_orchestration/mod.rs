use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkOrchestrationMode {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationPlanMode {
    Optimized,
    ManualFifo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationWorkKind {
    CycleCountItemLocation,
    CycleCountLocation,
    Putaway,
    LicensePlatePutaway,
    InventoryRelocation,
    Replenishment,
    CrossDock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkResourceKind {
    GeneralLabor,
    InventoryControl,
    MaterialHandling,
    DockDoor,
    PackStation,
    Automation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureWorkOrchestrationPolicyRequest {
    pub facility_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    pub mode: WorkOrchestrationMode,
    pub priority_weight: u32,
    pub due_urgency_weight: u32,
    pub proximity_weight: u32,
    pub interleaving_weight: u32,
    pub congestion_penalty_weight: u32,
    pub bottleneck_penalty_weight: u32,
    pub due_horizon_minutes: u32,
    pub max_candidates: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkOrchestrationPolicyResponse {
    pub policy_id: i64,
    pub facility_id: i64,
    pub inventory_owner_id: Option<i64>,
    pub mode: WorkOrchestrationMode,
    pub priority_weight: u32,
    pub due_urgency_weight: u32,
    pub proximity_weight: u32,
    pub interleaving_weight: u32,
    pub congestion_penalty_weight: u32,
    pub bottleneck_penalty_weight: u32,
    pub due_horizon_minutes: u32,
    pub max_candidates: u16,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
    pub effective_from: String,
    pub supersedes_policy_id: Option<i64>,
    pub effective_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkOrchestrationPolicyPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default = "default_true")]
    pub include_facility_defaults: bool,
    #[serde(default)]
    pub include_history: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

impl Default for WorkOrchestrationPolicyPageRequest {
    fn default() -> Self {
        Self {
            facility_id: None,
            inventory_owner_id: None,
            include_facility_defaults: true,
            include_history: false,
            cursor: None,
            limit: PageLimit::default(),
        }
    }
}

pub type WorkOrchestrationPolicyPage = CursorPage<WorkOrchestrationPolicyResponse>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordZoneCongestionSignalRequest {
    pub facility_id: i64,
    pub storage_zone_id: i64,
    pub congestion_basis_points: u16,
    pub queue_depth: i64,
    pub ttl_seconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordResourceCapacitySignalRequest {
    pub facility_id: i64,
    pub resource_kind: WorkResourceKind,
    pub available_units: i64,
    pub demand_units: i64,
    pub ttl_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneCongestionSignalResponse {
    pub signal_id: i64,
    pub facility_id: i64,
    pub storage_zone_id: i64,
    pub storage_zone_code: String,
    pub congestion_basis_points: u16,
    pub queue_depth: i64,
    pub ttl_seconds: u32,
    pub recorded_by: i64,
    pub observed_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceCapacitySignalResponse {
    pub signal_id: i64,
    pub facility_id: i64,
    pub resource_kind: WorkResourceKind,
    pub available_units: i64,
    pub demand_units: i64,
    pub utilization_basis_points: u16,
    pub ttl_seconds: u32,
    pub recorded_by: i64,
    pub observed_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationSignalWorkspaceRequest {
    pub facility_id: i64,
    #[serde(default)]
    pub include_history: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_cursor: Option<OpaqueCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationSignalWorkspaceResponse {
    pub zone_signals: Vec<ZoneCongestionSignalResponse>,
    pub resource_signals: Vec<ResourceCapacitySignalResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_zone_cursor: Option<OpaqueCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_resource_cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateWorkOrchestrationPlanRequest {
    pub facility_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    pub current_location_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_work_kind: Option<OrchestrationWorkKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_for_user_id: Option<i64>,
    pub expected_policy_id: i64,
    pub expected_policy_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationScoreEvidenceResponse {
    pub work_kind: OrchestrationWorkKind,
    pub task_priority: i64,
    pub due_at: Option<String>,
    pub overdue_seconds: i64,
    pub due_urgency_basis_points: u16,
    pub current_location_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: Option<i64>,
    pub current_travel_sequence: i64,
    pub source_travel_sequence: i64,
    pub destination_travel_sequence: Option<i64>,
    pub travel_distance: i64,
    pub proximity_basis_points: u16,
    pub previous_work_kind: Option<OrchestrationWorkKind>,
    pub interleaving_compatible: bool,
    pub source_zone_id: Option<i64>,
    pub source_zone_code: Option<String>,
    pub congestion_basis_points: u16,
    pub congestion_queue_depth: i64,
    pub resource_kind: WorkResourceKind,
    pub resource_available_units: i64,
    pub resource_demand_units: i64,
    pub resource_utilization_basis_points: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationScoreResponse {
    pub priority_component: i64,
    pub due_urgency_component: i64,
    pub proximity_component: i64,
    pub interleaving_component: i64,
    pub congestion_penalty: i64,
    pub bottleneck_penalty: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkOrchestrationPlanItemResponse {
    pub plan_item_id: i64,
    pub sequence: u16,
    pub work_task_id: i64,
    pub work_kind: OrchestrationWorkKind,
    pub inventory_owner_id: Option<i64>,
    pub title: String,
    pub instructions: Option<String>,
    pub task_status: String,
    pub task_created_at: String,
    pub source_location_label: String,
    pub destination_location_label: Option<String>,
    pub zone_signal_id: Option<i64>,
    pub resource_signal_id: Option<i64>,
    pub evidence: OrchestrationScoreEvidenceResponse,
    pub score: OrchestrationScoreResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkOrchestrationPlanResponse {
    pub plan_id: i64,
    pub facility_id: i64,
    pub requested_inventory_owner_id: Option<i64>,
    pub current_location_id: i64,
    pub current_location_label: String,
    pub previous_work_kind: Option<OrchestrationWorkKind>,
    pub generated_for_user_id: Option<i64>,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub policy_inventory_owner_id: Option<i64>,
    pub plan_mode: OrchestrationPlanMode,
    pub input_snapshot_at: String,
    pub configuration_snapshot: serde_json::Value,
    pub candidate_count: i64,
    pub item_count: i64,
    pub generated_by: i64,
    pub generated_at: String,
    pub items: Vec<WorkOrchestrationPlanItemResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkOrchestrationPlanSummaryResponse {
    pub plan_id: i64,
    pub facility_id: i64,
    pub requested_inventory_owner_id: Option<i64>,
    pub current_location_id: i64,
    pub current_location_label: String,
    pub previous_work_kind: Option<OrchestrationWorkKind>,
    pub generated_for_user_id: Option<i64>,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub policy_inventory_owner_id: Option<i64>,
    pub plan_mode: OrchestrationPlanMode,
    pub input_snapshot_at: String,
    pub candidate_count: i64,
    pub item_count: i64,
    pub generated_by: i64,
    pub generated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WorkOrchestrationPlanPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode: Option<OrchestrationPlanMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type WorkOrchestrationPlanPage = CursorPage<WorkOrchestrationPlanSummaryResponse>;

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_and_plan_contracts_are_strict_and_fallback_is_explicit() {
        assert!(
            serde_json::from_value::<ConfigureWorkOrchestrationPolicyRequest>(serde_json::json!({
                "facility_id": 1,
                "mode": "enabled",
                "priority_weight": 1,
                "due_urgency_weight": 1,
                "proximity_weight": 1,
                "interleaving_weight": 1,
                "congestion_penalty_weight": 1,
                "bottleneck_penalty_weight": 1,
                "due_horizon_minutes": 60,
                "max_candidates": 10,
                "unsafe_auto_assign": true
            }))
            .is_err()
        );
        assert_eq!(
            serde_json::to_string(&OrchestrationPlanMode::ManualFifo).unwrap(),
            "\"manual_fifo\""
        );
        assert!(
            serde_json::from_value::<GenerateWorkOrchestrationPlanRequest>(serde_json::json!({
                "facility_id": 1,
                "current_location_id": 2,
                "expected_policy_revision": 1
            }))
            .is_err()
        );
        let signals: OrchestrationSignalWorkspaceRequest =
            serde_json::from_value(serde_json::json!({ "facility_id": 1 })).unwrap();
        assert!(!signals.include_history);
        assert!(signals.zone_cursor.is_none());
        assert!(signals.resource_cursor.is_none());
    }
}
