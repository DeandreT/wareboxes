//! Immutable inventory journal primitives shared by inventory workflows.

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType};
use wareboxes_domain::{OwnerFacilityScope, TenantId, Timestamp};

use crate::db::{bind_tenant_context, now_iso};
use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;

use wareboxes_persistence_postgres::idempotency::load_stored_result;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

pub(crate) struct JournalCommand<'a> {
    pub tenant_id: TenantId,
    pub owner_facility: OwnerFacilityScope,
    pub actor_user_id: i64,
    pub transaction_type: InventoryTransactionType,
    pub reason: Option<&'a str>,
    pub reference_type: Option<&'a str>,
    pub reference_id: Option<i64>,
    pub correlation_id: Option<&'a str>,
    pub operation: &'a str,
    pub idempotency_key: Option<&'a str>,
    pub request_hash: &'a str,
}

pub(crate) struct JournalEntry {
    pub location_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub status: InventoryStatus,
    pub quantity_delta: i64,
}

pub(crate) fn owner_facility_scope(
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<OwnerFacilityScope> {
    Ok(OwnerFacilityScope::new(
        wareboxes_domain::InventoryOwnerId::new(inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        wareboxes_domain::FacilityId::new(facility_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
    ))
}

pub(crate) async fn lock_active_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: OwnerFacilityScope,
) -> AppResult<()> {
    bind_tenant_context(tx, tenant_id).await?;
    let assignment_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT assignment.id
        FROM inventory_owner_facilities assignment
        INNER JOIN inventory_owners owner
            ON owner.tenant_id = assignment.tenant_id
           AND owner.id = assignment.inventory_owner_id
           AND owner.deleted IS NULL
        INNER JOIN facilities facility
            ON facility.tenant_id = assignment.tenant_id
           AND facility.id = assignment.facility_id
           AND facility.deleted IS NULL
        WHERE assignment.tenant_id = $1
          AND assignment.inventory_owner_id = $2
          AND assignment.facility_id = $3
          AND assignment.deleted IS NULL
        FOR SHARE OF assignment
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    if assignment_id.is_none() {
        return Err(AppError::conflict(
            "inventory owner is not active in the facility",
        ));
    }
    Ok(())
}

pub(crate) async fn authorize_transaction_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    transaction_id: Option<i64>,
) -> AppResult<i64> {
    let transaction_id = transaction_id.ok_or_else(|| {
        AppError::internal("stored inventory command has no inventory transaction")
    })?;
    let row = sqlx::query(
        r#"
        SELECT transaction.inventory_owner_id,
               ARRAY(
                   SELECT DISTINCT entry.facility_id
                   FROM inventory_entries entry
                   WHERE entry.tenant_id = transaction.tenant_id
                     AND entry.transaction_id = transaction.id
                   ORDER BY entry.facility_id
               ) AS facility_ids,
               EXISTS (
                   SELECT 1
                   FROM inventory_owners owner
                   WHERE owner.tenant_id = transaction.tenant_id
                     AND owner.id = transaction.inventory_owner_id
                     AND owner.deleted IS NULL
               ) AS owner_active,
               NOT EXISTS (
                   SELECT 1
                   FROM inventory_entries entry
                   WHERE entry.tenant_id = transaction.tenant_id
                     AND entry.transaction_id = transaction.id
                     AND (
                         NOT EXISTS (
                             SELECT 1
                             FROM facilities facility
                             WHERE facility.tenant_id = entry.tenant_id
                               AND facility.id = entry.facility_id
                               AND facility.deleted IS NULL
                         )
                         OR NOT EXISTS (
                             SELECT 1
                             FROM inventory_owner_facilities assignment
                             WHERE assignment.tenant_id = entry.tenant_id
                               AND assignment.inventory_owner_id = transaction.inventory_owner_id
                               AND assignment.facility_id = entry.facility_id
                               AND assignment.deleted IS NULL
                         )
                     )
               ) AS facilities_active
        FROM inventory_transactions transaction
        WHERE transaction.tenant_id = $1 AND transaction.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(transaction_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::internal("stored command inventory transaction was not found"))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_ids: Vec<i64> = row.try_get("facility_ids")?;
    let owner_active: bool = row.try_get("owner_active")?;
    let facilities_active: bool = row.try_get("facilities_active")?;
    if !owner_active
        || !facilities_active
        || !scope.includes_inventory_owner(inventory_owner_id)
        || facility_ids.is_empty()
        || facility_ids
            .iter()
            .any(|facility_id| !scope.includes_facility(*facility_id))
    {
        return Err(AppError::forbidden());
    }
    Ok(transaction_id)
}

pub(crate) async fn replayed_inventory_transaction_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<Option<i64>> {
    let Some(stored) = load_stored_result(tx, prepared).await? else {
        return Ok(None);
    };
    let linked_transaction_id = stored.inventory_transaction_id();
    let result = prepared.resolve_replay::<i64>(stored)?;
    let authorized_transaction_id =
        authorize_transaction_replay_tx(tx, prepared.tenant_id(), scope, linked_transaction_id)
            .await?;
    if result != authorized_transaction_id {
        return Err(AppError::internal(
            "stored inventory command result does not match its inventory transaction",
        ));
    }
    Ok(Some(result))
}

pub(crate) async fn begin_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &JournalCommand<'_>,
) -> AppResult<i64> {
    begin_transaction_at(tx, command, now_iso(), false).await
}

pub(crate) async fn begin_batched_transaction_at(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &JournalCommand<'_>,
    occurred_at: Timestamp,
) -> AppResult<i64> {
    begin_transaction_at(tx, command, occurred_at, true).await
}

async fn begin_transaction_at(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &JournalCommand<'_>,
    occurred_at: Timestamp,
    replace_context: bool,
) -> AppResult<i64> {
    if command.operation.trim().is_empty() {
        return Err(AppError::internal("journal operation cannot be blank"));
    }
    lock_active_owner_facility_tx(tx, command.tenant_id, command.owner_facility).await?;

    let existing_transaction_id: Option<String> = sqlx::query_scalar(
        "SELECT NULLIF(current_setting('wareboxes.inventory_transaction_id', true), '')",
    )
    .fetch_one(&mut **tx)
    .await?;
    if !replace_context {
        if let Some(existing_transaction_id) = existing_transaction_id {
            return Err(AppError::internal(format!(
                "database transaction already contains inventory transaction {existing_transaction_id}"
            )));
        }
    }

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
    .bind(command.owner_facility.inventory_owner_id.get())
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

    sqlx::query_scalar::<_, String>(
        "SELECT set_config('wareboxes.inventory_transaction_id', $1, true)",
    )
    .bind(transaction_id.to_string())
    .fetch_one(&mut **tx)
    .await?;

    let event_key = format!("inventory-transaction:{transaction_id}");
    let aggregate_id = transaction_id.to_string();
    let payload = serde_json::json!({
        "inventory_transaction_id": transaction_id,
        "inventory_owner_id": command.owner_facility.inventory_owner_id,
        "facility_id": command.owner_facility.facility_id,
        "transaction_type": command.transaction_type.as_str(),
        "operation": command.operation,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: command.tenant_id,
            inventory_owner_id: Some(command.owner_facility.inventory_owner_id),
            facility_id: Some(command.owner_facility.facility_id),
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

    Ok(transaction_id)
}

pub(crate) async fn append_entry(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_facility: OwnerFacilityScope,
    transaction_id: i64,
    entry: &JournalEntry,
) -> AppResult<i64> {
    if entry.quantity_delta == 0 {
        return Err(AppError::internal(
            "inventory journal entries cannot have a zero quantity",
        ));
    }
    bind_tenant_context(tx, tenant_id).await?;

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
    .bind(owner_facility.inventory_owner_id.get())
    .bind(transaction_id)
    .bind(now_iso())
    .bind(owner_facility.facility_id.get())
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
