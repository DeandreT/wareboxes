use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use tempfile::TempDir;
use wareboxes_edge_agent::adapter::{AdapterRegistry, DeviceAdapter};
use wareboxes_edge_agent::command::{ScaleCommand, ScaleResult, COMMAND_SCHEMA_VERSION};
use wareboxes_edge_agent::{
    ActorId, AdapterCapabilities, AdapterFailure, CommandEnvelope, CommandRequest, CommandResult,
    CommandState, ControlAction, ControlMode, CorrelationId, DeviceClass, DeviceCommand,
    DeviceDescriptor, DeviceId, EdgeEngine, EdgeStore, EngineConfig, FacilityId, HealthReport,
    IdempotencyKey, RecoveryOutcome, RecoveryPolicy, SafetyConfirmation, TenantId,
};

#[derive(Debug)]
enum Step {
    Result(CommandResult),
    Failure(AdapterFailure),
    Panic,
}

#[derive(Debug)]
enum RecoveryStep {
    Outcome(RecoveryOutcome<CommandResult>),
    Failure(AdapterFailure),
}

#[derive(Debug, Default)]
struct Script {
    executions: VecDeque<Step>,
    recoveries: VecDeque<RecoveryStep>,
    seen_command_ids: Vec<String>,
    seen_correlations: Vec<String>,
}

struct ScriptedAdapter {
    descriptor: DeviceDescriptor,
    capabilities: AdapterCapabilities,
    health: HealthReport,
    script: Arc<Mutex<Script>>,
}

impl DeviceAdapter for ScriptedAdapter {
    fn descriptor(&self) -> &DeviceDescriptor {
        &self.descriptor
    }

    fn capabilities(&self) -> AdapterCapabilities {
        self.capabilities
    }

    fn heartbeat(&mut self) -> Result<HealthReport, AdapterFailure> {
        Ok(self.health.clone())
    }

    fn execute(&mut self, envelope: &CommandEnvelope) -> Result<CommandResult, AdapterFailure> {
        let mut script = self.script.lock().expect("script lock should be available");
        script
            .seen_command_ids
            .push(envelope.request.command_id.to_string());
        script
            .seen_correlations
            .push(envelope.request.correlation_id.to_string());
        match script
            .executions
            .pop_front()
            .expect("test adapter should have an execution step")
        {
            Step::Result(result) => Ok(result),
            Step::Failure(failure) => Err(failure),
            Step::Panic => panic!("simulated edge-agent process crash"),
        }
    }

    fn recover(
        &mut self,
        envelope: &CommandEnvelope,
    ) -> Result<RecoveryOutcome<CommandResult>, AdapterFailure> {
        let mut script = self.script.lock().expect("script lock should be available");
        script
            .seen_command_ids
            .push(envelope.request.command_id.to_string());
        script
            .seen_correlations
            .push(envelope.request.correlation_id.to_string());
        match script
            .recoveries
            .pop_front()
            .expect("test adapter should have a recovery step")
        {
            RecoveryStep::Outcome(outcome) => Ok(outcome),
            RecoveryStep::Failure(failure) => Err(failure),
        }
    }
}

fn at(second: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-15T12:00:00Z")
        .expect("fixed timestamp should parse")
        .with_timezone(&Utc)
        + TimeDelta::seconds(second)
}

fn descriptor() -> DeviceDescriptor {
    DeviceDescriptor {
        tenant_id: TenantId::new("tenant-1").expect("tenant ID should be valid"),
        facility_id: FacilityId::new("facility-1").expect("facility ID should be valid"),
        device_id: DeviceId::new("scale-1").expect("device ID should be valid"),
        class: DeviceClass::Scale,
        display_name: "Pack scale 1".into(),
    }
}

fn actor() -> ActorId {
    ActorId::new("operator-1").expect("actor ID should be valid")
}

fn command(key: &str, policy: RecoveryPolicy) -> CommandRequest {
    CommandRequest {
        schema_version: COMMAND_SCHEMA_VERSION,
        command_id: wareboxes_edge_agent::types::CommandId::new(format!("command-{key}"))
            .expect("command ID should be valid"),
        tenant_id: descriptor().tenant_id,
        facility_id: descriptor().facility_id,
        device_id: descriptor().device_id,
        correlation_id: CorrelationId::new(format!("correlation-{key}"))
            .expect("correlation ID should be valid"),
        idempotency_key: IdempotencyKey::new(key).expect("idempotency key should be valid"),
        recovery_policy: policy,
        command: DeviceCommand::Scale(ScaleCommand::Tare),
    }
}

fn result(mass_milligrams: i64) -> CommandResult {
    CommandResult::Scale(ScaleResult {
        mass_milligrams,
        stable: true,
    })
}

fn store_path(directory: &TempDir) -> std::path::PathBuf {
    directory.path().join("edge.sqlite3")
}

fn configure_store(path: &std::path::Path) -> EdgeStore {
    let mut store = EdgeStore::open(path).expect("edge store should open");
    store
        .register_device(descriptor(), &actor(), "initial safe registration", at(0))
        .expect("device should register");
    store
        .change_control_mode(
            &descriptor().device_id,
            ControlAction::ResumeAutomation(SafetyConfirmation::after_physical_safety_checklist()),
            &actor(),
            "commissioning checklist complete",
            at(0),
        )
        .expect("automation should enable");
    store
}

fn engine(
    store: EdgeStore,
    script: Arc<Mutex<Script>>,
    capabilities: AdapterCapabilities,
) -> EdgeEngine {
    engine_with_health(store, script, capabilities, HealthReport::healthy())
}

fn engine_with_health(
    store: EdgeStore,
    script: Arc<Mutex<Script>>,
    capabilities: AdapterCapabilities,
    health: HealthReport,
) -> EdgeEngine {
    let mut registry = AdapterRegistry::default();
    registry
        .register(ScriptedAdapter {
            descriptor: descriptor(),
            capabilities,
            health,
            script,
        })
        .expect("adapter should register");
    EdgeEngine::new(
        store,
        registry,
        ActorId::new("edge-agent-1").expect("agent ID should be valid"),
        EngineConfig {
            lease: Duration::from_secs(5),
            retry_delay: Duration::from_secs(1),
            retry_delay_cap: Duration::from_secs(4),
            max_attempts: 3,
            max_recovery_probes: 3,
            batch_size: 20,
        },
    )
    .expect("engine should configure")
}

#[test]
fn successful_delivery_preserves_identity_result_health_and_audit_across_restart() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let script = Arc::new(Mutex::new(Script {
        executions: VecDeque::from([Step::Result(result(42_000))]),
        ..Script::default()
    }));
    let mut engine = engine(
        configure_store(&store_path(&directory)),
        Arc::clone(&script),
        AdapterCapabilities {
            device_side_duplicate_protection: true,
            recovery_probe: true,
        },
    );
    let request = command("success-1", RecoveryPolicy::DeviceDeduplicatedReplay);
    assert!(!engine.submit(request.clone(), at(1)).unwrap().is_replay());
    assert!(engine.submit(request, at(1)).unwrap().is_replay());

    let summary = engine.run_once(at(2)).expect("delivery should run");
    assert_eq!(summary.claimed, 1);
    assert_eq!(summary.succeeded, 1);
    let stored = engine
        .store()
        .command("command-success-1")
        .expect("command should load");
    assert_eq!(stored.state, CommandState::Succeeded);
    assert_eq!(stored.result, Some(result(42_000)));
    assert_eq!(
        engine.store().attempts("command-success-1").unwrap().len(),
        1
    );
    assert_eq!(
        engine
            .store()
            .heartbeat_events(&descriptor().device_id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        script.lock().unwrap().seen_command_ids,
        ["command-success-1"]
    );
    drop(engine);

    let reopened = EdgeStore::open(store_path(&directory)).expect("store should reopen");
    assert_eq!(
        reopened.command("command-success-1").unwrap().state,
        CommandState::Succeeded
    );
}

#[test]
fn retryable_failure_uses_bounded_backoff_and_preserves_stable_correlation() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let script = Arc::new(Mutex::new(Script {
        executions: VecDeque::from([
            Step::Failure(AdapterFailure::retryable("controller is busy")),
            Step::Result(result(7_500)),
        ]),
        ..Script::default()
    }));
    let mut engine = engine(
        configure_store(&store_path(&directory)),
        Arc::clone(&script),
        AdapterCapabilities {
            device_side_duplicate_protection: true,
            recovery_probe: false,
        },
    );
    engine
        .submit(
            command("retry-1", RecoveryPolicy::DeviceDeduplicatedReplay),
            at(1),
        )
        .unwrap();
    let first = engine.run_once(at(2)).unwrap();
    assert_eq!(first.retryable_failures, 1);
    assert_eq!(
        engine.store().command("command-retry-1").unwrap().state,
        CommandState::RetryWait
    );
    assert_eq!(engine.run_once(at(2)).unwrap().claimed, 0);
    assert_eq!(engine.run_once(at(3)).unwrap().succeeded, 1);

    let attempts = engine.store().attempts("command-retry-1").unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].state, "retryable_failure");
    assert_eq!(attempts[1].state, "succeeded");
    let seen = script.lock().unwrap();
    assert_eq!(
        seen.seen_command_ids,
        ["command-retry-1", "command-retry-1"]
    );
    assert_eq!(
        seen.seen_correlations,
        ["correlation-retry-1", "correlation-retry-1"]
    );
}

#[test]
fn crash_with_probe_policy_recovers_without_blind_reexecution() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let crash_script = Arc::new(Mutex::new(Script {
        executions: VecDeque::from([Step::Panic]),
        ..Script::default()
    }));
    let mut first_engine = engine(
        configure_store(&store_path(&directory)),
        crash_script,
        AdapterCapabilities {
            device_side_duplicate_protection: false,
            recovery_probe: true,
        },
    );
    first_engine
        .submit(command("crash-1", RecoveryPolicy::ProbeThenRetry), at(1))
        .unwrap();
    let crash = std::panic::catch_unwind(AssertUnwindSafe(|| first_engine.run_once(at(2))));
    assert!(crash.is_err());
    assert_eq!(
        first_engine
            .store()
            .command("command-crash-1")
            .unwrap()
            .state,
        CommandState::Executing
    );
    drop(first_engine);

    let recovery_script = Arc::new(Mutex::new(Script {
        recoveries: VecDeque::from([RecoveryStep::Outcome(RecoveryOutcome::Completed(result(
            18_000,
        )))]),
        ..Script::default()
    }));
    let mut recovered_engine = engine(
        EdgeStore::open(store_path(&directory)).expect("store should reopen"),
        Arc::clone(&recovery_script),
        AdapterCapabilities {
            device_side_duplicate_protection: false,
            recovery_probe: true,
        },
    );
    let summary = recovered_engine.run_once(at(8)).unwrap();
    assert_eq!(summary.recovered_leases, 1);
    assert_eq!(summary.recovery_probes, 1);
    assert_eq!(summary.succeeded, 1);
    assert_eq!(
        recovered_engine
            .store()
            .command("command-crash-1")
            .unwrap()
            .state,
        CommandState::Succeeded
    );
    let attempts = recovered_engine
        .store()
        .attempts("command-crash-1")
        .unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].state, "abandoned");
    assert_eq!(attempts[1].kind, "recovery_probe");
    assert!(recovery_script.lock().unwrap().executions.is_empty());
}

#[test]
fn ambiguous_manual_policy_disables_automation_until_explicit_resolution() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let script = Arc::new(Mutex::new(Script {
        executions: VecDeque::from([Step::Failure(AdapterFailure::ambiguous(
            "controller disconnected after accepting the command",
        ))]),
        ..Script::default()
    }));
    let mut engine = engine(
        configure_store(&store_path(&directory)),
        script,
        AdapterCapabilities::manual_only(),
    );
    engine
        .submit(command("manual-1", RecoveryPolicy::ManualReview), at(1))
        .unwrap();
    let summary = engine.run_once(at(2)).unwrap();
    assert_eq!(summary.manual_reviews, 1);
    assert_eq!(
        engine.store().command("command-manual-1").unwrap().state,
        CommandState::ManualReview
    );
    assert_eq!(
        engine
            .store()
            .device_status(&descriptor().device_id)
            .unwrap()
            .control_mode,
        ControlMode::ManualFallback
    );
    assert_eq!(engine.run_once(at(3)).unwrap().claimed, 0);

    let resolved = engine
        .store_mut()
        .resolve_manually(
            "command-manual-1",
            &actor(),
            "operator completed the tare on the backup scale",
            at(4),
        )
        .unwrap();
    assert_eq!(resolved.state, CommandState::ResolvedManually);
}

#[test]
fn engine_rejects_replay_policy_without_downstream_duplicate_protection() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let mut engine = engine(
        configure_store(&store_path(&directory)),
        Arc::new(Mutex::new(Script::default())),
        AdapterCapabilities::manual_only(),
    );
    assert!(matches!(
        engine.submit(
            command("unsafe-1", RecoveryPolicy::DeviceDeduplicatedReplay),
            at(1)
        ),
        Err(wareboxes_edge_agent::engine::EngineError::UnsupportedRecoveryPolicy)
    ));
}

#[test]
fn concurrent_duplicate_submissions_create_one_durable_command() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    drop(configure_store(&store_path(&directory)));
    let mut threads = Vec::new();
    for _ in 0..8 {
        let path = store_path(&directory);
        threads.push(std::thread::spawn(move || {
            EdgeStore::open(path)
                .expect("store should open concurrently")
                .submit(command("race-1", RecoveryPolicy::ManualReview), at(1))
                .expect("duplicate submission should replay")
                .is_replay()
        }));
    }
    let replay_count = threads
        .into_iter()
        .map(|thread| thread.join().expect("submission thread should join"))
        .filter(|replayed| *replayed)
        .count();
    assert_eq!(replay_count, 7);
    assert_eq!(
        EdgeStore::open(store_path(&directory))
            .unwrap()
            .list_commands(10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn retryable_recovery_probe_never_blindly_reexecutes() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let script = Arc::new(Mutex::new(Script {
        executions: VecDeque::from([Step::Failure(AdapterFailure::ambiguous(
            "connection closed after dispatch",
        ))]),
        recoveries: VecDeque::from([
            RecoveryStep::Failure(AdapterFailure::retryable("controller unavailable")),
            RecoveryStep::Outcome(RecoveryOutcome::Completed(result(9_000))),
        ]),
        ..Script::default()
    }));
    let mut engine = engine(
        configure_store(&store_path(&directory)),
        Arc::clone(&script),
        AdapterCapabilities {
            device_side_duplicate_protection: false,
            recovery_probe: true,
        },
    );
    engine
        .submit(
            command("probe-retry-1", RecoveryPolicy::ProbeThenRetry),
            at(1),
        )
        .unwrap();
    engine.run_once(at(2)).unwrap();
    let first_probe = engine.run_once(at(3)).unwrap();
    assert_eq!(first_probe.recovery_probes, 1);
    assert_eq!(first_probe.retryable_failures, 1);
    assert_eq!(
        engine
            .store()
            .command("command-probe-retry-1")
            .unwrap()
            .state,
        CommandState::RecoveryWait
    );
    assert_eq!(engine.run_once(at(4)).unwrap().succeeded, 1);
    let script = script.lock().unwrap();
    assert!(script.executions.is_empty());
    assert!(script.recoveries.is_empty());
    assert_eq!(script.seen_command_ids.len(), 3);
}

#[test]
fn exhausted_recovery_probe_budget_enters_manual_fallback() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let script = Arc::new(Mutex::new(Script {
        executions: VecDeque::from([Step::Failure(AdapterFailure::ambiguous(
            "connection closed after dispatch",
        ))]),
        recoveries: VecDeque::from([
            RecoveryStep::Failure(AdapterFailure::retryable("controller unavailable")),
            RecoveryStep::Failure(AdapterFailure::retryable("controller unavailable")),
            RecoveryStep::Failure(AdapterFailure::retryable("controller unavailable")),
        ]),
        ..Script::default()
    }));
    let mut engine = engine(
        configure_store(&store_path(&directory)),
        Arc::clone(&script),
        AdapterCapabilities {
            device_side_duplicate_protection: false,
            recovery_probe: true,
        },
    );
    engine
        .submit(
            command("probe-budget-1", RecoveryPolicy::ProbeThenRetry),
            at(1),
        )
        .unwrap();
    engine.run_once(at(2)).unwrap();
    engine.run_once(at(3)).unwrap();
    engine.run_once(at(4)).unwrap();
    let exhausted = engine.run_once(at(6)).unwrap();
    assert_eq!(exhausted.manual_reviews, 1);
    assert_eq!(
        engine
            .store()
            .command("command-probe-budget-1")
            .unwrap()
            .state,
        CommandState::ManualReview
    );
    assert_eq!(
        engine
            .store()
            .device_status(&descriptor().device_id)
            .unwrap()
            .control_mode,
        ControlMode::ManualFallback
    );
    let script = script.lock().unwrap();
    assert!(script.executions.is_empty());
    assert!(script.recoveries.is_empty());
}

#[test]
fn not_found_probe_at_execution_limit_cannot_leave_a_queued_command_stranded() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let script = Arc::new(Mutex::new(Script {
        executions: VecDeque::from([
            Step::Failure(AdapterFailure::ambiguous("attempt 1 was ambiguous")),
            Step::Failure(AdapterFailure::ambiguous("attempt 2 was ambiguous")),
            Step::Failure(AdapterFailure::ambiguous("attempt 3 was ambiguous")),
        ]),
        recoveries: VecDeque::from([
            RecoveryStep::Outcome(RecoveryOutcome::NotFound),
            RecoveryStep::Outcome(RecoveryOutcome::NotFound),
            RecoveryStep::Outcome(RecoveryOutcome::NotFound),
        ]),
        ..Script::default()
    }));
    let mut engine = engine(
        configure_store(&store_path(&directory)),
        Arc::clone(&script),
        AdapterCapabilities {
            device_side_duplicate_protection: false,
            recovery_probe: true,
        },
    );
    engine
        .submit(
            command("probe-limit-1", RecoveryPolicy::ProbeThenRetry),
            at(1),
        )
        .unwrap();
    engine.run_once(at(2)).unwrap();
    engine.run_once(at(3)).unwrap();
    engine.run_once(at(5)).unwrap();
    let exhausted = engine.run_once(at(9)).unwrap();
    assert_eq!(exhausted.manual_reviews, 1);
    let record = engine.store().command("command-probe-limit-1").unwrap();
    assert_eq!(record.attempt_count, 3);
    assert_eq!(record.state, CommandState::ManualReview);
    let script = script.lock().unwrap();
    assert!(script.executions.is_empty());
    assert!(script.recoveries.is_empty());
}

#[test]
fn faulted_heartbeat_prevents_physical_execution_and_forces_manual_fallback() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let script = Arc::new(Mutex::new(Script::default()));
    let mut engine = engine_with_health(
        configure_store(&store_path(&directory)),
        Arc::clone(&script),
        AdapterCapabilities::manual_only(),
        HealthReport {
            state: wareboxes_edge_agent::HealthState::Faulted,
            message: Some("guard circuit open".into()),
            alarm_codes: vec!["GUARD_OPEN".into()],
        },
    );
    engine
        .submit(command("fault-1", RecoveryPolicy::ManualReview), at(1))
        .unwrap();
    let summary = engine.run_once(at(2)).unwrap();
    assert_eq!(summary.manual_reviews, 1);
    assert_eq!(
        engine.store().command("command-fault-1").unwrap().state,
        CommandState::ManualReview
    );
    assert!(script.lock().unwrap().seen_command_ids.is_empty());
}
