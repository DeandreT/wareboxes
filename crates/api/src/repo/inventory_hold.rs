//! Quantity-level inventory restrictions and their balance projection.

use serde::Serialize;
use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::dto::{PlaceInventoryHoldResult, ReleaseInventoryHoldResult};
use wareboxes_core::models::{
    InventoryHold, InventoryHoldReason, InventoryHoldReconciliationIssue, InventoryHoldStatus,
    InventoryStatus, TenantAccess, Timestamp,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::inventory_locking::{balance_license_plate_hint, lock_license_plate};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

const PLACE_OPERATION: &str = "inventory_hold.place.v1";
const RELEASE_OPERATION: &str = "inventory_hold.release.v1";

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PlaceInventoryHoldCommand<'a> {
    pub inventory_balance_id: i64,
    pub qty: i64,
    pub reason: InventoryHoldReason,
    pub note: Option<&'a str>,
    pub reference_type: Option<&'a str>,
    pub reference_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ReleaseInventoryHoldCommand {
    pub hold_id: i64,
}

#[derive(Debug)]
struct LockedBalance {
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    license_plate_id: Option<i64>,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    inventory_status: String,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
    deleted: Option<Timestamp>,
}

#[derive(Debug)]
struct LockedHold {
    inventory_owner_id: i64,
    inventory_balance_id: i64,
    facility_id: i64,
    location_id: i64,
    license_plate_id: Option<i64>,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    inventory_status: String,
    qty: i64,
    reason_code: String,
    note: Option<String>,
    reference_type: Option<String>,
    reference_id: Option<i64>,
    status: String,
}

#[derive(Debug, Clone, Copy)]
struct InventoryHoldEventContext<'a> {
    tenant_id: TenantId,
    actor_user_id: i64,
    transition: &'a str,
    aggregate_sequence: i64,
    occurred_at: Timestamp,
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value).ok_or_else(|| {
        AppError::internal(format!(
            "invalid inventory status in inventory hold: {value}"
        ))
    })
}

fn parse_hold_reason(value: &str) -> AppResult<InventoryHoldReason> {
    InventoryHoldReason::parse(value).ok_or_else(|| {
        AppError::internal(format!("invalid reason code in inventory hold: {value}"))
    })
}

fn parse_hold_status(value: &str) -> AppResult<InventoryHoldStatus> {
    InventoryHoldStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory hold status: {value}")))
}

fn map_hold(row: &sqlx::postgres::PgRow) -> AppResult<InventoryHold> {
    Ok(InventoryHold {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        modified: row.try_get("modified")?,
        deleted: row.try_get("deleted")?,
        created_by: row.try_get("created_by")?,
        released_by: row.try_get("released_by")?,
        released_at: row.try_get("released_at")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        inventory_status: parse_inventory_status(&row.try_get::<String, _>("inventory_status")?)?,
        qty: row.try_get("qty")?,
        reason: parse_hold_reason(&row.try_get::<String, _>("reason_code")?)?,
        note: row.try_get("note")?,
        reference_type: row.try_get("reference_type")?,
        reference_id: row.try_get("reference_id")?,
        status: parse_hold_status(&row.try_get::<String, _>("status")?)?,
    })
}

fn map_reconciliation_issue(
    row: &sqlx::postgres::PgRow,
) -> AppResult<InventoryHoldReconciliationIssue> {
    Ok(InventoryHoldReconciliationIssue {
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        inventory_status: parse_inventory_status(&row.try_get::<String, _>("inventory_status")?)?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
        allocated_qty: row.try_get("allocated_qty")?,
        qty_held: row.try_get("qty_held")?,
        held_qty: row.try_get("held_qty")?,
        overcommitted_qty: row.try_get("overcommitted_qty")?,
        issue_codes: row.try_get("issue_codes")?,
    })
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

fn validate_place_command<'a>(
    command: &'a PlaceInventoryHoldCommand<'a>,
) -> AppResult<PlaceInventoryHoldCommand<'a>> {
    if command.inventory_balance_id <= 0 {
        return Err(AppError::bad_request(
            "inventory balance ID must be positive",
        ));
    }
    if command.qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }
    let note = validated_optional_text(command.note, "hold note", 1000)?;
    let reference_type =
        validated_optional_text(command.reference_type, "hold reference type", 100)?;
    match (reference_type, command.reference_id) {
        (None, None) | (Some(_), Some(1..)) => {}
        _ => {
            return Err(AppError::bad_request(
                "hold reference type and positive ID must be provided together",
            ));
        }
    }
    if command.reason == InventoryHoldReason::Other && note.is_none() {
        return Err(AppError::bad_request(
            "hold note is required when reason is other",
        ));
    }
    Ok(PlaceInventoryHoldCommand {
        note,
        reference_type,
        ..*command
    })
}

async fn lock_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_balance_id: i64,
) -> AppResult<LockedBalance> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, location_id, license_plate_id,
               item_batch_id, item_id, uom, status AS inventory_status,
               qty_on_hand, qty_reserved, qty_held, deleted
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory balance"))?;

    Ok(LockedBalance {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        inventory_status: row.try_get("inventory_status")?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
        qty_held: row.try_get("qty_held")?,
        deleted: row.try_get("deleted")?,
    })
}

async fn lock_hold(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    hold_id: i64,
) -> AppResult<LockedHold> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, inventory_balance_id, facility_id,
               location_id, license_plate_id, item_batch_id, item_id, uom,
               inventory_status, qty, reason_code, note, reference_type,
               reference_id, status
        FROM inventory_holds
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(hold_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory hold"))?;

    Ok(LockedHold {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        inventory_status: row.try_get("inventory_status")?,
        qty: row.try_get("qty")?,
        reason_code: row.try_get("reason_code")?,
        note: row.try_get("note")?,
        reference_type: row.try_get("reference_type")?,
        reference_id: row.try_get("reference_id")?,
        status: row.try_get("status")?,
    })
}

async fn enqueue_hold_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    hold_id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    event: InventoryHoldEventContext<'_>,
    payload: &serde_json::Value,
) -> AppResult<()> {
    let event_key = format!("inventory-hold:{hold_id}:{}", event.transition);
    let aggregate_id = hold_id.to_string();
    let ordering_key = format!("inventory-hold:{hold_id}");
    let event_type = format!("inventory.hold.{}", event.transition);
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: Some(
                InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(event.actor_user_id),
            event_key: &event_key,
            aggregate_type: "inventory_hold",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: event.aggregate_sequence,
            event_type: &event_type,
            schema_version: 1,
            payload,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_hold_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    now: Timestamp,
    balance: &LockedBalance,
    command: PlaceInventoryHoldCommand<'_>,
) -> AppResult<i64> {
    let hold_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_holds (
            tenant_id, inventory_owner_id, created, modified, created_by,
            inventory_balance_id, facility_id, location_id, license_plate_id,
            item_batch_id, item_id, uom, inventory_status, qty, reason_code,
            note, reference_type, reference_id, status
        )
        VALUES (
            $1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
            $14, $15, $16, $17, 'active'
        )
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(balance.inventory_owner_id)
    .bind(now)
    .bind(actor_user_id)
    .bind(command.inventory_balance_id)
    .bind(balance.facility_id)
    .bind(balance.location_id)
    .bind(balance.license_plate_id)
    .bind(balance.item_batch_id)
    .bind(balance.item_id)
    .bind(&balance.uom)
    .bind(&balance.inventory_status)
    .bind(command.qty)
    .bind(command.reason.as_str())
    .bind(command.note)
    .bind(command.reference_type)
    .bind(command.reference_id)
    .fetch_one(&mut **tx)
    .await?;

    let payload = serde_json::json!({
        "hold_id": hold_id,
        "inventory_balance_id": command.inventory_balance_id,
        "inventory_owner_id": balance.inventory_owner_id,
        "facility_id": balance.facility_id,
        "location_id": balance.location_id,
        "license_plate_id": balance.license_plate_id,
        "item_batch_id": balance.item_batch_id,
        "item_id": balance.item_id,
        "uom": balance.uom,
        "inventory_status": balance.inventory_status,
        "quantity": command.qty,
        "reason": command.reason.as_str(),
        "note": command.note,
        "reference_type": command.reference_type,
        "reference_id": command.reference_id,
    });
    enqueue_hold_event(
        tx,
        hold_id,
        balance.inventory_owner_id,
        balance.facility_id,
        InventoryHoldEventContext {
            tenant_id,
            actor_user_id,
            transition: "placed",
            aggregate_sequence: 1,
            occurred_at: now,
        },
        &payload,
    )
    .await?;
    Ok(hold_id)
}

pub(crate) async fn place_composed_inventory_hold_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    now: Timestamp,
    command: &PlaceInventoryHoldCommand<'_>,
) -> AppResult<i64> {
    let command = validate_place_command(command)?;
    let balance = lock_balance(tx, tenant_id, command.inventory_balance_id).await?;
    if balance.deleted.is_some() {
        return Err(AppError::conflict("inventory balance is not active"));
    }
    let available = balance
        .qty_on_hand
        .checked_sub(balance.qty_reserved)
        .and_then(|quantity| quantity.checked_sub(balance.qty_held))
        .ok_or_else(|| AppError::internal("inventory commitments are out of range"))?;
    if command.qty > available {
        return Err(AppError::conflict(
            "insufficient uncommitted inventory to hold",
        ));
    }
    insert_hold_tx(tx, tenant_id, actor_user_id, now, &balance, command).await
}

pub async fn get_inventory_holds_in_scope(
    db: &Db,
    access: &TenantAccess,
    show_deleted: bool,
) -> AppResult<Vec<InventoryHold>> {
    let scope = ScopeBindings::for_access(access);
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, modified, deleted,
               created_by, released_by, released_at, inventory_balance_id,
               facility_id, location_id, license_plate_id, item_batch_id,
               item_id, uom, inventory_status, qty, reason_code, note,
               reference_type, reference_id, status
        FROM inventory_holds
        WHERE tenant_id = $1
          AND ($2 OR deleted IS NULL)
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        ORDER BY id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(show_deleted)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    let holds = rows.iter().map(map_hold).collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(holds)
}

pub async fn get_inventory_hold_reconciliation_issues_in_scope(
    db: &Db,
    access: &TenantAccess,
) -> AppResult<Vec<InventoryHoldReconciliationIssue>> {
    let scope = ScopeBindings::for_access(access);
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT inventory_balance_id, tenant_id, inventory_owner_id, facility_id,
               location_id, license_plate_id, item_batch_id, item_id, uom,
               inventory_status, qty_on_hand, qty_reserved, allocated_qty,
               qty_held, held_qty, overcommitted_qty, issue_codes
        FROM inventory_hold_reconciliation
        WHERE tenant_id = $1
          AND ($2 OR facility_id = ANY($3))
          AND ($4 OR inventory_owner_id = ANY($5))
        ORDER BY inventory_balance_id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    let issues = rows
        .iter()
        .map(map_reconciliation_issue)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(issues)
}

pub async fn place_inventory_hold(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlaceInventoryHoldCommand<'_>,
) -> AppResult<PlaceInventoryHoldResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let command = validate_place_command(command)?;
    let prepared = PreparedCommand::new_v1(context, PLACE_OPERATION, &command)?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    let license_plate_id =
        balance_license_plate_hint(&mut tx, access.tenant_id, command.inventory_balance_id).await?;
    lock_license_plate(&mut tx, access.tenant_id, license_plate_id).await?;
    let balance = lock_balance(&mut tx, access.tenant_id, command.inventory_balance_id).await?;
    require_scope(&scope, balance.inventory_owner_id, balance.facility_id)?;

    if let Some(result) = prepared
        .replayed::<PlaceInventoryHoldResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    if balance.license_plate_id != license_plate_id {
        return Err(AppError::conflict(
            "inventory balance license plate changed while acquiring locks",
        ));
    }
    if balance.deleted.is_some() {
        return Err(AppError::conflict("inventory balance is not active"));
    }
    let available = balance
        .qty_on_hand
        .checked_sub(balance.qty_reserved)
        .and_then(|quantity| quantity.checked_sub(balance.qty_held))
        .ok_or_else(|| AppError::internal("inventory commitments are out of range"))?;
    if command.qty > available {
        return Err(AppError::conflict(
            "insufficient uncommitted inventory to hold",
        ));
    }

    let hold_id = insert_hold_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        now,
        &balance,
        command,
    )
    .await?;

    Ok(prepared
        .commit(tx, PlaceInventoryHoldResult { hold_id })
        .await?)
}

pub async fn release_inventory_hold(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReleaseInventoryHoldCommand,
) -> AppResult<ReleaseInventoryHoldResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.hold_id <= 0 {
        return Err(AppError::bad_request("inventory hold ID must be positive"));
    }
    let prepared = PreparedCommand::new_v1(context, RELEASE_OPERATION, command)?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    let hint = sqlx::query(
        r#"
        SELECT inventory_balance_id, license_plate_id
        FROM inventory_holds
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.hold_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory hold"))?;
    let inventory_balance_id: i64 = hint.try_get("inventory_balance_id")?;
    let license_plate_id: Option<i64> = hint.try_get("license_plate_id")?;
    lock_license_plate(&mut tx, access.tenant_id, license_plate_id).await?;
    let balance = lock_balance(&mut tx, access.tenant_id, inventory_balance_id).await?;
    let hold = lock_hold(&mut tx, access.tenant_id, command.hold_id).await?;
    if hold.inventory_balance_id != inventory_balance_id {
        return Err(AppError::internal(
            "inventory hold balance changed while acquiring locks",
        ));
    }
    require_scope(&scope, hold.inventory_owner_id, hold.facility_id)?;

    if let Some(result) = prepared
        .replayed::<ReleaseInventoryHoldResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    if hold.license_plate_id != license_plate_id || balance.license_plate_id != license_plate_id {
        return Err(AppError::conflict(
            "inventory hold license plate changed while acquiring locks",
        ));
    }
    if hold.status != InventoryHoldStatus::Active.as_str() {
        return Err(AppError::conflict("inventory hold is not active"));
    }

    let updated = sqlx::query(
        r#"
        UPDATE inventory_holds
        SET modified = $1,
            deleted = $1,
            released_by = $2,
            released_at = $1,
            status = 'released'
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND status = 'active'
        "#,
    )
    .bind(now)
    .bind(context.actor_id.get())
    .bind(access.tenant_id.get())
    .bind(command.hold_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("inventory hold could not be released"));
    }

    let payload = serde_json::json!({
        "hold_id": command.hold_id,
        "inventory_balance_id": hold.inventory_balance_id,
        "inventory_owner_id": hold.inventory_owner_id,
        "facility_id": hold.facility_id,
        "location_id": hold.location_id,
        "license_plate_id": hold.license_plate_id,
        "item_batch_id": hold.item_batch_id,
        "item_id": hold.item_id,
        "uom": hold.uom,
        "inventory_status": hold.inventory_status,
        "released_quantity": hold.qty,
        "reason": hold.reason_code,
        "note": hold.note,
        "reference_type": hold.reference_type,
        "reference_id": hold.reference_id,
    });
    enqueue_hold_event(
        &mut tx,
        command.hold_id,
        hold.inventory_owner_id,
        hold.facility_id,
        InventoryHoldEventContext {
            tenant_id: access.tenant_id,
            actor_user_id: context.actor_id.get(),
            transition: "released",
            aggregate_sequence: 2,
            occurred_at: now,
        },
        &payload,
    )
    .await?;

    Ok(prepared
        .commit(
            tx,
            ReleaseInventoryHoldResult {
                hold_id: command.hold_id,
                released_qty: hold.qty,
            },
        )
        .await?)
}
