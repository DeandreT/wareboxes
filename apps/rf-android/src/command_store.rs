use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::wire::{
    DurableHttpRequest, HttpMethod, ResponseKind, WireRequestError, build_durable_request,
};
use crate::workflow::{DurableCommandDraft, PutawayCommand};

const STORE_SCHEMA_VERSION: i64 = 1;
const MAX_ERROR_LENGTH: usize = 1_000;

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
        if self.device_id.is_empty()
            || self.device_id.len() > 128
            || !self.device_id.bytes().all(|byte| byte.is_ascii_graphic())
        {
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
}

impl CommandOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ClaimNext => "claim_next",
            Self::ClaimById => "claim_by_id",
            Self::ConfirmLoose => "confirm_loose",
            Self::ConfirmLicensePlate => "confirm_license_plate",
            Self::Release => "release",
        }
    }

    fn parse(value: &str) -> Result<Self, CommandStoreError> {
        match value {
            "claim_next" => Ok(Self::ClaimNext),
            "claim_by_id" => Ok(Self::ClaimById),
            "confirm_loose" => Ok(Self::ConfirmLoose),
            "confirm_license_plate" => Ok(Self::ConfirmLicensePlate),
            "release" => Ok(Self::Release),
            _ => Err(CommandStoreError::CorruptRecord(
                "unknown command operation".into(),
            )),
        }
    }
}

impl From<&PutawayCommand> for CommandOperation {
    fn from(command: &PutawayCommand) -> Self {
        match command {
            PutawayCommand::ClaimNext { .. } => Self::ClaimNext,
            PutawayCommand::ClaimById { .. } => Self::ClaimById,
            PutawayCommand::ConfirmLoose { .. } => Self::ConfirmLoose,
            PutawayCommand::ConfirmLicensePlate { .. } => Self::ConfirmLicensePlate,
            PutawayCommand::Release { .. } => Self::Release,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Persisted,
    Dispatching,
    Ambiguous,
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
    #[error("the command identity already belongs to different immutable content")]
    IdentityConflict,
    #[error("another unresolved putaway command already exists for this device scope")]
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
    #[error("durable command record is corrupt: {0}")]
    CorruptRecord(String),
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

    fn configure(connection: Connection, persistent: bool) -> Result<Self, CommandStoreError> {
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
            Self::create_schema(&connection)?;
        } else if version != STORE_SCHEMA_VERSION {
            return Err(CommandStoreError::UnsupportedSchema(version));
        }

        let mut store = Self { connection };
        store.recover_interrupted_dispatches()?;
        Ok(store)
    }

    fn create_schema(connection: &Connection) -> Result<(), CommandStoreError> {
        connection.execute_batch(
            r#"
            BEGIN IMMEDIATE;

            CREATE TABLE rf_commands (
                record_id INTEGER PRIMARY KEY,
                command_id TEXT NOT NULL UNIQUE,
                tenant_id INTEGER NOT NULL CHECK (tenant_id > 0),
                operator_id INTEGER NOT NULL CHECK (operator_id > 0),
                device_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                operation TEXT NOT NULL,
                idempotency_key TEXT NOT NULL,
                draft_json BLOB NOT NULL,
                http_method TEXT NOT NULL,
                path TEXT NOT NULL,
                content_type TEXT NOT NULL,
                body BLOB NOT NULL,
                body_sha256 BLOB NOT NULL CHECK (length(body_sha256) = 32),
                response_kind TEXT NOT NULL,
                status TEXT NOT NULL,
                attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                last_error TEXT,
                response_status INTEGER,
                response_body BLOB,
                response_sha256 BLOB,
                server_request_id TEXT,
                UNIQUE (tenant_id, operation, idempotency_key)
            );

            CREATE UNIQUE INDEX rf_commands_one_unresolved_putaway
                ON rf_commands (tenant_id, operator_id, device_id)
                WHERE status IN (
                    'persisted',
                    'dispatching',
                    'ambiguous',
                    'response_recorded',
                    'reconcile_required'
                );

            CREATE TABLE rf_command_attempts (
                attempt_id TEXT PRIMARY KEY,
                record_id INTEGER NOT NULL
                    REFERENCES rf_commands(record_id) ON DELETE RESTRICT,
                ordinal INTEGER NOT NULL CHECK (ordinal > 0),
                request_id TEXT NOT NULL UNIQUE,
                started_at_ms INTEGER NOT NULL,
                finished_at_ms INTEGER,
                status TEXT NOT NULL,
                http_status INTEGER,
                server_request_id TEXT,
                error_message TEXT,
                UNIQUE (record_id, ordinal)
            );

            PRAGMA user_version = 1;
            COMMIT;
            "#,
        )?;
        Ok(())
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
                WHERE tenant_id = ?1
                  AND operator_id = ?2
                  AND device_id = ?3
                  AND status IN (
                      'persisted',
                      'dispatching',
                      'ambiguous',
                      'response_recorded',
                      'reconcile_required'
                  )
                LIMIT 1
                "#,
                params![scope.tenant_id, scope.operator_id, scope.device_id],
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
            CommandStatus::Persisted | CommandStatus::Ambiguous
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
            return Err(CommandStoreError::CorruptRecord(
                "active dispatch attempt is missing".into(),
            ));
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
            return Err(CommandStoreError::CorruptRecord(
                "active dispatch attempt is missing".into(),
            ));
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
        last_error
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
    let operation = CommandOperation::parse(&raw.operation)?;
    if operation != CommandOperation::from(&draft.command) {
        return Err(CommandStoreError::CorruptRecord(
            "draft operation does not match its index".into(),
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
        status: CommandStatus::parse(&raw.status)?,
        attempt_count: raw.attempt_count,
        created_at_ms: raw.created_at_ms,
        updated_at_ms: raw.updated_at_ms,
        last_error: raw.last_error,
    })
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
    }
}

fn parse_response_kind(value: &str) -> Result<ResponseKind, CommandStoreError> {
    match value {
        "optional_claim" => Ok(ResponseKind::OptionalClaim),
        "claim" => Ok(ResponseKind::Claim),
        "loose_confirmation" => Ok(ResponseKind::LooseConfirmation),
        "license_plate_confirmation" => Ok(ResponseKind::LicensePlateConfirmation),
        "release" => Ok(ResponseKind::Release),
        _ => Err(CommandStoreError::CorruptRecord(
            "unknown response kind".into(),
        )),
    }
}

fn bounded(message: &str) -> String {
    message.chars().take(MAX_ERROR_LENGTH).collect()
}

fn now_ms() -> i64 {
    let elapsed = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed,
        Err(_) => return 0,
    };
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{PutawayKind, ReleaseReason};

    fn scope() -> ExecutionScope {
        ExecutionScope {
            tenant_id: 7,
            operator_id: 9,
            device_id: "device-11".into(),
        }
    }

    fn draft(command_id: &str, key: &str, command: PutawayCommand) -> DurableCommandDraft {
        DurableCommandDraft {
            schema_version: 1,
            command_id: command_id.into(),
            idempotency_key: key.into(),
            command,
        }
    }

    fn claim_draft(command_id: &str, key: &str) -> DurableCommandDraft {
        draft(
            command_id,
            key,
            PutawayCommand::ClaimNext {
                workflow: PutawayKind::Loose,
            },
        )
    }

    #[test]
    fn command_is_durable_before_an_attempt_can_start() {
        let mut store = CommandStore::open_in_memory().expect("store should open");
        let record = store
            .persist(&scope(), claim_draft("command-1", "key-1"))
            .expect("command should persist");

        assert_eq!(record.status, CommandStatus::Persisted);
        assert_eq!(record.attempt_count, 0);
        let attempt = store
            .begin_attempt(&scope(), record.record_id)
            .expect("persisted command should dispatch");
        assert_eq!(attempt.command.status, CommandStatus::Dispatching);
        assert_eq!(attempt.ordinal, 1);
        assert_ne!(attempt.attempt_id, attempt.request_id);
    }

    #[test]
    fn exact_duplicate_returns_the_original_record() {
        let mut store = CommandStore::open_in_memory().expect("store should open");
        let draft = claim_draft("command-1", "key-1");
        let first = store
            .persist(&scope(), draft.clone())
            .expect("first command should persist");
        let replay = store
            .persist(&scope(), draft)
            .expect("exact command should replay");

        assert_eq!(replay, first);
    }

    #[test]
    fn changed_content_cannot_reuse_a_command_identity() {
        let mut store = CommandStore::open_in_memory().expect("store should open");
        store
            .persist(&scope(), claim_draft("command-1", "key-1"))
            .expect("first command should persist");
        let changed = draft(
            "command-1",
            "key-1",
            PutawayCommand::ClaimNext {
                workflow: PutawayKind::LicensePlate,
            },
        );

        assert!(matches!(
            store.persist(&scope(), changed),
            Err(CommandStoreError::IdentityConflict)
        ));
    }

    #[test]
    fn one_scope_cannot_create_concurrent_unresolved_commands() {
        let mut store = CommandStore::open_in_memory().expect("store should open");
        store
            .persist(&scope(), claim_draft("command-1", "key-1"))
            .expect("first command should persist");

        assert!(matches!(
            store.persist(&scope(), claim_draft("command-2", "key-2")),
            Err(CommandStoreError::UnresolvedCommandExists)
        ));
    }

    #[test]
    fn ambiguous_retry_keeps_the_request_and_changes_attempt_identity() {
        let mut store = CommandStore::open_in_memory().expect("store should open");
        let record = store
            .persist(
                &scope(),
                draft(
                    "command-1",
                    "confirm-key-1",
                    PutawayCommand::ConfirmLoose {
                        task_id: 42,
                        destination_location_barcode: "A-01-02".into(),
                    },
                ),
            )
            .expect("command should persist");
        let first = store
            .begin_attempt(&scope(), record.record_id)
            .expect("first attempt should start");
        store
            .mark_ambiguous(
                &scope(),
                record.record_id,
                &first.attempt_id,
                "connection closed",
            )
            .expect("ambiguous result should persist");
        let retry = store
            .begin_attempt(&scope(), record.record_id)
            .expect("retry should start");

        assert_eq!(retry.command.request, first.command.request);
        assert_eq!(
            retry.command.draft.idempotency_key,
            first.command.draft.idempotency_key
        );
        assert_ne!(retry.attempt_id, first.attempt_id);
        assert_ne!(retry.request_id, first.request_id);
        assert_eq!(retry.ordinal, 2);
    }

    #[test]
    fn response_is_recorded_before_completion() {
        let mut store = CommandStore::open_in_memory().expect("store should open");
        let record = store
            .persist(
                &scope(),
                draft(
                    "release-1",
                    "release-key-1",
                    PutawayCommand::Release {
                        task_id: 42,
                        reason: ReleaseReason::WorkInterrupted,
                        note: None,
                    },
                ),
            )
            .expect("command should persist");
        let attempt = store
            .begin_attempt(&scope(), record.record_id)
            .expect("attempt should start");
        let response = store
            .record_response(
                &scope(),
                record.record_id,
                &attempt.attempt_id,
                &DurableHttpResponse {
                    status: 200,
                    body: br#"{"task_id":42}"#.to_vec(),
                    server_request_id: Some("server-1".into()),
                },
            )
            .expect("response should persist");

        assert_eq!(response.status, CommandStatus::ResponseRecorded);
        let completed = store
            .finalize(&scope(), record.record_id, CommandStatus::Completed, None)
            .expect("recorded response should complete");
        assert_eq!(completed.status, CommandStatus::Completed);
    }

    #[test]
    fn another_scope_cannot_dispatch_a_record() {
        let mut store = CommandStore::open_in_memory().expect("store should open");
        let record = store
            .persist(&scope(), claim_draft("command-1", "key-1"))
            .expect("command should persist");
        let other_scope = ExecutionScope {
            tenant_id: 8,
            ..scope()
        };

        assert!(matches!(
            store.begin_attempt(&other_scope, record.record_id),
            Err(CommandStoreError::ScopeMismatch)
        ));
    }

    #[test]
    fn unfinished_dispatch_is_ambiguous_after_reopen() {
        let path =
            std::env::temp_dir().join(format!("wareboxes-rf-store-{}.sqlite3", Uuid::new_v4()));
        let record_id = {
            let mut store = CommandStore::open(&path).expect("store should open");
            let record = store
                .persist(&scope(), claim_draft("command-1", "key-1"))
                .expect("command should persist");
            store
                .begin_attempt(&scope(), record.record_id)
                .expect("attempt should start");
            record.record_id
        };

        let store = CommandStore::open(&path).expect("store should reopen");
        let unresolved = store
            .unresolved(&scope())
            .expect("unresolved command should load");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].record_id, record_id);
        assert_eq!(unresolved[0].status, CommandStatus::Ambiguous);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    }
}
