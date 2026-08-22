use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use wareboxes_api_contract::v1::{
    CancelTenantCellMoveRequest, CheckpointTenantCellMoveRequest, CompleteTenantCellMoveRequest,
    CutoverTenantCellMoveRequest, DataCellMode as ApiDataCellMode,
    DataCellStatus as ApiDataCellStatus, FreezeTenantCellMoveRequest, OpaqueCursor,
    PlanTenantCellMoveRequest, RollbackTenantCellMoveRequest, StartTenantCellMoveCopyRequest,
    TenantCellMoveAction as ApiAction, TenantCellMoveActionEligibilityResponse,
    TenantCellMoveBlocker as ApiBlocker, TenantCellMoveCheckpointEvidence,
    TenantCellMoveCheckpointResponse, TenantCellMoveCutoverVerificationEvidence,
    TenantCellMoveCutoverVerificationResponse, TenantCellMoveDataCellSummaryResponse,
    TenantCellMoveEventAction as ApiEventAction, TenantCellMoveEventPage as ApiEventPage,
    TenantCellMoveEventPageRequest, TenantCellMoveEventResponse, TenantCellMovePage as ApiPage,
    TenantCellMovePageRequest, TenantCellMoveResponse, TenantCellMoveRollbackVerificationEvidence,
    TenantCellMoveRollbackVerificationResponse, TenantCellMoveStatus as ApiStatus,
    TenantCellMoveTenantSummaryResponse, TenantCellMoveValidationEvidence,
    TenantCellMoveValidationResponse, TenantStatus as ApiTenantStatus,
    ValidateTenantCellMoveRequest, VerifyTenantCellMoveCutoverRequest,
};
use wareboxes_application::tenant_cell_move::{
    CancelTenantCellMoveCommand, CheckpointTenantCellMoveCommand, CompleteTenantCellMoveCommand,
    CutoverTenantCellMoveCommand, FreezeTenantCellMoveCommand, PlanTenantCellMoveCommand,
    RollbackTenantCellMoveCommand, StartTenantCellMoveCopyCommand,
    TenantCellMoveAction as ApplicationAction, TenantCellMoveBlocker as ApplicationBlocker,
    TenantCellMoveCursor, TenantCellMoveCutoverVerificationReadModel,
    TenantCellMoveDataCellSummary, TenantCellMoveEventAction as ApplicationEventAction,
    TenantCellMoveEventCursor, TenantCellMoveEventPageQuery, TenantCellMoveEventReadModel,
    TenantCellMovePageQuery, TenantCellMoveReadModel, TenantCellMoveRollbackVerificationReadModel,
    TenantCellMoveTenantSummary, TenantCellMoveValidationReadModel, ValidateTenantCellMoveCommand,
    VerifyTenantCellMoveCutoverCommand,
};
use wareboxes_domain::{
    DataCellId, DataCellMode, DataCellPlacementRevision, DataCellStatus, Sha256Checksum,
    TenantCellMoveCheckpoint, TenantCellMoveCheckpointInput, TenantCellMoveCopyReference,
    TenantCellMoveCutoverVerification, TenantCellMoveCutoverVerificationInput, TenantCellMoveId,
    TenantCellMoveReason, TenantCellMoveRevision, TenantCellMoveRollbackVerification,
    TenantCellMoveRollbackVerificationInput, TenantCellMoveRoutingReference, TenantCellMoveStatus,
    TenantCellMoveToolVersion, TenantCellMoveValidation, TenantCellMoveValidationInput, TenantId,
    TenantStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::observability::TenantCellMoveCommandMetric;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const CURSOR_PREFIX: &str = "tcmp1.";
const EVENT_CURSOR_PREFIX: &str = "tcme1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<TenantCellMovePageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_platform_administrator(&state.db).await?;
    let tenant_id = request
        .tenant_id
        .map(TenantId::new)
        .transpose()
        .map_err(validation)?;
    let data_cell_id = request
        .data_cell_id
        .map(DataCellId::new)
        .transpose()
        .map_err(validation)?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| {
            decode_cursor(
                cursor,
                request.tenant_id,
                request.data_cell_id,
                request.status,
            )
        })
        .transpose()?;
    let page = repo::tenant_cell_moves::page(
        &state.db,
        &user.tenant,
        &TenantCellMovePageQuery {
            tenant_id,
            data_cell_id,
            status: request.status.map(map_status),
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| {
            encode_cursor(
                cursor,
                request.tenant_id,
                request.data_cell_id,
                request.status,
            )
        })
        .transpose()?;
    Ok(Json(ApiPage::new(
        page.items
            .into_iter()
            .map(map_response)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(tenant_cell_move_id): Path<i64>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let result = repo::tenant_cell_moves::by_id(
        &state.db,
        &user.tenant,
        TenantCellMoveId::new(tenant_cell_move_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn events(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(tenant_cell_move_id): Path<i64>,
    Query(request): Query<TenantCellMoveEventPageRequest>,
) -> V1Result<Json<ApiEventPage>> {
    user.require_platform_administrator(&state.db).await?;
    let tenant_cell_move_id = TenantCellMoveId::new(tenant_cell_move_id).map_err(validation)?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_event_cursor(cursor, tenant_cell_move_id))
        .transpose()?;
    let page = repo::tenant_cell_moves::event_page(
        &state.db,
        &user.tenant,
        &TenantCellMoveEventPageQuery {
            tenant_cell_move_id,
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_event_cursor(cursor, tenant_cell_move_id))
        .transpose()?;
    Ok(Json(ApiEventPage::new(
        page.items
            .into_iter()
            .map(map_event)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn plan(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_id): Path<i64>,
    Json(body): Json<PlanTenantCellMoveRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = PlanTenantCellMoveCommand {
        tenant_id: TenantId::new(tenant_id).map_err(validation)?,
        target_data_cell_id: DataCellId::new(body.target_data_cell_id).map_err(validation)?,
        expected_placement_revision: DataCellPlacementRevision::new(
            body.expected_placement_revision.get(),
        )
        .map_err(validation)?,
        reason: TenantCellMoveReason::new(body.reason).map_err(validation)?,
    };
    let result = repo::tenant_cell_moves::plan(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn start_copy(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<StartTenantCellMoveCopyRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = StartTenantCellMoveCopyCommand {
        tenant_cell_move_id: move_id(tenant_cell_move_id)?,
        expected_revision: move_revision(body.expected_revision.get())?,
        copy_reference: TenantCellMoveCopyReference::new(body.copy_reference)
            .map_err(validation)?,
    };
    let result = repo::tenant_cell_moves::start_copy(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn checkpoint(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<CheckpointTenantCellMoveRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = CheckpointTenantCellMoveCommand {
        tenant_cell_move_id: move_id(tenant_cell_move_id)?,
        expected_revision: move_revision(body.expected_revision.get())?,
        checkpoint: map_checkpoint_request(body.checkpoint)?,
    };
    let result = repo::tenant_cell_moves::checkpoint(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn freeze(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<FreezeTenantCellMoveRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = FreezeTenantCellMoveCommand {
        tenant_cell_move_id: move_id(tenant_cell_move_id)?,
        expected_revision: move_revision(body.expected_revision.get())?,
    };
    let result = repo::tenant_cell_moves::freeze(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn validate(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<ValidateTenantCellMoveRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    record_command_rejection(
        &state,
        TenantCellMoveCommandMetric::Validate,
        async {
            let command = ValidateTenantCellMoveCommand {
                tenant_cell_move_id: move_id(tenant_cell_move_id)?,
                expected_revision: move_revision(body.expected_revision.get())?,
                validation: map_validation_request(body.validation)?,
            };
            let result = repo::tenant_cell_moves::validate(
                &state.db,
                &user.tenant,
                &user.command_context(&idempotency_key),
                &command,
            )
            .await?;
            Ok(Json(map_response(result)?))
        }
        .await,
    )
}

pub async fn cutover(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<CutoverTenantCellMoveRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    record_command_rejection(
        &state,
        TenantCellMoveCommandMetric::Cutover,
        async {
            let command = CutoverTenantCellMoveCommand {
                tenant_cell_move_id: move_id(tenant_cell_move_id)?,
                expected_revision: move_revision(body.expected_revision.get())?,
                expected_placement_revision: DataCellPlacementRevision::new(
                    body.expected_placement_revision.get(),
                )
                .map_err(validation)?,
            };
            let result = repo::tenant_cell_moves::cutover(
                &state.db,
                &user.tenant,
                &user.command_context(&idempotency_key),
                &command,
            )
            .await?;
            Ok(Json(map_response(result)?))
        }
        .await,
    )
}

pub async fn verify_cutover(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<VerifyTenantCellMoveCutoverRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = VerifyTenantCellMoveCutoverCommand {
        tenant_cell_move_id: move_id(tenant_cell_move_id)?,
        expected_revision: move_revision(body.expected_revision.get())?,
        verification: map_cutover_verification_request(body.verification)?,
    };
    let result = repo::tenant_cell_moves::verify_cutover(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn complete(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<CompleteTenantCellMoveRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = CompleteTenantCellMoveCommand {
        tenant_cell_move_id: move_id(tenant_cell_move_id)?,
        expected_revision: move_revision(body.expected_revision.get())?,
        reason: TenantCellMoveReason::new(body.reason).map_err(validation)?,
    };
    let result = repo::tenant_cell_moves::complete(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn rollback(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<RollbackTenantCellMoveRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    record_command_rejection(
        &state,
        TenantCellMoveCommandMetric::Rollback,
        async {
            let command = RollbackTenantCellMoveCommand {
                tenant_cell_move_id: move_id(tenant_cell_move_id)?,
                expected_revision: move_revision(body.expected_revision.get())?,
                verification: map_rollback_verification_request(body.verification)?,
                reason: TenantCellMoveReason::new(body.reason).map_err(validation)?,
            };
            let result = repo::tenant_cell_moves::rollback(
                &state.db,
                &user.tenant,
                &user.command_context(&idempotency_key),
                &command,
            )
            .await?;
            Ok(Json(map_response(result)?))
        }
        .await,
    )
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_cell_move_id): Path<i64>,
    Json(body): Json<CancelTenantCellMoveRequest>,
) -> V1Result<Json<TenantCellMoveResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = CancelTenantCellMoveCommand {
        tenant_cell_move_id: move_id(tenant_cell_move_id)?,
        expected_revision: move_revision(body.expected_revision.get())?,
        reason: TenantCellMoveReason::new(body.reason).map_err(validation)?,
    };
    let result = repo::tenant_cell_moves::cancel(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

fn move_id(value: i64) -> V1Result<TenantCellMoveId> {
    TenantCellMoveId::new(value).map_err(validation)
}

fn record_command_rejection<T>(
    state: &AppState,
    command: TenantCellMoveCommandMetric,
    result: V1Result<T>,
) -> V1Result<T> {
    if result.is_err() {
        state
            .metrics
            .record_tenant_cell_move_command_rejection(command);
    }
    result
}

fn move_revision(value: i64) -> V1Result<TenantCellMoveRevision> {
    TenantCellMoveRevision::new(value).map_err(validation)
}

fn map_checkpoint_request(
    value: TenantCellMoveCheckpointEvidence,
) -> V1Result<TenantCellMoveCheckpoint> {
    TenantCellMoveCheckpoint::new(TenantCellMoveCheckpointInput {
        source_lsn: value.source_lsn.parse().map_err(validation)?,
        target_replay_lsn: value.target_replay_lsn.parse().map_err(validation)?,
        copied_row_count: value.copied_row_count,
        copied_bytes: value.copied_bytes,
    })
    .map_err(validation)
}

fn map_validation_request(
    value: TenantCellMoveValidationEvidence,
) -> V1Result<TenantCellMoveValidation> {
    TenantCellMoveValidation::new(TenantCellMoveValidationInput {
        tool_version: TenantCellMoveToolVersion::new(value.tool_version).map_err(validation)?,
        source_lsn: value.source_lsn.parse().map_err(validation)?,
        target_replay_lsn: value.target_replay_lsn.parse().map_err(validation)?,
        source_row_count: value.source_row_count,
        target_row_count: value.target_row_count,
        source_data_checksum: Sha256Checksum::new(value.source_data_checksum)
            .map_err(validation)?,
        target_data_checksum: Sha256Checksum::new(value.target_data_checksum)
            .map_err(validation)?,
        source_schema_checksum: Sha256Checksum::new(value.source_schema_checksum)
            .map_err(validation)?,
        target_schema_checksum: Sha256Checksum::new(value.target_schema_checksum)
            .map_err(validation)?,
        source_object_manifest_checksum: Sha256Checksum::new(value.source_object_manifest_checksum)
            .map_err(validation)?,
        target_object_manifest_checksum: Sha256Checksum::new(value.target_object_manifest_checksum)
            .map_err(validation)?,
        inventory_reconciled: value.inventory_reconciled,
        idempotency_verified: value.idempotency_verified,
        outbox_verified: value.outbox_verified,
    })
    .map_err(validation)
}

fn map_cutover_verification_request(
    value: TenantCellMoveCutoverVerificationEvidence,
) -> V1Result<TenantCellMoveCutoverVerification> {
    TenantCellMoveCutoverVerification::new(TenantCellMoveCutoverVerificationInput {
        tool_version: TenantCellMoveToolVersion::new(value.tool_version).map_err(validation)?,
        routing_reference: TenantCellMoveRoutingReference::new(value.routing_reference)
            .map_err(validation)?,
        observed_data_cell_id: DataCellId::new(value.observed_data_cell_id).map_err(validation)?,
        observed_placement_revision: DataCellPlacementRevision::new(
            value.observed_placement_revision.get(),
        )
        .map_err(validation)?,
        routing_verified: value.routing_verified,
        target_read_verified: value.target_read_verified,
        write_fence_verified: value.write_fence_verified,
        inventory_reconciled: value.inventory_reconciled,
        idempotency_verified: value.idempotency_verified,
        outbox_verified: value.outbox_verified,
    })
    .map_err(validation)
}

fn map_rollback_verification_request(
    value: TenantCellMoveRollbackVerificationEvidence,
) -> V1Result<TenantCellMoveRollbackVerification> {
    TenantCellMoveRollbackVerification::new(TenantCellMoveRollbackVerificationInput {
        tool_version: TenantCellMoveToolVersion::new(value.tool_version).map_err(validation)?,
        routing_reference: TenantCellMoveRoutingReference::new(value.routing_reference)
            .map_err(validation)?,
        observed_data_cell_id: DataCellId::new(value.observed_data_cell_id).map_err(validation)?,
        expected_rollback_placement_revision: DataCellPlacementRevision::new(
            value.expected_rollback_placement_revision.get(),
        )
        .map_err(validation)?,
        routing_verified: value.routing_verified,
        source_read_verified: value.source_read_verified,
        write_fence_verified: value.write_fence_verified,
        inventory_reconciled: value.inventory_reconciled,
        idempotency_verified: value.idempotency_verified,
        outbox_verified: value.outbox_verified,
    })
    .map_err(validation)
}

pub(crate) fn map_response(value: TenantCellMoveReadModel) -> V1Result<TenantCellMoveResponse> {
    Ok(TenantCellMoveResponse {
        tenant_cell_move_id: value.tenant_cell_move_id.get(),
        tenant: map_tenant_summary(value.tenant)?,
        source_cell: map_cell_summary(value.source_cell)?,
        target_cell: map_cell_summary(value.target_cell)?,
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision.get())?,
        source_placement_revision: api_revision(value.source_placement_revision.get())?,
        cutover_placement_revision: value
            .cutover_placement_revision
            .map(|revision| api_revision(revision.get()))
            .transpose()?,
        rollback_placement_revision: value
            .rollback_placement_revision
            .map(|revision| api_revision(revision.get()))
            .transpose()?,
        residency_requirement: value.residency_requirement,
        reason: value.reason,
        copy_reference: value
            .copy_reference
            .map(|reference| reference.as_str().to_owned()),
        requested_at: value.requested_at.to_rfc3339(),
        requested_by: value.requested_by.get(),
        copy_started_at: value
            .copy_started_at
            .map(|timestamp| timestamp.to_rfc3339()),
        copy_started_by: value.copy_started_by.map(|user_id| user_id.get()),
        frozen_at: value.frozen_at.map(|timestamp| timestamp.to_rfc3339()),
        frozen_by: value.frozen_by.map(|user_id| user_id.get()),
        validated_at: value.validated_at.map(|timestamp| timestamp.to_rfc3339()),
        validated_by: value.validated_by.map(|user_id| user_id.get()),
        cutover_at: value.cutover_at.map(|timestamp| timestamp.to_rfc3339()),
        cutover_by: value.cutover_by.map(|user_id| user_id.get()),
        post_cutover_verified_at: value
            .post_cutover_verified_at
            .map(|timestamp| timestamp.to_rfc3339()),
        post_cutover_verified_by: value.post_cutover_verified_by.map(|user_id| user_id.get()),
        completed_at: value.completed_at.map(|timestamp| timestamp.to_rfc3339()),
        completed_by: value.completed_by.map(|user_id| user_id.get()),
        completion_reason: value.completion_reason,
        rolled_back_at: value.rolled_back_at.map(|timestamp| timestamp.to_rfc3339()),
        rolled_back_by: value.rolled_back_by.map(|user_id| user_id.get()),
        rollback_reason: value.rollback_reason,
        cancelled_at: value.cancelled_at.map(|timestamp| timestamp.to_rfc3339()),
        cancelled_by: value.cancelled_by.map(|user_id| user_id.get()),
        cancellation_reason: value.cancellation_reason,
        latest_checkpoint: value
            .latest_checkpoint
            .map(|checkpoint| {
                Ok::<_, V1Error>(TenantCellMoveCheckpointResponse {
                    move_revision: api_revision(checkpoint.move_revision.get())?,
                    checkpoint: map_checkpoint(checkpoint.checkpoint),
                    recorded_at: checkpoint.recorded_at.to_rfc3339(),
                    recorded_by: checkpoint.recorded_by.get(),
                })
            })
            .transpose()?,
        validation: value.validation.map(map_validation).transpose()?,
        cutover_verification: value
            .cutover_verification
            .map(map_cutover_verification)
            .transpose()?,
        rollback_verification: value
            .rollback_verification
            .map(map_rollback_verification)
            .transpose()?,
        write_frozen: value.write_frozen,
        action_eligibility: value
            .action_eligibility
            .into_iter()
            .map(|eligibility| TenantCellMoveActionEligibilityResponse {
                action: map_action(eligibility.action),
                eligible: eligibility.eligible,
                blockers: eligibility.blockers.into_iter().map(map_blocker).collect(),
            })
            .collect(),
    })
}

fn map_tenant_summary(
    value: TenantCellMoveTenantSummary,
) -> V1Result<TenantCellMoveTenantSummaryResponse> {
    Ok(TenantCellMoveTenantSummaryResponse {
        tenant_id: value.tenant_id.get(),
        slug: value.slug,
        name: value.name,
        status: map_tenant_status(value.status),
        revision: api_revision(value.revision.get())?,
    })
}

fn map_cell_summary(
    value: TenantCellMoveDataCellSummary,
) -> V1Result<TenantCellMoveDataCellSummaryResponse> {
    let placement_count = u32::try_from(value.placement_count)
        .map_err(|_| AppError::internal("tenant cell move placement count is invalid"))?;
    let reserved_count = u32::try_from(value.reserved_inbound_move_count)
        .map_err(|_| AppError::internal("tenant cell move reservation count is invalid"))?;
    let rollback_reserved_count =
        u32::try_from(value.reserved_rollback_move_count).map_err(|_| {
            AppError::internal("tenant cell move rollback reservation count is invalid")
        })?;
    Ok(TenantCellMoveDataCellSummaryResponse {
        data_cell_id: value.data_cell_id.get(),
        key: value.key,
        name: value.name,
        region: value.region,
        residency: value.residency,
        mode: map_cell_mode(value.mode),
        status: map_cell_status(value.status),
        revision: api_revision(value.revision.get())?,
        max_tenants: value.max_tenants,
        placement_count: value.placement_count,
        reserved_inbound_move_count: value.reserved_inbound_move_count,
        reserved_rollback_move_count: value.reserved_rollback_move_count,
        available_tenant_slots: value.max_tenants.saturating_sub(
            placement_count
                .saturating_add(reserved_count)
                .saturating_add(rollback_reserved_count),
        ),
    })
}

fn map_validation(
    value: TenantCellMoveValidationReadModel,
) -> V1Result<TenantCellMoveValidationResponse> {
    Ok(TenantCellMoveValidationResponse {
        move_revision: api_revision(value.move_revision.get())?,
        validation: TenantCellMoveValidationEvidence {
            tool_version: value.validation.tool_version().as_str().to_owned(),
            source_lsn: value.validation.source_lsn().to_string(),
            target_replay_lsn: value.validation.target_replay_lsn().to_string(),
            source_row_count: value.validation.source_row_count(),
            target_row_count: value.validation.target_row_count(),
            source_data_checksum: value.validation.source_data_checksum().as_str().to_owned(),
            target_data_checksum: value.validation.target_data_checksum().as_str().to_owned(),
            source_schema_checksum: value
                .validation
                .source_schema_checksum()
                .as_str()
                .to_owned(),
            target_schema_checksum: value
                .validation
                .target_schema_checksum()
                .as_str()
                .to_owned(),
            source_object_manifest_checksum: value
                .validation
                .source_object_manifest_checksum()
                .as_str()
                .to_owned(),
            target_object_manifest_checksum: value
                .validation
                .target_object_manifest_checksum()
                .as_str()
                .to_owned(),
            inventory_reconciled: value.validation.inventory_reconciled(),
            idempotency_verified: value.validation.idempotency_verified(),
            outbox_verified: value.validation.outbox_verified(),
        },
        validated_at: value.validated_at.to_rfc3339(),
        validated_by: value.validated_by.get(),
    })
}

fn map_cutover_verification(
    value: TenantCellMoveCutoverVerificationReadModel,
) -> V1Result<TenantCellMoveCutoverVerificationResponse> {
    Ok(TenantCellMoveCutoverVerificationResponse {
        move_revision: api_revision(value.move_revision.get())?,
        verification: TenantCellMoveCutoverVerificationEvidence {
            tool_version: value.verification.tool_version().as_str().to_owned(),
            routing_reference: value.verification.routing_reference().as_str().to_owned(),
            observed_data_cell_id: value.verification.observed_data_cell_id().get(),
            observed_placement_revision: api_revision(
                value.verification.observed_placement_revision().get(),
            )?,
            routing_verified: value.verification.routing_verified(),
            target_read_verified: value.verification.target_read_verified(),
            write_fence_verified: value.verification.write_fence_verified(),
            inventory_reconciled: value.verification.inventory_reconciled(),
            idempotency_verified: value.verification.idempotency_verified(),
            outbox_verified: value.verification.outbox_verified(),
        },
        verified_at: value.verified_at.to_rfc3339(),
        verified_by: value.verified_by.get(),
    })
}

fn map_rollback_verification(
    value: TenantCellMoveRollbackVerificationReadModel,
) -> V1Result<TenantCellMoveRollbackVerificationResponse> {
    Ok(TenantCellMoveRollbackVerificationResponse {
        move_revision: api_revision(value.move_revision.get())?,
        verification: TenantCellMoveRollbackVerificationEvidence {
            tool_version: value.verification.tool_version().as_str().to_owned(),
            routing_reference: value.verification.routing_reference().as_str().to_owned(),
            observed_data_cell_id: value.verification.observed_data_cell_id().get(),
            expected_rollback_placement_revision: api_revision(
                value
                    .verification
                    .expected_rollback_placement_revision()
                    .get(),
            )?,
            routing_verified: value.verification.routing_verified(),
            source_read_verified: value.verification.source_read_verified(),
            write_fence_verified: value.verification.write_fence_verified(),
            inventory_reconciled: value.verification.inventory_reconciled(),
            idempotency_verified: value.verification.idempotency_verified(),
            outbox_verified: value.verification.outbox_verified(),
        },
        verified_at: value.verified_at.to_rfc3339(),
        verified_by: value.verified_by.get(),
    })
}

fn map_checkpoint(value: TenantCellMoveCheckpoint) -> TenantCellMoveCheckpointEvidence {
    TenantCellMoveCheckpointEvidence {
        source_lsn: value.source_lsn().to_string(),
        target_replay_lsn: value.target_replay_lsn().to_string(),
        copied_row_count: value.copied_row_count(),
        copied_bytes: value.copied_bytes(),
    }
}

fn map_event(value: TenantCellMoveEventReadModel) -> V1Result<TenantCellMoveEventResponse> {
    Ok(TenantCellMoveEventResponse {
        event_id: value.event_id,
        tenant_cell_move_id: value.tenant_cell_move_id.get(),
        tenant_id: value.tenant_id.get(),
        action: map_event_action(value.action),
        move_revision: api_revision(value.move_revision.get())?,
        previous_status: value.previous_status.map(map_status_to_api),
        resulting_status: map_status_to_api(value.resulting_status),
        source_placement_revision: api_revision(value.source_placement_revision.get())?,
        resulting_placement_revision: value
            .resulting_placement_revision
            .map(|revision| api_revision(revision.get()))
            .transpose()?,
        actor_id: value.actor_id.get(),
        occurred_at: value.occurred_at.to_rfc3339(),
        reason: value.reason,
        request_id: value.request_id,
        evidence: value.evidence,
    })
}

fn api_revision(value: i64) -> V1Result<wareboxes_api_contract::v1::Revision> {
    wareboxes_api_contract::v1::Revision::new(value).map_err(invalid_result)
}

const fn map_status(value: ApiStatus) -> TenantCellMoveStatus {
    match value {
        ApiStatus::Planned => TenantCellMoveStatus::Planned,
        ApiStatus::Copying => TenantCellMoveStatus::Copying,
        ApiStatus::Frozen => TenantCellMoveStatus::Frozen,
        ApiStatus::Validated => TenantCellMoveStatus::Validated,
        ApiStatus::CutOver => TenantCellMoveStatus::CutOver,
        ApiStatus::Completed => TenantCellMoveStatus::Completed,
        ApiStatus::Cancelled => TenantCellMoveStatus::Cancelled,
        ApiStatus::RolledBack => TenantCellMoveStatus::RolledBack,
    }
}

const fn map_status_to_api(value: TenantCellMoveStatus) -> ApiStatus {
    match value {
        TenantCellMoveStatus::Planned => ApiStatus::Planned,
        TenantCellMoveStatus::Copying => ApiStatus::Copying,
        TenantCellMoveStatus::Frozen => ApiStatus::Frozen,
        TenantCellMoveStatus::Validated => ApiStatus::Validated,
        TenantCellMoveStatus::CutOver => ApiStatus::CutOver,
        TenantCellMoveStatus::Completed => ApiStatus::Completed,
        TenantCellMoveStatus::Cancelled => ApiStatus::Cancelled,
        TenantCellMoveStatus::RolledBack => ApiStatus::RolledBack,
    }
}

const fn map_action(value: ApplicationAction) -> ApiAction {
    match value {
        ApplicationAction::StartCopy => ApiAction::StartCopy,
        ApplicationAction::Checkpoint => ApiAction::Checkpoint,
        ApplicationAction::Freeze => ApiAction::Freeze,
        ApplicationAction::Validate => ApiAction::Validate,
        ApplicationAction::Cutover => ApiAction::Cutover,
        ApplicationAction::VerifyCutover => ApiAction::VerifyCutover,
        ApplicationAction::Complete => ApiAction::Complete,
        ApplicationAction::Rollback => ApiAction::Rollback,
        ApplicationAction::Cancel => ApiAction::Cancel,
    }
}

const fn map_blocker(value: ApplicationBlocker) -> ApiBlocker {
    match value {
        ApplicationBlocker::ActionNotAvailableInStatus => ApiBlocker::ActionNotAvailableInStatus,
        ApplicationBlocker::ActorTenantMustBeSwitched => ApiBlocker::ActorTenantMustBeSwitched,
        ApplicationBlocker::SourcePlacementChanged => ApiBlocker::SourcePlacementChanged,
        ApplicationBlocker::TargetNotActive => ApiBlocker::TargetNotActive,
        ApplicationBlocker::TargetCapacityUnavailable => ApiBlocker::TargetCapacityUnavailable,
        ApplicationBlocker::ResidencyMismatch => ApiBlocker::ResidencyMismatch,
        ApplicationBlocker::CopyReferenceMissing => ApiBlocker::CopyReferenceMissing,
        ApplicationBlocker::CheckpointMissing => ApiBlocker::CheckpointMissing,
        ApplicationBlocker::WriteFenceMissing => ApiBlocker::WriteFenceMissing,
        ApplicationBlocker::ValidationMissing => ApiBlocker::ValidationMissing,
        ApplicationBlocker::ValidationStale => ApiBlocker::ValidationStale,
        ApplicationBlocker::PostCutoverVerificationMissing => {
            ApiBlocker::PostCutoverVerificationMissing
        }
    }
}

const fn map_event_action(value: ApplicationEventAction) -> ApiEventAction {
    match value {
        ApplicationEventAction::Planned => ApiEventAction::Planned,
        ApplicationEventAction::CopyStarted => ApiEventAction::CopyStarted,
        ApplicationEventAction::CheckpointRecorded => ApiEventAction::CheckpointRecorded,
        ApplicationEventAction::WritesFrozen => ApiEventAction::WritesFrozen,
        ApplicationEventAction::Validated => ApiEventAction::Validated,
        ApplicationEventAction::CutOver => ApiEventAction::CutOver,
        ApplicationEventAction::PostCutoverVerified => ApiEventAction::PostCutoverVerified,
        ApplicationEventAction::Completed => ApiEventAction::Completed,
        ApplicationEventAction::RolledBack => ApiEventAction::RolledBack,
        ApplicationEventAction::Cancelled => ApiEventAction::Cancelled,
    }
}

const fn map_tenant_status(value: TenantStatus) -> ApiTenantStatus {
    match value {
        TenantStatus::Active => ApiTenantStatus::Active,
        TenantStatus::Suspended => ApiTenantStatus::Suspended,
    }
}

const fn map_cell_mode(value: DataCellMode) -> ApiDataCellMode {
    match value {
        DataCellMode::Shared => ApiDataCellMode::Shared,
        DataCellMode::Dedicated => ApiDataCellMode::Dedicated,
    }
}

const fn map_cell_status(value: DataCellStatus) -> ApiDataCellStatus {
    match value {
        DataCellStatus::Provisioning => ApiDataCellStatus::Provisioning,
        DataCellStatus::Active => ApiDataCellStatus::Active,
        DataCellStatus::Draining => ApiDataCellStatus::Draining,
        DataCellStatus::Retired => ApiDataCellStatus::Retired,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPayload {
    requested_at: String,
    tenant_cell_move_id: i64,
    tenant_id: Option<i64>,
    data_cell_id: Option<i64>,
    status: Option<ApiStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EventCursorPayload {
    occurred_at: String,
    event_id: i64,
    tenant_cell_move_id: i64,
}

fn encode_cursor(
    cursor: TenantCellMoveCursor,
    tenant_id: Option<i64>,
    data_cell_id: Option<i64>,
    status: Option<ApiStatus>,
) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&CursorPayload {
        requested_at: cursor.after_requested_at.to_rfc3339(),
        tenant_cell_move_id: cursor.after_tenant_cell_move_id.get(),
        tenant_id,
        data_cell_id,
        status,
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| AppError::internal(error.to_string()).into())
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    tenant_id: Option<i64>,
    data_cell_id: Option<i64>,
    status: Option<ApiStatus>,
) -> V1Result<TenantCellMoveCursor> {
    let payload: CursorPayload = decode_payload(cursor, CURSOR_PREFIX, "tenant cell move")?;
    if payload.tenant_id != tenant_id
        || payload.data_cell_id != data_cell_id
        || payload.status != status
    {
        return Err(V1Error::invalid_cursor_for("tenant cell move"));
    }
    Ok(TenantCellMoveCursor {
        after_requested_at: chrono::DateTime::parse_from_rfc3339(&payload.requested_at)
            .map_err(|_| V1Error::invalid_cursor_for("tenant cell move"))?
            .with_timezone(&chrono::Utc),
        after_tenant_cell_move_id: TenantCellMoveId::new(payload.tenant_cell_move_id)
            .map_err(|_| V1Error::invalid_cursor_for("tenant cell move"))?,
    })
}

fn encode_event_cursor(
    cursor: TenantCellMoveEventCursor,
    tenant_cell_move_id: TenantCellMoveId,
) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&EventCursorPayload {
        occurred_at: cursor.after_occurred_at.to_rfc3339(),
        event_id: cursor.after_event_id,
        tenant_cell_move_id: tenant_cell_move_id.get(),
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{EVENT_CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| AppError::internal(error.to_string()).into())
}

fn decode_event_cursor(
    cursor: &OpaqueCursor,
    tenant_cell_move_id: TenantCellMoveId,
) -> V1Result<TenantCellMoveEventCursor> {
    let payload: EventCursorPayload =
        decode_payload(cursor, EVENT_CURSOR_PREFIX, "tenant cell move event")?;
    if payload.event_id <= 0 || payload.tenant_cell_move_id != tenant_cell_move_id.get() {
        return Err(V1Error::invalid_cursor_for("tenant cell move event"));
    }
    Ok(TenantCellMoveEventCursor {
        after_occurred_at: chrono::DateTime::parse_from_rfc3339(&payload.occurred_at)
            .map_err(|_| V1Error::invalid_cursor_for("tenant cell move event"))?
            .with_timezone(&chrono::Utc),
        after_event_id: payload.event_id,
    })
}

fn decode_payload<T: for<'de> Deserialize<'de>>(
    cursor: &OpaqueCursor,
    prefix: &str,
    resource: &str,
) -> V1Result<T> {
    let encoded = cursor
        .as_str()
        .strip_prefix(prefix)
        .ok_or_else(|| V1Error::invalid_cursor_for(resource))?;
    serde_json::from_slice(
        &hex::decode(encoded).map_err(|_| V1Error::invalid_cursor_for(resource))?,
    )
    .map_err(|_| V1Error::invalid_cursor_for(resource))
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    AppError::internal(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(value: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn move_cursor_round_trips_and_is_bound_to_filters() {
        let cursor = TenantCellMoveCursor {
            after_requested_at: timestamp("2026-08-21T12:00:00Z"),
            after_tenant_cell_move_id: TenantCellMoveId::new(7).unwrap(),
        };
        let filters = (Some(11), Some(13), Some(ApiStatus::Copying));
        let encoded = encode_cursor(cursor, filters.0, filters.1, filters.2).unwrap();

        assert_eq!(
            decode_cursor(&encoded, filters.0, filters.1, filters.2).unwrap(),
            cursor
        );
        assert!(decode_cursor(&encoded, Some(12), filters.1, filters.2).is_err());
        assert!(decode_cursor(&encoded, filters.0, Some(14), filters.2).is_err());
        assert!(decode_cursor(&encoded, filters.0, filters.1, Some(ApiStatus::Frozen)).is_err());
    }

    #[test]
    fn event_cursor_is_bound_to_its_move() {
        let move_id = TenantCellMoveId::new(7).unwrap();
        let cursor = TenantCellMoveEventCursor {
            after_occurred_at: timestamp("2026-08-21T12:00:00Z"),
            after_event_id: 42,
        };
        let encoded = encode_event_cursor(cursor, move_id).unwrap();

        assert_eq!(decode_event_cursor(&encoded, move_id).unwrap(), cursor);
        assert!(decode_event_cursor(&encoded, TenantCellMoveId::new(8).unwrap()).is_err());
    }
}
