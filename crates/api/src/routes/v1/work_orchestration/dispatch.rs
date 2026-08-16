use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ActivateWorkOrchestrationDispatchRequest, CancelWorkOrchestrationDispatchRequest, Revision,
    WorkOrchestrationDispatchCancellationReason as ApiCancellationReason,
    WorkOrchestrationDispatchResponse, WorkOrchestrationDispatchStatus as ApiDispatchStatus,
};
use wareboxes_application::work_orchestration::{
    ActivateWorkOrchestrationDispatchCommand, CancelWorkOrchestrationDispatchCommand,
    WorkOrchestrationDispatchReadModel,
};
use wareboxes_domain::{
    WorkOrchestrationDispatchCancellationReason, WorkOrchestrationDispatchId,
    WorkOrchestrationDispatchRevision, WorkOrchestrationDispatchStatus, WorkOrchestrationPlanId,
};

use super::{map_work_kind_to_api, validation, V1Result, SUPERVISOR_PERMISSION};
use crate::auth::CurrentTenant;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

pub async fn activate(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(plan_id): Path<i64>,
    Json(_body): Json<ActivateWorkOrchestrationDispatchRequest>,
) -> V1Result<Json<WorkOrchestrationDispatchResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let result = repo::work_orchestration::activate_dispatch(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &ActivateWorkOrchestrationDispatchCommand {
            tenant_id: user.tenant.tenant_id,
            plan_id: WorkOrchestrationPlanId::new(plan_id).map_err(validation)?,
        },
    )
    .await?;
    Ok(Json(map_dispatch(result)?))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(dispatch_id): Path<i64>,
    Json(body): Json<CancelWorkOrchestrationDispatchRequest>,
) -> V1Result<Json<WorkOrchestrationDispatchResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let result = repo::work_orchestration::cancel_dispatch(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &CancelWorkOrchestrationDispatchCommand {
            tenant_id: user.tenant.tenant_id,
            dispatch_id: WorkOrchestrationDispatchId::new(dispatch_id).map_err(validation)?,
            expected_revision: WorkOrchestrationDispatchRevision::new(body.expected_revision.get())
                .map_err(validation)?,
            reason: map_cancellation_reason(body.reason),
            note: body.note,
        },
    )
    .await?;
    Ok(Json(map_dispatch(result)?))
}

const fn map_cancellation_reason(
    value: ApiCancellationReason,
) -> WorkOrchestrationDispatchCancellationReason {
    match value {
        ApiCancellationReason::OperatorCancelled => {
            WorkOrchestrationDispatchCancellationReason::OperatorCancelled
        }
        ApiCancellationReason::WorkerUnavailable => {
            WorkOrchestrationDispatchCancellationReason::WorkerUnavailable
        }
        ApiCancellationReason::ScopeChanged => {
            WorkOrchestrationDispatchCancellationReason::ScopeChanged
        }
        ApiCancellationReason::PlanInvalidated => {
            WorkOrchestrationDispatchCancellationReason::PlanInvalidated
        }
        ApiCancellationReason::Other => WorkOrchestrationDispatchCancellationReason::Other,
    }
}

const fn map_cancellation_reason_to_api(
    value: WorkOrchestrationDispatchCancellationReason,
) -> ApiCancellationReason {
    match value {
        WorkOrchestrationDispatchCancellationReason::OperatorCancelled => {
            ApiCancellationReason::OperatorCancelled
        }
        WorkOrchestrationDispatchCancellationReason::WorkerUnavailable => {
            ApiCancellationReason::WorkerUnavailable
        }
        WorkOrchestrationDispatchCancellationReason::ScopeChanged => {
            ApiCancellationReason::ScopeChanged
        }
        WorkOrchestrationDispatchCancellationReason::PlanInvalidated => {
            ApiCancellationReason::PlanInvalidated
        }
        WorkOrchestrationDispatchCancellationReason::Other => ApiCancellationReason::Other,
    }
}

const fn map_status(value: WorkOrchestrationDispatchStatus) -> ApiDispatchStatus {
    match value {
        WorkOrchestrationDispatchStatus::Active => ApiDispatchStatus::Active,
        WorkOrchestrationDispatchStatus::Completed => ApiDispatchStatus::Completed,
        WorkOrchestrationDispatchStatus::Cancelled => ApiDispatchStatus::Cancelled,
    }
}

pub(super) fn map_dispatch(
    value: WorkOrchestrationDispatchReadModel,
) -> V1Result<WorkOrchestrationDispatchResponse> {
    Ok(WorkOrchestrationDispatchResponse {
        dispatch_id: value.dispatch_id.get(),
        facility_id: value.facility_id.get(),
        inventory_owner_id: value.inventory_owner_id.map(|id| id.get()),
        plan_id: value.plan_id.get(),
        worker_user_id: value.worker_user_id.get(),
        status: map_status(value.status),
        revision: Revision::new(value.revision.get()).map_err(super::invalid_result)?,
        current_sequence: value.current_sequence,
        current_work_task_id: value.current_work_task_id,
        current_work_kind: value.current_work_kind.map(map_work_kind_to_api),
        completed_item_count: value.completed_item_count,
        cancelled_item_count: value.cancelled_item_count,
        remaining_item_count: value.remaining_item_count,
        activated_by: value.activated_by.get(),
        activated_at: value.activated_at.to_rfc3339(),
        ended_by: value.ended_by.map(|id| id.get()),
        ended_at: value.ended_at.map(|value| value.to_rfc3339()),
        cancellation_reason: value
            .cancellation_reason
            .map(map_cancellation_reason_to_api),
        cancellation_note: value.cancellation_note,
    })
}
