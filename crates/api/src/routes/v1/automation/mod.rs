pub(crate) mod mapping;

use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    AcknowledgeAutomationCommandRequest, AutomationCommandDeliveryPage, AutomationCommandResponse,
    AutomationDeviceResponse, AutomationEdgeDevicePage, AutomationEdgeDevicesRequest,
    AutomationWorkspaceRequest, AutomationWorkspaceResponse, ChangeAutomationControlRequest,
    EnqueueAutomationCommandRequest, PullAutomationCommandsRequest,
    RecordAutomationHeartbeatRequest, RegisterAutomationDeviceRequest,
    ReportAutomationCommandRequest, ResolveAutomationCommandRequest,
};
use wareboxes_application::automation::{
    AcknowledgeAutomationCommand, AutomationWorkspaceFilter, ChangeAutomationControlCommand,
    EnqueueAutomationCommand, PullAutomationCommands, RecordAutomationHeartbeat,
    RegisterAutomationDeviceCommand, ReportAutomationCommand, ResolveAutomationCommand,
};
use wareboxes_domain::{AutomationCommandId, AutomationDeviceId, FacilityId};

use super::error::V1Result;
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const SAFETY_CONFIRMATION: &str = "CONFIRM-SAFE-TO-RESUME";

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<AutomationWorkspaceRequest>,
) -> V1Result<Json<AutomationWorkspaceResponse>> {
    supervisor_identity(&state, &user).await?;
    let filter = AutomationWorkspaceFilter {
        facility_id: request
            .facility_id
            .map(FacilityId::new)
            .transpose()
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        include_history: request.include_history,
    };
    Ok(Json(
        workspace_for_access(&state, &user.tenant, &filter).await?,
    ))
}

pub(crate) async fn workspace_for_access(
    state: &AppState,
    access: &wareboxes_core::models::TenantAccess,
    filter: &AutomationWorkspaceFilter,
) -> Result<AutomationWorkspaceResponse, AppError> {
    mapping::workspace(repo::automation::workspace(&state.db, access, filter).await?)
}

pub async fn register_device(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RegisterAutomationDeviceRequest>,
) -> V1Result<Json<AutomationDeviceResponse>> {
    supervisor_identity(&state, &user).await?;
    let command = RegisterAutomationDeviceCommand {
        facility_id: FacilityId::new(body.facility_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        device_key: body.device_key,
        class: mapping::transcode_request(body.class)?,
        display_name: body.display_name,
    };
    let result = repo::automation::register_device(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::device(result)?))
}

pub async fn change_control(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(device_id): Path<i64>,
    Json(body): Json<ChangeAutomationControlRequest>,
) -> V1Result<Json<AutomationDeviceResponse>> {
    supervisor_identity(&state, &user).await?;
    let target_mode = mapping::transcode_request(body.target_mode)?;
    let safety_confirmed = body.safety_confirmation.as_deref() == Some(SAFETY_CONFIRMATION);
    if body.safety_confirmation.is_some() && !safety_confirmed {
        return Err(
            AppError::bad_request("automation safety confirmation token is invalid").into(),
        );
    }
    let command = ChangeAutomationControlCommand {
        device_id: AutomationDeviceId::new(device_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        expected_revision: revision(body.expected_revision)?,
        target_mode,
        reason: body.reason,
        safety_confirmed,
    };
    let result = repo::automation::change_control(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::device(result)?))
}

pub async fn enqueue_command(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(device_id): Path<i64>,
    Json(body): Json<EnqueueAutomationCommandRequest>,
) -> V1Result<Json<AutomationCommandResponse>> {
    supervisor_identity(&state, &user).await?;
    let command = EnqueueAutomationCommand {
        device_id: AutomationDeviceId::new(device_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        correlation_id: body.correlation_id,
        recovery_policy: mapping::transcode_request(body.recovery_policy)?,
        command: mapping::transcode_request(body.command)?,
        packing_scale_context: None,
        shipping_document_print_context: None,
    };
    let result = repo::automation::enqueue_command(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::command(result)?))
}

pub async fn resolve_command(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(command_id): Path<i64>,
    Json(body): Json<ResolveAutomationCommandRequest>,
) -> V1Result<Json<AutomationCommandResponse>> {
    supervisor_identity(&state, &user).await?;
    let command = ResolveAutomationCommand {
        command_id: AutomationCommandId::new(command_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        expected_revision: revision(body.expected_revision)?,
        outcome: mapping::transcode_request(body.outcome)?,
        reason: body.reason,
    };
    let result = repo::automation::resolve_command(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::command(result)?))
}

pub async fn pull_commands(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<PullAutomationCommandsRequest>,
) -> V1Result<Json<AutomationCommandDeliveryPage>> {
    let service_account_id = edge_identity(&state, &user).await?;
    let command = PullAutomationCommands {
        facility_id: FacilityId::new(body.facility_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        agent_instance: body.agent_instance,
        limit: body.limit,
    };
    let result = repo::automation::pull_commands(
        &state.db,
        &user.tenant,
        service_account_id,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(AutomationCommandDeliveryPage {
        items: result
            .into_iter()
            .map(mapping::delivery)
            .collect::<Result<_, _>>()?,
    }))
}

pub async fn assigned_devices(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(body): Query<AutomationEdgeDevicesRequest>,
) -> V1Result<Json<AutomationEdgeDevicePage>> {
    edge_identity(&state, &user).await?;
    let facility_id = FacilityId::new(body.facility_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let items = repo::automation::assigned_devices(&state.db, &user.tenant, facility_id)
        .await?
        .into_iter()
        .map(mapping::device)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(AutomationEdgeDevicePage { items }))
}

pub async fn acknowledge_command(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(command_id): Path<i64>,
    Json(body): Json<AcknowledgeAutomationCommandRequest>,
) -> V1Result<Json<AutomationCommandResponse>> {
    let service_account_id = edge_identity(&state, &user).await?;
    let command = AcknowledgeAutomationCommand {
        command_id: AutomationCommandId::new(command_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        delivery_token: body.delivery_token,
        expected_revision: revision(body.expected_revision)?,
    };
    let result = repo::automation::acknowledge_command(
        &state.db,
        &user.tenant,
        service_account_id,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::command(result)?))
}

pub async fn report_command(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(command_id): Path<i64>,
    Json(body): Json<ReportAutomationCommandRequest>,
) -> V1Result<Json<AutomationCommandResponse>> {
    let service_account_id = edge_identity(&state, &user).await?;
    let command = ReportAutomationCommand {
        command_id: AutomationCommandId::new(command_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        expected_revision: revision(body.expected_revision)?,
        status: mapping::transcode_request(body.status)?,
        result: body.result.map(mapping::transcode_request).transpose()?,
        error_code: body.error_code,
        error_message: body.error_message,
        occurred_at: chrono::DateTime::parse_from_rfc3339(&body.occurred_at)
            .map_err(|_| AppError::bad_request("automation occurred_at must be RFC3339"))?
            .with_timezone(&chrono::Utc),
    };
    let result = repo::automation::report_command(
        &state.db,
        &user.tenant,
        service_account_id,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::command(result)?))
}

pub async fn record_heartbeat(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(device_id): Path<i64>,
    Json(body): Json<RecordAutomationHeartbeatRequest>,
) -> V1Result<Json<wareboxes_api_contract::v1::AutomationHeartbeatResponse>> {
    let service_account_id = edge_identity(&state, &user).await?;
    let command = RecordAutomationHeartbeat {
        device_id: AutomationDeviceId::new(device_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        agent_instance: body.agent_instance,
        health: mapping::transcode_request(body.health)?,
        control_mode: mapping::transcode_request(body.control_mode)?,
        message: body.message,
        queued_commands: body.queued_commands,
        manual_review_commands: body.manual_review_commands,
        observed_at: chrono::DateTime::parse_from_rfc3339(&body.observed_at)
            .map_err(|_| AppError::bad_request("automation observed_at must be RFC3339"))?
            .with_timezone(&chrono::Utc),
    };
    let result = repo::automation::record_heartbeat(
        &state.db,
        &user.tenant,
        service_account_id,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::heartbeat(result)?))
}

async fn edge_identity(
    state: &AppState,
    user: &CurrentTenant,
) -> Result<wareboxes_domain::ServiceAccountId, AppError> {
    let service_account_id = user.service_account_id.ok_or_else(AppError::forbidden)?;
    user.require_permission(&state.db, repo::automation::EDGE_PERMISSION)
        .await?;
    Ok(service_account_id)
}

async fn supervisor_identity(state: &AppState, user: &CurrentTenant) -> Result<(), AppError> {
    if user.is_service_account() {
        return Err(AppError::forbidden());
    }
    user.require_permission(&state.db, repo::automation::SUPERVISOR_PERMISSION)
        .await
}

fn revision(value: wareboxes_api_contract::v1::Revision) -> Result<u32, AppError> {
    u32::try_from(value.get()).map_err(|_| AppError::bad_request("revision is too large"))
}
