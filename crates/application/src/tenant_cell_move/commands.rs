use serde::Serialize;
use wareboxes_domain::{
    DataCellId, DataCellPlacementRevision, TenantCellMoveCheckpoint, TenantCellMoveCopyReference,
    TenantCellMoveCutoverVerification, TenantCellMoveId, TenantCellMoveReason,
    TenantCellMoveRevision, TenantCellMoveRollbackVerification, TenantCellMoveValidation, TenantId,
};

pub const PLAN_TENANT_CELL_MOVE_OPERATION: &str = "platform.tenant_cell_move.plan.v1";
pub const START_TENANT_CELL_MOVE_COPY_OPERATION: &str = "platform.tenant_cell_move.start_copy.v1";
pub const CHECKPOINT_TENANT_CELL_MOVE_OPERATION: &str = "platform.tenant_cell_move.checkpoint.v1";
pub const FREEZE_TENANT_CELL_MOVE_OPERATION: &str = "platform.tenant_cell_move.freeze.v1";
pub const VALIDATE_TENANT_CELL_MOVE_OPERATION: &str = "platform.tenant_cell_move.validate.v1";
pub const CUTOVER_TENANT_CELL_MOVE_OPERATION: &str = "platform.tenant_cell_move.cutover.v1";
pub const VERIFY_TENANT_CELL_MOVE_CUTOVER_OPERATION: &str =
    "platform.tenant_cell_move.verify_cutover.v1";
pub const COMPLETE_TENANT_CELL_MOVE_OPERATION: &str = "platform.tenant_cell_move.complete.v1";
pub const ROLLBACK_TENANT_CELL_MOVE_OPERATION: &str = "platform.tenant_cell_move.rollback.v1";
pub const CANCEL_TENANT_CELL_MOVE_OPERATION: &str = "platform.tenant_cell_move.cancel.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanTenantCellMoveCommand {
    pub tenant_id: TenantId,
    pub target_data_cell_id: DataCellId,
    pub expected_placement_revision: DataCellPlacementRevision,
    pub reason: TenantCellMoveReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartTenantCellMoveCopyCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
    pub copy_reference: TenantCellMoveCopyReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckpointTenantCellMoveCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
    pub checkpoint: TenantCellMoveCheckpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FreezeTenantCellMoveCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidateTenantCellMoveCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
    pub validation: TenantCellMoveValidation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CutoverTenantCellMoveCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
    pub expected_placement_revision: DataCellPlacementRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyTenantCellMoveCutoverCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
    pub verification: TenantCellMoveCutoverVerification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompleteTenantCellMoveCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
    pub reason: TenantCellMoveReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RollbackTenantCellMoveCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
    pub verification: TenantCellMoveRollbackVerification,
    pub reason: TenantCellMoveReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelTenantCellMoveCommand {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub expected_revision: TenantCellMoveRevision,
    pub reason: TenantCellMoveReason,
}

pub type PlanTenantCellMoveResult = super::TenantCellMoveReadModel;
pub type StartTenantCellMoveCopyResult = super::TenantCellMoveReadModel;
pub type CheckpointTenantCellMoveResult = super::TenantCellMoveReadModel;
pub type FreezeTenantCellMoveResult = super::TenantCellMoveReadModel;
pub type ValidateTenantCellMoveResult = super::TenantCellMoveReadModel;
pub type CutoverTenantCellMoveResult = super::TenantCellMoveReadModel;
pub type VerifyTenantCellMoveCutoverResult = super::TenantCellMoveReadModel;
pub type CompleteTenantCellMoveResult = super::TenantCellMoveReadModel;
pub type RollbackTenantCellMoveResult = super::TenantCellMoveReadModel;
pub type CancelTenantCellMoveResult = super::TenantCellMoveReadModel;
