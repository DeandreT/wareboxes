//! Keyset-paginated inventory hold reads for the version 1 API.

use sqlx::Row;
use wareboxes_core::models::{TenantAccess, Timestamp};

use crate::db::{begin_tenant_transaction, Db};
use crate::error::AppResult;
use crate::repo::access::ScopeBindings;

#[derive(Debug)]
pub struct InventoryHoldPageRow {
    pub id: i64,
    pub created_at: Timestamp,
    pub created_by_user_id: i64,
    pub released_at: Option<Timestamp>,
    pub released_by_user_id: Option<i64>,
    pub inventory_balance_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_barcode: Option<String>,
    pub location_name: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub inventory_status: String,
    pub quantity: i64,
    pub reason: String,
    pub note: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub status: String,
}

pub struct InventoryHoldKeysetPage {
    pub rows: Vec<InventoryHoldPageRow>,
    pub next_before_id: Option<i64>,
}

/// Reads holds newest-first, after applying both caller scopes in PostgreSQL.
pub async fn get_inventory_hold_page(
    db: &Db,
    access: &TenantAccess,
    before_id: Option<i64>,
    limit: u16,
    status: Option<&str>,
) -> AppResult<InventoryHoldKeysetPage> {
    let scope = ScopeBindings::for_access(access);
    let fetch_limit = i64::from(limit) + 1;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT hold.id, hold.created AS created_at,
               hold.created_by AS created_by_user_id,
               hold.released_at, hold.released_by AS released_by_user_id,
               hold.inventory_balance_id,
               hold.inventory_owner_id, owner.name AS inventory_owner_name,
               hold.facility_id, facility.name AS facility_name,
               hold.location_id, location.barcode AS location_barcode,
               location.name AS location_name,
               hold.license_plate_id, plate.barcode AS license_plate_barcode,
               hold.item_batch_id, batch.lot, batch.serial, batch.expiration,
               hold.item_id, item.description AS item_description,
               hold.uom, hold.inventory_status, hold.qty AS quantity,
               hold.reason_code AS reason, hold.note, hold.reference_type,
               hold.reference_id, hold.status
        FROM inventory_holds hold
        INNER JOIN inventory_owners owner
            ON owner.tenant_id = hold.tenant_id
           AND owner.id = hold.inventory_owner_id
        INNER JOIN facilities facility
            ON facility.tenant_id = hold.tenant_id
           AND facility.id = hold.facility_id
        INNER JOIN locations location
            ON location.tenant_id = hold.tenant_id
           AND location.id = hold.location_id
        INNER JOIN item_batches batch
            ON batch.tenant_id = hold.tenant_id
           AND batch.inventory_owner_id = hold.inventory_owner_id
           AND batch.id = hold.item_batch_id
        INNER JOIN items item
            ON item.tenant_id = hold.tenant_id
           AND item.id = hold.item_id
        LEFT JOIN license_plates plate
            ON plate.tenant_id = hold.tenant_id
           AND plate.inventory_owner_id = hold.inventory_owner_id
           AND plate.facility_id = hold.facility_id
           AND plate.id = hold.license_plate_id
        WHERE hold.tenant_id = $1
          AND ($2::BIGINT IS NULL OR hold.id < $2)
          AND ($3::TEXT IS NULL OR hold.status = $3)
          AND ($4 OR hold.facility_id = ANY($5))
          AND ($6 OR hold.inventory_owner_id = ANY($7))
        ORDER BY hold.id DESC
        LIMIT $8
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(before_id)
    .bind(status)
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
        .map(map_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_before_id = if has_more {
        rows.last().map(|row| row.id)
    } else {
        None
    };
    tx.commit().await?;

    Ok(InventoryHoldKeysetPage {
        rows,
        next_before_id,
    })
}

fn map_row(row: &sqlx::postgres::PgRow) -> AppResult<InventoryHoldPageRow> {
    Ok(InventoryHoldPageRow {
        id: row.try_get("id")?,
        created_at: row.try_get("created_at")?,
        created_by_user_id: row.try_get("created_by_user_id")?,
        released_at: row.try_get("released_at")?,
        released_by_user_id: row.try_get("released_by_user_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        location_id: row.try_get("location_id")?,
        location_barcode: row.try_get("location_barcode")?,
        location_name: row.try_get("location_name")?,
        license_plate_id: row.try_get("license_plate_id")?,
        license_plate_barcode: row.try_get("license_plate_barcode")?,
        item_batch_id: row.try_get("item_batch_id")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        inventory_status: row.try_get("inventory_status")?,
        quantity: row.try_get("quantity")?,
        reason: row.try_get("reason")?,
        note: row.try_get("note")?,
        reference_type: row.try_get("reference_type")?,
        reference_id: row.try_get("reference_id")?,
        status: row.try_get("status")?,
    })
}
