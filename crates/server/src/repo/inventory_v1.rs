//! Keyset reads used by the version 1 inventory contract.

use sqlx::Row;
use wareboxes_core::models::TenantAccess;

use crate::db::{begin_tenant_transaction, Db};
use crate::error::AppResult;
use crate::repo::access::ScopeBindings;

/// Internal projection mapped into the public response by the transport layer.
#[derive(Debug)]
pub struct InventoryBalancePageRow {
    pub id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub uom: String,
    pub status: String,
    pub qty_on_hand: i64,
    pub qty_reserved: i64,
    pub qty_held: i64,
}

/// Internal keyset page.
pub struct InventoryBalanceKeysetPage {
    pub rows: Vec<InventoryBalancePageRow>,
    pub next_after_id: Option<i64>,
}

/// Reads active balances after a stable unique key, restricted to the caller's scopes.
pub async fn get_inventory_balance_page(
    db: &Db,
    access: &TenantAccess,
    after_id: Option<i64>,
    limit: u16,
) -> AppResult<InventoryBalanceKeysetPage> {
    let scope = ScopeBindings::for_access(access);
    let fetch_limit = i64::from(limit) + 1;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT balance.id, balance.inventory_owner_id,
               owner.name AS inventory_owner_name, balance.facility_id,
               facility.name AS facility_name, balance.location_id,
               location.name AS location_name, location.barcode AS location_barcode,
               balance.license_plate_id,
               license_plate.barcode AS license_plate_barcode,
               balance.item_batch_id, balance.item_id,
               item.description AS item_description, sku.name AS primary_sku,
               batch.lot, batch.serial, balance.uom, balance.status,
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
           AND location.id = balance.location_id
        INNER JOIN item_batches batch
            ON batch.tenant_id = balance.tenant_id
           AND batch.inventory_owner_id = balance.inventory_owner_id
           AND batch.id = balance.item_batch_id
        INNER JOIN items item
            ON item.tenant_id = balance.tenant_id
           AND item.id = balance.item_id
        LEFT JOIN license_plates license_plate
            ON license_plate.tenant_id = balance.tenant_id
           AND license_plate.inventory_owner_id = balance.inventory_owner_id
           AND license_plate.id = balance.license_plate_id
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
          AND ($2::BIGINT IS NULL OR balance.id > $2)
          AND ($3 OR balance.facility_id = ANY($4))
          AND ($5 OR balance.inventory_owner_id = ANY($6))
        ORDER BY balance.id
        LIMIT $7
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(after_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(limit);
    let rows = rows
        .iter()
        .take(usize::from(limit))
        .map(|row| {
            Ok(InventoryBalancePageRow {
                id: row.try_get("id")?,
                inventory_owner_id: row.try_get("inventory_owner_id")?,
                inventory_owner_name: row.try_get("inventory_owner_name")?,
                facility_id: row.try_get("facility_id")?,
                facility_name: row.try_get("facility_name")?,
                location_id: row.try_get("location_id")?,
                location_name: row.try_get("location_name")?,
                location_barcode: row.try_get("location_barcode")?,
                license_plate_id: row.try_get("license_plate_id")?,
                license_plate_barcode: row.try_get("license_plate_barcode")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                item_description: row.try_get("item_description")?,
                primary_sku: row.try_get("primary_sku")?,
                lot: row.try_get("lot")?,
                serial: row.try_get("serial")?,
                uom: row.try_get("uom")?,
                status: row.try_get("status")?,
                qty_on_hand: row.try_get("qty_on_hand")?,
                qty_reserved: row.try_get("qty_reserved")?,
                qty_held: row.try_get("qty_held")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let next_after_id = if has_more {
        rows.last().map(|row| row.id)
    } else {
        None
    };
    tx.commit().await?;

    Ok(InventoryBalanceKeysetPage {
        rows,
        next_after_id,
    })
}
