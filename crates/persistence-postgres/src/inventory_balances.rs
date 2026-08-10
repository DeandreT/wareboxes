//! Scope-safe cursor-paginated reads for operational inventory balances.

use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::inventory::{
    InventoryBalancePage, InventoryBalancePageQuery, InventoryBalanceReadModel,
    InventoryBalanceSort, InventoryBalanceStatus, InventoryQuantityProjection,
    MAX_INVENTORY_BALANCE_PAGE_SIZE, MAX_INVENTORY_BALANCE_QUERY_LENGTH,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId, OwnerScope, SiteScope, TenantId};

use crate::db::{begin_tenant_transaction, Db};
use crate::{PersistenceError, PersistenceResult};

pub async fn get_inventory_balance_page(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    owner_scope: &OwnerScope,
    request: &InventoryBalancePageQuery,
) -> PersistenceResult<InventoryBalancePage> {
    validate_page_request(request.offset, request.limit, request.query.as_deref())?;
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
    let fetch_limit = i64::from(request.limit) + 1;
    let offset_i64 = i64::try_from(request.offset)
        .map_err(|_| PersistenceError::invalid_input("inventory balance cursor is out of range"))?;
    let query_id = request
        .query
        .as_deref()
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0);
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
          AND ($2 OR balance.facility_id = ANY($3))
          AND ($4 OR balance.inventory_owner_id = ANY($5))
              AND (
              $6::TEXT IS NULL
              OR STRPOS(LOWER(owner.name), LOWER($6)) > 0
              OR STRPOS(LOWER(facility.name), LOWER($6)) > 0
              OR STRPOS(LOWER(COALESCE(location.name, '')), LOWER($6)) > 0
              OR STRPOS(LOWER(COALESCE(location.barcode, '')), LOWER($6)) > 0
              OR STRPOS(LOWER(COALESCE(license_plate.barcode, '')), LOWER($6)) > 0
              OR STRPOS(LOWER(COALESCE(sku.name, '')), LOWER($6)) > 0
              OR STRPOS(LOWER(COALESCE(item.description, '')), LOWER($6)) > 0
              OR STRPOS(LOWER(COALESCE(batch.lot, '')), LOWER($6)) > 0
              OR STRPOS(LOWER(COALESCE(batch.serial, '')), LOWER($6)) > 0
              OR STRPOS(LOWER(balance.status), LOWER($6)) > 0
              OR (
                  $7::BIGINT IS NOT NULL
                  AND (
                      balance.id = $7
                      OR balance.inventory_owner_id = $7
                      OR balance.facility_id = $7
                      OR balance.location_id = $7
                      OR balance.license_plate_id = $7
                      OR balance.item_batch_id = $7
                      OR balance.item_id = $7
                  )
              )
          )
          AND (NOT $12 OR balance.qty_on_hand - balance.qty_reserved - balance.qty_held > 0)
        ORDER BY
          CASE WHEN $8='position' AND $9 THEN balance.id END ASC,
          CASE WHEN $8='position' AND NOT $9 THEN balance.id END DESC,
          CASE WHEN $8='facility' AND $9 THEN LOWER(facility.name) END ASC,
          CASE WHEN $8='facility' AND NOT $9 THEN LOWER(facility.name) END DESC,
          CASE WHEN $8='client' AND $9 THEN LOWER(owner.name) END ASC,
          CASE WHEN $8='client' AND NOT $9 THEN LOWER(owner.name) END DESC,
          CASE WHEN $8='location' AND $9 THEN LOWER(COALESCE(location.barcode,location.name,'')) END ASC,
          CASE WHEN $8='location' AND NOT $9 THEN LOWER(COALESCE(location.barcode,location.name,'')) END DESC,
          CASE WHEN $8='item' AND $9 THEN LOWER(COALESCE(sku.name,item.description,'')) END ASC,
          CASE WHEN $8='item' AND NOT $9 THEN LOWER(COALESCE(sku.name,item.description,'')) END DESC,
          CASE WHEN $8='tracking' AND $9 THEN LOWER(CONCAT_WS('/',batch.lot,batch.serial)) END ASC,
          CASE WHEN $8='tracking' AND NOT $9 THEN LOWER(CONCAT_WS('/',batch.lot,batch.serial)) END DESC,
          CASE WHEN $8='license_plate' AND $9 THEN LOWER(COALESCE(license_plate.barcode,'')) END ASC,
          CASE WHEN $8='license_plate' AND NOT $9 THEN LOWER(COALESCE(license_plate.barcode,'')) END DESC,
          CASE WHEN $8='status' AND $9 THEN balance.status END ASC,
          CASE WHEN $8='status' AND NOT $9 THEN balance.status END DESC,
          CASE WHEN $8='on_hand' AND $9 THEN balance.qty_on_hand END ASC,
          CASE WHEN $8='on_hand' AND NOT $9 THEN balance.qty_on_hand END DESC,
          CASE WHEN $8='reserved' AND $9 THEN balance.qty_reserved END ASC,
          CASE WHEN $8='reserved' AND NOT $9 THEN balance.qty_reserved END DESC,
          CASE WHEN $8='held' AND $9 THEN balance.qty_held END ASC,
          CASE WHEN $8='held' AND NOT $9 THEN balance.qty_held END DESC,
          CASE WHEN $8='available' AND $9 THEN balance.qty_on_hand - balance.qty_reserved - balance.qty_held END ASC,
          CASE WHEN $8='available' AND NOT $9 THEN balance.qty_on_hand - balance.qty_reserved - balance.qty_held END DESC,
          balance.id ASC
        OFFSET $10 LIMIT $11
        "#,
    )
    .bind(tenant_id.get())
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .bind(request.query.as_deref())
    .bind(query_id)
    .bind(sort_key(request.sort))
    .bind(request.direction.is_ascending())
    .bind(offset_i64)
    .bind(fetch_limit)
    .bind(request.movable_only)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(request.limit);
    let items = rows
        .iter()
        .take(usize::from(request.limit))
        .map(map_balance)
        .collect::<PersistenceResult<Vec<_>>>()?;
    let next_offset = has_more.then_some(request.offset + u64::from(request.limit));
    tx.commit().await?;

    Ok(InventoryBalancePage { items, next_offset })
}

fn validate_page_request(offset: u64, limit: u16, query: Option<&str>) -> PersistenceResult<()> {
    let _ = i64::try_from(offset)
        .map_err(|_| PersistenceError::invalid_input("inventory balance cursor is out of range"))?;
    if !(1..=MAX_INVENTORY_BALANCE_PAGE_SIZE).contains(&limit) {
        return Err(PersistenceError::invalid_input(format!(
            "inventory balance page size must be between 1 and {MAX_INVENTORY_BALANCE_PAGE_SIZE}"
        )));
    }
    if let Some(query) = query {
        if query.is_empty() {
            return Err(PersistenceError::invalid_input(
                "inventory balance query cannot be empty",
            ));
        }
        if query.trim() != query {
            return Err(PersistenceError::invalid_input(
                "inventory balance query must be trimmed",
            ));
        }
        if query.chars().count() > MAX_INVENTORY_BALANCE_QUERY_LENGTH {
            return Err(PersistenceError::invalid_input(format!(
                "inventory balance query cannot exceed {MAX_INVENTORY_BALANCE_QUERY_LENGTH} characters"
            )));
        }
        if query.chars().any(char::is_control) {
            return Err(PersistenceError::invalid_input(
                "inventory balance query cannot contain control characters",
            ));
        }
    }
    Ok(())
}

fn sort_key(sort: InventoryBalanceSort) -> &'static str {
    match sort {
        InventoryBalanceSort::Position => "position",
        InventoryBalanceSort::Facility => "facility",
        InventoryBalanceSort::Client => "client",
        InventoryBalanceSort::Location => "location",
        InventoryBalanceSort::Item => "item",
        InventoryBalanceSort::Tracking => "tracking",
        InventoryBalanceSort::LicensePlate => "license_plate",
        InventoryBalanceSort::Status => "status",
        InventoryBalanceSort::OnHand => "on_hand",
        InventoryBalanceSort::Reserved => "reserved",
        InventoryBalanceSort::Held => "held",
        InventoryBalanceSort::Available => "available",
    }
}

fn map_balance(row: &PgRow) -> PersistenceResult<InventoryBalanceReadModel> {
    let id: i64 = row.try_get("id")?;
    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| PersistenceError::invalid_data(error.to_string()))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?)
        .map_err(|error| PersistenceError::invalid_data(error.to_string()))?;
    let status_value: String = row.try_get("status")?;
    let status = InventoryBalanceStatus::parse(&status_value).ok_or_else(|| {
        PersistenceError::invalid_data(format!(
            "inventory balance {id} has unknown status {status_value:?}"
        ))
    })?;
    let quantity = InventoryQuantityProjection::new(
        status,
        row.try_get("qty_on_hand")?,
        row.try_get("qty_reserved")?,
        row.try_get("qty_held")?,
    )
    .map_err(|error| {
        PersistenceError::invalid_data(format!(
            "inventory balance {id} has invalid quantities: {error:?}"
        ))
    })?;

    Ok(InventoryBalanceReadModel {
        id,
        inventory_owner_id,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id,
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
        status,
        quantity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_request_validation_rejects_untrusted_direct_inputs() {
        assert!(validate_page_request(0, 1, Some("SKU-1")).is_ok());
        assert!(matches!(
            validate_page_request(0, 0, None),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(0, MAX_INVENTORY_BALANCE_PAGE_SIZE + 1, None),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(0, 1, Some(" SKU-1")),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(
                0,
                1,
                Some(&"x".repeat(MAX_INVENTORY_BALANCE_QUERY_LENGTH + 1))
            ),
            Err(PersistenceError::InvalidInput(_))
        ));
    }
}
