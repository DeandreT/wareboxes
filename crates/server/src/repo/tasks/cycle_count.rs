use sqlx::Row;
use wareboxes_core::models::{
    InventoryStatus, InventoryTransactionType, ItemLocationCycleCountConfirmation, TenantAccess,
};
use wareboxes_domain::{CommandContext, FacilityId, InventoryOwnerId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::lock_current_scope_tx;
use crate::repo::idempotency::{require_command_context, PreparedCommand};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry, JournalStart};
use crate::repo::outbox::{self, NewOutboxEvent};

use super::{insert_progress_tx, require_replayed_task_visible_tx, TaskDimensions};

const OPERATION: &str = "task.confirm_item_location_cycle_count.v1";

struct CountTarget {
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    item_id: i64,
    inventory_balance_id: i64,
}

struct LockedBalance {
    item_batch_id: i64,
    license_plate_id: Option<i64>,
    uom: String,
    lot: Option<String>,
    expiration: Option<wareboxes_core::models::Timestamp>,
    serial: Option<String>,
    status: InventoryStatus,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
}

fn validated_note(note: Option<&str>) -> AppResult<Option<&str>> {
    let Some(note) = note else {
        return Ok(None);
    };
    if note.trim() != note || note.is_empty() {
        return Err(AppError::bad_request(
            "cycle count note must be trimmed and nonempty",
        ));
    }
    if note.chars().count() > 1000 {
        return Err(AppError::bad_request(
            "cycle count note cannot exceed 1000 characters",
        ));
    }
    Ok(Some(note))
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

async fn lock_task_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    task_id: i64,
    actor_user_id: i64,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<CountTarget> {
    let row = sqlx::query(
        r#"
        SELECT task.status,
               task.assigned_user_id,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               detail.inventory_owner_id,
               detail.facility_id,
               detail.location_id,
               detail.item_id,
               detail.inventory_balance_id
        FROM work_tasks task
        INNER JOIN cycle_count_item_location_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.deleted IS NULL
          AND task.task_type = 'cycle_count_item_location'
        FOR UPDATE OF task, detail
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("cycle count task"))?;

    let target = CountTarget {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        item_id: row.try_get("item_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
    };
    let dimensions = TaskDimensions {
        facility_id: Some(target.facility_id),
        inventory_owner_id: Some(target.inventory_owner_id),
    };
    if !dimensions.is_allowed_by(scope) {
        return Err(AppError::not_found("cycle count task"));
    }

    let status: String = row.try_get("status")?;
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    let lease_is_current: Option<bool> = row.try_get("lease_is_current")?;
    if status != "in_progress"
        || assigned_user_id != Some(actor_user_id)
        || lease_is_current != Some(true)
    {
        return Err(AppError::conflict(
            "cycle count task does not have an active claim for this operator",
        ));
    }
    Ok(target)
}

async fn lock_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &CountTarget,
) -> AppResult<LockedBalance> {
    let row = sqlx::query(
        r#"
        SELECT balance.inventory_owner_id,
               balance.facility_id,
               balance.location_id,
               balance.item_id,
               balance.item_batch_id,
               balance.license_plate_id,
               balance.uom,
               batch.lot,
               batch.expiration,
               batch.serial,
               balance.status,
               balance.qty_on_hand,
               balance.qty_reserved,
               balance.qty_held
        FROM inventory_balances balance
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
        WHERE balance.tenant_id = $1
          AND balance.id = $2
          AND balance.deleted IS NULL
          AND batch.deleted IS NULL
        FOR UPDATE OF balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_balance_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("cycle count inventory balance is no longer active"))?;

    if row.try_get::<i64, _>("inventory_owner_id")? != target.inventory_owner_id
        || row.try_get::<i64, _>("facility_id")? != target.facility_id
        || row.try_get::<i64, _>("location_id")? != target.location_id
        || row.try_get::<i64, _>("item_id")? != target.item_id
    {
        return Err(AppError::conflict(
            "cycle count inventory balance no longer matches the task target",
        ));
    }

    Ok(LockedBalance {
        item_batch_id: row.try_get("item_batch_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get("serial")?,
        status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
        qty_held: row.try_get("qty_held")?,
    })
}

pub async fn confirm_item_location_cycle_count_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    counted_quantity: i64,
    note: Option<&str>,
) -> AppResult<ItemLocationCycleCountConfirmation> {
    require_command_context(access, command)?;
    if task_id <= 0 {
        return Err(AppError::bad_request("task ID must be positive"));
    }
    if counted_quantity < 0 {
        return Err(AppError::bad_request("counted quantity cannot be negative"));
    }
    let note = validated_note(note)?;
    let prepared = PreparedCommand::new(command, OPERATION, &(task_id, counted_quantity, note))?;

    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;

    if let Some(result) = prepared
        .replayed::<ItemLocationCycleCountConfirmation>(&mut tx)
        .await?
    {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let target = lock_task_target(
        &mut tx,
        access.tenant_id,
        task_id,
        command.actor_id.get(),
        &scope,
    )
    .await?;
    let balance = lock_balance(&mut tx, access.tenant_id, &target).await?;
    let committed_quantity = balance
        .qty_reserved
        .checked_add(balance.qty_held)
        .ok_or_else(|| AppError::internal("inventory commitments are out of range"))?;
    if counted_quantity < committed_quantity {
        return Err(AppError::conflict(
            "counted quantity cannot be lower than reserved and held quantity",
        ));
    }
    let variance_quantity = counted_quantity
        .checked_sub(balance.qty_on_hand)
        .ok_or_else(|| AppError::bad_request("cycle count variance is out of range"))?;
    let confirmed_at = now_iso();

    let inventory_transaction_id = if variance_quantity == 0 {
        None
    } else {
        let owner_facility =
            inventory_journal::owner_facility_scope(target.inventory_owner_id, target.facility_id)?;
        let transaction_id = match inventory_journal::begin_transaction(
            &mut tx,
            &JournalCommand {
                tenant_id: access.tenant_id,
                owner_facility,
                actor_user_id: command.actor_id.get(),
                transaction_type: InventoryTransactionType::Adjust,
                reason: Some("cycle count confirmation"),
                reference_type: Some("cycle_count_item_location_task"),
                reference_id: Some(task_id),
                correlation_id: Some(&command.request_id),
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
                    "cycle count journal replay bypassed command replay",
                ));
            }
        };

        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: target.location_id,
                license_plate_id: balance.license_plate_id,
                item_batch_id: balance.item_batch_id,
                status: balance.status,
                quantity_delta: variance_quantity,
            },
        )
        .await?;

        let updated = sqlx::query(
            r#"
            UPDATE inventory_balances
            SET qty_on_hand = $1,
                modified = $2
            WHERE tenant_id = $3
              AND inventory_owner_id = $4
              AND id = $5
              AND deleted IS NULL
              AND qty_on_hand = $6
              AND qty_reserved = $7
              AND qty_held = $8
            "#,
        )
        .bind(counted_quantity)
        .bind(confirmed_at)
        .bind(access.tenant_id.get())
        .bind(target.inventory_owner_id)
        .bind(target.inventory_balance_id)
        .bind(balance.qty_on_hand)
        .bind(balance.qty_reserved)
        .bind(balance.qty_held)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "cycle count inventory balance changed during confirmation",
            ));
        }
        Some(transaction_id)
    };

    let confirmation = ItemLocationCycleCountConfirmation {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id: InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: target.facility_id,
        location_id: target.location_id,
        inventory_balance_id: target.inventory_balance_id,
        license_plate_id: balance.license_plate_id,
        item_batch_id: balance.item_batch_id,
        item_id: target.item_id,
        uom: balance.uom,
        lot: balance.lot,
        expiration: balance.expiration,
        serial: balance.serial,
        inventory_status: balance.status,
        previous_on_hand_quantity: balance.qty_on_hand,
        reserved_quantity: balance.qty_reserved,
        held_quantity: balance.qty_held,
        counted_quantity,
        variance_quantity,
        inventory_transaction_id,
        confirmed_by: command.actor_id.get(),
        confirmed_at,
        note: note.map(str::to_owned),
    };

    sqlx::query(
        r#"
        INSERT INTO cycle_count_item_location_results (
            tenant_id, task_id, inventory_owner_id, facility_id, location_id,
            item_id, inventory_balance_id, item_batch_id, license_plate_id, uom,
            lot, expiration, serial, status, system_qty_on_hand,
            system_qty_reserved, system_qty_held, counted_qty, variance_qty,
            inventory_transaction_id, confirmed_by, confirmed_at, note
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
            $15, $16, $17, $18, $19, $20, $21, $22, $23
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(target.location_id)
    .bind(target.item_id)
    .bind(target.inventory_balance_id)
    .bind(balance.item_batch_id)
    .bind(balance.license_plate_id)
    .bind(&confirmation.uom)
    .bind(&confirmation.lot)
    .bind(confirmation.expiration)
    .bind(&confirmation.serial)
    .bind(confirmation.inventory_status.as_str())
    .bind(confirmation.previous_on_hand_quantity)
    .bind(confirmation.reserved_quantity)
    .bind(confirmation.held_quantity)
    .bind(confirmation.counted_quantity)
    .bind(confirmation.variance_quantity)
    .bind(confirmation.inventory_transaction_id)
    .bind(confirmation.confirmed_by)
    .bind(confirmation.confirmed_at)
    .bind(&confirmation.note)
    .execute(&mut *tx)
    .await?;

    let completed = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'completed',
            completed_by = $1,
            completed_at = $2,
            lease_expires_at = NULL,
            modified = $2
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND status = 'in_progress'
          AND assigned_user_id = $1
          AND lease_expires_at > statement_timestamp()
        "#,
    )
    .bind(command.actor_id.get())
    .bind(confirmed_at)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if completed.rows_affected() != 1 {
        return Err(AppError::conflict(
            "cycle count task claim expired during confirmation",
        ));
    }

    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "cycle_count_confirmed",
        None,
        None,
        None,
        note,
        None,
    )
    .await?;

    let inventory_owner_id = confirmation.inventory_owner_id;
    let facility_id = FacilityId::new(target.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("cycle-count-confirmation:{task_id}");
    let aggregate_id = task_id.to_string();
    let payload = serde_json::json!({
        "task_id": task_id,
        "inventory_owner_id": target.inventory_owner_id,
        "facility_id": target.facility_id,
        "location_id": target.location_id,
        "inventory_balance_id": target.inventory_balance_id,
        "item_batch_id": balance.item_batch_id,
        "item_id": target.item_id,
        "license_plate_id": balance.license_plate_id,
        "status": balance.status.as_str(),
        "previous_on_hand_quantity": balance.qty_on_hand,
        "reserved_quantity": balance.qty_reserved,
        "held_quantity": balance.qty_held,
        "counted_quantity": counted_quantity,
        "variance_quantity": variance_quantity,
        "inventory_transaction_id": inventory_transaction_id,
    });
    outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(command.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "cycle_count_confirmation",
            aggregate_id: &aggregate_id,
            ordering_key: &event_key,
            aggregate_sequence: 1,
            event_type: "inventory.cycle_count.confirmed",
            schema_version: 1,
            payload: &payload,
            occurred_at: confirmed_at,
        },
    )
    .await?;

    prepared
        .commit_with_inventory_transaction(tx, confirmation, inventory_transaction_id)
        .await
}
