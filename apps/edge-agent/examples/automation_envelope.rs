use std::env;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use chrono::{DateTime, TimeDelta, Utc};
use tempfile::TempDir;
use wareboxes_edge_agent::adapter::{AdapterRegistry, DeviceAdapter};
use wareboxes_edge_agent::command::{ScaleCommand, ScaleResult, COMMAND_SCHEMA_VERSION};
use wareboxes_edge_agent::{
    ActorId, AdapterCapabilities, AdapterFailure, CommandEnvelope, CommandId, CommandRequest,
    CommandResult, CommandState, ControlAction, CorrelationId, DeviceClass, DeviceCommand,
    DeviceDescriptor, DeviceId, EdgeEngine, EdgeStore, EngineConfig, FacilityId, HealthReport,
    IdempotencyKey, RecoveryOutcome, RecoveryPolicy, SafetyConfirmation, TenantId,
};

const SUCCESS_DEVICE: &str = "scale-throughput";
const RECOVERY_DEVICE: &str = "scale-recovery";

#[derive(Clone, Copy)]
enum AdapterBehavior {
    Succeed,
    AmbiguousThenRecover,
}

struct EnvelopeAdapter {
    descriptor: DeviceDescriptor,
    behavior: AdapterBehavior,
}

impl DeviceAdapter for EnvelopeAdapter {
    fn descriptor(&self) -> &DeviceDescriptor {
        &self.descriptor
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            device_side_duplicate_protection: true,
            recovery_probe: true,
        }
    }

    fn heartbeat(&mut self) -> Result<HealthReport, AdapterFailure> {
        Ok(HealthReport::healthy())
    }

    fn execute(&mut self, _envelope: &CommandEnvelope) -> Result<CommandResult, AdapterFailure> {
        match self.behavior {
            AdapterBehavior::Succeed => Ok(scale_result()),
            AdapterBehavior::AmbiguousThenRecover => Err(AdapterFailure::ambiguous(
                "simulated transport acknowledgement loss",
            )),
        }
    }

    fn recover(
        &mut self,
        _envelope: &CommandEnvelope,
    ) -> Result<RecoveryOutcome<CommandResult>, AdapterFailure> {
        match self.behavior {
            AdapterBehavior::Succeed => Ok(RecoveryOutcome::NotFound),
            AdapterBehavior::AmbiguousThenRecover => Ok(RecoveryOutcome::Completed(scale_result())),
        }
    }
}

#[derive(Clone, Copy)]
struct Budget {
    minimum_per_second: f64,
    p99: Duration,
}

struct PhaseResult {
    elapsed: Duration,
    durations: Vec<Duration>,
}

fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let directory = TempDir::new().context("creating automation-envelope directory")?;
    let store_path = directory.path().join("edge.sqlite3");
    let now = DateTime::parse_from_rfc3339("2026-08-15T12:00:00Z")
        .context("parsing fixed envelope timestamp")?
        .with_timezone(&Utc);
    let actor = ActorId::new("automation-envelope")?;
    let tenant_id = TenantId::new("tenant-envelope")?;
    let facility_id = FacilityId::new("facility-envelope")?;

    let success = descriptor(&tenant_id, &facility_id, SUCCESS_DEVICE)?;
    let recovery = descriptor(&tenant_id, &facility_id, RECOVERY_DEVICE)?;
    let mut store = EdgeStore::open(&store_path)?;
    enable_device(&mut store, success.clone(), &actor, now)?;
    enable_device(&mut store, recovery.clone(), &actor, now)?;

    let mut registry = AdapterRegistry::default();
    registry.register(EnvelopeAdapter {
        descriptor: success.clone(),
        behavior: AdapterBehavior::Succeed,
    })?;
    registry.register(EnvelopeAdapter {
        descriptor: recovery.clone(),
        behavior: AdapterBehavior::AmbiguousThenRecover,
    })?;
    let mut engine = EdgeEngine::new(
        store,
        registry,
        actor.clone(),
        EngineConfig {
            lease: Duration::from_secs(5),
            retry_delay: Duration::from_millis(1),
            retry_delay_cap: Duration::from_millis(10),
            max_attempts: 3,
            max_recovery_probes: 3,
            batch_size: 1_000,
        },
    )?;

    let success_requests = requests(
        &tenant_id,
        &facility_id,
        &success.device_id,
        "success",
        config.success_commands,
        RecoveryPolicy::DeviceDeduplicatedReplay,
    )?;
    let recovery_requests = requests(
        &tenant_id,
        &facility_id,
        &recovery.device_id,
        "recovery",
        config.recovery_commands,
        RecoveryPolicy::ProbeThenRetry,
    )?;

    let submission = submit_all(
        &mut engine,
        success_requests.iter().chain(&recovery_requests),
        now,
        false,
    )?;
    enforce(
        "durable_submission",
        success_requests.len() + recovery_requests.len(),
        &submission,
        config.submission_budget,
    )?;

    let execution_started = Instant::now();
    let mut claimed = 0_u64;
    let mut succeeded = 0_u64;
    let mut ambiguous = 0_u64;
    loop {
        let summary = engine.run_once(now)?;
        claimed += summary.claimed;
        succeeded += summary.succeeded;
        ambiguous += summary.ambiguous_outcomes;
        if summary.claimed == 0 {
            break;
        }
    }
    let execution_elapsed = execution_started.elapsed();
    let expected_initial = u64::try_from(config.success_commands + config.recovery_commands)
        .context("initial command count does not fit u64")?;
    if claimed != expected_initial
        || succeeded != u64::try_from(config.success_commands)?
        || ambiguous != u64::try_from(config.recovery_commands)?
    {
        bail!(
            "initial device execution did not reconcile: claimed={claimed}, succeeded={succeeded}, ambiguous={ambiguous}"
        );
    }
    enforce_throughput(
        "device_execution",
        usize::try_from(claimed)?,
        execution_elapsed,
        config.execution_minimum_per_second,
    )?;

    let recovery_at = now
        .checked_add_signed(TimeDelta::seconds(1))
        .context("recovery timestamp overflowed")?;
    let recovery_started = Instant::now();
    let mut recovered = 0_u64;
    loop {
        let summary = engine.run_once(recovery_at)?;
        recovered += summary.succeeded;
        if summary.claimed == 0 {
            break;
        }
    }
    let recovery_elapsed = recovery_started.elapsed();
    if recovered != u64::try_from(config.recovery_commands)? {
        bail!(
            "ambiguous recovery did not reconcile: recovered={recovered}, expected={}",
            config.recovery_commands
        );
    }
    enforce_recovery(
        config.recovery_commands,
        recovery_elapsed,
        config.recovery_maximum,
    )?;

    let replay = submit_all(&mut engine, success_requests.iter(), recovery_at, true)?;
    enforce(
        "exact_replay",
        success_requests.len(),
        &replay,
        config.replay_budget,
    )?;

    assert_completed(&engine, success_requests.iter().chain(&recovery_requests))?;
    assert_manual_fallback(&mut engine, &success.device_id, &actor, recovery_at)?;

    println!(
        "event=automation_envelope_passed commands={} recovered={} submission_seconds={:.3} execution_seconds={:.3} recovery_seconds={:.3} replay_seconds={:.3}",
        config.success_commands + config.recovery_commands,
        config.recovery_commands,
        submission.elapsed.as_secs_f64(),
        execution_elapsed.as_secs_f64(),
        recovery_elapsed.as_secs_f64(),
        replay.elapsed.as_secs_f64(),
    );
    Ok(())
}

fn descriptor(
    tenant_id: &TenantId,
    facility_id: &FacilityId,
    device_id: &str,
) -> anyhow::Result<DeviceDescriptor> {
    Ok(DeviceDescriptor {
        tenant_id: tenant_id.clone(),
        facility_id: facility_id.clone(),
        device_id: DeviceId::new(device_id)?,
        class: DeviceClass::Scale,
        display_name: device_id.replace('-', " "),
    })
}

fn enable_device(
    store: &mut EdgeStore,
    descriptor: DeviceDescriptor,
    actor: &ActorId,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let device_id = descriptor.device_id.clone();
    store.register_device(descriptor, actor, "automation envelope registration", now)?;
    store.change_control_mode(
        &device_id,
        ControlAction::ResumeAutomation(SafetyConfirmation::after_physical_safety_checklist()),
        actor,
        "isolated envelope safety checklist complete",
        now,
    )?;
    Ok(())
}

fn requests(
    tenant_id: &TenantId,
    facility_id: &FacilityId,
    device_id: &DeviceId,
    prefix: &str,
    count: usize,
    recovery_policy: RecoveryPolicy,
) -> anyhow::Result<Vec<CommandRequest>> {
    (0..count)
        .map(|index| {
            Ok(CommandRequest {
                schema_version: COMMAND_SCHEMA_VERSION,
                command_id: CommandId::new(format!("{prefix}-command-{index}"))?,
                tenant_id: tenant_id.clone(),
                facility_id: facility_id.clone(),
                device_id: device_id.clone(),
                correlation_id: CorrelationId::new(format!("{prefix}-correlation-{index}"))?,
                idempotency_key: IdempotencyKey::new(format!("{prefix}-idempotency-{index}"))?,
                recovery_policy,
                command: DeviceCommand::Scale(ScaleCommand::Tare),
            })
        })
        .collect()
}

fn submit_all<'a>(
    engine: &mut EdgeEngine,
    requests: impl Iterator<Item = &'a CommandRequest>,
    now: DateTime<Utc>,
    expect_replay: bool,
) -> anyhow::Result<PhaseResult> {
    let phase_started = Instant::now();
    let mut durations = Vec::new();
    for request in requests {
        let started = Instant::now();
        let outcome = engine.submit(request.clone(), now)?;
        durations.push(started.elapsed());
        if outcome.is_replay() != expect_replay {
            bail!("durable command replay state did not match the phase contract");
        }
    }
    Ok(PhaseResult {
        elapsed: phase_started.elapsed(),
        durations,
    })
}

fn assert_completed<'a>(
    engine: &EdgeEngine,
    requests: impl Iterator<Item = &'a CommandRequest>,
) -> anyhow::Result<()> {
    for request in requests {
        if engine.store().command(request.command_id.as_str())?.state != CommandState::Succeeded {
            bail!(
                "durable command {} did not finish in the succeeded state",
                request.command_id
            );
        }
    }
    Ok(())
}

fn assert_manual_fallback(
    engine: &mut EdgeEngine,
    device_id: &DeviceId,
    actor: &ActorId,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    engine.store_mut().change_control_mode(
        device_id,
        ControlAction::Disable,
        actor,
        "automation envelope manual fallback check",
        now,
    )?;
    let quarantined = CommandRequest {
        schema_version: COMMAND_SCHEMA_VERSION,
        command_id: CommandId::new("manual-fallback-command")?,
        tenant_id: TenantId::new("tenant-envelope")?,
        facility_id: FacilityId::new("facility-envelope")?,
        device_id: device_id.clone(),
        correlation_id: CorrelationId::new("manual-fallback-correlation")?,
        idempotency_key: IdempotencyKey::new("manual-fallback-idempotency")?,
        recovery_policy: RecoveryPolicy::ManualReview,
        command: DeviceCommand::Scale(ScaleCommand::Tare),
    };
    let outcome = engine.submit(quarantined, now)?;
    if outcome.record().state != CommandState::ManualReview {
        bail!("disabled automation did not quarantine new work for manual review");
    }
    Ok(())
}

fn scale_result() -> CommandResult {
    CommandResult::Scale(ScaleResult {
        mass_milligrams: 42_000,
        stable: true,
    })
}

fn enforce(name: &str, count: usize, result: &PhaseResult, budget: Budget) -> anyhow::Result<()> {
    enforce_throughput(name, count, result.elapsed, budget.minimum_per_second)?;
    let p99 = percentile(&result.durations, 99)?;
    if p99 > budget.p99 {
        bail!(
            "{name} p99 {:.1}ms exceeded {:.1}ms",
            p99.as_secs_f64() * 1_000.0,
            budget.p99.as_secs_f64() * 1_000.0
        );
    }
    println!(
        "event=automation_phase_completed phase={name} commands={count} seconds={:.3} commands_per_second={:.1} p99_millis={:.1}",
        result.elapsed.as_secs_f64(),
        rate(count, result.elapsed),
        p99.as_secs_f64() * 1_000.0
    );
    Ok(())
}

fn enforce_throughput(
    name: &str,
    count: usize,
    elapsed: Duration,
    minimum_per_second: f64,
) -> anyhow::Result<()> {
    let observed = rate(count, elapsed);
    if observed < minimum_per_second {
        bail!("{name} throughput {observed:.1}/s was below {minimum_per_second:.1}/s");
    }
    println!(
        "event=automation_throughput_completed phase={name} commands={count} seconds={:.3} commands_per_second={observed:.1}",
        elapsed.as_secs_f64()
    );
    Ok(())
}

fn enforce_recovery(count: usize, elapsed: Duration, maximum: Duration) -> anyhow::Result<()> {
    if elapsed > maximum {
        bail!(
            "ambiguous recovery {:.3}s exceeded {:.3}s",
            elapsed.as_secs_f64(),
            maximum.as_secs_f64()
        );
    }
    println!(
        "event=automation_recovery_completed commands={count} seconds={:.3}",
        elapsed.as_secs_f64()
    );
    Ok(())
}

fn rate(count: usize, elapsed: Duration) -> f64 {
    count as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
}

fn percentile(values: &[Duration], percentile: usize) -> anyhow::Result<Duration> {
    if values.is_empty() || !(1..=100).contains(&percentile) {
        bail!("invalid percentile input");
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    let rank = (values.len() * percentile).div_ceil(100).saturating_sub(1);
    Ok(values[rank])
}

struct Config {
    success_commands: usize,
    recovery_commands: usize,
    submission_budget: Budget,
    execution_minimum_per_second: f64,
    recovery_maximum: Duration,
    replay_budget: Budget,
}

impl Config {
    fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            success_commands: integer_env("AUTOMATION_SUCCESS_COMMANDS", 10_000, 1, 100_000)?,
            recovery_commands: integer_env("AUTOMATION_RECOVERY_COMMANDS", 1_000, 1, 10_000)?,
            submission_budget: Budget {
                minimum_per_second: integer_env(
                    "AUTOMATION_SUBMISSION_MIN_PER_SECOND",
                    100,
                    1,
                    100_000,
                )? as f64,
                p99: millis_env("AUTOMATION_SUBMISSION_P99_MILLIS", 100)?,
            },
            execution_minimum_per_second: integer_env(
                "AUTOMATION_EXECUTION_MIN_PER_SECOND",
                75,
                1,
                100_000,
            )? as f64,
            recovery_maximum: millis_env("AUTOMATION_RECOVERY_MAX_MILLIS", 5_000)?,
            replay_budget: Budget {
                minimum_per_second: integer_env(
                    "AUTOMATION_REPLAY_MIN_PER_SECOND",
                    200,
                    1,
                    100_000,
                )? as f64,
                p99: millis_env("AUTOMATION_REPLAY_P99_MILLIS", 50)?,
            },
        })
    }
}

fn integer_env(
    name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> anyhow::Result<usize> {
    let value = env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("{name} must be an integer"))
        })
        .transpose()?
        .unwrap_or(default);
    if !(minimum..=maximum).contains(&value) {
        bail!("{name} must be between {minimum} and {maximum}");
    }
    Ok(value)
}

fn millis_env(name: &str, default: usize) -> anyhow::Result<Duration> {
    let millis = integer_env(name, default, 1, 600_000)?;
    Ok(Duration::from_millis(u64::try_from(millis)?))
}
