use serde::{Deserialize, Serialize};

use super::Revision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationDeviceClass {
    Plc,
    Conveyor,
    Robotics,
    Sortation,
    Printer,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationControlMode {
    Disabled,
    Automatic,
    ManualFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationHealthState {
    Unknown,
    Healthy,
    Degraded,
    Offline,
    Faulted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRecoveryPolicy {
    DeviceDeduplicatedReplay,
    ProbeThenRetry,
    ManualReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationCommandStatus {
    Queued,
    Delivered,
    Accepted,
    Succeeded,
    Failed,
    ManualReview,
    ResolvedManually,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationManualResolution {
    ConfirmedExecuted,
    ConfirmedNotExecuted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRobotMissionKind {
    Pick,
    Place,
    Transport,
    Charge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationPrintFormat {
    Zpl,
    Pdf,
    Png,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationWeightUnit {
    Gram,
    Kilogram,
    Pound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationPlcCommand {
    SetDiscreteOutput { point: String, value: bool },
    PulseDiscreteOutput { point: String, duration_ms: u32 },
    ResetFault { fault_code: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationConveyorCommand {
    RouteCarrier {
        carrier_id: String,
        destination: String,
    },
    StartZone {
        zone: String,
    },
    StopZone {
        zone: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationRoboticsCommand {
    DispatchMission {
        mission_id: String,
        mission_kind: AutomationRobotMissionKind,
        source: String,
        destination: String,
        payload_id: Option<String>,
    },
    CancelMission {
        mission_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationSortationCommand {
    Divert {
        tracking_id: String,
        chute: String,
    },
    Reject {
        tracking_id: String,
        lane: String,
        reason_code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationPrinterCommand {
    PrintDocument {
        document_id: String,
        format: AutomationPrintFormat,
        content: String,
        copies: u16,
    },
    CancelPrintJob {
        spool_job_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationScaleCommand {
    ReadStableWeight {
        requested_unit: AutomationWeightUnit,
        timeout_ms: u32,
    },
    Tare,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "device_class", content = "command", rename_all = "snake_case")]
pub enum AutomationDeviceCommand {
    Plc(AutomationPlcCommand),
    Conveyor(AutomationConveyorCommand),
    Robotics(AutomationRoboticsCommand),
    Sortation(AutomationSortationCommand),
    Printer(AutomationPrinterCommand),
    Scale(AutomationScaleCommand),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationPlcResult {
    pub controller_reference: Option<String>,
    pub output_state: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationConveyorResult {
    pub controller_reference: Option<String>,
    pub observed_zone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationRoboticsResult {
    pub controller_reference: String,
    pub mission_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationSortationResult {
    pub controller_reference: Option<String>,
    pub observed_lane: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationPrinterResult {
    pub spool_job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationScaleResult {
    pub mass_milligrams: i64,
    pub stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "device_class", content = "result", rename_all = "snake_case")]
pub enum AutomationCommandResult {
    Plc(AutomationPlcResult),
    Conveyor(AutomationConveyorResult),
    Robotics(AutomationRoboticsResult),
    Sortation(AutomationSortationResult),
    Printer(AutomationPrinterResult),
    Scale(AutomationScaleResult),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAutomationDeviceRequest {
    pub facility_id: i64,
    pub device_key: String,
    pub class: AutomationDeviceClass,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeAutomationControlRequest {
    pub expected_revision: Revision,
    pub target_mode: AutomationControlMode,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_confirmation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnqueueAutomationCommandRequest {
    pub correlation_id: String,
    pub recovery_policy: AutomationRecoveryPolicy,
    pub command: AutomationDeviceCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveAutomationCommandRequest {
    pub expected_revision: Revision,
    pub outcome: AutomationManualResolution,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullAutomationCommandsRequest {
    pub facility_id: i64,
    pub agent_instance: String,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcknowledgeAutomationCommandRequest {
    pub delivery_token: String,
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportAutomationCommandRequest {
    pub expected_revision: Revision,
    pub status: AutomationCommandStatus,
    pub result: Option<AutomationCommandResult>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordAutomationHeartbeatRequest {
    pub agent_instance: String,
    pub health: AutomationHealthState,
    pub control_mode: AutomationControlMode,
    pub message: Option<String>,
    pub queued_commands: u32,
    pub manual_review_commands: u32,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationWorkspaceRequest {
    pub facility_id: Option<i64>,
    #[serde(default)]
    pub include_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationDeviceResponse {
    pub device_id: i64,
    pub facility_id: i64,
    pub device_key: String,
    pub class: AutomationDeviceClass,
    pub display_name: String,
    pub control_mode: AutomationControlMode,
    pub control_reason: String,
    pub control_changed_by: i64,
    pub control_changed_at: String,
    pub revision: Revision,
    pub health: AutomationHealthState,
    pub health_message: Option<String>,
    pub last_heartbeat_at: Option<String>,
    pub registered_by: i64,
    pub registered_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationCommandResponse {
    pub command_id: i64,
    pub facility_id: i64,
    pub device_id: i64,
    pub device_key: String,
    pub device_class: AutomationDeviceClass,
    pub correlation_id: String,
    pub recovery_policy: AutomationRecoveryPolicy,
    pub command: AutomationDeviceCommand,
    pub status: AutomationCommandStatus,
    pub revision: Revision,
    pub delivery_attempts: u32,
    pub assigned_service_account_id: Option<i64>,
    pub agent_instance: Option<String>,
    pub delivered_at: Option<String>,
    pub accepted_at: Option<String>,
    pub completed_at: Option<String>,
    pub result: Option<AutomationCommandResult>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub resolved_by: Option<i64>,
    pub resolution_outcome: Option<AutomationManualResolution>,
    pub resolution_reason: Option<String>,
    pub resolved_at: Option<String>,
    pub requested_by: i64,
    pub requested_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationCommandDeliveryResponse {
    pub command: AutomationCommandResponse,
    pub delivery_token: String,
    pub delivery_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationCommandDeliveryPage {
    pub items: Vec<AutomationCommandDeliveryResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationHeartbeatResponse {
    pub heartbeat_id: i64,
    pub device_id: i64,
    pub service_account_id: i64,
    pub agent_instance: String,
    pub health: AutomationHealthState,
    pub control_mode: AutomationControlMode,
    pub message: Option<String>,
    pub queued_commands: u32,
    pub manual_review_commands: u32,
    pub observed_at: String,
    pub received_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationWorkspaceResponse {
    pub devices: Vec<AutomationDeviceResponse>,
    pub commands: Vec<AutomationCommandResponse>,
    pub heartbeats: Vec<AutomationHeartbeatResponse>,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_payloads_are_strict_and_typed() {
        let command: AutomationDeviceCommand = serde_json::from_value(serde_json::json!({
            "device_class": "conveyor",
            "command": {
                "operation": "route_carrier",
                "carrier_id": "LPN-100",
                "destination": "CHUTE-7"
            }
        }))
        .unwrap();
        assert!(matches!(command, AutomationDeviceCommand::Conveyor(_)));
        assert!(
            serde_json::from_value::<AutomationDeviceCommand>(serde_json::json!({
                "device_class": "conveyor",
                "command": {
                    "operation": "route_carrier",
                    "carrier_id": "LPN-100",
                    "destination": "CHUTE-7",
                    "arbitrary": true
                }
            }))
            .is_err()
        );
    }
}
