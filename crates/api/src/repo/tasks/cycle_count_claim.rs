use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    CycleCountClaim, CycleCountClaimItem, CycleCountClaimLocation, CycleCountClaimStock,
    InventoryStatus, TenantAccess, Timestamp,
};
use wareboxes_domain::InventoryOwnerId;

use crate::db::{bind_tenant_context, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::leasing::{release_expired_tasks_tx, release_inaccessible_active_tasks_tx};
use super::{insert_progress_tx, require_replayed_task_visible_tx, TaskDimensions};

const CLAIM_NEXT_OPERATION: &str = "cycle_count.claim_next.v1";
const CLAIM_BY_ID_OPERATION: &str = "cycle_count.claim_by_id.v1";

fn required_barcode(value: Option<String>, label: &str) -> AppResult<String> {
    value
        .filter(|barcode| !barcode.trim().is_empty())
        .ok_or_else(|| AppError::conflict(format!("{label} must have a scannable barcode")))
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

async fn load_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_user_id: i64,
) -> AppResult<CycleCountClaim> {
    let row = sqlx::query(
        r#"
        SELECT task.inventory_owner_id,
               task.facility_id,
               task.priority,
               task.instructions,
               task.due_at,
               task.lease_expires_at,
               detail.location_id,
               detail.item_id,
               detail.inventory_balance_id,
               location.barcode AS location_barcode,
               location.name AS location_name,
               location.active AS location_active,
               balance.location_id AS balance_location_id,
               balance.item_id AS balance_item_id,
               balance.license_plate_id,
               balance.uom,
               balance.status,
               balance.deleted AS balance_deleted,
               batch.lot,
               batch.expiration,
               batch.serial,
               batch.deleted AS batch_deleted,
               item.description AS item_description,
               item.deleted AS item_deleted,
               plate.barcode AS license_plate_barcode,
               plate.location_id AS plate_location_id,
               plate.deleted AS plate_deleted,
               ARRAY(
                   SELECT barcode.name
                   FROM barcodes barcode
                   WHERE barcode.tenant_id = detail.tenant_id
                     AND barcode.item_id = detail.item_id
                     AND barcode.deleted IS NULL
                     AND BTRIM(barcode.name) <> ''
                   ORDER BY barcode.id
               ) AS item_barcodes
        FROM work_tasks task
        INNER JOIN cycle_count_item_location_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        INNER JOIN locations location
          ON location.tenant_id = detail.tenant_id
         AND location.facility_id = detail.facility_id
         AND location.id = detail.location_id
         AND location.deleted IS NULL
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = detail.tenant_id
         AND balance.inventory_owner_id = detail.inventory_owner_id
         AND balance.facility_id = detail.facility_id
         AND balance.id = detail.inventory_balance_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
        INNER JOIN items item
          ON item.tenant_id = detail.tenant_id
         AND item.id = detail.item_id
        LEFT JOIN license_plates plate
          ON plate.tenant_id = balance.tenant_id
         AND plate.inventory_owner_id = balance.inventory_owner_id
         AND plate.facility_id = balance.facility_id
         AND plate.id = balance.license_plate_id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.task_type = 'cycle_count_item_location'
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
    .ok_or_else(|| AppError::conflict("cycle count claim is no longer executable"))?;

    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    if !(TaskDimensions {
        facility_id: Some(facility_id),
        inventory_owner_id: Some(inventory_owner_id),
    })
    .is_allowed_by(&ScopeBindings::for_access(access))
    {
        return Err(AppError::not_found("cycle count task"));
    }

    let location_id: i64 = row.try_get("location_id")?;
    let item_id: i64 = row.try_get("item_id")?;
    let license_plate_id: Option<i64> = row.try_get("license_plate_id")?;
    let plate_location_id: Option<i64> = row.try_get("plate_location_id")?;
    if !row.try_get::<bool, _>("location_active")?
        || row
            .try_get::<Option<Timestamp>, _>("balance_deleted")?
            .is_some()
        || row
            .try_get::<Option<Timestamp>, _>("batch_deleted")?
            .is_some()
        || row
            .try_get::<Option<Timestamp>, _>("item_deleted")?
            .is_some()
        || row.try_get::<i64, _>("balance_location_id")? != location_id
        || row.try_get::<i64, _>("balance_item_id")? != item_id
        || license_plate_id.is_some()
            && (row
                .try_get::<Option<Timestamp>, _>("plate_deleted")?
                .is_some()
                || plate_location_id != Some(location_id))
    {
        return Err(AppError::conflict(
            "cycle count claim is no longer executable",
        ));
    }

    let item_barcodes: Vec<String> = row.try_get("item_barcodes")?;
    if item_barcodes.is_empty() {
        return Err(AppError::conflict(
            "cycle count item must have a scannable barcode",
        ));
    }
    let license_plate_barcode = match license_plate_id {
        Some(_) => Some(required_barcode(
            row.try_get("license_plate_barcode")?,
            "cycle count license plate",
        )?),
        None => None,
    };

    Ok(CycleCountClaim {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id: InventoryOwnerId::new(inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id,
        priority: row.try_get("priority")?,
        instructions: row.try_get("instructions")?,
        due_at: row.try_get("due_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        location: CycleCountClaimLocation {
            location_id,
            barcode: required_barcode(row.try_get("location_barcode")?, "cycle count location")?,
            name: row.try_get("location_name")?,
        },
        item: CycleCountClaimItem {
            item_id,
            description: row.try_get("item_description")?,
            barcodes: item_barcodes,
        },
        stock: CycleCountClaimStock {
            inventory_balance_id: row.try_get("inventory_balance_id")?,
            license_plate_id,
            license_plate_barcode,
            uom: row.try_get("uom")?,
            lot: row.try_get("lot")?,
            expiration: row.try_get("expiration")?,
            serial: row.try_get("serial")?,
            inventory_status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
        },
    })
}

pub async fn claim_next_cycle_count_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
) -> AppResult<Option<CycleCountClaim>> {
    command.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(command, CLAIM_NEXT_OPERATION, &())?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(access.tenant_id.get())
        .bind(command.actor_id.get())
        .execute(&mut *tx)
        .await?;

    if let Some(claim) = prepared
        .replayed::<Option<CycleCountClaim>>(&mut tx)
        .await?
    {
        if let Some(claim) = claim.as_ref() {
            require_replayed_task_visible_tx(&mut tx, access.tenant_id, claim.task_id, &scope)
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
    if operator_has_conflicting_active_task_tx(&mut tx, access, command.actor_id.get()).await? {
        return Err(AppError::conflict(
            "user already has active work; resume or release it first",
        ));
    }

    let now = crate::db::now_iso();
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"
        WITH candidate AS (
            SELECT task.id
            FROM work_tasks task
            INNER JOIN cycle_count_item_location_tasks detail
              ON detail.tenant_id = task.tenant_id
             AND detail.task_id = task.id
            INNER JOIN locations location
              ON location.tenant_id = detail.tenant_id
             AND location.facility_id = detail.facility_id
             AND location.id = detail.location_id
            INNER JOIN inventory_balances balance
              ON balance.tenant_id = detail.tenant_id
             AND balance.inventory_owner_id = detail.inventory_owner_id
             AND balance.facility_id = detail.facility_id
             AND balance.id = detail.inventory_balance_id
            INNER JOIN item_batches batch
              ON batch.tenant_id = balance.tenant_id
             AND batch.inventory_owner_id = balance.inventory_owner_id
             AND batch.id = balance.item_batch_id
            INNER JOIN items item
              ON item.tenant_id = detail.tenant_id
             AND item.id = detail.item_id
            LEFT JOIN license_plates plate
              ON plate.tenant_id = balance.tenant_id
             AND plate.inventory_owner_id = balance.inventory_owner_id
             AND plate.facility_id = balance.facility_id
             AND plate.id = balance.license_plate_id
            WHERE task.tenant_id = $1
              AND task.task_type = 'cycle_count_item_location'
              AND ((task.status = 'assigned' AND task.assigned_user_id = $3)
                OR (task.status = 'open' AND task.assigned_user_id IS NULL))
              AND task.deleted IS NULL
              AND (task.scheduled_for IS NULL OR task.scheduled_for <= $2)
              AND ($4 OR task.facility_id = ANY($5))
              AND ($6 OR task.inventory_owner_id = ANY($7))
              AND location.deleted IS NULL
              AND location.active
              AND BTRIM(COALESCE(location.barcode, '')) <> ''
              AND balance.deleted IS NULL
              AND balance.location_id = detail.location_id
              AND balance.item_id = detail.item_id
              AND batch.deleted IS NULL
              AND item.deleted IS NULL
              AND EXISTS (
                  SELECT 1
                  FROM barcodes barcode
                  WHERE barcode.tenant_id = detail.tenant_id
                    AND barcode.item_id = detail.item_id
                    AND barcode.deleted IS NULL
                    AND BTRIM(barcode.name) <> ''
              )
              AND (
                  balance.license_plate_id IS NULL
                  OR (
                      plate.deleted IS NULL
                      AND plate.location_id = detail.location_id
                      AND BTRIM(COALESCE(plate.barcode, '')) <> ''
                  )
              )
            ORDER BY CASE WHEN task.status='assigned' THEN 0 ELSE 1 END,
                     task.priority DESC, task.due_at ASC NULLS LAST,
                     COALESCE(task.scheduled_for, task.created), task.created, task.id
            FOR UPDATE OF task SKIP LOCKED
            LIMIT 1
        )
        UPDATE work_tasks task
        SET status = 'in_progress',
            assigned_user_id = $3,
            started_at = COALESCE(task.started_at, $2),
            lease_expires_at =
                $2 + make_interval(secs => task.task_timeout_seconds::INT),
            modified = $2
        FROM candidate
        WHERE task.tenant_id = $1 AND task.id = candidate.id
        RETURNING task.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(now)
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
            Some(load_claim_tx(&mut tx, access, task_id, command.actor_id.get()).await?)
        }
        None => None,
    };
    Ok(prepared.commit(tx, claim).await?)
}

async fn operator_has_conflicting_active_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    actor_user_id: i64,
) -> AppResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id
        FROM work_tasks
        WHERE tenant_id = $1
          AND assigned_user_id = $2
          AND status IN ('assigned', 'in_progress')
          AND deleted IS NULL
          AND NOT (status='assigned' AND task_type='cycle_count_item_location')
        LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .is_some())
}

async fn task_is_scannable_in_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    actor_user_id: i64,
    task_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT task.facility_id, task.inventory_owner_id,
               task.status,
               task.assigned_user_id,
               location.active
                   AND location.deleted IS NULL
                   AND BTRIM(COALESCE(location.barcode, '')) <> ''
                   AND balance.deleted IS NULL
                   AND balance.location_id = detail.location_id
                   AND balance.item_id = detail.item_id
                   AND batch.deleted IS NULL
                   AND item.deleted IS NULL
                   AND EXISTS (
                       SELECT 1 FROM barcodes barcode
                       WHERE barcode.tenant_id = detail.tenant_id
                         AND barcode.item_id = detail.item_id
                         AND barcode.deleted IS NULL
                         AND BTRIM(barcode.name) <> ''
                   )
                   AND (
                       balance.license_plate_id IS NULL
                       OR (
                           plate.deleted IS NULL
                           AND plate.location_id = detail.location_id
                           AND BTRIM(COALESCE(plate.barcode, '')) <> ''
                       )
                   ) AS scannable
        FROM work_tasks task
        INNER JOIN cycle_count_item_location_tasks detail
          ON detail.tenant_id = task.tenant_id AND detail.task_id = task.id
        INNER JOIN locations location
          ON location.tenant_id = detail.tenant_id
         AND location.facility_id = detail.facility_id
         AND location.id = detail.location_id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = detail.tenant_id
         AND balance.inventory_owner_id = detail.inventory_owner_id
         AND balance.facility_id = detail.facility_id
         AND balance.id = detail.inventory_balance_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
        INNER JOIN items item
          ON item.tenant_id = detail.tenant_id AND item.id = detail.item_id
        LEFT JOIN license_plates plate
          ON plate.tenant_id = balance.tenant_id
         AND plate.inventory_owner_id = balance.inventory_owner_id
         AND plate.facility_id = balance.facility_id
         AND plate.id = balance.license_plate_id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.task_type = 'cycle_count_item_location'
          AND task.deleted IS NULL
        FOR UPDATE OF task
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let row = row.ok_or_else(|| AppError::not_found("cycle count task"))?;
    let dimensions = TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    };
    if !dimensions.is_allowed_by(scope) {
        return Err(AppError::not_found("cycle count task"));
    }
    let status: String = row.try_get("status")?;
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    if !((status == "open" && assigned_user_id.is_none())
        || (status == "assigned" && assigned_user_id == Some(actor_user_id)))
        || !row.try_get::<bool, _>("scannable")?
    {
        return Err(AppError::conflict(
            "cycle count task is not available for scanner execution",
        ));
    }
    Ok(())
}

async fn claim_specific_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    actor_user_id: i64,
    task_id: i64,
) -> AppResult<bool> {
    let now = crate::db::now_iso();
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
          AND task_type = 'cycle_count_item_location'
          AND ((status='assigned' AND assigned_user_id=$1)
            OR (status='open' AND assigned_user_id IS NULL))
          AND deleted IS NULL
        "#,
    )
    .bind(actor_user_id)
    .bind(now)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn claim_cycle_count_by_id_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
) -> AppResult<CycleCountClaim> {
    command.require_actor(access.tenant_id, access.user_id)?;
    if task_id <= 0 {
        return Err(AppError::bad_request(
            "cycle count task ID must be positive",
        ));
    }
    let prepared = PreparedCommand::new_v1(command, CLAIM_BY_ID_OPERATION, &task_id)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(access.tenant_id.get())
        .bind(command.actor_id.get())
        .execute(&mut *tx)
        .await?;
    if let Some(claim) = prepared.replayed::<CycleCountClaim>(&mut tx).await? {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
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
    let conflicting_active: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM work_tasks
        WHERE tenant_id=$1 AND assigned_user_id=$2 AND deleted IS NULL
          AND status IN ('assigned','in_progress') AND id<>$3)"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.actor_id.get())
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await?;
    if conflicting_active {
        return Err(AppError::conflict(
            "user already has active work; resume or release it first",
        ));
    }
    task_is_scannable_in_scope_tx(&mut tx, access, command.actor_id.get(), task_id, &scope).await?;
    if !claim_specific_tx(&mut tx, access, command.actor_id.get(), task_id).await? {
        return Err(AppError::conflict(
            "cycle count task is not available for scanner execution",
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
    let claim = load_claim_tx(&mut tx, access, task_id, command.actor_id.get()).await?;
    Ok(prepared.commit(tx, claim).await?)
}

pub async fn get_current_cycle_count_claim_in_scope(
    db: &Db,
    access: &TenantAccess,
    actor_user_id: i64,
) -> AppResult<Option<CycleCountClaim>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM work_tasks
        WHERE tenant_id = $1
          AND task_type = 'cycle_count_item_location'
          AND status = 'in_progress'
          AND assigned_user_id = $2
          AND lease_expires_at > statement_timestamp()
          AND deleted IS NULL
        ORDER BY id
        LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(actor_user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let claim = match task_id {
        Some(task_id) => Some(load_claim_tx(&mut tx, access, task_id, actor_user_id).await?),
        None => None,
    };
    tx.commit().await?;
    Ok(claim)
}
