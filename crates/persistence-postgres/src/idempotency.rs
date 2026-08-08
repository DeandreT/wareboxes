//! PostgreSQL adapter for durable command idempotency records.

use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::{Postgres, Row, Transaction};
use wareboxes_application::idempotency::{
    CommandRequestHash, NewCommandResult, PreparedCommand, StoredCommandResult,
};
use wareboxes_application::ApplicationError;
use wareboxes_domain::UserId;

use crate::db::{bind_tenant_context, now_iso};
use crate::{PersistenceError, PersistenceResult};

#[derive(Debug, thiserror::Error)]
pub enum CommandIdempotencyError {
    #[error(transparent)]
    Application(#[from] ApplicationError),
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
}

pub type CommandIdempotencyResult<T> = Result<T, CommandIdempotencyError>;

pub async fn load_stored_result(
    tx: &mut Transaction<'_, Postgres>,
    prepared: &PreparedCommand,
) -> CommandIdempotencyResult<Option<StoredCommandResult>> {
    bind_tenant_context(tx, prepared.tenant_id()).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "command-idempotency:{}:{}:{}",
            prepared.tenant_id(),
            prepared.operation().as_str(),
            prepared.idempotency_key()
        ))
        .execute(&mut **tx)
        .await
        .map_err(PersistenceError::from)?;
    let record = sqlx::query(
        r#"
        SELECT request_hash, result_json::TEXT AS result_json, actor_user_id,
               result_schema_version, inventory_transaction_id
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = $2 AND idempotency_key = $3
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await
    .map_err(PersistenceError::from)?;
    record.map(map_stored_result).transpose()
}

fn map_stored_result(
    record: sqlx::postgres::PgRow,
) -> CommandIdempotencyResult<StoredCommandResult> {
    let request_hash = CommandRequestHash::from_stored(
        record
            .try_get::<String, _>("request_hash")
            .map_err(PersistenceError::from)?,
    )?;
    let actor_id = record
        .try_get::<Option<i64>, _>("actor_user_id")
        .map_err(PersistenceError::from)?
        .ok_or_else(|| {
            ApplicationError::Internal("stored command result has no actor attribution".into())
        })
        .and_then(|actor_id| {
            UserId::new(actor_id).map_err(|error| {
                ApplicationError::Internal(format!(
                    "stored command result has invalid actor attribution: {error}"
                ))
            })
        })?;
    let result_schema_version = record
        .try_get::<i32, _>("result_schema_version")
        .map_err(PersistenceError::from)?;
    let result_schema_version = u32::try_from(result_schema_version).map_err(|_| {
        ApplicationError::Internal("stored command result schema version is invalid".into())
    })?;
    let result_json = serde_json::from_str(
        &record
            .try_get::<String, _>("result_json")
            .map_err(PersistenceError::from)?,
    )
    .map_err(|error| {
        ApplicationError::Internal(format!("decoding stored command result: {error}"))
    })?;

    StoredCommandResult::from_persisted(
        request_hash,
        result_json,
        actor_id,
        result_schema_version,
        record
            .try_get("inventory_transaction_id")
            .map_err(PersistenceError::from)?,
    )
    .map_err(CommandIdempotencyError::from)
}

pub async fn insert_result(
    tx: &mut Transaction<'_, Postgres>,
    result: &NewCommandResult,
) -> PersistenceResult<()> {
    bind_tenant_context(tx, result.tenant_id()).await?;
    let result_schema_version = i32::try_from(result.result_schema_version())
        .map_err(|_| PersistenceError::invalid_input("result schema version exceeds i32"))?;
    sqlx::query(
        r#"
        INSERT INTO command_idempotency_records
            (tenant_id, created, operation, idempotency_key, request_hash,
             result_json, inventory_transaction_id, actor_user_id, request_id,
             result_schema_version)
        VALUES ($1, $2, $3, $4, $5, $6::JSONB, $7, $8, $9, $10)
        "#,
    )
    .bind(result.tenant_id().get())
    .bind(now_iso())
    .bind(result.operation().as_str())
    .bind(result.idempotency_key().as_str())
    .bind(result.request_hash().as_str())
    .bind(result.result_json().to_string())
    .bind(result.inventory_transaction_id())
    .bind(result.actor_id().get())
    .bind(result.request_id())
    .bind(result_schema_version)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(async_fn_in_trait)]
pub trait PostgresPreparedCommandExt {
    async fn replayed<T: DeserializeOwned>(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> CommandIdempotencyResult<Option<T>>;

    async fn commit<T: Serialize>(
        &self,
        tx: Transaction<'_, Postgres>,
        result: T,
    ) -> CommandIdempotencyResult<T>;

    async fn commit_with_inventory_transaction<T: Serialize>(
        &self,
        tx: Transaction<'_, Postgres>,
        result: T,
        inventory_transaction_id: Option<i64>,
    ) -> CommandIdempotencyResult<T>;
}

impl PostgresPreparedCommandExt for PreparedCommand {
    async fn replayed<T: DeserializeOwned>(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> CommandIdempotencyResult<Option<T>> {
        load_stored_result(tx, self)
            .await?
            .map(|stored| self.resolve_replay(stored))
            .transpose()
            .map_err(CommandIdempotencyError::from)
    }

    async fn commit<T: Serialize>(
        &self,
        tx: Transaction<'_, Postgres>,
        result: T,
    ) -> CommandIdempotencyResult<T> {
        self.commit_with_inventory_transaction(tx, result, None)
            .await
    }

    async fn commit_with_inventory_transaction<T: Serialize>(
        &self,
        mut tx: Transaction<'_, Postgres>,
        result: T,
        inventory_transaction_id: Option<i64>,
    ) -> CommandIdempotencyResult<T> {
        let completed = self.completed_result(&result, inventory_transaction_id)?;
        insert_result(&mut tx, &completed).await?;
        tx.commit().await.map_err(PersistenceError::from)?;
        Ok(result)
    }
}
