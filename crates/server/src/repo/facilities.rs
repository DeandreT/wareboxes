//! Ported from `app/utils/locations.ts` (`getFacilities`). The original app
//! only listed facilities (no create/update); kept faithful here.

use sqlx::Row;
use wareboxes_core::models::{Facility, SiteScope};
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::AppResult;

fn map(row: &sqlx::postgres::PgRow) -> AppResult<Facility> {
    Ok(Facility {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| crate::error::AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        address_id: row.try_get("address_id")?,
    })
}

pub async fn get_facilities(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<Facility>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, name, address_id
        FROM facilities
        WHERE tenant_id = $1 AND ($2 OR deleted IS NULL)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(db)
    .await?;
    rows.iter().map(map).collect()
}

pub async fn get_facilities_in_scope(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    show_deleted: bool,
) -> AppResult<Vec<Facility>> {
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, name, address_id
        FROM facilities
        WHERE tenant_id = $1
          AND ($2 OR deleted IS NULL)
          AND ($3 OR id = ANY($4))
        ORDER BY id
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

pub async fn active_facility_exists(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM facilities WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

pub async fn active_facility_exists_in_scope(
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
            FROM facilities
            WHERE tenant_id = $1
              AND id = $2
              AND deleted IS NULL
              AND ($3 OR id = ANY($4))
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

/// Not part of the original app; provided so the data set is usable for
/// testing and future inventory owner to facility linking.
pub async fn add_facility(db: &Db, tenant_id: TenantId, name: &str) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO facilities (tenant_id, name, created) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(name)
    .bind(now_iso())
    .fetch_one(db)
    .await?;
    Ok(id)
}
