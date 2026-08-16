use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use wareboxes_api_contract::v1::{
    AcknowledgeAutomationCommandRequest, AutomationCommandDeliveryResponse,
    AutomationCommandResult, AutomationCommandStatus, AutomationControlMode, AutomationHealthState,
    AutomationRecoveryPolicy, RecordAutomationHeartbeatRequest, ReportAutomationCommandRequest,
    Revision,
};

use crate::command::{
    CommandRequest, CommandResult, CommandState, DeviceCommand, RecoveryPolicy,
    COMMAND_SCHEMA_VERSION,
};
use crate::store::{CloudDelivery, PendingCloudCommand};
use crate::types::{
    CommandId, ControlMode, CorrelationId, DeviceId, DeviceStatus, FacilityId, HealthState,
    IdempotencyKey, TenantId,
};

#[derive(Debug, Error)]
pub enum CloudMappingError {
    #[error("cloud command identity is invalid: {0}")]
    InvalidIdentity(String),
    #[error("cloud command revision is invalid")]
    InvalidRevision,
    #[error("cloud command payload is incompatible with the local engine: {0}")]
    IncompatiblePayload(String),
    #[error("local command {0} is not ready for a cloud report")]
    NotReportable(String),
    #[error("local succeeded command has no result")]
    MissingResult,
}

pub fn delivery_to_local(
    delivery: &AutomationCommandDeliveryResponse,
    tenant_id: i64,
) -> Result<(CommandRequest, CloudDelivery), CloudMappingError> {
    let command_id = delivery.command.command_id;
    let local_command_id = CommandId::new(format!("cloud:{command_id}"))
        .map_err(|error| CloudMappingError::InvalidIdentity(error.to_string()))?;
    let request = CommandRequest {
        schema_version: COMMAND_SCHEMA_VERSION,
        command_id: local_command_id,
        tenant_id: TenantId::new(tenant_id.to_string())
            .map_err(|error| CloudMappingError::InvalidIdentity(error.to_string()))?,
        facility_id: FacilityId::new(delivery.command.facility_id.to_string())
            .map_err(|error| CloudMappingError::InvalidIdentity(error.to_string()))?,
        device_id: DeviceId::new(delivery.command.device_key.clone())
            .map_err(|error| CloudMappingError::InvalidIdentity(error.to_string()))?,
        correlation_id: CorrelationId::new(format!("cloud:{command_id}"))
            .map_err(|error| CloudMappingError::InvalidIdentity(error.to_string()))?,
        idempotency_key: IdempotencyKey::new(format!("cloud:{command_id}"))
            .map_err(|error| CloudMappingError::InvalidIdentity(error.to_string()))?,
        recovery_policy: recovery_policy(delivery.command.recovery_policy),
        command: transcode::<_, DeviceCommand>(&delivery.command.command)?,
    };
    let cloud = CloudDelivery {
        cloud_command_id: command_id,
        cloud_device_id: delivery.command.device_id,
        delivery_token: delivery.delivery_token.clone(),
        delivery_revision: revision_u32(delivery.command.revision)?,
    };
    Ok((request, cloud))
}

pub fn acknowledgement_request(
    pending: &PendingCloudCommand,
) -> Result<AcknowledgeAutomationCommandRequest, CloudMappingError> {
    Ok(AcknowledgeAutomationCommandRequest {
        delivery_token: pending.cloud.delivery.delivery_token.clone(),
        expected_revision: revision(pending.cloud.delivery.delivery_revision)?,
    })
}

pub fn report_request(
    pending: &PendingCloudCommand,
) -> Result<ReportAutomationCommandRequest, CloudMappingError> {
    let expected_revision = pending
        .cloud
        .acknowledgement_revision
        .ok_or(CloudMappingError::InvalidRevision)?;
    let (status, result, error_code, error_message) = match pending.command.state {
        CommandState::Succeeded => (
            AutomationCommandStatus::Succeeded,
            Some(command_result(
                pending
                    .command
                    .result
                    .as_ref()
                    .ok_or(CloudMappingError::MissingResult)?,
            )?),
            None,
            None,
        ),
        CommandState::Failed => (
            AutomationCommandStatus::Failed,
            None,
            Some("EDGE_PERMANENT_FAILURE".into()),
            Some(error_message(&pending.command, "edge command failed")),
        ),
        CommandState::Cancelled => (
            AutomationCommandStatus::Failed,
            None,
            Some("EDGE_CANCELLED".into()),
            Some(error_message(
                &pending.command,
                "edge command was cancelled",
            )),
        ),
        CommandState::ManualReview | CommandState::ResolvedManually => (
            AutomationCommandStatus::ManualReview,
            None,
            Some("EDGE_MANUAL_REVIEW".into()),
            Some(error_message(
                &pending.command,
                "edge command requires manual reconciliation",
            )),
        ),
        CommandState::Queued
        | CommandState::Executing
        | CommandState::RetryWait
        | CommandState::RecoveryWait => {
            return Err(CloudMappingError::NotReportable(
                pending.cloud.local_command_id.clone(),
            ));
        }
    };
    Ok(ReportAutomationCommandRequest {
        expected_revision: revision(expected_revision)?,
        status,
        result,
        error_code,
        error_message,
        occurred_at: pending.command.updated_at.to_rfc3339(),
    })
}

pub fn heartbeat_request(
    status: &DeviceStatus,
    agent_instance: &str,
    queued_commands: u32,
    manual_review_commands: u32,
    observed_at: chrono::DateTime<chrono::Utc>,
) -> Result<RecordAutomationHeartbeatRequest, CloudMappingError> {
    Ok(RecordAutomationHeartbeatRequest {
        agent_instance: agent_instance.to_owned(),
        health: health(status.health),
        control_mode: control_mode(status.control_mode),
        message: status.health_message.clone(),
        queued_commands,
        manual_review_commands,
        observed_at: observed_at.to_rfc3339(),
    })
}

pub const fn local_control_mode(value: AutomationControlMode) -> ControlMode {
    match value {
        AutomationControlMode::Disabled => ControlMode::Disabled,
        AutomationControlMode::Automatic => ControlMode::Automatic,
        AutomationControlMode::ManualFallback => ControlMode::ManualFallback,
    }
}

fn recovery_policy(value: AutomationRecoveryPolicy) -> RecoveryPolicy {
    match value {
        AutomationRecoveryPolicy::DeviceDeduplicatedReplay => {
            RecoveryPolicy::DeviceDeduplicatedReplay
        }
        AutomationRecoveryPolicy::ProbeThenRetry => RecoveryPolicy::ProbeThenRetry,
        AutomationRecoveryPolicy::ManualReview => RecoveryPolicy::ManualReview,
    }
}

fn health(value: HealthState) -> AutomationHealthState {
    match value {
        HealthState::Unknown => AutomationHealthState::Unknown,
        HealthState::Healthy => AutomationHealthState::Healthy,
        HealthState::Degraded => AutomationHealthState::Degraded,
        HealthState::Offline => AutomationHealthState::Offline,
        HealthState::Faulted => AutomationHealthState::Faulted,
    }
}

fn control_mode(value: ControlMode) -> AutomationControlMode {
    match value {
        ControlMode::Disabled => AutomationControlMode::Disabled,
        ControlMode::Automatic => AutomationControlMode::Automatic,
        ControlMode::ManualFallback => AutomationControlMode::ManualFallback,
    }
}

fn command_result(value: &CommandResult) -> Result<AutomationCommandResult, CloudMappingError> {
    transcode(value)
}

fn revision(value: u32) -> Result<Revision, CloudMappingError> {
    Revision::new(i64::from(value)).map_err(|_| CloudMappingError::InvalidRevision)
}

pub fn revision_u32(value: Revision) -> Result<u32, CloudMappingError> {
    u32::try_from(value.get()).map_err(|_| CloudMappingError::InvalidRevision)
}

fn transcode<T: Serialize, U: DeserializeOwned>(value: &T) -> Result<U, CloudMappingError> {
    serde_json::from_value(
        serde_json::to_value(value)
            .map_err(|error| CloudMappingError::IncompatiblePayload(error.to_string()))?,
    )
    .map_err(|error| CloudMappingError::IncompatiblePayload(error.to_string()))
}

fn error_message(command: &crate::command::CommandRecord, fallback: &str) -> String {
    command
        .resolution_note
        .as_ref()
        .or(command.last_error.as_ref())
        .cloned()
        .unwrap_or_else(|| fallback.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{AutomationDeviceCommand, AutomationScaleCommand};

    #[test]
    fn typed_scale_delivery_maps_to_stable_local_identity() {
        let delivery = AutomationCommandDeliveryResponse {
            command: wareboxes_api_contract::v1::AutomationCommandResponse {
                command_id: 41,
                facility_id: 7,
                device_id: 12,
                device_key: "scale-01".into(),
                device_class: wareboxes_api_contract::v1::AutomationDeviceClass::Scale,
                correlation_id: "carton 123 weight".into(),
                recovery_policy: AutomationRecoveryPolicy::ManualReview,
                command: AutomationDeviceCommand::Scale(AutomationScaleCommand::Tare),
                status: AutomationCommandStatus::Delivered,
                revision: Revision::new(2).unwrap(),
                delivery_attempts: 1,
                assigned_service_account_id: Some(5),
                agent_instance: Some("edge-a".into()),
                delivered_at: Some("2026-08-16T00:00:00Z".into()),
                accepted_at: None,
                completed_at: None,
                result: None,
                error_code: None,
                error_message: None,
                resolved_by: None,
                resolution_outcome: None,
                resolution_reason: None,
                resolved_at: None,
                requested_by: 3,
                requested_at: "2026-08-16T00:00:00Z".into(),
            },
            delivery_token: "T".repeat(48),
            delivery_expires_at: "2026-08-16T00:00:30Z".into(),
        };
        let (local, cloud) = delivery_to_local(&delivery, 9).unwrap();
        assert_eq!(local.command_id.as_str(), "cloud:41");
        assert_eq!(local.device_id.as_str(), "scale-01");
        assert_eq!(cloud.cloud_device_id, 12);
    }
}
