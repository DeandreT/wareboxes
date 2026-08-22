use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    DataCellId, DataCellMode, DataCellPlacementRevision, DataCellRevision, DataCellStatus,
    TenantCellMoveCheckpoint, TenantCellMoveCopyReference, TenantCellMoveCutoverVerification,
    TenantCellMoveId, TenantCellMoveRevision, TenantCellMoveRollbackVerification,
    TenantCellMoveStatus, TenantCellMoveValidation, TenantId, TenantRevision, TenantStatus,
    Timestamp, UserId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveTenantSummary {
    pub tenant_id: TenantId,
    pub slug: String,
    pub name: String,
    pub status: TenantStatus,
    pub revision: TenantRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveDataCellSummary {
    pub data_cell_id: DataCellId,
    pub key: String,
    pub name: String,
    pub region: String,
    pub residency: String,
    pub mode: DataCellMode,
    pub status: DataCellStatus,
    pub revision: DataCellRevision,
    pub max_tenants: u32,
    pub placement_count: i64,
    pub reserved_inbound_move_count: i64,
    pub reserved_rollback_move_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantCellMoveAction {
    StartCopy,
    Checkpoint,
    Freeze,
    Validate,
    Cutover,
    VerifyCutover,
    Complete,
    Rollback,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantCellMoveBlocker {
    ActionNotAvailableInStatus,
    ActorTenantMustBeSwitched,
    SourcePlacementChanged,
    TargetNotActive,
    TargetCapacityUnavailable,
    ResidencyMismatch,
    CopyReferenceMissing,
    CheckpointMissing,
    WriteFenceMissing,
    ValidationMissing,
    ValidationStale,
    PostCutoverVerificationMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveActionEligibility {
    pub action: TenantCellMoveAction,
    pub eligible: bool,
    pub blockers: Vec<TenantCellMoveBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveCheckpointReadModel {
    pub move_revision: TenantCellMoveRevision,
    pub checkpoint: TenantCellMoveCheckpoint,
    pub recorded_at: Timestamp,
    pub recorded_by: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveValidationReadModel {
    pub move_revision: TenantCellMoveRevision,
    pub validation: TenantCellMoveValidation,
    pub validated_at: Timestamp,
    pub validated_by: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveCutoverVerificationReadModel {
    pub move_revision: TenantCellMoveRevision,
    pub verification: TenantCellMoveCutoverVerification,
    pub verified_at: Timestamp,
    pub verified_by: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveRollbackVerificationReadModel {
    pub move_revision: TenantCellMoveRevision,
    pub verification: TenantCellMoveRollbackVerification,
    pub verified_at: Timestamp,
    pub verified_by: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveReadModel {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub tenant: TenantCellMoveTenantSummary,
    pub source_cell: TenantCellMoveDataCellSummary,
    pub target_cell: TenantCellMoveDataCellSummary,
    pub status: TenantCellMoveStatus,
    pub revision: TenantCellMoveRevision,
    pub source_placement_revision: DataCellPlacementRevision,
    pub cutover_placement_revision: Option<DataCellPlacementRevision>,
    pub rollback_placement_revision: Option<DataCellPlacementRevision>,
    pub residency_requirement: String,
    pub reason: String,
    pub copy_reference: Option<TenantCellMoveCopyReference>,
    pub requested_at: Timestamp,
    pub requested_by: UserId,
    pub copy_started_at: Option<Timestamp>,
    pub copy_started_by: Option<UserId>,
    pub frozen_at: Option<Timestamp>,
    pub frozen_by: Option<UserId>,
    pub validated_at: Option<Timestamp>,
    pub validated_by: Option<UserId>,
    pub cutover_at: Option<Timestamp>,
    pub cutover_by: Option<UserId>,
    pub post_cutover_verified_at: Option<Timestamp>,
    pub post_cutover_verified_by: Option<UserId>,
    pub completed_at: Option<Timestamp>,
    pub completed_by: Option<UserId>,
    pub completion_reason: Option<String>,
    pub rolled_back_at: Option<Timestamp>,
    pub rolled_back_by: Option<UserId>,
    pub rollback_reason: Option<String>,
    pub cancelled_at: Option<Timestamp>,
    pub cancelled_by: Option<UserId>,
    pub cancellation_reason: Option<String>,
    pub latest_checkpoint: Option<TenantCellMoveCheckpointReadModel>,
    pub validation: Option<TenantCellMoveValidationReadModel>,
    pub cutover_verification: Option<TenantCellMoveCutoverVerificationReadModel>,
    pub rollback_verification: Option<TenantCellMoveRollbackVerificationReadModel>,
    pub write_frozen: bool,
    pub action_eligibility: Vec<TenantCellMoveActionEligibility>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantCellMoveCursor {
    pub after_requested_at: Timestamp,
    pub after_tenant_cell_move_id: TenantCellMoveId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantCellMovePageQuery {
    pub tenant_id: Option<TenantId>,
    pub data_cell_id: Option<DataCellId>,
    pub status: Option<TenantCellMoveStatus>,
    pub cursor: Option<TenantCellMoveCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantCellMovePage {
    pub items: Vec<TenantCellMoveReadModel>,
    pub next_cursor: Option<TenantCellMoveCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantCellMoveEventAction {
    Planned,
    CopyStarted,
    CheckpointRecorded,
    WritesFrozen,
    Validated,
    CutOver,
    PostCutoverVerified,
    Completed,
    RolledBack,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellMoveEventReadModel {
    pub event_id: i64,
    pub tenant_cell_move_id: TenantCellMoveId,
    pub tenant_id: TenantId,
    pub action: TenantCellMoveEventAction,
    pub move_revision: TenantCellMoveRevision,
    pub previous_status: Option<TenantCellMoveStatus>,
    pub resulting_status: TenantCellMoveStatus,
    pub source_placement_revision: DataCellPlacementRevision,
    pub resulting_placement_revision: Option<DataCellPlacementRevision>,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub reason: Option<String>,
    pub request_id: String,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantCellMoveEventCursor {
    pub after_occurred_at: Timestamp,
    pub after_event_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantCellMoveEventPageQuery {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub cursor: Option<TenantCellMoveEventCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantCellMoveEventPage {
    pub items: Vec<TenantCellMoveEventReadModel>,
    pub next_cursor: Option<TenantCellMoveEventCursor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility_is_typed_and_explains_blocked_actions() {
        let eligibility = TenantCellMoveActionEligibility {
            action: TenantCellMoveAction::Cutover,
            eligible: false,
            blockers: vec![
                TenantCellMoveBlocker::ActorTenantMustBeSwitched,
                TenantCellMoveBlocker::TargetNotActive,
            ],
        };
        let json = serde_json::to_string(&eligibility).unwrap();
        assert!(json.contains("actor_tenant_must_be_switched"));
        assert!(json.contains("target_not_active"));
        assert_eq!(
            serde_json::from_str::<TenantCellMoveActionEligibility>(&json).unwrap(),
            eligibility
        );
    }
}
