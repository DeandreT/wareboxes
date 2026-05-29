//! Warehouse location hierarchy. Locations cover zones, aisles, bins, pallets,
//! carts, docks, and virtual locations through `type` plus parent linkage.

use sqlx::Row;
use wareboxes_core::models::Location;

use crate::db::{now_iso, Db};
use crate::error::AppResult;

fn map(row: &sqlx::postgres::PgRow) -> AppResult<Location> {
    Ok(Location {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        warehouse_id: row.try_get("warehouse_id")?,
        warehouse_name: row.try_get("warehouse_name")?,
        parent_location_id: row.try_get("parent_location_id")?,
        barcode: row.try_get("barcode")?,
        name: row.try_get("name")?,
        r#type: row.try_get("type")?,
        active: row.try_get("active")?,
        pickable: row.try_get("pickable")?,
        receivable: row.try_get("receivable")?,
    })
}

pub async fn get_locations(db: &Db, show_deleted: bool) -> AppResult<Vec<Location>> {
    let sql = if show_deleted {
        r#"
        SELECT l.id, l.created, l.deleted, l.warehouse_id, w.name AS warehouse_name,
               l.parent_location_id, l.barcode, l.name, l.type, l.active, l.pickable, l.receivable
        FROM locations l
        LEFT JOIN warehouses w ON w.id = l.warehouse_id
        ORDER BY l.id
        "#
    } else {
        r#"
        SELECT l.id, l.created, l.deleted, l.warehouse_id, w.name AS warehouse_name,
               l.parent_location_id, l.barcode, l.name, l.type, l.active, l.pickable, l.receivable
        FROM locations l
        LEFT JOIN warehouses w ON w.id = l.warehouse_id
        WHERE l.deleted IS NULL
        ORDER BY l.id
        "#
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    rows.iter().map(map).collect()
}

pub async fn active_location_exists(db: &Db, id: i64) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM locations WHERE id = $1 AND deleted IS NULL)",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

pub async fn location_active_state(db: &Db, id: i64) -> AppResult<Option<bool>> {
    let active =
        sqlx::query_scalar("SELECT active FROM locations WHERE id = $1 AND deleted IS NULL")
            .bind(id)
            .fetch_optional(db)
            .await?;
    Ok(active)
}

#[allow(clippy::too_many_arguments)]
pub async fn add_location(
    db: &Db,
    warehouse_id: i64,
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
            (created, warehouse_id, parent_location_id, barcode, name, type, active, pickable, receivable)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id
        "#,
    )
    .bind(now_iso())
    .bind(warehouse_id)
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
        WHERE id = $8
        "#,
    )
    .bind(parent_location_id)
    .bind(barcode)
    .bind(name)
    .bind(location_type)
    .bind(active)
    .bind(pickable)
    .bind(receivable)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_location_deleted(db: &Db, id: i64, deleted: bool) -> AppResult<bool> {
    let res = sqlx::query("UPDATE locations SET deleted = $1 WHERE id = $2")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
