//! Cloud-side automation device registry, command delivery, and evidence contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    AutomationCommandId, AutomationCommandResult, AutomationControlMode, AutomationDeviceClass,
    AutomationDeviceCommand, AutomationDeviceId, AutomationHealthState, AutomationHeartbeatId,
    AutomationRecoveryPolicy, FacilityId, ServiceAccountId, TenantId, Timestamp, UserId,
};

pub const REGISTER_AUTOMATION_DEVICE_OPERATION: &str = "automation.device.register.v1";
pub const CHANGE_AUTOMATION_CONTROL_OPERATION: &str = "automation.device.control.v1";
pub const ENQUEUE_AUTOMATION_COMMAND_OPERATION: &str = "automation.command.enqueue.v1";
pub const RESOLVE_AUTOMATION_COMMAND_OPERATION: &str = "automation.command.resolve.v1";
pub const ACK_AUTOMATION_COMMAND_OPERATION: &str = "automation.command.ack.v1";
pub const REPORT_AUTOMATION_COMMAND_OPERATION: &str = "automation.command.report.v1";
pub const RECORD_AUTOMATION_HEARTBEAT_OPERATION: &str = "automation.device.heartbeat.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

impl AutomationCommandStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Delivered => "delivered",
            Self::Accepted => "accepted",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::ManualReview => "manual_review",
            Self::ResolvedManually => "resolved_manually",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::ManualReview
                | Self::ResolvedManually
                | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationManualResolution {
    ConfirmedExecuted,
    ConfirmedNotExecuted,
}

impl AutomationManualResolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmedExecuted => "confirmed_executed",
            Self::ConfirmedNotExecuted => "confirmed_not_executed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterAutomationDeviceCommand {
    pub facility_id: FacilityId,
    pub device_key: String,
    pub class: AutomationDeviceClass,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeAutomationControlCommand {
    pub device_id: AutomationDeviceId,
    pub expected_revision: u32,
    pub target_mode: AutomationControlMode,
    pub reason: String,
    pub safety_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnqueueAutomationCommand {
    pub device_id: AutomationDeviceId,
    pub correlation_id: String,
    pub recovery_policy: AutomationRecoveryPolicy,
    pub command: AutomationDeviceCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveAutomationCommand {
    pub command_id: AutomationCommandId,
    pub expected_revision: u32,
    pub outcome: AutomationManualResolution,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullAutomationCommands {
    pub facility_id: FacilityId,
    pub agent_instance: String,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcknowledgeAutomationCommand {
    pub command_id: AutomationCommandId,
    pub delivery_token: String,
    pub expected_revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportAutomationCommand {
    pub command_id: AutomationCommandId,
    pub expected_revision: u32,
    pub status: AutomationCommandStatus,
    pub result: Option<AutomationCommandResult>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordAutomationHeartbeat {
    pub device_id: AutomationDeviceId,
    pub agent_instance: String,
    pub health: AutomationHealthState,
    pub control_mode: AutomationControlMode,
    pub message: Option<String>,
    pub queued_commands: u32,
    pub manual_review_commands: u32,
    pub observed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationDeviceReadModel {
    pub device_id: AutomationDeviceId,
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub device_key: String,
    pub class: AutomationDeviceClass,
    pub display_name: String,
    pub control_mode: AutomationControlMode,
    pub control_reason: String,
    pub control_changed_by: UserId,
    pub control_changed_at: Timestamp,
    pub revision: u32,
    pub health: AutomationHealthState,
    pub health_message: Option<String>,
    pub last_heartbeat_at: Option<Timestamp>,
    pub registered_by: UserId,
    pub registered_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationCommandReadModel {
    pub command_id: AutomationCommandId,
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub device_id: AutomationDeviceId,
    pub device_key: String,
    pub device_class: AutomationDeviceClass,
    pub correlation_id: String,
    pub recovery_policy: AutomationRecoveryPolicy,
    pub command: AutomationDeviceCommand,
    pub status: AutomationCommandStatus,
    pub revision: u32,
    pub delivery_attempts: u32,
    pub assigned_service_account_id: Option<ServiceAccountId>,
    pub agent_instance: Option<String>,
    pub delivered_at: Option<Timestamp>,
    pub accepted_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub result: Option<AutomationCommandResult>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub resolved_by: Option<UserId>,
    pub resolution_outcome: Option<AutomationManualResolution>,
    pub resolution_reason: Option<String>,
    pub resolved_at: Option<Timestamp>,
    pub requested_by: UserId,
    pub requested_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationDeliveryReadModel {
    pub command: AutomationCommandReadModel,
    pub delivery_token: String,
    pub delivery_expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationHeartbeatReadModel {
    pub heartbeat_id: AutomationHeartbeatId,
    pub device_id: AutomationDeviceId,
    pub service_account_id: ServiceAccountId,
    pub agent_instance: String,
    pub health: AutomationHealthState,
    pub control_mode: AutomationControlMode,
    pub message: Option<String>,
    pub queued_commands: u32,
    pub manual_review_commands: u32,
    pub observed_at: Timestamp,
    pub received_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationWorkspaceReadModel {
    pub devices: Vec<AutomationDeviceReadModel>,
    pub commands: Vec<AutomationCommandReadModel>,
    pub heartbeats: Vec<AutomationHeartbeatReadModel>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutomationWorkspaceFilter {
    pub facility_id: Option<FacilityId>,
    pub include_history: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_command_outcomes_are_terminal() {
        assert!(!AutomationCommandStatus::Accepted.is_terminal());
        assert!(AutomationCommandStatus::ManualReview.is_terminal());
        assert!(AutomationCommandStatus::ResolvedManually.is_terminal());
        assert!(AutomationCommandStatus::Succeeded.is_terminal());
    }
}
