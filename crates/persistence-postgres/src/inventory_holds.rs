//! Scope-safe keyset reads for inventory holds.

use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::inventory::{
    InventoryBalanceStatus, InventoryHoldPage, InventoryHoldPageFilter, InventoryHoldQuantity,
    InventoryHoldReadModel, InventoryHoldReason, InventoryHoldStatus, MAX_INVENTORY_HOLD_PAGE_SIZE,
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
    validate_page_filter(filter)?;
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
          AND ($2::BIGINT IS NULL OR hold.id < $2)
          AND ($3::TEXT IS NULL OR hold.status = $3)
          AND ($4 OR hold.facility_id = ANY($5))
          AND ($6 OR hold.inventory_owner_id = ANY($7))
        ORDER BY hold.id DESC
        LIMIT $8
        "#,
    )
    .bind(tenant_id.get())
    .bind(filter.before_id)
    .bind(status)
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(filter.limit);
    let items = rows
        .iter()
        .take(usize::from(filter.limit))
        .map(map_hold)
        .collect::<PersistenceResult<Vec<_>>>()?;
    let next_before_id = if has_more {
        items.last().map(|hold| hold.id)
    } else {
        None
    };
    tx.commit().await?;

    Ok(InventoryHoldPage {
        items,
        next_before_id,
    })
}

fn validate_page_filter(filter: InventoryHoldPageFilter) -> PersistenceResult<()> {
    if filter.before_id.is_some_and(|id| id <= 0) {
        return Err(PersistenceError::invalid_input(
            "inventory hold cursor ID must be positive",
        ));
    }
    if !(1..=MAX_INVENTORY_HOLD_PAGE_SIZE).contains(&filter.limit) {
        return Err(PersistenceError::invalid_input(format!(
            "inventory hold page size must be between 1 and {MAX_INVENTORY_HOLD_PAGE_SIZE}"
        )));
    }
    Ok(())
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

    #[test]
    fn page_filter_validation_rejects_untrusted_direct_inputs() {
        assert!(validate_page_filter(InventoryHoldPageFilter {
            before_id: None,
            limit: 1,
            status: Some(InventoryHoldStatus::Active),
        })
        .is_ok());
        assert!(matches!(
            validate_page_filter(InventoryHoldPageFilter {
                before_id: Some(0),
                limit: 1,
                status: None,
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_filter(InventoryHoldPageFilter {
                before_id: None,
                limit: 0,
                status: None,
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_page_filter(InventoryHoldPageFilter {
                before_id: None,
                limit: MAX_INVENTORY_HOLD_PAGE_SIZE + 1,
                status: None,
            }),
            Err(PersistenceError::InvalidInput(_))
        ));
    }
}
