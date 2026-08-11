//! Load-scoped receipt of physically present stock that is absent from the ASN quantity.

use serde::Serialize;
use sqlx::Row;
use wareboxes_application::idempotency::{command_request_hash, PreparedCommand};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    ConfirmUnexpectedReceiptResult, InventoryStatus, InventoryTransactionType, LoadStatus,
    LoadType, TenantAccess, Timestamp, UnexpectedReceiptReason,
};
use wareboxes_domain::TenantId;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::lock_current_scope_tx;
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::{inventory, inventory_hold, license_plates};

const OPERATION: &str = "inbound.confirm_unexpected_receipt.v1";

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ConfirmUnexpectedReceiptCommand<'a> {
    pub item_barcode: &'a str,
    pub receiving_location_barcode: &'a str,
    pub quantity: i64,
    pub license_plate_barcode: Option<&'a str>,
    pub lot: Option<&'a str>,
    pub serial: Option<&'a str>,
    pub expiration: Option<Timestamp>,
    pub reason: UnexpectedReceiptReason,
    pub note: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ValidatedCommand<'a> {
    item_barcode: &'a str,
    receiving_location_barcode: &'a str,
    quantity: i64,
    license_plate_barcode: Option<&'a str>,
    lot: Option<&'a str>,
    serial: Option<&'a str>,
    expiration: Option<Timestamp>,
    reason: UnexpectedReceiptReason,
    note: Option<&'a str>,
}

fn required_text<'a>(value: &'a str, label: &str, maximum: usize) -> AppResult<&'a str> {
    if value.is_empty() || value.trim() != value || value.chars().count() > maximum {
        return Err(AppError::bad_request(format!(
            "{label} must be trimmed, nonempty, and at most {maximum} characters"
        )));
    }
    Ok(value)
}

fn optional_text<'a>(
    value: Option<&'a str>,
    label: &str,
    maximum: usize,
) -> AppResult<Option<&'a str>> {
    value
        .map(|value| required_text(value, label, maximum))
        .transpose()
}

fn validate_command<'a>(
    command: &'a ConfirmUnexpectedReceiptCommand<'a>,
) -> AppResult<ValidatedCommand<'a>> {
    if command.quantity <= 0 {
        return Err(AppError::bad_request(
            "unexpected receipt quantity must be positive",
        ));
    }
    let note = optional_text(command.note, "unexpected receipt note", 1_000)?;
    if command.reason == UnexpectedReceiptReason::Other && note.is_none() {
        return Err(AppError::bad_request(
            "unexpected receipt note is required when the reason is other",
        ));
    }
    Ok(ValidatedCommand {
        item_barcode: required_text(command.item_barcode, "item barcode", 200)?,
        receiving_location_barcode: required_text(
            command.receiving_location_barcode,
            "receiving location barcode",
            200,
        )?,
        quantity: command.quantity,
        license_plate_barcode: optional_text(
            command.license_plate_barcode,
            "license plate barcode",
            200,
        )?,
        lot: optional_text(command.lot, "lot", 200)?,
        serial: optional_text(command.serial, "serial", 200)?,
        expiration: command.expiration,
        reason: command.reason,
        note,
    })
}

#[allow(clippy::too_many_arguments)]
async fn upsert_quarantine_balance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    license_plate_id: Option<i64>,
    item_batch_id: i64,
    item_id: i64,
    uom: &str,
    quantity: i64,
    now: Timestamp,
) -> AppResult<i64> {
    let balance_id = if let Some(license_plate_id) = license_plate_id {
        sqlx::query_scalar(
            r#"
            INSERT INTO inventory_balances
                (tenant_id, inventory_owner_id, created, modified, facility_id,
                 location_id, license_plate_id, item_batch_id, item_id, uom,
                 status, qty_on_hand, qty_reserved)
            VALUES ($1,$2,$3,$3,$4,$5,$6,$7,$8,$9,'quarantine',$10,0)
            ON CONFLICT (
                tenant_id, inventory_owner_id, location_id, license_plate_id,
                item_batch_id, uom, status
            ) WHERE license_plate_id IS NOT NULL
            DO UPDATE SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
                          modified=excluded.modified, deleted=NULL
            RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id)
        .bind(now)
        .bind(facility_id)
        .bind(location_id)
        .bind(license_plate_id)
        .bind(item_batch_id)
        .bind(item_id)
        .bind(uom)
        .bind(quantity)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO inventory_balances
                (tenant_id, inventory_owner_id, created, modified, facility_id,
                 location_id, license_plate_id, item_batch_id, item_id, uom,
                 status, qty_on_hand, qty_reserved)
            VALUES ($1,$2,$3,$3,$4,$5,NULL,$6,$7,$8,'quarantine',$9,0)
            ON CONFLICT (
                tenant_id, inventory_owner_id, location_id, item_batch_id, uom, status
            ) WHERE license_plate_id IS NULL
            DO UPDATE SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
                          modified=excluded.modified, deleted=NULL
            RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id)
        .bind(now)
        .bind(facility_id)
        .bind(location_id)
        .bind(item_batch_id)
        .bind(item_id)
        .bind(uom)
        .bind(quantity)
        .fetch_one(&mut **tx)
        .await?
    };
    Ok(balance_id)
}

pub async fn confirm_unexpected_receipt(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    load_id: i64,
    command: &ConfirmUnexpectedReceiptCommand<'_>,
) -> AppResult<ConfirmUnexpectedReceiptResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if load_id <= 0 {
        return Err(AppError::bad_request("load ID must be positive"));
    }
    let command = validate_command(command)?;
    let prepared = PreparedCommand::new_v1(context, OPERATION, &(load_id, command))?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;

    let load_hint = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id,status,type,dock_door_location_id
        FROM loads
        WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound load"))?;
    let inventory_owner_id: i64 = load_hint.try_get("inventory_owner_id")?;
    let facility_id: i64 = load_hint.try_get("facility_id")?;
    if !scope.includes_inventory_owner(inventory_owner_id) || !scope.includes_facility(facility_id)
    {
        return Err(AppError::not_found("inbound load"));
    }
    let load = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id,status,type,dock_door_location_id
        FROM loads
        WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
          AND inventory_owner_id=$3 AND facility_id=$4
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(inventory_owner_id)
    .bind(facility_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound load"))?;
    if let Some(result) = prepared
        .replayed::<ConfirmUnexpectedReceiptResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let load_type = LoadType::parse(&load.try_get::<String, _>("type")?)
        .ok_or_else(|| AppError::internal("invalid load type in database"))?;
    let load_status = LoadStatus::parse(&load.try_get::<String, _>("status")?)
        .ok_or_else(|| AppError::internal("invalid load status in database"))?;
    if load_type != LoadType::Inbound
        || !matches!(load_status, LoadStatus::Receiving | LoadStatus::Received)
    {
        return Err(AppError::conflict(
            "inbound unloading must be started before recording unexpected inventory",
        ));
    }

    let owner_facility = inventory_journal::owner_facility_scope(inventory_owner_id, facility_id)?;
    inventory_journal::lock_active_owner_facility_tx(&mut tx, access.tenant_id, owner_facility)
        .await?;

    let dock_door_location_id: Option<i64> = load.try_get("dock_door_location_id")?;
    let receiving_location_id: i64 = sqlx::query_scalar(
        r#"
        SELECT id FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
          AND deleted IS NULL AND active AND receivable AND barcode=$4
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(dock_door_location_id)
    .bind(command.receiving_location_barcode)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        AppError::conflict(
            "receiving location barcode does not match the load's active receiving dock",
        )
    })?;

    let item = sqlx::query(
        r#"
        SELECT item.id,item.packaging_unit
        FROM barcodes barcode
        INNER JOIN items item
          ON item.tenant_id=barcode.tenant_id AND item.id=barcode.item_id
        WHERE barcode.tenant_id=$1 AND barcode.deleted IS NULL
          AND item.deleted IS NULL AND lower(barcode.name)=lower($2)
        FOR SHARE OF barcode,item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.item_barcode)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("item barcode is not active in this tenant"))?;
    let item_id: i64 = item.try_get("id")?;
    let uom: String = item.try_get("packaging_unit")?;
    let expected_on_load: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM load_lines WHERE tenant_id=$1 AND load_id=$2 AND item_id=$3 AND deleted IS NULL)",
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(item_id)
    .fetch_one(&mut *tx)
    .await?;
    if (command.reason == UnexpectedReceiptReason::Excess && !expected_on_load)
        || (command.reason == UnexpectedReceiptReason::UnexpectedItem && expected_on_load)
    {
        return Err(AppError::conflict(
            "unexpected receipt reason does not match the load expectation",
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO inventory_owner_items(tenant_id,created,inventory_owner_id,item_id)
        VALUES ($1,$2,$3,$4)
        ON CONFLICT (tenant_id,inventory_owner_id,item_id) DO UPDATE SET deleted=NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(now)
    .bind(inventory_owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await?;

    let license_plate_id = license_plates::find_or_create_license_plate_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        command.license_plate_barcode,
        None,
        receiving_location_id,
    )
    .await?;
    let item_batch_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO item_batches
            (tenant_id,inventory_owner_id,created,item_id,uom,load_id,lot,serial,expiration)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .bind(now)
    .bind(item_id)
    .bind(&uom)
    .bind(load_id)
    .bind(command.lot)
    .bind(command.serial)
    .bind(command.expiration)
    .fetch_one(&mut *tx)
    .await?;
    inventory::ensure_location_accepts_batch_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        receiving_location_id,
        item_batch_id,
    )
    .await?;

    let unexpected_receipt_id: i64 =
        sqlx::query_scalar("SELECT nextval('unexpected_receipts_id_seq')")
            .fetch_one(&mut *tx)
            .await?;
    let inventory_transaction_id = inventory_journal::begin_batched_transaction_at(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Receive,
            reason: Some(command.reason.as_str()),
            reference_type: Some("unexpected_receipt"),
            reference_id: Some(unexpected_receipt_id),
            correlation_id: Some(&context.request_id),
            operation: OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
        now,
    )
    .await?;
    let inventory_balance_id = upsert_quarantine_balance_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        facility_id,
        receiving_location_id,
        license_plate_id,
        item_batch_id,
        item_id,
        &uom,
        command.quantity,
        now,
    )
    .await?;
    inventory_journal::append_entry(
        &mut tx,
        access.tenant_id,
        owner_facility,
        inventory_transaction_id,
        &JournalEntry {
            location_id: receiving_location_id,
            license_plate_id,
            item_batch_id,
            status: InventoryStatus::Quarantine,
            quantity_delta: command.quantity,
        },
    )
    .await?;
    let inventory_hold_id = inventory_hold::place_composed_inventory_hold_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        now,
        &inventory_hold::PlaceInventoryHoldCommand {
            inventory_balance_id,
            qty: command.quantity,
            reason: command.reason.hold_reason(),
            note: command.note,
            reference_type: Some("unexpected_receipt"),
            reference_id: Some(unexpected_receipt_id),
        },
    )
    .await?;

    sqlx::query(
        r#"
        INSERT INTO unexpected_receipts
            (id,tenant_id,inventory_owner_id,facility_id,load_id,item_id,uom,quantity,
             receiving_location_id,observed_item_barcode,
             observed_receiving_location_barcode,license_plate_id,license_plate_barcode,
             item_batch_id,lot,serial,expiration,reason_code,note,
             inventory_transaction_id,inventory_balance_id,inventory_hold_id,
             confirmed_by_user_id,confirmed_at)
        OVERRIDING SYSTEM VALUE
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,
                $19,$20,$21,$22,$23,$24)
        "#,
    )
    .bind(unexpected_receipt_id)
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(load_id)
    .bind(item_id)
    .bind(&uom)
    .bind(command.quantity)
    .bind(receiving_location_id)
    .bind(command.item_barcode)
    .bind(command.receiving_location_barcode)
    .bind(license_plate_id)
    .bind(command.license_plate_barcode)
    .bind(item_batch_id)
    .bind(command.lot)
    .bind(command.serial)
    .bind(command.expiration)
    .bind(command.reason.as_str())
    .bind(command.note)
    .bind(inventory_transaction_id)
    .bind(inventory_balance_id)
    .bind(inventory_hold_id)
    .bind(context.actor_id.get())
    .bind(now)
    .execute(&mut *tx)
    .await?;

    let metadata = serde_json::to_string(&serde_json::json!({
        "unexpected_receipt_id": unexpected_receipt_id,
        "item_id": item_id,
        "uom": uom,
        "quantity": command.quantity,
        "receiving_location_id": receiving_location_id,
        "license_plate_id": license_plate_id,
        "item_batch_id": item_batch_id,
        "inventory_balance_id": inventory_balance_id,
        "inventory_hold_id": inventory_hold_id,
        "inventory_transaction_id": inventory_transaction_id,
        "reason": command.reason.as_str(),
        "note": command.note,
    }))
    .map_err(|error| {
        AppError::internal(format!("encoding unexpected receipt activity: {error}"))
    })?;
    sqlx::query(
        r#"
        INSERT INTO load_activity(tenant_id,created,load_id,user_id,action,message,metadata_json)
        VALUES ($1,$2,$3,$4,'unexpected_receipt_confirmed',
                'unexpected inventory received into quarantine',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(now)
    .bind(load_id)
    .bind(context.actor_id.get())
    .bind(metadata)
    .execute(&mut *tx)
    .await?;

    let result = ConfirmUnexpectedReceiptResult {
        unexpected_receipt_id,
        load_id,
        inventory_owner_id,
        facility_id,
        item_id,
        uom,
        quantity: command.quantity,
        receiving_location_id,
        observed_item_barcode: command.item_barcode.to_owned(),
        observed_receiving_location_barcode: command.receiving_location_barcode.to_owned(),
        inventory_transaction_id,
        inventory_balance_id,
        item_batch_id,
        license_plate_id,
        license_plate_barcode: command.license_plate_barcode.map(str::to_owned),
        lot: command.lot.map(str::to_owned),
        serial: command.serial.map(str::to_owned),
        expiration: command.expiration,
        inventory_hold_id,
        inventory_status: InventoryStatus::Quarantine,
        reason: command.reason,
        note: command.note.map(str::to_owned),
        load_status,
        confirmed_by_user_id: context.actor_id.get(),
        confirmed_at: now,
    };
    let event_identity = command_request_hash(
        prepared.actor_id(),
        prepared.operation(),
        prepared.schema(),
        &(prepared.idempotency_key(), prepared.request_hash()),
    )?;
    let event_key = format!("inbound-unexpected-receipt:{}", event_identity.as_str());
    let aggregate_id = unexpected_receipt_id.to_string();
    let payload = serde_json::to_value(&result).map_err(|error| {
        AppError::internal(format!("encoding unexpected receipt event: {error}"))
    })?;
    outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(owner_facility.inventory_owner_id),
            facility_id: Some(owner_facility.facility_id),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "unexpected_receipt",
            aggregate_id: &aggregate_id,
            ordering_key: &event_key,
            aggregate_sequence: 1,
            event_type: "inbound.unexpected_receipt.confirmed",
            schema_version: 1,
            payload: &payload,
            occurred_at: now,
        },
    )
    .await?;

    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(inventory_transaction_id))
        .await?)
}
