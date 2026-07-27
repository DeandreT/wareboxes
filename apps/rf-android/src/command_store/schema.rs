use rusqlite::{Connection, TransactionBehavior};
use uuid::Uuid;

use super::{CommandStoreError, STORE_SCHEMA_VERSION};

pub(super) fn create(connection: &mut Connection) -> Result<(), CommandStoreError> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        r#"
        CREATE TABLE rf_device_profile (
            singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
            device_id TEXT NOT NULL UNIQUE
                CHECK (length(device_id) BETWEEN 1 AND 128),
            server_url TEXT
        );
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
            CHECK (
                (
                    response_status IS NULL
                    AND response_body IS NULL
                    AND response_sha256 IS NULL
                    AND server_request_id IS NULL
                )
                OR (
                    response_status IS NOT NULL
                    AND response_status BETWEEN 0 AND 65535
                    AND response_body IS NOT NULL
                    AND response_sha256 IS NOT NULL
                    AND length(response_sha256) = 32
                )
            ),
            UNIQUE (tenant_id, operation, idempotency_key)
        );
        CREATE UNIQUE INDEX rf_commands_one_unresolved_per_device
            ON rf_commands (device_id)
            WHERE status IN (
                'persisted',
                'dispatching',
                'ambiguous',
                'retryable',
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
        "#,
    )?;
    tx.execute(
        r#"
        INSERT INTO rf_device_profile (singleton_id, device_id, server_url)
        VALUES (1, ?1, NULL)
        "#,
        [Uuid::new_v4().to_string()],
    )?;
    tx.pragma_update(None, "user_version", STORE_SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}
