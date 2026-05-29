//! Ported from `app/utils/types/db/items.ts` (items + dims + skus + barcodes).

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_core::models::{Barcode, Item, Sku};

use crate::db::{now_iso, Db};
use crate::error::AppResult;

fn map_item(row: &sqlx::postgres::PgRow) -> AppResult<Item> {
    Ok(Item {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        description: row.try_get("description")?,
        notes: row.try_get("notes")?,
        packaging_unit: row.try_get("packaging_unit")?,
        dims_id: row.try_get("dims_id")?,
        pallet_hi: row.try_get("pallet_hi")?,
        pallet_ti: row.try_get("pallet_ti")?,
        inner_units: row.try_get("inner_units")?,
        skus: Vec::new(),
        barcodes: Vec::new(),
    })
}

async fn skus_by_item(db: &Db) -> AppResult<HashMap<i64, Vec<Sku>>> {
    let rows = sqlx::query(
        "SELECT id, created, deleted, name, item_id, notes FROM skus WHERE deleted IS NULL",
    )
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<Sku>> = HashMap::new();
    for r in &rows {
        let item_id = r.try_get("item_id")?;
        map.entry(item_id).or_default().push(Sku {
            id: r.try_get("id")?,
            created: r.try_get("created")?,
            deleted: r.try_get("deleted")?,
            name: r.try_get("name")?,
            item_id,
            notes: r.try_get("notes")?,
        });
    }
    Ok(map)
}

async fn barcodes_by_item(db: &Db) -> AppResult<HashMap<i64, Vec<Barcode>>> {
    let rows = sqlx::query(
        "SELECT id, created, deleted, name, type, item_id, notes FROM barcodes WHERE deleted IS NULL",
    )
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<Barcode>> = HashMap::new();
    for r in &rows {
        let item_id = r.try_get("item_id")?;
        map.entry(item_id).or_default().push(Barcode {
            id: r.try_get("id")?,
            created: r.try_get("created")?,
            deleted: r.try_get("deleted")?,
            name: r.try_get("name")?,
            r#type: r.try_get("type")?,
            item_id,
            notes: r.try_get("notes")?,
        });
    }
    Ok(map)
}

pub async fn get_items(db: &Db, show_deleted: bool) -> AppResult<Vec<Item>> {
    let sql = if show_deleted {
        "SELECT id, created, deleted, description, notes, packaging_unit, dims_id, pallet_hi, pallet_ti, inner_units FROM items ORDER BY id"
    } else {
        "SELECT id, created, deleted, description, notes, packaging_unit, dims_id, pallet_hi, pallet_ti, inner_units FROM items WHERE deleted IS NULL ORDER BY id"
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    let mut skus = skus_by_item(db).await?;
    let mut barcodes = barcodes_by_item(db).await?;
    rows.iter()
        .map(|r| {
            let mut it = map_item(r)?;
            it.skus = skus.remove(&it.id).unwrap_or_default();
            it.barcodes = barcodes.remove(&it.id).unwrap_or_default();
            Ok(it)
        })
        .collect()
}

pub async fn active_item_exists(db: &Db, id: i64) -> AppResult<bool> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items WHERE id = $1 AND deleted IS NULL)")
            .bind(id)
            .fetch_one(db)
            .await?;
    Ok(exists)
}

#[allow(clippy::too_many_arguments)]
pub async fn add_item(
    db: &Db,
    description: &str,
    notes: Option<&str>,
    packaging_unit: &str,
    length: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
    length_uom: Option<&str>,
    weight: Option<i64>,
    weight_uom: Option<&str>,
) -> AppResult<i64> {
    let now = now_iso();
    let dims_id: i64 = sqlx::query_scalar(
        "INSERT INTO dims (created, length, width, height, length_uom, weight, weight_uom) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(&now)
    .bind(length)
    .bind(width)
    .bind(height)
    .bind(length_uom)
    .bind(weight)
    .bind(weight_uom)
    .fetch_one(db)
    .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO items (created, description, notes, packaging_unit, dims_id) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(&now)
    .bind(description)
    .bind(notes)
    .bind(packaging_unit)
    .bind(dims_id)
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn update_item(
    db: &Db,
    id: i64,
    description: Option<&str>,
    notes: Option<&str>,
    packaging_unit: Option<&str>,
) -> AppResult<bool> {
    let res = sqlx::query(
        r#"
        UPDATE items
        SET description = COALESCE($1, description),
            notes = COALESCE($2, notes),
            packaging_unit = COALESCE($3, packaging_unit)
        WHERE id = $4
        "#,
    )
    .bind(description)
    .bind(notes)
    .bind(packaging_unit)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_item_deleted(db: &Db, id: i64, deleted: bool) -> AppResult<bool> {
    let res = sqlx::query("UPDATE items SET deleted = $1 WHERE id = $2")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn add_sku(db: &Db, item_id: i64, name: &str, notes: Option<&str>) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO skus (created, name, item_id, notes) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(now_iso())
    .bind(name)
    .bind(item_id)
    .bind(notes)
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn add_barcode(
    db: &Db,
    item_id: i64,
    name: &str,
    barcode_type: &str,
    notes: Option<&str>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO barcodes (created, name, type, item_id, notes) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(now_iso())
    .bind(name)
    .bind(barcode_type)
    .bind(item_id)
    .bind(notes)
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn active_barcode_item_by_name(db: &Db, name: &str) -> AppResult<Option<i64>> {
    let item_id = sqlx::query_scalar(
        "SELECT item_id FROM barcodes WHERE deleted IS NULL AND lower(name) = lower($1) LIMIT 1",
    )
    .bind(name)
    .fetch_optional(db)
    .await?;
    Ok(item_id)
}

pub async fn set_barcode_deleted(db: &Db, id: i64, deleted: bool) -> AppResult<bool> {
    let res = sqlx::query("UPDATE barcodes SET deleted = $1 WHERE id = $2")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
