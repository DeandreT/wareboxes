//! Scope-safe aggregate inventory reads.

use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::inventory::{
    InventoryFacilityRollupPage, InventoryFacilityRollupReadModel, InventoryItemRollupPage,
    InventoryItemRollupReadModel, InventoryLocationRollupPage, InventoryLocationRollupReadModel,
    InventoryRollupCount, InventoryRollupPageQuery, InventoryRollupQuantity,
    MAX_INVENTORY_ROLLUP_PAGE_SIZE,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId, OwnerScope, SiteScope, TenantId};

use crate::db::{begin_tenant_transaction, Db};
use crate::{PersistenceError, PersistenceResult};

pub async fn get_inventory_location_rollup_page(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    owner_scope: &OwnerScope,
    query: &InventoryRollupPageQuery,
) -> PersistenceResult<InventoryLocationRollupPage> {
    validate_page_request(query)?;
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let inventory_owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let fetch_limit = i64::from(query.limit) + 1;
    let offset = i64::try_from(query.offset)
        .map_err(|_| PersistenceError::invalid_input("inventory rollup offset is too large"))?;
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
              AND ($6::TEXT IS NULL OR CONCAT_WS(' ', owner.name, sku.name,
                    item.description, facility.name, location.name, location.barcode)
                    ILIKE '%' || $6 || '%')
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
        ORDER BY
            CASE WHEN $7 = 'client' AND $8 = 'ascending' THEN LOWER(stats.inventory_owner_name) END ASC,
            CASE WHEN $7 = 'client' AND $8 = 'descending' THEN LOWER(stats.inventory_owner_name) END DESC,
            CASE WHEN $7 = 'item' AND $8 = 'ascending' THEN LOWER(COALESCE(stats.primary_sku, stats.item_description, 'Item #' || stats.item_id)) END ASC,
            CASE WHEN $7 = 'item' AND $8 = 'descending' THEN LOWER(COALESCE(stats.primary_sku, stats.item_description, 'Item #' || stats.item_id)) END DESC,
            CASE WHEN $7 = 'scope' AND $8 = 'ascending' THEN LOWER(COALESCE(stats.location_barcode, stats.location_name, 'Location #' || stats.location_id)) END ASC,
            CASE WHEN $7 = 'scope' AND $8 = 'descending' THEN LOWER(COALESCE(stats.location_barcode, stats.location_name, 'Location #' || stats.location_id)) END DESC,
            CASE WHEN $7 = 'balances' AND $8 = 'ascending' THEN stats.balance_count END ASC,
            CASE WHEN $7 = 'balances' AND $8 = 'descending' THEN stats.balance_count END DESC,
            CASE WHEN $7 = 'batches' AND $8 = 'ascending' THEN stats.batch_count END ASC,
            CASE WHEN $7 = 'batches' AND $8 = 'descending' THEN stats.batch_count END DESC,
            stats.inventory_owner_id, stats.item_id, stats.location_id
        LIMIT $9 OFFSET $10
        "#,
    )
    .bind(tenant_id.get())
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .bind(query.query.as_deref())
    .bind(query.sort.as_str())
    .bind(query.direction.as_str())
    .bind(fetch_limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(query.limit);
    let rows = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_location_row)
        .collect::<PersistenceResult<Vec<_>>>()?;
    let next_offset = has_more.then(|| query.offset + u64::from(query.limit));
    tx.commit().await?;

    Ok(InventoryLocationRollupPage {
        items: rows,
        next_offset,
    })
}

pub async fn get_inventory_facility_rollup_page(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    owner_scope: &OwnerScope,
    query: &InventoryRollupPageQuery,
) -> PersistenceResult<InventoryFacilityRollupPage> {
    validate_page_request(query)?;
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let inventory_owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let fetch_limit = i64::from(query.limit) + 1;
    let offset = i64::try_from(query.offset)
        .map_err(|_| PersistenceError::invalid_input("inventory rollup offset is too large"))?;
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
              AND ($6::TEXT IS NULL OR CONCAT_WS(' ', owner.name, sku.name,
                    item.description, facility.name) ILIKE '%' || $6 || '%')
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
        ORDER BY
            CASE WHEN $7 = 'client' AND $8 = 'ascending' THEN LOWER(stats.inventory_owner_name) END ASC,
            CASE WHEN $7 = 'client' AND $8 = 'descending' THEN LOWER(stats.inventory_owner_name) END DESC,
            CASE WHEN $7 = 'item' AND $8 = 'ascending' THEN LOWER(COALESCE(stats.primary_sku, stats.item_description, 'Item #' || stats.item_id)) END ASC,
            CASE WHEN $7 = 'item' AND $8 = 'descending' THEN LOWER(COALESCE(stats.primary_sku, stats.item_description, 'Item #' || stats.item_id)) END DESC,
            CASE WHEN $7 = 'scope' AND $8 = 'ascending' THEN LOWER(COALESCE(stats.facility_name, 'Facility #' || stats.facility_id)) END ASC,
            CASE WHEN $7 = 'scope' AND $8 = 'descending' THEN LOWER(COALESCE(stats.facility_name, 'Facility #' || stats.facility_id)) END DESC,
            CASE WHEN $7 = 'locations' AND $8 = 'ascending' THEN stats.location_count END ASC,
            CASE WHEN $7 = 'locations' AND $8 = 'descending' THEN stats.location_count END DESC,
            CASE WHEN $7 = 'balances' AND $8 = 'ascending' THEN stats.balance_count END ASC,
            CASE WHEN $7 = 'balances' AND $8 = 'descending' THEN stats.balance_count END DESC,
            CASE WHEN $7 = 'batches' AND $8 = 'ascending' THEN stats.batch_count END ASC,
            CASE WHEN $7 = 'batches' AND $8 = 'descending' THEN stats.batch_count END DESC,
            stats.inventory_owner_id, stats.item_id, stats.facility_id
        LIMIT $9 OFFSET $10
        "#,
    )
    .bind(tenant_id.get())
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .bind(query.query.as_deref())
    .bind(query.sort.as_str())
    .bind(query.direction.as_str())
    .bind(fetch_limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(query.limit);
    let rows = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_facility_row)
        .collect::<PersistenceResult<Vec<_>>>()?;
    let next_offset = has_more.then(|| query.offset + u64::from(query.limit));
    tx.commit().await?;

    Ok(InventoryFacilityRollupPage {
        items: rows,
        next_offset,
    })
}

pub async fn get_inventory_item_rollup_page(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    owner_scope: &OwnerScope,
    query: &InventoryRollupPageQuery,
) -> PersistenceResult<InventoryItemRollupPage> {
    validate_page_request(query)?;
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let inventory_owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let fetch_limit = i64::from(query.limit) + 1;
    let offset = i64::try_from(query.offset)
        .map_err(|_| PersistenceError::invalid_input("inventory rollup offset is too large"))?;
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
              AND ($6::TEXT IS NULL OR CONCAT_WS(' ', owner.name, sku.name,
                    item.description) ILIKE '%' || $6 || '%')
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
        ORDER BY
            CASE WHEN $7 = 'client' AND $8 = 'ascending' THEN LOWER(stats.inventory_owner_name) END ASC,
            CASE WHEN $7 = 'client' AND $8 = 'descending' THEN LOWER(stats.inventory_owner_name) END DESC,
            CASE WHEN $7 = 'item' AND $8 = 'ascending' THEN LOWER(COALESCE(stats.primary_sku, stats.item_description, 'Item #' || stats.item_id)) END ASC,
            CASE WHEN $7 = 'item' AND $8 = 'descending' THEN LOWER(COALESCE(stats.primary_sku, stats.item_description, 'Item #' || stats.item_id)) END DESC,
            CASE WHEN $7 = 'scope' AND $8 = 'ascending' THEN stats.facility_count END ASC,
            CASE WHEN $7 = 'scope' AND $8 = 'descending' THEN stats.facility_count END DESC,
            CASE WHEN $7 = 'locations' AND $8 = 'ascending' THEN stats.location_count END ASC,
            CASE WHEN $7 = 'locations' AND $8 = 'descending' THEN stats.location_count END DESC,
            CASE WHEN $7 = 'balances' AND $8 = 'ascending' THEN stats.balance_count END ASC,
            CASE WHEN $7 = 'balances' AND $8 = 'descending' THEN stats.balance_count END DESC,
            CASE WHEN $7 = 'batches' AND $8 = 'ascending' THEN stats.batch_count END ASC,
            CASE WHEN $7 = 'batches' AND $8 = 'descending' THEN stats.batch_count END DESC,
            stats.inventory_owner_id, stats.item_id
        LIMIT $9 OFFSET $10
        "#,
    )
    .bind(tenant_id.get())
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .bind(query.query.as_deref())
    .bind(query.sort.as_str())
    .bind(query.direction.as_str())
    .bind(fetch_limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(query.limit);
    let rows = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_item_row)
        .collect::<PersistenceResult<Vec<_>>>()?;
    let next_offset = has_more.then(|| query.offset + u64::from(query.limit));
    tx.commit().await?;

    Ok(InventoryItemRollupPage {
        items: rows,
        next_offset,
    })
}

fn map_quantities(row: &PgRow) -> PersistenceResult<Vec<InventoryRollupQuantity>> {
    map_quantity_columns(
        row.try_get("uoms")?,
        row.try_get("on_hand")?,
        row.try_get("reserved")?,
        row.try_get("held")?,
        row.try_get("available")?,
    )
}

fn map_quantity_columns(
    uoms: Vec<String>,
    on_hand: Vec<i64>,
    reserved: Vec<i64>,
    held: Vec<i64>,
    available: Vec<i64>,
) -> PersistenceResult<Vec<InventoryRollupQuantity>> {
    let count = uoms.len();
    if count == 0
        || on_hand.len() != count
        || reserved.len() != count
        || held.len() != count
        || available.len() != count
    {
        return Err(PersistenceError::invalid_data(
            "inventory rollup quantity columns are inconsistent",
        ));
    }

    uoms.into_iter()
        .zip(on_hand)
        .zip(reserved)
        .zip(held)
        .zip(available)
        .map(|((((uom, on_hand), reserved), held), available)| {
            InventoryRollupQuantity::new(uom, on_hand, reserved, held, available).map_err(|error| {
                PersistenceError::invalid_data(format!(
                    "inventory rollup quantities are inconsistent: {error:?}"
                ))
            })
        })
        .collect()
}

fn map_location_row(row: &PgRow) -> PersistenceResult<InventoryLocationRollupReadModel> {
    Ok(InventoryLocationRollupReadModel {
        inventory_owner_id: map_inventory_owner_id(row)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        facility_id: map_facility_id(row)?,
        facility_name: row.try_get("facility_name")?,
        location_id: row.try_get("location_id")?,
        location_name: row.try_get("location_name")?,
        location_barcode: row.try_get("location_barcode")?,
        quantities: map_quantities(row)?,
        balance_count: map_count(row, "balance_count")?,
        batch_count: map_count(row, "batch_count")?,
    })
}

fn map_facility_row(row: &PgRow) -> PersistenceResult<InventoryFacilityRollupReadModel> {
    Ok(InventoryFacilityRollupReadModel {
        inventory_owner_id: map_inventory_owner_id(row)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        facility_id: map_facility_id(row)?,
        facility_name: row.try_get("facility_name")?,
        quantities: map_quantities(row)?,
        balance_count: map_count(row, "balance_count")?,
        batch_count: map_count(row, "batch_count")?,
        location_count: map_count(row, "location_count")?,
    })
}

fn map_item_row(row: &PgRow) -> PersistenceResult<InventoryItemRollupReadModel> {
    Ok(InventoryItemRollupReadModel {
        inventory_owner_id: map_inventory_owner_id(row)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        quantities: map_quantities(row)?,
        balance_count: map_count(row, "balance_count")?,
        batch_count: map_count(row, "batch_count")?,
        location_count: map_count(row, "location_count")?,
        facility_count: map_count(row, "facility_count")?,
    })
}

fn map_inventory_owner_id(row: &PgRow) -> PersistenceResult<InventoryOwnerId> {
    InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| PersistenceError::invalid_data(error.to_string()))
}

fn map_facility_id(row: &PgRow) -> PersistenceResult<FacilityId> {
    FacilityId::new(row.try_get("facility_id")?)
        .map_err(|error| PersistenceError::invalid_data(error.to_string()))
}

fn map_count(row: &PgRow, column: &str) -> PersistenceResult<InventoryRollupCount> {
    let count = row.try_get(column)?;
    InventoryRollupCount::new(count).ok_or_else(|| {
        PersistenceError::invalid_data(format!("inventory rollup {column} is invalid"))
    })
}

fn validate_page_request(query: &InventoryRollupPageQuery) -> PersistenceResult<()> {
    if !(1..=MAX_INVENTORY_ROLLUP_PAGE_SIZE).contains(&query.limit) {
        return Err(PersistenceError::invalid_input(format!(
            "inventory rollup page size must be between 1 and {MAX_INVENTORY_ROLLUP_PAGE_SIZE}"
        )));
    }
    if query.offset > i64::MAX as u64 {
        return Err(PersistenceError::invalid_input(
            "inventory rollup offset is too large",
        ));
    }
    if query.query.as_ref().is_some_and(|value| {
        value.is_empty()
            || value.trim() != value
            || value.chars().count() > 200
            || value.chars().any(char::is_control)
    }) {
        return Err(PersistenceError::invalid_input(
            "inventory rollup query is invalid",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_request_validation_rejects_untrusted_direct_inputs() {
        let request = InventoryRollupPageQuery {
            offset: 0,
            limit: 1,
            query: None,
            sort: wareboxes_application::inventory::InventoryRollupSort::Client,
            direction: wareboxes_application::inventory::InventoryRollupSortDirection::Ascending,
        };
        assert!(validate_page_request(&request).is_ok());
        assert!(matches!(
            validate_page_request(&InventoryRollupPageQuery {
                limit: 0,
                ..request.clone()
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(&InventoryRollupPageQuery {
                limit: MAX_INVENTORY_ROLLUP_PAGE_SIZE + 1,
                ..request.clone()
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(&InventoryRollupPageQuery {
                query: Some(" not-trimmed ".to_owned()),
                ..request
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn quantity_mapping_rejects_misaligned_database_columns() {
        assert!(matches!(
            map_quantity_columns(
                vec!["each".to_owned()],
                vec![5],
                Vec::new(),
                vec![0],
                vec![5],
            ),
            Err(PersistenceError::InvalidData(_))
        ));
    }
}
