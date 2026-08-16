//! Explainable, replay-safe planning over canonical warehouse work.

mod options;

pub use options::{
    WorkOrchestrationWorkerCursor, WorkOrchestrationWorkerOptionReadModel,
    WorkOrchestrationWorkerPage, WorkOrchestrationWorkerPageQuery,
};

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, LocationId, OrchestrationPlanMode, OrchestrationScore,
    OrchestrationScoreEvidence, OrchestrationWorkKind, ResourceCapacitySignal, StorageZoneId,
    TenantId, Timestamp, UserId, WorkOrchestrationDispatchCancellationReason,
    WorkOrchestrationDispatchId, WorkOrchestrationDispatchRevision,
    WorkOrchestrationDispatchStatus, WorkOrchestrationPlanId, WorkOrchestrationPlanItemId,
    WorkOrchestrationPolicyDefinition, WorkOrchestrationPolicyId, WorkOrchestrationPolicyRevision,
    WorkOrchestrationSignalId, WorkResourceKind, ZoneCongestionSignal,
};

pub const CONFIGURE_WORK_ORCHESTRATION_POLICY_OPERATION: &str =
    "optimization.work_orchestration.policy.configure.v1";
pub const RECORD_ZONE_CONGESTION_OPERATION: &str =
    "optimization.work_orchestration.zone_signal.record.v1";
pub const RECORD_RESOURCE_CAPACITY_OPERATION: &str =
    "optimization.work_orchestration.resource_signal.record.v1";
pub const GENERATE_WORK_ORCHESTRATION_PLAN_OPERATION: &str =
    "optimization.work_orchestration.plan.generate.v1";
pub const ACTIVATE_WORK_ORCHESTRATION_DISPATCH_OPERATION: &str =
    "optimization.work_orchestration.dispatch.activate.v1";
pub const CANCEL_WORK_ORCHESTRATION_DISPATCH_OPERATION: &str =
    "optimization.work_orchestration.dispatch.cancel.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureWorkOrchestrationPolicyCommand {
    pub definition: WorkOrchestrationPolicyDefinition,
    pub expected_revision: Option<WorkOrchestrationPolicyRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RecordZoneCongestionCommand {
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub storage_zone_id: StorageZoneId,
    pub signal: ZoneCongestionSignal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RecordResourceCapacityCommand {
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub resource_kind: WorkResourceKind,
    pub signal: ResourceCapacitySignal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GenerateWorkOrchestrationPlanCommand {
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub current_location_id: LocationId,
    pub previous_work_kind: Option<OrchestrationWorkKind>,
    pub generated_for_user_id: Option<UserId>,
    pub expected_policy_id: WorkOrchestrationPolicyId,
    pub expected_policy_revision: WorkOrchestrationPolicyRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ActivateWorkOrchestrationDispatchCommand {
    pub tenant_id: TenantId,
    pub plan_id: WorkOrchestrationPlanId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelWorkOrchestrationDispatchCommand {
    pub tenant_id: TenantId,
    pub dispatch_id: WorkOrchestrationDispatchId,
    pub expected_revision: WorkOrchestrationDispatchRevision,
    pub reason: WorkOrchestrationDispatchCancellationReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkOrchestrationPolicyReadModel {
    pub policy_id: WorkOrchestrationPolicyId,
    pub definition: WorkOrchestrationPolicyDefinition,
    pub revision: WorkOrchestrationPolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub effective_from: Timestamp,
    pub supersedes_policy_id: Option<WorkOrchestrationPolicyId>,
    pub effective_to: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneCongestionSignalReadModel {
    pub signal_id: WorkOrchestrationSignalId,
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub storage_zone_id: StorageZoneId,
    pub storage_zone_code: String,
    pub signal: ZoneCongestionSignal,
    pub recorded_by: UserId,
    pub observed_at: Timestamp,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCapacitySignalReadModel {
    pub signal_id: WorkOrchestrationSignalId,
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub resource_kind: WorkResourceKind,
    pub signal: ResourceCapacitySignal,
    pub utilization_basis_points: u16,
    pub recorded_by: UserId,
    pub observed_at: Timestamp,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkOrchestrationPlanItemReadModel {
    pub plan_item_id: WorkOrchestrationPlanItemId,
    pub sequence: u16,
    pub work_task_id: i64,
    pub work_kind: OrchestrationWorkKind,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub title: String,
    pub instructions: Option<String>,
    pub task_status: String,
    pub task_created_at: Timestamp,
    pub source_location_label: String,
    pub destination_location_label: Option<String>,
    pub zone_signal_id: Option<WorkOrchestrationSignalId>,
    pub resource_signal_id: Option<WorkOrchestrationSignalId>,
    pub evidence: OrchestrationScoreEvidence,
    pub score: OrchestrationScore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkOrchestrationDispatchReadModel {
    pub dispatch_id: WorkOrchestrationDispatchId,
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub plan_id: WorkOrchestrationPlanId,
    pub worker_user_id: UserId,
    pub status: WorkOrchestrationDispatchStatus,
    pub revision: WorkOrchestrationDispatchRevision,
    pub current_sequence: Option<u16>,
    pub current_work_task_id: Option<i64>,
    pub current_work_kind: Option<OrchestrationWorkKind>,
    pub completed_item_count: i64,
    pub cancelled_item_count: i64,
    pub remaining_item_count: i64,
    pub activated_by: UserId,
    pub activated_at: Timestamp,
    pub ended_by: Option<UserId>,
    pub ended_at: Option<Timestamp>,
    pub cancellation_reason: Option<WorkOrchestrationDispatchCancellationReason>,
    pub cancellation_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkOrchestrationPlanReadModel {
    pub plan_id: WorkOrchestrationPlanId,
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub requested_inventory_owner_id: Option<InventoryOwnerId>,
    pub current_location_id: LocationId,
    pub current_location_label: String,
    pub previous_work_kind: Option<OrchestrationWorkKind>,
    pub generated_for_user_id: Option<UserId>,
    pub policy_id: WorkOrchestrationPolicyId,
    pub policy_revision: WorkOrchestrationPolicyRevision,
    pub policy_inventory_owner_id: Option<InventoryOwnerId>,
    pub plan_mode: OrchestrationPlanMode,
    pub input_snapshot_at: Timestamp,
    pub configuration_snapshot: serde_json::Value,
    pub candidate_count: i64,
    pub item_count: i64,
    pub generated_by: UserId,
    pub generated_at: Timestamp,
    pub dispatch: Option<WorkOrchestrationDispatchReadModel>,
    pub items: Vec<WorkOrchestrationPlanItemReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkOrchestrationPlanSummaryReadModel {
    pub plan_id: WorkOrchestrationPlanId,
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub requested_inventory_owner_id: Option<InventoryOwnerId>,
    pub current_location_id: LocationId,
    pub current_location_label: String,
    pub previous_work_kind: Option<OrchestrationWorkKind>,
    pub generated_for_user_id: Option<UserId>,
    pub policy_id: WorkOrchestrationPolicyId,
    pub policy_revision: WorkOrchestrationPolicyRevision,
    pub policy_inventory_owner_id: Option<InventoryOwnerId>,
    pub plan_mode: OrchestrationPlanMode,
    pub input_snapshot_at: Timestamp,
    pub candidate_count: i64,
    pub item_count: i64,
    pub generated_by: UserId,
    pub generated_at: Timestamp,
    pub dispatch: Option<WorkOrchestrationDispatchReadModel>,
}

pub type ConfigureWorkOrchestrationPolicyResult = WorkOrchestrationPolicyReadModel;
pub type RecordZoneCongestionResult = ZoneCongestionSignalReadModel;
pub type RecordResourceCapacityResult = ResourceCapacitySignalReadModel;
pub type GenerateWorkOrchestrationPlanResult = WorkOrchestrationPlanReadModel;
pub type ActivateWorkOrchestrationDispatchResult = WorkOrchestrationDispatchReadModel;
pub type CancelWorkOrchestrationDispatchResult = WorkOrchestrationDispatchReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrchestrationPolicyCursor {
    pub after_configured_at: Timestamp,
    pub after_policy_id: WorkOrchestrationPolicyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrchestrationPolicyPageQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub include_facility_defaults: bool,
    pub include_history: bool,
    pub cursor: Option<WorkOrchestrationPolicyCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOrchestrationPolicyPage {
    pub items: Vec<WorkOrchestrationPolicyReadModel>,
    pub next_cursor: Option<WorkOrchestrationPolicyCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrchestrationSignalCursor {
    pub after_observed_at: Timestamp,
    pub after_signal_id: WorkOrchestrationSignalId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrchestrationSignalQuery {
    pub facility_id: FacilityId,
    pub include_history: bool,
    pub zone_cursor: Option<WorkOrchestrationSignalCursor>,
    pub resource_cursor: Option<WorkOrchestrationSignalCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOrchestrationSignalWorkspace {
    pub zone_signals: Vec<ZoneCongestionSignalReadModel>,
    pub resource_signals: Vec<ResourceCapacitySignalReadModel>,
    pub next_zone_cursor: Option<WorkOrchestrationSignalCursor>,
    pub next_resource_cursor: Option<WorkOrchestrationSignalCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrchestrationPlanCursor {
    pub after_generated_at: Timestamp,
    pub after_plan_id: WorkOrchestrationPlanId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrchestrationPlanPageQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub plan_mode: Option<OrchestrationPlanMode>,
    pub cursor: Option<WorkOrchestrationPlanCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOrchestrationPlanPage {
    pub items: Vec<WorkOrchestrationPlanSummaryReadModel>,
    pub next_cursor: Option<WorkOrchestrationPlanCursor>,
}
