//! Ported from `app/utils/locations.ts` (`getWarehouses`). The original app
//! only listed warehouses (no create/update); kept faithful here.

use sqlx::Row;
use wareboxes_core::models::Warehouse;
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::AppResult;

fn map(row: &sqlx::postgres::PgRow) -> AppResult<Warehouse> {
    Ok(Warehouse {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| crate::error::AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        address_id: row.try_get("address_id")?,
    })
}

pub async fn get_warehouses(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<Warehouse>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, name, address_id
        FROM warehouses
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

pub async fn active_warehouse_exists(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM warehouses WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

/// Not part of the original app; provided so the data set is usable for
/// testing and future account↔warehouse linking.
pub async fn add_warehouse(db: &Db, tenant_id: TenantId, name: &str) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO warehouses (tenant_id, name, created) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(name)
    .bind(now_iso())
    .fetch_one(db)
    .await?;
    Ok(id)
}
