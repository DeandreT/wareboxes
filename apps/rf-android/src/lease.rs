//! Pure lease scheduling for claimed movement work.
//!
//! Server wall-clock timestamps are converted to a monotonic deadline as soon as
//! they are observed. Subsequent decisions use only monotonic time, so device
//! wall-clock corrections cannot extend a claim locally.

use std::cmp::min;
use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;

/// Default time before the local lease deadline when inventory actions stop.
pub const DEFAULT_ACTION_GUARD: Duration = Duration::from_secs(60);

/// Default delay between retryable heartbeat failures.
pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(5);

/// Default time allowed for a heartbeat callback before retry recovery begins.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Wall-clock and monotonic readings captured together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSample {
    pub wall_time: DateTime<Utc>,
    pub monotonic: Duration,
}

impl ClockSample {
    #[must_use]
    pub const fn new(wall_time: DateTime<Utc>, monotonic: Duration) -> Self {
        Self {
            wall_time,
            monotonic,
        }
    }
}

/// Timing policy for one RF claim lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeasePolicy {
    action_guard: Duration,
    retry_delay: Duration,
    request_timeout: Duration,
}

impl LeasePolicy {
    /// Creates a policy with non-zero safety, retry, and request timeout windows.
    pub fn new(
        action_guard: Duration,
        retry_delay: Duration,
        request_timeout: Duration,
    ) -> Result<Self, LeaseError> {
        if action_guard.is_zero() {
            return Err(LeaseError::InvalidPolicy(
                "action guard must be greater than zero",
            ));
        }
        if retry_delay.is_zero() {
            return Err(LeaseError::InvalidPolicy(
                "retry delay must be greater than zero",
            ));
        }
        if request_timeout.is_zero() {
            return Err(LeaseError::InvalidPolicy(
                "request timeout must be greater than zero",
            ));
        }
        Ok(Self {
            action_guard,
            retry_delay,
            request_timeout,
        })
    }

    #[must_use]
    pub const fn action_guard(self) -> Duration {
        self.action_guard
    }

    #[must_use]
    pub const fn retry_delay(self) -> Duration {
        self.retry_delay
    }

    #[must_use]
    pub const fn request_timeout(self) -> Duration {
        self.request_timeout
    }
}

impl Default for LeasePolicy {
    fn default() -> Self {
        Self {
            action_guard: DEFAULT_ACTION_GUARD,
            retry_delay: DEFAULT_RETRY_DELAY,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

/// Stable identity for a single heartbeat request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatAttemptId(u64);

impl HeartbeatAttemptId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A heartbeat attempt that the caller may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatAttempt {
    pub id: HeartbeatAttemptId,
    pub task_id: i64,
    pub started_at: Duration,
}

/// Why a heartbeat did not establish a new lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatFailureKind {
    /// The outcome is retryable or ambiguous and the existing deadline still applies.
    Retryable,
    /// The server definitively rejected ownership of the claim.
    LeaseRejected,
}

/// The most recently recorded heartbeat outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatOutcome {
    Succeeded {
        attempt_id: HeartbeatAttemptId,
        received_at: Duration,
    },
    Failed {
        attempt_id: HeartbeatAttemptId,
        received_at: Duration,
        kind: HeartbeatFailureKind,
        consecutive_failures: u32,
    },
}

/// Current heartbeat scheduling state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatState {
    Scheduled {
        next_attempt_at: Duration,
    },
    InFlight {
        attempt_id: HeartbeatAttemptId,
        started_at: Duration,
    },
    RetryScheduled {
        next_attempt_at: Duration,
        consecutive_failures: u32,
    },
    LeaseRejected,
}

/// Reason the client must not begin a new inventory-affecting action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionBlockReason {
    /// The claim has not yet been revalidated with the server on this app run.
    UnverifiedLease,
    /// The guard window has begun, but a heartbeat may still renew the lease.
    LeaseExpiresSoon,
    /// The monotonic lease deadline has passed.
    LeaseExpired,
    /// The server definitively rejected the claim.
    LeaseRejected,
}

/// Parsed heartbeat response fields needed to renew the monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatLease<'a> {
    pub task_id: i64,
    pub heartbeat_at: &'a str,
    pub lease_expires_at: &'a str,
}

/// Pure state machine for one claimed movement task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovementLeaseMonitor {
    task_id: i64,
    lease_expires_at: DateTime<Utc>,
    lease_deadline: Duration,
    policy: LeasePolicy,
    heartbeat_state: HeartbeatState,
    last_outcome: Option<HeartbeatOutcome>,
    next_attempt_id: u64,
    consecutive_failures: u32,
    heartbeat_cycle_started_at: Option<Duration>,
    verified: bool,
}

impl MovementLeaseMonitor {
    /// Anchors a server lease expiry to a local monotonic clock sample.
    pub fn new(
        task_id: i64,
        lease_expires_at: &str,
        observed_at: ClockSample,
        policy: LeasePolicy,
    ) -> Result<Self, LeaseError> {
        let lease_expires_at = parse_server_timestamp("lease_expires_at", lease_expires_at)?;
        let remaining = lease_expires_at
            .signed_duration_since(observed_at.wall_time)
            .to_std()
            .unwrap_or(Duration::ZERO);
        let lease_deadline = checked_add(observed_at.monotonic, remaining)?;
        let next_attempt_at = observed_at.monotonic;

        Ok(Self {
            task_id,
            lease_expires_at,
            lease_deadline,
            policy,
            heartbeat_state: HeartbeatState::Scheduled { next_attempt_at },
            last_outcome: None,
            next_attempt_id: 1,
            consecutive_failures: 0,
            heartbeat_cycle_started_at: None,
            verified: false,
        })
    }

    #[must_use]
    pub const fn task_id(&self) -> i64 {
        self.task_id
    }

    #[must_use]
    pub const fn lease_expires_at(&self) -> DateTime<Utc> {
        self.lease_expires_at
    }

    #[must_use]
    pub const fn lease_deadline(&self) -> Duration {
        self.lease_deadline
    }

    #[must_use]
    pub const fn heartbeat_state(&self) -> HeartbeatState {
        self.heartbeat_state
    }

    #[must_use]
    pub const fn last_outcome(&self) -> Option<HeartbeatOutcome> {
        self.last_outcome
    }

    #[must_use]
    pub const fn is_verified(&self) -> bool {
        self.verified
    }

    /// Returns whether the scheduler is ready to start another heartbeat.
    #[must_use]
    pub fn heartbeat_due(&self, monotonic_now: Duration) -> bool {
        if monotonic_now >= self.lease_deadline {
            return false;
        }
        match self.heartbeat_state {
            HeartbeatState::Scheduled { next_attempt_at }
            | HeartbeatState::RetryScheduled {
                next_attempt_at, ..
            } => monotonic_now >= next_attempt_at,
            HeartbeatState::InFlight { .. } | HeartbeatState::LeaseRejected => false,
        }
    }

    /// Starts a due heartbeat. Repeated calls cannot create concurrent attempts.
    pub fn begin_heartbeat(&mut self, monotonic_now: Duration) -> Option<HeartbeatAttempt> {
        if !self.heartbeat_due(monotonic_now) {
            return None;
        }

        if matches!(self.heartbeat_state, HeartbeatState::Scheduled { .. }) {
            self.heartbeat_cycle_started_at = Some(monotonic_now);
        }
        let attempt_id = HeartbeatAttemptId(self.next_attempt_id);
        self.next_attempt_id = self.next_attempt_id.saturating_add(1);
        self.heartbeat_state = HeartbeatState::InFlight {
            attempt_id,
            started_at: monotonic_now,
        };
        Some(HeartbeatAttempt {
            id: attempt_id,
            task_id: self.task_id,
            started_at: monotonic_now,
        })
    }

    /// Returns whether the in-flight attempt has exceeded the request timeout.
    #[must_use]
    pub fn heartbeat_request_timed_out(&self, monotonic_now: Duration) -> bool {
        match self.heartbeat_state {
            HeartbeatState::InFlight { started_at, .. } => monotonic_now
                .checked_sub(started_at)
                .is_some_and(|elapsed| elapsed >= self.policy.request_timeout),
            _ => false,
        }
    }

    /// Converts a lost heartbeat callback into a retryable failure.
    ///
    /// Once this transition occurs, a callback for the timed-out attempt is stale
    /// and cannot update the lease.
    pub fn expire_timed_out_heartbeat(
        &mut self,
        monotonic_now: Duration,
    ) -> Result<Option<HeartbeatAttemptId>, LeaseError> {
        if !self.heartbeat_request_timed_out(monotonic_now) {
            return Ok(None);
        }
        let HeartbeatState::InFlight { attempt_id, .. } = self.heartbeat_state else {
            return Ok(None);
        };
        self.heartbeat_failed(attempt_id, HeartbeatFailureKind::Retryable, monotonic_now)?;
        Ok(Some(attempt_id))
    }

    /// Records a successful renewal and schedules the next attempt.
    ///
    /// The new monotonic deadline is anchored to the first request in this
    /// idempotency cycle rather than a retry or response receipt. Since the server
    /// cannot renew before that request is sent, this deliberately underestimates
    /// the remaining lease across both network latency and replayed retries.
    pub fn heartbeat_succeeded(
        &mut self,
        attempt_id: HeartbeatAttemptId,
        response: HeartbeatLease<'_>,
        received_at: Duration,
    ) -> Result<(), LeaseError> {
        let attempt_started_at = self.require_in_flight(attempt_id)?;
        validate_monotonic_order(attempt_started_at, received_at)?;
        let cycle_started_at = self
            .heartbeat_cycle_started_at
            .ok_or(LeaseError::MissingHeartbeatCycle)?;
        validate_monotonic_order(cycle_started_at, attempt_started_at)?;
        if response.task_id != self.task_id {
            return Err(LeaseError::TaskMismatch {
                expected: self.task_id,
                actual: response.task_id,
            });
        }

        let heartbeat_at = parse_server_timestamp("heartbeat_at", response.heartbeat_at)?;
        let lease_expires_at =
            parse_server_timestamp("lease_expires_at", response.lease_expires_at)?;
        let lease_duration = lease_expires_at
            .signed_duration_since(heartbeat_at)
            .to_std()
            .map_err(|_| LeaseError::InvalidLeaseWindow)?;
        if lease_duration.is_zero() {
            return Err(LeaseError::InvalidLeaseWindow);
        }

        let lease_deadline = checked_add(cycle_started_at, lease_duration)?;
        let next_attempt_at = checked_add(
            cycle_started_at,
            heartbeat_offset(lease_duration, self.policy),
        )?;
        self.lease_expires_at = lease_expires_at;
        self.lease_deadline = lease_deadline;
        self.heartbeat_state = HeartbeatState::Scheduled { next_attempt_at };
        self.last_outcome = Some(HeartbeatOutcome::Succeeded {
            attempt_id,
            received_at,
        });
        self.consecutive_failures = 0;
        self.heartbeat_cycle_started_at = None;
        self.verified = true;
        Ok(())
    }

    /// Records a failed attempt without extending the current lease.
    pub fn heartbeat_failed(
        &mut self,
        attempt_id: HeartbeatAttemptId,
        kind: HeartbeatFailureKind,
        received_at: Duration,
    ) -> Result<(), LeaseError> {
        let started_at = self.require_in_flight(attempt_id)?;
        validate_monotonic_order(started_at, received_at)?;
        let consecutive_failures = self.consecutive_failures.saturating_add(1);
        let heartbeat_state = match kind {
            HeartbeatFailureKind::Retryable => {
                let retry_at = checked_add(received_at, self.policy.retry_delay)?;
                HeartbeatState::RetryScheduled {
                    next_attempt_at: min(retry_at, self.action_block_at()),
                    consecutive_failures,
                }
            }
            HeartbeatFailureKind::LeaseRejected => HeartbeatState::LeaseRejected,
        };

        self.last_outcome = Some(HeartbeatOutcome::Failed {
            attempt_id,
            received_at,
            kind,
            consecutive_failures,
        });
        self.consecutive_failures = consecutive_failures;
        self.heartbeat_state = heartbeat_state;
        if kind == HeartbeatFailureKind::LeaseRejected {
            self.heartbeat_cycle_started_at = None;
        }
        Ok(())
    }

    /// Returns the safety reason that blocks new inventory work, if any.
    #[must_use]
    pub fn action_block_reason(&self, monotonic_now: Duration) -> Option<ActionBlockReason> {
        if self.heartbeat_state == HeartbeatState::LeaseRejected {
            return Some(ActionBlockReason::LeaseRejected);
        }
        if monotonic_now >= self.lease_deadline {
            return Some(ActionBlockReason::LeaseExpired);
        }
        if !self.verified {
            return Some(ActionBlockReason::UnverifiedLease);
        }
        if monotonic_now >= self.action_block_at() {
            return Some(ActionBlockReason::LeaseExpiresSoon);
        }
        None
    }

    /// New inventory-affecting commands are allowed only outside the guard window.
    #[must_use]
    pub fn inventory_actions_allowed(&self, monotonic_now: Duration) -> bool {
        self.action_block_reason(monotonic_now).is_none()
    }

    fn action_block_at(&self) -> Duration {
        self.lease_deadline.saturating_sub(self.policy.action_guard)
    }

    fn require_in_flight(
        &self,
        actual_attempt_id: HeartbeatAttemptId,
    ) -> Result<Duration, LeaseError> {
        match self.heartbeat_state {
            HeartbeatState::InFlight {
                attempt_id,
                started_at,
            } if attempt_id == actual_attempt_id => Ok(started_at),
            HeartbeatState::InFlight { attempt_id, .. } if actual_attempt_id.0 < attempt_id.0 => {
                Err(LeaseError::StaleHeartbeatAttempt {
                    actual: actual_attempt_id,
                })
            }
            HeartbeatState::InFlight { attempt_id, .. } => Err(LeaseError::AttemptMismatch {
                expected: attempt_id,
                actual: actual_attempt_id,
            }),
            _ if actual_attempt_id.0 < self.next_attempt_id => {
                Err(LeaseError::StaleHeartbeatAttempt {
                    actual: actual_attempt_id,
                })
            }
            _ => Err(LeaseError::NoHeartbeatInFlight),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LeaseError {
    #[error("invalid lease policy: {0}")]
    InvalidPolicy(&'static str),
    #[error("{field} is not a valid RFC 3339 timestamp")]
    InvalidTimestamp { field: &'static str },
    #[error("heartbeat lease expiry must be later than heartbeat time")]
    InvalidLeaseWindow,
    #[error("monotonic time moved backwards during a heartbeat attempt")]
    MonotonicTimeMovedBackwards,
    #[error("lease timing exceeds the supported monotonic clock range")]
    MonotonicOverflow,
    #[error("no heartbeat attempt is in flight")]
    NoHeartbeatInFlight,
    #[error("heartbeat attempt has no active idempotency cycle")]
    MissingHeartbeatCycle,
    #[error("heartbeat attempt {actual:?} is stale")]
    StaleHeartbeatAttempt { actual: HeartbeatAttemptId },
    #[error("heartbeat attempt mismatch: expected {expected:?}, received {actual:?}")]
    AttemptMismatch {
        expected: HeartbeatAttemptId,
        actual: HeartbeatAttemptId,
    },
    #[error("heartbeat task mismatch: expected {expected}, received {actual}")]
    TaskMismatch { expected: i64, actual: i64 },
}

fn parse_server_timestamp(field: &'static str, value: &str) -> Result<DateTime<Utc>, LeaseError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| LeaseError::InvalidTimestamp { field })
}

fn checked_add(base: Duration, delta: Duration) -> Result<Duration, LeaseError> {
    base.checked_add(delta).ok_or(LeaseError::MonotonicOverflow)
}

fn validate_monotonic_order(started_at: Duration, received_at: Duration) -> Result<(), LeaseError> {
    if received_at < started_at {
        return Err(LeaseError::MonotonicTimeMovedBackwards);
    }
    Ok(())
}

fn heartbeat_offset(lease_duration: Duration, policy: LeasePolicy) -> Duration {
    let halfway = lease_duration / 2;
    let retry_reserve = policy.retry_delay.checked_mul(2).unwrap_or(Duration::MAX);
    let safety_reserve = policy
        .action_guard
        .checked_add(retry_reserve)
        .unwrap_or(Duration::MAX);
    min(halfway, lease_duration.saturating_sub(safety_reserve))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK_ID: i64 = 42;

    fn timestamp(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn clock(wall_time: &str, monotonic_secs: u64) -> ClockSample {
        ClockSample::new(timestamp(wall_time), Duration::from_secs(monotonic_secs))
    }

    fn monitor() -> MovementLeaseMonitor {
        MovementLeaseMonitor::new(
            TASK_ID,
            "2026-07-27T01:30:00Z",
            clock("2026-07-27T01:00:00Z", 100),
            LeasePolicy::default(),
        )
        .unwrap()
    }

    #[test]
    fn parses_rfc3339_offsets_and_anchors_to_monotonic_time() {
        let monitor = MovementLeaseMonitor::new(
            TASK_ID,
            "2026-07-26T18:30:00-07:00",
            clock("2026-07-27T01:00:00Z", 100),
            LeasePolicy::default(),
        )
        .unwrap();

        assert_eq!(
            monitor.lease_expires_at(),
            timestamp("2026-07-27T01:30:00Z")
        );
        assert_eq!(monitor.lease_deadline(), Duration::from_secs(1_900));
        assert_eq!(
            monitor.heartbeat_state(),
            HeartbeatState::Scheduled {
                next_attempt_at: Duration::from_secs(100)
            }
        );
    }

    #[test]
    fn rejects_malformed_server_timestamp() {
        assert_eq!(
            MovementLeaseMonitor::new(
                TASK_ID,
                "tomorrow",
                clock("2026-07-27T01:00:00Z", 100),
                LeasePolicy::default(),
            ),
            Err(LeaseError::InvalidTimestamp {
                field: "lease_expires_at"
            })
        );
    }

    #[test]
    fn expired_initial_lease_is_represented_as_blocked() {
        let monitor = MovementLeaseMonitor::new(
            TASK_ID,
            "2026-07-27T00:59:59Z",
            clock("2026-07-27T01:00:00Z", 100),
            LeasePolicy::default(),
        )
        .unwrap();

        assert_eq!(
            monitor.action_block_reason(Duration::from_secs(100)),
            Some(ActionBlockReason::LeaseExpired)
        );
        assert!(!monitor.heartbeat_due(Duration::from_secs(100)));
    }

    #[test]
    fn inventory_stays_blocked_until_initial_heartbeat_succeeds() {
        let monitor = monitor();

        assert_eq!(
            monitor.action_block_reason(Duration::from_secs(100)),
            Some(ActionBlockReason::UnverifiedLease)
        );
        assert!(!monitor.inventory_actions_allowed(Duration::from_secs(100)));
        assert!(monitor.heartbeat_due(Duration::from_secs(100)));
    }

    #[test]
    fn verified_lease_uses_only_monotonic_time_for_action_guard() {
        let mut monitor = monitor();
        let attempt = monitor.begin_heartbeat(Duration::from_secs(100)).unwrap();
        monitor
            .heartbeat_succeeded(
                attempt.id,
                HeartbeatLease {
                    task_id: TASK_ID,
                    heartbeat_at: "2026-07-27T01:00:00Z",
                    lease_expires_at: "2026-07-27T01:30:00Z",
                },
                Duration::from_secs(101),
            )
            .unwrap();

        assert!(monitor.inventory_actions_allowed(Duration::from_secs(1_839)));
        assert_eq!(
            monitor.action_block_reason(Duration::from_secs(1_840)),
            Some(ActionBlockReason::LeaseExpiresSoon)
        );
        assert_eq!(
            monitor.action_block_reason(Duration::from_secs(1_900)),
            Some(ActionBlockReason::LeaseExpired)
        );
    }

    #[test]
    fn starts_only_one_attempt_when_heartbeat_is_due() {
        let mut monitor = monitor();

        assert!(monitor.begin_heartbeat(Duration::from_secs(99)).is_none());
        let attempt = monitor.begin_heartbeat(Duration::from_secs(100)).unwrap();
        assert_eq!(attempt.id.get(), 1);
        assert_eq!(attempt.task_id, TASK_ID);
        assert!(monitor.begin_heartbeat(Duration::from_secs(101)).is_none());
        assert_eq!(
            monitor.heartbeat_state(),
            HeartbeatState::InFlight {
                attempt_id: attempt.id,
                started_at: Duration::from_secs(100)
            }
        );
    }

    #[test]
    fn successful_heartbeat_uses_request_start_as_conservative_anchor() {
        let mut monitor = monitor();
        let attempt = monitor.begin_heartbeat(Duration::from_secs(1_000)).unwrap();

        monitor
            .heartbeat_succeeded(
                attempt.id,
                HeartbeatLease {
                    task_id: TASK_ID,
                    heartbeat_at: "2026-07-27T01:15:03Z",
                    lease_expires_at: "2026-07-27T01:45:03Z",
                },
                Duration::from_secs(1_008),
            )
            .unwrap();

        assert_eq!(monitor.lease_deadline(), Duration::from_secs(2_800));
        assert_eq!(
            monitor.heartbeat_state(),
            HeartbeatState::Scheduled {
                next_attempt_at: Duration::from_secs(1_900)
            }
        );
        assert_eq!(
            monitor.last_outcome(),
            Some(HeartbeatOutcome::Succeeded {
                attempt_id: attempt.id,
                received_at: Duration::from_secs(1_008)
            })
        );
    }

    #[test]
    fn retryable_failure_preserves_deadline_and_schedules_retry() {
        let mut monitor = monitor();
        let attempt = monitor.begin_heartbeat(Duration::from_secs(1_000)).unwrap();

        monitor
            .heartbeat_failed(
                attempt.id,
                HeartbeatFailureKind::Retryable,
                Duration::from_secs(1_002),
            )
            .unwrap();

        assert_eq!(monitor.lease_deadline(), Duration::from_secs(1_900));
        assert_eq!(
            monitor.heartbeat_state(),
            HeartbeatState::RetryScheduled {
                next_attempt_at: Duration::from_secs(1_007),
                consecutive_failures: 1
            }
        );
        assert!(monitor.heartbeat_due(Duration::from_secs(1_007)));
    }

    #[test]
    fn retry_is_brought_forward_to_action_guard() {
        let policy = LeasePolicy::new(
            Duration::from_secs(30),
            Duration::from_secs(20),
            Duration::from_secs(15),
        )
        .unwrap();
        let mut monitor = MovementLeaseMonitor::new(
            TASK_ID,
            "2026-07-27T01:01:00Z",
            clock("2026-07-27T01:00:00Z", 100),
            policy,
        )
        .unwrap();
        let attempt = monitor.begin_heartbeat(Duration::from_secs(100)).unwrap();

        monitor
            .heartbeat_failed(
                attempt.id,
                HeartbeatFailureKind::Retryable,
                Duration::from_secs(145),
            )
            .unwrap();

        assert_eq!(
            monitor.heartbeat_state(),
            HeartbeatState::RetryScheduled {
                next_attempt_at: Duration::from_secs(130),
                consecutive_failures: 1
            }
        );
        assert!(monitor.heartbeat_due(Duration::from_secs(145)));
    }

    #[test]
    fn rejected_lease_blocks_actions_immediately() {
        let mut monitor = monitor();
        let attempt = monitor.begin_heartbeat(Duration::from_secs(1_000)).unwrap();

        monitor
            .heartbeat_failed(
                attempt.id,
                HeartbeatFailureKind::LeaseRejected,
                Duration::from_secs(1_001),
            )
            .unwrap();

        assert_eq!(
            monitor.action_block_reason(Duration::from_secs(1_001)),
            Some(ActionBlockReason::LeaseRejected)
        );
        assert!(!monitor.inventory_actions_allowed(Duration::from_secs(1_001)));
        assert!(!monitor.heartbeat_due(Duration::from_secs(1_001)));
    }

    #[test]
    fn lost_callback_times_out_and_schedules_retry() {
        let mut monitor = monitor();
        let attempt = monitor.begin_heartbeat(Duration::from_secs(100)).unwrap();

        assert!(!monitor.heartbeat_request_timed_out(Duration::from_secs(114)));
        assert_eq!(
            monitor
                .expire_timed_out_heartbeat(Duration::from_secs(114))
                .unwrap(),
            None
        );
        assert!(monitor.heartbeat_request_timed_out(Duration::from_secs(115)));
        assert_eq!(
            monitor
                .expire_timed_out_heartbeat(Duration::from_secs(115))
                .unwrap(),
            Some(attempt.id)
        );
        assert_eq!(
            monitor.heartbeat_state(),
            HeartbeatState::RetryScheduled {
                next_attempt_at: Duration::from_secs(120),
                consecutive_failures: 1
            }
        );
        assert_eq!(
            monitor.last_outcome(),
            Some(HeartbeatOutcome::Failed {
                attempt_id: attempt.id,
                received_at: Duration::from_secs(115),
                kind: HeartbeatFailureKind::Retryable,
                consecutive_failures: 1
            })
        );
    }

    #[test]
    fn callback_after_timeout_is_stale_even_after_retry_starts() {
        let mut monitor = monitor();
        let first = monitor.begin_heartbeat(Duration::from_secs(100)).unwrap();
        monitor
            .expire_timed_out_heartbeat(Duration::from_secs(115))
            .unwrap();
        let before_late_callback = monitor.clone();

        assert_eq!(
            monitor.heartbeat_succeeded(
                first.id,
                HeartbeatLease {
                    task_id: TASK_ID,
                    heartbeat_at: "2026-07-27T01:00:15Z",
                    lease_expires_at: "2026-07-27T01:30:15Z",
                },
                Duration::from_secs(116),
            ),
            Err(LeaseError::StaleHeartbeatAttempt { actual: first.id })
        );
        assert_eq!(monitor, before_late_callback);

        let second = monitor.begin_heartbeat(Duration::from_secs(120)).unwrap();
        assert_eq!(second.id.get(), 2);
        assert_eq!(
            monitor.heartbeat_failed(
                first.id,
                HeartbeatFailureKind::Retryable,
                Duration::from_secs(121),
            ),
            Err(LeaseError::StaleHeartbeatAttempt { actual: first.id })
        );
        assert_eq!(
            monitor.heartbeat_state(),
            HeartbeatState::InFlight {
                attempt_id: second.id,
                started_at: Duration::from_secs(120)
            }
        );
    }

    #[test]
    fn replayed_retry_anchors_lease_to_original_cycle_start() {
        let mut monitor = monitor();
        let first = monitor.begin_heartbeat(Duration::from_secs(100)).unwrap();
        monitor
            .expire_timed_out_heartbeat(Duration::from_secs(115))
            .unwrap();
        let retry = monitor.begin_heartbeat(Duration::from_secs(120)).unwrap();
        assert_ne!(retry.id, first.id);

        monitor
            .heartbeat_succeeded(
                retry.id,
                HeartbeatLease {
                    task_id: TASK_ID,
                    heartbeat_at: "2026-07-27T01:00:00Z",
                    lease_expires_at: "2026-07-27T01:30:00Z",
                },
                Duration::from_secs(121),
            )
            .unwrap();

        assert_eq!(monitor.lease_deadline(), Duration::from_secs(1_900));
        assert_eq!(
            monitor.heartbeat_state(),
            HeartbeatState::Scheduled {
                next_attempt_at: Duration::from_secs(1_000)
            }
        );

        let next_cycle = monitor.begin_heartbeat(Duration::from_secs(1_000)).unwrap();
        monitor
            .heartbeat_succeeded(
                next_cycle.id,
                HeartbeatLease {
                    task_id: TASK_ID,
                    heartbeat_at: "2026-07-27T01:15:00Z",
                    lease_expires_at: "2026-07-27T01:45:00Z",
                },
                Duration::from_secs(1_001),
            )
            .unwrap();
        assert_eq!(monitor.lease_deadline(), Duration::from_secs(2_800));
    }

    #[test]
    fn invalid_success_does_not_mutate_the_in_flight_attempt() {
        let mut monitor = monitor();
        let attempt = monitor.begin_heartbeat(Duration::from_secs(1_000)).unwrap();
        let previous = monitor.clone();

        assert_eq!(
            monitor.heartbeat_succeeded(
                attempt.id,
                HeartbeatLease {
                    task_id: TASK_ID,
                    heartbeat_at: "2026-07-27T01:45:03Z",
                    lease_expires_at: "2026-07-27T01:45:03Z",
                },
                Duration::from_secs(1_008),
            ),
            Err(LeaseError::InvalidLeaseWindow)
        );
        assert_eq!(monitor, previous);
    }

    #[test]
    fn rejects_stale_attempt_and_wrong_task_without_mutation() {
        let mut monitor = monitor();
        let attempt = monitor.begin_heartbeat(Duration::from_secs(1_000)).unwrap();
        let previous = monitor.clone();

        assert_eq!(
            monitor.heartbeat_failed(
                HeartbeatAttemptId(999),
                HeartbeatFailureKind::Retryable,
                Duration::from_secs(1_001),
            ),
            Err(LeaseError::AttemptMismatch {
                expected: attempt.id,
                actual: HeartbeatAttemptId(999)
            })
        );
        assert_eq!(monitor, previous);

        assert_eq!(
            monitor.heartbeat_succeeded(
                attempt.id,
                HeartbeatLease {
                    task_id: TASK_ID + 1,
                    heartbeat_at: "2026-07-27T01:15:03Z",
                    lease_expires_at: "2026-07-27T01:45:03Z",
                },
                Duration::from_secs(1_008),
            ),
            Err(LeaseError::TaskMismatch {
                expected: TASK_ID,
                actual: TASK_ID + 1
            })
        );
        assert_eq!(monitor, previous);
    }

    #[test]
    fn short_lease_heartbeats_immediately_and_blocks_inventory() {
        let monitor = MovementLeaseMonitor::new(
            TASK_ID,
            "2026-07-27T01:00:20Z",
            clock("2026-07-27T01:00:00Z", 100),
            LeasePolicy::default(),
        )
        .unwrap();

        assert!(monitor.heartbeat_due(Duration::from_secs(100)));
        assert_eq!(
            monitor.action_block_reason(Duration::from_secs(100)),
            Some(ActionBlockReason::UnverifiedLease)
        );
    }

    #[test]
    fn policy_rejects_zero_windows() {
        assert_eq!(
            LeasePolicy::new(
                Duration::ZERO,
                Duration::from_secs(1),
                Duration::from_secs(1)
            ),
            Err(LeaseError::InvalidPolicy(
                "action guard must be greater than zero"
            ))
        );
        assert_eq!(
            LeasePolicy::new(
                Duration::from_secs(1),
                Duration::ZERO,
                Duration::from_secs(1)
            ),
            Err(LeaseError::InvalidPolicy(
                "retry delay must be greater than zero"
            ))
        );
        assert_eq!(
            LeasePolicy::new(
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::ZERO
            ),
            Err(LeaseError::InvalidPolicy(
                "request timeout must be greater than zero"
            ))
        );
    }
}
