use serde::de::DeserializeOwned;
use serde::Serialize;
use wareboxes_api_contract::v1::{
    AutomationCommandDeliveryResponse, AutomationCommandResponse, AutomationDeviceResponse,
    AutomationHeartbeatResponse, AutomationWorkspaceResponse, PackingScaleCommandContextResponse,
    Revision, ShippingDocumentPrintContextResponse,
};
use wareboxes_application::automation::{
    AutomationCommandReadModel, AutomationDeliveryReadModel, AutomationDeviceReadModel,
    AutomationHeartbeatReadModel, AutomationWorkspaceReadModel,
};

use crate::error::{AppError, AppResult};

pub(super) fn transcode_request<T, U>(value: T) -> AppResult<U>
where
    T: Serialize,
    U: DeserializeOwned,
{
    serde_json::from_value(
        serde_json::to_value(value).map_err(|error| AppError::bad_request(error.to_string()))?,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))
}

fn transcode_response<T, U>(value: T) -> AppResult<U>
where
    T: Serialize,
    U: DeserializeOwned,
{
    serde_json::from_value(
        serde_json::to_value(value).map_err(|error| AppError::internal(error.to_string()))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))
}

fn revision(value: u32) -> AppResult<Revision> {
    Revision::new(i64::from(value)).map_err(|error| AppError::internal(error.to_string()))
}

pub(crate) fn device(value: AutomationDeviceReadModel) -> AppResult<AutomationDeviceResponse> {
    Ok(AutomationDeviceResponse {
        device_id: value.device_id.get(),
        facility_id: value.facility_id.get(),
        device_key: value.device_key,
        class: transcode_response(value.class)?,
        display_name: value.display_name,
        control_mode: transcode_response(value.control_mode)?,
        control_reason: value.control_reason,
        control_changed_by: value.control_changed_by.get(),
        control_changed_at: value.control_changed_at.to_rfc3339(),
        revision: revision(value.revision)?,
        health: transcode_response(value.health)?,
        health_message: value.health_message,
        last_heartbeat_at: value.last_heartbeat_at.map(|time| time.to_rfc3339()),
        registered_by: value.registered_by.get(),
        registered_at: value.registered_at.to_rfc3339(),
    })
}

pub(crate) fn command(value: AutomationCommandReadModel) -> AppResult<AutomationCommandResponse> {
    Ok(AutomationCommandResponse {
        command_id: value.command_id.get(),
        facility_id: value.facility_id.get(),
        device_id: value.device_id.get(),
        device_key: value.device_key,
        device_class: transcode_response(value.device_class)?,
        correlation_id: value.correlation_id,
        recovery_policy: transcode_response(value.recovery_policy)?,
        command: transcode_response(value.command)?,
        packing_scale_context: value.packing_scale_context.map(|context| {
            PackingScaleCommandContextResponse {
                inventory_owner_id: context.inventory_owner_id.get(),
                session_id: context.session_id.get(),
                carton_id: context.carton_id.get(),
                carton_reopen_count: context.carton_reopen_count,
            }
        }),
        shipping_document_print_context: value.shipping_document_print_context.map(|context| {
            ShippingDocumentPrintContextResponse {
                inventory_owner_id: context.inventory_owner_id.get(),
                shipment_id: context.shipment_id.get(),
                document_id: context.document_id.get(),
                content_sha256: context.content_sha256,
            }
        }),
        status: transcode_response(value.status)?,
        revision: revision(value.revision)?,
        delivery_attempts: value.delivery_attempts,
        assigned_service_account_id: value.assigned_service_account_id.map(|id| id.get()),
        agent_instance: value.agent_instance,
        delivered_at: value.delivered_at.map(|time| time.to_rfc3339()),
        accepted_at: value.accepted_at.map(|time| time.to_rfc3339()),
        completed_at: value.completed_at.map(|time| time.to_rfc3339()),
        result: value.result.map(transcode_response).transpose()?,
        error_code: value.error_code,
        error_message: value.error_message,
        resolved_by: value.resolved_by.map(|id| id.get()),
        resolution_outcome: value
            .resolution_outcome
            .map(transcode_response)
            .transpose()?,
        resolution_reason: value.resolution_reason,
        resolved_at: value.resolved_at.map(|time| time.to_rfc3339()),
        requested_by: value.requested_by.get(),
        requested_at: value.requested_at.to_rfc3339(),
    })
}

pub(super) fn delivery(
    value: AutomationDeliveryReadModel,
) -> AppResult<AutomationCommandDeliveryResponse> {
    Ok(AutomationCommandDeliveryResponse {
        command: command(value.command)?,
        delivery_token: value.delivery_token,
        delivery_expires_at: value.delivery_expires_at.to_rfc3339(),
    })
}

pub(super) fn heartbeat(
    value: AutomationHeartbeatReadModel,
) -> AppResult<AutomationHeartbeatResponse> {
    Ok(AutomationHeartbeatResponse {
        heartbeat_id: value.heartbeat_id.get(),
        device_id: value.device_id.get(),
        service_account_id: value.service_account_id.get(),
        agent_instance: value.agent_instance,
        health: transcode_response(value.health)?,
        control_mode: transcode_response(value.control_mode)?,
        message: value.message,
        queued_commands: value.queued_commands,
        manual_review_commands: value.manual_review_commands,
        observed_at: value.observed_at.to_rfc3339(),
        received_at: value.received_at.to_rfc3339(),
    })
}

pub(super) fn workspace(
    value: AutomationWorkspaceReadModel,
) -> AppResult<AutomationWorkspaceResponse> {
    Ok(AutomationWorkspaceResponse {
        devices: value
            .devices
            .into_iter()
            .map(device)
            .collect::<AppResult<_>>()?,
        commands: value
            .commands
            .into_iter()
            .map(command)
            .collect::<AppResult<_>>()?,
        heartbeats: value
            .heartbeats
            .into_iter()
            .map(heartbeat)
            .collect::<AppResult<_>>()?,
        truncated: value.truncated,
    })
}
