use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CancelOutboundQaRequest, CompleteOutboundQaRequest, ConfigureOutboundQaPolicyRequest,
    OutboundQaCancellationReason as ApiCancellationReason, OutboundQaCancellationResponse,
    OutboundQaCartonResponse, OutboundQaPolicyResponse, OutboundQaProgressResponse,
    OutboundQaRequirement as ApiRequirement, OutboundQaSessionResponse,
    OutboundQaSessionStatus as ApiSessionStatus, Revision, StartOutboundQaRequest,
    VerifyOutboundQaCartonRequest,
};
use wareboxes_application::outbound_qa::{
    CancelOutboundQaCommand, CompleteOutboundQaCommand, ConfigureOutboundQaPolicyCommand,
    OutboundQaPolicyReadModel, OutboundQaSessionReadModel, StartOutboundQaCommand,
    VerifyOutboundQaCartonCommand,
};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, OrderRevision, OutboundQaCancellationDetails,
    OutboundQaCancellationNote, OutboundQaCancellationReason, OutboundQaPolicyRevision,
    OutboundQaRequirement, OutboundQaScanValue, OutboundQaSessionId, OutboundQaSessionRevision,
    OutboundQaSessionStatus, PackSessionId,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

pub async fn configure_policy(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureOutboundQaPolicyRequest>,
) -> V1Result<Json<OutboundQaPolicyResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = ConfigureOutboundQaPolicyCommand {
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id).map_err(invalid)?,
        facility_id: FacilityId::new(body.facility_id).map_err(invalid)?,
        requirement: map_requirement(body.requirement),
        expected_revision: body
            .expected_revision
            .map(|revision| OutboundQaPolicyRevision::new(revision.get()).map_err(invalid))
            .transpose()?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::outbound_qa::configure_policy(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(policy_response(result)?))
}

pub async fn start(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(session_id): Path<i64>,
    Json(body): Json<StartOutboundQaRequest>,
) -> V1Result<Json<OutboundQaSessionResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = StartOutboundQaCommand {
        packing_session_id: PackSessionId::new(session_id).map_err(invalid)?,
        expected_order_revision: OrderRevision::new(body.expected_order_revision.get())
            .map_err(invalid)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::outbound_qa::start(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(session_response(result)?))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(session_id): Path<i64>,
) -> V1Result<Json<OutboundQaSessionResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let result = repo::outbound_qa::get_session(
        &state.db,
        &user.tenant,
        OutboundQaSessionId::new(session_id).map_err(invalid)?,
    )
    .await?;
    Ok(Json(session_response(result)?))
}

pub async fn verify_carton(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(session_id): Path<i64>,
    Json(body): Json<VerifyOutboundQaCartonRequest>,
) -> V1Result<Json<OutboundQaSessionResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = VerifyOutboundQaCartonCommand {
        session_id: OutboundQaSessionId::new(session_id).map_err(invalid)?,
        expected_revision: OutboundQaSessionRevision::new(body.expected_revision.get())
            .map_err(invalid)?,
        carton_barcode: OutboundQaScanValue::new(body.carton_barcode).map_err(invalid)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::outbound_qa::verify_carton(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(session_response(result)?))
}

pub async fn complete(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(session_id): Path<i64>,
    Json(body): Json<CompleteOutboundQaRequest>,
) -> V1Result<Json<OutboundQaSessionResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = CompleteOutboundQaCommand {
        session_id: OutboundQaSessionId::new(session_id).map_err(invalid)?,
        expected_revision: OutboundQaSessionRevision::new(body.expected_revision.get())
            .map_err(invalid)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::outbound_qa::complete(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(session_response(result)?))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(session_id): Path<i64>,
    Json(body): Json<CancelOutboundQaRequest>,
) -> V1Result<Json<OutboundQaSessionResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let note = body
        .note
        .map(OutboundQaCancellationNote::new)
        .transpose()
        .map_err(invalid)?;
    let command = CancelOutboundQaCommand {
        session_id: OutboundQaSessionId::new(session_id).map_err(invalid)?,
        expected_revision: OutboundQaSessionRevision::new(body.expected_revision.get())
            .map_err(invalid)?,
        details: OutboundQaCancellationDetails::new(map_cancellation_reason(body.reason), note)
            .map_err(invalid)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::outbound_qa::cancel(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(session_response(result)?))
}

fn policy_response(model: OutboundQaPolicyReadModel) -> V1Result<OutboundQaPolicyResponse> {
    Ok(OutboundQaPolicyResponse {
        policy_id: model.policy_id.get(),
        inventory_owner_id: model.inventory_owner_id.get(),
        facility_id: model.facility_id.get(),
        requirement: api_requirement(model.requirement),
        revision: Revision::new(model.revision.get()).map_err(invalid)?,
        configured_by: model.configured_by.get(),
        configured_at: model.configured_at.to_rfc3339(),
    })
}

pub(crate) fn session_response(
    model: OutboundQaSessionReadModel,
) -> V1Result<OutboundQaSessionResponse> {
    Ok(OutboundQaSessionResponse {
        session_id: model.session_id.get(),
        packing_session_id: model.packing_session_id.get(),
        order_id: model.order_id.get(),
        inventory_owner_id: model.inventory_owner_id.get(),
        facility_id: model.facility_id.get(),
        policy_id: model.policy_id.get(),
        policy_revision: Revision::new(model.policy_revision.get()).map_err(invalid)?,
        status: match model.status {
            OutboundQaSessionStatus::Open => ApiSessionStatus::Open,
            OutboundQaSessionStatus::Passed => ApiSessionStatus::Passed,
            OutboundQaSessionStatus::Cancelled => ApiSessionStatus::Cancelled,
        },
        attempt: model.attempt,
        revision: Revision::new(model.revision.get()).map_err(invalid)?,
        progress: OutboundQaProgressResponse {
            expected_carton_count: model.progress.expected_carton_count(),
            verified_carton_count: model.progress.verified_carton_count(),
        },
        started_by: model.started_by.get(),
        started_at: model.started_at.to_rfc3339(),
        passed_by: model.passed_by.map(|user| user.get()),
        passed_at: model.passed_at.map(|time| time.to_rfc3339()),
        cancellation: model
            .cancellation
            .map(|cancellation| OutboundQaCancellationResponse {
                cancellation_id: cancellation.cancellation_id.get(),
                previous_status: match cancellation.previous_status {
                    OutboundQaSessionStatus::Open => ApiSessionStatus::Open,
                    OutboundQaSessionStatus::Passed => ApiSessionStatus::Passed,
                    OutboundQaSessionStatus::Cancelled => ApiSessionStatus::Cancelled,
                },
                reason: api_cancellation_reason(cancellation.details.reason()),
                note: cancellation
                    .details
                    .note()
                    .map(|note| note.as_str().to_owned()),
                cancelled_by: cancellation.cancelled_by.get(),
                cancelled_at: cancellation.cancelled_at.to_rfc3339(),
            }),
        verifications: model
            .verifications
            .into_iter()
            .map(|verification| OutboundQaCartonResponse {
                verification_id: verification.verification_id.get(),
                carton_id: verification.carton_id.get(),
                license_plate_id: verification.license_plate_id.get(),
                sequence: verification.sequence,
                carton_barcode: verification.carton_barcode.as_str().to_owned(),
                content_count: verification.content_count,
                packed_quantity: verification.packed_quantity,
                verified_by: verification.verified_by.get(),
                verified_at: verification.verified_at.to_rfc3339(),
            })
            .collect(),
    })
}

const fn map_requirement(requirement: ApiRequirement) -> OutboundQaRequirement {
    match requirement {
        ApiRequirement::NotRequired => OutboundQaRequirement::NotRequired,
        ApiRequirement::ScanEveryCarton => OutboundQaRequirement::ScanEveryCarton,
    }
}

const fn api_requirement(requirement: OutboundQaRequirement) -> ApiRequirement {
    match requirement {
        OutboundQaRequirement::NotRequired => ApiRequirement::NotRequired,
        OutboundQaRequirement::ScanEveryCarton => ApiRequirement::ScanEveryCarton,
    }
}

const fn map_cancellation_reason(reason: ApiCancellationReason) -> OutboundQaCancellationReason {
    match reason {
        ApiCancellationReason::PackingCorrection => OutboundQaCancellationReason::PackingCorrection,
        ApiCancellationReason::QualityIssue => OutboundQaCancellationReason::QualityIssue,
        ApiCancellationReason::PolicyError => OutboundQaCancellationReason::PolicyError,
        ApiCancellationReason::OperatorError => OutboundQaCancellationReason::OperatorError,
        ApiCancellationReason::Other => OutboundQaCancellationReason::Other,
    }
}

const fn api_cancellation_reason(reason: OutboundQaCancellationReason) -> ApiCancellationReason {
    match reason {
        OutboundQaCancellationReason::PackingCorrection => ApiCancellationReason::PackingCorrection,
        OutboundQaCancellationReason::QualityIssue => ApiCancellationReason::QualityIssue,
        OutboundQaCancellationReason::PolicyError => ApiCancellationReason::PolicyError,
        OutboundQaCancellationReason::OperatorError => ApiCancellationReason::OperatorError,
        OutboundQaCancellationReason::Other => ApiCancellationReason::Other,
    }
}

fn invalid(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_requirement_without_loss() {
        assert_eq!(
            api_requirement(map_requirement(ApiRequirement::ScanEveryCarton)),
            ApiRequirement::ScanEveryCarton
        );
        assert_eq!(
            api_cancellation_reason(map_cancellation_reason(ApiCancellationReason::QualityIssue)),
            ApiCancellationReason::QualityIssue
        );
    }
}
