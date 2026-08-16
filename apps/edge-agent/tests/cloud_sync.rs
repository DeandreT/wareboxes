use chrono::{DateTime, Utc};
use wareboxes_api_contract::v1::{
    AcknowledgeAutomationCommandRequest, AutomationCommandDeliveryPage,
    AutomationCommandDeliveryResponse, AutomationCommandResponse, AutomationCommandStatus,
    AutomationControlMode, AutomationDeviceClass, AutomationDeviceCommand,
    AutomationDeviceResponse, AutomationHealthState, AutomationHeartbeatResponse,
    AutomationRecoveryPolicy, AutomationScaleCommand, PullAutomationCommandsRequest,
    RecordAutomationHeartbeatRequest, ReportAutomationCommandRequest, Revision,
};
use wareboxes_edge_agent::command::ScaleResult;
use wareboxes_edge_agent::{
    ActorId, AdapterCapabilities, AdapterFailure, AdapterRegistry, CloudSync, CloudSyncConfig,
    CloudSyncLoopConfig, CloudSyncLoopControl, CloudTransport, CloudTransportError,
    CommandEnvelope, CommandResult, CommandState, ControlAction, DeviceAdapter, DeviceClass,
    DeviceDescriptor, DeviceId, EdgeEngine, EdgeStore, EngineConfig, FacilityId, HealthReport,
    RecoveryOutcome, SafetyConfirmation, TenantId,
};

struct SuccessfulScale {
    descriptor: DeviceDescriptor,
}

impl DeviceAdapter for SuccessfulScale {
    fn descriptor(&self) -> &DeviceDescriptor {
        &self.descriptor
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::manual_only()
    }

    fn heartbeat(&mut self) -> Result<HealthReport, AdapterFailure> {
        Ok(HealthReport::healthy())
    }

    fn execute(&mut self, _envelope: &CommandEnvelope) -> Result<CommandResult, AdapterFailure> {
        Ok(CommandResult::Scale(ScaleResult {
            mass_milligrams: 0,
            stable: true,
        }))
    }

    fn recover(
        &mut self,
        _envelope: &CommandEnvelope,
    ) -> Result<RecoveryOutcome<CommandResult>, AdapterFailure> {
        Ok(RecoveryOutcome::NotFound)
    }
}

struct FakeCloud {
    device: AutomationDeviceResponse,
    delivery: AutomationCommandDeliveryResponse,
    delivery_returned: bool,
    fail_first_ack: bool,
    acknowledgements: usize,
    reports: usize,
    heartbeats: usize,
    last_heartbeat_control: Option<AutomationControlMode>,
}

impl CloudTransport for FakeCloud {
    fn assigned_devices(
        &mut self,
        facility_id: i64,
    ) -> Result<Vec<AutomationDeviceResponse>, CloudTransportError> {
        assert_eq!(facility_id, self.device.facility_id);
        Ok(vec![self.device.clone()])
    }

    fn pull_commands(
        &mut self,
        request: &PullAutomationCommandsRequest,
        _idempotency_key: &str,
    ) -> Result<AutomationCommandDeliveryPage, CloudTransportError> {
        assert_eq!(request.facility_id, self.device.facility_id);
        if self.delivery_returned {
            Ok(AutomationCommandDeliveryPage { items: Vec::new() })
        } else {
            self.delivery_returned = true;
            Ok(AutomationCommandDeliveryPage {
                items: vec![self.delivery.clone()],
            })
        }
    }

    fn acknowledge_command(
        &mut self,
        command_id: i64,
        request: &AcknowledgeAutomationCommandRequest,
        _idempotency_key: &str,
    ) -> Result<AutomationCommandResponse, CloudTransportError> {
        assert_eq!(command_id, self.delivery.command.command_id);
        assert_eq!(request.delivery_token, self.delivery.delivery_token);
        self.acknowledgements += 1;
        if self.fail_first_ack {
            self.fail_first_ack = false;
            return Err(CloudTransportError::InvalidPath(
                "simulated acknowledgement loss".into(),
            ));
        }
        let mut command = self.delivery.command.clone();
        command.status = AutomationCommandStatus::Accepted;
        command.revision = Revision::new(3).unwrap();
        command.accepted_at = Some(fixed_time().to_rfc3339());
        Ok(command)
    }

    fn report_command(
        &mut self,
        command_id: i64,
        request: &ReportAutomationCommandRequest,
        _idempotency_key: &str,
    ) -> Result<AutomationCommandResponse, CloudTransportError> {
        assert_eq!(command_id, self.delivery.command.command_id);
        self.reports += 1;
        let mut command = self.delivery.command.clone();
        command.status = request.status;
        command.revision = Revision::new(4).unwrap();
        command.accepted_at = Some(fixed_time().to_rfc3339());
        command.completed_at = Some(request.occurred_at.clone());
        command.result = request.result.clone();
        command.error_code = request.error_code.clone();
        command.error_message = request.error_message.clone();
        Ok(command)
    }

    fn record_heartbeat(
        &mut self,
        device_id: i64,
        request: &RecordAutomationHeartbeatRequest,
        _idempotency_key: &str,
    ) -> Result<AutomationHeartbeatResponse, CloudTransportError> {
        assert_eq!(device_id, self.device.device_id);
        self.heartbeats += 1;
        self.last_heartbeat_control = Some(request.control_mode);
        Ok(AutomationHeartbeatResponse {
            heartbeat_id: i64::try_from(self.heartbeats).unwrap(),
            device_id,
            service_account_id: 5,
            agent_instance: request.agent_instance.clone(),
            health: request.health,
            control_mode: request.control_mode,
            message: request.message.clone(),
            queued_commands: request.queued_commands,
            manual_review_commands: request.manual_review_commands,
            observed_at: request.observed_at.clone(),
            received_at: request.observed_at.clone(),
        })
    }
}

#[test]
fn cloud_delivery_is_durable_before_ack_and_recovers_after_transport_loss() {
    let now = fixed_time();
    let actor = ActorId::new("edge-agent-a").unwrap();
    let descriptor = DeviceDescriptor {
        tenant_id: TenantId::new("9").unwrap(),
        facility_id: FacilityId::new("7").unwrap(),
        device_id: DeviceId::new("scale-01").unwrap(),
        class: DeviceClass::Scale,
        display_name: "Scale 01".into(),
    };
    let mut store = EdgeStore::open_in_memory().unwrap();
    store
        .register_device(descriptor.clone(), &actor, "commissioned", now)
        .unwrap();
    store
        .change_control_mode(
            &descriptor.device_id,
            ControlAction::ResumeAutomation(SafetyConfirmation::after_physical_safety_checklist()),
            &actor,
            "local safety checklist complete",
            now,
        )
        .unwrap();
    let mut adapters = AdapterRegistry::default();
    adapters
        .register(SuccessfulScale {
            descriptor: descriptor.clone(),
        })
        .unwrap();
    let mut engine = EdgeEngine::new(
        store,
        adapters,
        actor.clone(),
        EngineConfig {
            batch_size: 10,
            ..EngineConfig::default()
        },
    )
    .unwrap();
    let mut cloud = fake_cloud();
    let config = CloudSyncConfig {
        tenant_id: 9,
        facility_id: 7,
        agent_instance: "edge-host-a/boot-1".into(),
        pull_limit: 10,
        sync_batch_size: 10,
    };

    let first = CloudSync::new(&mut cloud, &mut engine, actor.clone(), config.clone())
        .unwrap()
        .run_once(now);
    assert!(first.is_err());
    assert_eq!(
        engine.store().command("cloud:41").unwrap().state,
        CommandState::Queued
    );
    let durable = engine.store().cloud_delivery(41).unwrap();
    assert!(durable.acknowledgement_revision.is_none());
    assert_eq!(cloud.reports, 0);

    let summary = CloudSync::new(&mut cloud, &mut engine, actor, config)
        .unwrap()
        .run_once(now + chrono::Duration::seconds(1))
        .unwrap();
    assert_eq!(summary.acknowledgements_published, 1);
    assert_eq!(summary.commands_executed, 1);
    assert_eq!(summary.reports_published, 1);
    assert_eq!(
        engine.store().command("cloud:41").unwrap().state,
        CommandState::Succeeded
    );
    let durable = engine.store().cloud_delivery(41).unwrap();
    assert_eq!(durable.acknowledgement_revision, Some(3));
    assert_eq!(durable.reported_revision, Some(4));
    assert_eq!(durable.reported_status.as_deref(), Some("succeeded"));
    assert_eq!(cloud.acknowledgements, 2);
    assert_eq!(cloud.reports, 1);
    assert_eq!(cloud.heartbeats, 2);

    cloud.device.control_mode = AutomationControlMode::Disabled;
    cloud.device.control_reason = "cloud emergency stop".into();
    let restricted = CloudSync::new(
        &mut cloud,
        &mut engine,
        ActorId::new("edge-agent-a").unwrap(),
        CloudSyncConfig {
            tenant_id: 9,
            facility_id: 7,
            agent_instance: "edge-host-a/boot-1".into(),
            pull_limit: 10,
            sync_batch_size: 10,
        },
    )
    .unwrap()
    .run_once(now + chrono::Duration::seconds(2))
    .unwrap();
    assert_eq!(restricted.controls_restricted, 1);
    assert_eq!(
        engine
            .store()
            .device_status(&descriptor.device_id)
            .unwrap()
            .control_mode,
        wareboxes_edge_agent::ControlMode::Disabled
    );
    assert_eq!(
        cloud.last_heartbeat_control,
        Some(AutomationControlMode::Disabled)
    );

    let mut cycles = 0_u8;
    CloudSync::new(
        &mut cloud,
        &mut engine,
        ActorId::new("edge-agent-a").unwrap(),
        CloudSyncConfig {
            tenant_id: 9,
            facility_id: 7,
            agent_instance: "edge-host-a/boot-1".into(),
            pull_limit: 10,
            sync_batch_size: 10,
        },
    )
    .unwrap()
    .run_until_shutdown(
        CloudSyncLoopConfig {
            poll_interval: Duration::from_millis(100),
            error_backoff: Duration::from_millis(100),
        },
        |_| {
            cycles += 1;
            CloudSyncLoopControl::Shutdown
        },
    )
    .unwrap();
    assert_eq!(cycles, 1);
}

fn fake_cloud() -> FakeCloud {
    let device = AutomationDeviceResponse {
        device_id: 12,
        facility_id: 7,
        device_key: "scale-01".into(),
        class: AutomationDeviceClass::Scale,
        display_name: "Scale 01".into(),
        control_mode: AutomationControlMode::Automatic,
        control_reason: "cloud safety approval".into(),
        control_changed_by: 3,
        control_changed_at: fixed_time().to_rfc3339(),
        revision: Revision::new(2).unwrap(),
        health: AutomationHealthState::Healthy,
        health_message: None,
        last_heartbeat_at: Some(fixed_time().to_rfc3339()),
        registered_by: 3,
        registered_at: fixed_time().to_rfc3339(),
    };
    let command = AutomationCommandResponse {
        command_id: 41,
        facility_id: 7,
        device_id: 12,
        device_key: "scale-01".into(),
        device_class: AutomationDeviceClass::Scale,
        correlation_id: "pack carton 100 weight".into(),
        recovery_policy: AutomationRecoveryPolicy::ManualReview,
        command: AutomationDeviceCommand::Scale(AutomationScaleCommand::Tare),
        packing_scale_context: None,
        shipping_document_print_context: None,
        status: AutomationCommandStatus::Delivered,
        revision: Revision::new(2).unwrap(),
        delivery_attempts: 1,
        assigned_service_account_id: Some(5),
        agent_instance: Some("edge-host-a/boot-1".into()),
        delivered_at: Some(fixed_time().to_rfc3339()),
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
        requested_at: fixed_time().to_rfc3339(),
    };
    FakeCloud {
        device,
        delivery: AutomationCommandDeliveryResponse {
            command,
            delivery_token: "D".repeat(48),
            delivery_expires_at: (fixed_time() + chrono::Duration::seconds(30)).to_rfc3339(),
        },
        delivery_returned: false,
        fail_first_ack: true,
        acknowledgements: 0,
        reports: 0,
        heartbeats: 0,
        last_heartbeat_control: None,
    }
}

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-16T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
use std::time::Duration;
