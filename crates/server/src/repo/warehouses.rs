//! Ported from `app/utils/locations.ts` (`getWarehouses`). The original app
//! only listed warehouses (no create/update); kept faithful here.

use sqlx::Row;
use wareboxes_core::models::Warehouse;

use crate::db::{now_iso, Db};
use crate::error::AppResult;

fn map(row: &sqlx::postgres::PgRow) -> AppResult<Warehouse> {
    Ok(Warehouse {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        address_id: row.try_get("address_id")?,
    })
}

pub async fn get_warehouses(db: &Db, show_deleted: bool) -> AppResult<Vec<Warehouse>> {
    let sql = if show_deleted {
        "SELECT id, created, deleted, name, address_id FROM warehouses ORDER BY id"
    } else {
        "SELECT id, created, deleted, name, address_id FROM warehouses WHERE deleted IS NULL ORDER BY id"
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    rows.iter().map(map).collect()
}

pub async fn active_warehouse_exists(db: &Db, id: i64) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM warehouses WHERE id = $1 AND deleted IS NULL)",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

/// Not part of the original app; provided so the data set is usable for
/// testing and future account↔warehouse linking.
pub async fn add_warehouse(db: &Db, name: &str) -> AppResult<i64> {
    let id: i64 =
        sqlx::query_scalar("INSERT INTO warehouses (name, created) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(now_iso())
            .fetch_one(db)
            .await?;
    Ok(id)
}
