mod client;
mod mapping;
mod sync;

use wareboxes_api_contract::v1::{
    AcknowledgeAutomationCommandRequest, AutomationCommandDeliveryPage, AutomationCommandResponse,
    AutomationDeviceResponse, AutomationHeartbeatResponse, PullAutomationCommandsRequest,
    RecordAutomationHeartbeatRequest, ReportAutomationCommandRequest,
};

pub use client::{CloudClient, CloudClientConfig, CloudTransportError};
pub use sync::{
    CloudSync, CloudSyncConfig, CloudSyncError, CloudSyncLoopConfig, CloudSyncLoopControl,
    CloudSyncSummary,
};

pub trait CloudTransport {
    fn assigned_devices(
        &mut self,
        facility_id: i64,
    ) -> Result<Vec<AutomationDeviceResponse>, CloudTransportError>;

    fn pull_commands(
        &mut self,
        request: &PullAutomationCommandsRequest,
        idempotency_key: &str,
    ) -> Result<AutomationCommandDeliveryPage, CloudTransportError>;

    fn acknowledge_command(
        &mut self,
        command_id: i64,
        request: &AcknowledgeAutomationCommandRequest,
        idempotency_key: &str,
    ) -> Result<AutomationCommandResponse, CloudTransportError>;

    fn report_command(
        &mut self,
        command_id: i64,
        request: &ReportAutomationCommandRequest,
        idempotency_key: &str,
    ) -> Result<AutomationCommandResponse, CloudTransportError>;

    fn record_heartbeat(
        &mut self,
        device_id: i64,
        request: &RecordAutomationHeartbeatRequest,
        idempotency_key: &str,
    ) -> Result<AutomationHeartbeatResponse, CloudTransportError>;
}
