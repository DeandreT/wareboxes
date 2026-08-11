//! Expected inbound receipt commands.

use serde::Serialize;
use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InboundReceiptExceptionReason, InboundReceiptQuarantineReason, InventoryStatus,
    InventoryTransactionType, LoadLineStatus, LoadStatus, LoadType, ReceiveExpectedInventoryResult,
    TenantAccess, Timestamp,
};
use wareboxes_domain::{OwnerFacilityScope, TenantId};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::{inventory, inventory_hold, license_plates};
use wareboxes_application::idempotency::{command_request_hash, PreparedCommand};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

const INTERNAL_OPERATION: &str = "inbound.receive_expected_inventory.v1";
const SCANNER_OPERATION: &str = "inbound.confirm_expected_receipt.v1";

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ReceiveExpectedInventoryCommand<'a> {
    pub receiving_location_id: Option<i64>,
    pub received_qty: i64,
    pub rejected_qty: i64,
    pub missing_qty: i64,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<&'a str>,
    pub lot: Option<&'a str>,
    pub serial: Option<&'a str>,
    pub expiration: Option<Timestamp>,
    pub exception_reason: Option<InboundReceiptExceptionReason>,
    pub exception_note: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum ConfirmExpectedReceiptCommand<'a> {
    Received {
        item_barcode: &'a str,
        receiving_location_barcode: &'a str,
        quantity: i64,
        license_plate_barcode: Option<&'a str>,
        lot: Option<&'a str>,
        serial: Option<&'a str>,
        expiration: Option<Timestamp>,
    },
    Quarantined {
        item_barcode: &'a str,
        receiving_location_barcode: &'a str,
        quantity: i64,
        license_plate_barcode: Option<&'a str>,
        lot: Option<&'a str>,
        serial: Option<&'a str>,
        expiration: Option<Timestamp>,
        reason: InboundReceiptQuarantineReason,
        note: Option<&'a str>,
    },
    Rejected {
        item_barcode: &'a str,
        quantity: i64,
        reason: InboundReceiptExceptionReason,
        note: Option<&'a str>,
    },
    Missing {
        quantity: i64,
        reason: InboundReceiptExceptionReason,
        note: Option<&'a str>,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ValidatedReceipt<'a> {
    receiving_location_id: Option<i64>,
    received_qty: i64,
    rejected_qty: i64,
    missing_qty: i64,
    license_plate_id: Option<i64>,
    license_plate_barcode: Option<&'a str>,
    lot: Option<&'a str>,
    serial: Option<&'a str>,
    expiration: Option<Timestamp>,
    exception_reason: Option<InboundReceiptExceptionReason>,
    exception_note: Option<&'a str>,
    quarantine_reason: Option<InboundReceiptQuarantineReason>,
}

impl ValidatedReceipt<'_> {
    fn physical_quantity(self) -> i64 {
        if self.quarantine_reason.is_some() {
            self.rejected_qty
        } else {
            self.received_qty
        }
    }

    fn inventory_status(self) -> Option<InventoryStatus> {
        (self.physical_quantity() > 0).then_some(if self.quarantine_reason.is_some() {
            InventoryStatus::Quarantine
        } else {
            InventoryStatus::Available
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ValidatedScannerReceipt<'a> {
    receipt: ValidatedReceipt<'a>,
    item_barcode: Option<&'a str>,
    receiving_location_barcode: Option<&'a str>,
}

fn validated_optional_text<'a>(
    value: Option<&'a str>,
    label: &str,
    maximum_characters: usize,
) -> AppResult<Option<&'a str>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::bad_request(format!("{label} cannot be blank")));
    }
    if trimmed.chars().count() > maximum_characters {
        return Err(AppError::bad_request(format!(
            "{label} cannot exceed {maximum_characters} characters"
        )));
    }
    Ok(Some(trimmed))
}

fn validate_command<'a>(
    command: &'a ReceiveExpectedInventoryCommand<'a>,
) -> AppResult<ValidatedReceipt<'a>> {
    if command.received_qty < 0 || command.rejected_qty < 0 || command.missing_qty < 0 {
        return Err(AppError::bad_request(
            "received, rejected, and missing quantities cannot be negative",
        ));
    }
    let resolved_quantity = command
        .received_qty
        .checked_add(command.rejected_qty)
        .and_then(|quantity| quantity.checked_add(command.missing_qty))
        .ok_or_else(|| AppError::bad_request("receipt quantity is too large"))?;
    if resolved_quantity == 0 {
        return Err(AppError::bad_request(
            "received, rejected, or missing quantity is required",
        ));
    }

    let license_plate_barcode =
        validated_optional_text(command.license_plate_barcode, "license plate barcode", 200)?;
    let lot = validated_optional_text(command.lot, "lot", 200)?;
    let serial = validated_optional_text(command.serial, "serial", 200)?;
    let exception_note = validated_optional_text(command.exception_note, "exception note", 1_000)?;

    if command.received_qty > 0 {
        if command.receiving_location_id.is_none() {
            return Err(AppError::bad_request(
                "receiving location is required for received inventory",
            ));
        }
    } else if command.receiving_location_id.is_some()
        || command.license_plate_id.is_some()
        || license_plate_barcode.is_some()
        || lot.is_some()
        || serial.is_some()
        || command.expiration.is_some()
    {
        return Err(AppError::bad_request(
            "stock dimensions are not allowed when no inventory is received",
        ));
    }
    if command.license_plate_id.is_some() && license_plate_barcode.is_some() {
        return Err(AppError::bad_request(
            "provide a license plate ID or barcode, not both",
        ));
    }
    if command
        .receiving_location_id
        .is_some_and(|location_id| location_id <= 0)
        || command
            .license_plate_id
            .is_some_and(|license_plate_id| license_plate_id <= 0)
    {
        return Err(AppError::bad_request(
            "receiving location and license plate IDs must be positive",
        ));
    }

    let has_exception = command.rejected_qty > 0 || command.missing_qty > 0;
    if has_exception && command.exception_reason.is_none() {
        return Err(AppError::bad_request(
            "exception reason is required for rejected or missing inventory",
        ));
    }
    if !has_exception && command.exception_reason.is_some() {
        return Err(AppError::bad_request(
            "exception reason requires rejected or missing inventory",
        ));
    }
    if command.exception_reason == Some(InboundReceiptExceptionReason::Other)
        && exception_note.is_none()
    {
        return Err(AppError::bad_request(
            "exception note is required when the reason is other",
        ));
    }

    Ok(ValidatedReceipt {
        receiving_location_id: command.receiving_location_id,
        received_qty: command.received_qty,
        rejected_qty: command.rejected_qty,
        missing_qty: command.missing_qty,
        license_plate_id: command.license_plate_id,
        license_plate_barcode,
        lot,
        serial,
        expiration: command.expiration,
        exception_reason: command.exception_reason,
        exception_note,
        quarantine_reason: None,
    })
}

fn required_text<'a>(value: &'a str, label: &str, maximum_characters: usize) -> AppResult<&'a str> {
    validated_optional_text(Some(value), label, maximum_characters)?
        .ok_or_else(|| AppError::internal(format!("validated {label} is missing")))
}

fn validate_scanner_command<'a>(
    command: &'a ConfirmExpectedReceiptCommand<'a>,
) -> AppResult<ValidatedScannerReceipt<'a>> {
    match command {
        ConfirmExpectedReceiptCommand::Received {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            expiration,
        } => {
            require_positive_quantity(*quantity)?;
            Ok(ValidatedScannerReceipt {
                receipt: ValidatedReceipt {
                    receiving_location_id: None,
                    received_qty: *quantity,
                    rejected_qty: 0,
                    missing_qty: 0,
                    license_plate_id: None,
                    license_plate_barcode: validated_optional_text(
                        *license_plate_barcode,
                        "license plate barcode",
                        200,
                    )?,
                    lot: validated_optional_text(*lot, "lot", 200)?,
                    serial: validated_optional_text(*serial, "serial", 200)?,
                    expiration: *expiration,
                    exception_reason: None,
                    exception_note: None,
                    quarantine_reason: None,
                },
                item_barcode: Some(required_text(item_barcode, "item barcode", 200)?),
                receiving_location_barcode: Some(required_text(
                    receiving_location_barcode,
                    "receiving location barcode",
                    200,
                )?),
            })
        }
        ConfirmExpectedReceiptCommand::Quarantined {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            expiration,
            reason,
            note,
        } => {
            require_positive_quantity(*quantity)?;
            let note = validated_optional_text(*note, "exception note", 1_000)?;
            if *reason == InboundReceiptQuarantineReason::Other && note.is_none() {
                return Err(AppError::bad_request(
                    "exception note is required when the quarantine reason is other",
                ));
            }
            Ok(ValidatedScannerReceipt {
                receipt: ValidatedReceipt {
                    receiving_location_id: None,
                    received_qty: 0,
                    rejected_qty: *quantity,
                    missing_qty: 0,
                    license_plate_id: None,
                    license_plate_barcode: validated_optional_text(
                        *license_plate_barcode,
                        "license plate barcode",
                        200,
                    )?,
                    lot: validated_optional_text(*lot, "lot", 200)?,
                    serial: validated_optional_text(*serial, "serial", 200)?,
                    expiration: *expiration,
                    exception_reason: Some(reason.exception_reason()),
                    exception_note: note,
                    quarantine_reason: Some(*reason),
                },
                item_barcode: Some(required_text(item_barcode, "item barcode", 200)?),
                receiving_location_barcode: Some(required_text(
                    receiving_location_barcode,
                    "receiving location barcode",
                    200,
                )?),
            })
        }
        ConfirmExpectedReceiptCommand::Rejected {
            item_barcode,
            quantity,
            reason,
            note,
        } => {
            require_positive_quantity(*quantity)?;
            let note = validated_optional_text(*note, "exception note", 1_000)?;
            require_other_note(*reason, note)?;
            Ok(ValidatedScannerReceipt {
                receipt: ValidatedReceipt {
                    receiving_location_id: None,
                    received_qty: 0,
                    rejected_qty: *quantity,
                    missing_qty: 0,
                    license_plate_id: None,
                    license_plate_barcode: None,
                    lot: None,
                    serial: None,
                    expiration: None,
                    exception_reason: Some(*reason),
                    exception_note: note,
                    quarantine_reason: None,
                },
                item_barcode: Some(required_text(item_barcode, "item barcode", 200)?),
                receiving_location_barcode: None,
            })
        }
        ConfirmExpectedReceiptCommand::Missing {
            quantity,
            reason,
            note,
        } => {
            require_positive_quantity(*quantity)?;
            let note = validated_optional_text(*note, "exception note", 1_000)?;
            require_other_note(*reason, note)?;
            Ok(ValidatedScannerReceipt {
                receipt: ValidatedReceipt {
                    receiving_location_id: None,
                    received_qty: 0,
                    rejected_qty: 0,
                    missing_qty: *quantity,
                    license_plate_id: None,
                    license_plate_barcode: None,
                    lot: None,
                    serial: None,
                    expiration: None,
                    exception_reason: Some(*reason),
                    exception_note: note,
                    quarantine_reason: None,
                },
                item_barcode: None,
                receiving_location_barcode: None,
            })
        }
    }
}

fn require_positive_quantity(quantity: i64) -> AppResult<()> {
    if quantity > 0 {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "expected receipt quantity must be positive",
        ))
    }
}

fn require_other_note(reason: InboundReceiptExceptionReason, note: Option<&str>) -> AppResult<()> {
    if reason == InboundReceiptExceptionReason::Other && note.is_none() {
        Err(AppError::bad_request(
            "exception note is required when the reason is other",
        ))
    } else {
        Ok(())
    }
}

fn load_line_status(expected: i64, received: i64, rejected: i64, missing: i64) -> LoadLineStatus {
    if received + rejected + missing >= expected {
        if received > 0 {
            LoadLineStatus::Received
        } else if rejected > 0 {
            LoadLineStatus::Rejected
        } else {
            LoadLineStatus::Missing
        }
    } else if received > 0 || rejected > 0 || missing > 0 {
        LoadLineStatus::Partial
    } else {
        LoadLineStatus::Pending
    }
}

fn require_scope(
    scope: &ScopeBindings,
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<()> {
    if scope.includes_inventory_owner(inventory_owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found("load line"))
    }
}

fn compatible_text_dimension(
    existing: Option<String>,
    requested: Option<&str>,
    label: &str,
) -> AppResult<Option<String>> {
    match (existing, requested) {
        (Some(existing), Some(requested)) if existing != requested => Err(AppError::conflict(
            format!("{label} does not match the expected load line"),
        )),
        (Some(existing), _) => Ok(Some(existing)),
        (None, requested) => Ok(requested.map(str::to_owned)),
    }
}

fn compatible_expiration(
    existing: Option<Timestamp>,
    requested: Option<Timestamp>,
) -> AppResult<Option<Timestamp>> {
    match (existing, requested) {
        (Some(existing), Some(requested)) if existing != requested => Err(AppError::conflict(
            "expiration does not match the expected load line",
        )),
        (Some(existing), _) => Ok(Some(existing)),
        (None, requested) => Ok(requested),
    }
}

async fn require_expected_item_barcode(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    item_id: i64,
    item_barcode: &str,
) -> AppResult<()> {
    let barcode_id = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id
        FROM barcodes
        WHERE tenant_id = $1
          AND item_id = $2
          AND deleted IS NULL
          AND lower(name) = lower($3)
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(item_id)
    .bind(item_barcode)
    .fetch_optional(&mut **tx)
    .await?;
    if barcode_id.is_some() {
        Ok(())
    } else {
        Err(AppError::conflict(
            "item barcode does not match the expected receipt line",
        ))
    }
}

async fn require_expected_receiving_location(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: i64,
    dock_door_location_id: Option<i64>,
    receiving_location_barcode: &str,
) -> AppResult<i64> {
    let Some(dock_door_location_id) = dock_door_location_id else {
        return Err(AppError::conflict("load has no receiving dock assigned"));
    };
    sqlx::query_scalar(
        r#"
        SELECT id
        FROM locations
        WHERE tenant_id = $1
          AND id = $2
          AND facility_id = $3
          AND deleted IS NULL
          AND active
          AND receivable
          AND barcode = $4
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(dock_door_location_id)
    .bind(facility_id)
    .bind(receiving_location_barcode)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        AppError::conflict(
            "receiving location barcode does not match the load's active receiving dock",
        )
    })
}

async fn enqueue_receipt_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    owner_facility: OwnerFacilityScope,
    prepared: &PreparedCommand,
    result: &ReceiveExpectedInventoryResult,
    receipt: &ValidatedReceipt<'_>,
) -> AppResult<()> {
    let event_identity = command_request_hash(
        prepared.actor_id(),
        prepared.operation(),
        prepared.schema(),
        &(prepared.idempotency_key(), prepared.request_hash()),
    )?;
    let event_key = format!("inbound-expected-receipt:{}", event_identity.as_str());
    let disposition = receipt_disposition(receipt);
    let payload = serde_json::json!({
        "load_id": result.load_id,
        "load_line_id": result.load_line_id,
        "disposition": disposition,
        "inventory_transaction_id": result.inventory_transaction_id,
        "item_batch_id": result.item_batch_id,
        "inventory_balance_id": result.inventory_balance_id,
        "license_plate_id": result.license_plate_id,
        "inventory_hold_id": result.inventory_hold_id,
        "inventory_status": result.inventory_status.as_ref().map(InventoryStatus::as_str),
        "inventory_owner_id": owner_facility.inventory_owner_id,
        "facility_id": owner_facility.facility_id,
        "receiving_location_id": receipt.receiving_location_id,
        "received_qty": receipt.received_qty,
        "rejected_qty": receipt.rejected_qty,
        "missing_qty": receipt.missing_qty,
        "exception_reason": receipt.exception_reason.map(|reason| reason.as_str()),
        "exception_note": receipt.exception_note,
        "load_status": result.load_status.as_str(),
        "line_status": result.line_status.as_str(),
        "remaining_quantity": result.remaining_quantity,
        "receive_completed": result.receive_completed,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner_facility.inventory_owner_id),
            facility_id: Some(owner_facility.facility_id),
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "inbound_receipt",
            aggregate_id: event_identity.as_str(),
            ordering_key: &event_key,
            aggregate_sequence: 1,
            event_type: "inbound.expected_receipt.confirmed",
            schema_version: 1,
            payload: &payload,
            occurred_at: now_iso(),
        },
    )
    .await?;
    Ok(())
}

fn receipt_disposition(receipt: &ValidatedReceipt<'_>) -> &'static str {
    if receipt.quarantine_reason.is_some() {
        "quarantined"
    } else if receipt.received_qty > 0 {
        "received"
    } else if receipt.rejected_qty > 0 {
        "rejected"
    } else {
        "missing"
    }
}

pub async fn receive_expected_inventory(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    load_line_id: i64,
    command: &ReceiveExpectedInventoryCommand<'_>,
) -> AppResult<ReceiveExpectedInventoryResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if load_line_id <= 0 {
        return Err(AppError::bad_request("load line ID must be positive"));
    }
    let receipt = validate_command(command)?;
    let prepared = PreparedCommand::new_v1(context, INTERNAL_OPERATION, &(load_line_id, receipt))?;
    execute_expected_receipt(
        db,
        access,
        context,
        load_line_id,
        receipt,
        None,
        prepared,
        INTERNAL_OPERATION,
    )
    .await
}

pub async fn confirm_expected_receipt(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    load_line_id: i64,
    command: &ConfirmExpectedReceiptCommand<'_>,
) -> AppResult<ReceiveExpectedInventoryResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if load_line_id <= 0 {
        return Err(AppError::bad_request("load line ID must be positive"));
    }
    let scanner_receipt = validate_scanner_command(command)?;
    let prepared =
        PreparedCommand::new_v1(context, SCANNER_OPERATION, &(load_line_id, scanner_receipt))?;
    execute_expected_receipt(
        db,
        access,
        context,
        load_line_id,
        scanner_receipt.receipt,
        Some(scanner_receipt),
        prepared,
        SCANNER_OPERATION,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_expected_receipt(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    load_line_id: i64,
    mut receipt: ValidatedReceipt<'_>,
    scanner_receipt: Option<ValidatedScannerReceipt<'_>>,
    prepared: PreparedCommand,
    operation: &'static str,
) -> AppResult<ReceiveExpectedInventoryResult> {
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;

    let load_id: i64 = sqlx::query_scalar(
        r#"
        SELECT load_id
        FROM load_lines
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_line_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("load line"))?;

    let load_row = sqlx::query(
        r#"
        SELECT status AS load_status, type AS load_type, facility_id,
               inventory_owner_id, dock_door_location_id
        FROM loads
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("load line"))?;

    let line_row = sqlx::query(
        r#"
        SELECT ll.item_id, ll.expected_qty, ll.received_qty, ll.rejected_qty,
               ll.missing_qty, ll.lot, ll.serial, ll.expiration,
               item.packaging_unit AS uom, item.deleted AS item_deleted
        FROM load_lines ll
        INNER JOIN items item
            ON item.tenant_id = ll.tenant_id
           AND item.id = ll.item_id
        WHERE ll.tenant_id = $1
          AND ll.id = $2
          AND ll.load_id = $3
          AND ll.deleted IS NULL
        FOR UPDATE OF ll
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_line_id)
    .bind(load_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("load line"))?;

    let inventory_owner_id: i64 = load_row.try_get("inventory_owner_id")?;
    let facility_id: i64 = load_row.try_get("facility_id")?;
    require_scope(&scope, inventory_owner_id, facility_id)?;
    let owner_facility = inventory_journal::owner_facility_scope(inventory_owner_id, facility_id)?;

    if let Some(result) = prepared
        .replayed::<ReceiveExpectedInventoryResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let is_customer_return: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM customer_return_load_plans
            WHERE tenant_id=$1 AND load_id=$2)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await?;
    if is_customer_return && receipt.received_qty > 0 && receipt.quarantine_reason.is_none() {
        return Err(AppError::conflict(
            "returned inventory must be received into quarantine for inspection",
        ));
    }

    inventory_journal::lock_active_owner_facility_tx(&mut tx, access.tenant_id, owner_facility)
        .await?;
    if line_row
        .try_get::<Option<Timestamp>, _>("item_deleted")?
        .is_some()
    {
        return Err(AppError::conflict(
            "expected receipt line item is no longer active",
        ));
    }

    let load_type = LoadType::parse(&load_row.try_get::<String, _>("load_type")?)
        .ok_or_else(|| AppError::internal("invalid load type in database"))?;
    if load_type != LoadType::Inbound {
        return Err(AppError::conflict(
            "expected inventory can only be received against an inbound load",
        ));
    }
    let load_status = LoadStatus::parse(&load_row.try_get::<String, _>("load_status")?)
        .ok_or_else(|| AppError::internal("invalid load status in database"))?;
    if load_status != LoadStatus::Receiving {
        return Err(AppError::conflict(
            "inbound unloading must be started before receiving inventory",
        ));
    }

    if let Some(scanner_receipt) = scanner_receipt {
        if let Some(item_barcode) = scanner_receipt.item_barcode {
            require_expected_item_barcode(
                &mut tx,
                access.tenant_id,
                line_row.try_get("item_id")?,
                item_barcode,
            )
            .await?;
        }
        if let Some(receiving_location_barcode) = scanner_receipt.receiving_location_barcode {
            receipt.receiving_location_id = Some(
                require_expected_receiving_location(
                    &mut tx,
                    access.tenant_id,
                    facility_id,
                    load_row.try_get("dock_door_location_id")?,
                    receiving_location_barcode,
                )
                .await?,
            );
        }
    }

    let expected_qty: i64 = line_row.try_get("expected_qty")?;
    let prior_received_qty: i64 = line_row.try_get("received_qty")?;
    let prior_rejected_qty: i64 = line_row.try_get("rejected_qty")?;
    let prior_missing_qty: i64 = line_row.try_get("missing_qty")?;
    let cumulative_received_qty = prior_received_qty
        .checked_add(receipt.received_qty)
        .ok_or_else(|| AppError::bad_request("received quantity is too large"))?;
    let cumulative_rejected_qty = prior_rejected_qty
        .checked_add(receipt.rejected_qty)
        .ok_or_else(|| AppError::bad_request("rejected quantity is too large"))?;
    let cumulative_missing_qty = prior_missing_qty
        .checked_add(receipt.missing_qty)
        .ok_or_else(|| AppError::bad_request("missing quantity is too large"))?;
    let cumulative_resolved_qty = cumulative_received_qty
        .checked_add(cumulative_rejected_qty)
        .and_then(|quantity| quantity.checked_add(cumulative_missing_qty))
        .ok_or_else(|| AppError::bad_request("resolved quantity is too large"))?;
    if cumulative_resolved_qty > expected_qty {
        return Err(AppError::conflict(
            "cannot receive, reject, or mark missing more than expected quantity",
        ));
    }

    let lot = compatible_text_dimension(line_row.try_get("lot")?, receipt.lot, "lot")?;
    let serial = compatible_text_dimension(line_row.try_get("serial")?, receipt.serial, "serial")?;
    let expiration = compatible_expiration(line_row.try_get("expiration")?, receipt.expiration)?;
    let line_status = load_line_status(
        expected_qty,
        cumulative_received_qty,
        cumulative_rejected_qty,
        cumulative_missing_qty,
    );

    let mut inventory_transaction_id = None;
    let mut item_batch_id = None;
    let mut inventory_balance_id = None;
    let mut resolved_license_plate_id = None;
    let mut inventory_hold_id = None;
    let physical_quantity = receipt.physical_quantity();
    let inventory_status = receipt.inventory_status();

    if physical_quantity > 0 {
        let receiving_location_id = receipt
            .receiving_location_id
            .ok_or_else(|| AppError::internal("validated receipt is missing its location"))?;
        let location_exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM locations
                WHERE tenant_id = $1
                  AND id = $2
                  AND facility_id = $3
                  AND deleted IS NULL
                  AND active
                  AND receivable
                FOR SHARE
            )
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(receiving_location_id)
        .bind(facility_id)
        .fetch_one(&mut *tx)
        .await?;
        if !location_exists {
            return Err(AppError::bad_request(
                "receiving location must be active and receivable in the load facility",
            ));
        }

        resolved_license_plate_id = license_plates::find_or_create_license_plate_tx(
            &mut tx,
            access.tenant_id,
            inventory_owner_id,
            receipt.license_plate_barcode,
            receipt.license_plate_id,
            receiving_location_id,
        )
        .await?;

        let item_id: i64 = line_row.try_get("item_id")?;
        let uom: String = line_row.try_get("uom")?;
        sqlx::query(
            r#"
            INSERT INTO inventory_owner_items
                (tenant_id, created, inventory_owner_id, item_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (tenant_id, inventory_owner_id, item_id)
            DO UPDATE SET deleted = NULL
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(now)
        .bind(inventory_owner_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;

        let batch_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO item_batches
                (tenant_id, inventory_owner_id, created, item_id, uom,
                 load_id, lot, serial, expiration)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(now)
        .bind(item_id)
        .bind(&uom)
        .bind(load_id)
        .bind(lot.as_deref())
        .bind(serial.as_deref())
        .bind(expiration)
        .fetch_one(&mut *tx)
        .await?;
        item_batch_id = Some(batch_id);

        inventory::ensure_location_accepts_batch_tx(
            &mut tx,
            access.tenant_id,
            inventory_owner_id,
            receiving_location_id,
            batch_id,
        )
        .await?;

        let transaction_reason = receipt
            .exception_reason
            .map_or("expected_receipt", InboundReceiptExceptionReason::as_str);
        let transaction_id = inventory_journal::begin_transaction(
            &mut tx,
            &JournalCommand {
                tenant_id: access.tenant_id,
                owner_facility,
                actor_user_id: context.actor_id.get(),
                transaction_type: InventoryTransactionType::Receive,
                reason: Some(transaction_reason),
                reference_type: Some("load_line"),
                reference_id: Some(load_line_id),
                correlation_id: Some(&context.request_id),
                operation,
                idempotency_key: Some(prepared.idempotency_key()),
                request_hash: prepared.request_hash(),
            },
        )
        .await?;
        inventory_transaction_id = Some(transaction_id);
        let status = inventory_status
            .ok_or_else(|| AppError::internal("physical receipt is missing inventory status"))?;

        if let Some(license_plate_id) = resolved_license_plate_id {
            let balance_id = sqlx::query_scalar(
                r#"
                INSERT INTO inventory_balances
                    (tenant_id, inventory_owner_id, created, modified, facility_id,
                     location_id, license_plate_id, item_batch_id, item_id, uom,
                     status, qty_on_hand, qty_reserved)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 0)
                ON CONFLICT (
                    tenant_id, inventory_owner_id, location_id, license_plate_id,
                    item_batch_id, uom, status
                ) WHERE license_plate_id IS NOT NULL
                DO UPDATE SET
                    qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                    modified = excluded.modified,
                    deleted = NULL
                RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(inventory_owner_id)
            .bind(now)
            .bind(now)
            .bind(facility_id)
            .bind(receiving_location_id)
            .bind(license_plate_id)
            .bind(batch_id)
            .bind(item_id)
            .bind(&uom)
            .bind(status.as_str())
            .bind(physical_quantity)
            .fetch_one(&mut *tx)
            .await?;
            inventory_balance_id = Some(balance_id);
        } else {
            let balance_id = sqlx::query_scalar(
                r#"
                INSERT INTO inventory_balances
                    (tenant_id, inventory_owner_id, created, modified, facility_id,
                     location_id, license_plate_id, item_batch_id, item_id, uom,
                     status, qty_on_hand, qty_reserved)
                VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9, $10, $11, 0)
                ON CONFLICT (
                    tenant_id, inventory_owner_id, location_id, item_batch_id,
                    uom, status
                ) WHERE license_plate_id IS NULL
                DO UPDATE SET
                    qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                    modified = excluded.modified,
                    deleted = NULL
                RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(inventory_owner_id)
            .bind(now)
            .bind(now)
            .bind(facility_id)
            .bind(receiving_location_id)
            .bind(batch_id)
            .bind(item_id)
            .bind(&uom)
            .bind(status.as_str())
            .bind(physical_quantity)
            .fetch_one(&mut *tx)
            .await?;
            inventory_balance_id = Some(balance_id);
        }

        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: receiving_location_id,
                license_plate_id: resolved_license_plate_id,
                item_batch_id: batch_id,
                status,
                quantity_delta: physical_quantity,
            },
        )
        .await?;

        if let Some(quarantine_reason) = receipt.quarantine_reason {
            inventory_hold_id = Some(
                inventory_hold::place_composed_inventory_hold_tx(
                    &mut tx,
                    access.tenant_id,
                    context.actor_id.get(),
                    now,
                    &inventory_hold::PlaceInventoryHoldCommand {
                        inventory_balance_id: inventory_balance_id.ok_or_else(|| {
                            AppError::internal("physical receipt is missing inventory balance")
                        })?,
                        qty: physical_quantity,
                        reason: quarantine_reason.hold_reason(),
                        note: receipt.exception_note,
                        reference_type: Some("expected_receipt_line"),
                        reference_id: Some(load_line_id),
                    },
                )
                .await?,
            );
        }
    }

    sqlx::query(
        r#"
        UPDATE load_lines
        SET received_qty = $1,
            rejected_qty = $2,
            missing_qty = $3,
            missing_confirmed_by = COALESCE($4, missing_confirmed_by),
            missing_confirmed_at = COALESCE($5, missing_confirmed_at),
            lot = $6,
            serial = $7,
            expiration = $8,
            status = $9
        WHERE tenant_id = $10
          AND id = $11
        "#,
    )
    .bind(cumulative_received_qty)
    .bind(cumulative_rejected_qty)
    .bind(cumulative_missing_qty)
    .bind((receipt.missing_qty > 0).then_some(context.actor_id.get()))
    .bind((receipt.missing_qty > 0).then_some(now))
    .bind(lot.as_deref())
    .bind(serial.as_deref())
    .bind(expiration)
    .bind(line_status.as_str())
    .bind(access.tenant_id.get())
    .bind(load_line_id)
    .execute(&mut *tx)
    .await?;

    let open_line_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM load_lines
        WHERE tenant_id = $1
          AND load_id = $2
          AND deleted IS NULL
          AND status IN ('pending', 'partial')
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await?;
    let receive_completed = open_line_count == 0;
    let next_load_status = if receive_completed {
        LoadStatus::Received
    } else {
        LoadStatus::Receiving
    };
    sqlx::query(
        r#"
        UPDATE loads
        SET status = $1,
            receive_completed = $2,
            actual_time = COALESCE(actual_time, $3)
        WHERE tenant_id = $4
          AND id = $5
        "#,
    )
    .bind(next_load_status.as_str())
    .bind(receive_completed)
    .bind(now)
    .bind(access.tenant_id.get())
    .bind(load_id)
    .execute(&mut *tx)
    .await?;

    let activity_metadata = serde_json::to_string(&serde_json::json!({
        "load_line_id": load_line_id,
        "disposition": receipt_disposition(&receipt),
        "receiving_location_id": receipt.receiving_location_id,
        "license_plate_id": resolved_license_plate_id,
        "item_batch_id": item_batch_id,
        "inventory_balance_id": inventory_balance_id,
        "inventory_hold_id": inventory_hold_id,
        "inventory_status": inventory_status.as_ref().map(InventoryStatus::as_str),
        "received_qty": receipt.received_qty,
        "rejected_qty": receipt.rejected_qty,
        "missing_qty": receipt.missing_qty,
        "exception_reason": receipt.exception_reason.map(|reason| reason.as_str()),
        "exception_note": receipt.exception_note,
    }))
    .map_err(|error| AppError::internal(format!("encoding receipt activity: {error}")))?;
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id, created, load_id, user_id, action, message, metadata_json)
        VALUES ($1, $2, $3, $4, 'expected_receipt_confirmed',
                'expected receipt confirmation recorded', $5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(now)
    .bind(load_id)
    .bind(context.actor_id.get())
    .bind(activity_metadata)
    .execute(&mut *tx)
    .await?;

    let result = ReceiveExpectedInventoryResult {
        load_id,
        load_line_id,
        inventory_transaction_id,
        item_batch_id,
        inventory_balance_id,
        license_plate_id: resolved_license_plate_id,
        inventory_hold_id,
        inventory_status,
        load_status: next_load_status,
        line_status,
        cumulative_received_qty,
        cumulative_rejected_qty,
        cumulative_missing_qty,
        remaining_quantity: expected_qty - cumulative_resolved_qty,
        receive_completed,
    };
    enqueue_receipt_event(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        owner_facility,
        &prepared,
        &result,
        &receipt,
    )
    .await?;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, inventory_transaction_id)
        .await?)
}
