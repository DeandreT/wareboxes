use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    AbandonPackSessionRequest, AbandonPackSessionResponse,
    PackSessionAbandonmentReason as ApiPackSessionAbandonmentReason,
    PackSessionStatus as ApiSessionStatus, PackingOrderStatus, PackingQueueOrderStatus,
};
use wareboxes_application::packing::{AbandonPackSessionCommand, AbandonPackSessionResult};
use wareboxes_domain::{
    OrderStatus, PackSessionAbandonmentDetails, PackSessionAbandonmentNote,
    PackSessionAbandonmentReason,
};

use super::{
    domain_validation, map_progress_with_status, order_revision, revision, session_id_value,
    PERMISSION,
};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::routes::v1::error::V1Result;
use crate::state::AppState;

pub async fn abandon_session(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(session_id): Path<i64>,
    Json(body): Json<AbandonPackSessionRequest>,
) -> V1Result<Json<AbandonPackSessionResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = abandon_session_command(session_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::packing::abandon_session(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_result(result)?))
}

pub(super) fn abandon_session_command(
    session_id: i64,
    body: AbandonPackSessionRequest,
) -> V1Result<AbandonPackSessionCommand> {
    let reason = match body.reason {
        ApiPackSessionAbandonmentReason::OrderCancellation => {
            PackSessionAbandonmentReason::OrderCancellation
        }
        ApiPackSessionAbandonmentReason::Repack => PackSessionAbandonmentReason::Repack,
        ApiPackSessionAbandonmentReason::StationIssue => PackSessionAbandonmentReason::StationIssue,
        ApiPackSessionAbandonmentReason::Other => PackSessionAbandonmentReason::Other,
    };
    let note = body
        .note
        .map(PackSessionAbandonmentNote::new)
        .transpose()
        .map_err(domain_validation)?;
    Ok(AbandonPackSessionCommand {
        session_id: session_id_value(session_id)?,
        expected_revision: order_revision(body.expected_revision)?,
        details: PackSessionAbandonmentDetails::new(reason, note).map_err(domain_validation)?,
    })
}

fn map_result(result: AbandonPackSessionResult) -> V1Result<AbandonPackSessionResponse> {
    let reason = match result.details.reason() {
        PackSessionAbandonmentReason::OrderCancellation => {
            ApiPackSessionAbandonmentReason::OrderCancellation
        }
        PackSessionAbandonmentReason::Repack => ApiPackSessionAbandonmentReason::Repack,
        PackSessionAbandonmentReason::StationIssue => ApiPackSessionAbandonmentReason::StationIssue,
        PackSessionAbandonmentReason::Other => ApiPackSessionAbandonmentReason::Other,
    };
    Ok(AbandonPackSessionResponse {
        session_id: result.session_id.get(),
        order_id: result.order_id.get(),
        previous_order_status: match result.previous_order_status {
            OrderStatus::Packing => PackingOrderStatus::Packing,
            _ => {
                return Err(
                    AppError::internal("invalid abandoned-session previous order status").into(),
                )
            }
        },
        order_status: match result.order_status {
            OrderStatus::AwaitingPacking => PackingQueueOrderStatus::AwaitingPacking,
            _ => return Err(AppError::internal("invalid abandoned-session order status").into()),
        },
        session_status: ApiSessionStatus::Abandoned,
        revision: revision(result.revision)?,
        progress: map_progress_with_status(result.progress, ApiSessionStatus::Abandoned),
        reason,
        note: result.details.note().map(|value| value.as_str().to_owned()),
        abandoned_by: result.abandoned_by.get(),
        abandoned_at: result.abandoned_at.to_rfc3339(),
    })
}
