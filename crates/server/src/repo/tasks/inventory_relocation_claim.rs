use sqlx::Row;
use wareboxes_core::models::{
    InventoryRelocationClaim, InventoryRelocationClaimWork, InventoryRelocationLocation,
    InventoryRelocationWorkflow, InventoryStatus, TenantAccess, Timestamp,
};
use wareboxes_domain::{CommandContext, InventoryOwnerId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::idempotency::{require_command_context, PreparedCommand};

use super::leasing::{release_expired_tasks_tx, release_inaccessible_active_tasks_tx};
use super::{insert_progress_tx, TaskDimensions};

const CLAIM_NEXT_OPERATION: &str = "inventory_relocation.claim_next.v1";
const CLAIM_BY_ID_OPERATION: &str = "inventory_relocation.claim_by_id.v1";

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
    expected_workflow: Option<InventoryRelocationWorkflow>,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT task.facility_id, task.inventory_owner_id, detail.workflow
        FROM work_tasks task
        INNER JOIN inventory_relocation_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.deleted IS NULL
          AND task.task_type = 'inventory_relocation'
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory relocation task"))?;
    let workflow = InventoryRelocationWorkflow::parse(&row.try_get::<String, _>("workflow")?)
        .ok_or_else(|| AppError::internal("inventory relocation has an invalid workflow"))?;
    if expected_workflow.is_some_and(|expected| expected != workflow)
        || !task_dimensions(&row)?.is_allowed_by(scope)
    {
        return Err(AppError::not_found("inventory relocation task"));
    }
    Ok(())
}

async fn load_relocation_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_user_id: i64,
) -> AppResult<InventoryRelocationClaim> {
    let row = sqlx::query(
        r#"
        SELECT task.inventory_owner_id, task.facility_id, task.priority,
               task.instructions, task.due_at, task.lease_expires_at,
               detail.workflow, detail.source_inventory_balance_id,
               detail.license_plate_id, detail.source_location_id,
               detail.destination_location_id, detail.item_batch_id,
               detail.item_id, detail.uom, detail.inventory_status,
               detail.planned_quantity, detail.planned_balance_count,
               source_location.barcode AS source_barcode,
               source_location.name AS source_name,
               source_location.active AS source_active,
               destination_location.barcode AS destination_barcode,
               destination_location.name AS destination_name,
               destination_location.active AS destination_active,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_license_plate_id,
               balance.item_batch_id AS balance_item_batch_id,
               balance.item_id AS balance_item_id,
               balance.uom AS balance_uom,
               balance.status AS balance_status,
               balance.qty_on_hand, balance.qty_reserved, balance.qty_held,
               balance.deleted AS balance_deleted,
               batch.lot, batch.serial, batch.expiration,
               batch.deleted AS batch_deleted,
               item.description AS item_description,
               item.deleted AS item_deleted,
               plate.barcode AS license_plate_barcode,
               plate.location_id AS plate_location_id,
               plate.deleted AS plate_deleted
        FROM work_tasks task
        INNER JOIN inventory_relocation_tasks detail
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
        LEFT JOIN inventory_balances balance
          ON balance.tenant_id = detail.tenant_id
         AND balance.inventory_owner_id = detail.inventory_owner_id
         AND balance.facility_id = detail.facility_id
         AND balance.id = detail.source_inventory_balance_id
        LEFT JOIN item_batches batch
          ON batch.tenant_id = detail.tenant_id
         AND batch.inventory_owner_id = detail.inventory_owner_id
         AND batch.id = detail.item_batch_id
        LEFT JOIN items item
          ON item.tenant_id = detail.tenant_id
         AND item.id = detail.item_id
        LEFT JOIN license_plates plate
          ON plate.tenant_id = detail.tenant_id
         AND plate.inventory_owner_id = detail.inventory_owner_id
         AND plate.facility_id = detail.facility_id
         AND plate.id = detail.license_plate_id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.task_type = 'inventory_relocation'
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
    .ok_or_else(|| AppError::conflict("inventory relocation claim is no longer executable"))?;

    if !row.try_get::<bool, _>("source_active")? || !row.try_get::<bool, _>("destination_active")? {
        return Err(AppError::conflict(
            "inventory relocation locations are no longer active",
        ));
    }
    let workflow = InventoryRelocationWorkflow::parse(&row.try_get::<String, _>("workflow")?)
        .ok_or_else(|| AppError::internal("inventory relocation has an invalid workflow"))?;
    let work = match workflow {
        InventoryRelocationWorkflow::LooseBalance => {
            validate_loose_claim(&row)?;
            InventoryRelocationClaimWork::LooseBalance {
                source_inventory_balance_id: required_i64(
                    &row,
                    "source_inventory_balance_id",
                    "source balance",
                )?,
                item_batch_id: required_i64(&row, "item_batch_id", "item batch")?,
                item_id: required_i64(&row, "item_id", "item")?,
                item_description: row.try_get("item_description")?,
                uom: required_string(&row, "uom", "UOM")?,
                lot: row.try_get("lot")?,
                serial: row.try_get("serial")?,
                expiration: row.try_get("expiration")?,
                inventory_status: parse_inventory_status(&required_string(
                    &row,
                    "inventory_status",
                    "inventory status",
                )?)?,
                quantity: required_i64(&row, "planned_quantity", "planned quantity")?,
            }
        }
        InventoryRelocationWorkflow::LicensePlate => {
            validate_plate_claim(tx, access, task_id, &row).await?;
            InventoryRelocationClaimWork::LicensePlate {
                license_plate_id: required_i64(&row, "license_plate_id", "license plate")?,
                license_plate_barcode: required_barcode(
                    row.try_get("license_plate_barcode")?,
                    "license plate",
                )?,
                planned_balance_count: required_i64(
                    &row,
                    "planned_balance_count",
                    "planned balance count",
                )?,
            }
        }
    };
    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(InventoryRelocationClaim {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id,
        facility_id: row.try_get("facility_id")?,
        priority: row.try_get("priority")?,
        instructions: row.try_get("instructions")?,
        due_at: row.try_get("due_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        source_location: InventoryRelocationLocation {
            location_id: row.try_get("source_location_id")?,
            barcode: optional_barcode(row.try_get("source_barcode")?),
            name: row.try_get("source_name")?,
        },
        destination_location: InventoryRelocationLocation {
            location_id: row.try_get("destination_location_id")?,
            barcode: Some(required_barcode(
                row.try_get("destination_barcode")?,
                "relocation destination",
            )?),
            name: row.try_get("destination_name")?,
        },
        work,
    })
}

fn validate_loose_claim(row: &sqlx::postgres::PgRow) -> AppResult<()> {
    let planned_quantity = required_i64(row, "planned_quantity", "planned quantity")?;
    let available_quantity = row
        .try_get::<Option<i64>, _>("qty_on_hand")?
        .and_then(|on_hand| on_hand.checked_sub(row.try_get::<i64, _>("qty_reserved").ok()?))
        .and_then(|quantity| quantity.checked_sub(row.try_get::<i64, _>("qty_held").ok()?));
    if row
        .try_get::<Option<Timestamp>, _>("balance_deleted")?
        .is_some()
        || row
            .try_get::<Option<Timestamp>, _>("batch_deleted")?
            .is_some()
        || row
            .try_get::<Option<Timestamp>, _>("item_deleted")?
            .is_some()
        || row.try_get::<Option<i64>, _>("balance_location_id")?
            != row.try_get::<Option<i64>, _>("source_location_id")?
        || row
            .try_get::<Option<i64>, _>("balance_license_plate_id")?
            .is_some()
        || row.try_get::<Option<i64>, _>("balance_item_batch_id")?
            != row.try_get::<Option<i64>, _>("item_batch_id")?
        || row.try_get::<Option<i64>, _>("balance_item_id")?
            != row.try_get::<Option<i64>, _>("item_id")?
        || row.try_get::<Option<String>, _>("balance_uom")?
            != row.try_get::<Option<String>, _>("uom")?
        || row.try_get::<Option<String>, _>("balance_status")?
            != row.try_get::<Option<String>, _>("inventory_status")?
        || available_quantity.is_none_or(|quantity| quantity < planned_quantity)
    {
        return Err(AppError::conflict(
            "loose inventory relocation is no longer executable",
        ));
    }
    Ok(())
}

async fn validate_plate_claim(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    row: &sqlx::postgres::PgRow,
) -> AppResult<()> {
    let plate_id = required_i64(row, "license_plate_id", "license plate")?;
    if row
        .try_get::<Option<Timestamp>, _>("plate_deleted")?
        .is_some()
        || row.try_get::<Option<i64>, _>("plate_location_id")?
            != row.try_get::<Option<i64>, _>("source_location_id")?
    {
        return Err(AppError::conflict(
            "license plate relocation is no longer executable",
        ));
    }
    required_barcode(row.try_get("license_plate_barcode")?, "license plate")?;
    let snapshot_matches: bool = sqlx::query_scalar(
        r#"
        SELECT
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
                  AND (
                      balance.location_id <> detail.source_location_id
                      OR (
                          balance.qty_on_hand > 0
                          AND (balance.qty_reserved <> 0 OR balance.qty_held <> 0)
                      )
                      OR (
                          balance.qty_on_hand > 0
                          AND NOT EXISTS (
                              SELECT 1
                              FROM inventory_relocation_task_contents planned
                              WHERE planned.tenant_id = detail.tenant_id
                                AND planned.task_id = detail.task_id
                                AND planned.inventory_balance_id = balance.id
                                AND planned.item_batch_id = balance.item_batch_id
                                AND planned.item_id = balance.item_id
                                AND planned.uom = balance.uom
                                AND planned.inventory_status = balance.status
                                AND planned.planned_quantity = balance.qty_on_hand
                          )
                      )
                  )
            )
        FROM inventory_relocation_tasks detail
        WHERE detail.tenant_id = $1
          AND detail.task_id = $2
          AND detail.license_plate_id = $3
          AND detail.workflow = 'license_plate'
          AND detail.closed_at IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(plate_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("license plate relocation detail is no longer open"))?;
    if !snapshot_matches {
        return Err(AppError::conflict(
            "license plate contents changed after relocation planning",
        ));
    }
    Ok(())
}

fn required_i64(row: &sqlx::postgres::PgRow, column: &str, label: &str) -> AppResult<i64> {
    row.try_get::<Option<i64>, _>(column)?
        .ok_or_else(|| AppError::internal(format!("inventory relocation has no {label}")))
}

fn required_string(row: &sqlx::postgres::PgRow, column: &str, label: &str) -> AppResult<String> {
    row.try_get::<Option<String>, _>(column)?
        .ok_or_else(|| AppError::internal(format!("inventory relocation has no {label}")))
}

async fn active_task_for_user_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
) -> AppResult<Option<(i64, String, Option<InventoryRelocationWorkflow>, String)>> {
    let row = sqlx::query(
        r#"
        SELECT task.id, task.task_type, task.status, detail.workflow
        FROM work_tasks task
        LEFT JOIN inventory_relocation_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND task.assigned_user_id = $2
          AND task.deleted IS NULL
          AND task.status IN ('assigned', 'in_progress')
        LIMIT 1
        FOR UPDATE OF task
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(access.user_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        let workflow = row
            .try_get::<Option<String>, _>("workflow")?
            .map(|value| {
                InventoryRelocationWorkflow::parse(&value).ok_or_else(|| {
                    AppError::internal("inventory relocation has an invalid workflow")
                })
            })
            .transpose()?;
        Ok((
            row.try_get("id")?,
            row.try_get("task_type")?,
            workflow,
            row.try_get("status")?,
        ))
    })
    .transpose()
}

pub async fn claim_next_inventory_relocation_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    workflow: InventoryRelocationWorkflow,
) -> AppResult<Option<InventoryRelocationClaim>> {
    require_command_context(access, command)?;
    let prepared = PreparedCommand::new(command, CLAIM_NEXT_OPERATION, &workflow)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    if let Some(claim) = prepared
        .replayed::<Option<InventoryRelocationClaim>>(&mut tx)
        .await?
    {
        if let Some(claim) = claim.as_ref() {
            require_claim_visible_tx(&mut tx, access, claim.task_id, Some(workflow), &scope)
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
    if let Some((_, task_type, active_workflow, status)) =
        active_task_for_user_tx(&mut tx, access).await?
    {
        if status == "in_progress"
            || task_type != "inventory_relocation"
            || active_workflow != Some(workflow)
        {
            return Err(AppError::conflict(
                "user already has active work; resume or release it first",
            ));
        }
    }

    let claimed_at = now_iso();
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"
        WITH candidate AS (
            SELECT task.id
            FROM work_tasks task
            INNER JOIN inventory_relocation_tasks detail
              ON detail.tenant_id = task.tenant_id
             AND detail.task_id = task.id
             AND detail.closed_at IS NULL
            WHERE task.tenant_id = $1
              AND task.deleted IS NULL
              AND task.task_type = 'inventory_relocation'
              AND detail.workflow = $2
              AND task.required_permission = 'wms'
              AND (task.scheduled_for IS NULL OR task.scheduled_for <= $3)
              AND (
                  (task.status = 'assigned' AND task.assigned_user_id = $4)
                  OR (task.status = 'open' AND task.assigned_user_id IS NULL)
              )
              AND ($5 OR task.facility_id = ANY($6))
              AND ($7 OR task.inventory_owner_id = ANY($8))
            ORDER BY
                CASE WHEN task.status = 'assigned' THEN 0 ELSE 1 END,
                task.priority DESC,
                task.due_at ASC NULLS LAST,
                COALESCE(task.scheduled_for, task.created),
                task.created,
                task.id
            FOR UPDATE OF task SKIP LOCKED
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
    .bind(workflow.as_str())
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
            Some(load_relocation_claim_tx(&mut tx, access, task_id, command.actor_id.get()).await?)
        }
        None => None,
    };
    prepared.commit(tx, claim).await
}

pub async fn claim_inventory_relocation_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
) -> AppResult<InventoryRelocationClaim> {
    require_command_context(access, command)?;
    if task_id <= 0 {
        return Err(AppError::bad_request(
            "inventory relocation task ID must be positive",
        ));
    }
    let prepared = PreparedCommand::new(command, CLAIM_BY_ID_OPERATION, &task_id)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    if let Some(claim) = prepared
        .replayed::<InventoryRelocationClaim>(&mut tx)
        .await?
    {
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
        SELECT task.status, task.assigned_user_id, task.scheduled_for,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               task.facility_id, task.inventory_owner_id
        FROM work_tasks task
        INNER JOIN inventory_relocation_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
         AND detail.closed_at IS NULL
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.deleted IS NULL
          AND task.task_type = 'inventory_relocation'
        FOR UPDATE OF task
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory relocation task"))?;
    if !task_dimensions(&target)?.is_allowed_by(&scope) {
        return Err(AppError::not_found("inventory relocation task"));
    }
    let status: String = target.try_get("status")?;
    let assigned_user_id: Option<i64> = target.try_get("assigned_user_id")?;
    let lease_is_current: Option<bool> = target.try_get("lease_is_current")?;
    if status == "in_progress"
        && assigned_user_id == Some(command.actor_id.get())
        && lease_is_current == Some(true)
    {
        let claim =
            load_relocation_claim_tx(&mut tx, access, task_id, command.actor_id.get()).await?;
        return prepared.commit(tx, claim).await;
    }
    if !matches!(status.as_str(), "open" | "assigned")
        || assigned_user_id.is_some_and(|assigned| assigned != command.actor_id.get())
    {
        return Err(AppError::conflict(
            "inventory relocation task cannot be claimed",
        ));
    }
    let claimed_at = now_iso();
    if target
        .try_get::<Option<Timestamp>, _>("scheduled_for")?
        .is_some_and(|scheduled_for| scheduled_for > claimed_at)
    {
        return Err(AppError::conflict(
            "inventory relocation task is not scheduled yet",
        ));
    }
    if let Some((active_task_id, _, _, _)) = active_task_for_user_tx(&mut tx, access).await? {
        if active_task_id != task_id {
            return Err(AppError::conflict(
                "user already has active work; resume or release it first",
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
          AND task_type = 'inventory_relocation'
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
        return Err(AppError::conflict(
            "inventory relocation task cannot be claimed",
        ));
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
    let claim = load_relocation_claim_tx(&mut tx, access, task_id, command.actor_id.get()).await?;
    prepared.commit(tx, claim).await
}

pub async fn current_inventory_relocation_claim_in_scope(
    db: &Db,
    access: &TenantAccess,
) -> AppResult<Option<InventoryRelocationClaim>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let row = sqlx::query(
        r#"
        SELECT id, task_type, status,
               lease_expires_at > statement_timestamp() AS lease_is_current,
               facility_id, inventory_owner_id
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
    if row.try_get::<String, _>("task_type")? != "inventory_relocation" {
        return Err(AppError::conflict(
            "active task is not an inventory relocation workflow",
        ));
    }
    if row.try_get::<String, _>("status")? != "in_progress"
        || row.try_get::<Option<bool>, _>("lease_is_current")? != Some(true)
    {
        tx.commit().await?;
        return Ok(None);
    }
    let task_id: i64 = row.try_get("id")?;
    let claim = load_relocation_claim_tx(&mut tx, access, task_id, access.user_id.get()).await?;
    tx.commit().await?;
    Ok(Some(claim))
}
