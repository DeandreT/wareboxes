//! Scope-safe keyset reads for inventory holds.

use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::inventory::{
    InventoryBalanceStatus, InventoryHoldPage, InventoryHoldPageFilter, InventoryHoldQuantity,
    InventoryHoldReadModel, InventoryHoldReason, InventoryHoldSort, InventoryHoldStatus,
    MAX_INVENTORY_BALANCE_QUERY_LENGTH, MAX_INVENTORY_HOLD_PAGE_SIZE,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId, OwnerScope, SiteScope, TenantId};

use crate::db::{begin_tenant_transaction, Db};
use crate::{PersistenceError, PersistenceResult};

pub async fn get_inventory_hold_page(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    owner_scope: &OwnerScope,
    filter: InventoryHoldPageFilter,
) -> PersistenceResult<InventoryHoldPage> {
    validate_page_filter(&filter)?;
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
    let status = filter.status.map(InventoryHoldStatus::as_str);
    let fetch_limit = i64::from(filter.limit) + 1;
    let offset = i64::try_from(filter.offset)
        .map_err(|_| PersistenceError::invalid_input("inventory hold cursor is out of range"))?;
    let query_id = filter
        .query
        .as_deref()
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0);
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
          AND ($2::TEXT IS NULL OR hold.status = $2)
          AND ($3 OR hold.facility_id = ANY($4))
          AND ($5 OR hold.inventory_owner_id = ANY($6))
          AND (
              $7::TEXT IS NULL
              OR STRPOS(LOWER(owner.name), LOWER($7)) > 0
              OR STRPOS(LOWER(facility.name), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(location.name, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(location.barcode, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(plate.barcode, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(item.description, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(batch.lot, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(batch.serial, '')), LOWER($7)) > 0
              OR STRPOS(LOWER(hold.reason_code), LOWER($7)) > 0
              OR STRPOS(LOWER(COALESCE(hold.note, '')), LOWER($7)) > 0
              OR ($8::BIGINT IS NOT NULL AND (hold.id = $8 OR hold.inventory_balance_id = $8))
          )
        ORDER BY
          CASE WHEN $9='id' AND $10 THEN hold.id END ASC,
          CASE WHEN $9='id' AND NOT $10 THEN hold.id END DESC,
          CASE WHEN $9='item' AND $10 THEN LOWER(COALESCE(item.description,'')) END ASC,
          CASE WHEN $9='item' AND NOT $10 THEN LOWER(COALESCE(item.description,'')) END DESC,
          CASE WHEN $9='client' AND $10 THEN LOWER(owner.name) END ASC,
          CASE WHEN $9='client' AND NOT $10 THEN LOWER(owner.name) END DESC,
          CASE WHEN $9='position' AND $10 THEN LOWER(facility.name) END ASC,
          CASE WHEN $9='position' AND NOT $10 THEN LOWER(facility.name) END DESC,
          CASE WHEN $9='position' AND $10 THEN LOWER(COALESCE(location.barcode,location.name,'')) END ASC,
          CASE WHEN $9='position' AND NOT $10 THEN LOWER(COALESCE(location.barcode,location.name,'')) END DESC,
          CASE WHEN $9='reason' AND $10 THEN hold.reason_code END ASC,
          CASE WHEN $9='reason' AND NOT $10 THEN hold.reason_code END DESC,
          CASE WHEN $9='created' AND $10 THEN hold.created END ASC,
          CASE WHEN $9='created' AND NOT $10 THEN hold.created END DESC,
          CASE WHEN $9='quantity' AND $10 THEN hold.qty END ASC,
          CASE WHEN $9='quantity' AND NOT $10 THEN hold.qty END DESC,
          CASE WHEN $10 THEN hold.id END ASC,
          CASE WHEN NOT $10 THEN hold.id END DESC
        OFFSET $11 LIMIT $12
        "#,
    )
    .bind(tenant_id.get())
    .bind(status)
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .bind(filter.query.as_deref())
    .bind(query_id)
    .bind(sort_key(filter.sort))
    .bind(filter.direction.is_ascending())
    .bind(offset)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(filter.limit);
    let items = rows
        .iter()
        .take(usize::from(filter.limit))
        .map(map_hold)
        .collect::<PersistenceResult<Vec<_>>>()?;
    let next_offset = has_more.then_some(filter.offset + u64::from(filter.limit));
    tx.commit().await?;

    Ok(InventoryHoldPage { items, next_offset })
}

fn validate_page_filter(filter: &InventoryHoldPageFilter) -> PersistenceResult<()> {
    let _ = i64::try_from(filter.offset)
        .map_err(|_| PersistenceError::invalid_input("inventory hold cursor is out of range"))?;
    if !(1..=MAX_INVENTORY_HOLD_PAGE_SIZE).contains(&filter.limit) {
        return Err(PersistenceError::invalid_input(format!(
            "inventory hold page size must be between 1 and {MAX_INVENTORY_HOLD_PAGE_SIZE}"
        )));
    }
    if let Some(query) = filter.query.as_deref() {
        if query.is_empty() || query.trim() != query {
            return Err(PersistenceError::invalid_input(
                "inventory hold query must be nonempty and trimmed",
            ));
        }
        if query.chars().count() > MAX_INVENTORY_BALANCE_QUERY_LENGTH
            || query.chars().any(char::is_control)
        {
            return Err(PersistenceError::invalid_input(
                "inventory hold query is invalid",
            ));
        }
    }
    Ok(())
}

fn sort_key(sort: InventoryHoldSort) -> &'static str {
    match sort {
        InventoryHoldSort::Id => "id",
        InventoryHoldSort::Item => "item",
        InventoryHoldSort::Client => "client",
        InventoryHoldSort::Position => "position",
        InventoryHoldSort::Reason => "reason",
        InventoryHoldSort::Created => "created",
        InventoryHoldSort::Quantity => "quantity",
    }
}

fn map_hold(row: &PgRow) -> PersistenceResult<InventoryHoldReadModel> {
    let id: i64 = row.try_get("id")?;
    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| PersistenceError::invalid_data(error.to_string()))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?)
        .map_err(|error| PersistenceError::invalid_data(error.to_string()))?;
    let inventory_status_value: String = row.try_get("inventory_status")?;
    let inventory_status =
        InventoryBalanceStatus::parse(&inventory_status_value).ok_or_else(|| {
            PersistenceError::invalid_data(format!(
                "inventory hold {id} has unknown inventory status {inventory_status_value:?}"
            ))
        })?;
    let quantity_value: i64 = row.try_get("quantity")?;
    let quantity = InventoryHoldQuantity::new(quantity_value).ok_or_else(|| {
        PersistenceError::invalid_data(format!(
            "inventory hold {id} has nonpositive quantity {quantity_value}"
        ))
    })?;
    let reason_value: String = row.try_get("reason")?;
    let reason = InventoryHoldReason::parse(&reason_value).ok_or_else(|| {
        PersistenceError::invalid_data(format!(
            "inventory hold {id} has unknown reason {reason_value:?}"
        ))
    })?;
    let status_value: String = row.try_get("status")?;
    let status = InventoryHoldStatus::parse(&status_value).ok_or_else(|| {
        PersistenceError::invalid_data(format!(
            "inventory hold {id} has unknown status {status_value:?}"
        ))
    })?;

    Ok(InventoryHoldReadModel {
        id,
        created_at: row.try_get("created_at")?,
        created_by_user_id: row.try_get("created_by_user_id")?,
        released_at: row.try_get("released_at")?,
        released_by_user_id: row.try_get("released_by_user_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        inventory_owner_id,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id,
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
        inventory_status,
        quantity,
        reason,
        note: row.try_get("note")?,
        reference_type: row.try_get("reference_type")?,
        reference_id: row.try_get("reference_id")?,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_application::inventory::InventoryBalanceSortDirection;

    #[test]
    fn page_filter_validation_rejects_untrusted_direct_inputs() {
        assert!(validate_page_filter(&InventoryHoldPageFilter {
            offset: 0,
            limit: 1,
            status: Some(InventoryHoldStatus::Active),
            query: None,
            sort: InventoryHoldSort::Created,
            direction: InventoryBalanceSortDirection::Descending,
        })
        .is_ok());
        assert!(matches!(
            validate_page_filter(&InventoryHoldPageFilter {
                offset: u64::MAX,
                limit: 1,
                status: None,
                query: None,
                sort: InventoryHoldSort::Created,
                direction: InventoryBalanceSortDirection::Descending,
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_filter(&InventoryHoldPageFilter {
                offset: 0,
                limit: 0,
                status: None,
                query: None,
                sort: InventoryHoldSort::Created,
                direction: InventoryBalanceSortDirection::Descending,
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_filter(&InventoryHoldPageFilter {
                offset: 0,
                limit: MAX_INVENTORY_HOLD_PAGE_SIZE + 1,
                status: None,
                query: None,
                sort: InventoryHoldSort::Created,
                direction: InventoryBalanceSortDirection::Descending,
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_filter(&InventoryHoldPageFilter {
                offset: 0,
                limit: 1,
                status: None,
                query: Some(" untrimmed ".to_owned()),
                sort: InventoryHoldSort::Created,
                direction: InventoryBalanceSortDirection::Descending,
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
    }
}
