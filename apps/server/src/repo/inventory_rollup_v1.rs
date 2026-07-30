//! Scope-safe aggregate inventory reads for the version 1 API.

use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_core::models::TenantAccess;

use crate::db::{begin_tenant_transaction, Db};
use crate::error::AppResult;
use crate::repo::access::ScopeBindings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocationRollupCursor {
    pub inventory_owner_id: i64,
    pub item_id: i64,
    pub location_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FacilityRollupCursor {
    pub inventory_owner_id: i64,
    pub item_id: i64,
    pub facility_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemRollupCursor {
    pub inventory_owner_id: i64,
    pub item_id: i64,
}

#[derive(Debug)]
pub struct InventoryRollupQuantityColumns {
    pub uoms: Vec<String>,
    pub on_hand: Vec<i64>,
    pub reserved: Vec<i64>,
    pub held: Vec<i64>,
    pub available: Vec<i64>,
}

#[derive(Debug)]
pub struct InventoryLocationRollupRow {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub quantities: InventoryRollupQuantityColumns,
    pub balance_count: i64,
    pub batch_count: i64,
}

pub struct InventoryLocationRollupKeysetPage {
    pub rows: Vec<InventoryLocationRollupRow>,
    pub next_cursor: Option<LocationRollupCursor>,
}

#[derive(Debug)]
pub struct InventoryFacilityRollupRow {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub quantities: InventoryRollupQuantityColumns,
    pub balance_count: i64,
    pub batch_count: i64,
    pub location_count: i64,
}

pub struct InventoryFacilityRollupKeysetPage {
    pub rows: Vec<InventoryFacilityRollupRow>,
    pub next_cursor: Option<FacilityRollupCursor>,
}

#[derive(Debug)]
pub struct InventoryItemRollupRow {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub quantities: InventoryRollupQuantityColumns,
    pub balance_count: i64,
    pub batch_count: i64,
    pub location_count: i64,
    pub facility_count: i64,
}

pub struct InventoryItemRollupKeysetPage {
    pub rows: Vec<InventoryItemRollupRow>,
    pub next_cursor: Option<ItemRollupCursor>,
}

pub async fn get_inventory_location_rollup_page(
    db: &Db,
    access: &TenantAccess,
    after: Option<LocationRollupCursor>,
    limit: u16,
) -> AppResult<InventoryLocationRollupKeysetPage> {
    let scope = ScopeBindings::for_access(access);
    let fetch_limit = i64::from(limit) + 1;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        WITH base AS (
            SELECT balance.inventory_owner_id, owner.name AS inventory_owner_name,
                   balance.item_id, item.description AS item_description,
                   sku.name AS primary_sku, balance.facility_id,
                   facility.name AS facility_name, balance.location_id,
                   location.name AS location_name, location.barcode AS location_barcode,
                   balance.item_batch_id, balance.uom, balance.status,
                   balance.qty_on_hand, balance.qty_reserved, balance.qty_held
            FROM inventory_balances balance
            INNER JOIN inventory_owners owner
                ON owner.tenant_id = balance.tenant_id
               AND owner.id = balance.inventory_owner_id
            INNER JOIN facilities facility
                ON facility.tenant_id = balance.tenant_id
               AND facility.id = balance.facility_id
            INNER JOIN locations location
                ON location.tenant_id = balance.tenant_id
               AND location.facility_id = balance.facility_id
               AND location.id = balance.location_id
            INNER JOIN items item
                ON item.tenant_id = balance.tenant_id
               AND item.id = balance.item_id
            LEFT JOIN LATERAL (
                SELECT item_sku.name
                FROM skus item_sku
                WHERE item_sku.tenant_id = balance.tenant_id
                  AND item_sku.item_id = balance.item_id
                  AND item_sku.deleted IS NULL
                ORDER BY item_sku.id
                LIMIT 1
            ) sku ON TRUE
            WHERE balance.tenant_id = $1
              AND balance.deleted IS NULL
              AND ($2 OR balance.facility_id = ANY($3))
              AND ($4 OR balance.inventory_owner_id = ANY($5))
              AND (
                    $6::BIGINT IS NULL
                    OR (balance.inventory_owner_id, balance.item_id, balance.location_id)
                       > ($6, $7, $8)
              )
        ),
        by_uom AS (
            SELECT inventory_owner_id, item_id, facility_id, location_id, uom,
                   SUM(qty_on_hand)::BIGINT AS on_hand,
                   SUM(qty_reserved)::BIGINT AS reserved,
                   SUM(qty_held)::BIGINT AS held,
                   SUM(
                       CASE WHEN status = 'available'
                           THEN qty_on_hand - qty_reserved - qty_held
                           ELSE 0
                       END
                   )::BIGINT AS available
            FROM base
            GROUP BY inventory_owner_id, item_id, facility_id, location_id, uom
        ),
        group_stats AS (
            SELECT inventory_owner_id, inventory_owner_name, item_id, item_description,
                   primary_sku, facility_id, facility_name, location_id, location_name,
                   location_barcode, COUNT(*)::BIGINT AS balance_count,
                   COUNT(DISTINCT item_batch_id)::BIGINT AS batch_count
            FROM base
            GROUP BY inventory_owner_id, inventory_owner_name, item_id, item_description,
                     primary_sku, facility_id, facility_name, location_id, location_name,
                     location_barcode
        )
        SELECT stats.inventory_owner_id, stats.inventory_owner_name,
               stats.item_id, stats.item_description, stats.primary_sku,
               stats.facility_id, stats.facility_name, stats.location_id,
               stats.location_name, stats.location_barcode,
               ARRAY_AGG(quantity.uom ORDER BY quantity.uom) AS uoms,
               ARRAY_AGG(quantity.on_hand ORDER BY quantity.uom) AS on_hand,
               ARRAY_AGG(quantity.reserved ORDER BY quantity.uom) AS reserved,
               ARRAY_AGG(quantity.held ORDER BY quantity.uom) AS held,
               ARRAY_AGG(quantity.available ORDER BY quantity.uom) AS available,
               stats.balance_count, stats.batch_count
        FROM group_stats stats
        INNER JOIN by_uom quantity
            ON quantity.inventory_owner_id = stats.inventory_owner_id
           AND quantity.item_id = stats.item_id
           AND quantity.facility_id = stats.facility_id
           AND quantity.location_id = stats.location_id
        GROUP BY stats.inventory_owner_id, stats.inventory_owner_name,
                 stats.item_id, stats.item_description, stats.primary_sku,
                 stats.facility_id, stats.facility_name, stats.location_id,
                 stats.location_name, stats.location_barcode,
                 stats.balance_count, stats.batch_count
        ORDER BY stats.inventory_owner_id, stats.item_id, stats.location_id
        LIMIT $9
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(after.map(|cursor| cursor.inventory_owner_id))
    .bind(after.map(|cursor| cursor.item_id))
    .bind(after.map(|cursor| cursor.location_id))
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(limit);
    let rows = rows
        .iter()
        .take(usize::from(limit))
        .map(map_location_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if has_more {
        rows.last().map(|row| LocationRollupCursor {
            inventory_owner_id: row.inventory_owner_id,
            item_id: row.item_id,
            location_id: row.location_id,
        })
    } else {
        None
    };
    tx.commit().await?;

    Ok(InventoryLocationRollupKeysetPage { rows, next_cursor })
}

pub async fn get_inventory_facility_rollup_page(
    db: &Db,
    access: &TenantAccess,
    after: Option<FacilityRollupCursor>,
    limit: u16,
) -> AppResult<InventoryFacilityRollupKeysetPage> {
    let scope = ScopeBindings::for_access(access);
    let fetch_limit = i64::from(limit) + 1;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        WITH base AS (
            SELECT balance.inventory_owner_id, owner.name AS inventory_owner_name,
                   balance.item_id, item.description AS item_description,
                   sku.name AS primary_sku, balance.facility_id,
                   facility.name AS facility_name, balance.location_id,
                   balance.item_batch_id, balance.uom, balance.status,
                   balance.qty_on_hand, balance.qty_reserved, balance.qty_held
            FROM inventory_balances balance
            INNER JOIN inventory_owners owner
                ON owner.tenant_id = balance.tenant_id
               AND owner.id = balance.inventory_owner_id
            INNER JOIN facilities facility
                ON facility.tenant_id = balance.tenant_id
               AND facility.id = balance.facility_id
            INNER JOIN items item
                ON item.tenant_id = balance.tenant_id
               AND item.id = balance.item_id
            LEFT JOIN LATERAL (
                SELECT item_sku.name
                FROM skus item_sku
                WHERE item_sku.tenant_id = balance.tenant_id
                  AND item_sku.item_id = balance.item_id
                  AND item_sku.deleted IS NULL
                ORDER BY item_sku.id
                LIMIT 1
            ) sku ON TRUE
            WHERE balance.tenant_id = $1
              AND balance.deleted IS NULL
              AND ($2 OR balance.facility_id = ANY($3))
              AND ($4 OR balance.inventory_owner_id = ANY($5))
              AND (
                    $6::BIGINT IS NULL
                    OR (balance.inventory_owner_id, balance.item_id, balance.facility_id)
                       > ($6, $7, $8)
              )
        ),
        by_uom AS (
            SELECT inventory_owner_id, item_id, facility_id, uom,
                   SUM(qty_on_hand)::BIGINT AS on_hand,
                   SUM(qty_reserved)::BIGINT AS reserved,
                   SUM(qty_held)::BIGINT AS held,
                   SUM(
                       CASE WHEN status = 'available'
                           THEN qty_on_hand - qty_reserved - qty_held
                           ELSE 0
                       END
                   )::BIGINT AS available
            FROM base
            GROUP BY inventory_owner_id, item_id, facility_id, uom
        ),
        group_stats AS (
            SELECT inventory_owner_id, inventory_owner_name, item_id, item_description,
                   primary_sku, facility_id, facility_name,
                   COUNT(*)::BIGINT AS balance_count,
                   COUNT(DISTINCT item_batch_id)::BIGINT AS batch_count,
                   COUNT(DISTINCT location_id)::BIGINT AS location_count
            FROM base
            GROUP BY inventory_owner_id, inventory_owner_name, item_id, item_description,
                     primary_sku, facility_id, facility_name
        )
        SELECT stats.inventory_owner_id, stats.inventory_owner_name,
               stats.item_id, stats.item_description, stats.primary_sku,
               stats.facility_id, stats.facility_name,
               ARRAY_AGG(quantity.uom ORDER BY quantity.uom) AS uoms,
               ARRAY_AGG(quantity.on_hand ORDER BY quantity.uom) AS on_hand,
               ARRAY_AGG(quantity.reserved ORDER BY quantity.uom) AS reserved,
               ARRAY_AGG(quantity.held ORDER BY quantity.uom) AS held,
               ARRAY_AGG(quantity.available ORDER BY quantity.uom) AS available,
               stats.balance_count, stats.batch_count, stats.location_count
        FROM group_stats stats
        INNER JOIN by_uom quantity
            ON quantity.inventory_owner_id = stats.inventory_owner_id
           AND quantity.item_id = stats.item_id
           AND quantity.facility_id = stats.facility_id
        GROUP BY stats.inventory_owner_id, stats.inventory_owner_name,
                 stats.item_id, stats.item_description, stats.primary_sku,
                 stats.facility_id, stats.facility_name,
                 stats.balance_count, stats.batch_count, stats.location_count
        ORDER BY stats.inventory_owner_id, stats.item_id, stats.facility_id
        LIMIT $9
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(after.map(|cursor| cursor.inventory_owner_id))
    .bind(after.map(|cursor| cursor.item_id))
    .bind(after.map(|cursor| cursor.facility_id))
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(limit);
    let rows = rows
        .iter()
        .take(usize::from(limit))
        .map(map_facility_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if has_more {
        rows.last().map(|row| FacilityRollupCursor {
            inventory_owner_id: row.inventory_owner_id,
            item_id: row.item_id,
            facility_id: row.facility_id,
        })
    } else {
        None
    };
    tx.commit().await?;

    Ok(InventoryFacilityRollupKeysetPage { rows, next_cursor })
}

pub async fn get_inventory_item_rollup_page(
    db: &Db,
    access: &TenantAccess,
    after: Option<ItemRollupCursor>,
    limit: u16,
) -> AppResult<InventoryItemRollupKeysetPage> {
    let scope = ScopeBindings::for_access(access);
    let fetch_limit = i64::from(limit) + 1;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        WITH base AS (
            SELECT balance.inventory_owner_id, owner.name AS inventory_owner_name,
                   balance.item_id, item.description AS item_description,
                   sku.name AS primary_sku, balance.facility_id, balance.location_id,
                   balance.item_batch_id, balance.uom, balance.status,
                   balance.qty_on_hand, balance.qty_reserved, balance.qty_held
            FROM inventory_balances balance
            INNER JOIN inventory_owners owner
                ON owner.tenant_id = balance.tenant_id
               AND owner.id = balance.inventory_owner_id
            INNER JOIN items item
                ON item.tenant_id = balance.tenant_id
               AND item.id = balance.item_id
            LEFT JOIN LATERAL (
                SELECT item_sku.name
                FROM skus item_sku
                WHERE item_sku.tenant_id = balance.tenant_id
                  AND item_sku.item_id = balance.item_id
                  AND item_sku.deleted IS NULL
                ORDER BY item_sku.id
                LIMIT 1
            ) sku ON TRUE
            WHERE balance.tenant_id = $1
              AND balance.deleted IS NULL
              AND ($2 OR balance.facility_id = ANY($3))
              AND ($4 OR balance.inventory_owner_id = ANY($5))
              AND (
                    $6::BIGINT IS NULL
                    OR (balance.inventory_owner_id, balance.item_id) > ($6, $7)
              )
        ),
        by_uom AS (
            SELECT inventory_owner_id, item_id, uom,
                   SUM(qty_on_hand)::BIGINT AS on_hand,
                   SUM(qty_reserved)::BIGINT AS reserved,
                   SUM(qty_held)::BIGINT AS held,
                   SUM(
                       CASE WHEN status = 'available'
                           THEN qty_on_hand - qty_reserved - qty_held
                           ELSE 0
                       END
                   )::BIGINT AS available
            FROM base
            GROUP BY inventory_owner_id, item_id, uom
        ),
        group_stats AS (
            SELECT inventory_owner_id, inventory_owner_name, item_id, item_description,
                   primary_sku, COUNT(*)::BIGINT AS balance_count,
                   COUNT(DISTINCT item_batch_id)::BIGINT AS batch_count,
                   COUNT(DISTINCT location_id)::BIGINT AS location_count,
                   COUNT(DISTINCT facility_id)::BIGINT AS facility_count
            FROM base
            GROUP BY inventory_owner_id, inventory_owner_name, item_id, item_description,
                     primary_sku
        )
        SELECT stats.inventory_owner_id, stats.inventory_owner_name,
               stats.item_id, stats.item_description, stats.primary_sku,
               ARRAY_AGG(quantity.uom ORDER BY quantity.uom) AS uoms,
               ARRAY_AGG(quantity.on_hand ORDER BY quantity.uom) AS on_hand,
               ARRAY_AGG(quantity.reserved ORDER BY quantity.uom) AS reserved,
               ARRAY_AGG(quantity.held ORDER BY quantity.uom) AS held,
               ARRAY_AGG(quantity.available ORDER BY quantity.uom) AS available,
               stats.balance_count, stats.batch_count,
               stats.location_count, stats.facility_count
        FROM group_stats stats
        INNER JOIN by_uom quantity
            ON quantity.inventory_owner_id = stats.inventory_owner_id
           AND quantity.item_id = stats.item_id
        GROUP BY stats.inventory_owner_id, stats.inventory_owner_name,
                 stats.item_id, stats.item_description, stats.primary_sku,
                 stats.balance_count, stats.batch_count,
                 stats.location_count, stats.facility_count
        ORDER BY stats.inventory_owner_id, stats.item_id
        LIMIT $8
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(after.map(|cursor| cursor.inventory_owner_id))
    .bind(after.map(|cursor| cursor.item_id))
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(limit);
    let rows = rows
        .iter()
        .take(usize::from(limit))
        .map(map_item_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if has_more {
        rows.last().map(|row| ItemRollupCursor {
            inventory_owner_id: row.inventory_owner_id,
            item_id: row.item_id,
        })
    } else {
        None
    };
    tx.commit().await?;

    Ok(InventoryItemRollupKeysetPage { rows, next_cursor })
}

fn map_quantity_columns(row: &PgRow) -> AppResult<InventoryRollupQuantityColumns> {
    Ok(InventoryRollupQuantityColumns {
        uoms: row.try_get("uoms")?,
        on_hand: row.try_get("on_hand")?,
        reserved: row.try_get("reserved")?,
        held: row.try_get("held")?,
        available: row.try_get("available")?,
    })
}

fn map_location_row(row: &PgRow) -> AppResult<InventoryLocationRollupRow> {
    Ok(InventoryLocationRollupRow {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        location_id: row.try_get("location_id")?,
        location_name: row.try_get("location_name")?,
        location_barcode: row.try_get("location_barcode")?,
        quantities: map_quantity_columns(row)?,
        balance_count: row.try_get("balance_count")?,
        batch_count: row.try_get("batch_count")?,
    })
}

fn map_facility_row(row: &PgRow) -> AppResult<InventoryFacilityRollupRow> {
    Ok(InventoryFacilityRollupRow {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        quantities: map_quantity_columns(row)?,
        balance_count: row.try_get("balance_count")?,
        batch_count: row.try_get("batch_count")?,
        location_count: row.try_get("location_count")?,
    })
}

fn map_item_row(row: &PgRow) -> AppResult<InventoryItemRollupRow> {
    Ok(InventoryItemRollupRow {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        quantities: map_quantity_columns(row)?,
        balance_count: row.try_get("balance_count")?,
        batch_count: row.try_get("batch_count")?,
        location_count: row.try_get("location_count")?,
        facility_count: row.try_get("facility_count")?,
    })
}
