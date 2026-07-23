use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{CommandContext, TenantId};

use crate::db::now_iso;
use crate::error::{AppError, AppResult};

pub(crate) fn require_command_context(
    access: &TenantAccess,
    command: &CommandContext,
) -> AppResult<()> {
    if command.tenant_id == access.tenant_id && command.actor_id == access.user_id {
        Ok(())
    } else {
        Err(AppError::forbidden())
    }
}

fn validate_identity(operation: &str, idempotency_key: &str) -> AppResult<()> {
    if operation.trim().is_empty() {
        return Err(AppError::internal("command operation cannot be blank"));
    }
    if idempotency_key.trim().is_empty() || idempotency_key.len() > 200 {
        return Err(if idempotency_key.trim().is_empty() {
            AppError::idempotency_key_required()
        } else {
            AppError::bad_request("idempotency key cannot exceed 200 characters")
        });
    }
    Ok(())
}

pub(crate) fn request_hash<T: Serialize>(request: &T) -> AppResult<String> {
    let encoded = serde_json::to_vec(request)
        .map_err(|error| AppError::internal(format!("serializing command request: {error}")))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

pub(crate) async fn replayed_result<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    operation: &str,
    idempotency_key: Option<&str>,
    request_hash: &str,
) -> AppResult<Option<T>> {
    let Some(idempotency_key) = idempotency_key else {
        return Ok(None);
    };
    validate_identity(operation, idempotency_key)?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "command-idempotency:{tenant_id}:{operation}:{idempotency_key}"
        ))
        .execute(&mut **tx)
        .await?;
    let record = sqlx::query(
        r#"
        SELECT request_hash, result_json::TEXT AS result_json
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = $2 AND idempotency_key = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(operation)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(record) = record else {
        return Ok(None);
    };

    let stored_hash: String = record.try_get("request_hash")?;
    if stored_hash != request_hash {
        return Err(AppError::idempotency_key_reused());
    }
    let result_json: String = record.try_get("result_json")?;
    serde_json::from_str(&result_json)
        .map(Some)
        .map_err(|error| AppError::internal(format!("decoding stored command result: {error}")))
}

pub(crate) async fn replayed_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    operation: &str,
    idempotency_key: Option<&str>,
    request_hash: &str,
) -> AppResult<Option<i64>> {
    replayed_result(tx, tenant_id, operation, idempotency_key, request_hash).await
}

#[allow(clippy::too_many_arguments)]
async fn record_result<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    operation: &str,
    idempotency_key: &str,
    request_hash: &str,
    result: &T,
    inventory_transaction_id: Option<i64>,
    actor_user_id: Option<i64>,
    request_id: Option<&str>,
) -> AppResult<()> {
    validate_identity(operation, idempotency_key)?;
    let created = now_iso();
    let result_json = serde_json::to_string(result)
        .map_err(|error| AppError::internal(format!("encoding command result: {error}")))?;
    sqlx::query(
        r#"
        INSERT INTO command_idempotency_records
            (tenant_id, created, operation, idempotency_key, request_hash,
             result_json, inventory_transaction_id, actor_user_id, request_id)
        VALUES ($1, $2, $3, $4, $5, $6::JSONB, $7, $8, $9)
        "#,
    )
    .bind(tenant_id.get())
    .bind(created)
    .bind(operation)
    .bind(idempotency_key)
    .bind(request_hash)
    .bind(result_json)
    .bind(inventory_transaction_id)
    .bind(actor_user_id)
    .bind(request_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) struct NewCommandResult<'a, T> {
    pub tenant_id: TenantId,
    pub operation: &'a str,
    pub idempotency_key: &'a str,
    pub request_hash: &'a str,
    pub result: &'a T,
    pub inventory_transaction_id: Option<i64>,
    pub actor_user_id: i64,
}

pub(crate) async fn record_command_result<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &NewCommandResult<'_, T>,
) -> AppResult<()> {
    record_result(
        tx,
        command.tenant_id,
        command.operation,
        command.idempotency_key,
        command.request_hash,
        command.result,
        command.inventory_transaction_id,
        Some(command.actor_user_id),
        None,
    )
    .await
}

pub(crate) struct PreparedCommand<'a> {
    context: &'a CommandContext,
    operation: &'static str,
    idempotency_key: &'a str,
    request_hash: String,
}

impl<'a> PreparedCommand<'a> {
    pub(crate) fn new<T: Serialize>(
        context: &'a CommandContext,
        operation: &'static str,
        request: &T,
    ) -> AppResult<Self> {
        let idempotency_key = context
            .idempotency_key
            .as_deref()
            .ok_or_else(AppError::idempotency_key_required)?;
        validate_identity(operation, idempotency_key)?;
        let request_hash = request_hash(&(context.actor_id.get(), request))?;
        Ok(Self {
            context,
            operation,
            idempotency_key,
            request_hash,
        })
    }

    pub(crate) async fn replayed<T: DeserializeOwned>(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> AppResult<Option<T>> {
        replayed_result(
            tx,
            self.context.tenant_id,
            self.operation,
            Some(self.idempotency_key),
            &self.request_hash,
        )
        .await
    }

    pub(crate) async fn commit<T: Serialize>(
        &self,
        mut tx: sqlx::Transaction<'_, sqlx::Postgres>,
        result: T,
    ) -> AppResult<T> {
        record_result(
            &mut tx,
            self.context.tenant_id,
            self.operation,
            self.idempotency_key,
            &self.request_hash,
            &result,
            None,
            Some(self.context.actor_id.get()),
            Some(&self.context.request_id),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }
}
