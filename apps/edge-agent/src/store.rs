use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::adapter::{AdapterFailure, HealthReport};
use crate::command::{
    CommandEnvelope, CommandError, CommandRecord, CommandRequest, CommandResult, CommandState,
    RecoveryPolicy, SubmissionOutcome,
};
use crate::types::{
    validate_reason, ActorId, ControlAction, ControlMode, DeviceClass, DeviceDescriptor, DeviceId,
    DeviceStatus, FacilityId, HealthState, TenantId, TypeError,
};

const STORE_SCHEMA_VERSION: i64 = 1;
const MAX_PERSISTED_MESSAGE_LENGTH: usize = 1_000;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("edge command database file failed: {0}")]
    FileSystem(#[from] std::io::Error),
    #[error("edge command database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("edge command JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error(transparent)]
    Type(#[from] TypeError),
    #[error("edge database schema version {0} is unsupported")]
    UnsupportedSchema(i64),
    #[error("device {0} is not registered")]
    DeviceNotFound(DeviceId),
    #[error("device {0} is already registered with different immutable configuration")]
    DeviceConfigurationConflict(DeviceId),
    #[error("command identity was already used with different immutable content")]
    IdentityConflict,
    #[error("command {0} does not exist")]
    CommandNotFound(CommandIdRef),
    #[error("command scope does not match its registered device")]
    ScopeMismatch,
    #[error("command class {actual} does not match device class {expected}")]
    DeviceClassMismatch {
        expected: DeviceClass,
        actual: DeviceClass,
    },
    #[error("command {command_id} cannot transition from {state:?} to {target}")]
    InvalidTransition {
        command_id: String,
        state: CommandState,
        target: &'static str,
    },
    #[error("command {0} is no longer held by this execution lease")]
    LeaseMismatch(String),
    #[error("device {0} must be automatic before a command can be retried")]
    DeviceNotAutomatic(DeviceId),
    #[error("edge database record is corrupt: {0}")]
    CorruptRecord(String),
    #[error("duration is too large for the edge command database")]
    DurationOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandIdRef(pub String);

impl std::fmt::Display for CommandIdRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimKind {
    Execute,
    RecoveryProbe,
}

impl ClaimKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::RecoveryProbe => "recovery_probe",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ClaimedCommand {
    pub envelope: CommandEnvelope,
    pub lease_token: String,
    pub attempt_id: String,
    pub kind: ClaimKind,
    pub recovery_probe_count: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryLimits {
    pub execution_attempts: u32,
    pub recovery_probes: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeaseRecoverySummary {
    pub replayable: u64,
    pub probes_required: u64,
    pub manual_reviews: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub command_id: String,
    pub sequence: u32,
    pub execution_attempt: u32,
    pub kind: String,
    pub state: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEvent {
    pub command_id: String,
    pub from_state: Option<String>,
    pub to_state: String,
    pub actor: Option<String>,
    pub reason: String,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlEvent {
    pub device_id: String,
    pub from_mode: Option<String>,
    pub to_mode: String,
    pub actor: String,
    pub reason: String,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatEvent {
    pub device_id: String,
    pub health: HealthState,
    pub message: Option<String>,
    pub alarm_codes: Vec<String>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug)]
struct RawCommand {
    request_hash: Vec<u8>,
    request_json: Vec<u8>,
    state: String,
    attempt_count: i64,
    created_at_ms: i64,
    updated_at_ms: i64,
    next_attempt_at_ms: i64,
    result_json: Option<Vec<u8>>,
    last_error: Option<String>,
    resolution_note: Option<String>,
}

#[derive(Debug)]
struct RawDevice {
    device_id: String,
    tenant_id: String,
    facility_id: String,
    device_class: String,
    display_name: String,
    control_mode: String,
    control_reason: String,
    control_actor: String,
    control_changed_at_ms: i64,
    health_state: String,
    health_message: Option<String>,
    last_heartbeat_at_ms: Option<i64>,
    consecutive_health_failures: i64,
}

struct ClaimCompletion<'a> {
    target: CommandState,
    attempt_state: &'a str,
    reason: &'a str,
    next_attempt_at: DateTime<Utc>,
    result_json: Option<Vec<u8>>,
    last_error: Option<String>,
    resolution_note: Option<String>,
    manual_fallback: Option<ManualFallback<'a>>,
}

struct ManualFallback<'a> {
    actor: &'a str,
    reason: &'a str,
}

struct UnleasedTransition<'a> {
    command_id: &'a str,
    from: CommandState,
    to: CommandState,
    actor: Option<&'a str>,
    reason: &'a str,
    next_attempt_at: Option<DateTime<Utc>>,
    resolution_note: Option<&'a str>,
}

pub struct EdgeStore {
    connection: Connection,
}

impl EdgeStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        let connection = Connection::open(path)?;
        let store = Self::configure(connection, true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        Self::configure(connection, false)
    }

    fn configure(mut connection: Connection, persistent: bool) -> Result<Self, StoreError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        if persistent {
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }

        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(include_str!("store/schema.sql"))?;
                tx.pragma_update(None, "user_version", STORE_SCHEMA_VERSION)?;
                tx.commit()?;
            }
            STORE_SCHEMA_VERSION => {}
            other => return Err(StoreError::UnsupportedSchema(other)),
        }
        Ok(Self { connection })
    }

    pub fn register_device(
        &mut self,
        mut descriptor: DeviceDescriptor,
        actor: &ActorId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<DeviceStatus, StoreError> {
        descriptor.validate()?;
        descriptor.display_name = descriptor.display_name.trim().to_owned();
        let reason = validate_reason(reason)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load_device(&tx, &descriptor.device_id)? {
            let status = decode_device(existing)?;
            if status.descriptor != descriptor {
                return Err(StoreError::DeviceConfigurationConflict(
                    descriptor.device_id,
                ));
            }
            tx.commit()?;
            return Ok(status);
        }

        let at = now.timestamp_millis();
        tx.execute(
            r#"
            INSERT INTO edge_devices (
                device_id, tenant_id, facility_id, device_class, display_name,
                control_mode, control_reason, control_actor, control_changed_at_ms,
                health_state, created_at_ms, updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'disabled', ?6, ?7, ?8, 'unknown', ?8, ?8)
            "#,
            params![
                descriptor.device_id.as_str(),
                descriptor.tenant_id.as_str(),
                descriptor.facility_id.as_str(),
                descriptor.class.as_str(),
                descriptor.display_name,
                reason,
                actor.as_str(),
                at,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO edge_control_events (
                tenant_id, facility_id, device_id, from_mode, to_mode, actor, reason,
                occurred_at_ms
            ) VALUES (?1, ?2, ?3, NULL, 'disabled', ?4, ?5, ?6)
            "#,
            params![
                descriptor.tenant_id.as_str(),
                descriptor.facility_id.as_str(),
                descriptor.device_id.as_str(),
                actor.as_str(),
                reason,
                at,
            ],
        )?;
        let status = decode_device(
            load_device(&tx, &descriptor.device_id)?
                .ok_or_else(|| StoreError::DeviceNotFound(descriptor.device_id.clone()))?,
        )?;
        tx.commit()?;
        Ok(status)
    }

    pub fn device_status(&self, device_id: &DeviceId) -> Result<DeviceStatus, StoreError> {
        load_device(&self.connection, device_id)?
            .map(decode_device)
            .transpose()?
            .ok_or_else(|| StoreError::DeviceNotFound(device_id.clone()))
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceStatus>, StoreError> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT device_id, tenant_id, facility_id, device_class, display_name,
                   control_mode, control_reason, control_actor, control_changed_at_ms,
                   health_state, health_message, last_heartbeat_at_ms,
                   consecutive_health_failures
              FROM edge_devices
             ORDER BY tenant_id, facility_id, device_id
            "#,
        )?;
        let devices = statement
            .query_map([], raw_device_from_row)?
            .map(|row| decode_device(row?))
            .collect();
        devices
    }

    pub fn change_control_mode(
        &mut self,
        device_id: &DeviceId,
        action: ControlAction,
        actor: &ActorId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<DeviceStatus, StoreError> {
        let reason = validate_reason(reason)?;
        let target = action.target_mode();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = load_device(&tx, device_id)?
            .ok_or_else(|| StoreError::DeviceNotFound(device_id.clone()))?;
        let existing = decode_device(existing)?;
        if target != ControlMode::Automatic {
            quarantine_device_commands(&tx, device_id, actor.as_str(), &reason, now)?;
        }
        update_device_mode(&tx, &existing, target, actor.as_str(), &reason, now)?;
        let updated = decode_device(
            load_device(&tx, device_id)?
                .ok_or_else(|| StoreError::DeviceNotFound(device_id.clone()))?,
        )?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn record_heartbeat(
        &mut self,
        device_id: &DeviceId,
        report: &HealthReport,
        now: DateTime<Utc>,
    ) -> Result<DeviceStatus, StoreError> {
        report
            .validate()
            .map_err(|error| StoreError::CorruptRecord(error.to_string()))?;
        let alarms = serde_json::to_vec(&report.alarm_codes)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let device = decode_device(
            load_device(&tx, device_id)?
                .ok_or_else(|| StoreError::DeviceNotFound(device_id.clone()))?,
        )?;
        let failures = if report.state.permits_automatic_work() {
            0
        } else {
            device.consecutive_health_failures.saturating_add(1)
        };
        let at = now.timestamp_millis();
        tx.execute(
            r#"
            UPDATE edge_devices
               SET health_state = ?1,
                   health_message = ?2,
                   last_heartbeat_at_ms = ?3,
                   consecutive_health_failures = ?4,
                   updated_at_ms = ?3
             WHERE device_id = ?5
            "#,
            params![
                report.state.as_str(),
                report.message,
                at,
                failures,
                device_id.as_str(),
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO edge_heartbeat_events (
                tenant_id, facility_id, device_id, health_state, message,
                alarm_codes_json, observed_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                device.descriptor.tenant_id.as_str(),
                device.descriptor.facility_id.as_str(),
                device_id.as_str(),
                report.state.as_str(),
                report.message,
                alarms,
                at,
            ],
        )?;
        let updated = decode_device(
            load_device(&tx, device_id)?
                .ok_or_else(|| StoreError::DeviceNotFound(device_id.clone()))?,
        )?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn record_heartbeat_failure(
        &mut self,
        device_id: &DeviceId,
        failure: &AdapterFailure,
        now: DateTime<Utc>,
    ) -> Result<DeviceStatus, StoreError> {
        let report = HealthReport {
            state: HealthState::Offline,
            message: Some(truncate_message(&failure.message)),
            alarm_codes: Vec::new(),
        };
        self.record_heartbeat(device_id, &report, now)
    }

    pub fn submit(
        &mut self,
        request: CommandRequest,
        now: DateTime<Utc>,
    ) -> Result<SubmissionOutcome, StoreError> {
        request.validate()?;
        let request_hash = request.request_hash()?;
        let request_json = serde_json::to_vec(&request)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let device = decode_device(
            load_device(&tx, &request.device_id)?
                .ok_or_else(|| StoreError::DeviceNotFound(request.device_id.clone()))?,
        )?;
        if device.descriptor.tenant_id != request.tenant_id
            || device.descriptor.facility_id != request.facility_id
        {
            return Err(StoreError::ScopeMismatch);
        }
        if device.descriptor.class != request.command.device_class() {
            return Err(StoreError::DeviceClassMismatch {
                expected: device.descriptor.class,
                actual: request.command.device_class(),
            });
        }

        let candidates = command_identity_candidates(&tx, &request)?;
        if !candidates.is_empty() {
            if candidates.len() != 1 {
                return Err(StoreError::IdentityConflict);
            }
            let command_id = candidates
                .iter()
                .next()
                .ok_or(StoreError::IdentityConflict)?;
            let raw = load_raw_command(&tx, command_id)?
                .ok_or_else(|| StoreError::CommandNotFound(CommandIdRef(command_id.clone())))?;
            if raw.request_hash.as_slice() != request_hash || raw.request_json != request_json {
                return Err(StoreError::IdentityConflict);
            }
            let record = decode_command(raw)?;
            tx.commit()?;
            return Ok(SubmissionOutcome::Replayed(record));
        }

        let state = if device.control_mode == ControlMode::Automatic {
            CommandState::Queued
        } else {
            CommandState::ManualReview
        };
        let status_reason = if state == CommandState::Queued {
            "durable device command accepted"
        } else {
            "device command quarantined because automation is not enabled"
        };
        let at = now.timestamp_millis();
        tx.execute(
            r#"
            INSERT INTO edge_commands (
                command_id, tenant_id, facility_id, device_id, correlation_id,
                idempotency_key, schema_version, device_class, recovery_policy,
                request_hash, request_json, state, attempt_count, created_at_ms,
                updated_at_ms, next_attempt_at_ms, last_error
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0,
                ?13, ?13, ?13, ?14
            )
            "#,
            params![
                request.command_id.as_str(),
                request.tenant_id.as_str(),
                request.facility_id.as_str(),
                request.device_id.as_str(),
                request.correlation_id.as_str(),
                request.idempotency_key.as_str(),
                i64::from(request.schema_version),
                request.command.device_class().as_str(),
                request.recovery_policy.as_str(),
                request_hash.as_slice(),
                request_json,
                state.as_str(),
                at,
                (state == CommandState::ManualReview).then_some(status_reason),
            ],
        )?;
        append_command_event(
            &tx,
            request.command_id.as_str(),
            None,
            state,
            None,
            status_reason,
            now,
        )?;
        let raw = load_raw_command(&tx, request.command_id.as_str())?.ok_or_else(|| {
            StoreError::CommandNotFound(CommandIdRef(request.command_id.to_string()))
        })?;
        let record = decode_command(raw)?;
        tx.commit()?;
        Ok(SubmissionOutcome::Accepted(record))
    }

    pub fn command(&self, command_id: &str) -> Result<CommandRecord, StoreError> {
        load_raw_command(&self.connection, command_id)?
            .map(decode_command)
            .transpose()?
            .ok_or_else(|| StoreError::CommandNotFound(CommandIdRef(command_id.to_owned())))
    }

    pub fn list_commands(&self, limit: usize) -> Result<Vec<CommandRecord>, StoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(StoreError::CorruptRecord(
                "command query limit must be between 1 and 1,000".into(),
            ));
        }
        let limit = i64::try_from(limit)
            .map_err(|_| StoreError::CorruptRecord("command query limit overflowed".into()))?;
        let mut statement = self.connection.prepare(
            r#"
            SELECT request_hash, request_json, state, attempt_count, created_at_ms,
                   updated_at_ms, next_attempt_at_ms, result_json, last_error,
                   resolution_note
              FROM edge_commands
             ORDER BY created_at_ms DESC, command_id DESC
             LIMIT ?1
            "#,
        )?;
        let commands = statement
            .query_map([limit], raw_command_from_row)?
            .map(|row| decode_command(row?))
            .collect();
        commands
    }

    pub fn attempts(&self, command_id: &str) -> Result<Vec<AttemptRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT attempt_id, command_id, sequence, execution_attempt, attempt_kind,
                   state, started_at_ms, finished_at_ms, message
              FROM edge_command_attempts
             WHERE command_id = ?1
             ORDER BY sequence
            "#,
        )?;
        let raw = statement.query_map([command_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })?;
        raw.map(|row| {
            let (
                attempt_id,
                command_id,
                sequence,
                execution_attempt,
                kind,
                state,
                started,
                finished,
                message,
            ) = row?;
            Ok(AttemptRecord {
                attempt_id,
                command_id,
                sequence: checked_u32(sequence, "attempt sequence")?,
                execution_attempt: checked_u32(execution_attempt, "execution attempt")?,
                kind,
                state,
                started_at: timestamp(started)?,
                finished_at: finished.map(timestamp).transpose()?,
                message,
            })
        })
        .collect()
    }

    pub fn command_events(&self, command_id: &str) -> Result<Vec<CommandEvent>, StoreError> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT command_id, from_state, to_state, actor, reason, occurred_at_ms
              FROM edge_command_events
             WHERE command_id = ?1
             ORDER BY event_id
            "#,
        )?;
        let events = statement
            .query_map([command_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .map(|row| {
                let (command_id, from_state, to_state, actor, reason, occurred_at_ms) = row?;
                Ok(CommandEvent {
                    command_id,
                    from_state,
                    to_state,
                    actor,
                    reason,
                    occurred_at: timestamp(occurred_at_ms)?,
                })
            })
            .collect();
        events
    }

    pub fn control_events(&self, device_id: &DeviceId) -> Result<Vec<ControlEvent>, StoreError> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT device_id, from_mode, to_mode, actor, reason, occurred_at_ms
              FROM edge_control_events
             WHERE device_id = ?1
             ORDER BY event_id
            "#,
        )?;
        let events = statement
            .query_map([device_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .map(|row| {
                let (device_id, from_mode, to_mode, actor, reason, occurred_at_ms) = row?;
                Ok(ControlEvent {
                    device_id,
                    from_mode,
                    to_mode,
                    actor,
                    reason,
                    occurred_at: timestamp(occurred_at_ms)?,
                })
            })
            .collect();
        events
    }

    pub fn heartbeat_events(
        &self,
        device_id: &DeviceId,
    ) -> Result<Vec<HeartbeatEvent>, StoreError> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT device_id, health_state, message, alarm_codes_json, observed_at_ms
              FROM edge_heartbeat_events
             WHERE device_id = ?1
             ORDER BY event_id
            "#,
        )?;
        let events = statement
            .query_map([device_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .map(|row| {
                let (device_id, health, message, alarms, observed_at_ms) = row?;
                Ok(HeartbeatEvent {
                    device_id,
                    health: HealthState::parse_storage(&health)?,
                    message,
                    alarm_codes: serde_json::from_slice(&alarms)?,
                    observed_at: timestamp(observed_at_ms)?,
                })
            })
            .collect();
        events
    }

    pub fn resolve_manually(
        &mut self,
        command_id: &str,
        actor: &ActorId,
        note: &str,
        now: DateTime<Utc>,
    ) -> Result<CommandRecord, StoreError> {
        let note = validate_reason(note)?;
        self.operator_transition(
            command_id,
            actor,
            &note,
            CommandState::ResolvedManually,
            &[CommandState::ManualReview],
            now,
        )
    }

    pub fn cancel_command(
        &mut self,
        command_id: &str,
        actor: &ActorId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<CommandRecord, StoreError> {
        let reason = validate_reason(reason)?;
        self.operator_transition(
            command_id,
            actor,
            &reason,
            CommandState::Cancelled,
            &[
                CommandState::Queued,
                CommandState::RetryWait,
                CommandState::RecoveryWait,
                CommandState::ManualReview,
            ],
            now,
        )
    }

    pub fn retry_after_review(
        &mut self,
        command_id: &str,
        actor: &ActorId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<CommandRecord, StoreError> {
        let reason = validate_reason(reason)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let raw = load_raw_command(&tx, command_id)?
            .ok_or_else(|| StoreError::CommandNotFound(CommandIdRef(command_id.to_owned())))?;
        let record = decode_command(raw)?;
        if record.state != CommandState::ManualReview {
            return Err(StoreError::InvalidTransition {
                command_id: command_id.to_owned(),
                state: record.state,
                target: "retry_after_review",
            });
        }
        let device = decode_device(
            load_device(&tx, &record.request.device_id)?
                .ok_or_else(|| StoreError::DeviceNotFound(record.request.device_id.clone()))?,
        )?;
        if device.control_mode != ControlMode::Automatic {
            return Err(StoreError::DeviceNotAutomatic(record.request.device_id));
        }
        transition_unleased(
            &tx,
            now,
            UnleasedTransition {
                command_id,
                from: record.state,
                to: CommandState::Queued,
                actor: Some(actor.as_str()),
                reason: &reason,
                next_attempt_at: Some(now),
                resolution_note: None,
            },
        )?;
        let updated =
            decode_command(load_raw_command(&tx, command_id)?.ok_or_else(|| {
                StoreError::CommandNotFound(CommandIdRef(command_id.to_owned()))
            })?)?;
        tx.commit()?;
        Ok(updated)
    }

    fn operator_transition(
        &mut self,
        command_id: &str,
        actor: &ActorId,
        reason: &str,
        target: CommandState,
        allowed: &[CommandState],
        now: DateTime<Utc>,
    ) -> Result<CommandRecord, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let raw = load_raw_command(&tx, command_id)?
            .ok_or_else(|| StoreError::CommandNotFound(CommandIdRef(command_id.to_owned())))?;
        let record = decode_command(raw)?;
        if !allowed.contains(&record.state) {
            return Err(StoreError::InvalidTransition {
                command_id: command_id.to_owned(),
                state: record.state,
                target: target.as_str(),
            });
        }
        transition_unleased(
            &tx,
            now,
            UnleasedTransition {
                command_id,
                from: record.state,
                to: target,
                actor: Some(actor.as_str()),
                reason,
                next_attempt_at: None,
                resolution_note: (target == CommandState::ResolvedManually).then_some(reason),
            },
        )?;
        let updated =
            decode_command(load_raw_command(&tx, command_id)?.ok_or_else(|| {
                StoreError::CommandNotFound(CommandIdRef(command_id.to_owned()))
            })?)?;
        tx.commit()?;
        Ok(updated)
    }

    pub(crate) fn recover_expired_leases(
        &mut self,
        now: DateTime<Utc>,
        agent: &ActorId,
    ) -> Result<LeaseRecoverySummary, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let expired = {
            let mut statement = tx.prepare(
                r#"
                SELECT command_id, device_id, recovery_policy
                  FROM edge_commands
                 WHERE state = 'executing'
                   AND lease_until_ms <= ?1
                 ORDER BY created_at_ms, command_id
                "#,
            )?;
            let rows = statement
                .query_map([now.timestamp_millis()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let mut summary = LeaseRecoverySummary::default();
        for (command_id, device_id, policy) in expired {
            let policy = RecoveryPolicy::parse_storage(&policy)?;
            let (target, reason) = match policy {
                RecoveryPolicy::DeviceDeduplicatedReplay => {
                    summary.replayable += 1;
                    (
                        CommandState::RetryWait,
                        "execution lease expired; downstream duplicate protection permits replay",
                    )
                }
                RecoveryPolicy::ProbeThenRetry => {
                    summary.probes_required += 1;
                    (
                        CommandState::RecoveryWait,
                        "execution lease expired; downstream state must be probed before replay",
                    )
                }
                RecoveryPolicy::ManualReview => {
                    summary.manual_reviews += 1;
                    (
                        CommandState::ManualReview,
                        "execution lease expired with a manual-review recovery policy",
                    )
                }
            };
            abandon_active_attempt(&tx, &command_id, reason, now)?;
            transition_leased_without_token(
                &tx,
                &command_id,
                target,
                Some(agent.as_str()),
                reason,
                now,
            )?;
            if target == CommandState::ManualReview {
                force_manual_fallback(
                    &tx,
                    &DeviceId::new(device_id)?,
                    agent.as_str(),
                    reason,
                    now,
                )?;
            }
        }
        tx.commit()?;
        Ok(summary)
    }

    pub(crate) fn claim_next(
        &mut self,
        now: DateTime<Utc>,
        lease: Duration,
        max_attempts: u32,
    ) -> Result<Option<ClaimedCommand>, StoreError> {
        let lease_ms = duration_millis(lease)?;
        let lease_until = now
            .timestamp_millis()
            .checked_add(lease_ms)
            .ok_or(StoreError::DurationOverflow)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = tx
            .query_row(
                r#"
                SELECT command_id, state
                  FROM edge_commands command
                  JOIN edge_devices device ON device.device_id = command.device_id
                 WHERE command.state IN ('queued', 'retry_wait', 'recovery_wait')
                   AND command.next_attempt_at_ms <= ?1
                   AND device.control_mode = 'automatic'
                   AND (
                       command.state = 'recovery_wait'
                       OR command.attempt_count < ?2
                   )
                   AND NOT EXISTS (
                       SELECT 1
                         FROM edge_commands active
                        WHERE active.device_id = command.device_id
                          AND active.state = 'executing'
                   )
                 ORDER BY command.next_attempt_at_ms, command.created_at_ms, command.command_id
                 LIMIT 1
                "#,
                params![now.timestamp_millis(), i64::from(max_attempts)],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((command_id, prior_state_text)) = candidate else {
            tx.commit()?;
            return Ok(None);
        };
        let prior_state = CommandState::parse_storage(&prior_state_text)?;
        let kind = if prior_state == CommandState::RecoveryWait {
            ClaimKind::RecoveryProbe
        } else {
            ClaimKind::Execute
        };
        let lease_token = Uuid::new_v4().to_string();
        let attempt_id = Uuid::new_v4().to_string();
        let updated = tx.execute(
            r#"
            UPDATE edge_commands
               SET state = 'executing',
                   attempt_count = attempt_count + ?1,
                   updated_at_ms = ?2,
                   lease_token = ?3,
                   lease_until_ms = ?4
             WHERE command_id = ?5
               AND state = ?6
            "#,
            params![
                i64::from(kind == ClaimKind::Execute),
                now.timestamp_millis(),
                lease_token,
                lease_until,
                command_id,
                prior_state.as_str(),
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::LeaseMismatch(command_id));
        }
        let raw = load_raw_command(&tx, &command_id)?
            .ok_or_else(|| StoreError::CommandNotFound(CommandIdRef(command_id.clone())))?;
        let record = decode_command(raw)?;
        let sequence: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM edge_command_attempts WHERE command_id = ?1",
            [&command_id],
            |row| row.get(0),
        )?;
        tx.execute(
            r#"
            INSERT INTO edge_command_attempts (
                attempt_id, command_id, sequence, execution_attempt, attempt_kind,
                state, started_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6)
            "#,
            params![
                attempt_id,
                command_id,
                sequence,
                i64::from(record.attempt_count),
                kind.as_str(),
                now.timestamp_millis(),
            ],
        )?;
        let recovery_probe_count: i64 = tx.query_row(
            r#"
            SELECT COUNT(*)
              FROM edge_command_attempts
             WHERE command_id = ?1
               AND attempt_kind = 'recovery_probe'
            "#,
            [&command_id],
            |row| row.get(0),
        )?;
        append_command_event(
            &tx,
            &command_id,
            Some(prior_state),
            CommandState::Executing,
            None,
            kind.as_str(),
            now,
        )?;
        let claimed = ClaimedCommand {
            envelope: CommandEnvelope {
                request: record.request,
                attempt: record.attempt_count,
            },
            lease_token,
            attempt_id,
            kind,
            recovery_probe_count: checked_u32(recovery_probe_count, "recovery probe count")?,
        };
        tx.commit()?;
        Ok(Some(claimed))
    }

    pub(crate) fn complete_success(
        &mut self,
        claim: &ClaimedCommand,
        result: &CommandResult,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        result.validate_for(&claim.envelope.request.command)?;
        let encoded = serde_json::to_vec(result)?;
        self.finish_claim(
            claim,
            now,
            ClaimCompletion {
                target: CommandState::Succeeded,
                attempt_state: "succeeded",
                reason: "device command completed",
                next_attempt_at: now,
                result_json: Some(encoded),
                last_error: None,
                resolution_note: None,
                manual_fallback: None,
            },
        )
    }

    pub(crate) fn retryable_failure(
        &mut self,
        claim: &ClaimedCommand,
        message: &str,
        next_attempt_at: DateTime<Utc>,
        limits: RetryLimits,
        agent: &ActorId,
        now: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let message = truncate_message(message);
        let exhausted = match claim.kind {
            ClaimKind::Execute => claim.envelope.attempt >= limits.execution_attempts,
            ClaimKind::RecoveryProbe => claim.recovery_probe_count >= limits.recovery_probes,
        };
        let target = if exhausted {
            CommandState::ManualReview
        } else if claim.kind == ClaimKind::RecoveryProbe {
            CommandState::RecoveryWait
        } else {
            CommandState::RetryWait
        };
        let attempt_state = if exhausted {
            "manual_review"
        } else {
            "retryable_failure"
        };
        self.finish_claim(
            claim,
            now,
            ClaimCompletion {
                target,
                attempt_state,
                reason: &message,
                next_attempt_at,
                result_json: None,
                last_error: Some(message.clone()),
                resolution_note: None,
                manual_fallback: exhausted.then_some(ManualFallback {
                    actor: agent.as_str(),
                    reason: "device command exhausted its retry budget",
                }),
            },
        )?;
        Ok(exhausted)
    }

    pub(crate) fn permanent_failure(
        &mut self,
        claim: &ClaimedCommand,
        message: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let message = truncate_message(message);
        self.finish_claim(
            claim,
            now,
            ClaimCompletion {
                target: CommandState::Failed,
                attempt_state: "permanent_failure",
                reason: &message,
                next_attempt_at: now,
                result_json: None,
                last_error: Some(message.clone()),
                resolution_note: None,
                manual_fallback: None,
            },
        )
    }

    pub(crate) fn ambiguous_failure(
        &mut self,
        claim: &ClaimedCommand,
        message: &str,
        next_attempt_at: DateTime<Utc>,
        agent: &ActorId,
        now: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let message = truncate_message(message);
        let target = match claim.envelope.request.recovery_policy {
            RecoveryPolicy::DeviceDeduplicatedReplay => CommandState::RetryWait,
            RecoveryPolicy::ProbeThenRetry => CommandState::RecoveryWait,
            RecoveryPolicy::ManualReview => CommandState::ManualReview,
        };
        self.finish_claim(
            claim,
            now,
            ClaimCompletion {
                target,
                attempt_state: "ambiguous",
                reason: &message,
                next_attempt_at,
                result_json: None,
                last_error: Some(message.clone()),
                resolution_note: None,
                manual_fallback: (target == CommandState::ManualReview).then_some(ManualFallback {
                    actor: agent.as_str(),
                    reason: "ambiguous device outcome requires manual reconciliation",
                }),
            },
        )?;
        if target == CommandState::ManualReview {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(crate) fn defer_recovery(
        &mut self,
        claim: &ClaimedCommand,
        message: &str,
        next_attempt_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        if claim.kind != ClaimKind::RecoveryProbe {
            return Err(StoreError::InvalidTransition {
                command_id: claim.envelope.request.command_id.to_string(),
                state: CommandState::Executing,
                target: "defer_recovery",
            });
        }
        let message = truncate_message(message);
        self.finish_claim(
            claim,
            now,
            ClaimCompletion {
                target: CommandState::RecoveryWait,
                attempt_state: "still_processing",
                reason: &message,
                next_attempt_at,
                result_json: None,
                last_error: Some(message.clone()),
                resolution_note: None,
                manual_fallback: None,
            },
        )
    }

    pub(crate) fn recovery_not_found(
        &mut self,
        claim: &ClaimedCommand,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        if claim.kind != ClaimKind::RecoveryProbe {
            return Err(StoreError::InvalidTransition {
                command_id: claim.envelope.request.command_id.to_string(),
                state: CommandState::Executing,
                target: "recovery_not_found",
            });
        }
        self.finish_claim(
            claim,
            now,
            ClaimCompletion {
                target: CommandState::Queued,
                attempt_state: "not_found",
                reason: "downstream recovery probe found no prior command",
                next_attempt_at: now,
                result_json: None,
                last_error: None,
                resolution_note: None,
                manual_fallback: None,
            },
        )
    }

    pub(crate) fn require_manual_review(
        &mut self,
        claim: &ClaimedCommand,
        reason: &str,
        agent: &ActorId,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let reason = truncate_message(reason);
        self.finish_claim(
            claim,
            now,
            ClaimCompletion {
                target: CommandState::ManualReview,
                attempt_state: "manual_review",
                reason: &reason,
                next_attempt_at: now,
                result_json: None,
                last_error: Some(reason.clone()),
                resolution_note: None,
                manual_fallback: Some(ManualFallback {
                    actor: agent.as_str(),
                    reason: &reason,
                }),
            },
        )
    }

    fn finish_claim(
        &mut self,
        claim: &ClaimedCommand,
        now: DateTime<Utc>,
        completion: ClaimCompletion<'_>,
    ) -> Result<(), StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            r#"
            UPDATE edge_commands
               SET state = ?1,
                   updated_at_ms = ?2,
                   next_attempt_at_ms = ?3,
                   lease_token = NULL,
                   lease_until_ms = NULL,
                   result_json = ?4,
                   last_error = ?5,
                   resolution_note = ?6
             WHERE command_id = ?7
               AND state = 'executing'
               AND lease_token = ?8
            "#,
            params![
                completion.target.as_str(),
                now.timestamp_millis(),
                completion.next_attempt_at.timestamp_millis(),
                completion.result_json,
                completion.last_error,
                completion.resolution_note,
                claim.envelope.request.command_id.as_str(),
                claim.lease_token,
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::LeaseMismatch(
                claim.envelope.request.command_id.to_string(),
            ));
        }
        let attempt_changed = tx.execute(
            r#"
            UPDATE edge_command_attempts
               SET state = ?1,
                   finished_at_ms = ?2,
                   message = ?3,
                   result_json = ?4
             WHERE attempt_id = ?5
               AND command_id = ?6
               AND state = 'active'
            "#,
            params![
                completion.attempt_state,
                now.timestamp_millis(),
                completion.reason,
                completion.result_json,
                claim.attempt_id,
                claim.envelope.request.command_id.as_str(),
            ],
        )?;
        if attempt_changed != 1 {
            return Err(StoreError::LeaseMismatch(
                claim.envelope.request.command_id.to_string(),
            ));
        }
        append_command_event(
            &tx,
            claim.envelope.request.command_id.as_str(),
            Some(CommandState::Executing),
            completion.target,
            None,
            completion.reason,
            now,
        )?;
        if let Some(fallback) = completion.manual_fallback {
            force_manual_fallback(
                &tx,
                &claim.envelope.request.device_id,
                fallback.actor,
                fallback.reason,
                now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn load_device(
    connection: &Connection,
    device_id: &DeviceId,
) -> Result<Option<RawDevice>, rusqlite::Error> {
    connection
        .query_row(
            r#"
            SELECT device_id, tenant_id, facility_id, device_class, display_name,
                   control_mode, control_reason, control_actor, control_changed_at_ms,
                   health_state, health_message, last_heartbeat_at_ms,
                   consecutive_health_failures
              FROM edge_devices
             WHERE device_id = ?1
            "#,
            [device_id.as_str()],
            raw_device_from_row,
        )
        .optional()
}

fn raw_device_from_row(row: &Row<'_>) -> Result<RawDevice, rusqlite::Error> {
    Ok(RawDevice {
        device_id: row.get(0)?,
        tenant_id: row.get(1)?,
        facility_id: row.get(2)?,
        device_class: row.get(3)?,
        display_name: row.get(4)?,
        control_mode: row.get(5)?,
        control_reason: row.get(6)?,
        control_actor: row.get(7)?,
        control_changed_at_ms: row.get(8)?,
        health_state: row.get(9)?,
        health_message: row.get(10)?,
        last_heartbeat_at_ms: row.get(11)?,
        consecutive_health_failures: row.get(12)?,
    })
}

fn decode_device(raw: RawDevice) -> Result<DeviceStatus, StoreError> {
    Ok(DeviceStatus {
        descriptor: DeviceDescriptor {
            tenant_id: TenantId::new(raw.tenant_id)?,
            facility_id: FacilityId::new(raw.facility_id)?,
            device_id: DeviceId::new(raw.device_id)?,
            class: DeviceClass::parse_storage(&raw.device_class)?,
            display_name: raw.display_name,
        },
        control_mode: ControlMode::parse_storage(&raw.control_mode)?,
        control_reason: raw.control_reason,
        control_actor: ActorId::new(raw.control_actor)?,
        control_changed_at: timestamp(raw.control_changed_at_ms)?,
        health: HealthState::parse_storage(&raw.health_state)?,
        health_message: raw.health_message,
        last_heartbeat_at: raw.last_heartbeat_at_ms.map(timestamp).transpose()?,
        consecutive_health_failures: checked_u32(
            raw.consecutive_health_failures,
            "health failure count",
        )?,
    })
}

fn load_raw_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<RawCommand>, rusqlite::Error> {
    connection
        .query_row(
            r#"
            SELECT request_hash, request_json, state, attempt_count, created_at_ms,
                   updated_at_ms, next_attempt_at_ms, result_json, last_error,
                   resolution_note
              FROM edge_commands
             WHERE command_id = ?1
            "#,
            [command_id],
            raw_command_from_row,
        )
        .optional()
}

fn raw_command_from_row(row: &Row<'_>) -> Result<RawCommand, rusqlite::Error> {
    Ok(RawCommand {
        request_hash: row.get(0)?,
        request_json: row.get(1)?,
        state: row.get(2)?,
        attempt_count: row.get(3)?,
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
        next_attempt_at_ms: row.get(6)?,
        result_json: row.get(7)?,
        last_error: row.get(8)?,
        resolution_note: row.get(9)?,
    })
}

fn decode_command(raw: RawCommand) -> Result<CommandRecord, StoreError> {
    let request: CommandRequest = serde_json::from_slice(&raw.request_json)?;
    request.validate()?;
    if raw.request_hash.as_slice() != request.request_hash()? {
        return Err(StoreError::CorruptRecord(
            "stored request hash does not match immutable command content".into(),
        ));
    }
    let result: Option<CommandResult> = raw
        .result_json
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()?;
    if let Some(result) = &result {
        result.validate_for(&request.command)?;
    }
    Ok(CommandRecord {
        request,
        state: CommandState::parse_storage(&raw.state)?,
        attempt_count: checked_u32(raw.attempt_count, "command attempt count")?,
        created_at: timestamp(raw.created_at_ms)?,
        updated_at: timestamp(raw.updated_at_ms)?,
        next_attempt_at: timestamp(raw.next_attempt_at_ms)?,
        result,
        last_error: raw.last_error,
        resolution_note: raw.resolution_note,
    })
}

fn command_identity_candidates(
    tx: &Transaction<'_>,
    request: &CommandRequest,
) -> Result<BTreeSet<String>, rusqlite::Error> {
    let mut statement = tx.prepare(
        r#"
        SELECT command_id
          FROM edge_commands
         WHERE command_id = ?1
            OR (
                tenant_id = ?2
                AND facility_id = ?3
                AND device_id = ?4
                AND (correlation_id = ?5 OR idempotency_key = ?6)
            )
        "#,
    )?;
    let candidates = statement
        .query_map(
            params![
                request.command_id.as_str(),
                request.tenant_id.as_str(),
                request.facility_id.as_str(),
                request.device_id.as_str(),
                request.correlation_id.as_str(),
                request.idempotency_key.as_str(),
            ],
            |row| row.get(0),
        )?
        .collect();
    candidates
}

fn append_command_event(
    tx: &Transaction<'_>,
    command_id: &str,
    from_state: Option<CommandState>,
    to_state: CommandState,
    actor: Option<&str>,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        r#"
        INSERT INTO edge_command_events (
            command_id, from_state, to_state, actor, reason, occurred_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![
            command_id,
            from_state.map(CommandState::as_str),
            to_state.as_str(),
            actor,
            reason,
            now.timestamp_millis(),
        ],
    )?;
    Ok(())
}

fn transition_unleased(
    tx: &Transaction<'_>,
    now: DateTime<Utc>,
    transition: UnleasedTransition<'_>,
) -> Result<(), StoreError> {
    let changed = tx.execute(
        r#"
        UPDATE edge_commands
           SET state = ?1,
               updated_at_ms = ?2,
               next_attempt_at_ms = COALESCE(?3, next_attempt_at_ms),
               last_error = CASE WHEN ?1 = 'queued' THEN NULL ELSE last_error END,
               resolution_note = COALESCE(?4, resolution_note)
         WHERE command_id = ?5
           AND state = ?6
           AND lease_token IS NULL
        "#,
        params![
            transition.to.as_str(),
            now.timestamp_millis(),
            transition
                .next_attempt_at
                .map(|value| value.timestamp_millis()),
            transition.resolution_note,
            transition.command_id,
            transition.from.as_str(),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::InvalidTransition {
            command_id: transition.command_id.to_owned(),
            state: transition.from,
            target: transition.to.as_str(),
        });
    }
    append_command_event(
        tx,
        transition.command_id,
        Some(transition.from),
        transition.to,
        transition.actor,
        transition.reason,
        now,
    )?;
    Ok(())
}

fn transition_leased_without_token(
    tx: &Transaction<'_>,
    command_id: &str,
    to: CommandState,
    actor: Option<&str>,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let changed = tx.execute(
        r#"
        UPDATE edge_commands
           SET state = ?1,
               updated_at_ms = ?2,
               next_attempt_at_ms = ?2,
               lease_token = NULL,
               lease_until_ms = NULL,
               last_error = ?3
         WHERE command_id = ?4
           AND state = 'executing'
        "#,
        params![to.as_str(), now.timestamp_millis(), reason, command_id],
    )?;
    if changed != 1 {
        return Err(StoreError::LeaseMismatch(command_id.to_owned()));
    }
    append_command_event(
        tx,
        command_id,
        Some(CommandState::Executing),
        to,
        actor,
        reason,
        now,
    )?;
    Ok(())
}

fn abandon_active_attempt(
    tx: &Transaction<'_>,
    command_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let changed = tx.execute(
        r#"
        UPDATE edge_command_attempts
           SET state = 'abandoned', finished_at_ms = ?1, message = ?2
         WHERE command_id = ?3 AND state = 'active'
        "#,
        params![now.timestamp_millis(), reason, command_id],
    )?;
    if changed != 1 {
        return Err(StoreError::CorruptRecord(format!(
            "executing command {command_id} does not have exactly one active attempt"
        )));
    }
    Ok(())
}

fn quarantine_device_commands(
    tx: &Transaction<'_>,
    device_id: &DeviceId,
    actor: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let commands = {
        let mut statement = tx.prepare(
            r#"
            SELECT command_id, state
              FROM edge_commands
             WHERE device_id = ?1
               AND state IN ('queued', 'executing', 'retry_wait', 'recovery_wait')
             ORDER BY created_at_ms, command_id
            "#,
        )?;
        let rows = statement
            .query_map([device_id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (command_id, state) in commands {
        let from = CommandState::parse_storage(&state)?;
        if from == CommandState::Executing {
            abandon_active_attempt(tx, &command_id, reason, now)?;
            transition_leased_without_token(
                tx,
                &command_id,
                CommandState::ManualReview,
                Some(actor),
                reason,
                now,
            )?;
        } else {
            transition_unleased(
                tx,
                now,
                UnleasedTransition {
                    command_id: &command_id,
                    from,
                    to: CommandState::ManualReview,
                    actor: Some(actor),
                    reason,
                    next_attempt_at: None,
                    resolution_note: None,
                },
            )?;
        }
    }
    Ok(())
}

fn update_device_mode(
    tx: &Transaction<'_>,
    existing: &DeviceStatus,
    target: ControlMode,
    actor: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        r#"
        UPDATE edge_devices
           SET control_mode = ?1,
               control_reason = ?2,
               control_actor = ?3,
               control_changed_at_ms = ?4,
               updated_at_ms = ?4
         WHERE device_id = ?5
        "#,
        params![
            target.as_str(),
            reason,
            actor,
            now.timestamp_millis(),
            existing.descriptor.device_id.as_str(),
        ],
    )?;
    tx.execute(
        r#"
        INSERT INTO edge_control_events (
            tenant_id, facility_id, device_id, from_mode, to_mode, actor, reason,
            occurred_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            existing.descriptor.tenant_id.as_str(),
            existing.descriptor.facility_id.as_str(),
            existing.descriptor.device_id.as_str(),
            existing.control_mode.as_str(),
            target.as_str(),
            actor,
            reason,
            now.timestamp_millis(),
        ],
    )?;
    Ok(())
}

fn force_manual_fallback(
    tx: &Transaction<'_>,
    device_id: &DeviceId,
    actor: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let existing = decode_device(
        load_device(tx, device_id)?.ok_or_else(|| StoreError::DeviceNotFound(device_id.clone()))?,
    )?;
    quarantine_device_commands(tx, device_id, actor, reason, now)?;
    if existing.control_mode != ControlMode::ManualFallback {
        update_device_mode(
            tx,
            &existing,
            ControlMode::ManualFallback,
            actor,
            reason,
            now,
        )?;
    }
    Ok(())
}

fn timestamp(milliseconds: i64) -> Result<DateTime<Utc>, StoreError> {
    DateTime::from_timestamp_millis(milliseconds).ok_or_else(|| {
        StoreError::CorruptRecord(format!("invalid timestamp milliseconds: {milliseconds}"))
    })
}

fn checked_u32(value: i64, field: &str) -> Result<u32, StoreError> {
    u32::try_from(value)
        .map_err(|_| StoreError::CorruptRecord(format!("{field} does not fit in u32")))
}

fn duration_millis(duration: Duration) -> Result<i64, StoreError> {
    i64::try_from(duration.as_millis()).map_err(|_| StoreError::DurationOverflow)
}

fn truncate_message(message: &str) -> String {
    let mut message = message.trim().to_owned();
    if message.is_empty() {
        message = "no detail supplied".into();
    }
    message.truncate(MAX_PERSISTED_MESSAGE_LENGTH);
    message
}

#[cfg(test)]
mod tests;
