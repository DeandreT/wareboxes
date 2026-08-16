use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::adapter::{
    AdapterFailure, AdapterFailureClass, AdapterRegistry, RecoveryOutcome, RegistryError,
};
use crate::command::{CommandError, CommandRequest, RecoveryPolicy, SubmissionOutcome};
use crate::store::{ClaimKind, CloudDelivery, EdgeStore, RetryLimits, StoreError};
use crate::types::{ActorId, ControlMode, DeviceStatus};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub lease: Duration,
    pub retry_delay: Duration,
    pub retry_delay_cap: Duration,
    pub max_attempts: u32,
    pub max_recovery_probes: u32,
    pub batch_size: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            lease: Duration::from_secs(30),
            retry_delay: Duration::from_secs(2),
            retry_delay_cap: Duration::from_secs(120),
            max_attempts: 8,
            max_recovery_probes: 20,
            batch_size: 100,
        }
    }
}

impl EngineConfig {
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.lease.is_zero() {
            return Err(EngineError::InvalidConfig("lease must be positive"));
        }
        if self.retry_delay.is_zero() {
            return Err(EngineError::InvalidConfig("retry delay must be positive"));
        }
        if self.retry_delay_cap < self.retry_delay {
            return Err(EngineError::InvalidConfig(
                "retry delay cap must not be shorter than the base delay",
            ));
        }
        if self.max_attempts == 0 {
            return Err(EngineError::InvalidConfig(
                "maximum attempts must be positive",
            ));
        }
        if self.max_recovery_probes == 0 {
            return Err(EngineError::InvalidConfig(
                "maximum recovery probes must be positive",
            ));
        }
        if !(1..=1_000).contains(&self.batch_size) {
            return Err(EngineError::InvalidConfig(
                "batch size must be between 1 and 1,000",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error("invalid edge engine configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("device {0} has no registered adapter")]
    AdapterNotRegistered(String),
    #[error("registered adapter does not match the durable device configuration")]
    AdapterConfigurationMismatch,
    #[error("recovery policy requires a capability the adapter did not declare")]
    UnsupportedRecoveryPolicy,
    #[error("edge retry timestamp overflowed")]
    TimeOverflow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub recovered_leases: u64,
    pub claimed: u64,
    pub succeeded: u64,
    pub retryable_failures: u64,
    pub permanent_failures: u64,
    pub ambiguous_outcomes: u64,
    pub recovery_probes: u64,
    pub manual_reviews: u64,
}

pub struct EdgeEngine {
    store: EdgeStore,
    adapters: AdapterRegistry,
    agent_id: ActorId,
    config: EngineConfig,
}

impl EdgeEngine {
    pub fn new(
        store: EdgeStore,
        adapters: AdapterRegistry,
        agent_id: ActorId,
        config: EngineConfig,
    ) -> Result<Self, EngineError> {
        config.validate()?;
        Ok(Self {
            store,
            adapters,
            agent_id,
            config,
        })
    }

    pub fn store(&self) -> &EdgeStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut EdgeStore {
        &mut self.store
    }

    pub fn submit(
        &mut self,
        request: CommandRequest,
        now: DateTime<Utc>,
    ) -> Result<SubmissionOutcome, EngineError> {
        self.validate_submission(&request)?;
        Ok(self.store.submit(request, now)?)
    }

    pub fn submit_cloud_delivery(
        &mut self,
        request: CommandRequest,
        delivery: &CloudDelivery,
        now: DateTime<Utc>,
    ) -> Result<SubmissionOutcome, EngineError> {
        self.validate_submission(&request)?;
        Ok(self.store.submit_cloud_delivery(request, delivery, now)?)
    }

    fn validate_submission(&self, request: &CommandRequest) -> Result<(), EngineError> {
        request.validate()?;
        let adapter = self
            .adapters
            .get(&request.device_id)
            .ok_or_else(|| EngineError::AdapterNotRegistered(request.device_id.to_string()))?;
        let durable = self.store.device_status(&request.device_id)?;
        if adapter.descriptor() != &durable.descriptor
            || adapter.descriptor().tenant_id != request.tenant_id
            || adapter.descriptor().facility_id != request.facility_id
            || adapter.descriptor().class != request.command.device_class()
        {
            return Err(EngineError::AdapterConfigurationMismatch);
        }
        let capabilities = adapter.capabilities();
        match request.recovery_policy {
            RecoveryPolicy::DeviceDeduplicatedReplay
                if !capabilities.device_side_duplicate_protection =>
            {
                return Err(EngineError::UnsupportedRecoveryPolicy);
            }
            RecoveryPolicy::ProbeThenRetry if !capabilities.recovery_probe => {
                return Err(EngineError::UnsupportedRecoveryPolicy);
            }
            _ => {}
        }
        Ok(())
    }

    pub fn refresh_heartbeats(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Vec<DeviceStatus>, EngineError> {
        let mut statuses = Vec::with_capacity(self.adapters.len());
        for device_id in self.adapters.device_ids() {
            let heartbeat = self
                .adapters
                .get_mut(&device_id)
                .ok_or_else(|| EngineError::AdapterNotRegistered(device_id.to_string()))?
                .heartbeat();
            let status = match heartbeat {
                Ok(report) => {
                    report.validate().map_err(|failure| {
                        EngineError::Store(StoreError::CorruptRecord(failure.to_string()))
                    })?;
                    self.store.record_heartbeat(&device_id, &report, now)?
                }
                Err(failure) => self
                    .store
                    .record_heartbeat_failure(&device_id, &failure, now)?,
            };
            statuses.push(status);
        }
        Ok(statuses)
    }

    pub fn run_once(&mut self, now: DateTime<Utc>) -> Result<RunSummary, EngineError> {
        let recovered = self.store.recover_expired_leases(now, &self.agent_id)?;
        let mut summary = RunSummary {
            recovered_leases: recovered.replayable
                + recovered.probes_required
                + recovered.manual_reviews,
            manual_reviews: recovered.manual_reviews,
            ..RunSummary::default()
        };
        for _ in 0..self.config.batch_size {
            let Some(claim) =
                self.store
                    .claim_next(now, self.config.lease, self.config.max_attempts)?
            else {
                break;
            };
            summary.claimed += 1;
            self.process_claim(&claim, now, &mut summary)?;
        }
        Ok(summary)
    }

    fn process_claim(
        &mut self,
        claim: &crate::store::ClaimedCommand,
        now: DateTime<Utc>,
        summary: &mut RunSummary,
    ) -> Result<(), EngineError> {
        let device_id = &claim.envelope.request.device_id;
        let Some(adapter) = self.adapters.get_mut(device_id) else {
            self.store.require_manual_review(
                claim,
                "no adapter is registered for this durable device command",
                &self.agent_id,
                now,
            )?;
            summary.manual_reviews += 1;
            return Ok(());
        };
        if adapter.descriptor().tenant_id != claim.envelope.request.tenant_id
            || adapter.descriptor().facility_id != claim.envelope.request.facility_id
            || adapter.descriptor().class != claim.envelope.request.command.device_class()
        {
            self.store.require_manual_review(
                claim,
                "adapter configuration changed after command persistence",
                &self.agent_id,
                now,
            )?;
            summary.manual_reviews += 1;
            return Ok(());
        }
        let capabilities = adapter.capabilities();
        let missing_recovery_capability = match claim.envelope.request.recovery_policy {
            RecoveryPolicy::DeviceDeduplicatedReplay => {
                !capabilities.device_side_duplicate_protection
            }
            RecoveryPolicy::ProbeThenRetry => !capabilities.recovery_probe,
            RecoveryPolicy::ManualReview => false,
        };
        if missing_recovery_capability {
            self.store.require_manual_review(
                claim,
                "adapter recovery capabilities changed after command persistence",
                &self.agent_id,
                now,
            )?;
            summary.manual_reviews += 1;
            return Ok(());
        }

        let health = match adapter.heartbeat() {
            Ok(report) => {
                report.validate().map_err(|failure| {
                    EngineError::Store(StoreError::CorruptRecord(failure.to_string()))
                })?;
                self.store.record_heartbeat(device_id, &report, now)?;
                report
            }
            Err(failure) => {
                self.store
                    .record_heartbeat_failure(device_id, &failure, now)?;
                return self.handle_failure(claim, failure, now, summary);
            }
        };
        if !health.state.permits_automatic_work() {
            let message = health
                .message
                .unwrap_or_else(|| format!("device health is {}", health.state.as_str()));
            if health.state == crate::types::HealthState::Faulted {
                self.store
                    .require_manual_review(claim, &message, &self.agent_id, now)?;
                summary.manual_reviews += 1;
            } else {
                let next = self.next_attempt_at(retry_ordinal(claim), now)?;
                let exhausted = self.store.retryable_failure(
                    claim,
                    &message,
                    next,
                    self.retry_limits(),
                    &self.agent_id,
                    now,
                )?;
                summary.retryable_failures += 1;
                summary.manual_reviews += u64::from(exhausted);
            }
            return Ok(());
        }

        match claim.kind {
            ClaimKind::Execute => match adapter.execute(&claim.envelope) {
                Ok(result) => {
                    self.store.complete_success(claim, &result, now)?;
                    summary.succeeded += 1;
                    Ok(())
                }
                Err(failure) => self.handle_failure(claim, failure, now, summary),
            },
            ClaimKind::RecoveryProbe => {
                summary.recovery_probes += 1;
                match adapter.recover(&claim.envelope) {
                    Ok(RecoveryOutcome::Completed(result)) => {
                        self.store.complete_success(claim, &result, now)?;
                        summary.succeeded += 1;
                    }
                    Ok(RecoveryOutcome::StillProcessing) => {
                        if claim.recovery_probe_count >= self.config.max_recovery_probes {
                            self.store.require_manual_review(
                                claim,
                                "downstream command exceeded its recovery-probe budget",
                                &self.agent_id,
                                now,
                            )?;
                            summary.manual_reviews += 1;
                        } else {
                            let next = self.next_attempt_at(claim.recovery_probe_count, now)?;
                            self.store.defer_recovery(
                                claim,
                                "downstream command is still processing",
                                next,
                                now,
                            )?;
                        }
                    }
                    Ok(RecoveryOutcome::NotFound) => {
                        if claim.envelope.attempt >= self.config.max_attempts {
                            self.store.require_manual_review(
                                claim,
                                "recovery found no downstream command, but the execution retry budget is exhausted",
                                &self.agent_id,
                                now,
                            )?;
                            summary.manual_reviews += 1;
                        } else {
                            self.store.recovery_not_found(claim, now)?;
                        }
                    }
                    Ok(RecoveryOutcome::ManualReview { reason }) => {
                        self.store
                            .require_manual_review(claim, &reason, &self.agent_id, now)?;
                        summary.manual_reviews += 1;
                    }
                    Err(failure) => self.handle_failure(claim, failure, now, summary)?,
                }
                Ok(())
            }
        }
    }

    fn handle_failure(
        &mut self,
        claim: &crate::store::ClaimedCommand,
        failure: AdapterFailure,
        now: DateTime<Utc>,
        summary: &mut RunSummary,
    ) -> Result<(), EngineError> {
        match failure.class {
            AdapterFailureClass::Retryable => {
                let next = self.next_attempt_at(retry_ordinal(claim), now)?;
                let exhausted = self.store.retryable_failure(
                    claim,
                    &failure.message,
                    next,
                    self.retry_limits(),
                    &self.agent_id,
                    now,
                )?;
                summary.retryable_failures += 1;
                summary.manual_reviews += u64::from(exhausted);
            }
            AdapterFailureClass::Permanent => {
                if claim.kind == ClaimKind::RecoveryProbe {
                    self.store.require_manual_review(
                        claim,
                        &format!("recovery probe failed permanently: {}", failure.message),
                        &self.agent_id,
                        now,
                    )?;
                    summary.manual_reviews += 1;
                } else {
                    self.store.permanent_failure(claim, &failure.message, now)?;
                    summary.permanent_failures += 1;
                }
            }
            AdapterFailureClass::Ambiguous => {
                let next = self.next_attempt_at(retry_ordinal(claim), now)?;
                let manual = self.store.ambiguous_failure(
                    claim,
                    &failure.message,
                    next,
                    &self.agent_id,
                    now,
                )?;
                summary.ambiguous_outcomes += 1;
                summary.manual_reviews += u64::from(manual);
            }
        }
        Ok(())
    }

    fn next_attempt_at(
        &self,
        attempt: u32,
        now: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, EngineError> {
        let exponent = attempt.saturating_sub(1).min(31);
        let multiplier = 1_u32 << exponent;
        let delay = self
            .config
            .retry_delay
            .checked_mul(multiplier)
            .unwrap_or(self.config.retry_delay_cap)
            .min(self.config.retry_delay_cap);
        chrono::Duration::from_std(delay)
            .ok()
            .and_then(|delay| now.checked_add_signed(delay))
            .ok_or(EngineError::TimeOverflow)
    }

    fn retry_limits(&self) -> RetryLimits {
        RetryLimits {
            execution_attempts: self.config.max_attempts,
            recovery_probes: self.config.max_recovery_probes,
        }
    }

    pub fn automatic_devices(&self) -> Result<usize, EngineError> {
        Ok(self
            .store
            .list_devices()?
            .into_iter()
            .filter(|device| device.control_mode == ControlMode::Automatic)
            .count())
    }
}

fn retry_ordinal(claim: &crate::store::ClaimedCommand) -> u32 {
    match claim.kind {
        ClaimKind::Execute => claim.envelope.attempt,
        ClaimKind::RecoveryProbe => claim.recovery_probe_count,
    }
}
