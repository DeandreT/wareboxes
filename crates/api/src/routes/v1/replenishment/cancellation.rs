use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CancelReplenishmentWorkRequest, ReplenishmentWorkCancellationReason as ApiReason,
    ReplenishmentWorkCancellationResponse, Revision,
};
use wareboxes_application::replenishment::{
    CancelReplenishmentWorkCommand, CancelReplenishmentWorkResult,
};
use wareboxes_domain::{
    ReplenishmentWorkCancellationNote, ReplenishmentWorkCancellationReason, ReplenishmentWorkStatus,
};

use super::{invalid, map_work_status, work_id_value, V1Error, V1Result, MAX_RELEASE_NOTE_LENGTH};
use crate::auth::CurrentTenant;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const SUPERVISOR_PERMISSION: &str = "wms_supervisor";

pub(crate) async fn cancel_work(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(body): Json<CancelReplenishmentWorkRequest>,
) -> V1Result<Json<ReplenishmentWorkCancellationResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    validate_request(&body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::replenishment::cancel_work(
        &state.db,
        &user.tenant,
        &context,
        CancelReplenishmentWorkCommand::new(
            work_id_value(work_id)?,
            reason_to_domain(body.reason),
            body.note
                .map(ReplenishmentWorkCancellationNote::new)
                .transpose()
                .map_err(|error| invalid(error.to_string()))?,
        )
        .map_err(|error| invalid(error.to_string()))?,
    )
    .await?;
    Ok(Json(map_result(result)?))
}

fn validate_request(body: &CancelReplenishmentWorkRequest) -> V1Result<()> {
    if let Some(note) = body.note.as_deref() {
        if note.is_empty() || note.trim() != note || note.chars().any(char::is_control) {
            return Err(invalid(
                "note must be trimmed, nonempty, and control-free when provided",
            ));
        }
        if note.chars().count() > MAX_RELEASE_NOTE_LENGTH {
            return Err(invalid(format!(
                "note cannot exceed {MAX_RELEASE_NOTE_LENGTH} characters"
            )));
        }
    }
    if body.reason == ApiReason::Other && body.note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    Ok(())
}

fn map_result(
    result: CancelReplenishmentWorkResult,
) -> V1Result<ReplenishmentWorkCancellationResponse> {
    if result.status != ReplenishmentWorkStatus::Cancelled {
        return Err(V1Error::internal(
            "cancellation produced an invalid replenishment work status",
        ));
    }
    Ok(ReplenishmentWorkCancellationResponse {
        cancellation_id: result.cancellation_id.get(),
        work_id: result.work_id.get(),
        plan_id: result.plan_id.get(),
        policy_id: result.policy_id.get(),
        policy_revision: Revision::new(result.policy_revision.get())
            .map_err(|_| V1Error::internal("repository produced an invalid policy revision"))?,
        inventory_owner_id: result.scope.inventory_owner_id.get(),
        facility_id: result.scope.facility_id.get(),
        item_id: result.scope.item_id.get(),
        uom: result.scope.uom.as_str().to_owned(),
        pick_face_location_id: result.scope.pick_face_location_id.get(),
        source_inventory_balance_id: result.source_inventory_balance_id.get(),
        item_batch_id: result.item_batch_id.get(),
        quantity: result.quantity.get(),
        previous_status: map_work_status(result.previous_status),
        previous_assigned_user_id: result.previous_assigned_user_id.map(|value| value.get()),
        status: map_work_status(result.status),
        reason: reason_to_api(result.reason),
        note: result.note.map(|note| note.as_str().to_owned()),
        cancelled_by: result.cancelled_by.get(),
        cancelled_at: result.cancelled_at.to_rfc3339(),
    })
}

const fn reason_to_domain(reason: ApiReason) -> ReplenishmentWorkCancellationReason {
    match reason {
        ApiReason::DemandRemoved => ReplenishmentWorkCancellationReason::DemandRemoved,
        ApiReason::PolicyReconfigured => ReplenishmentWorkCancellationReason::PolicyReconfigured,
        ApiReason::SourceUnavailable => ReplenishmentWorkCancellationReason::SourceUnavailable,
        ApiReason::DestinationUnavailable => {
            ReplenishmentWorkCancellationReason::DestinationUnavailable
        }
        ApiReason::PlanningError => ReplenishmentWorkCancellationReason::PlanningError,
        ApiReason::Other => ReplenishmentWorkCancellationReason::Other,
    }
}

const fn reason_to_api(reason: ReplenishmentWorkCancellationReason) -> ApiReason {
    match reason {
        ReplenishmentWorkCancellationReason::DemandRemoved => ApiReason::DemandRemoved,
        ReplenishmentWorkCancellationReason::PolicyReconfigured => ApiReason::PolicyReconfigured,
        ReplenishmentWorkCancellationReason::SourceUnavailable => ApiReason::SourceUnavailable,
        ReplenishmentWorkCancellationReason::DestinationUnavailable => {
            ApiReason::DestinationUnavailable
        }
        ReplenishmentWorkCancellationReason::PlanningError => ApiReason::PlanningError,
        ReplenishmentWorkCancellationReason::Other => ApiReason::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_requires_bounded_evidence_for_other() {
        assert!(validate_request(&CancelReplenishmentWorkRequest {
            reason: ApiReason::Other,
            note: None,
        })
        .is_err());
        assert!(validate_request(&CancelReplenishmentWorkRequest {
            reason: ApiReason::PlanningError,
            note: Some("verified planning error".into()),
        })
        .is_ok());
    }
}
