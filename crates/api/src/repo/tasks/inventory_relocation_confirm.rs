use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InventoryRelocationConfirmation, InventoryRelocationConfirmationResult,
    InventoryRelocationWorkflow, InventoryTransactionType, TenantAccess, Timestamp,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::inventory;
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::inventory_relocation::{
    lock_plate_contents, lock_relocation_destination, movable_quantity, parse_inventory_status,
    require_movable_plate_contents, require_plate_destination_compatible, validate_barcode,
    PlateContent, RelocationTarget,
};
use super::license_plate_tree::lock_root_tree_tx;
use super::{insert_progress_tx, require_replayed_task_visible_tx, TaskDimensions};

const CONFIRM_OPERATION: &str = "task.confirm_inventory_relocation.v1";

#[derive(Debug)]
struct LockedLooseBalance {
    id: i64,
    location_id: i64,
    uom: String,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
    active: bool,
}

pub async fn confirm_inventory_relocation_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    scanned_destination_location_barcode: &str,
    scanned_license_plate_barcode: Option<&str>,
) -> AppResult<InventoryRelocationConfirmation> {
    command.require_actor(access.tenant_id, access.user_id)?;
    if task_id <= 0 {
        return Err(AppError::bad_request(
            "inventory relocation task ID must be positive",
        ));
    }
    validate_barcode(
        scanned_destination_location_barcode,
        "destination location barcode",
    )?;
    if let Some(barcode) = scanned_license_plate_barcode {
        validate_barcode(barcode, "license plate barcode")?;
    }
    let prepared = PreparedCommand::new_v1(
        command,
        CONFIRM_OPERATION,
        &(
            task_id,
            scanned_destination_location_barcode,
            scanned_license_plate_barcode,
        ),
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    if let Some(result) = prepared
        .replayed::<InventoryRelocationConfirmation>(&mut tx)
        .await?
    {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let target =
        lock_relocation_target(&mut tx, access, task_id, command.actor_id.get(), &scope).await?;
    let destination_barcode = lock_relocation_destination(
        &mut tx,
        access.tenant_id,
        target.facility_id,
        target.destination_location_id,
    )
    .await?;
    if destination_barcode != scanned_destination_location_barcode {
        return Err(AppError::conflict(
            "scanned destination does not match the directed relocation location",
        ));
    }
    let result = match target.workflow {
        InventoryRelocationWorkflow::LooseBalance => {
            if scanned_license_plate_barcode.is_some() {
                return Err(AppError::bad_request(
                    "loose inventory relocation does not accept a license plate scan",
                ));
            }
            confirm_loose(
                &mut tx,
                access,
                command,
                &prepared,
                &target,
                destination_barcode,
            )
            .await?
        }
        InventoryRelocationWorkflow::LicensePlate => {
            let scanned_plate = scanned_license_plate_barcode.ok_or_else(|| {
                AppError::bad_request("license plate barcode is required for container relocation")
            })?;
            confirm_plate(
                &mut tx,
                access,
                command,
                &prepared,
                &target,
                destination_barcode,
                scanned_plate,
            )
            .await?
        }
    };
    complete_relocation_task(&mut tx, access, command, &result).await?;
    enqueue_confirmation(&mut tx, command, &result).await?;
    let transaction_id = result.inventory_transaction_id;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await?)
}

async fn lock_relocation_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<RelocationTarget> {
    let row = sqlx::query(
        r#"
        SELECT task.status, task.assigned_user_id,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               detail.workflow, detail.inventory_owner_id, detail.facility_id,
               detail.source_inventory_balance_id, detail.license_plate_id,
               detail.source_location_id, detail.destination_location_id,
               detail.item_batch_id, detail.item_id, detail.uom,
               detail.inventory_status, detail.planned_quantity,
               detail.planned_balance_count, detail.closed_at
        FROM work_tasks task
        INNER JOIN inventory_relocation_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.deleted IS NULL
          AND task.task_type = 'inventory_relocation'
        FOR UPDATE OF task
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory relocation task"))?;
    let target = RelocationTarget {
        task_id,
        workflow: InventoryRelocationWorkflow::parse(&row.try_get::<String, _>("workflow")?)
            .ok_or_else(|| AppError::internal("inventory relocation has an invalid workflow"))?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        source_inventory_balance_id: row.try_get("source_inventory_balance_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        source_location_id: row.try_get("source_location_id")?,
        destination_location_id: row.try_get("destination_location_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        status: row
            .try_get::<Option<String>, _>("inventory_status")?
            .map(|status| parse_inventory_status(&status))
            .transpose()?,
        quantity: row.try_get("planned_quantity")?,
        planned_balance_count: row.try_get("planned_balance_count")?,
    };
    if !(TaskDimensions {
        facility_id: Some(target.facility_id),
        inventory_owner_id: Some(target.inventory_owner_id),
    })
    .is_allowed_by(scope)
    {
        return Err(AppError::not_found("inventory relocation task"));
    }
    if row.try_get::<String, _>("status")? != "in_progress"
        || row.try_get::<Option<i64>, _>("assigned_user_id")? != Some(actor_user_id)
        || row.try_get::<Option<bool>, _>("lease_is_current")? != Some(true)
        || row.try_get::<Option<Timestamp>, _>("closed_at")?.is_some()
    {
        return Err(AppError::conflict(
            "inventory relocation task does not have an active claim for this operator",
        ));
    }
    Ok(target)
}

async fn confirm_loose(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CommandContext,
    prepared: &PreparedCommand,
    target: &RelocationTarget,
    destination_barcode: String,
) -> AppResult<InventoryRelocationConfirmation> {
    let source_balance_id = required(target.source_inventory_balance_id, "source balance")?;
    let item_batch_id = required(target.item_batch_id, "item batch")?;
    let item_id = required(target.item_id, "item")?;
    let status = target
        .status
        .ok_or_else(|| AppError::internal("loose relocation has no inventory status"))?;
    let quantity = required(target.quantity, "planned quantity")?;
    let planned_uom = target
        .uom
        .as_deref()
        .ok_or_else(|| AppError::internal("loose relocation has no UOM"))?;
    let balances = lock_loose_balances(tx, access, target).await?;
    let source = balances
        .iter()
        .find(|balance| balance.id == source_balance_id && balance.active)
        .ok_or_else(|| AppError::conflict("relocation source inventory is no longer active"))?;
    if source.location_id != target.source_location_id || source.uom != planned_uom {
        return Err(AppError::conflict(
            "relocation source inventory no longer matches the task",
        ));
    }
    if movable_quantity(source.qty_on_hand, source.qty_reserved, source.qty_held)? < quantity {
        return Err(AppError::conflict(
            "insufficient uncommitted inventory for relocation",
        ));
    }
    inventory::ensure_location_accepts_batch_tx(
        tx,
        access.tenant_id,
        target.inventory_owner_id,
        target.destination_location_id,
        item_batch_id,
    )
    .await?;
    let transaction_id = begin_relocation_transaction(
        tx,
        access,
        command,
        prepared,
        target.task_id,
        target.inventory_owner_id,
        target.facility_id,
    )
    .await?;
    let confirmed_at = now_iso();
    let source_update = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1, modified = $2
        WHERE tenant_id = $3
          AND inventory_owner_id = $4
          AND facility_id = $5
          AND id = $6
          AND location_id = $7
          AND item_batch_id = $8
          AND item_id = $9
          AND uom = $10
          AND status = $11
          AND license_plate_id IS NULL
          AND deleted IS NULL
          AND qty_on_hand - qty_reserved - qty_held >= $1
        "#,
    )
    .bind(quantity)
    .bind(confirmed_at)
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(source_balance_id)
    .bind(target.source_location_id)
    .bind(item_batch_id)
    .bind(item_id)
    .bind(planned_uom)
    .bind(status.as_str())
    .execute(&mut **tx)
    .await?;
    if source_update.rows_affected() != 1 {
        return Err(AppError::conflict(
            "relocation source inventory changed during confirmation",
        ));
    }
    let destination_inventory_balance_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_balances (
            tenant_id, inventory_owner_id, created, modified, facility_id,
            location_id, license_plate_id, item_batch_id, item_id, uom,
            status, qty_on_hand, qty_reserved
        )
        VALUES ($1, $2, $3, $3, $4, $5, NULL, $6, $7, $8, $9, $10, 0)
        ON CONFLICT (
            tenant_id, inventory_owner_id, location_id, item_batch_id, uom, status
        ) WHERE license_plate_id IS NULL
        DO UPDATE SET
            qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
            modified = excluded.modified,
            deleted = NULL
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(confirmed_at)
    .bind(target.facility_id)
    .bind(target.destination_location_id)
    .bind(item_batch_id)
    .bind(item_id)
    .bind(planned_uom)
    .bind(status.as_str())
    .bind(quantity)
    .fetch_one(&mut **tx)
    .await?;
    append_move_entries(
        tx,
        access,
        target,
        transaction_id,
        item_batch_id,
        status,
        None,
        quantity,
    )
    .await?;
    let result = InventoryRelocationConfirmation {
        tenant_id: access.tenant_id,
        task_id: target.task_id,
        inventory_owner_id: InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: target.facility_id,
        source_location_id: target.source_location_id,
        destination_location_id: target.destination_location_id,
        destination_location_barcode: destination_barcode,
        inventory_transaction_id: transaction_id,
        confirmed_by: command.actor_id.get(),
        confirmed_at,
        result: InventoryRelocationConfirmationResult::LooseBalance {
            source_inventory_balance_id: source_balance_id,
            destination_inventory_balance_id,
            item_batch_id,
            item_id,
            inventory_status: status,
            uom: planned_uom.to_owned(),
            quantity,
        },
    };
    insert_result(tx, &result).await?;
    Ok(result)
}

async fn confirm_plate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CommandContext,
    prepared: &PreparedCommand,
    target: &RelocationTarget,
    destination_barcode: String,
    scanned_plate_barcode: &str,
) -> AppResult<InventoryRelocationConfirmation> {
    let plate_id = required(target.license_plate_id, "license plate")?;
    let plate = lock_root_tree_tx(tx, access.tenant_id, plate_id).await?;
    if plate.inventory_owner_id != target.inventory_owner_id
        || plate.facility_id != target.facility_id
        || plate.location_id != target.source_location_id
    {
        return Err(AppError::conflict(
            "license plate no longer matches the relocation task",
        ));
    }
    if plate.barcode != scanned_plate_barcode {
        return Err(AppError::conflict(
            "scanned license plate does not match the relocation task",
        ));
    }
    let contents = lock_plate_contents(
        tx,
        access.tenant_id,
        target.inventory_owner_id,
        target.facility_id,
        &plate.plate_ids,
    )
    .await?;
    let positive = require_movable_plate_contents(&contents, target.source_location_id)?;
    let planned = load_planned_contents(tx, access.tenant_id, target.task_id).await?;
    require_exact_snapshot(target, &positive, &planned)?;
    require_plate_destination_compatible(
        tx,
        access.tenant_id,
        target.inventory_owner_id,
        target.destination_location_id,
        &positive,
    )
    .await?;
    let transaction_id = begin_relocation_transaction(
        tx,
        access,
        command,
        prepared,
        target.task_id,
        target.inventory_owner_id,
        target.facility_id,
    )
    .await?;
    let moved_at = now_iso();
    let balance_ids = contents
        .iter()
        .map(|content| content.inventory_balance_id)
        .collect::<Vec<_>>();
    let updated_balances = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET location_id = $1, modified = $2
        WHERE tenant_id = $3
          AND inventory_owner_id = $4
          AND facility_id = $5
          AND license_plate_id = ANY($6)
          AND id = ANY($7)
          AND deleted IS NULL
        "#,
    )
    .bind(target.destination_location_id)
    .bind(moved_at)
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(&plate.plate_ids)
    .bind(&balance_ids)
    .execute(&mut **tx)
    .await?;
    if usize::try_from(updated_balances.rows_affected()).ok() != Some(balance_ids.len()) {
        return Err(AppError::conflict(
            "license plate inventory changed during relocation confirmation",
        ));
    }
    let updated_plate = sqlx::query(
        r#"
        UPDATE license_plates
        SET location_id = $1
        WHERE tenant_id = $2
          AND inventory_owner_id = $3
          AND facility_id = $4
          AND id = ANY($5)
          AND location_id = $6
          AND deleted IS NULL
        "#,
    )
    .bind(target.destination_location_id)
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(&plate.plate_ids)
    .bind(target.source_location_id)
    .execute(&mut **tx)
    .await?;
    if usize::try_from(updated_plate.rows_affected()).ok() != Some(plate.plate_ids.len()) {
        return Err(AppError::conflict(
            "license plate location changed during relocation confirmation",
        ));
    }
    for content in &positive {
        append_move_entries(
            tx,
            access,
            target,
            transaction_id,
            content.item_batch_id,
            content.status,
            Some(content.license_plate_id),
            content.quantity,
        )
        .await?;
    }
    let result = InventoryRelocationConfirmation {
        tenant_id: access.tenant_id,
        task_id: target.task_id,
        inventory_owner_id: InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: target.facility_id,
        source_location_id: target.source_location_id,
        destination_location_id: target.destination_location_id,
        destination_location_barcode: destination_barcode,
        inventory_transaction_id: transaction_id,
        confirmed_by: command.actor_id.get(),
        confirmed_at: moved_at,
        result: InventoryRelocationConfirmationResult::LicensePlate {
            license_plate_id: plate_id,
            license_plate_barcode: plate.barcode,
            moved_balance_count: required(
                target.planned_balance_count,
                "planned license plate balance count",
            )?,
        },
    };
    insert_result(tx, &result).await?;
    Ok(result)
}

async fn lock_loose_balances(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    target: &RelocationTarget,
) -> AppResult<Vec<LockedLooseBalance>> {
    let rows = sqlx::query(
        r#"
        SELECT id, location_id, uom, qty_on_hand, qty_reserved, qty_held,
               deleted IS NULL AS active
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND facility_id = $3
          AND license_plate_id IS NULL
          AND (
              id = $4
              OR (
                  location_id = $5
                  AND item_batch_id = $6
                  AND item_id = $7
                  AND uom = $8
                  AND status = $9
              )
          )
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(target.source_inventory_balance_id)
    .bind(target.destination_location_id)
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(&target.uom)
    .bind(target.status.map(|status| status.as_str()))
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(LockedLooseBalance {
                id: row.try_get("id")?,
                location_id: row.try_get("location_id")?,
                uom: row.try_get("uom")?,
                qty_on_hand: row.try_get("qty_on_hand")?,
                qty_reserved: row.try_get("qty_reserved")?,
                qty_held: row.try_get("qty_held")?,
                active: row.try_get("active")?,
            })
        })
        .collect()
}

async fn load_planned_contents(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    task_id: i64,
) -> AppResult<Vec<PlateContent>> {
    let rows = sqlx::query(
        r#"
        SELECT inventory_balance_id, content_license_plate_id, item_batch_id, item_id, uom,
               inventory_status, planned_quantity
        FROM inventory_relocation_task_contents
        WHERE tenant_id = $1 AND task_id = $2
        ORDER BY inventory_balance_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(PlateContent {
                inventory_balance_id: row.try_get("inventory_balance_id")?,
                license_plate_id: row.try_get("content_license_plate_id")?,
                location_id: 0,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                status: parse_inventory_status(&row.try_get::<String, _>("inventory_status")?)?,
                quantity: row.try_get("planned_quantity")?,
                qty_reserved: 0,
                qty_held: 0,
            })
        })
        .collect()
}

fn require_exact_snapshot(
    target: &RelocationTarget,
    current: &[PlateContent],
    planned: &[PlateContent],
) -> AppResult<()> {
    if i64::try_from(planned.len()).ok() != target.planned_balance_count
        || current.len() != planned.len()
        || current.iter().zip(planned).any(|(current, planned)| {
            current.inventory_balance_id != planned.inventory_balance_id
                || current.license_plate_id != planned.license_plate_id
                || current.item_batch_id != planned.item_batch_id
                || current.item_id != planned.item_id
                || current.uom != planned.uom
                || current.status != planned.status
                || current.quantity != planned.quantity
        })
    {
        return Err(AppError::conflict(
            "license plate contents changed after relocation planning",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn begin_relocation_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CommandContext,
    prepared: &PreparedCommand,
    task_id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<i64> {
    let owner_facility = inventory_journal::owner_facility_scope(inventory_owner_id, facility_id)?;
    inventory_journal::begin_transaction(
        tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: command.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("scanner-confirmed inventory relocation"),
            reference_type: Some("inventory_relocation_task"),
            reference_id: Some(task_id),
            correlation_id: Some(&command.request_id),
            operation: CONFIRM_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn append_move_entries(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    target: &RelocationTarget,
    transaction_id: i64,
    item_batch_id: i64,
    status: wareboxes_core::models::InventoryStatus,
    license_plate_id: Option<i64>,
    quantity: i64,
) -> AppResult<()> {
    let owner_facility =
        inventory_journal::owner_facility_scope(target.inventory_owner_id, target.facility_id)?;
    for (location_id, quantity_delta) in [
        (target.source_location_id, -quantity),
        (target.destination_location_id, quantity),
    ] {
        inventory_journal::append_entry(
            tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id,
                license_plate_id,
                item_batch_id,
                status,
                quantity_delta,
            },
        )
        .await?;
    }
    Ok(())
}

async fn insert_result(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    result: &InventoryRelocationConfirmation,
) -> AppResult<()> {
    let (
        workflow,
        source_balance_id,
        destination_balance_id,
        plate_id,
        plate_barcode,
        item_batch_id,
        item_id,
        uom,
        status,
        quantity,
        moved_balance_count,
    ) = match &result.result {
        InventoryRelocationConfirmationResult::LooseBalance {
            source_inventory_balance_id,
            destination_inventory_balance_id,
            item_batch_id,
            item_id,
            inventory_status,
            uom,
            quantity,
        } => (
            "loose_balance",
            Some(*source_inventory_balance_id),
            Some(*destination_inventory_balance_id),
            None,
            None,
            Some(*item_batch_id),
            Some(*item_id),
            Some(uom.as_str()),
            Some(inventory_status.as_str()),
            Some(*quantity),
            None,
        ),
        InventoryRelocationConfirmationResult::LicensePlate {
            license_plate_id,
            license_plate_barcode,
            moved_balance_count,
        } => (
            "license_plate",
            None,
            None,
            Some(*license_plate_id),
            Some(license_plate_barcode.as_str()),
            None,
            None,
            None,
            None,
            None,
            Some(*moved_balance_count),
        ),
    };
    sqlx::query(
        r#"
        INSERT INTO inventory_relocation_results (
            tenant_id, task_id, inventory_owner_id, facility_id, workflow,
            source_location_id, destination_location_id,
            destination_location_barcode, inventory_transaction_id,
            source_inventory_balance_id, destination_inventory_balance_id,
            license_plate_id, license_plate_barcode, item_batch_id, item_id,
            uom, inventory_status, quantity, moved_balance_count,
            confirmed_by, confirmed_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
            $14, $15, $16, $17, $18, $19, $20, $21
        )
        "#,
    )
    .bind(result.tenant_id.get())
    .bind(result.task_id)
    .bind(result.inventory_owner_id.get())
    .bind(result.facility_id)
    .bind(workflow)
    .bind(result.source_location_id)
    .bind(result.destination_location_id)
    .bind(&result.destination_location_barcode)
    .bind(result.inventory_transaction_id)
    .bind(source_balance_id)
    .bind(destination_balance_id)
    .bind(plate_id)
    .bind(plate_barcode)
    .bind(item_batch_id)
    .bind(item_id)
    .bind(uom)
    .bind(status)
    .bind(quantity)
    .bind(moved_balance_count)
    .bind(result.confirmed_by)
    .bind(result.confirmed_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn complete_relocation_task(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CommandContext,
    result: &InventoryRelocationConfirmation,
) -> AppResult<()> {
    let completed = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'completed', completed_by = $1, completed_at = $2,
            lease_expires_at = NULL, modified = $2
        WHERE tenant_id = $3
          AND id = $4
          AND task_type = 'inventory_relocation'
          AND deleted IS NULL
          AND status = 'in_progress'
          AND assigned_user_id = $1
          AND lease_expires_at > statement_timestamp()
        "#,
    )
    .bind(command.actor_id.get())
    .bind(result.confirmed_at)
    .bind(access.tenant_id.get())
    .bind(result.task_id)
    .execute(&mut **tx)
    .await?;
    if completed.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inventory relocation claim expired during confirmation",
        ));
    }
    let quantity = match &result.result {
        InventoryRelocationConfirmationResult::LooseBalance { quantity, .. } => Some(*quantity),
        InventoryRelocationConfirmationResult::LicensePlate { .. } => None,
    };
    insert_progress_tx(
        tx,
        access.tenant_id,
        result.task_id,
        None,
        Some(command.actor_id.get()),
        "inventory_relocation_confirmed",
        quantity,
        Some(result.source_location_id),
        Some(result.destination_location_id),
        None,
        None,
    )
    .await?;
    Ok(())
}

async fn enqueue_confirmation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &CommandContext,
    result: &InventoryRelocationConfirmation,
) -> AppResult<()> {
    let facility_id = FacilityId::new(result.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("inventory-relocation-confirmation:{}", result.task_id);
    let aggregate_id = result.task_id.to_string();
    let payload = serde_json::to_value(result).map_err(|error| {
        AppError::internal(format!("could not encode relocation event: {error}"))
    })?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: result.tenant_id,
            inventory_owner_id: Some(result.inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(command.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inventory_relocation_confirmation",
            aggregate_id: &aggregate_id,
            ordering_key: &event_key,
            aggregate_sequence: 1,
            event_type: "inventory.relocation.confirmed",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.confirmed_at,
        },
    )
    .await?;
    Ok(())
}

fn required(value: Option<i64>, label: &str) -> AppResult<i64> {
    value.ok_or_else(|| AppError::internal(format!("inventory relocation has no {label}")))
}
