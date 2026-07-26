use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    HeartbeatPutawayClaimRequest, PutawayClaimHeartbeatResponse, PutawayClaimReleaseReason,
    PutawayClaimReleaseResponse, ReleasePutawayClaimRequest,
};
use wareboxes_core::models::PutawayClaimReleaseReason as CoreReleaseReason;

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const MAX_RELEASE_NOTE_LENGTH: usize = 500;

pub async fn heartbeat(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<HeartbeatPutawayClaimRequest>,
) -> V1Result<Json<PutawayClaimHeartbeatResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let context = user.command_context(&idempotency_key);
    let heartbeat =
        repo::tasks::heartbeat_putaway_claim_in_scope(&state.db, &user.tenant, &context, task_id)
            .await?;

    Ok(Json(PutawayClaimHeartbeatResponse {
        task_id: heartbeat.task_id,
        heartbeat_at: heartbeat.heartbeat_at.to_rfc3339(),
        lease_expires_at: heartbeat.lease_expires_at.to_rfc3339(),
    }))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ReleasePutawayClaimRequest>,
) -> V1Result<Json<PutawayClaimReleaseResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    validate_release(&body)?;
    let context = user.command_context(&idempotency_key);
    let reason = map_release_reason(body.reason);
    let release = repo::tasks::release_putaway_claim_in_scope(
        &state.db,
        &user.tenant,
        &context,
        task_id,
        reason,
        body.note.as_deref(),
    )
    .await?;

    Ok(Json(PutawayClaimReleaseResponse {
        task_id: release.task_id,
        released_at: release.released_at.to_rfc3339(),
        release_count: release.release_count,
        reason: body.reason,
        note: release.note,
    }))
}

fn validate_release(body: &ReleasePutawayClaimRequest) -> V1Result<()> {
    if let Some(note) = body.note.as_deref() {
        if note.trim() != note || note.is_empty() {
            return Err(invalid("note must be trimmed and nonempty when provided"));
        }
        if note.chars().count() > MAX_RELEASE_NOTE_LENGTH {
            return Err(invalid(format!(
                "note cannot exceed {MAX_RELEASE_NOTE_LENGTH} characters"
            )));
        }
    }
    if body.reason == PutawayClaimReleaseReason::Other && body.note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    Ok(())
}

fn map_release_reason(reason: PutawayClaimReleaseReason) -> CoreReleaseReason {
    match reason {
        PutawayClaimReleaseReason::WorkInterrupted => CoreReleaseReason::WorkInterrupted,
        PutawayClaimReleaseReason::EquipmentUnavailable => CoreReleaseReason::EquipmentUnavailable,
        PutawayClaimReleaseReason::DestinationBlocked => CoreReleaseReason::DestinationBlocked,
        PutawayClaimReleaseReason::SafetyIssue => CoreReleaseReason::SafetyIssue,
        PutawayClaimReleaseReason::Other => CoreReleaseReason::Other,
    }
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
