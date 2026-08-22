use serde::{Deserialize, Serialize};

use super::{
    CursorPage, DataCellMode, DataCellStatus, OpaqueCursor, PageLimit, Revision, TenantStatus,
};

pub const MAX_TENANT_CELL_MOVE_REASON_LENGTH: usize = 500;
pub const MAX_TENANT_CELL_MOVE_COPY_REFERENCE_LENGTH: usize = 200;
pub const MAX_TENANT_CELL_MOVE_ROUTING_REFERENCE_LENGTH: usize = 200;
pub const MAX_TENANT_CELL_MOVE_TOOL_VERSION_LENGTH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantCellMoveStatus {
    Planned,
    Copying,
    Frozen,
    Validated,
    CutOver,
    Completed,
    Cancelled,
    RolledBack,
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
#[serde(deny_unknown_fields)]
pub struct PlanTenantCellMoveRequest {
    pub target_data_cell_id: i64,
    pub expected_placement_revision: Revision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartTenantCellMoveCopyRequest {
    pub expected_revision: Revision,
    pub copy_reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveCheckpointEvidence {
    pub source_lsn: String,
    pub target_replay_lsn: String,
    pub copied_row_count: i64,
    pub copied_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointTenantCellMoveRequest {
    pub expected_revision: Revision,
    pub checkpoint: TenantCellMoveCheckpointEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreezeTenantCellMoveRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveValidationEvidence {
    pub tool_version: String,
    pub source_lsn: String,
    pub target_replay_lsn: String,
    pub source_row_count: i64,
    pub target_row_count: i64,
    pub source_data_checksum: String,
    pub target_data_checksum: String,
    pub source_schema_checksum: String,
    pub target_schema_checksum: String,
    pub source_object_manifest_checksum: String,
    pub target_object_manifest_checksum: String,
    pub inventory_reconciled: bool,
    pub idempotency_verified: bool,
    pub outbox_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateTenantCellMoveRequest {
    pub expected_revision: Revision,
    pub validation: TenantCellMoveValidationEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutoverTenantCellMoveRequest {
    pub expected_revision: Revision,
    pub expected_placement_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveCutoverVerificationEvidence {
    pub tool_version: String,
    pub routing_reference: String,
    pub observed_data_cell_id: i64,
    pub observed_placement_revision: Revision,
    pub routing_verified: bool,
    pub target_read_verified: bool,
    pub write_fence_verified: bool,
    pub inventory_reconciled: bool,
    pub idempotency_verified: bool,
    pub outbox_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyTenantCellMoveCutoverRequest {
    pub expected_revision: Revision,
    pub verification: TenantCellMoveCutoverVerificationEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteTenantCellMoveRequest {
    pub expected_revision: Revision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveRollbackVerificationEvidence {
    pub tool_version: String,
    pub routing_reference: String,
    pub observed_data_cell_id: i64,
    pub expected_rollback_placement_revision: Revision,
    pub routing_verified: bool,
    pub source_read_verified: bool,
    pub write_fence_verified: bool,
    pub inventory_reconciled: bool,
    pub idempotency_verified: bool,
    pub outbox_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackTenantCellMoveRequest {
    pub expected_revision: Revision,
    pub verification: TenantCellMoveRollbackVerificationEvidence,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelTenantCellMoveRequest {
    pub expected_revision: Revision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMovePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_cell_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TenantCellMoveStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveTenantSummaryResponse {
    pub tenant_id: i64,
    pub slug: String,
    pub name: String,
    pub status: TenantStatus,
    pub revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveDataCellSummaryResponse {
    pub data_cell_id: i64,
    pub key: String,
    pub name: String,
    pub region: String,
    pub residency: String,
    pub mode: DataCellMode,
    pub status: DataCellStatus,
    pub revision: Revision,
    pub max_tenants: u32,
    pub placement_count: i64,
    pub reserved_inbound_move_count: i64,
    pub reserved_rollback_move_count: i64,
    pub available_tenant_slots: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveActionEligibilityResponse {
    pub action: TenantCellMoveAction,
    pub eligible: bool,
    pub blockers: Vec<TenantCellMoveBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveCheckpointResponse {
    pub move_revision: Revision,
    pub checkpoint: TenantCellMoveCheckpointEvidence,
    pub recorded_at: String,
    pub recorded_by: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveValidationResponse {
    pub move_revision: Revision,
    pub validation: TenantCellMoveValidationEvidence,
    pub validated_at: String,
    pub validated_by: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveCutoverVerificationResponse {
    pub move_revision: Revision,
    pub verification: TenantCellMoveCutoverVerificationEvidence,
    pub verified_at: String,
    pub verified_by: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveRollbackVerificationResponse {
    pub move_revision: Revision,
    pub verification: TenantCellMoveRollbackVerificationEvidence,
    pub verified_at: String,
    pub verified_by: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveResponse {
    pub tenant_cell_move_id: i64,
    pub tenant: TenantCellMoveTenantSummaryResponse,
    pub source_cell: TenantCellMoveDataCellSummaryResponse,
    pub target_cell: TenantCellMoveDataCellSummaryResponse,
    pub status: TenantCellMoveStatus,
    pub revision: Revision,
    pub source_placement_revision: Revision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutover_placement_revision: Option<Revision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_placement_revision: Option<Revision>,
    pub residency_requirement: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_reference: Option<String>,
    pub requested_at: String,
    pub requested_by: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_started_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frozen_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frozen_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validated_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutover_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutover_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_cutover_verified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_cutover_verified_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolled_back_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolled_back_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancellation_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<TenantCellMoveCheckpointResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<TenantCellMoveValidationResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutover_verification: Option<TenantCellMoveCutoverVerificationResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_verification: Option<TenantCellMoveRollbackVerificationResponse>,
    pub write_frozen: bool,
    pub action_eligibility: Vec<TenantCellMoveActionEligibilityResponse>,
}

pub type TenantCellMovePage = CursorPage<TenantCellMoveResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveEventPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCellMoveEventResponse {
    pub event_id: i64,
    pub tenant_cell_move_id: i64,
    pub tenant_id: i64,
    pub action: TenantCellMoveEventAction,
    pub move_revision: Revision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<TenantCellMoveStatus>,
    pub resulting_status: TenantCellMoveStatus,
    pub source_placement_revision: Revision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resulting_placement_revision: Option<Revision>,
    pub actor_id: i64,
    pub occurred_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub request_id: String,
    pub evidence: serde_json::Value,
}

pub type TenantCellMoveEventPage = CursorPage<TenantCellMoveEventResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(value: i64) -> Revision {
        Revision::new(value).unwrap()
    }

    #[test]
    fn move_commands_have_exact_public_shapes() {
        let request = PlanTenantCellMoveRequest {
            target_data_cell_id: 8,
            expected_placement_revision: revision(3),
            reason: "Evacuate draining cell under INC-42".into(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<PlanTenantCellMoveRequest>(&json).unwrap(),
            request
        );
        assert!(serde_json::from_str::<PlanTenantCellMoveRequest>(
            r#"{"target_data_cell_id":8,"expected_placement_revision":3,"reason":"move","force":true}"#
        )
        .is_err());

        let checkpoint = CheckpointTenantCellMoveRequest {
            expected_revision: revision(2),
            checkpoint: TenantCellMoveCheckpointEvidence {
                source_lsn: "16/B374D848".into(),
                target_replay_lsn: "16/B374D800".into(),
                copied_row_count: 90,
                copied_bytes: 4096,
            },
        };
        let mut value = serde_json::to_value(&checkpoint).unwrap();
        value["checkpoint"]["secret"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<CheckpointTenantCellMoveRequest>(value).is_err());
    }

    #[test]
    fn lifecycle_wire_values_are_stable() {
        assert_eq!(
            serde_json::to_string(&TenantCellMoveStatus::CutOver).unwrap(),
            r#""cut_over""#
        );
        assert_eq!(
            serde_json::to_string(&TenantCellMoveStatus::RolledBack).unwrap(),
            r#""rolled_back""#
        );
        assert_eq!(
            serde_json::to_string(&TenantCellMoveEventAction::CheckpointRecorded).unwrap(),
            r#""checkpoint_recorded""#
        );
        assert_eq!(
            serde_json::to_string(&TenantCellMoveEventAction::PostCutoverVerified).unwrap(),
            r#""post_cutover_verified""#
        );
    }

    #[test]
    fn cutover_verification_contract_is_exact_and_revision_bound() {
        let request = VerifyTenantCellMoveCutoverRequest {
            expected_revision: revision(7),
            verification: TenantCellMoveCutoverVerificationEvidence {
                tool_version: "cell-validator/1.2.3".into(),
                routing_reference: "route-change/INC-42".into(),
                observed_data_cell_id: 8,
                observed_placement_revision: revision(4),
                routing_verified: true,
                target_read_verified: true,
                write_fence_verified: true,
                inventory_reconciled: true,
                idempotency_verified: true,
                outbox_verified: true,
            },
        };
        let mut value = serde_json::to_value(request).unwrap();
        value["verification"]["database_url"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<VerifyTenantCellMoveCutoverRequest>(value).is_err());
    }

    #[test]
    fn rollback_verification_contract_is_required_exact_and_revision_bound() {
        let request = RollbackTenantCellMoveRequest {
            expected_revision: revision(8),
            verification: TenantCellMoveRollbackVerificationEvidence {
                tool_version: "cell-validator/1.2.3".into(),
                routing_reference: "route-change/INC-43".into(),
                observed_data_cell_id: 7,
                expected_rollback_placement_revision: revision(5),
                routing_verified: true,
                source_read_verified: true,
                write_fence_verified: true,
                inventory_reconciled: true,
                idempotency_verified: true,
                outbox_verified: true,
            },
            reason: "target health regressed".into(),
        };
        let mut value = serde_json::to_value(&request).unwrap();
        value["verification"]["database_url"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<RollbackTenantCellMoveRequest>(value).is_err());

        let mut missing = serde_json::to_value(request).unwrap();
        missing.as_object_mut().unwrap().remove("verification");
        assert!(serde_json::from_value::<RollbackTenantCellMoveRequest>(missing).is_err());
    }
}
