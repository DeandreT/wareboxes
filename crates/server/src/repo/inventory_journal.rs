//! Immutable inventory journal primitives shared by inventory workflows.

use wareboxes_core::models::{InventoryStatus, InventoryTransactionType};
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId};

use crate::db::now_iso;
use crate::error::{AppError, AppResult};

use super::outbox::{self, NewOutboxEvent};

pub(crate) use super::idempotency::{
    record_command_result, replayed_result, replayed_transaction, request_hash, NewCommandResult,
};

pub(crate) struct JournalCommand<'a> {
    pub tenant_id: TenantId,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub actor_user_id: i64,
    pub transaction_type: InventoryTransactionType,
    pub reason: Option<&'a str>,
    pub reference_type: Option<&'a str>,
    pub reference_id: Option<i64>,
    pub correlation_id: Option<&'a str>,
    pub operation: &'a str,
    pub idempotency_key: Option<&'a str>,
    pub request_hash: &'a str,
    pub record_idempotency: bool,
}

pub(crate) enum JournalStart {
    New(i64),
    Replay(i64),
}

pub(crate) struct JournalEntry {
    pub facility_id: i64,
    pub location_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub status: InventoryStatus,
    pub quantity_delta: i64,
}

pub(crate) async fn begin_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &JournalCommand<'_>,
) -> AppResult<JournalStart> {
    if command.operation.trim().is_empty() {
        return Err(AppError::internal("journal operation cannot be blank"));
    }

    if command.record_idempotency {
        if let Some(transaction_id) = replayed_transaction(
            tx,
            command.tenant_id,
            command.operation,
            command.idempotency_key,
            command.request_hash,
        )
        .await?
        {
            return Ok(JournalStart::Replay(transaction_id));
        }
    }

    let occurred_at = now_iso();
    let transaction_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_transactions
            (tenant_id, inventory_owner_id, created, actor_user_id, transaction_type,
             reason, reference_type, reference_id, correlation_id, operation,
             idempotency_key, request_hash)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING id
        "#,
    )
    .bind(command.tenant_id.get())
    .bind(command.inventory_owner_id)
    .bind(occurred_at)
    .bind(command.actor_user_id)
    .bind(command.transaction_type.as_str())
    .bind(command.reason)
    .bind(command.reference_type)
    .bind(command.reference_id)
    .bind(command.correlation_id)
    .bind(command.operation)
    .bind(command.idempotency_key)
    .bind(command.request_hash)
    .fetch_one(&mut **tx)
    .await?;

    let inventory_owner_id = InventoryOwnerId::new(command.inventory_owner_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility_id = FacilityId::new(command.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("inventory-transaction:{transaction_id}");
    let aggregate_id = transaction_id.to_string();
    let payload = serde_json::json!({
        "inventory_transaction_id": transaction_id,
        "inventory_owner_id": command.inventory_owner_id,
        "facility_id": command.facility_id,
        "transaction_type": command.transaction_type.as_str(),
        "operation": command.operation,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: command.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(command.actor_user_id),
            event_key: &event_key,
            aggregate_type: "inventory_transaction",
            aggregate_id: &aggregate_id,
            ordering_key: &event_key,
            aggregate_sequence: 1,
            event_type: "inventory.transaction.recorded",
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;

    if command.record_idempotency {
        if let Some(idempotency_key) = command.idempotency_key {
            sqlx::query(
                r#"
            INSERT INTO command_idempotency_records
                (tenant_id, created, operation, idempotency_key, request_hash,
                 result_json, inventory_transaction_id, actor_user_id)
            VALUES ($1, $2, $3, $4, $5, to_jsonb($6::BIGINT), $6, $7)
            "#,
            )
            .bind(command.tenant_id.get())
            .bind(now_iso())
            .bind(command.operation)
            .bind(idempotency_key)
            .bind(command.request_hash)
            .bind(transaction_id)
            .bind(command.actor_user_id)
            .execute(&mut **tx)
            .await?;
        }
    }

    Ok(JournalStart::New(transaction_id))
}

pub(crate) async fn append_entry(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    transaction_id: i64,
    entry: &JournalEntry,
) -> AppResult<i64> {
    if entry.quantity_delta == 0 {
        return Err(AppError::internal(
            "inventory journal entries cannot have a zero quantity",
        ));
    }

    let entry_id = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_entries
            (tenant_id, inventory_owner_id, transaction_id, created, facility_id,
             location_id, license_plate_id, item_batch_id, item_id, uom, lot,
             expiration, serial, status, quantity_delta)
        SELECT b.tenant_id, b.inventory_owner_id, $3, $4, $5, $6, $7, b.id,
               b.item_id, b.uom, b.lot, b.expiration, b.serial, $9, $10
        FROM item_batches b
        WHERE b.tenant_id = $1
          AND b.inventory_owner_id = $2
          AND b.id = $8
          AND b.deleted IS NULL
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(transaction_id)
    .bind(now_iso())
    .bind(entry.facility_id)
    .bind(entry.location_id)
    .bind(entry.license_plate_id)
    .bind(entry.item_batch_id)
    .bind(entry.status.as_str())
    .bind(entry.quantity_delta)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("item batch is outside the command scope"))?;

    Ok(entry_id)
}
