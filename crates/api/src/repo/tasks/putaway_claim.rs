use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InventoryStatus, PutawayClaim, PutawayClaimDestinationLocation, PutawayClaimSourceLocation,
    PutawayClaimWork, TenantAccess, Timestamp, WorkTaskType,
};
use wareboxes_domain::InventoryOwnerId;

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::idempotency::{require_command_context, PreparedCommand};

use super::leasing::{release_expired_tasks_tx, release_inaccessible_active_tasks_tx};
use super::{insert_progress_tx, TaskDimensions};

const CLAIM_NEXT_OPERATION: &str = "putaway.claim_next.v1";
const CLAIM_BY_ID_OPERATION: &str = "putaway.claim_by_id.v1";

fn require_putaway_type(task_type: WorkTaskType) -> AppResult<WorkTaskType> {
    if matches!(
        task_type,
        WorkTaskType::Putaway | WorkTaskType::LicensePlatePutaway
    ) {
        Ok(task_type)
    } else {
        Err(AppError::bad_request(
            "putaway claims require a putaway workflow type",
        ))
    }
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

fn required_barcode(value: Option<String>, label: &str) -> AppResult<String> {
    value
        .filter(|barcode| !barcode.trim().is_empty())
        .ok_or_else(|| AppError::conflict(format!("{label} must have a scannable barcode")))
}

fn optional_barcode(value: Option<String>) -> Option<String> {
    value.filter(|barcode| !barcode.trim().is_empty())
}

fn task_dimensions(row: &sqlx::postgres::PgRow) -> AppResult<TaskDimensions> {
    Ok(TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    })
}

async fn require_claim_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    expected_type: Option<WorkTaskType>,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id, task_type
        FROM work_tasks
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("putaway task"))?;
    let task_type = WorkTaskType::parse(&row.try_get::<String, _>("task_type")?)
        .ok_or_else(|| AppError::internal("work task has an invalid type"))?;
    require_putaway_type(task_type)?;
    if expected_type.is_some_and(|expected_type| expected_type != task_type)
        || !task_dimensions(&row)?.is_allowed_by(scope)
    {
        return Err(AppError::not_found("putaway task"));
    }
    Ok(())
}

async fn load_putaway_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_user_id: i64,
) -> AppResult<PutawayClaim> {
    let task_type: String = sqlx::query_scalar(
        r#"
        SELECT task_type
        FROM work_tasks
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
          AND status = 'in_progress'
          AND assigned_user_id = $3
          AND lease_expires_at > statement_timestamp()
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("putaway claim is no longer active"))?;
    let task_type = WorkTaskType::parse(&task_type)
        .ok_or_else(|| AppError::internal("work task has an invalid type"))?;

    match require_putaway_type(task_type)? {
        WorkTaskType::Putaway => load_loose_claim_tx(tx, access, task_id, actor_user_id).await,
        WorkTaskType::LicensePlatePutaway => {
            load_license_plate_claim_tx(tx, access, task_id, actor_user_id).await
        }
        _ => unreachable!("putaway type was validated"),
    }
}

async fn load_loose_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_user_id: i64,
) -> AppResult<PutawayClaim> {
    let row = sqlx::query(
        r#"
        SELECT task.inventory_owner_id,
               task.facility_id,
               task.priority,
               task.instructions,
               task.due_at,
               task.lease_expires_at,
               detail.source_inventory_balance_id,
               detail.source_location_id,
               detail.destination_location_id,
               detail.item_batch_id,
               detail.item_id,
               detail.inventory_status,
               detail.planned_quantity,
               source_location.barcode AS source_barcode,
               source_location.name AS source_name,
               source_location.active AS source_active,
               source_location.receivable AS source_receivable,
               destination_location.barcode AS destination_barcode,
               destination_location.name AS destination_name,
               destination_location.active AS destination_active,
               destination_location.receivable AS destination_receivable,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_license_plate_id,
               balance.item_batch_id AS balance_item_batch_id,
               balance.item_id AS balance_item_id,
               balance.uom,
               balance.status AS balance_status,
               balance.qty_on_hand,
               balance.qty_reserved,
               balance.qty_held,
               balance.deleted AS balance_deleted,
               batch.lot,
               batch.serial,
               batch.expiration,
               batch.deleted AS batch_deleted,
               item.description AS item_description,
               item.deleted AS item_deleted
        FROM work_tasks task
        INNER JOIN putaway_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
         AND detail.closed_at IS NULL
        INNER JOIN locations source_location
          ON source_location.tenant_id = detail.tenant_id
         AND source_location.facility_id = detail.facility_id
         AND source_location.id = detail.source_location_id
         AND source_location.deleted IS NULL
        INNER JOIN locations destination_location
          ON destination_location.tenant_id = detail.tenant_id
         AND destination_location.facility_id = detail.facility_id
         AND destination_location.id = detail.destination_location_id
         AND destination_location.deleted IS NULL
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = detail.tenant_id
         AND balance.inventory_owner_id = detail.inventory_owner_id
         AND balance.facility_id = detail.facility_id
         AND balance.id = detail.source_inventory_balance_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = detail.tenant_id
         AND batch.inventory_owner_id = detail.inventory_owner_id
         AND batch.id = detail.item_batch_id
        INNER JOIN items item
          ON item.tenant_id = detail.tenant_id
         AND item.id = detail.item_id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.task_type = 'putaway'
          AND task.status = 'in_progress'
          AND task.assigned_user_id = $3
          AND task.lease_expires_at > statement_timestamp()
          AND task.deleted IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("loose putaway claim is no longer executable"))?;

    let qty_on_hand: i64 = row.try_get("qty_on_hand")?;
    let qty_reserved: i64 = row.try_get("qty_reserved")?;
    let qty_held: i64 = row.try_get("qty_held")?;
    let planned_quantity: i64 = row.try_get("planned_quantity")?;
    let available_quantity = qty_on_hand
        .checked_sub(qty_reserved)
        .and_then(|quantity| quantity.checked_sub(qty_held));
    if !row.try_get::<bool, _>("source_active")?
        || !row.try_get::<bool, _>("source_receivable")?
        || !row.try_get::<bool, _>("destination_active")?
        || row.try_get::<bool, _>("destination_receivable")?
        || row
            .try_get::<Option<Timestamp>, _>("balance_deleted")?
            .is_some()
        || row
            .try_get::<Option<Timestamp>, _>("batch_deleted")?
            .is_some()
        || row
            .try_get::<Option<Timestamp>, _>("item_deleted")?
            .is_some()
        || row.try_get::<i64, _>("balance_location_id")?
            != row.try_get::<i64, _>("source_location_id")?
        || row
            .try_get::<Option<i64>, _>("balance_license_plate_id")?
            .is_some()
        || row.try_get::<i64, _>("balance_item_batch_id")?
            != row.try_get::<i64, _>("item_batch_id")?
        || row.try_get::<i64, _>("balance_item_id")? != row.try_get::<i64, _>("item_id")?
        || row.try_get::<String, _>("balance_status")?
            != row.try_get::<String, _>("inventory_status")?
        || match available_quantity {
            Some(quantity) => quantity < planned_quantity,
            None => true,
        }
    {
        return Err(AppError::conflict(
            "loose putaway claim is no longer executable",
        ));
    }

    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(PutawayClaim {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id,
        facility_id: row.try_get("facility_id")?,
        priority: row.try_get("priority")?,
        instructions: row.try_get("instructions")?,
        due_at: row.try_get("due_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        source_location: PutawayClaimSourceLocation {
            location_id: row.try_get("source_location_id")?,
            barcode: optional_barcode(row.try_get("source_barcode")?),
            name: row.try_get("source_name")?,
        },
        destination_location: PutawayClaimDestinationLocation {
            location_id: row.try_get("destination_location_id")?,
            barcode: required_barcode(row.try_get("destination_barcode")?, "putaway destination")?,
            name: row.try_get("destination_name")?,
        },
        work: PutawayClaimWork::Loose {
            source_inventory_balance_id: row.try_get("source_inventory_balance_id")?,
            item_batch_id: row.try_get("item_batch_id")?,
            item_id: row.try_get("item_id")?,
            item_description: row.try_get("item_description")?,
            uom: row.try_get("uom")?,
            lot: row.try_get("lot")?,
            serial: row.try_get("serial")?,
            expiration: row.try_get("expiration")?,
            inventory_status: parse_inventory_status(
                &row.try_get::<String, _>("inventory_status")?,
            )?,
            quantity: planned_quantity,
        },
    })
}

async fn load_license_plate_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_user_id: i64,
) -> AppResult<PutawayClaim> {
    let row = sqlx::query(
        r#"
        SELECT task.inventory_owner_id,
               task.facility_id,
               task.priority,
               task.instructions,
               task.due_at,
               task.lease_expires_at,
               detail.license_plate_id,
               detail.source_location_id,
               detail.destination_location_id,
               detail.planned_balance_count,
               source_location.barcode AS source_barcode,
               source_location.name AS source_name,
               source_location.active AS source_active,
               source_location.receivable AS source_receivable,
               destination_location.barcode AS destination_barcode,
               destination_location.name AS destination_name,
               destination_location.active AS destination_active,
               destination_location.receivable AS destination_receivable,
               plate.barcode AS license_plate_barcode,
               plate.location_id AS plate_location_id,
               plate.deleted AS plate_deleted,
               (
                   (
                       SELECT COUNT(*)
                       FROM inventory_balances balance
                       WHERE balance.tenant_id = detail.tenant_id
                         AND balance.inventory_owner_id = detail.inventory_owner_id
                         AND balance.facility_id = detail.facility_id
                         AND balance.license_plate_id = detail.license_plate_id
                         AND balance.deleted IS NULL
                         AND balance.qty_on_hand > 0
                   ) = detail.planned_balance_count
                   AND NOT EXISTS (
                       SELECT 1
                       FROM inventory_balances balance
                       WHERE balance.tenant_id = detail.tenant_id
                         AND balance.inventory_owner_id = detail.inventory_owner_id
                         AND balance.facility_id = detail.facility_id
                         AND balance.license_plate_id = detail.license_plate_id
                         AND balance.deleted IS NULL
                         AND balance.qty_on_hand > 0
                         AND (
                             balance.location_id <> detail.source_location_id
                             OR balance.status <> 'available'
                             OR balance.qty_reserved <> 0
                             OR balance.qty_held <> 0
                             OR NOT EXISTS (
                                 SELECT 1
                                 FROM license_plate_putaway_task_contents content
                                 WHERE content.tenant_id = detail.tenant_id
                                   AND content.task_id = detail.task_id
                                   AND content.inventory_balance_id = balance.id
                                   AND content.item_batch_id = balance.item_batch_id
                                   AND content.item_id = balance.item_id
                                   AND content.uom = balance.uom
                                   AND content.inventory_status = balance.status
                                   AND content.planned_quantity = balance.qty_on_hand
                             )
                             OR NOT EXISTS (
                                 SELECT 1
                                 FROM item_batches batch
                                 INNER JOIN items item
                                   ON item.tenant_id = batch.tenant_id
                                  AND item.id = batch.item_id
                                  AND item.deleted IS NULL
                                 WHERE batch.tenant_id = balance.tenant_id
                                   AND batch.inventory_owner_id =
                                       balance.inventory_owner_id
                                   AND batch.id = balance.item_batch_id
                                   AND batch.item_id = balance.item_id
                                   AND batch.deleted IS NULL
                             )
                         )
                   )
               ) AS contents_match
        FROM work_tasks task
        INNER JOIN license_plate_putaway_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
         AND detail.closed_at IS NULL
        INNER JOIN locations source_location
          ON source_location.tenant_id = detail.tenant_id
         AND source_location.facility_id = detail.facility_id
         AND source_location.id = detail.source_location_id
         AND source_location.deleted IS NULL
        INNER JOIN locations destination_location
          ON destination_location.tenant_id = detail.tenant_id
         AND destination_location.facility_id = detail.facility_id
         AND destination_location.id = detail.destination_location_id
         AND destination_location.deleted IS NULL
        INNER JOIN license_plates plate
          ON plate.tenant_id = detail.tenant_id
         AND plate.inventory_owner_id = detail.inventory_owner_id
         AND plate.facility_id = detail.facility_id
         AND plate.id = detail.license_plate_id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.task_type = 'license_plate_putaway'
          AND task.status = 'in_progress'
          AND task.assigned_user_id = $3
          AND task.lease_expires_at > statement_timestamp()
          AND task.deleted IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("license plate putaway claim is no longer executable"))?;

    if !row.try_get::<bool, _>("source_active")?
        || !row.try_get::<bool, _>("source_receivable")?
        || !row.try_get::<bool, _>("destination_active")?
        || row.try_get::<bool, _>("destination_receivable")?
        || row
            .try_get::<Option<Timestamp>, _>("plate_deleted")?
            .is_some()
        || row.try_get::<i64, _>("plate_location_id")?
            != row.try_get::<i64, _>("source_location_id")?
        || !row.try_get::<bool, _>("contents_match")?
    {
        return Err(AppError::conflict(
            "license plate putaway claim is no longer executable",
        ));
    }

    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(PutawayClaim {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id,
        facility_id: row.try_get("facility_id")?,
        priority: row.try_get("priority")?,
        instructions: row.try_get("instructions")?,
        due_at: row.try_get("due_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        source_location: PutawayClaimSourceLocation {
            location_id: row.try_get("source_location_id")?,
            barcode: optional_barcode(row.try_get("source_barcode")?),
            name: row.try_get("source_name")?,
        },
        destination_location: PutawayClaimDestinationLocation {
            location_id: row.try_get("destination_location_id")?,
            barcode: required_barcode(row.try_get("destination_barcode")?, "putaway destination")?,
            name: row.try_get("destination_name")?,
        },
        work: PutawayClaimWork::LicensePlate {
            license_plate_id: row.try_get("license_plate_id")?,
            license_plate_barcode: required_barcode(
                row.try_get("license_plate_barcode")?,
                "license plate",
            )?,
            planned_balance_count: row.try_get("planned_balance_count")?,
        },
    })
}

async fn active_task_for_user_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
) -> AppResult<Option<(i64, WorkTaskType, String)>> {
    let row = sqlx::query(
        r#"
        SELECT id, task_type, status
        FROM work_tasks
        WHERE tenant_id = $1
          AND assigned_user_id = $2
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(access.user_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        let value: String = row.try_get("task_type")?;
        let task_type = WorkTaskType::parse(&value)
            .ok_or_else(|| AppError::internal(format!("invalid work task type: {value}")))?;
        Ok((row.try_get("id")?, task_type, row.try_get("status")?))
    })
    .transpose()
}

pub async fn claim_next_putaway_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_type: WorkTaskType,
) -> AppResult<Option<PutawayClaim>> {
    require_command_context(access, command)?;
    let task_type = require_putaway_type(task_type)?;
    let prepared = PreparedCommand::new(command, CLAIM_NEXT_OPERATION, &task_type)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;

    if let Some(claim) = prepared.replayed::<Option<PutawayClaim>>(&mut tx).await? {
        if let Some(claim) = claim.as_ref() {
            require_claim_visible_tx(&mut tx, access, claim.task_id, Some(task_type), &scope)
                .await?;
        }
        tx.commit().await?;
        return Ok(claim);
    }

    release_expired_tasks_tx(
        &mut tx,
        access.tenant_id,
        Some(command.actor_id.get()),
        &scope,
    )
    .await?;
    release_inaccessible_active_tasks_tx(&mut tx, access.tenant_id, command.actor_id.get(), &scope)
        .await?;
    if let Some((_, active_type, status)) = active_task_for_user_tx(&mut tx, access).await? {
        if status == "in_progress" || active_type != task_type {
            return Err(AppError::conflict(
                "user already has an active task; resume or release it first",
            ));
        }
    }

    let claimed_at = now_iso();
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"
        WITH candidate AS (
            SELECT id
            FROM work_tasks
            WHERE tenant_id = $1
              AND deleted IS NULL
              AND task_type = $2
              AND required_permission = 'wms'
              AND (scheduled_for IS NULL OR scheduled_for <= $3)
              AND (
                  (status = 'assigned' AND assigned_user_id = $4)
                  OR
                  (status = 'open' AND assigned_user_id IS NULL)
              )
              AND ($5 OR facility_id = ANY($6))
              AND ($7 OR inventory_owner_id = ANY($8))
            ORDER BY
                CASE WHEN status = 'assigned' THEN 0 ELSE 1 END,
                priority DESC,
                due_at ASC NULLS LAST,
                COALESCE(scheduled_for, created),
                created,
                id
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE work_tasks AS task
        SET status = 'in_progress',
            assigned_user_id = $4,
            started_at = COALESCE(task.started_at, $3),
            lease_expires_at =
                $3 + make_interval(secs => task.task_timeout_seconds::INT),
            modified = $3
        FROM candidate
        WHERE task.tenant_id = $1
          AND task.id = candidate.id
        RETURNING task.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_type.as_str())
    .bind(claimed_at)
    .bind(command.actor_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let claim = match task_id {
        Some(task_id) => {
            insert_progress_tx(
                &mut tx,
                access.tenant_id,
                task_id,
                None,
                Some(command.actor_id.get()),
                "started",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            Some(load_putaway_claim_tx(&mut tx, access, task_id, command.actor_id.get()).await?)
        }
        None => None,
    };
    prepared.commit(tx, claim).await
}

pub async fn claim_putaway_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
) -> AppResult<PutawayClaim> {
    require_command_context(access, command)?;
    if task_id <= 0 {
        return Err(AppError::bad_request("putaway task ID must be positive"));
    }
    let prepared = PreparedCommand::new(command, CLAIM_BY_ID_OPERATION, &task_id)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;

    if let Some(claim) = prepared.replayed::<PutawayClaim>(&mut tx).await? {
        require_claim_visible_tx(&mut tx, access, task_id, None, &scope).await?;
        tx.commit().await?;
        return Ok(claim);
    }

    release_expired_tasks_tx(
        &mut tx,
        access.tenant_id,
        Some(command.actor_id.get()),
        &scope,
    )
    .await?;
    release_inaccessible_active_tasks_tx(&mut tx, access.tenant_id, command.actor_id.get(), &scope)
        .await?;

    let target = sqlx::query(
        r#"
        SELECT task_type,
               status,
               assigned_user_id,
               scheduled_for,
               lease_expires_at > statement_timestamp() AS lease_is_current,
               facility_id,
               inventory_owner_id
        FROM work_tasks
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
          AND task_type IN ('putaway', 'license_plate_putaway')
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("putaway task"))?;
    if !task_dimensions(&target)?.is_allowed_by(&scope) {
        return Err(AppError::not_found("putaway task"));
    }
    let task_type = WorkTaskType::parse(&target.try_get::<String, _>("task_type")?)
        .ok_or_else(|| AppError::internal("work task has an invalid type"))?;
    require_putaway_type(task_type)?;
    let status: String = target.try_get("status")?;
    let assigned_user_id: Option<i64> = target.try_get("assigned_user_id")?;
    let lease_is_current: Option<bool> = target.try_get("lease_is_current")?;
    if status == "in_progress"
        && assigned_user_id == Some(command.actor_id.get())
        && lease_is_current == Some(true)
    {
        let claim = load_putaway_claim_tx(&mut tx, access, task_id, command.actor_id.get()).await?;
        return prepared.commit(tx, claim).await;
    }
    if !matches!(status.as_str(), "open" | "assigned")
        || assigned_user_id.is_some_and(|assigned| assigned != command.actor_id.get())
    {
        return Err(AppError::conflict("putaway task cannot be claimed"));
    }
    let scheduled_for: Option<Timestamp> = target.try_get("scheduled_for")?;
    let claimed_at = now_iso();
    if scheduled_for.is_some_and(|scheduled_for| scheduled_for > claimed_at) {
        return Err(AppError::conflict("putaway task is not scheduled yet"));
    }
    if let Some((active_task_id, _, _)) = active_task_for_user_tx(&mut tx, access).await? {
        if active_task_id != task_id {
            return Err(AppError::conflict(
                "user already has an active task; resume or release it first",
            ));
        }
    }

    let updated = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'in_progress',
            assigned_user_id = $1,
            started_at = COALESCE(started_at, $2),
            lease_expires_at =
                $2 + make_interval(secs => task_timeout_seconds::INT),
            modified = $2
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND task_type IN ('putaway', 'license_plate_putaway')
          AND status IN ('open', 'assigned')
          AND (assigned_user_id IS NULL OR assigned_user_id = $1)
        "#,
    )
    .bind(command.actor_id.get())
    .bind(claimed_at)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("putaway task cannot be claimed"));
    }
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "started",
        None,
        None,
        None,
        None,
        None,
    )
    .await?;
    let claim = load_putaway_claim_tx(&mut tx, access, task_id, command.actor_id.get()).await?;
    prepared.commit(tx, claim).await
}

pub async fn current_putaway_claim_in_scope(
    db: &Db,
    access: &TenantAccess,
) -> AppResult<Option<PutawayClaim>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let row = sqlx::query(
        r#"
        SELECT id,
               task_type,
               status,
               lease_expires_at > statement_timestamp() AS lease_is_current,
               facility_id,
               inventory_owner_id
        FROM work_tasks
        WHERE tenant_id = $1
          AND assigned_user_id = $2
          AND deleted IS NULL
          AND status IN ('assigned', 'in_progress')
        LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(access.user_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    if !task_dimensions(&row)?.is_allowed_by(&scope) {
        tx.commit().await?;
        return Ok(None);
    }
    let task_type = WorkTaskType::parse(&row.try_get::<String, _>("task_type")?)
        .ok_or_else(|| AppError::internal("work task has an invalid type"))?;
    if !matches!(
        task_type,
        WorkTaskType::Putaway | WorkTaskType::LicensePlatePutaway
    ) {
        return Err(AppError::conflict("active task is not a putaway workflow"));
    }
    let status: String = row.try_get("status")?;
    let lease_is_current: Option<bool> = row.try_get("lease_is_current")?;
    if status != "in_progress" || lease_is_current != Some(true) {
        tx.commit().await?;
        return Ok(None);
    }
    let task_id: i64 = row.try_get("id")?;
    let claim = load_putaway_claim_tx(&mut tx, access, task_id, access.user_id.get()).await?;
    tx.commit().await?;
    Ok(Some(claim))
}
