//! Typed inventory disposition changes backed by the immutable inventory journal.

use serde::Serialize;
use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::dto::ChangeInventoryStatusResult;
use wareboxes_core::models::{
    InventoryStatus, InventoryStatusChangeReason, InventoryTransactionType, TenantAccess,
};
use wareboxes_domain::TenantId;

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::idempotency::{require_command_context, PreparedCommand};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry, JournalStart};
use crate::repo::inventory_locking::{balance_license_plate_hint, lock_license_plate};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

const OPERATION: &str = "inventory.status_change.v1";

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ChangeInventoryStatusCommand<'a> {
    pub inventory_balance_id: i64,
    pub qty: i64,
    pub to_status: InventoryStatus,
    pub reason: InventoryStatusChangeReason,
    pub note: Option<&'a str>,
    pub reference_type: Option<&'a str>,
    pub reference_id: Option<i64>,
}

#[derive(Debug)]
struct BalanceHint {
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    license_plate_id: Option<i64>,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    status: InventoryStatus,
}

#[derive(Debug, Clone)]
struct LockedBalance {
    id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    license_plate_id: Option<i64>,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    status: InventoryStatus,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
    active: bool,
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

fn require_scope(
    scope: &ScopeBindings,
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<()> {
    if scope.includes_inventory_owner(inventory_owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::forbidden())
    }
}

fn validated_optional_text<'a>(
    value: Option<&'a str>,
    label: &str,
    maximum_characters: usize,
) -> AppResult<Option<&'a str>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim() != value || value.is_empty() {
        return Err(AppError::bad_request(format!(
            "{label} must be trimmed and nonempty"
        )));
    }
    if value.chars().count() > maximum_characters {
        return Err(AppError::bad_request(format!(
            "{label} cannot exceed {maximum_characters} characters"
        )));
    }
    Ok(Some(value))
}

fn validate_command<'a>(
    command: &'a ChangeInventoryStatusCommand<'a>,
) -> AppResult<ChangeInventoryStatusCommand<'a>> {
    if command.inventory_balance_id <= 0 {
        return Err(AppError::bad_request(
            "inventory balance ID must be positive",
        ));
    }
    if command.qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }
    let note = validated_optional_text(command.note, "status change note", 1000)?;
    let reference_type =
        validated_optional_text(command.reference_type, "status change reference type", 100)?;
    match (reference_type, command.reference_id) {
        (None, None) | (Some(_), Some(1..)) => {}
        _ => {
            return Err(AppError::bad_request(
                "status change reference type and positive ID must be provided together",
            ));
        }
    }
    if command.reason == InventoryStatusChangeReason::Other && note.is_none() {
        return Err(AppError::bad_request(
            "status change note is required when reason is other",
        ));
    }
    if !command.reason.allows_target_status(command.to_status) {
        return Err(AppError::bad_request(
            "status change reason does not permit the requested target status",
        ));
    }
    Ok(ChangeInventoryStatusCommand {
        note,
        reference_type,
        ..*command
    })
}

async fn get_balance_hint(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_balance_id: i64,
) -> AppResult<BalanceHint> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, location_id, license_plate_id,
               item_batch_id, item_id, uom, status
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory balance"))?;

    Ok(BalanceHint {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
    })
}

async fn lock_status_balances(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    hint: &BalanceHint,
    to_status: InventoryStatus,
) -> AppResult<Vec<LockedBalance>> {
    let rows = sqlx::query(
        r#"
        SELECT id, inventory_owner_id, facility_id, location_id,
               license_plate_id, item_batch_id, item_id, uom, status,
               qty_on_hand, qty_reserved, qty_held, deleted IS NULL AS active
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND facility_id = $3
          AND location_id = $4
          AND license_plate_id IS NOT DISTINCT FROM $5
          AND item_batch_id = $6
          AND item_id = $7
          AND uom = $8
          AND (status = $9 OR status = $10)
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(hint.inventory_owner_id)
    .bind(hint.facility_id)
    .bind(hint.location_id)
    .bind(hint.license_plate_id)
    .bind(hint.item_batch_id)
    .bind(hint.item_id)
    .bind(&hint.uom)
    .bind(hint.status.as_str())
    .bind(to_status.as_str())
    .fetch_all(&mut **tx)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(LockedBalance {
                id: row.try_get("id")?,
                inventory_owner_id: row.try_get("inventory_owner_id")?,
                facility_id: row.try_get("facility_id")?,
                location_id: row.try_get("location_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
                qty_on_hand: row.try_get("qty_on_hand")?,
                qty_reserved: row.try_get("qty_reserved")?,
                qty_held: row.try_get("qty_held")?,
                active: row.try_get("active")?,
            })
        })
        .collect()
}

fn balance_matches_hint(balance: &LockedBalance, hint: &BalanceHint) -> bool {
    balance.inventory_owner_id == hint.inventory_owner_id
        && balance.facility_id == hint.facility_id
        && balance.location_id == hint.location_id
        && balance.license_plate_id == hint.license_plate_id
        && balance.item_batch_id == hint.item_batch_id
        && balance.item_id == hint.item_id
        && balance.uom == hint.uom
        && balance.status == hint.status
}

async fn decrement_source_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    source: &LockedBalance,
    qty: i64,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1,
            modified = $2
        WHERE tenant_id = $3
          AND inventory_owner_id = $4
          AND id = $5
          AND status = $6
          AND deleted IS NULL
          AND qty_on_hand = $7
          AND qty_reserved = $8
          AND qty_held = $9
          AND qty_on_hand - qty_reserved - qty_held >= $1
        "#,
    )
    .bind(qty)
    .bind(now_iso())
    .bind(tenant_id.get())
    .bind(source.inventory_owner_id)
    .bind(source.id)
    .bind(source.status.as_str())
    .bind(source.qty_on_hand)
    .bind(source.qty_reserved)
    .bind(source.qty_held)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inventory balance changed during status change",
        ));
    }
    Ok(())
}

async fn increment_target_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    source: &LockedBalance,
    to_status: InventoryStatus,
    qty: i64,
) -> AppResult<i64> {
    let now = now_iso();
    let target_id = if source.license_plate_id.is_some() {
        sqlx::query_scalar(
            r#"
            INSERT INTO inventory_balances (
                tenant_id, inventory_owner_id, created, modified, facility_id,
                location_id, license_plate_id, item_batch_id, item_id, uom,
                status, qty_on_hand, qty_reserved, qty_held
            )
            VALUES (
                $1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, 0, 0
            )
            ON CONFLICT (
                tenant_id, inventory_owner_id, location_id, license_plate_id,
                item_batch_id, uom, status
            )
                WHERE license_plate_id IS NOT NULL DO UPDATE
            SET qty_on_hand = inventory_balances.qty_on_hand
                    + excluded.qty_on_hand,
                modified = excluded.modified,
                deleted = NULL
            RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(source.inventory_owner_id)
        .bind(now)
        .bind(source.facility_id)
        .bind(source.location_id)
        .bind(source.license_plate_id)
        .bind(source.item_batch_id)
        .bind(source.item_id)
        .bind(&source.uom)
        .bind(to_status.as_str())
        .bind(qty)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO inventory_balances (
                tenant_id, inventory_owner_id, created, modified, facility_id,
                location_id, license_plate_id, item_batch_id, item_id, uom,
                status, qty_on_hand, qty_reserved, qty_held
            )
            VALUES (
                $1, $2, $3, $3, $4, $5, NULL, $6, $7, $8, $9, $10, 0, 0
            )
            ON CONFLICT (
                tenant_id, inventory_owner_id, location_id, item_batch_id,
                uom, status
            )
                WHERE license_plate_id IS NULL DO UPDATE
            SET qty_on_hand = inventory_balances.qty_on_hand
                    + excluded.qty_on_hand,
                modified = excluded.modified,
                deleted = NULL
            RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(source.inventory_owner_id)
        .bind(now)
        .bind(source.facility_id)
        .bind(source.location_id)
        .bind(source.item_batch_id)
        .bind(source.item_id)
        .bind(&source.uom)
        .bind(to_status.as_str())
        .bind(qty)
        .fetch_one(&mut **tx)
        .await?
    };
    Ok(target_id)
}

#[allow(clippy::too_many_arguments)]
async fn insert_transition_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    source: &LockedBalance,
    target_balance_id: i64,
    transaction_id: i64,
    command: &ChangeInventoryStatusCommand<'_>,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        r#"
        INSERT INTO inventory_status_transitions (
            tenant_id, inventory_owner_id, facility_id, transaction_id,
            source_balance_id, destination_balance_id, from_status, to_status,
            qty, reason_code, reason_note, reference_type, reference_id,
            created_by, created
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
            $15
        )
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(source.inventory_owner_id)
    .bind(source.facility_id)
    .bind(transaction_id)
    .bind(source.id)
    .bind(target_balance_id)
    .bind(source.status.as_str())
    .bind(command.to_status.as_str())
    .bind(command.qty)
    .bind(command.reason.as_str())
    .bind(command.note)
    .bind(command.reference_type)
    .bind(command.reference_id)
    .bind(actor_user_id)
    .bind(now_iso())
    .fetch_one(&mut **tx)
    .await?)
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_status_changed_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    source: &LockedBalance,
    target_balance_id: i64,
    transaction_id: i64,
    transition_id: i64,
    command: &ChangeInventoryStatusCommand<'_>,
) -> AppResult<()> {
    let event_key = format!("inventory-transaction:{transaction_id}:status-changed");
    let aggregate_id = transaction_id.to_string();
    let ordering_key = format!("inventory-transaction:{transaction_id}");
    let payload = serde_json::json!({
        "inventory_status_transition_id": transition_id,
        "inventory_transaction_id": transaction_id,
        "source_inventory_balance_id": source.id,
        "target_inventory_balance_id": target_balance_id,
        "inventory_owner_id": source.inventory_owner_id,
        "facility_id": source.facility_id,
        "location_id": source.location_id,
        "license_plate_id": source.license_plate_id,
        "item_batch_id": source.item_batch_id,
        "item_id": source.item_id,
        "uom": source.uom,
        "quantity": command.qty,
        "from_status": source.status.as_str(),
        "to_status": command.to_status.as_str(),
        "reason": command.reason.as_str(),
        "note": command.note,
        "reference_type": command.reference_type,
        "reference_id": command.reference_id,
    });
    let owner_facility =
        inventory_journal::owner_facility_scope(source.inventory_owner_id, source.facility_id)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner_facility.inventory_owner_id),
            facility_id: Some(owner_facility.facility_id),
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "inventory_transaction",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: 2,
            event_type: "inventory.status.changed",
            schema_version: 1,
            payload: &payload,
            occurred_at: now_iso(),
        },
    )
    .await?;
    Ok(())
}

pub async fn change_inventory_status(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ChangeInventoryStatusCommand<'_>,
) -> AppResult<ChangeInventoryStatusResult> {
    require_command_context(access, context)?;
    let command = validate_command(command)?;
    let prepared = PreparedCommand::new(context, OPERATION, &command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;

    let license_plate_id =
        balance_license_plate_hint(&mut tx, access.tenant_id, command.inventory_balance_id).await?;
    lock_license_plate(&mut tx, access.tenant_id, license_plate_id).await?;
    let hint = get_balance_hint(&mut tx, access.tenant_id, command.inventory_balance_id).await?;
    let locked = lock_status_balances(&mut tx, access.tenant_id, &hint, command.to_status).await?;
    let source = locked
        .iter()
        .find(|balance| balance.id == command.inventory_balance_id)
        .cloned()
        .ok_or_else(|| {
            AppError::conflict("inventory balance changed while acquiring status-change locks")
        })?;
    require_scope(&scope, source.inventory_owner_id, source.facility_id)?;

    if let Some(result) = prepared
        .replayed::<ChangeInventoryStatusResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    if license_plate_id != source.license_plate_id || !balance_matches_hint(&source, &hint) {
        return Err(AppError::conflict(
            "inventory balance changed while acquiring status-change locks",
        ));
    }
    if !source.active {
        return Err(AppError::conflict("inventory balance is not active"));
    }
    if source.status == command.to_status {
        return Err(AppError::conflict(
            "inventory balance already has the requested status",
        ));
    }
    let uncommitted = source
        .qty_on_hand
        .checked_sub(source.qty_reserved)
        .and_then(|quantity| quantity.checked_sub(source.qty_held))
        .ok_or_else(|| AppError::internal("inventory commitments are out of range"))?;
    if command.qty > uncommitted {
        return Err(AppError::conflict(
            "insufficient uncommitted inventory for status change",
        ));
    }

    let owner_facility =
        inventory_journal::owner_facility_scope(source.inventory_owner_id, source.facility_id)?;
    let transaction_id = match inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::StatusChange,
            reason: Some(command.reason.as_str()),
            reference_type: command.reference_type,
            reference_id: command.reference_id,
            correlation_id: Some(&context.request_id),
            operation: OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
            record_idempotency: false,
        },
    )
    .await?
    {
        JournalStart::New(transaction_id) => transaction_id,
        JournalStart::Replay(_) => {
            return Err(AppError::internal(
                "status-change journal replay bypassed command replay",
            ));
        }
    };

    decrement_source_balance(&mut tx, access.tenant_id, &source, command.qty).await?;
    let target_balance_id = increment_target_balance(
        &mut tx,
        access.tenant_id,
        &source,
        command.to_status,
        command.qty,
    )
    .await?;

    for (status, quantity_delta) in [
        (source.status, -command.qty),
        (command.to_status, command.qty),
    ] {
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: source.location_id,
                license_plate_id: source.license_plate_id,
                item_batch_id: source.item_batch_id,
                status,
                quantity_delta,
            },
        )
        .await?;
    }

    let transition_id = insert_transition_audit(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &source,
        target_balance_id,
        transaction_id,
        &command,
    )
    .await?;
    enqueue_status_changed_event(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &source,
        target_balance_id,
        transaction_id,
        transition_id,
        &command,
    )
    .await?;

    let result = ChangeInventoryStatusResult {
        inventory_transaction_id: transaction_id,
        source_inventory_balance_id: source.id,
        target_inventory_balance_id: target_balance_id,
        qty: command.qty,
        from_status: source.status,
        to_status: command.to_status,
    };
    prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await
}
