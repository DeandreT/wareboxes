//! Inventory audit waves and scoped location counts.

use sqlx::Row;
use wareboxes_core::dto::{AddAuditLocationCount, AuditLocationCountUpdate};
use wareboxes_core::models::{AuditApprovalStatus, AuditLocationCount, AuditWave, TenantAccess};
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};

async fn lock_wave_dependencies_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    audit_id: i64,
) -> AppResult<bool> {
    let row = sqlx::query(
        r#"
        SELECT wave.id
        FROM audit_waves wave
        INNER JOIN facilities facility
            ON facility.tenant_id = wave.tenant_id
           AND facility.id = wave.facility_id
           AND facility.deleted IS NULL
        INNER JOIN inventory_owners inventory_owner
            ON inventory_owner.tenant_id = wave.tenant_id
           AND inventory_owner.id = wave.inventory_owner_id
           AND inventory_owner.deleted IS NULL
        INNER JOIN inventory_owner_facilities assignment
            ON assignment.tenant_id = wave.tenant_id
           AND assignment.inventory_owner_id = wave.inventory_owner_id
           AND assignment.facility_id = wave.facility_id
           AND assignment.deleted IS NULL
        WHERE wave.tenant_id = $1 AND wave.id = $2
        FOR UPDATE OF wave
        FOR SHARE OF facility, inventory_owner, assignment
        "#,
    )
    .bind(tenant_id.get())
    .bind(audit_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.is_some())
}

async fn lock_count_dependencies_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    count_id: i64,
) -> AppResult<bool> {
    let row = sqlx::query(
        r#"
        SELECT count.id
        FROM audit_location_counts count
        INNER JOIN audit_waves wave
            ON wave.tenant_id = count.tenant_id
           AND wave.inventory_owner_id = count.inventory_owner_id
           AND wave.facility_id = count.facility_id
           AND wave.id = count.audit_id
           AND wave.deleted IS NULL
        INNER JOIN facilities facility
            ON facility.tenant_id = count.tenant_id
           AND facility.id = count.facility_id
           AND facility.deleted IS NULL
        INNER JOIN inventory_owners inventory_owner
            ON inventory_owner.tenant_id = count.tenant_id
           AND inventory_owner.id = count.inventory_owner_id
           AND inventory_owner.deleted IS NULL
        INNER JOIN inventory_owner_facilities assignment
            ON assignment.tenant_id = count.tenant_id
           AND assignment.inventory_owner_id = count.inventory_owner_id
           AND assignment.facility_id = count.facility_id
           AND assignment.deleted IS NULL
        INNER JOIN locations location
            ON location.tenant_id = count.tenant_id
           AND location.facility_id = count.facility_id
           AND location.id = count.location_id
           AND location.deleted IS NULL
        INNER JOIN items item
            ON item.tenant_id = count.tenant_id
           AND item.id = count.item_id
           AND item.deleted IS NULL
        INNER JOIN inventory_owner_items owner_item
            ON owner_item.tenant_id = count.tenant_id
           AND owner_item.inventory_owner_id = count.inventory_owner_id
           AND owner_item.item_id = count.item_id
           AND owner_item.deleted IS NULL
        WHERE count.tenant_id = $1 AND count.id = $2
        FOR UPDATE OF count
        FOR SHARE OF wave, facility, inventory_owner, assignment, location, item, owner_item
        "#,
    )
    .bind(tenant_id.get())
    .bind(count_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.is_some())
}

async fn lock_wave_count_dependencies_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    audit_id: i64,
) -> AppResult<bool> {
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_location_counts WHERE tenant_id = $1 AND audit_id = $2 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(audit_id)
    .fetch_one(&mut **tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT count.id
        FROM audit_location_counts count
        INNER JOIN locations location
            ON location.tenant_id = count.tenant_id
           AND location.facility_id = count.facility_id
           AND location.id = count.location_id
           AND location.deleted IS NULL
        INNER JOIN items item
            ON item.tenant_id = count.tenant_id
           AND item.id = count.item_id
           AND item.deleted IS NULL
        INNER JOIN inventory_owner_items owner_item
            ON owner_item.tenant_id = count.tenant_id
           AND owner_item.inventory_owner_id = count.inventory_owner_id
           AND owner_item.item_id = count.item_id
           AND owner_item.deleted IS NULL
        WHERE count.tenant_id = $1
          AND count.audit_id = $2
          AND count.deleted IS NULL
        FOR UPDATE OF count
        FOR SHARE OF location, item, owner_item
        "#,
    )
    .bind(tenant_id.get())
    .bind(audit_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(i64::try_from(rows.len()).is_ok_and(|valid| valid == total))
}

fn map_wave(row: &sqlx::postgres::PgRow) -> AppResult<AuditWave> {
    Ok(AuditWave {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_by: row.try_get("created_by")?,
    })
}

fn map_count(row: &sqlx::postgres::PgRow) -> AppResult<AuditLocationCount> {
    let approval_status: String = row.try_get("approval_status")?;
    Ok(AuditLocationCount {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        started: row.try_get("started")?,
        ended: row.try_get("ended")?,
        audit_id: row.try_get("audit_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get("serial")?,
        on_hand: row.try_get("on_hand")?,
        count: row.try_get("count")?,
        revision: row.try_get("revision")?,
        approval_status: AuditApprovalStatus::parse(&approval_status).ok_or_else(|| {
            AppError::internal(format!("unknown audit approval status: {approval_status}"))
        })?,
    })
}

pub async fn get_audit_waves(
    db: &Db,
    access: &TenantAccess,
    show_deleted: bool,
) -> AppResult<Vec<AuditWave>> {
    let scope = ScopeBindings::for_access(access);
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, facility_id, inventory_owner_id, created, deleted,
               name, description, created_by
        FROM audit_waves
        WHERE tenant_id = $1
          AND ($2 OR deleted IS NULL)
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        ORDER BY id DESC
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(show_deleted)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(db)
    .await?;
    rows.iter().map(map_wave).collect()
}

pub async fn add_audit_wave(
    db: &Db,
    access: &TenantAccess,
    user_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    name: &str,
    description: Option<&str>,
) -> AppResult<Option<i64>> {
    let mut tx = db.begin().await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let dependencies = sqlx::query(
        r#"
        SELECT facility.id
        FROM facilities facility
        INNER JOIN inventory_owner_facilities assignment
            ON assignment.tenant_id = facility.tenant_id
           AND assignment.facility_id = facility.id
           AND assignment.inventory_owner_id = $3
           AND assignment.deleted IS NULL
        INNER JOIN inventory_owners inventory_owner
            ON inventory_owner.tenant_id = assignment.tenant_id
           AND inventory_owner.id = assignment.inventory_owner_id
           AND inventory_owner.deleted IS NULL
        WHERE facility.tenant_id = $1
          AND facility.id = $2
          AND facility.deleted IS NULL
          AND ($4 OR facility.id = ANY($5))
          AND ($6 OR inventory_owner.id = ANY($7))
        FOR SHARE OF facility, assignment, inventory_owner
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(inventory_owner_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    if dependencies.is_none() {
        return Ok(None);
    }
    let id = sqlx::query_scalar(
        r#"
        INSERT INTO audit_waves
            (tenant_id, facility_id, inventory_owner_id, created, name, description, created_by)
        SELECT facility.tenant_id, facility.id, inventory_owner.id, $6, $7, $8, $9
        FROM facilities facility
        INNER JOIN inventory_owner_facilities assignment
            ON assignment.tenant_id = facility.tenant_id
           AND assignment.facility_id = facility.id
           AND assignment.inventory_owner_id = $3
           AND assignment.deleted IS NULL
        INNER JOIN inventory_owners inventory_owner
            ON inventory_owner.tenant_id = assignment.tenant_id
           AND inventory_owner.id = assignment.inventory_owner_id
           AND inventory_owner.deleted IS NULL
        WHERE facility.tenant_id = $1
          AND facility.id = $2
          AND facility.deleted IS NULL
          AND ($4 OR facility.id = ANY($5))
          AND ($10 OR inventory_owner.id = ANY($11))
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(inventory_owner_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(now_iso())
    .bind(name)
    .bind(description)
    .bind(user_id)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn update_audit_wave(
    db: &Db,
    access: &TenantAccess,
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let result = sqlx::query(
        r#"
        UPDATE audit_waves
        SET name = COALESCE($1, name), description = COALESCE($2, description)
        WHERE tenant_id = $3
          AND id = $4
          AND ($5 OR facility_id = ANY($6))
          AND ($7 OR inventory_owner_id = ANY($8))
        "#,
    )
    .bind(name)
    .bind(description)
    .bind(access.tenant_id.get())
    .bind(id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut *tx)
    .await?;
    let changed = result.rows_affected() > 0;
    tx.commit().await?;
    Ok(changed)
}

pub async fn set_audit_wave_deleted(
    db: &Db,
    access: &TenantAccess,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    if !deleted
        && (!lock_wave_dependencies_tx(&mut tx, access.tenant_id, id).await?
            || !lock_wave_count_dependencies_tx(&mut tx, access.tenant_id, id).await?)
    {
        return Ok(false);
    }
    let result = sqlx::query(
        r#"
        UPDATE audit_waves
        SET deleted = $1
        WHERE tenant_id = $2
          AND id = $3
          AND (
              ($1::TIMESTAMPTZ IS NOT NULL AND deleted IS NULL)
              OR ($1::TIMESTAMPTZ IS NULL AND deleted IS NOT NULL)
          )
          AND ($4 OR facility_id = ANY($5))
          AND ($6 OR inventory_owner_id = ANY($7))
          AND (
              $1::TIMESTAMPTZ IS NOT NULL
              OR EXISTS (
                  SELECT 1
                  FROM facilities facility
                  INNER JOIN inventory_owners inventory_owner
                      ON inventory_owner.tenant_id = facility.tenant_id
                     AND inventory_owner.id = audit_waves.inventory_owner_id
                     AND inventory_owner.deleted IS NULL
                  INNER JOIN inventory_owner_facilities assignment
                      ON assignment.tenant_id = facility.tenant_id
                     AND assignment.facility_id = facility.id
                     AND assignment.inventory_owner_id = inventory_owner.id
                     AND assignment.deleted IS NULL
                  WHERE facility.tenant_id = audit_waves.tenant_id
                    AND facility.id = audit_waves.facility_id
                    AND facility.deleted IS NULL
              )
          )
          AND (
              $1::TIMESTAMPTZ IS NOT NULL
              OR NOT EXISTS (
                  SELECT 1
                  FROM audit_location_counts count
                  LEFT JOIN locations location
                      ON location.tenant_id = count.tenant_id
                     AND location.facility_id = count.facility_id
                     AND location.id = count.location_id
                     AND location.deleted IS NULL
                  LEFT JOIN items item
                      ON item.tenant_id = count.tenant_id
                     AND item.id = count.item_id
                     AND item.deleted IS NULL
                  LEFT JOIN inventory_owner_items owner_item
                      ON owner_item.tenant_id = count.tenant_id
                     AND owner_item.inventory_owner_id = count.inventory_owner_id
                     AND owner_item.item_id = count.item_id
                     AND owner_item.deleted IS NULL
                  WHERE count.tenant_id = audit_waves.tenant_id
                    AND count.audit_id = audit_waves.id
                    AND count.deleted IS NULL
                    AND (
                        location.id IS NULL
                        OR item.id IS NULL
                        OR owner_item.item_id IS NULL
                    )
              )
          )
        "#,
    )
    .bind(if deleted { Some(now_iso()) } else { None })
    .bind(access.tenant_id.get())
    .bind(id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut *tx)
    .await?;
    let changed = result.rows_affected() > 0;
    tx.commit().await?;
    Ok(changed)
}

pub async fn get_location_counts(
    db: &Db,
    access: &TenantAccess,
    audit_id: i64,
) -> AppResult<Vec<AuditLocationCount>> {
    let scope = ScopeBindings::for_access(access);
    let rows = sqlx::query(
        r#"
        SELECT count.id, count.tenant_id, count.created, count.deleted, count.started,
               count.ended, count.audit_id, count.inventory_owner_id, count.facility_id,
               count.location_id, count.item_id, count.uom, count.lot, count.expiration, count.serial,
               count.on_hand, count.count, count.revision, count.approval_status
        FROM audit_location_counts count
        INNER JOIN audit_waves wave
            ON wave.tenant_id = count.tenant_id
           AND wave.inventory_owner_id = count.inventory_owner_id
           AND wave.facility_id = count.facility_id
           AND wave.id = count.audit_id
        WHERE count.tenant_id = $1
          AND count.audit_id = $2
          AND count.deleted IS NULL
          AND wave.deleted IS NULL
          AND ($3 OR count.facility_id = ANY($4))
          AND ($5 OR count.inventory_owner_id = ANY($6))
        ORDER BY count.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(audit_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(db)
    .await?;
    rows.iter().map(map_count).collect()
}

pub async fn add_location_count(
    db: &Db,
    access: &TenantAccess,
    count: &AddAuditLocationCount,
) -> AppResult<Option<i64>> {
    let mut tx = db.begin().await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let dependencies = sqlx::query_as::<_, (i64, i64, bool)>(
        r#"
        SELECT wave.id, wave.inventory_owner_id, item.packaging_unit = $9 AS current_uom
        FROM audit_waves wave
        INNER JOIN facilities facility
            ON facility.tenant_id = wave.tenant_id
           AND facility.id = wave.facility_id
           AND facility.deleted IS NULL
        INNER JOIN inventory_owners inventory_owner
            ON inventory_owner.tenant_id = wave.tenant_id
           AND inventory_owner.id = wave.inventory_owner_id
           AND inventory_owner.deleted IS NULL
        INNER JOIN inventory_owner_facilities owner_facility
            ON owner_facility.tenant_id = wave.tenant_id
           AND owner_facility.inventory_owner_id = wave.inventory_owner_id
           AND owner_facility.facility_id = wave.facility_id
           AND owner_facility.deleted IS NULL
        INNER JOIN locations location
            ON location.tenant_id = wave.tenant_id
           AND location.facility_id = wave.facility_id
           AND location.id = $3
           AND location.deleted IS NULL
        INNER JOIN items item
            ON item.tenant_id = wave.tenant_id
           AND item.id = $4
           AND item.deleted IS NULL
        INNER JOIN inventory_owner_items owner_item
            ON owner_item.tenant_id = wave.tenant_id
           AND owner_item.inventory_owner_id = wave.inventory_owner_id
           AND owner_item.item_id = item.id
           AND owner_item.deleted IS NULL
        WHERE wave.tenant_id = $1
          AND wave.id = $2
          AND wave.deleted IS NULL
          AND ($5 OR wave.facility_id = ANY($6))
          AND ($7 OR wave.inventory_owner_id = ANY($8))
          AND (
              item.packaging_unit = $9
              OR EXISTS (
                  SELECT 1
                  FROM item_batches batch
                  WHERE batch.tenant_id = wave.tenant_id
                    AND batch.inventory_owner_id = wave.inventory_owner_id
                    AND batch.item_id = item.id
                    AND batch.uom = $9
                    AND batch.deleted IS NULL
              )
          )
        FOR UPDATE OF wave
        FOR SHARE OF facility, inventory_owner, owner_facility, location, item, owner_item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(count.audit_wave_id)
    .bind(count.location_id)
    .bind(count.item_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(&count.uom)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((_, inventory_owner_id, current_uom)) = dependencies else {
        return Ok(None);
    };
    if !current_uom {
        let batch: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT id
            FROM item_batches
            WHERE tenant_id = $1
              AND inventory_owner_id = $2
              AND item_id = $3
              AND uom = $4
              AND deleted IS NULL
            LIMIT 1
            FOR SHARE
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(count.item_id)
        .bind(&count.uom)
        .fetch_optional(&mut *tx)
        .await?;
        if batch.is_none() {
            return Ok(None);
        }
    }
    let id = sqlx::query_scalar(
        r#"
        INSERT INTO audit_location_counts
            (tenant_id, created, audit_id, inventory_owner_id, facility_id, location_id,
             item_id, uom, lot, expiration, serial, on_hand, count, approval_status)
        SELECT wave.tenant_id, $3, wave.id, wave.inventory_owner_id, wave.facility_id,
               location.id, item.id, $6, $7, $8, $9,
               COALESCE((
                   SELECT SUM(balance.qty_on_hand)::BIGINT
                   FROM inventory_balances balance
                   INNER JOIN item_batches batch
                       ON batch.tenant_id = balance.tenant_id
                      AND batch.inventory_owner_id = balance.inventory_owner_id
                      AND batch.id = balance.item_batch_id
                   WHERE balance.tenant_id = wave.tenant_id
                     AND balance.inventory_owner_id = wave.inventory_owner_id
                     AND balance.facility_id = wave.facility_id
                     AND balance.location_id = location.id
                     AND balance.item_id = item.id
                     AND balance.uom = $6
                     AND balance.deleted IS NULL
                     AND batch.deleted IS NULL
                     AND batch.lot IS NOT DISTINCT FROM $7
                     AND batch.expiration IS NOT DISTINCT FROM $8
                     AND batch.serial IS NOT DISTINCT FROM $9
               ), 0),
               $10, 'pending'
        FROM audit_waves wave
        INNER JOIN facilities facility
            ON facility.tenant_id = wave.tenant_id
           AND facility.id = wave.facility_id
           AND facility.deleted IS NULL
        INNER JOIN inventory_owners inventory_owner
            ON inventory_owner.tenant_id = wave.tenant_id
           AND inventory_owner.id = wave.inventory_owner_id
           AND inventory_owner.deleted IS NULL
        INNER JOIN inventory_owner_facilities owner_facility
            ON owner_facility.tenant_id = wave.tenant_id
           AND owner_facility.inventory_owner_id = wave.inventory_owner_id
           AND owner_facility.facility_id = wave.facility_id
           AND owner_facility.deleted IS NULL
        INNER JOIN locations location
            ON location.tenant_id = wave.tenant_id
           AND location.facility_id = wave.facility_id
           AND location.id = $4
           AND location.deleted IS NULL
        INNER JOIN items item
            ON item.tenant_id = wave.tenant_id
           AND item.id = $5
           AND item.deleted IS NULL
        INNER JOIN inventory_owner_items owner_item
            ON owner_item.tenant_id = wave.tenant_id
           AND owner_item.inventory_owner_id = wave.inventory_owner_id
           AND owner_item.item_id = item.id
           AND owner_item.deleted IS NULL
        WHERE wave.tenant_id = $1
          AND wave.id = $2
          AND wave.deleted IS NULL
          AND (
              item.packaging_unit = $6
              OR EXISTS (
                  SELECT 1
                  FROM item_batches batch
                  WHERE batch.tenant_id = wave.tenant_id
                    AND batch.inventory_owner_id = wave.inventory_owner_id
                    AND batch.item_id = item.id
                    AND batch.uom = $6
                    AND batch.deleted IS NULL
              )
          )
          AND ($11 OR wave.facility_id = ANY($12))
          AND ($13 OR wave.inventory_owner_id = ANY($14))
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(count.audit_wave_id)
    .bind(now_iso())
    .bind(count.location_id)
    .bind(count.item_id)
    .bind(&count.uom)
    .bind(&count.lot)
    .bind(count.expiration)
    .bind(&count.serial)
    .bind(count.count)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn update_location_count(
    db: &Db,
    access: &TenantAccess,
    update: &AuditLocationCountUpdate,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    if !lock_count_dependencies_tx(&mut tx, access.tenant_id, update.audit_location_count_id)
        .await?
    {
        return Ok(false);
    }
    let result = sqlx::query(
        r#"
        UPDATE audit_location_counts count
        SET count = $1, revision = count.revision + 1
        FROM audit_waves wave
        INNER JOIN facilities facility
            ON facility.tenant_id = wave.tenant_id
           AND facility.id = wave.facility_id
           AND facility.deleted IS NULL
        INNER JOIN inventory_owners inventory_owner
            ON inventory_owner.tenant_id = wave.tenant_id
           AND inventory_owner.id = wave.inventory_owner_id
           AND inventory_owner.deleted IS NULL
        INNER JOIN inventory_owner_facilities owner_facility
            ON owner_facility.tenant_id = wave.tenant_id
           AND owner_facility.inventory_owner_id = wave.inventory_owner_id
           AND owner_facility.facility_id = wave.facility_id
           AND owner_facility.deleted IS NULL
        WHERE count.tenant_id = $3
          AND count.id = $4
          AND count.audit_id = wave.id
          AND count.tenant_id = wave.tenant_id
          AND count.inventory_owner_id = wave.inventory_owner_id
          AND count.facility_id = wave.facility_id
          AND count.deleted IS NULL
          AND wave.deleted IS NULL
          AND EXISTS (
              SELECT 1
              FROM locations location
              INNER JOIN items item
                  ON item.tenant_id = location.tenant_id
                 AND item.id = count.item_id
                 AND item.deleted IS NULL
              INNER JOIN inventory_owner_items owner_item
                  ON owner_item.tenant_id = item.tenant_id
                 AND owner_item.inventory_owner_id = count.inventory_owner_id
                 AND owner_item.item_id = item.id
                 AND owner_item.deleted IS NULL
              WHERE location.tenant_id = count.tenant_id
                AND location.facility_id = count.facility_id
                AND location.id = count.location_id
                AND location.deleted IS NULL
          )
          AND count.approval_status = 'pending'
          AND count.revision = $2
          AND ($5 OR count.facility_id = ANY($6))
          AND ($7 OR count.inventory_owner_id = ANY($8))
        "#,
    )
    .bind(update.count)
    .bind(update.expected_revision)
    .bind(access.tenant_id.get())
    .bind(update.audit_location_count_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut *tx)
    .await?;
    let changed = result.rows_affected() > 0;
    tx.commit().await?;
    Ok(changed)
}

pub async fn set_location_count_deleted(
    db: &Db,
    access: &TenantAccess,
    id: i64,
    expected_revision: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id, deleted
        FROM audit_location_counts
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let facility_id: i64 = row.try_get("facility_id")?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let current_deleted: Option<wareboxes_core::models::Timestamp> = row.try_get("deleted")?;
    if (!scope.all_facilities && !scope.facility_ids.contains(&facility_id))
        || (!scope.all_inventory_owners && !scope.inventory_owner_ids.contains(&inventory_owner_id))
        || (deleted && current_deleted.is_some())
        || (!deleted && current_deleted.is_none())
    {
        return Ok(false);
    }
    if !deleted && !lock_count_dependencies_tx(&mut tx, access.tenant_id, id).await? {
        return Ok(false);
    }
    let result = sqlx::query(
        r#"
        UPDATE audit_location_counts count
        SET deleted = $1, revision = count.revision + 1
        WHERE count.revision = $2
          AND count.tenant_id = $3
          AND count.id = $4
          AND count.approval_status = 'pending'
          AND ($5 OR count.facility_id = ANY($6))
          AND ($7 OR count.inventory_owner_id = ANY($8))
          AND (
              $1::TIMESTAMPTZ IS NOT NULL
              OR EXISTS (
                  SELECT 1
                  FROM audit_waves wave
                  INNER JOIN facilities facility
                      ON facility.tenant_id = wave.tenant_id
                     AND facility.id = wave.facility_id
                     AND facility.deleted IS NULL
                  INNER JOIN inventory_owners inventory_owner
                      ON inventory_owner.tenant_id = wave.tenant_id
                     AND inventory_owner.id = wave.inventory_owner_id
                     AND inventory_owner.deleted IS NULL
                  INNER JOIN inventory_owner_facilities assignment
                      ON assignment.tenant_id = wave.tenant_id
                     AND assignment.facility_id = wave.facility_id
                     AND assignment.inventory_owner_id = wave.inventory_owner_id
                     AND assignment.deleted IS NULL
                  INNER JOIN locations location
                      ON location.tenant_id = count.tenant_id
                     AND location.facility_id = count.facility_id
                     AND location.id = count.location_id
                     AND location.deleted IS NULL
                  INNER JOIN items item
                      ON item.tenant_id = count.tenant_id
                     AND item.id = count.item_id
                     AND item.deleted IS NULL
                  INNER JOIN inventory_owner_items owner_item
                      ON owner_item.tenant_id = count.tenant_id
                     AND owner_item.inventory_owner_id = count.inventory_owner_id
                     AND owner_item.item_id = count.item_id
                     AND owner_item.deleted IS NULL
                  WHERE wave.tenant_id = count.tenant_id
                    AND wave.id = count.audit_id
                    AND wave.deleted IS NULL
              )
          )
        "#,
    )
    .bind(if deleted { Some(now_iso()) } else { None })
    .bind(expected_revision)
    .bind(access.tenant_id.get())
    .bind(id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut *tx)
    .await?;
    let changed = result.rows_affected() > 0;
    tx.commit().await?;
    Ok(changed)
}
