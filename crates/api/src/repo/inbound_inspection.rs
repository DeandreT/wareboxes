//! Atomic terminal disposition of one quarantined inbound receipt hold.

use serde::Serialize;
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inbound_inspection::{
    DisposeInboundInspectionCommand, DisposeInboundInspectionResult,
    DISPOSE_INBOUND_INSPECTION_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess, Timestamp};
use wareboxes_domain::{
    decide_inbound_inspection, FacilityId, InboundInspectionDispositionId,
    InboundInspectionOutcome, InboundInspectionTargetStatus, InventoryBalanceId, InventoryHoldId,
    InventoryOwnerId, ItemBatchId, LocationId, TenantId, UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::inventory_locking::lock_license_plate;

const PERMISSION: &str = "wms_supervisor";

#[derive(Debug, Serialize)]
struct ValidatedCommand<'a> {
    inventory_hold_id: InventoryHoldId,
    outcome: InboundInspectionOutcome,
    note: &'a str,
}

#[derive(Debug)]
struct HoldTarget {
    inventory_owner_id: i64,
    facility_id: i64,
    inventory_balance_id: i64,
    location_id: i64,
    license_plate_id: Option<i64>,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    quantity: i64,
    inventory_status: String,
    status: String,
    reference_type: String,
    reference_id: i64,
}

#[derive(Debug, Clone)]
struct Balance {
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

fn id<T>(
    value: i64,
    constructor: fn(i64) -> Result<T, wareboxes_domain::InvalidId>,
) -> AppResult<T> {
    constructor(value).map_err(|error| AppError::internal(error.to_string()))
}

fn core_status(target: InboundInspectionTargetStatus) -> InventoryStatus {
    match target {
        InboundInspectionTargetStatus::Available => InventoryStatus::Available,
        InboundInspectionTargetStatus::Damaged => InventoryStatus::Damaged,
    }
}

fn status_reason(outcome: InboundInspectionOutcome) -> &'static str {
    match outcome {
        InboundInspectionOutcome::Approved => "inspection_passed",
        InboundInspectionOutcome::Damaged => "damage_confirmed",
    }
}

async fn visible_hold_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    hold_id: InventoryHoldId,
) -> AppResult<(i64, Option<i64>, i64, i64)> {
    let row = sqlx::query(
        r#"
        SELECT inventory_balance_id, license_plate_id,
               inventory_owner_id, facility_id
        FROM inventory_holds
        WHERE tenant_id = $1
          AND id = $2
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(hold_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound inspection hold"))?;
    Ok((
        row.try_get("inventory_balance_id")?,
        row.try_get("license_plate_id")?,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    ))
}

async fn lock_hold_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    hold_id: InventoryHoldId,
) -> AppResult<HoldTarget> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, inventory_balance_id,
               location_id, license_plate_id, item_batch_id, item_id, uom,
               qty, inventory_status, status, reference_type, reference_id
        FROM inventory_holds
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(hold_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound inspection hold"))?;
    let reference_type: Option<String> = row.try_get("reference_type")?;
    let reference_id: Option<i64> = row.try_get("reference_id")?;
    Ok(HoldTarget {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        quantity: row.try_get("qty")?,
        inventory_status: row.try_get("inventory_status")?,
        status: row.try_get("status")?,
        reference_type: reference_type
            .ok_or_else(|| AppError::conflict("hold is not tied to an inbound receipt"))?,
        reference_id: reference_id
            .ok_or_else(|| AppError::conflict("hold is not tied to an inbound receipt"))?,
    })
}

async fn lock_balances_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    source_id: i64,
    target_status: InventoryStatus,
) -> AppResult<Vec<Balance>> {
    let source = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, location_id, license_plate_id,
               item_batch_id, item_id, uom, status
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(source_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound inspection inventory"))?;
    let rows = sqlx::query(
        r#"
        SELECT id, inventory_owner_id, facility_id, location_id, license_plate_id,
               item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved,
               qty_held, deleted IS NULL AS active
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND facility_id = $3
          AND location_id = $4
          AND license_plate_id IS NOT DISTINCT FROM $5
          AND item_batch_id = $6
          AND item_id = $7
          AND uom = $8
          AND status IN ('quarantine', $9)
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(source.try_get::<i64, _>("inventory_owner_id")?)
    .bind(source.try_get::<i64, _>("facility_id")?)
    .bind(source.try_get::<i64, _>("location_id")?)
    .bind(source.try_get::<Option<i64>, _>("license_plate_id")?)
    .bind(source.try_get::<i64, _>("item_batch_id")?)
    .bind(source.try_get::<i64, _>("item_id")?)
    .bind(source.try_get::<String, _>("uom")?)
    .bind(target_status.as_str())
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Balance {
                id: row.try_get("id")?,
                inventory_owner_id: row.try_get("inventory_owner_id")?,
                facility_id: row.try_get("facility_id")?,
                location_id: row.try_get("location_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                status: InventoryStatus::parse(&row.try_get::<String, _>("status")?)
                    .ok_or_else(|| AppError::internal("invalid inventory status"))?,
                qty_on_hand: row.try_get("qty_on_hand")?,
                qty_reserved: row.try_get("qty_reserved")?,
                qty_held: row.try_get("qty_held")?,
                active: row.try_get("active")?,
            })
        })
        .collect()
}

fn require_hold_matches_balance(hold: &HoldTarget, balance: &Balance) -> AppResult<()> {
    if hold.inventory_balance_id != balance.id
        || hold.inventory_owner_id != balance.inventory_owner_id
        || hold.facility_id != balance.facility_id
        || hold.location_id != balance.location_id
        || hold.license_plate_id != balance.license_plate_id
        || hold.item_batch_id != balance.item_batch_id
        || hold.item_id != balance.item_id
        || hold.uom != balance.uom
    {
        return Err(AppError::conflict(
            "receipt hold inventory changed while acquiring inspection locks",
        ));
    }
    Ok(())
}

async fn require_receipt_reference_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    hold_id: InventoryHoldId,
    hold: &HoldTarget,
) -> AppResult<i64> {
    let load_id = match hold.reference_type.as_str() {
        "expected_receipt_line" => {
            sqlx::query_scalar(
                r#"
                SELECT line.load_id
                FROM load_lines line
                INNER JOIN loads load
                  ON load.tenant_id = line.tenant_id AND load.id = line.load_id
                WHERE line.tenant_id = $1
                  AND line.id = $2
                  AND line.item_id = $3
                  AND line.deleted IS NULL
                  AND load.inventory_owner_id = $4
                  AND load.facility_id = $5
                  AND load.type = 'inbound'
                  AND load.deleted IS NULL
                FOR SHARE OF line, load
                "#,
            )
            .bind(tenant_id.get())
            .bind(hold.reference_id)
            .bind(hold.item_id)
            .bind(hold.inventory_owner_id)
            .bind(hold.facility_id)
            .fetch_optional(&mut **tx)
            .await?
        }
        "unexpected_receipt" => {
            sqlx::query_scalar(
                r#"
                SELECT load_id
                FROM unexpected_receipts
                WHERE tenant_id = $1
                  AND id = $2
                  AND inventory_owner_id = $3
                  AND facility_id = $4
                  AND inventory_hold_id = $5
                  AND inventory_balance_id = $6
                  AND item_id = $7
                  AND quantity = $8
                "#,
            )
            .bind(tenant_id.get())
            .bind(hold.reference_id)
            .bind(hold.inventory_owner_id)
            .bind(hold.facility_id)
            .bind(hold_id.get())
            .bind(hold.inventory_balance_id)
            .bind(hold.item_id)
            .bind(hold.quantity)
            .fetch_optional(&mut **tx)
            .await?
        }
        _ => return Err(AppError::conflict("hold is not tied to an inbound receipt")),
    };
    load_id.ok_or_else(|| AppError::conflict("inbound receipt evidence is no longer valid"))
}

async fn release_hold_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    hold_id: InventoryHoldId,
    inspected_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_holds
        SET modified = $1, deleted = $1, released_by = $2,
            released_at = $1, status = 'released'
        WHERE tenant_id = $3 AND id = $4
          AND deleted IS NULL AND status = 'active'
        "#,
    )
    .bind(inspected_at)
    .bind(actor_user_id)
    .bind(tenant_id.get())
    .bind(hold_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("inbound inspection hold is not active"));
    }
    Ok(())
}

async fn decrement_source_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    source: &Balance,
    quantity: i64,
    now: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1, modified = $2
        WHERE tenant_id = $3 AND inventory_owner_id = $4 AND id = $5
          AND status = 'quarantine' AND deleted IS NULL
          AND qty_on_hand = $6 AND qty_reserved = 0 AND qty_held = 0
          AND qty_on_hand >= $1
        "#,
    )
    .bind(quantity)
    .bind(now)
    .bind(tenant_id.get())
    .bind(source.inventory_owner_id)
    .bind(source.id)
    .bind(source.qty_on_hand)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "quarantined inventory changed during inspection",
        ));
    }
    Ok(())
}

async fn increment_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    source: &Balance,
    target_status: InventoryStatus,
    quantity: i64,
    now: Timestamp,
) -> AppResult<i64> {
    let target_id = if source.license_plate_id.is_some() {
        sqlx::query_scalar(
            r#"
            INSERT INTO inventory_balances (
                tenant_id, inventory_owner_id, created, modified, facility_id,
                location_id, license_plate_id, item_batch_id, item_id, uom,
                status, qty_on_hand, qty_reserved, qty_held
            ) VALUES ($1,$2,$3,$3,$4,$5,$6,$7,$8,$9,$10,$11,0,0)
            ON CONFLICT (tenant_id, inventory_owner_id, location_id,
                         license_plate_id, item_batch_id, uom, status)
                WHERE license_plate_id IS NOT NULL DO UPDATE
            SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                modified = excluded.modified, deleted = NULL
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
        .bind(target_status.as_str())
        .bind(quantity)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO inventory_balances (
                tenant_id, inventory_owner_id, created, modified, facility_id,
                location_id, license_plate_id, item_batch_id, item_id, uom,
                status, qty_on_hand, qty_reserved, qty_held
            ) VALUES ($1,$2,$3,$3,$4,$5,NULL,$6,$7,$8,$9,$10,0,0)
            ON CONFLICT (tenant_id, inventory_owner_id, location_id,
                         item_batch_id, uom, status)
                WHERE license_plate_id IS NULL DO UPDATE
            SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                modified = excluded.modified, deleted = NULL
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
        .bind(target_status.as_str())
        .bind(quantity)
        .fetch_one(&mut **tx)
        .await?
    };
    Ok(target_id)
}

#[allow(clippy::too_many_arguments)]
async fn insert_evidence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    inspected_at: Timestamp,
    hold_id: InventoryHoldId,
    hold: &HoldTarget,
    target_balance_id: i64,
    outcome: InboundInspectionOutcome,
    target_status: InboundInspectionTargetStatus,
    note: &str,
    transaction_id: i64,
    transition_id: i64,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        r#"
        INSERT INTO inbound_inspection_dispositions (
            tenant_id, inventory_owner_id, facility_id, inventory_hold_id,
            source_inventory_balance_id, target_inventory_balance_id,
            location_id, license_plate_id, item_batch_id, item_id, uom,
            quantity, outcome, target_status, note, inventory_transaction_id,
            inventory_status_transition_id, source_reference_type,
            source_reference_id, inspected_by_user_id, inspected_at
        ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
            $18,$19,$20,$21
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(hold.inventory_owner_id)
    .bind(hold.facility_id)
    .bind(hold_id.get())
    .bind(hold.inventory_balance_id)
    .bind(target_balance_id)
    .bind(hold.location_id)
    .bind(hold.license_plate_id)
    .bind(hold.item_batch_id)
    .bind(hold.item_id)
    .bind(&hold.uom)
    .bind(hold.quantity)
    .bind(outcome.as_str())
    .bind(target_status.as_str())
    .bind(note)
    .bind(transaction_id)
    .bind(transition_id)
    .bind(&hold.reference_type)
    .bind(hold.reference_id)
    .bind(actor_user_id)
    .bind(inspected_at)
    .fetch_one(&mut **tx)
    .await?)
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_events_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    inspected_at: Timestamp,
    load_id: i64,
    disposition_id: i64,
    hold_id: InventoryHoldId,
    hold: &HoldTarget,
    target_balance_id: i64,
    outcome: InboundInspectionOutcome,
    target_status: InboundInspectionTargetStatus,
    note: &str,
    transaction_id: i64,
) -> AppResult<()> {
    let owner = InventoryOwnerId::new(hold.inventory_owner_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility =
        FacilityId::new(hold.facility_id).map_err(|error| AppError::internal(error.to_string()))?;
    let common = serde_json::json!({
        "disposition_id": disposition_id,
        "inventory_hold_id": hold_id.get(),
        "inventory_transaction_id": transaction_id,
        "source_inventory_balance_id": hold.inventory_balance_id,
        "target_inventory_balance_id": target_balance_id,
        "inventory_owner_id": hold.inventory_owner_id,
        "facility_id": hold.facility_id,
        "location_id": hold.location_id,
        "license_plate_id": hold.license_plate_id,
        "item_batch_id": hold.item_batch_id,
        "item_id": hold.item_id,
        "uom": hold.uom,
        "quantity": hold.quantity,
        "outcome": outcome.as_str(),
        "target_status": target_status.as_str(),
        "note": note,
        "source_reference_type": hold.reference_type,
        "source_reference_id": hold.reference_id,
    });
    let aggregate_id = disposition_id.to_string();
    let event_key = format!("inbound-inspection-disposition:{disposition_id}:recorded");
    let ordering_key = format!("load:{load_id}");
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner),
            facility_id: Some(facility),
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "inbound_inspection_disposition",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: 1,
            event_type: "inbound.inspection.disposed",
            schema_version: 1,
            payload: &common,
            occurred_at: inspected_at,
        },
    )
    .await?;
    let hold_ordering_key = format!("inventory-hold:{}", hold_id.get());
    let hold_aggregate_id = hold_id.get().to_string();
    let hold_event_key = format!("inventory-hold:{}:released", hold_id.get());
    let hold_payload = serde_json::json!({
        "hold_id": hold_id.get(),
        "inventory_balance_id": hold.inventory_balance_id,
        "inventory_owner_id": hold.inventory_owner_id,
        "facility_id": hold.facility_id,
        "location_id": hold.location_id,
        "license_plate_id": hold.license_plate_id,
        "item_batch_id": hold.item_batch_id,
        "item_id": hold.item_id,
        "uom": hold.uom,
        "inventory_status": "quarantine",
        "released_quantity": hold.quantity,
        "reference_type": hold.reference_type,
        "reference_id": hold.reference_id,
        "inspection_disposition_id": disposition_id,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner),
            facility_id: Some(facility),
            actor_user_id: Some(actor_user_id),
            event_key: &hold_event_key,
            aggregate_type: "inventory_hold",
            aggregate_id: &hold_aggregate_id,
            ordering_key: &hold_ordering_key,
            aggregate_sequence: 2,
            event_type: "inventory.hold.released",
            schema_version: 1,
            payload: &hold_payload,
            occurred_at: inspected_at,
        },
    )
    .await?;
    let transaction_aggregate_id = transaction_id.to_string();
    let transaction_event_key = format!("inventory-transaction:{transaction_id}:status-changed");
    let transaction_ordering_key = format!("inventory-transaction:{transaction_id}");
    let transaction_payload = serde_json::json!({
        "inventory_transaction_id": transaction_id,
        "source_inventory_balance_id": hold.inventory_balance_id,
        "target_inventory_balance_id": target_balance_id,
        "inventory_owner_id": hold.inventory_owner_id,
        "facility_id": hold.facility_id,
        "location_id": hold.location_id,
        "license_plate_id": hold.license_plate_id,
        "item_batch_id": hold.item_batch_id,
        "item_id": hold.item_id,
        "uom": hold.uom,
        "quantity": hold.quantity,
        "from_status": "quarantine",
        "to_status": target_status.as_str(),
        "reason": status_reason(outcome),
        "note": note,
        "reference_type": "inbound_inspection_hold",
        "reference_id": hold_id.get(),
        "inspection_disposition_id": disposition_id,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner),
            facility_id: Some(facility),
            actor_user_id: Some(actor_user_id),
            event_key: &transaction_event_key,
            aggregate_type: "inventory_transaction",
            aggregate_id: &transaction_aggregate_id,
            ordering_key: &transaction_ordering_key,
            aggregate_sequence: 2,
            event_type: "inventory.status.changed",
            schema_version: 1,
            payload: &transaction_payload,
            occurred_at: inspected_at,
        },
    )
    .await?;
    Ok(())
}

pub async fn dispose(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &DisposeInboundInspectionCommand,
) -> AppResult<DisposeInboundInspectionResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let validated = ValidatedCommand {
        inventory_hold_id: command.inventory_hold_id,
        outcome: command.outcome,
        note: command.note.as_str(),
    };
    let prepared =
        PreparedCommand::new_v1(context, DISPOSE_INBOUND_INSPECTION_OPERATION, &validated)?;
    let inspected_at = now_iso();
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    let (source_balance_id, hinted_plate_id, hinted_owner_id, hinted_facility_id) =
        visible_hold_hint_tx(&mut tx, access.tenant_id, &scope, command.inventory_hold_id).await?;
    let owner_facility =
        inventory_journal::owner_facility_scope(hinted_owner_id, hinted_facility_id)?;
    inventory_journal::lock_active_owner_facility_tx(&mut tx, access.tenant_id, owner_facility)
        .await?;
    lock_license_plate(&mut tx, access.tenant_id, hinted_plate_id).await?;
    let target_status = command.outcome.target_status();
    let locked = lock_balances_tx(
        &mut tx,
        access.tenant_id,
        source_balance_id,
        core_status(target_status),
    )
    .await?;
    let source = locked
        .iter()
        .find(|balance| balance.id == source_balance_id)
        .cloned()
        .ok_or_else(|| AppError::conflict("inspection source changed while acquiring locks"))?;
    let hold = lock_hold_tx(&mut tx, access.tenant_id, command.inventory_hold_id).await?;
    require_hold_matches_balance(&hold, &source)?;

    if let Some(result) = prepared
        .replayed::<DisposeInboundInspectionResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    if source.license_plate_id != hinted_plate_id || !source.active {
        return Err(AppError::conflict(
            "inspection source changed while acquiring locks",
        ));
    }
    decide_inbound_inspection(
        hold.status == "active",
        hold.inventory_status == "quarantine" && source.status == InventoryStatus::Quarantine,
        hold.quantity,
        command.outcome,
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    if !matches!(
        hold.reference_type.as_str(),
        "expected_receipt_line" | "unexpected_receipt"
    ) {
        return Err(AppError::conflict("hold is not tied to an inbound receipt"));
    }
    if source.qty_on_hand < hold.quantity
        || source.qty_reserved != 0
        || source.qty_held != hold.quantity
    {
        return Err(AppError::conflict(
            "receipt hold no longer covers the exact quarantined quantity",
        ));
    }
    let load_id =
        require_receipt_reference_tx(&mut tx, access.tenant_id, command.inventory_hold_id, &hold)
            .await?;
    release_hold_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command.inventory_hold_id,
        inspected_at,
    )
    .await?;
    let reason = status_reason(command.outcome);
    let transaction_id = inventory_journal::begin_batched_transaction_at(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::StatusChange,
            reason: Some(reason),
            reference_type: Some("inbound_inspection_hold"),
            reference_id: Some(command.inventory_hold_id.get()),
            correlation_id: Some(&context.request_id),
            operation: DISPOSE_INBOUND_INSPECTION_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
        inspected_at,
    )
    .await?;
    decrement_source_tx(
        &mut tx,
        access.tenant_id,
        &source,
        hold.quantity,
        inspected_at,
    )
    .await?;
    let target_balance_id = increment_target_tx(
        &mut tx,
        access.tenant_id,
        &source,
        core_status(target_status),
        hold.quantity,
        inspected_at,
    )
    .await?;
    for (status, quantity_delta) in [
        (InventoryStatus::Quarantine, -hold.quantity),
        (core_status(target_status), hold.quantity),
    ] {
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: hold.location_id,
                license_plate_id: hold.license_plate_id,
                item_batch_id: hold.item_batch_id,
                status,
                quantity_delta,
            },
        )
        .await?;
    }
    let transition_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_status_transitions (
            tenant_id, inventory_owner_id, facility_id, transaction_id,
            source_balance_id, destination_balance_id, from_status, to_status,
            qty, reason_code, reason_note, reference_type, reference_id,
            created_by, created
        ) VALUES ($1,$2,$3,$4,$5,$6,'quarantine',$7,$8,$9,$10,
                  'inbound_inspection_hold',$11,$12,$13)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(hold.inventory_owner_id)
    .bind(hold.facility_id)
    .bind(transaction_id)
    .bind(hold.inventory_balance_id)
    .bind(target_balance_id)
    .bind(target_status.as_str())
    .bind(hold.quantity)
    .bind(reason)
    .bind(command.note.as_str())
    .bind(command.inventory_hold_id.get())
    .bind(context.actor_id.get())
    .bind(inspected_at)
    .fetch_one(&mut *tx)
    .await?;
    let disposition_id = insert_evidence_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        inspected_at,
        command.inventory_hold_id,
        &hold,
        target_balance_id,
        command.outcome,
        target_status,
        command.note.as_str(),
        transaction_id,
        transition_id,
    )
    .await?;
    let metadata = serde_json::to_string(&serde_json::json!({
        "disposition_id": disposition_id,
        "inventory_hold_id": command.inventory_hold_id.get(),
        "inventory_transaction_id": transaction_id,
        "outcome": command.outcome.as_str(),
        "target_status": target_status.as_str(),
        "quantity": hold.quantity,
        "note": command.note.as_str(),
    }))
    .map_err(|error| AppError::internal(format!("encoding inspection activity: {error}")))?;
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id, created, load_id, user_id, action, message, metadata_json)
        VALUES ($1,$2,$3,$4,'inbound_inspection_disposed',
                'quarantined receipt inspection disposition recorded',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inspected_at)
    .bind(load_id)
    .bind(context.actor_id.get())
    .bind(metadata)
    .execute(&mut *tx)
    .await?;
    enqueue_events_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        inspected_at,
        load_id,
        disposition_id,
        command.inventory_hold_id,
        &hold,
        target_balance_id,
        command.outcome,
        target_status,
        command.note.as_str(),
        transaction_id,
    )
    .await?;
    let result = DisposeInboundInspectionResult {
        disposition_id: id(disposition_id, InboundInspectionDispositionId::new)?,
        inventory_hold_id: command.inventory_hold_id,
        inventory_owner_id: id(hold.inventory_owner_id, InventoryOwnerId::new)?,
        facility_id: id(hold.facility_id, FacilityId::new)?,
        source_inventory_balance_id: id(hold.inventory_balance_id, InventoryBalanceId::new)?,
        target_inventory_balance_id: id(target_balance_id, InventoryBalanceId::new)?,
        location_id: id(hold.location_id, LocationId::new)?,
        license_plate_id: hold.license_plate_id,
        item_batch_id: id(hold.item_batch_id, ItemBatchId::new)?,
        item_id: hold.item_id,
        uom: hold.uom,
        quantity: hold.quantity,
        outcome: command.outcome,
        target_status,
        note: command.note.clone(),
        inventory_transaction_id: transaction_id,
        inspected_by: id(context.actor_id.get(), UserId::new)?,
        inspected_at,
    };
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await?)
}
