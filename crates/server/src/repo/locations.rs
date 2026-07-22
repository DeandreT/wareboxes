//! Warehouse location hierarchy. Locations cover zones, aisles, bins, pallets,
//! carts, docks, and virtual locations through `type` plus parent linkage.

use sqlx::Row;
use wareboxes_core::models::Location;
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

pub async fn get_locations(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<Location>> {
    let rows = sqlx::query(
        r#"
        SELECT l.id, l.tenant_id, l.created, l.deleted, l.warehouse_id,
               w.name AS warehouse_name, l.parent_location_id, l.barcode,
               l.name, l.type, l.active, l.pickable, l.receivable
        FROM locations l
        INNER JOIN warehouses w
            ON w.tenant_id = l.tenant_id AND w.id = l.warehouse_id
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
            (tenant_id, created, warehouse_id, parent_location_id, barcode, name, type, active, pickable, receivable)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
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
