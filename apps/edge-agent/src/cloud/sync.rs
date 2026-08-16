use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;
use wareboxes_api_contract::v1::{
    AutomationCommandStatus, AutomationDeviceClass, AutomationDeviceResponse,
    PullAutomationCommandsRequest,
};

use super::mapping::{
    acknowledgement_request, delivery_to_local, heartbeat_request, local_control_mode,
    report_request, revision_u32, CloudMappingError,
};
use super::{CloudTransport, CloudTransportError};
use crate::engine::{EdgeEngine, EngineError};
use crate::store::StoreError;
use crate::types::{ActorId, ControlAction, ControlMode, DeviceClass, DeviceStatus};

#[derive(Debug, Clone)]
pub struct CloudSyncConfig {
    pub tenant_id: i64,
    pub facility_id: i64,
    pub agent_instance: String,
    pub pull_limit: u16,
    pub sync_batch_size: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct CloudSyncLoopConfig {
    pub poll_interval: Duration,
    pub error_backoff: Duration,
}

impl Default for CloudSyncLoopConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            error_backoff: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudSyncLoopControl {
    Continue,
    Shutdown,
}

impl CloudSyncConfig {
    fn validate(&self) -> Result<(), CloudSyncError> {
        if self.tenant_id <= 0 || self.facility_id <= 0 {
            return Err(CloudSyncError::InvalidConfig(
                "tenant and facility IDs must be positive",
            ));
        }
        if self.agent_instance.trim() != self.agent_instance
            || self.agent_instance.is_empty()
            || self.agent_instance.len() > 1_000
            || self.agent_instance.chars().any(char::is_control)
        {
            return Err(CloudSyncError::InvalidConfig(
                "agent instance must contain between 1 and 1,000 safe characters",
            ));
        }
        if !(1..=100).contains(&self.pull_limit) {
            return Err(CloudSyncError::InvalidConfig(
                "pull limit must be between 1 and 100",
            ));
        }
        if !(1..=1_000).contains(&self.sync_batch_size) {
            return Err(CloudSyncError::InvalidConfig(
                "sync batch size must be between 1 and 1,000",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum CloudSyncError {
    #[error("invalid cloud sync configuration: {0}")]
    InvalidConfig(&'static str),
    #[error(transparent)]
    Transport(#[from] CloudTransportError),
    #[error(transparent)]
    Mapping(#[from] CloudMappingError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("cloud protocol evidence is inconsistent: {0}")]
    Protocol(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloudSyncSummary {
    pub controls_restricted: u64,
    pub heartbeats_published: u64,
    pub commands_pulled: u64,
    pub commands_persisted: u64,
    pub acknowledgements_published: u64,
    pub commands_executed: u64,
    pub reports_published: u64,
}

pub struct CloudSync<'a, T> {
    transport: &'a mut T,
    engine: &'a mut EdgeEngine,
    actor: ActorId,
    config: CloudSyncConfig,
}

impl<'a, T> CloudSync<'a, T>
where
    T: CloudTransport,
{
    pub fn new(
        transport: &'a mut T,
        engine: &'a mut EdgeEngine,
        actor: ActorId,
        config: CloudSyncConfig,
    ) -> Result<Self, CloudSyncError> {
        config.validate()?;
        Ok(Self {
            transport,
            engine,
            actor,
            config,
        })
    }

    pub fn run_once(&mut self, now: DateTime<Utc>) -> Result<CloudSyncSummary, CloudSyncError> {
        let mut summary = CloudSyncSummary::default();
        summary.acknowledgements_published += self.flush_acknowledgements(now)?;
        summary.reports_published += self.flush_reports(now)?;

        let devices = self.transport.assigned_devices(self.config.facility_id)?;
        let cloud_devices = self.validate_devices(devices)?;
        summary.controls_restricted += self.reconcile_restrictive_controls(&cloud_devices, now)?;

        let statuses = self.engine.refresh_heartbeats(now)?;
        summary.heartbeats_published += self.publish_heartbeats(&cloud_devices, &statuses, now)?;

        let page = self.transport.pull_commands(
            &PullAutomationCommandsRequest {
                facility_id: self.config.facility_id,
                agent_instance: self.config.agent_instance.clone(),
                limit: self.config.pull_limit,
            },
            &format!("edge-pull:{}", Uuid::new_v4()),
        )?;
        if page.items.len() > usize::from(self.config.pull_limit) {
            return Err(CloudSyncError::Protocol(
                "command pull exceeded the requested limit".into(),
            ));
        }
        summary.commands_pulled = u64::try_from(page.items.len())
            .map_err(|_| CloudSyncError::Protocol("command pull count overflowed".into()))?;
        for delivery in page.items {
            if delivery.command.facility_id != self.config.facility_id
                || delivery.command.status != AutomationCommandStatus::Delivered
                || delivery.command.agent_instance.as_deref()
                    != Some(self.config.agent_instance.as_str())
            {
                return Err(CloudSyncError::Protocol(
                    "command delivery scope or assignment does not match this agent".into(),
                ));
            }
            let (request, cloud) = delivery_to_local(&delivery, self.config.tenant_id)?;
            self.engine.submit_cloud_delivery(request, &cloud, now)?;
            summary.commands_persisted += 1;
        }
        summary.acknowledgements_published += self.flush_acknowledgements(now)?;

        let execution = self.engine.run_once(now)?;
        summary.commands_executed = execution.claimed;
        summary.reports_published += self.flush_reports(now)?;
        Ok(summary)
    }

    pub fn run_until_shutdown<F>(
        &mut self,
        loop_config: CloudSyncLoopConfig,
        mut observe_cycle: F,
    ) -> Result<(), CloudSyncError>
    where
        F: FnMut(&Result<CloudSyncSummary, CloudSyncError>) -> CloudSyncLoopControl,
    {
        const MIN_LOOP_INTERVAL: Duration = Duration::from_millis(100);
        if loop_config.poll_interval < MIN_LOOP_INTERVAL
            || loop_config.error_backoff < MIN_LOOP_INTERVAL
        {
            return Err(CloudSyncError::InvalidConfig(
                "cloud loop intervals must be at least 100 milliseconds",
            ));
        }
        loop {
            let result = self.run_once(Utc::now());
            let sleep_for = if result.is_ok() {
                loop_config.poll_interval
            } else {
                loop_config.error_backoff
            };
            if observe_cycle(&result) == CloudSyncLoopControl::Shutdown {
                return Ok(());
            }
            std::thread::sleep(sleep_for);
        }
    }

    fn flush_acknowledgements(&mut self, now: DateTime<Utc>) -> Result<u64, CloudSyncError> {
        let pending = self
            .engine
            .store()
            .pending_cloud_acknowledgements(self.config.sync_batch_size)?;
        let mut published = 0_u64;
        for item in pending {
            let request = acknowledgement_request(&item)?;
            let cloud_command_id = item.cloud.delivery.cloud_command_id;
            let result = match self.transport.acknowledge_command(
                cloud_command_id,
                &request,
                &format!(
                    "edge-ack:{cloud_command_id}:{}",
                    item.cloud.delivery.delivery_revision
                ),
            ) {
                Ok(result) => result,
                Err(error) => {
                    self.engine.store_mut().record_cloud_error(
                        cloud_command_id,
                        &error.to_string(),
                        now,
                    )?;
                    return Err(error.into());
                }
            };
            if result.command_id != cloud_command_id
                || result.status != AutomationCommandStatus::Accepted
            {
                return Err(CloudSyncError::Protocol(
                    "cloud acknowledgement returned the wrong command or status".into(),
                ));
            }
            self.engine.store_mut().mark_cloud_acknowledged(
                cloud_command_id,
                item.cloud.delivery.delivery_revision,
                revision_u32(result.revision)?,
                now,
            )?;
            published += 1;
        }
        Ok(published)
    }

    fn flush_reports(&mut self, now: DateTime<Utc>) -> Result<u64, CloudSyncError> {
        let pending = self
            .engine
            .store()
            .pending_cloud_reports(self.config.sync_batch_size)?;
        let mut published = 0_u64;
        for item in pending {
            let request = report_request(&item)?;
            let cloud_command_id = item.cloud.delivery.cloud_command_id;
            let status = request.status;
            let result = match self.transport.report_command(
                cloud_command_id,
                &request,
                &format!(
                    "edge-report:{cloud_command_id}:{}:{}",
                    item.cloud
                        .acknowledgement_revision
                        .ok_or(CloudMappingError::InvalidRevision)?,
                    status_wire(status)
                ),
            ) {
                Ok(result) => result,
                Err(error) => {
                    self.engine.store_mut().record_cloud_error(
                        cloud_command_id,
                        &error.to_string(),
                        now,
                    )?;
                    return Err(error.into());
                }
            };
            if result.command_id != cloud_command_id || result.status != status {
                return Err(CloudSyncError::Protocol(
                    "cloud report returned the wrong command or status".into(),
                ));
            }
            self.engine.store_mut().mark_cloud_reported(
                cloud_command_id,
                revision_u32(result.revision)?,
                status_wire(status),
                now,
            )?;
            published += 1;
        }
        Ok(published)
    }

    fn validate_devices(
        &self,
        devices: Vec<AutomationDeviceResponse>,
    ) -> Result<BTreeMap<String, AutomationDeviceResponse>, CloudSyncError> {
        let mut result = BTreeMap::new();
        for device in devices {
            if device.facility_id != self.config.facility_id
                || result.insert(device.device_key.clone(), device).is_some()
            {
                return Err(CloudSyncError::Protocol(
                    "cloud device registry returned duplicate or cross-facility data".into(),
                ));
            }
        }
        Ok(result)
    }

    fn reconcile_restrictive_controls(
        &mut self,
        cloud_devices: &BTreeMap<String, AutomationDeviceResponse>,
        now: DateTime<Utc>,
    ) -> Result<u64, CloudSyncError> {
        let mut changed = 0_u64;
        for local in self.engine.store().list_devices()? {
            if local.descriptor.tenant_id.as_str() != self.config.tenant_id.to_string()
                || local.descriptor.facility_id.as_str() != self.config.facility_id.to_string()
            {
                continue;
            }
            let Some(cloud) = cloud_devices.get(local.descriptor.device_id.as_str()) else {
                continue;
            };
            if local.descriptor.class != device_class(cloud.class) {
                return Err(CloudSyncError::Protocol(format!(
                    "cloud device {} class does not match the local adapter",
                    cloud.device_key
                )));
            }
            let target = local_control_mode(cloud.control_mode);
            let action = match target {
                ControlMode::Disabled if local.control_mode != ControlMode::Disabled => {
                    Some(ControlAction::Disable)
                }
                ControlMode::ManualFallback
                    if local.control_mode != ControlMode::ManualFallback =>
                {
                    Some(ControlAction::EnterManualFallback)
                }
                ControlMode::Automatic | ControlMode::Disabled | ControlMode::ManualFallback => {
                    None
                }
            };
            if let Some(action) = action {
                self.engine.store_mut().change_control_mode(
                    &local.descriptor.device_id,
                    action,
                    &self.actor,
                    &cloud.control_reason,
                    now,
                )?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    fn publish_heartbeats(
        &mut self,
        cloud_devices: &BTreeMap<String, AutomationDeviceResponse>,
        statuses: &[DeviceStatus],
        now: DateTime<Utc>,
    ) -> Result<u64, CloudSyncError> {
        let mut published = 0_u64;
        for status in statuses {
            if status.descriptor.tenant_id.as_str() != self.config.tenant_id.to_string()
                || status.descriptor.facility_id.as_str() != self.config.facility_id.to_string()
            {
                continue;
            }
            let cloud = cloud_devices
                .get(status.descriptor.device_id.as_str())
                .ok_or_else(|| {
                    CloudSyncError::Protocol(format!(
                        "local adapter {} is not registered in the cloud facility",
                        status.descriptor.device_id
                    ))
                })?;
            let (queued, review) = self
                .engine
                .store()
                .cloud_reportable_count(status.descriptor.device_id.as_str())?;
            let request =
                heartbeat_request(status, &self.config.agent_instance, queued, review, now)?;
            let result = self.transport.record_heartbeat(
                cloud.device_id,
                &request,
                &format!(
                    "edge-heartbeat:{}:{}",
                    cloud.device_id,
                    now.timestamp_millis()
                ),
            )?;
            if result.device_id != cloud.device_id
                || result.agent_instance != self.config.agent_instance
            {
                return Err(CloudSyncError::Protocol(
                    "cloud heartbeat response does not match the local device".into(),
                ));
            }
            published += 1;
        }
        Ok(published)
    }
}

fn device_class(value: AutomationDeviceClass) -> DeviceClass {
    match value {
        AutomationDeviceClass::Plc => DeviceClass::Plc,
        AutomationDeviceClass::Conveyor => DeviceClass::Conveyor,
        AutomationDeviceClass::Robotics => DeviceClass::Robotics,
        AutomationDeviceClass::Sortation => DeviceClass::Sortation,
        AutomationDeviceClass::Printer => DeviceClass::Printer,
        AutomationDeviceClass::Scale => DeviceClass::Scale,
    }
}

fn status_wire(value: AutomationCommandStatus) -> &'static str {
    match value {
        AutomationCommandStatus::Succeeded => "succeeded",
        AutomationCommandStatus::Failed => "failed",
        AutomationCommandStatus::ManualReview => "manual_review",
        AutomationCommandStatus::Queued
        | AutomationCommandStatus::Delivered
        | AutomationCommandStatus::Accepted
        | AutomationCommandStatus::ResolvedManually
        | AutomationCommandStatus::Cancelled => "unsupported",
    }
}
