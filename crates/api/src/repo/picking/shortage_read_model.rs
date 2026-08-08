use sqlx::Row;
use wareboxes_application::picking::{
    PickShortageCursor, PickShortageHoldResult, PickShortagePage, PickShortagePageQuery,
    PickShortageQuery, PickShortageReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    ActualPickQuantity, FacilityId, InventoryBalanceId, InventoryHoldId, InventoryOwnerId,
    LicensePlateId, LocationId, OrderId, OrderLineId, OrderRevision, PickContentId, PickQuantity,
    PickScanValue, PickShortageDetails, PickShortageId, PickShortageNote, PickShortageQuantities,
    PickShortageReason, PickShortageRevision, PickShortageStatus, PickTaskId, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

const SHORTAGE_SELECT: &str = r#"
    SELECT shortage.id, shortage.revision, shortage.status,
           shortage.inventory_owner_id, inventory_owner.name AS inventory_owner_name,
           shortage.facility_id, facility.name AS facility_name,
           shortage.order_id, order_header.order_key, order_header.revision AS order_revision,
           shortage.order_item_id, shortage.task_id, shortage.pick_task_content_id,
           shortage.source_inventory_balance_id, shortage.source_location_id,
           source_location.barcode AS source_location_barcode,
           source_location.name AS source_location_name,
           shortage.source_license_plate_id,
           source_plate.barcode AS source_license_plate_barcode,
           shortage.item_id, item.description AS item_description,
           shortage.uom, batch.lot, batch.serial, batch.expiration,
           shortage.planned_qty, shortage.picked_qty, shortage.short_qty,
           shortage.reallocated_qty, shortage.recovery_terminal_qty,
           shortage.remaining_to_allocate_qty, shortage.observed_item_barcode,
           shortage.observed_lot, shortage.observed_serial,
           shortage.reason_code, shortage.note, shortage.inventory_hold_id,
           shortage.source_inventory_balance_id AS hold_balance_id,
           shortage.short_qty AS held_quantity, shortage.reported_by_user_id,
           shortage.reported_at, shortage.resolved_at
    FROM pick_shortages shortage
    INNER JOIN inventory_owners inventory_owner
      ON inventory_owner.tenant_id = shortage.tenant_id
     AND inventory_owner.id = shortage.inventory_owner_id
    INNER JOIN facilities facility
      ON facility.tenant_id = shortage.tenant_id
     AND facility.id = shortage.facility_id
    INNER JOIN orders order_header
      ON order_header.tenant_id = shortage.tenant_id
     AND order_header.inventory_owner_id = shortage.inventory_owner_id
     AND order_header.id = shortage.order_id
    INNER JOIN locations source_location
      ON source_location.tenant_id = shortage.tenant_id
     AND source_location.facility_id = shortage.facility_id
     AND source_location.id = shortage.source_location_id
    INNER JOIN item_batches batch
      ON batch.tenant_id = shortage.tenant_id
     AND batch.inventory_owner_id = shortage.inventory_owner_id
     AND batch.id = shortage.item_batch_id
    INNER JOIN items item
      ON item.tenant_id = shortage.tenant_id AND item.id = shortage.item_id
    LEFT JOIN license_plates source_plate
      ON source_plate.tenant_id = shortage.tenant_id
     AND source_plate.inventory_owner_id = shortage.inventory_owner_id
     AND source_plate.facility_id = shortage.facility_id
     AND source_plate.id = shortage.source_license_plate_id
"#;

pub async fn get_shortage(
    db: &Db,
    access: &TenantAccess,
    query: PickShortageQuery,
) -> AppResult<PickShortageReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    let sql = format!("{SHORTAGE_SELECT} WHERE shortage.tenant_id = $1 AND shortage.id = $2");
    let row = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(query.shortage_id.get())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::not_found("pick shortage"))?;
    let model = map_shortage(row, &scope)?;
    tx.commit().await?;
    Ok(model)
}

pub async fn list_shortages(
    db: &Db,
    access: &TenantAccess,
    query: PickShortagePageQuery,
) -> AppResult<PickShortagePage> {
    if query.limit == 0 || query.limit > 100 {
        return Err(AppError::bad_request(
            "pick shortage page limit must be between 1 and 100",
        ));
    }
    if query.cursor.is_some_and(|cursor| {
        cursor.shortage_id.get() <= 0 || cursor.reported_at.timestamp_micros() == i64::MAX
    }) {
        return Err(AppError::bad_request("pick shortage cursor is invalid"));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    if query
        .facility_id
        .is_some_and(|id| !scope.includes_facility(id.get()))
        || query
            .inventory_owner_id
            .is_some_and(|id| !scope.includes_inventory_owner(id.get()))
    {
        return Err(AppError::not_found("pick shortages"));
    }
    let sql = format!(
        r#"{SHORTAGE_SELECT}
        WHERE shortage.tenant_id = $1
          AND ($2 OR shortage.facility_id = ANY($3))
          AND ($4 OR shortage.inventory_owner_id = ANY($5))
          AND ($6::BIGINT IS NULL OR shortage.facility_id = $6)
          AND ($7::BIGINT IS NULL OR shortage.inventory_owner_id = $7)
          AND ($8::BIGINT IS NULL OR shortage.order_id = $8)
          AND (($9::TEXT IS NULL AND shortage.status <> 'resolved') OR shortage.status = $9)
          AND ($10::TIMESTAMPTZ IS NULL OR (shortage.reported_at, shortage.id) > ($10, $11))
        ORDER BY shortage.reported_at, shortage.id
        LIMIT $12"#
    );
    let fetch_limit = i64::from(query.limit) + 1;
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.facility_id.map(|id| id.get()))
        .bind(query.inventory_owner_id.map(|id| id.get()))
        .bind(query.order_id.map(|id| id.get()))
        .bind(query.status.map(PickShortageStatus::as_str))
        .bind(query.cursor.map(|cursor| cursor.reported_at))
        .bind(query.cursor.map(|cursor| cursor.shortage_id.get()))
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await?;
    let mut items = rows
        .into_iter()
        .map(|row| map_shortage(row, &scope))
        .collect::<AppResult<Vec<_>>>()?;
    let has_more = items.len() > usize::from(query.limit);
    if has_more {
        items.pop();
    }
    let next_cursor = has_more
        .then(|| {
            items.last().map(|item| PickShortageCursor {
                reported_at: item.reported_at,
                shortage_id: item.shortage_id,
            })
        })
        .flatten();
    tx.commit().await?;
    Ok(PickShortagePage { items, next_cursor })
}

fn map_shortage(
    row: sqlx::postgres::PgRow,
    scope: &ScopeBindings,
) -> AppResult<PickShortageReadModel> {
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    if !scope.includes_inventory_owner(owner_id) || !scope.includes_facility(facility_id) {
        return Err(AppError::not_found("pick shortage"));
    }
    let status = PickShortageStatus::parse(&row.try_get::<String, _>("status")?)
        .ok_or_else(|| AppError::internal("pick shortage has invalid status"))?;
    let reason = PickShortageReason::parse(&row.try_get::<String, _>("reason_code")?)
        .ok_or_else(|| AppError::internal("pick shortage has invalid reason"))?;
    let note = row
        .try_get::<Option<String>, _>("note")?
        .map(PickShortageNote::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let details = PickShortageDetails::new(reason, note)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let planned = PickQuantity::new(row.try_get("planned_qty")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let picked = ActualPickQuantity::new(row.try_get("picked_qty")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let quantities = PickShortageQuantities::new(planned, picked)
        .map_err(|error| AppError::internal(error.to_string()))?;
    if quantities.short().get() != row.try_get::<i64, _>("short_qty")? {
        return Err(AppError::internal(
            "pick shortage quantities do not conserve",
        ));
    }
    let model = PickShortageReadModel {
        shortage_id: PickShortageId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        shortage_revision: PickShortageRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status,
        inventory_owner_id: InventoryOwnerId::new(owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(facility_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_key: row.try_get("order_key")?,
        order_revision: OrderRevision::new(row.try_get("order_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_line_id: OrderLineId::new(row.try_get("order_item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        task_id: PickTaskId::new(row.try_get("task_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        content_id: PickContentId::new(row.try_get("pick_task_content_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_inventory_balance_id: InventoryBalanceId::new(
            row.try_get("source_inventory_balance_id")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_id: LocationId::new(row.try_get("source_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_barcode: PickScanValue::new(
            row.try_get::<Option<String>, _>("source_location_barcode")?
                .ok_or_else(|| AppError::internal("pick shortage source has no barcode"))?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_name: row.try_get("source_location_name")?,
        source_license_plate_id: row
            .try_get::<Option<i64>, _>("source_license_plate_id")?
            .map(LicensePlateId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_license_plate_barcode: row
            .try_get::<Option<String>, _>("source_license_plate_barcode")?
            .map(PickScanValue::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        quantities,
        reallocated_quantity: ActualPickQuantity::new(row.try_get("reallocated_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        recovery_terminal_quantity: ActualPickQuantity::new(row.try_get("recovery_terminal_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        remaining_to_allocate_quantity: ActualPickQuantity::new(
            row.try_get("remaining_to_allocate_qty")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        observed_item_barcode: optional_scan(&row, "observed_item_barcode")?,
        observed_lot: optional_scan(&row, "observed_lot")?,
        observed_serial: optional_scan(&row, "observed_serial")?,
        details,
        hold: PickShortageHoldResult {
            hold_id: InventoryHoldId::new(row.try_get("inventory_hold_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            inventory_balance_id: InventoryBalanceId::new(row.try_get("hold_balance_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            held_quantity: PickQuantity::new(row.try_get("held_quantity")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
        },
        reported_by: UserId::new(row.try_get("reported_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        reported_at: row.try_get("reported_at")?,
        resolved_at: row.try_get("resolved_at")?,
    };
    if !model.recovery_quantities_are_consistent() {
        return Err(AppError::internal(
            "pick shortage recovery quantities are inconsistent",
        ));
    }
    Ok(model)
}

fn optional_scan(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<Option<PickScanValue>> {
    row.try_get::<Option<String>, _>(column)?
        .map(PickScanValue::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))
}
