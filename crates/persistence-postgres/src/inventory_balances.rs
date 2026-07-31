//! Scope-safe keyset reads for operational inventory balances.

use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::inventory::{
    InventoryBalancePage, InventoryBalanceReadModel, InventoryBalanceStatus,
    InventoryQuantityProjection, MAX_INVENTORY_BALANCE_PAGE_SIZE,
    MAX_INVENTORY_BALANCE_QUERY_LENGTH,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId, OwnerScope, SiteScope, TenantId};

use crate::db::{begin_tenant_transaction, Db};
use crate::{PersistenceError, PersistenceResult};

pub async fn get_inventory_balance_page(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    owner_scope: &OwnerScope,
    after_id: Option<i64>,
    limit: u16,
    query: Option<&str>,
) -> PersistenceResult<InventoryBalancePage> {
    validate_page_request(after_id, limit, query)?;
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
    let fetch_limit = i64::from(limit) + 1;
    let query_id = query
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
          AND ($2::BIGINT IS NULL OR balance.id > $2)
          AND ($3 OR balance.facility_id = ANY($4))
          AND ($5 OR balance.inventory_owner_id = ANY($6))
          AND (
              $7::TEXT IS NULL
              OR STRPOS(LOWER(COALESCE(location.name, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(location.barcode, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(license_plate.barcode, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(sku.name, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(item.description, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(batch.lot, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(batch.serial, '')), LOWER($7)) > 0
              OR (
                  $8::BIGINT IS NOT NULL
                  AND (
                      balance.id = $8
                      OR balance.inventory_owner_id = $8
                      OR balance.facility_id = $8
                      OR balance.location_id = $8
                      OR balance.license_plate_id = $8
                      OR balance.item_batch_id = $8
                      OR balance.item_id = $8
                  )
              )
          )
        ORDER BY balance.id
        LIMIT $9
        "#,
    )
    .bind(tenant_id.get())
    .bind(after_id)
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .bind(query)
    .bind(query_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(limit);
    let items = rows
        .iter()
        .take(usize::from(limit))
        .map(map_balance)
        .collect::<PersistenceResult<Vec<_>>>()?;
    let next_after_id = if has_more {
        items.last().map(|balance| balance.id)
    } else {
        None
    };
    tx.commit().await?;

    Ok(InventoryBalancePage {
        items,
        next_after_id,
    })
}

fn validate_page_request(
    after_id: Option<i64>,
    limit: u16,
    query: Option<&str>,
) -> PersistenceResult<()> {
    if after_id.is_some_and(|id| id <= 0) {
        return Err(PersistenceError::invalid_input(
            "inventory balance cursor ID must be positive",
        ));
    }
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
        assert!(validate_page_request(None, 1, Some("SKU-1")).is_ok());
        assert!(matches!(
            validate_page_request(Some(0), 1, None),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(None, 0, None),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(None, MAX_INVENTORY_BALANCE_PAGE_SIZE + 1, None),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(None, 1, Some(" SKU-1")),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_request(
                None,
                1,
                Some(&"x".repeat(MAX_INVENTORY_BALANCE_QUERY_LENGTH + 1))
            ),
            Err(PersistenceError::InvalidInput(_))
        ));
    }
}
