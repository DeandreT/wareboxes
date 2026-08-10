//! Ported from `app/utils/types/db/items.ts` (items + dims + skus + barcodes).

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_core::models::{Barcode, InventoryOwnerItem, Item, ItemPackLink, Sku};
use wareboxes_domain::{OwnerScope, TenantId};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};

fn map_tenant_id(row: &sqlx::postgres::PgRow) -> AppResult<TenantId> {
    TenantId::new(row.try_get("tenant_id")?)
        .map_err(|error| crate::error::AppError::internal(error.to_string()))
}

fn map_item(row: &sqlx::postgres::PgRow) -> AppResult<Item> {
    Ok(Item {
        id: row.try_get("id")?,
        tenant_id: map_tenant_id(row)?,
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

async fn skus_by_item(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
) -> AppResult<HashMap<i64, Vec<Sku>>> {
    let rows = sqlx::query(
        "SELECT id, tenant_id, created, deleted, name, item_id, notes FROM skus WHERE tenant_id = $1 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let mut map: HashMap<i64, Vec<Sku>> = HashMap::new();
    for r in &rows {
        let item_id = r.try_get("item_id")?;
        map.entry(item_id).or_default().push(Sku {
            id: r.try_get("id")?,
            tenant_id: map_tenant_id(r)?,
            created: r.try_get("created")?,
            deleted: r.try_get("deleted")?,
            name: r.try_get("name")?,
            item_id,
            notes: r.try_get("notes")?,
        });
    }
    Ok(map)
}

async fn barcodes_by_item(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
) -> AppResult<HashMap<i64, Vec<Barcode>>> {
    let rows = sqlx::query(
        "SELECT id, tenant_id, created, deleted, name, type, item_id, notes FROM barcodes WHERE tenant_id = $1 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let mut map: HashMap<i64, Vec<Barcode>> = HashMap::new();
    for r in &rows {
        let item_id = r.try_get("item_id")?;
        map.entry(item_id).or_default().push(Barcode {
            id: r.try_get("id")?,
            tenant_id: map_tenant_id(r)?,
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

pub async fn get_items(db: &Db, tenant_id: TenantId, show_deleted: bool) -> AppResult<Vec<Item>> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, description, notes, packaging_unit,
               dims_id, pallet_hi, pallet_ti, inner_units
        FROM items
        WHERE tenant_id = $1 AND ($2 OR deleted IS NULL)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(&mut *tx)
    .await?;
    let mut skus = skus_by_item(&mut tx, tenant_id).await?;
    let mut barcodes = barcodes_by_item(&mut tx, tenant_id).await?;
    let items = rows
        .iter()
        .map(|r| {
            let mut it = map_item(r)?;
            it.skus = skus.remove(&it.id).unwrap_or_default();
            it.barcodes = barcodes.remove(&it.id).unwrap_or_default();
            Ok(it)
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(items)
}

pub async fn active_item_exists(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM items WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(exists)
}

async fn lock_active_item_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    item_id: i64,
) -> AppResult<bool> {
    let item = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM items WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL FOR SHARE",
    )
    .bind(tenant_id.get())
    .bind(item_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(item.is_some())
}

fn map_item_pack_link(row: &sqlx::postgres::PgRow) -> AppResult<ItemPackLink> {
    Ok(ItemPackLink {
        id: row.try_get("id")?,
        tenant_id: map_tenant_id(row)?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        master_item_id: row.try_get("master_item_id")?,
        single_item_id: row.try_get("single_item_id")?,
        inner_qty: row.try_get("inner_qty")?,
        notes: row.try_get("notes")?,
    })
}

fn map_inventory_owner_item(row: &sqlx::postgres::PgRow) -> AppResult<InventoryOwnerItem> {
    Ok(InventoryOwnerItem {
        id: row.try_get("id")?,
        tenant_id: map_tenant_id(row)?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        item_id: row.try_get("item_id")?,
    })
}

pub async fn get_inventory_owner_items_in_scope(
    db: &Db,
    tenant_id: TenantId,
    owner_scope: &OwnerScope,
    show_deleted: bool,
) -> AppResult<Vec<InventoryOwnerItem>> {
    let owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, inventory_owner_id, item_id
        FROM inventory_owner_items
        WHERE tenant_id=$1
          AND ($2 OR deleted IS NULL)
          AND ($3 OR inventory_owner_id=ANY($4))
        ORDER BY inventory_owner_id, item_id, id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .bind(owner_scope.all_inventory_owners)
    .bind(&owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    let assignments = rows
        .iter()
        .map(map_inventory_owner_item)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(assignments)
}

pub async fn add_inventory_owner_item(
    db: &Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    item_id: i64,
) -> AppResult<InventoryOwnerItem> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let owner = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM inventory_owners WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL FOR SHARE",
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .fetch_optional(&mut *tx)
    .await?;
    if owner.is_none() {
        return Err(AppError::bad_request("inventory owner not found"));
    }
    if !lock_active_item_tx(&mut tx, tenant_id, item_id).await? {
        return Err(AppError::bad_request("item not found"));
    }

    let existing = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, inventory_owner_id, item_id
        FROM inventory_owner_items
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND item_id=$3
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(item_id)
    .fetch_optional(&mut *tx)
    .await?;
    let assignment = if let Some(row) = existing {
        if row
            .try_get::<Option<wareboxes_domain::Timestamp>, _>("deleted")?
            .is_none()
        {
            return Err(AppError::conflict(
                "inventory owner is already assigned to this item",
            ));
        }
        let row = sqlx::query(
            r#"
            UPDATE inventory_owner_items SET deleted=NULL
            WHERE tenant_id=$1 AND id=$2
            RETURNING id, tenant_id, created, deleted, inventory_owner_id, item_id
            "#,
        )
        .bind(tenant_id.get())
        .bind(row.try_get::<i64, _>("id")?)
        .fetch_one(&mut *tx)
        .await?;
        map_inventory_owner_item(&row)?
    } else {
        let row = sqlx::query(
            r#"
            INSERT INTO inventory_owner_items
                (tenant_id, created, inventory_owner_id, item_id)
            VALUES ($1,$2,$3,$4)
            RETURNING id, tenant_id, created, deleted, inventory_owner_id, item_id
            "#,
        )
        .bind(tenant_id.get())
        .bind(now_iso())
        .bind(inventory_owner_id)
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;
        map_inventory_owner_item(&row)?
    };
    tx.commit().await?;
    Ok(assignment)
}

pub async fn deactivate_inventory_owner_item_in_scope(
    db: &Db,
    tenant_id: TenantId,
    owner_scope: &OwnerScope,
    inventory_owner_item_id: i64,
) -> AppResult<bool> {
    let owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let assignment_id = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id
        FROM inventory_owner_items
        WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
          AND ($3 OR inventory_owner_id=ANY($4))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_item_id)
    .bind(owner_scope.all_inventory_owners)
    .bind(&owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(assignment_id) = assignment_id else {
        tx.rollback().await?;
        return Ok(false);
    };
    let result = sqlx::query(
        "UPDATE inventory_owner_items SET deleted=$1 WHERE tenant_id=$2 AND id=$3 AND deleted IS NULL",
    )
    .bind(now_iso())
    .bind(tenant_id.get())
    .bind(assignment_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}

pub async fn get_item_pack_links(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<ItemPackLink>> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, master_item_id, single_item_id, inner_qty, notes
        FROM item_pack_links
        WHERE tenant_id = $1 AND ($2 OR deleted IS NULL)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(&mut *tx)
    .await?;
    let links = rows
        .iter()
        .map(map_item_pack_link)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(links)
}

pub async fn add_item_pack_link(
    db: &Db,
    tenant_id: TenantId,
    master_item_id: i64,
    single_item_id: i64,
    inner_qty: i64,
    notes: Option<&str>,
) -> AppResult<i64> {
    if master_item_id == single_item_id {
        return Err(AppError::bad_request(
            "master item and single item must differ",
        ));
    }
    if inner_qty <= 1 {
        return Err(AppError::bad_request("inner quantity must be at least 2"));
    }
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('item-pack-graph:' || $1::TEXT, 0))",
    )
    .bind(tenant_id.get())
    .execute(&mut *tx)
    .await?;
    let item_ids = [master_item_id, single_item_id];
    let active_items = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id
        FROM items
        WHERE tenant_id = $1
          AND id = ANY($2)
          AND deleted IS NULL
        ORDER BY id
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(item_ids.as_slice())
    .fetch_all(&mut *tx)
    .await?;
    if active_items.len() != 2 {
        return Err(AppError::bad_request("item not found"));
    }
    let existing: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM item_pack_links
          WHERE tenant_id=$1 AND master_item_id=$2 AND single_item_id=$3 AND deleted IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(master_item_id)
    .bind(single_item_id)
    .fetch_one(&mut *tx)
    .await?;
    if existing {
        return Err(AppError::conflict(
            "an active pack conversion already exists for these items",
        ));
    }
    let creates_cycle: bool = sqlx::query_scalar(
        r#"
        WITH RECURSIVE descendants(item_id) AS (
          SELECT link.single_item_id
          FROM item_pack_links link
          WHERE link.tenant_id=$1 AND link.master_item_id=$2 AND link.deleted IS NULL
          UNION
          SELECT link.single_item_id
          FROM item_pack_links link
          JOIN descendants current ON current.item_id=link.master_item_id
          WHERE link.tenant_id=$1 AND link.deleted IS NULL
        )
        SELECT EXISTS(SELECT 1 FROM descendants WHERE item_id=$3)
        "#,
    )
    .bind(tenant_id.get())
    .bind(single_item_id)
    .bind(master_item_id)
    .fetch_one(&mut *tx)
    .await?;
    if creates_cycle {
        return Err(AppError::conflict(
            "pack conversion would create a circular item hierarchy",
        ));
    }
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO item_pack_links (tenant_id, created, master_item_id, single_item_id, inner_qty, notes)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(master_item_id)
    .bind(single_item_id)
    .bind(inner_qty)
    .bind(notes)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn set_item_pack_link_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let res =
        sqlx::query("UPDATE item_pack_links SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(if deleted { Some(now_iso()) } else { None })
            .bind(tenant_id.get())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

#[allow(clippy::too_many_arguments)]
pub async fn add_item(
    db: &Db,
    tenant_id: TenantId,
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
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let dims_id: i64 = sqlx::query_scalar(
        "INSERT INTO dims (tenant_id, created, length, width, height, length_uom, weight, weight_uom) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(now)
    .bind(length)
    .bind(width)
    .bind(height)
    .bind(length_uom)
    .bind(weight)
    .bind(weight_uom)
    .fetch_one(&mut *tx)
    .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO items (tenant_id, created, description, notes, packaging_unit, dims_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(now)
    .bind(description)
    .bind(notes)
    .bind(packaging_unit)
    .bind(dims_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn update_item(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    description: Option<&str>,
    notes: Option<&str>,
    packaging_unit: Option<&str>,
) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let res = sqlx::query(
        r#"
        UPDATE items
        SET description = COALESCE($1, description),
            notes = COALESCE($2, notes),
            packaging_unit = COALESCE($3, packaging_unit)
        WHERE tenant_id = $4 AND id = $5
        "#,
    )
    .bind(description)
    .bind(notes)
    .bind(packaging_unit)
    .bind(tenant_id.get())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_item_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let res = sqlx::query("UPDATE items SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(tenant_id.get())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn add_sku(
    db: &Db,
    tenant_id: TenantId,
    item_id: i64,
    name: &str,
    notes: Option<&str>,
) -> AppResult<i64> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    if !lock_active_item_tx(&mut tx, tenant_id, item_id).await? {
        return Err(AppError::bad_request("item not found"));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO skus (tenant_id, created, name, item_id, notes) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(name)
    .bind(item_id)
    .bind(notes)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn add_barcode(
    db: &Db,
    tenant_id: TenantId,
    item_id: i64,
    name: &str,
    barcode_type: &str,
    notes: Option<&str>,
) -> AppResult<i64> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    if !lock_active_item_tx(&mut tx, tenant_id, item_id).await? {
        return Err(AppError::bad_request("item not found"));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO barcodes (tenant_id, created, name, type, item_id, notes) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(name)
    .bind(barcode_type)
    .bind(item_id)
    .bind(notes)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn active_barcode_item_by_name(
    db: &Db,
    tenant_id: TenantId,
    name: &str,
) -> AppResult<Option<i64>> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let item_id = sqlx::query_scalar(
        "SELECT item_id FROM barcodes WHERE tenant_id = $1 AND deleted IS NULL AND lower(name) = lower($2) LIMIT 1",
    )
    .bind(tenant_id.get())
    .bind(name)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(item_id)
}

pub async fn set_barcode_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let res = sqlx::query("UPDATE barcodes SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(tenant_id.get())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}
