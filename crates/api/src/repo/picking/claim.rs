use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::picking::{
    ClaimNextPickCommand, ClaimPickByIdCommand, PickClaim, PickClaimContent,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryOwnerId, ItemBatchId,
    LicensePlateId, LocationId, OrderId, OrderLineId, PickContentId, PickContentState,
    PickQuantity, PickScanValue, PickTaskId, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

use super::{CLAIM_BY_ID_OPERATION, CLAIM_NEXT_OPERATION};

pub async fn claim_next(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ClaimNextPickCommand,
) -> AppResult<Option<PickClaim>> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLAIM_NEXT_OPERATION, &())?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    if let Some(claim) = prepared.replayed::<Option<PickClaim>>(&mut tx).await? {
        if let Some(claim) = claim.as_ref() {
            require_task_visible_tx(&mut tx, access.tenant_id, claim.task_id, &scope).await?;
        }
        tx.commit().await?;
        return Ok(claim);
    }

    let _ = command;
    release_expired_claims_tx(&mut tx, access.tenant_id, &scope).await?;
    release_inaccessible_claim_tx(&mut tx, access.tenant_id, context.actor_id.get(), &scope)
        .await?;
    if active_task_for_user_tx(&mut tx, access.tenant_id, context.actor_id.get())
        .await?
        .is_some()
    {
        return Err(AppError::conflict(
            "operator already has active pick work; resume or release it first",
        ));
    }

    let claimed_at = now_iso();
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"
        WITH candidate AS (
            SELECT id
            FROM pick_tasks
            WHERE tenant_id = $1 AND status = 'open'
              AND assigned_user_id IS NULL
              AND ($3 OR facility_id = ANY($4))
              AND ($5 OR inventory_owner_id = ANY($6))
            ORDER BY priority DESC, ship_by ASC NULLS LAST, created_at, id
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE pick_tasks task
        SET status = 'in_progress', assigned_user_id = $2,
            claimed_at = $7,
            lease_expires_at = $7 + make_interval(secs => task.task_timeout_seconds::INT)
        FROM candidate
        WHERE task.tenant_id = $1 AND task.id = candidate.id
        RETURNING task.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(context.actor_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(claimed_at)
    .fetch_optional(&mut *tx)
    .await?;
    let claim = match task_id {
        Some(task_id) => Some(
            load_claim_tx(
                &mut tx,
                access.tenant_id,
                PickTaskId::new(task_id).map_err(|error| AppError::internal(error.to_string()))?,
                context.actor_id.get(),
            )
            .await?,
        ),
        None => None,
    };
    Ok(prepared.commit(tx, claim).await?)
}

pub async fn claim_by_id(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ClaimPickByIdCommand,
) -> AppResult<PickClaim> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLAIM_BY_ID_OPERATION, &command.task_id)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    if let Some(claim) = prepared.replayed::<PickClaim>(&mut tx).await? {
        require_task_visible_tx(&mut tx, access.tenant_id, command.task_id, &scope).await?;
        tx.commit().await?;
        return Ok(claim);
    }

    release_expired_claims_tx(&mut tx, access.tenant_id, &scope).await?;
    release_inaccessible_claim_tx(&mut tx, access.tenant_id, context.actor_id.get(), &scope)
        .await?;
    let row = sqlx::query(
        r#"
        SELECT status, assigned_user_id,
               lease_expires_at > statement_timestamp() AS lease_is_current,
               facility_id, inventory_owner_id
        FROM pick_tasks
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.task_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick task"))?;
    require_scope_row(&row, &scope)?;
    let status: String = row.try_get("status")?;
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    if status == "in_progress"
        && assigned_user_id == Some(context.actor_id.get())
        && row.try_get::<Option<bool>, _>("lease_is_current")? == Some(true)
    {
        let claim = load_claim_tx(
            &mut tx,
            access.tenant_id,
            command.task_id,
            context.actor_id.get(),
        )
        .await?;
        return Ok(prepared.commit(tx, claim).await?);
    }
    if status != "open" || assigned_user_id.is_some() {
        return Err(AppError::conflict("pick task cannot be claimed"));
    }
    if active_task_for_user_tx(&mut tx, access.tenant_id, context.actor_id.get())
        .await?
        .is_some()
    {
        return Err(AppError::conflict(
            "operator already has active pick work; resume or release it first",
        ));
    }

    let claimed_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE pick_tasks
        SET status = 'in_progress', assigned_user_id = $1,
            claimed_at = $2,
            lease_expires_at = $2 + make_interval(secs => task_timeout_seconds::INT)
        WHERE tenant_id = $3 AND id = $4
          AND status = 'open' AND assigned_user_id IS NULL
        "#,
    )
    .bind(context.actor_id.get())
    .bind(claimed_at)
    .bind(access.tenant_id.get())
    .bind(command.task_id.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("pick task cannot be claimed"));
    }
    let claim = load_claim_tx(
        &mut tx,
        access.tenant_id,
        command.task_id,
        context.actor_id.get(),
    )
    .await?;
    Ok(prepared.commit(tx, claim).await?)
}

pub async fn current(db: &Db, access: &TenantAccess) -> AppResult<Option<PickClaim>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    release_expired_claims_tx(&mut tx, access.tenant_id, &scope).await?;
    release_inaccessible_claim_tx(&mut tx, access.tenant_id, access.user_id.get(), &scope).await?;
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM pick_tasks
        WHERE tenant_id = $1 AND assigned_user_id = $2
          AND status = 'in_progress' AND lease_expires_at > statement_timestamp()
        ORDER BY id LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(access.user_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    let claim = match task_id {
        Some(task_id) => Some(
            load_claim_tx(
                &mut tx,
                access.tenant_id,
                PickTaskId::new(task_id).map_err(|error| AppError::internal(error.to_string()))?,
                access.user_id.get(),
            )
            .await?,
        ),
        None => None,
    };
    tx.commit().await?;
    Ok(claim)
}

pub(super) async fn load_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: PickTaskId,
    actor_user_id: i64,
) -> AppResult<PickClaim> {
    let row = sqlx::query(
        r#"
        SELECT task.inventory_owner_id, task.facility_id, task.order_id,
               task.priority, task.ship_by, task.lease_expires_at,
               task.destination_location_id, orders.order_key,
               destination.barcode AS destination_barcode,
               destination.name AS destination_name,
               destination.active AS destination_active,
               destination.pickable AS destination_pickable,
               content.id AS content_id, content.order_item_id,
               content.source_allocation_id, content.source_inventory_balance_id,
               content.item_batch_id, content.source_location_id,
               content.source_license_plate_id, content.item_id, content.uom,
               content.inventory_status, content.planned_qty, content.state,
               source.barcode AS source_barcode, source.name AS source_name,
               source.active AS source_active, source.pickable AS source_pickable,
               source_plate.barcode AS source_license_plate_barcode,
               source_plate.deleted AS source_license_plate_deleted,
               allocation.inventory_balance_id AS allocation_balance_id,
               allocation.location_id AS allocation_location_id,
               allocation.license_plate_id AS allocation_license_plate_id,
               allocation.item_batch_id AS allocation_batch_id,
               allocation.item_id AS allocation_item_id,
               allocation.uom AS allocation_uom,
               allocation.inventory_status AS allocation_status,
               allocation.qty AS allocation_qty,
               allocation.status AS allocation_lifecycle,
               allocation.deleted AS allocation_deleted,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_license_plate_id,
               balance.item_batch_id AS balance_batch_id,
               balance.item_id AS balance_item_id,
               balance.uom AS balance_uom, balance.status AS balance_status,
               balance.qty_on_hand, balance.qty_reserved,
               balance.deleted AS balance_deleted,
               batch.lot, batch.serial, batch.expiration,
               batch.deleted AS batch_deleted,
               item.description AS item_description,
               item.deleted AS item_deleted,
               ARRAY(
                   SELECT barcode.name FROM barcodes barcode
                   WHERE barcode.tenant_id = content.tenant_id
                     AND barcode.item_id = content.item_id
                     AND barcode.deleted IS NULL
                   ORDER BY barcode.id
               ) AS item_barcodes
        FROM pick_tasks task
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        INNER JOIN orders
          ON orders.tenant_id = task.tenant_id
         AND orders.inventory_owner_id = task.inventory_owner_id
         AND orders.id = task.order_id AND orders.deleted IS NULL
        INNER JOIN locations source
          ON source.tenant_id = content.tenant_id
         AND source.facility_id = content.facility_id
         AND source.id = content.source_location_id AND source.deleted IS NULL
        INNER JOIN locations destination
          ON destination.tenant_id = task.tenant_id
         AND destination.facility_id = task.facility_id
         AND destination.id = task.destination_location_id
         AND destination.deleted IS NULL
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id = content.tenant_id
         AND allocation.inventory_owner_id = content.inventory_owner_id
         AND allocation.id = content.source_allocation_id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = content.tenant_id
         AND balance.inventory_owner_id = content.inventory_owner_id
         AND balance.facility_id = content.facility_id
         AND balance.id = content.source_inventory_balance_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = content.tenant_id
         AND batch.inventory_owner_id = content.inventory_owner_id
         AND batch.id = content.item_batch_id
        INNER JOIN items item
          ON item.tenant_id = content.tenant_id AND item.id = content.item_id
        LEFT JOIN license_plates source_plate
          ON source_plate.tenant_id = content.tenant_id
         AND source_plate.inventory_owner_id = content.inventory_owner_id
         AND source_plate.facility_id = content.facility_id
         AND source_plate.id = content.source_license_plate_id
        WHERE task.tenant_id = $1 AND task.id = $2
          AND task.status = 'in_progress' AND task.assigned_user_id = $3
          AND task.lease_expires_at > statement_timestamp()
          AND content.state = 'pending'
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id.get())
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("pick claim is no longer executable"))?;
    validate_claim_row(&row)?;

    let item_barcodes: Vec<String> = row.try_get("item_barcodes")?;
    Ok(PickClaim {
        task_id,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_key: row.try_get("order_key")?,
        priority: row.try_get("priority")?,
        ship_by: row.try_get("ship_by")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        destination_location_barcode: required_scan(
            row.try_get("destination_barcode")?,
            "destination location",
        )?,
        destination_location_name: row.try_get("destination_name")?,
        content: PickClaimContent {
            content_id: PickContentId::new(row.try_get("content_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            order_line_id: OrderLineId::new(row.try_get("order_item_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            inventory_allocation_id: InventoryAllocationId::new(
                row.try_get("source_allocation_id")?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
            source_inventory_balance_id: InventoryBalanceId::new(
                row.try_get("source_inventory_balance_id")?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
            item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            source_location_id: LocationId::new(row.try_get("source_location_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            source_location_barcode: required_scan(
                row.try_get("source_barcode")?,
                "source location",
            )?,
            source_location_name: row.try_get("source_name")?,
            source_license_plate_id: row
                .try_get::<Option<i64>, _>("source_license_plate_id")?
                .map(LicensePlateId::new)
                .transpose()
                .map_err(|error| AppError::internal(error.to_string()))?,
            source_license_plate_barcode: row
                .try_get::<Option<String>, _>("source_license_plate_barcode")?
                .map(PickScanValue::new)
                .transpose()
                .map_err(|error| AppError::internal(error.to_string()))?,
            item_id: row.try_get("item_id")?,
            item_description: row.try_get("item_description")?,
            item_barcodes: item_barcodes
                .into_iter()
                .map(PickScanValue::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::internal(error.to_string()))?,
            uom: row.try_get("uom")?,
            lot: row.try_get("lot")?,
            serial: row.try_get("serial")?,
            expiration: row.try_get("expiration")?,
            planned_quantity: PickQuantity::new(row.try_get("planned_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            state: PickContentState::Pending,
        },
    })
}

fn validate_claim_row(row: &sqlx::postgres::PgRow) -> AppResult<()> {
    let planned_qty: i64 = row.try_get("planned_qty")?;
    let source_license_plate_id: Option<i64> = row.try_get("source_license_plate_id")?;
    let source_plate_barcode: Option<String> = row.try_get("source_license_plate_barcode")?;
    let item_barcodes: Vec<String> = row.try_get("item_barcodes")?;
    let valid = row.try_get::<bool, _>("destination_active")?
        && !row.try_get::<bool, _>("destination_pickable")?
        && row.try_get::<bool, _>("source_active")?
        && row.try_get::<bool, _>("source_pickable")?
        && row.try_get::<String, _>("state")? == "pending"
        && row.try_get::<i64, _>("allocation_balance_id")?
            == row.try_get::<i64, _>("source_inventory_balance_id")?
        && row.try_get::<i64, _>("allocation_location_id")?
            == row.try_get::<i64, _>("source_location_id")?
        && row.try_get::<Option<i64>, _>("allocation_license_plate_id")? == source_license_plate_id
        && row.try_get::<i64, _>("allocation_batch_id")?
            == row.try_get::<i64, _>("item_batch_id")?
        && row.try_get::<i64, _>("allocation_item_id")? == row.try_get::<i64, _>("item_id")?
        && row.try_get::<String, _>("allocation_uom")? == row.try_get::<String, _>("uom")?
        && row.try_get::<String, _>("allocation_status")? == "available"
        && row.try_get::<i64, _>("allocation_qty")? == planned_qty
        && row.try_get::<String, _>("allocation_lifecycle")? == "allocated"
        && row
            .try_get::<Option<Timestamp>, _>("allocation_deleted")?
            .is_none()
        && row.try_get::<i64, _>("balance_location_id")?
            == row.try_get::<i64, _>("source_location_id")?
        && row.try_get::<Option<i64>, _>("balance_license_plate_id")? == source_license_plate_id
        && row.try_get::<i64, _>("balance_batch_id")? == row.try_get::<i64, _>("item_batch_id")?
        && row.try_get::<i64, _>("balance_item_id")? == row.try_get::<i64, _>("item_id")?
        && row.try_get::<String, _>("balance_uom")? == row.try_get::<String, _>("uom")?
        && row.try_get::<String, _>("balance_status")? == "available"
        && row.try_get::<i64, _>("qty_on_hand")? >= planned_qty
        && row.try_get::<i64, _>("qty_reserved")? >= planned_qty
        && row
            .try_get::<Option<Timestamp>, _>("balance_deleted")?
            .is_none()
        && row
            .try_get::<Option<Timestamp>, _>("batch_deleted")?
            .is_none()
        && row
            .try_get::<Option<Timestamp>, _>("item_deleted")?
            .is_none()
        && !item_barcodes.is_empty()
        && source_license_plate_id.is_none_or(|_| {
            row.try_get::<Option<Timestamp>, _>("source_license_plate_deleted")
                .ok()
                .flatten()
                .is_none()
                && source_plate_barcode.is_some_and(|value| !value.trim().is_empty())
        });
    if !valid {
        return Err(AppError::conflict("pick claim is no longer executable"));
    }
    Ok(())
}

fn required_scan(value: Option<String>, label: &str) -> AppResult<PickScanValue> {
    value
        .ok_or_else(|| AppError::conflict(format!("{label} must have a scannable barcode")))
        .and_then(|value| {
            PickScanValue::new(value).map_err(|error| AppError::conflict(error.to_string()))
        })
}

pub(super) async fn require_task_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: PickTaskId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT facility_id, inventory_owner_id FROM pick_tasks WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(task_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick task"))?;
    require_scope_row(&row, scope)
}

fn require_scope_row(row: &sqlx::postgres::PgRow, scope: &ScopeBindings) -> AppResult<()> {
    if !scope.includes_facility(row.try_get("facility_id")?)
        || !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
    {
        return Err(AppError::not_found("pick task"));
    }
    Ok(())
}

async fn active_task_for_user_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
) -> AppResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT id FROM pick_tasks
        WHERE tenant_id = $1 AND assigned_user_id = $2 AND status = 'in_progress'
        ORDER BY id LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?)
}

async fn release_expired_claims_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE pick_tasks
        SET status = 'open', assigned_user_id = NULL, claimed_at = NULL,
            lease_expires_at = NULL, last_released_at = statement_timestamp(),
            last_release_reason = 'lease_expired', last_release_note = NULL,
            release_count = release_count + 1
        WHERE tenant_id = $1 AND status = 'in_progress'
          AND lease_expires_at <= statement_timestamp()
          AND ($2 OR facility_id = ANY($3))
          AND ($4 OR inventory_owner_id = ANY($5))
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn release_inaccessible_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE pick_tasks
        SET status = 'open', assigned_user_id = NULL, claimed_at = NULL,
            lease_expires_at = NULL, last_released_at = statement_timestamp(),
            last_release_reason = 'scope_revoked', last_release_note = NULL,
            release_count = release_count + 1
        WHERE tenant_id = $1 AND assigned_user_id = $2 AND status = 'in_progress'
          AND (NOT $3 AND facility_id <> ALL($4)
               OR NOT $5 AND inventory_owner_id <> ALL($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(actor_user_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
