//! Facility location hierarchy. Locations cover zones, aisles, bins, pallets,
//! carts, docks, and virtual locations through `type` plus parent linkage.

use sqlx::Row;
use wareboxes_core::models::{Location, SiteScope};
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::AppResult;

fn map(row: &sqlx::postgres::PgRow) -> AppResult<Location> {
    Ok(Location {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| crate::error::AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        parent_location_id: row.try_get("parent_location_id")?,
        barcode: row.try_get("barcode")?,
        name: row.try_get("name")?,
        r#type: row.try_get("type")?,
        active: row.try_get("active")?,
        pickable: row.try_get("pickable")?,
        receivable: row.try_get("receivable")?,
    })
}

pub async fn get_locations(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<Location>> {
    let rows = sqlx::query(
        r#"
        SELECT l.id, l.tenant_id, l.created, l.deleted, l.facility_id,
               w.name AS facility_name, l.parent_location_id, l.barcode,
               l.name, l.type, l.active, l.pickable, l.receivable
        FROM locations l
        INNER JOIN facilities w
            ON w.tenant_id = l.tenant_id AND w.id = l.facility_id
        WHERE l.tenant_id = $1 AND ($2 OR l.deleted IS NULL)
        ORDER BY l.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(db)
    .await?;
    rows.iter().map(map).collect()
}

pub async fn get_locations_in_scope(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    show_deleted: bool,
) -> AppResult<Vec<Location>> {
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT location.id, location.tenant_id, location.created, location.deleted,
               location.facility_id, facility.name AS facility_name,
               location.parent_location_id, location.barcode, location.name,
               location.type, location.active, location.pickable, location.receivable
        FROM locations location
        INNER JOIN facilities facility
            ON facility.tenant_id = location.tenant_id
           AND facility.id = location.facility_id
        WHERE location.tenant_id = $1
          AND ($2 OR location.deleted IS NULL)
          AND ($3 OR location.facility_id = ANY($4))
        ORDER BY location.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .fetch_all(db)
    .await?;
    rows.iter().map(map).collect()
}

pub async fn active_location_exists(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM locations WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

pub async fn active_location_exists_in_scope(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    id: i64,
) -> AppResult<bool> {
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|facility_id| facility_id.get())
        .collect::<Vec<_>>();
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM locations
            WHERE tenant_id = $1
              AND id = $2
              AND deleted IS NULL
              AND ($3 OR facility_id = ANY($4))
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(id)
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

pub async fn active_location_exists_in_facility(
    db: &Db,
    tenant_id: TenantId,
    facility_id: i64,
    id: i64,
) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM locations
            WHERE tenant_id = $1
              AND facility_id = $2
              AND id = $3
              AND deleted IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

pub async fn active_location_facility_in_scope(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    id: i64,
) -> AppResult<Option<i64>> {
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|facility_id| facility_id.get())
        .collect::<Vec<_>>();
    let facility_id = sqlx::query_scalar(
        r#"
        SELECT facility_id
        FROM locations
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
          AND ($3 OR facility_id = ANY($4))
        "#,
    )
    .bind(tenant_id.get())
    .bind(id)
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .fetch_optional(db)
    .await?;
    Ok(facility_id)
}

pub async fn location_active_state(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
) -> AppResult<Option<bool>> {
    let active = sqlx::query_scalar(
        "SELECT active FROM locations WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(active)
}

#[allow(clippy::too_many_arguments)]
pub async fn add_location(
    db: &Db,
    tenant_id: TenantId,
    facility_id: i64,
    parent_location_id: Option<i64>,
    barcode: Option<&str>,
    name: Option<&str>,
    location_type: &str,
    active: bool,
    pickable: bool,
    receivable: bool,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO locations
            (tenant_id, created, facility_id, parent_location_id, barcode, name, type, active, pickable, receivable)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(facility_id)
    .bind(parent_location_id)
    .bind(barcode)
    .bind(name)
    .bind(location_type)
    .bind(active)
    .bind(pickable)
    .bind(receivable)
    .fetch_one(db)
    .await?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_location(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    parent_location_id: Option<i64>,
    barcode: Option<&str>,
    name: Option<&str>,
    location_type: Option<&str>,
    active: Option<bool>,
    pickable: Option<bool>,
    receivable: Option<bool>,
) -> AppResult<bool> {
    let res = sqlx::query(
        r#"
        UPDATE locations
        SET parent_location_id = COALESCE($1, parent_location_id),
            barcode = COALESCE($2, barcode),
            name = COALESCE($3, name),
            type = COALESCE($4, type),
            active = COALESCE($5, active),
            pickable = COALESCE($6, pickable),
            receivable = COALESCE($7, receivable)
        WHERE tenant_id = $8 AND id = $9
        "#,
    )
    .bind(parent_location_id)
    .bind(barcode)
    .bind(name)
    .bind(location_type)
    .bind(active)
    .bind(pickable)
    .bind(receivable)
    .bind(tenant_id.get())
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_location_in_scope(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    id: i64,
    parent_location_id: Option<i64>,
    barcode: Option<&str>,
    name: Option<&str>,
    location_type: Option<&str>,
    active: Option<bool>,
    pickable: Option<bool>,
    receivable: Option<bool>,
) -> AppResult<bool> {
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|facility_id| facility_id.get())
        .collect::<Vec<_>>();
    let result = sqlx::query(
        r#"
        UPDATE locations
        SET parent_location_id = COALESCE($1, parent_location_id),
            barcode = COALESCE($2, barcode),
            name = COALESCE($3, name),
            type = COALESCE($4, type),
            active = COALESCE($5, active),
            pickable = COALESCE($6, pickable),
            receivable = COALESCE($7, receivable)
        WHERE tenant_id = $8
          AND id = $9
          AND ($10 OR facility_id = ANY($11))
        "#,
    )
    .bind(parent_location_id)
    .bind(barcode)
    .bind(name)
    .bind(location_type)
    .bind(active)
    .bind(pickable)
    .bind(receivable)
    .bind(tenant_id.get())
    .bind(id)
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_location_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let res = sqlx::query("UPDATE locations SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(tenant_id.get())
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_location_deleted_in_scope(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|facility_id| facility_id.get())
        .collect::<Vec<_>>();
    let result = sqlx::query(
        r#"
        UPDATE locations
        SET deleted = $1
        WHERE tenant_id = $2
          AND id = $3
          AND ($4 OR facility_id = ANY($5))
        "#,
    )
    .bind(if deleted { Some(now_iso()) } else { None })
    .bind(tenant_id.get())
    .bind(id)
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
