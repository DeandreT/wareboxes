use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    AutomationCommandResponse, AutomationHealthState as ApiAutomationHealthState,
    PackingScaleDevicePage, PackingScaleDeviceResponse, RequestPackingScaleWeight,
};
use wareboxes_application::automation::EnqueueAutomationCommand;
use wareboxes_domain::{
    AutomationCommandId, AutomationDeviceCommand, AutomationDeviceId, AutomationHealthState,
    AutomationRecoveryPolicy, AutomationScaleCommand, AutomationWeightUnit,
};

use super::{carton_id_value, session_id_value, PERMISSION};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::routes::v1::error::V1Result;
use crate::state::AppState;

pub async fn packing_scale_devices(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(session_id): Path<i64>,
) -> V1Result<Json<PackingScaleDevicePage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let devices = repo::packing::packing_scale_devices(
        &state.db,
        &user.tenant,
        session_id_value(session_id)?,
    )
    .await?;
    Ok(Json(PackingScaleDevicePage {
        items: devices
            .into_iter()
            .map(|device| {
                Ok(PackingScaleDeviceResponse {
                    device_id: device.device_id.get(),
                    device_key: device.device_key,
                    display_name: device.display_name,
                    health: health(device.health),
                    last_heartbeat_at: device
                        .last_heartbeat_at
                        .map(|time| time.to_rfc3339())
                        .ok_or_else(|| AppError::internal("available scale lacks a heartbeat"))?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?,
    }))
}

pub async fn request_packing_scale_weight(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(session_id): Path<i64>,
    Json(body): Json<RequestPackingScaleWeight>,
) -> V1Result<Json<AutomationCommandResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let session_id = session_id_value(session_id)?;
    let device_id = AutomationDeviceId::new(body.device_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let carton_id = carton_id_value(body.carton_id)?;
    let packing_scale_context = repo::packing::require_packing_scale_device(
        &state.db,
        &user.tenant,
        session_id,
        carton_id,
        device_id,
    )
    .await?;
    let command = EnqueueAutomationCommand {
        device_id,
        correlation_id: format!(
            "packing-scale:{}:{}:{}",
            session_id.get(),
            carton_id.get(),
            idempotency_key.as_str()
        ),
        recovery_policy: AutomationRecoveryPolicy::DeviceDeduplicatedReplay,
        command: AutomationDeviceCommand::Scale(AutomationScaleCommand::ReadStableWeight {
            requested_unit: AutomationWeightUnit::Gram,
            timeout_ms: body.timeout_ms,
        }),
        packing_scale_context: Some(packing_scale_context),
    };
    let result = repo::automation::enqueue_command(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(crate::routes::v1::automation::mapping::command(
        result,
    )?))
}

pub async fn packing_scale_reading(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path((session_id, command_id)): Path<(i64, i64)>,
) -> V1Result<Json<AutomationCommandResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let result = repo::packing::packing_scale_reading(
        &state.db,
        &user.tenant,
        session_id_value(session_id)?,
        AutomationCommandId::new(command_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
    )
    .await?;
    Ok(Json(crate::routes::v1::automation::mapping::command(
        result,
    )?))
}

const fn health(value: AutomationHealthState) -> ApiAutomationHealthState {
    match value {
        AutomationHealthState::Unknown => ApiAutomationHealthState::Unknown,
        AutomationHealthState::Healthy => ApiAutomationHealthState::Healthy,
        AutomationHealthState::Degraded => ApiAutomationHealthState::Degraded,
        AutomationHealthState::Offline => ApiAutomationHealthState::Offline,
        AutomationHealthState::Faulted => ApiAutomationHealthState::Faulted,
    }
}
