use sqlx::Row;
use wareboxes_application::automation::{
    AutomationCommandReadModel, AutomationCommandStatus, AutomationDeviceReadModel,
    AutomationHeartbeatReadModel, AutomationManualResolution,
};
use wareboxes_domain::{
    AutomationCommandId, AutomationCommandResult, AutomationControlMode, AutomationDeviceClass,
    AutomationDeviceId, AutomationHealthState, AutomationHeartbeatId, AutomationRecoveryPolicy,
    FacilityId, ServiceAccountId, TenantId, UserId,
};

use crate::error::{AppError, AppResult};

pub(super) fn device(row: &sqlx::postgres::PgRow) -> AppResult<AutomationDeviceReadModel> {
    Ok(AutomationDeviceReadModel {
        device_id: id(row, "id", AutomationDeviceId::new)?,
        tenant_id: id(row, "tenant_id", TenantId::new)?,
        facility_id: id(row, "facility_id", FacilityId::new)?,
        device_key: row.try_get("device_key")?,
        class: device_class(row.try_get("device_class")?)?,
        display_name: row.try_get("display_name")?,
        control_mode: control_mode(row.try_get("control_mode")?)?,
        control_reason: row.try_get("control_reason")?,
        control_changed_by: id(row, "control_changed_by_user_id", UserId::new)?,
        control_changed_at: row.try_get("control_changed_at")?,
        revision: revision(row, "revision")?,
        health: health(row.try_get("health")?)?,
        health_message: row.try_get("health_message")?,
        last_heartbeat_at: row.try_get("last_heartbeat_at")?,
        registered_by: id(row, "registered_by_user_id", UserId::new)?,
        registered_at: row.try_get("registered_at")?,
    })
}

pub(super) fn command(row: &sqlx::postgres::PgRow) -> AppResult<AutomationCommandReadModel> {
    let payload: serde_json::Value = row.try_get("command_payload")?;
    let result_payload: Option<serde_json::Value> = row.try_get("result_payload")?;
    Ok(AutomationCommandReadModel {
        command_id: id(row, "id", AutomationCommandId::new)?,
        tenant_id: id(row, "tenant_id", TenantId::new)?,
        facility_id: id(row, "facility_id", FacilityId::new)?,
        device_id: id(row, "device_id", AutomationDeviceId::new)?,
        device_key: row.try_get("device_key")?,
        device_class: device_class(row.try_get("device_class")?)?,
        correlation_id: row.try_get("correlation_id")?,
        recovery_policy: recovery_policy(row.try_get("recovery_policy")?)?,
        command: serde_json::from_value(payload)
            .map_err(|error| AppError::internal(format!("invalid automation command: {error}")))?,
        status: command_status(row.try_get("status")?)?,
        revision: revision(row, "revision")?,
        delivery_attempts: u32::try_from(row.try_get::<i32, _>("delivery_attempts")?)
            .map_err(|_| AppError::internal("invalid automation delivery attempts"))?,
        assigned_service_account_id: row
            .try_get::<Option<i64>, _>("assigned_service_account_id")?
            .map(ServiceAccountId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        agent_instance: row.try_get("agent_instance")?,
        delivered_at: row.try_get("delivered_at")?,
        accepted_at: row.try_get("accepted_at")?,
        completed_at: row.try_get("completed_at")?,
        result: result_payload
            .map(serde_json::from_value::<AutomationCommandResult>)
            .transpose()
            .map_err(|error| AppError::internal(format!("invalid automation result: {error}")))?,
        error_code: row.try_get("error_code")?,
        error_message: row.try_get("error_message")?,
        resolved_by: row
            .try_get::<Option<i64>, _>("resolved_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        resolution_outcome: row
            .try_get::<Option<&str>, _>("resolution_outcome")?
            .map(manual_resolution)
            .transpose()?,
        resolution_reason: row.try_get("resolution_reason")?,
        resolved_at: row.try_get("resolved_at")?,
        requested_by: id(row, "requested_by_user_id", UserId::new)?,
        requested_at: row.try_get("requested_at")?,
    })
}

pub(super) fn heartbeat(row: &sqlx::postgres::PgRow) -> AppResult<AutomationHeartbeatReadModel> {
    Ok(AutomationHeartbeatReadModel {
        heartbeat_id: id(row, "id", AutomationHeartbeatId::new)?,
        device_id: id(row, "device_id", AutomationDeviceId::new)?,
        service_account_id: id(row, "service_account_id", ServiceAccountId::new)?,
        agent_instance: row.try_get("agent_instance")?,
        health: health(row.try_get("health")?)?,
        control_mode: control_mode(row.try_get("control_mode")?)?,
        message: row.try_get("message")?,
        queued_commands: u32::try_from(row.try_get::<i32, _>("queued_commands")?)
            .map_err(|_| AppError::internal("invalid queued automation command count"))?,
        manual_review_commands: u32::try_from(row.try_get::<i32, _>("manual_review_commands")?)
            .map_err(|_| AppError::internal("invalid manual-review automation command count"))?,
        observed_at: row.try_get("observed_at")?,
        received_at: row.try_get("received_at")?,
    })
}

fn id<T, E>(
    row: &sqlx::postgres::PgRow,
    column: &str,
    constructor: impl FnOnce(i64) -> Result<T, E>,
) -> AppResult<T>
where
    E: std::fmt::Display,
{
    constructor(row.try_get(column)?).map_err(|error| AppError::internal(error.to_string()))
}

fn revision(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<u32> {
    u32::try_from(row.try_get::<i32, _>(column)?)
        .map_err(|_| AppError::internal("invalid automation revision"))
}

pub(super) fn device_class(value: &str) -> AppResult<AutomationDeviceClass> {
    match value {
        "plc" => Ok(AutomationDeviceClass::Plc),
        "conveyor" => Ok(AutomationDeviceClass::Conveyor),
        "robotics" => Ok(AutomationDeviceClass::Robotics),
        "sortation" => Ok(AutomationDeviceClass::Sortation),
        "printer" => Ok(AutomationDeviceClass::Printer),
        "scale" => Ok(AutomationDeviceClass::Scale),
        _ => Err(AppError::internal("invalid automation device class")),
    }
}

pub(super) fn control_mode(value: &str) -> AppResult<AutomationControlMode> {
    match value {
        "disabled" => Ok(AutomationControlMode::Disabled),
        "automatic" => Ok(AutomationControlMode::Automatic),
        "manual_fallback" => Ok(AutomationControlMode::ManualFallback),
        _ => Err(AppError::internal("invalid automation control mode")),
    }
}

pub(super) fn health(value: &str) -> AppResult<AutomationHealthState> {
    match value {
        "unknown" => Ok(AutomationHealthState::Unknown),
        "healthy" => Ok(AutomationHealthState::Healthy),
        "degraded" => Ok(AutomationHealthState::Degraded),
        "offline" => Ok(AutomationHealthState::Offline),
        "faulted" => Ok(AutomationHealthState::Faulted),
        _ => Err(AppError::internal("invalid automation health state")),
    }
}

pub(super) fn recovery_policy(value: &str) -> AppResult<AutomationRecoveryPolicy> {
    match value {
        "device_deduplicated_replay" => Ok(AutomationRecoveryPolicy::DeviceDeduplicatedReplay),
        "probe_then_retry" => Ok(AutomationRecoveryPolicy::ProbeThenRetry),
        "manual_review" => Ok(AutomationRecoveryPolicy::ManualReview),
        _ => Err(AppError::internal("invalid automation recovery policy")),
    }
}

pub(super) fn command_status(value: &str) -> AppResult<AutomationCommandStatus> {
    match value {
        "queued" => Ok(AutomationCommandStatus::Queued),
        "delivered" => Ok(AutomationCommandStatus::Delivered),
        "accepted" => Ok(AutomationCommandStatus::Accepted),
        "succeeded" => Ok(AutomationCommandStatus::Succeeded),
        "failed" => Ok(AutomationCommandStatus::Failed),
        "manual_review" => Ok(AutomationCommandStatus::ManualReview),
        "resolved_manually" => Ok(AutomationCommandStatus::ResolvedManually),
        "cancelled" => Ok(AutomationCommandStatus::Cancelled),
        _ => Err(AppError::internal("invalid automation command status")),
    }
}

pub(super) fn manual_resolution(value: &str) -> AppResult<AutomationManualResolution> {
    match value {
        "confirmed_executed" => Ok(AutomationManualResolution::ConfirmedExecuted),
        "confirmed_not_executed" => Ok(AutomationManualResolution::ConfirmedNotExecuted),
        _ => Err(AppError::internal(
            "invalid automation manual resolution outcome",
        )),
    }
}

pub(super) const DEVICE_COLUMNS: &str = r#"
id,tenant_id,facility_id,device_key,device_class,display_name,control_mode,
control_reason,control_changed_by_user_id,control_changed_at,revision,health,
health_message,last_heartbeat_at,registered_by_user_id,registered_at"#;

pub(super) const COMMAND_COLUMNS: &str = r#"
command.id,command.tenant_id,command.facility_id,command.device_id,device.device_key,
command.device_class,command.correlation_id,command.recovery_policy,command.command_payload,
command.status,command.revision,command.delivery_attempts,command.assigned_service_account_id,
command.agent_instance,command.delivered_at,command.accepted_at,command.completed_at,
command.result_payload,command.error_code,command.error_message,command.resolved_by_user_id,
command.resolution_outcome,command.resolution_reason,command.resolved_at,
command.requested_by_user_id,command.requested_at"#;
