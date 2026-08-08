use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::picking::PickingCommand;
use crate::wire::{
    DurableHttpRequest, HttpMethod, ResponseKind, WireRequestError, build_durable_request,
};
use crate::workflow::{
    CycleCountCommand, DurableCommandDraft, InventoryRelocationCommand, PutawayCommand, RfCommand,
};

mod schema;

const STORE_SCHEMA_VERSION: i64 = 3;
const MAX_ERROR_LENGTH: usize = 1_000;
const MAX_SERVER_URL_LENGTH: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceProfile {
    pub device_id: String,
    pub server_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionScope {
    pub tenant_id: i64,
    pub operator_id: i64,
    pub device_id: String,
}

impl ExecutionScope {
    fn validate(&self) -> Result<(), CommandStoreError> {
        if self.tenant_id <= 0 || self.operator_id <= 0 {
            return Err(CommandStoreError::InvalidScope);
        }
        if !valid_device_id(&self.device_id) {
            return Err(CommandStoreError::InvalidScope);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOperation {
    ClaimNext,
    ClaimById,
    ConfirmLoose,
    ConfirmLicensePlate,
    Release,
    ExpectedReceiptConfirmation,
    CycleCountConfirmation,
    PickConfirmation,
}

impl CommandOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ClaimNext => "claim_next",
            Self::ClaimById => "claim_by_id",
            Self::ConfirmLoose => "confirm_loose",
            Self::ConfirmLicensePlate => "confirm_license_plate",
            Self::Release => "release",
            Self::ExpectedReceiptConfirmation => "expected_receipt_confirmation",
            Self::CycleCountConfirmation => "cycle_count_confirmation",
            Self::PickConfirmation => "pick_confirmation",
        }
    }

    fn parse(value: &str) -> Result<Self, CommandStoreError> {
        match value {
            "claim_next" => Ok(Self::ClaimNext),
            "claim_by_id" => Ok(Self::ClaimById),
            "confirm_loose" => Ok(Self::ConfirmLoose),
            "confirm_license_plate" => Ok(Self::ConfirmLicensePlate),
            "release" => Ok(Self::Release),
            "expected_receipt_confirmation" => Ok(Self::ExpectedReceiptConfirmation),
            "cycle_count_confirmation" => Ok(Self::CycleCountConfirmation),
            "pick_confirmation" => Ok(Self::PickConfirmation),
            _ => Err(CommandStoreError::CorruptRecord(
                "unknown command operation".into(),
            )),
        }
    }
}

impl From<&RfCommand> for CommandOperation {
    fn from(command: &RfCommand) -> Self {
        match command {
            RfCommand::Putaway(PutawayCommand::ClaimNext { .. })
            | RfCommand::InventoryRelocation(InventoryRelocationCommand::ClaimNext { .. }) => {
                Self::ClaimNext
            }
            RfCommand::Putaway(PutawayCommand::ClaimById { .. })
            | RfCommand::InventoryRelocation(InventoryRelocationCommand::ClaimById { .. }) => {
                Self::ClaimById
            }
            RfCommand::Putaway(PutawayCommand::ConfirmLoose { .. })
            | RfCommand::InventoryRelocation(InventoryRelocationCommand::ConfirmLoose { .. }) => {
                Self::ConfirmLoose
            }
            RfCommand::Putaway(PutawayCommand::ConfirmLicensePlate { .. })
            | RfCommand::InventoryRelocation(InventoryRelocationCommand::ConfirmLicensePlate {
                ..
            }) => Self::ConfirmLicensePlate,
            RfCommand::Putaway(PutawayCommand::Release { .. })
            | RfCommand::InventoryRelocation(InventoryRelocationCommand::Release { .. }) => {
                Self::Release
            }
            RfCommand::ExpectedReceipt(_) => Self::ExpectedReceiptConfirmation,
            RfCommand::CycleCount(CycleCountCommand::ClaimNext) => Self::ClaimNext,
            RfCommand::CycleCount(CycleCountCommand::ClaimById { .. }) => Self::ClaimById,
            RfCommand::CycleCount(CycleCountCommand::Confirm { .. }) => {
                Self::CycleCountConfirmation
            }
            RfCommand::CycleCount(CycleCountCommand::Release { .. }) => Self::Release,
            RfCommand::Picking(PickingCommand::ClaimNext) => Self::ClaimNext,
            RfCommand::Picking(PickingCommand::ClaimById { .. }) => Self::ClaimById,
            RfCommand::Picking(PickingCommand::Confirm { .. }) => Self::PickConfirmation,
            RfCommand::Picking(PickingCommand::Release { .. }) => Self::Release,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Persisted,
    Dispatching,
    Ambiguous,
    Retryable,
    ResponseRecorded,
    ReconcileRequired,
    Completed,
    Rejected,
}

impl CommandStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Persisted => "persisted",
            Self::Dispatching => "dispatching",
            Self::Ambiguous => "ambiguous",
            Self::Retryable => "retryable",
            Self::ResponseRecorded => "response_recorded",
            Self::ReconcileRequired => "reconcile_required",
            Self::Completed => "completed",
            Self::Rejected => "rejected",
        }
    }

    fn parse(value: &str) -> Result<Self, CommandStoreError> {
        match value {
            "persisted" => Ok(Self::Persisted),
            "dispatching" => Ok(Self::Dispatching),
            "ambiguous" => Ok(Self::Ambiguous),
            "retryable" => Ok(Self::Retryable),
            "response_recorded" => Ok(Self::ResponseRecorded),
            "reconcile_required" => Ok(Self::ReconcileRequired),
            "completed" => Ok(Self::Completed),
            "rejected" => Ok(Self::Rejected),
            _ => Err(CommandStoreError::CorruptRecord(
                "unknown command status".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableCommandRecord {
    pub record_id: i64,
    pub scope: ExecutionScope,
    pub operation: CommandOperation,
    pub draft: DurableCommandDraft,
    pub request: DurableHttpRequest,
    pub status: CommandStatus,
    pub attempt_count: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_error: Option<String>,
    pub response: Option<DurableHttpResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchAttempt {
    pub attempt_id: String,
    pub request_id: String,
    pub ordinal: i64,
    pub command: DurableCommandRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub server_request_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum CommandStoreError {
    #[error("device command database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Wire(#[from] WireRequestError),
    #[error("could not encode or decode a durable command: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the device command database schema version {0} is unsupported")]
    UnsupportedSchema(i64),
    #[error("tenant, operator, and device scope must be valid")]
    InvalidScope,
    #[error("server URL must be an HTTP(S) endpoint without credentials, query, or fragment")]
    InvalidServerUrl,
    #[error("server URL cannot change while this device has unresolved commands")]
    ServerUrlChangeBlocked,
    #[error("the command identity already belongs to different immutable content")]
    IdentityConflict,
    #[error("another unresolved command already exists on this device")]
    UnresolvedCommandExists,
    #[error("durable command {0} does not exist")]
    NotFound(i64),
    #[error("durable command scope does not match the authenticated device scope")]
    ScopeMismatch,
    #[error("durable command {record_id} cannot transition from {status:?} to {target}")]
    InvalidTransition {
        record_id: i64,
        status: CommandStatus,
        target: &'static str,
    },
    #[error("dispatch attempt {attempt_id} is not active for durable command {record_id}")]
    AttemptMismatch { record_id: i64, attempt_id: String },
    #[error("durable command record is corrupt: {0}")]
    CorruptRecord(String),
    #[error("HTTP status {0} is not a known retryable outcome")]
    NonRetryableHttpStatus(u16),
    #[error("HTTP status {0} must be recorded as a retryable outcome")]
    RetryableHttpStatus(u16),
}

pub struct CommandStore {
    connection: Connection,
}

impl CommandStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CommandStoreError> {
        let connection = Connection::open(path)?;
        Self::configure(connection, true)
    }

    pub fn open_in_memory() -> Result<Self, CommandStoreError> {
        let connection = Connection::open_in_memory()?;
        Self::configure(connection, false)
    }

    fn configure(mut connection: Connection, persistent: bool) -> Result<Self, CommandStoreError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        if persistent {
            let _: String =
                connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }

        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == 0 {
            schema::create(&mut connection)?;
        } else if version != STORE_SCHEMA_VERSION {
            return Err(CommandStoreError::UnsupportedSchema(version));
        }

        let mut store = Self { connection };
        store.recover_interrupted_dispatches()?;
        Ok(store)
    }

    pub fn device_profile(&self) -> Result<DeviceProfile, CommandStoreError> {
        load_device_profile(&self.connection)
    }

    pub fn server_url(&self) -> Result<Option<String>, CommandStoreError> {
        Ok(self.device_profile()?.server_url)
    }

    pub fn set_server_url(
        &mut self,
        server_url: Option<&str>,
    ) -> Result<DeviceProfile, CommandStoreError> {
        let server_url = normalize_server_url(server_url)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let profile = load_device_profile(&tx)?;
        if profile.server_url == server_url {
            tx.commit()?;
            return Ok(profile);
        }
        let unresolved_exists = tx
            .query_row(
                r#"
                SELECT 1
                FROM rf_commands
                WHERE device_id = ?1
                  AND status IN (
                      'persisted',
                      'dispatching',
                      'ambiguous',
                      'retryable',
                      'response_recorded',
                      'reconcile_required'
                  )
                LIMIT 1
                "#,
                [&profile.device_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if unresolved_exists {
            return Err(CommandStoreError::ServerUrlChangeBlocked);
        }
        let changed = tx.execute(
            r#"
            UPDATE rf_device_profile
            SET server_url = ?1
            WHERE singleton_id = 1
            "#,
            [server_url],
        )?;
        if changed != 1 {
            return Err(CommandStoreError::CorruptRecord(
                "device profile is missing".into(),
            ));
        }
        let profile = load_device_profile(&tx)?;
        tx.commit()?;
        Ok(profile)
    }

    pub fn persist(
        &mut self,
        scope: &ExecutionScope,
        draft: DurableCommandDraft,
    ) -> Result<DurableCommandRecord, CommandStoreError> {
        scope.validate()?;
        let request = build_durable_request(&draft)?;
        let operation = CommandOperation::from(&draft.command);
        let draft_json = serde_json::to_vec(&draft)?;
        let now = now_ms();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing_id) = tx
            .query_row(
                "SELECT record_id FROM rf_commands WHERE command_id = ?1",
                [&draft.command_id],
                |row| row.get(0),
            )
            .optional()?
        {
            let existing = load_record(&tx, existing_id)?;
            if existing.scope == *scope
                && existing.operation == operation
                && existing.draft == draft
                && existing.request == request
            {
                tx.commit()?;
                return Ok(existing);
            }
            return Err(CommandStoreError::IdentityConflict);
        }

        if tx
            .query_row(
                r#"
                SELECT 1
                FROM rf_commands
                WHERE device_id = ?1
                  AND status IN (
                      'persisted',
                      'dispatching',
                      'ambiguous',
                      'retryable',
                      'response_recorded',
                      'reconcile_required'
                )
                LIMIT 1
                "#,
                [&scope.device_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
        {
            return Err(CommandStoreError::UnresolvedCommandExists);
        }

        tx.execute(
            r#"
            INSERT INTO rf_commands (
                command_id,
                tenant_id,
                operator_id,
                device_id,
                schema_version,
                operation,
                idempotency_key,
                draft_json,
                http_method,
                path,
                content_type,
                body,
                body_sha256,
                response_kind,
                status,
                created_at_ms,
                updated_at_ms
            )
            VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                'persisted', ?15, ?15
            )
            "#,
            params![
                draft.command_id,
                scope.tenant_id,
                scope.operator_id,
                scope.device_id,
                draft.schema_version,
                operation.as_str(),
                draft.idempotency_key,
                draft_json,
                method_name(request.method),
                request.path,
                request.content_type,
                request.body,
                request.body_sha256.as_slice(),
                response_kind_name(request.response_kind),
                now,
            ],
        )
        .map_err(|error| map_insert_error(error, &tx, scope, operation, &draft.idempotency_key))?;
        let record_id = tx.last_insert_rowid();
        let record = load_record(&tx, record_id)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn begin_attempt(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
    ) -> Result<DispatchAttempt, CommandStoreError> {
        scope.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_record(&tx, record_id)?;
        require_scope(&record, scope)?;
        if !matches!(
            record.status,
            CommandStatus::Persisted | CommandStatus::Ambiguous | CommandStatus::Retryable
        ) {
            return Err(CommandStoreError::InvalidTransition {
                record_id,
                status: record.status,
                target: "dispatching",
            });
        }
        if !record.request.verify_body() {
            return Err(CommandStoreError::CorruptRecord(
                "request body hash does not match".into(),
            ));
        }

        let ordinal = record
            .attempt_count
            .checked_add(1)
            .ok_or_else(|| CommandStoreError::CorruptRecord("attempt count overflow".into()))?;
        let attempt_id = Uuid::new_v4().to_string();
        let request_id = format!("rf-{}", Uuid::new_v4());
        let now = now_ms();
        tx.execute(
            r#"
            INSERT INTO rf_command_attempts (
                attempt_id,
                record_id,
                ordinal,
                request_id,
                started_at_ms,
                status
            )
            VALUES (?1, ?2, ?3, ?4, ?5, 'dispatching')
            "#,
            params![attempt_id, record_id, ordinal, request_id, now],
        )?;
        tx.execute(
            r#"
            UPDATE rf_commands
            SET status = 'dispatching',
                attempt_count = ?1,
                updated_at_ms = ?2,
                last_error = NULL
            WHERE record_id = ?3
            "#,
            params![ordinal, now, record_id],
        )?;
        let command = load_record(&tx, record_id)?;
        tx.commit()?;
        Ok(DispatchAttempt {
            attempt_id,
            request_id,
            ordinal,
            command,
        })
    }

    pub fn mark_ambiguous(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        attempt_id: &str,
        message: &str,
    ) -> Result<DurableCommandRecord, CommandStoreError> {
        scope.validate()?;
        let message = bounded(message);
        let now = now_ms();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_record(&tx, record_id)?;
        require_scope(&record, scope)?;
        if record.status != CommandStatus::Dispatching {
            return Err(CommandStoreError::InvalidTransition {
                record_id,
                status: record.status,
                target: "ambiguous",
            });
        }
        let changed = tx.execute(
            r#"
            UPDATE rf_command_attempts
            SET status = 'ambiguous', finished_at_ms = ?1, error_message = ?2
            WHERE attempt_id = ?3 AND record_id = ?4 AND status = 'dispatching'
            "#,
            params![now, message, attempt_id, record_id],
        )?;
        if changed != 1 {
            return Err(CommandStoreError::AttemptMismatch {
                record_id,
                attempt_id: attempt_id.to_owned(),
            });
        }
        tx.execute(
            r#"
            UPDATE rf_commands
            SET status = 'ambiguous', updated_at_ms = ?1, last_error = ?2
            WHERE record_id = ?3
            "#,
            params![now, message, record_id],
        )?;
        let record = load_record(&tx, record_id)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn record_response(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        attempt_id: &str,
        response: &DurableHttpResponse,
    ) -> Result<DurableCommandRecord, CommandStoreError> {
        scope.validate()?;
        if is_retryable_http_status(response.status) {
            return Err(CommandStoreError::RetryableHttpStatus(response.status));
        }
        let now = now_ms();
        let response_sha256 = Sha256::digest(&response.body);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_record(&tx, record_id)?;
        require_scope(&record, scope)?;
        if record.status != CommandStatus::Dispatching {
            return Err(CommandStoreError::InvalidTransition {
                record_id,
                status: record.status,
                target: "response_recorded",
            });
        }
        let changed = tx.execute(
            r#"
            UPDATE rf_command_attempts
            SET status = 'responded',
                finished_at_ms = ?1,
                http_status = ?2,
                server_request_id = ?3
            WHERE attempt_id = ?4 AND record_id = ?5 AND status = 'dispatching'
            "#,
            params![
                now,
                i64::from(response.status),
                response.server_request_id,
                attempt_id,
                record_id,
            ],
        )?;
        if changed != 1 {
            return Err(CommandStoreError::AttemptMismatch {
                record_id,
                attempt_id: attempt_id.to_owned(),
            });
        }
        tx.execute(
            r#"
            UPDATE rf_commands
            SET status = 'response_recorded',
                updated_at_ms = ?1,
                last_error = NULL,
                response_status = ?2,
                response_body = ?3,
                response_sha256 = ?4,
                server_request_id = ?5
            WHERE record_id = ?6
            "#,
            params![
                now,
                i64::from(response.status),
                response.body,
                response_sha256.as_slice(),
                response.server_request_id,
                record_id,
            ],
        )?;
        let record = load_record(&tx, record_id)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn record_retryable_response(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        attempt_id: &str,
        response: &DurableHttpResponse,
    ) -> Result<DurableCommandRecord, CommandStoreError> {
        scope.validate()?;
        if !is_retryable_http_status(response.status) {
            return Err(CommandStoreError::NonRetryableHttpStatus(response.status));
        }
        let message = format!("HTTP {} response is retryable", response.status);
        let now = now_ms();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_record(&tx, record_id)?;
        require_scope(&record, scope)?;
        if record.status != CommandStatus::Dispatching {
            return Err(CommandStoreError::InvalidTransition {
                record_id,
                status: record.status,
                target: "retryable",
            });
        }
        let changed = tx.execute(
            r#"
            UPDATE rf_command_attempts
            SET status = 'retryable',
                finished_at_ms = ?1,
                http_status = ?2,
                server_request_id = ?3,
                error_message = ?4
            WHERE attempt_id = ?5 AND record_id = ?6 AND status = 'dispatching'
            "#,
            params![
                now,
                i64::from(response.status),
                response.server_request_id,
                message,
                attempt_id,
                record_id,
            ],
        )?;
        if changed != 1 {
            return Err(CommandStoreError::AttemptMismatch {
                record_id,
                attempt_id: attempt_id.to_owned(),
            });
        }
        tx.execute(
            r#"
            UPDATE rf_commands
            SET status = 'retryable', updated_at_ms = ?1, last_error = ?2
            WHERE record_id = ?3
            "#,
            params![now, message, record_id],
        )?;
        let record = load_record(&tx, record_id)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn finalize(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        status: CommandStatus,
        message: Option<&str>,
    ) -> Result<DurableCommandRecord, CommandStoreError> {
        scope.validate()?;
        if !matches!(
            status,
            CommandStatus::Completed | CommandStatus::Rejected | CommandStatus::ReconcileRequired
        ) {
            return Err(CommandStoreError::InvalidTransition {
                record_id,
                status,
                target: "terminal status",
            });
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_record(&tx, record_id)?;
        require_scope(&record, scope)?;
        if record.status != CommandStatus::ResponseRecorded {
            return Err(CommandStoreError::InvalidTransition {
                record_id,
                status: record.status,
                target: status.as_str(),
            });
        }
        let message = message.map(bounded);
        tx.execute(
            r#"
            UPDATE rf_commands
            SET status = ?1, updated_at_ms = ?2, last_error = ?3
            WHERE record_id = ?4
            "#,
            params![status.as_str(), now_ms(), message, record_id],
        )?;
        let record = load_record(&tx, record_id)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn unresolved(
        &self,
        scope: &ExecutionScope,
    ) -> Result<Vec<DurableCommandRecord>, CommandStoreError> {
        scope.validate()?;
        let mut statement = self.connection.prepare(
            r#"
            SELECT record_id
            FROM rf_commands
            WHERE tenant_id = ?1
              AND operator_id = ?2
              AND device_id = ?3
              AND status IN (
                  'persisted',
                  'dispatching',
                  'ambiguous',
                  'retryable',
                  'response_recorded',
                  'reconcile_required'
              )
            ORDER BY record_id
            "#,
        )?;
        let ids = statement
            .query_map(
                params![scope.tenant_id, scope.operator_id, scope.device_id],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|record_id| load_record_connection(&self.connection, record_id))
            .collect()
    }

    pub fn unresolved_for_device(
        &self,
        device_id: &str,
    ) -> Result<Vec<DurableCommandRecord>, CommandStoreError> {
        if !valid_device_id(device_id) {
            return Err(CommandStoreError::InvalidScope);
        }
        let mut statement = self.connection.prepare(
            r#"
            SELECT record_id
            FROM rf_commands
            WHERE device_id = ?1
              AND status IN (
                  'persisted',
                  'dispatching',
                  'ambiguous',
                  'retryable',
                  'response_recorded',
                  'reconcile_required'
              )
            ORDER BY record_id
            "#,
        )?;
        let ids = statement
            .query_map([device_id], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|record_id| load_record_connection(&self.connection, record_id))
            .collect()
    }

    fn recover_interrupted_dispatches(&mut self) -> Result<(), CommandStoreError> {
        let now = now_ms();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            r#"
            UPDATE rf_command_attempts
            SET status = 'ambiguous',
                finished_at_ms = ?1,
                error_message = 'application stopped while the request was in flight'
            WHERE status = 'dispatching'
            "#,
            [now],
        )?;
        tx.execute(
            r#"
            UPDATE rf_commands
            SET status = 'ambiguous',
                updated_at_ms = ?1,
                last_error = 'application stopped while the request was in flight'
            WHERE status = 'dispatching'
            "#,
            [now],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[derive(Debug)]
struct RawRecord {
    record_id: i64,
    command_id: String,
    tenant_id: i64,
    operator_id: i64,
    device_id: String,
    operation: String,
    draft_json: Vec<u8>,
    http_method: String,
    path: String,
    content_type: String,
    body: Vec<u8>,
    body_sha256: Vec<u8>,
    response_kind: String,
    status: String,
    attempt_count: i64,
    created_at_ms: i64,
    updated_at_ms: i64,
    last_error: Option<String>,
    response_status: Option<i64>,
    response_body: Option<Vec<u8>>,
    response_sha256: Option<Vec<u8>>,
    server_request_id: Option<String>,
}

fn load_record(
    tx: &Transaction<'_>,
    record_id: i64,
) -> Result<DurableCommandRecord, CommandStoreError> {
    let raw = tx
        .query_row(RECORD_QUERY, [record_id], raw_record)
        .optional()?
        .ok_or(CommandStoreError::NotFound(record_id))?;
    decode_record(raw)
}

fn load_record_connection(
    connection: &Connection,
    record_id: i64,
) -> Result<DurableCommandRecord, CommandStoreError> {
    let raw = connection
        .query_row(RECORD_QUERY, [record_id], raw_record)
        .optional()?
        .ok_or(CommandStoreError::NotFound(record_id))?;
    decode_record(raw)
}

const RECORD_QUERY: &str = r#"
    SELECT
        record_id,
        command_id,
        tenant_id,
        operator_id,
        device_id,
        operation,
        draft_json,
        http_method,
        path,
        content_type,
        body,
        body_sha256,
        response_kind,
        status,
        attempt_count,
        created_at_ms,
        updated_at_ms,
        last_error,
        response_status,
        response_body,
        response_sha256,
        server_request_id
    FROM rf_commands
    WHERE record_id = ?1
"#;

fn raw_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRecord> {
    Ok(RawRecord {
        record_id: row.get(0)?,
        command_id: row.get(1)?,
        tenant_id: row.get(2)?,
        operator_id: row.get(3)?,
        device_id: row.get(4)?,
        operation: row.get(5)?,
        draft_json: row.get(6)?,
        http_method: row.get(7)?,
        path: row.get(8)?,
        content_type: row.get(9)?,
        body: row.get(10)?,
        body_sha256: row.get(11)?,
        response_kind: row.get(12)?,
        status: row.get(13)?,
        attempt_count: row.get(14)?,
        created_at_ms: row.get(15)?,
        updated_at_ms: row.get(16)?,
        last_error: row.get(17)?,
        response_status: row.get(18)?,
        response_body: row.get(19)?,
        response_sha256: row.get(20)?,
        server_request_id: row.get(21)?,
    })
}

fn decode_record(raw: RawRecord) -> Result<DurableCommandRecord, CommandStoreError> {
    let draft: DurableCommandDraft = serde_json::from_slice(&raw.draft_json)?;
    if draft.command_id != raw.command_id {
        return Err(CommandStoreError::CorruptRecord(
            "draft command ID does not match its index".into(),
        ));
    }
    let body_sha256: [u8; 32] = raw
        .body_sha256
        .try_into()
        .map_err(|_| CommandStoreError::CorruptRecord("invalid request body hash".into()))?;
    let method = parse_method(&raw.http_method)?;
    let response_kind = parse_response_kind(&raw.response_kind)?;
    let request = DurableHttpRequest {
        method,
        path: raw.path,
        content_type: raw.content_type,
        body: raw.body,
        body_sha256,
        response_kind,
    };
    if !request.verify_body() {
        return Err(CommandStoreError::CorruptRecord(
            "request body hash does not match".into(),
        ));
    }
    let expected_request = build_durable_request(&draft)?;
    if request != expected_request {
        return Err(CommandStoreError::CorruptRecord(
            "durable request does not match its typed command".into(),
        ));
    }
    let operation = CommandOperation::parse(&raw.operation)?;
    if operation != CommandOperation::from(&draft.command) {
        return Err(CommandStoreError::CorruptRecord(
            "draft operation does not match its index".into(),
        ));
    }
    let status = CommandStatus::parse(&raw.status)?;
    let response = decode_response(
        raw.response_status,
        raw.response_body,
        raw.response_sha256,
        raw.server_request_id,
    )?;
    let response_required = matches!(
        status,
        CommandStatus::ResponseRecorded
            | CommandStatus::ReconcileRequired
            | CommandStatus::Completed
            | CommandStatus::Rejected
    );
    if response_required != response.is_some() {
        return Err(CommandStoreError::CorruptRecord(
            "command status and recorded response do not match".into(),
        ));
    }

    Ok(DurableCommandRecord {
        record_id: raw.record_id,
        scope: ExecutionScope {
            tenant_id: raw.tenant_id,
            operator_id: raw.operator_id,
            device_id: raw.device_id,
        },
        operation,
        draft,
        request,
        status,
        attempt_count: raw.attempt_count,
        created_at_ms: raw.created_at_ms,
        updated_at_ms: raw.updated_at_ms,
        last_error: raw.last_error,
        response,
    })
}

fn decode_response(
    status: Option<i64>,
    body: Option<Vec<u8>>,
    sha256: Option<Vec<u8>>,
    server_request_id: Option<String>,
) -> Result<Option<DurableHttpResponse>, CommandStoreError> {
    match (status, body, sha256) {
        (None, None, None) if server_request_id.is_none() => Ok(None),
        (Some(status), Some(body), Some(sha256)) => {
            let status = u16::try_from(status).map_err(|_| {
                CommandStoreError::CorruptRecord("invalid recorded HTTP status".into())
            })?;
            let expected: [u8; 32] = sha256.try_into().map_err(|_| {
                CommandStoreError::CorruptRecord("invalid response body hash".into())
            })?;
            let actual = Sha256::digest(&body);
            if actual.as_slice() != expected {
                return Err(CommandStoreError::CorruptRecord(
                    "response body hash does not match".into(),
                ));
            }
            Ok(Some(DurableHttpResponse {
                status,
                body,
                server_request_id,
            }))
        }
        _ => Err(CommandStoreError::CorruptRecord(
            "recorded response fields are incomplete".into(),
        )),
    }
}

fn load_device_profile(connection: &Connection) -> Result<DeviceProfile, CommandStoreError> {
    let raw = connection
        .query_row(
            r#"
            SELECT device_id, server_url
            FROM rf_device_profile
            WHERE singleton_id = 1
            "#,
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .ok_or_else(|| CommandStoreError::CorruptRecord("device profile is missing".into()))?;
    decode_device_profile(raw.0, raw.1)
}

fn decode_device_profile(
    device_id: String,
    server_url: Option<String>,
) -> Result<DeviceProfile, CommandStoreError> {
    if !valid_device_id(&device_id) {
        return Err(CommandStoreError::CorruptRecord(
            "device profile contains an invalid device ID".into(),
        ));
    }
    let normalized_server_url = normalize_server_url(server_url.as_deref()).map_err(|_| {
        CommandStoreError::CorruptRecord("device profile contains an invalid server URL".into())
    })?;
    if normalized_server_url != server_url {
        return Err(CommandStoreError::CorruptRecord(
            "device profile contains a non-canonical server URL".into(),
        ));
    }
    Ok(DeviceProfile {
        device_id,
        server_url,
    })
}

fn valid_device_id(device_id: &str) -> bool {
    !device_id.is_empty()
        && device_id.len() <= 128
        && device_id.bytes().all(|byte| byte.is_ascii_graphic())
}

fn normalize_server_url(server_url: Option<&str>) -> Result<Option<String>, CommandStoreError> {
    let Some(server_url) = server_url else {
        return Ok(None);
    };
    let server_url = server_url.trim();
    if server_url.is_empty() || server_url.len() > MAX_SERVER_URL_LENGTH {
        return Err(CommandStoreError::InvalidServerUrl);
    }
    let url = Url::parse(server_url).map_err(|_| CommandStoreError::InvalidServerUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CommandStoreError::InvalidServerUrl);
    }
    Ok(Some(url.as_str().trim_end_matches('/').to_owned()))
}

fn require_scope(
    record: &DurableCommandRecord,
    scope: &ExecutionScope,
) -> Result<(), CommandStoreError> {
    if record.scope != *scope {
        return Err(CommandStoreError::ScopeMismatch);
    }
    Ok(())
}

fn map_insert_error(
    error: rusqlite::Error,
    tx: &Transaction<'_>,
    scope: &ExecutionScope,
    operation: CommandOperation,
    idempotency_key: &str,
) -> CommandStoreError {
    let identity_exists = tx
        .query_row(
            r#"
            SELECT 1
            FROM rf_commands
            WHERE tenant_id = ?1 AND operation = ?2 AND idempotency_key = ?3
            LIMIT 1
            "#,
            params![scope.tenant_id, operation.as_str(), idempotency_key],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .ok()
        .flatten()
        .is_some();
    if identity_exists {
        CommandStoreError::IdentityConflict
    } else {
        CommandStoreError::Database(error)
    }
}

const fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Post => "POST",
    }
}

fn parse_method(value: &str) -> Result<HttpMethod, CommandStoreError> {
    match value {
        "POST" => Ok(HttpMethod::Post),
        _ => Err(CommandStoreError::CorruptRecord(
            "unknown HTTP method".into(),
        )),
    }
}

const fn response_kind_name(kind: ResponseKind) -> &'static str {
    match kind {
        ResponseKind::OptionalClaim => "optional_claim",
        ResponseKind::Claim => "claim",
        ResponseKind::LooseConfirmation => "loose_confirmation",
        ResponseKind::LicensePlateConfirmation => "license_plate_confirmation",
        ResponseKind::Release => "release",
        ResponseKind::RelocationOptionalClaim => "relocation_optional_claim",
        ResponseKind::RelocationClaim => "relocation_claim",
        ResponseKind::RelocationConfirmation => "relocation_confirmation",
        ResponseKind::RelocationRelease => "relocation_release",
        ResponseKind::CycleCountOptionalClaim => "cycle_count_optional_claim",
        ResponseKind::CycleCountClaim => "cycle_count_claim",
        ResponseKind::CycleCountConfirmation => "cycle_count_confirmation",
        ResponseKind::CycleCountRelease => "cycle_count_release",
        ResponseKind::PickOptionalClaim => "pick_optional_claim",
        ResponseKind::PickClaim => "pick_claim",
        ResponseKind::PickConfirmation => "pick_confirmation",
        ResponseKind::PickRelease => "pick_release",
        ResponseKind::ExpectedReceiptConfirmation => "expected_receipt_confirmation",
    }
}

fn parse_response_kind(value: &str) -> Result<ResponseKind, CommandStoreError> {
    match value {
        "optional_claim" => Ok(ResponseKind::OptionalClaim),
        "claim" => Ok(ResponseKind::Claim),
        "loose_confirmation" => Ok(ResponseKind::LooseConfirmation),
        "license_plate_confirmation" => Ok(ResponseKind::LicensePlateConfirmation),
        "release" => Ok(ResponseKind::Release),
        "relocation_optional_claim" => Ok(ResponseKind::RelocationOptionalClaim),
        "relocation_claim" => Ok(ResponseKind::RelocationClaim),
        "relocation_confirmation" => Ok(ResponseKind::RelocationConfirmation),
        "relocation_release" => Ok(ResponseKind::RelocationRelease),
        "cycle_count_optional_claim" => Ok(ResponseKind::CycleCountOptionalClaim),
        "cycle_count_claim" => Ok(ResponseKind::CycleCountClaim),
        "cycle_count_confirmation" => Ok(ResponseKind::CycleCountConfirmation),
        "cycle_count_release" => Ok(ResponseKind::CycleCountRelease),
        "pick_optional_claim" => Ok(ResponseKind::PickOptionalClaim),
        "pick_claim" => Ok(ResponseKind::PickClaim),
        "pick_confirmation" => Ok(ResponseKind::PickConfirmation),
        "pick_release" => Ok(ResponseKind::PickRelease),
        "expected_receipt_confirmation" => Ok(ResponseKind::ExpectedReceiptConfirmation),
        _ => Err(CommandStoreError::CorruptRecord(
            "unknown response kind".into(),
        )),
    }
}

fn bounded(message: &str) -> String {
    message.chars().take(MAX_ERROR_LENGTH).collect()
}

pub const fn is_retryable_http_status(status: u16) -> bool {
    status == 401 || status == 408 || status == 429 || (status >= 500 && status < 600)
}

fn now_ms() -> i64 {
    let elapsed = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed,
        Err(_) => return 0,
    };
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;
