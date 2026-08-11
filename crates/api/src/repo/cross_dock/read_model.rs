use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::cross_dock::{
    CrossDockLocationReadModel, CrossDockPlanningOptionPage, CrossDockPlanningOptionPageFilter,
    CrossDockPlanningOptionReadModel, CrossDockWorkPage, CrossDockWorkPageFilter,
    CrossDockWorkReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, CrossDockPlanId, CrossDockQuantity, CrossDockScanValue, CrossDockUom,
    CrossDockWorkId, CrossDockWorkStatus, FacilityId, InboundLoadId, InventoryBalanceId,
    InventoryOwnerId, LocationId, OrderId, OrderLineId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, require_permission_tx};

pub async fn planning_option_page(
    db: &Db,
    access: &TenantAccess,
    filter: CrossDockPlanningOptionPageFilter,
) -> AppResult<CrossDockPlanningOptionPage> {
    if filter.limit == 0 || filter.limit > 100 {
        return Err(AppError::bad_request(
            "cross-dock planning option limit must be 1 through 100",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    let rows = sqlx::query(
        r#"
        WITH allocated AS (
          SELECT reservation_id,SUM(qty)::bigint quantity
          FROM inventory_allocations
          WHERE tenant_id=$1 AND status='allocated' AND deleted IS NULL
          GROUP BY reservation_id
        ), active_cross_dock AS (
          SELECT detail.reservation_id,SUM(detail.planned_quantity)::bigint quantity
          FROM cross_dock_tasks detail
          JOIN work_tasks work ON work.tenant_id=detail.tenant_id AND work.id=detail.task_id
          WHERE detail.tenant_id=$1 AND detail.closed_at IS NULL
            AND work.status IN ('open','assigned','in_progress')
          GROUP BY detail.reservation_id
        ), receipt_committed AS (
          SELECT plan.source_receipt_inventory_transaction_id,
                 SUM(plan.planned_quantity)::bigint quantity
          FROM cross_dock_plan_runs plan
          JOIN cross_dock_tasks detail
            ON detail.tenant_id=plan.tenant_id AND detail.plan_run_id=plan.id
          JOIN work_tasks work ON work.tenant_id=detail.tenant_id AND work.id=detail.task_id
          WHERE plan.tenant_id=$1 AND work.status<>'cancelled'
          GROUP BY plan.source_receipt_inventory_transaction_id
        )
        SELECT orders.id AS order_id,orders.order_key,orders.revision AS order_revision,
               line.id AS order_item_id,line.line_key,reservation.id AS reservation_id,
               reservation.inventory_owner_id,owner.name AS inventory_owner_name,
               reservation.facility_id,facility.name AS facility_name,
               line.item_id,item.description AS item_description,
               (SELECT barcode.name FROM barcodes barcode
                WHERE barcode.tenant_id=line.tenant_id AND barcode.item_id=line.item_id
                  AND barcode.deleted IS NULL ORDER BY barcode.id LIMIT 1) AS primary_sku,
               line.uom,entry.lot,entry.serial,entry.expiration,
               reservation.qty-COALESCE(allocated.quantity,0)-COALESCE(active_cross_dock.quantity,0)
                 AS unallocated_quantity,
               transaction.id AS receipt_transaction_id,batch.load_id AS inbound_load_id,
               load.reference_number AS inbound_load_reference,balance.id AS source_balance_id,
               source.id AS source_location_id,source.barcode AS source_barcode,
               source.name AS source_name,
               balance.qty_on_hand-balance.qty_reserved-balance.qty_held AS source_free_quantity,
               entry.quantity_delta-COALESCE(receipt_committed.quantity,0)
                 AS receipt_remaining_quantity
        FROM orders
        JOIN order_items line ON line.tenant_id=orders.tenant_id
          AND line.inventory_owner_id=orders.inventory_owner_id AND line.order_id=orders.id
          AND line.deleted IS NULL
        JOIN inventory_reservations reservation ON reservation.tenant_id=line.tenant_id
          AND reservation.inventory_owner_id=line.inventory_owner_id
          AND reservation.order_id=line.order_id AND reservation.order_item_id=line.id
          AND reservation.status='active' AND reservation.deleted IS NULL
        JOIN inventory_owners owner ON owner.tenant_id=reservation.tenant_id
          AND owner.id=reservation.inventory_owner_id AND owner.deleted IS NULL
        JOIN facilities facility ON facility.tenant_id=reservation.tenant_id
          AND facility.id=reservation.facility_id AND facility.deleted IS NULL
        JOIN items item ON item.tenant_id=line.tenant_id AND item.id=line.item_id
        JOIN inventory_transactions transaction ON transaction.tenant_id=reservation.tenant_id
          AND transaction.inventory_owner_id=reservation.inventory_owner_id
          AND transaction.transaction_type='receive'
          AND transaction.operation='inbound.receive_expected_inventory.v1'
          AND transaction.reference_type='load_line'
        JOIN inventory_entries entry ON entry.tenant_id=transaction.tenant_id
          AND entry.inventory_owner_id=transaction.inventory_owner_id
          AND entry.transaction_id=transaction.id AND entry.quantity_delta>0
          AND entry.facility_id=reservation.facility_id
          AND entry.item_id=line.item_id AND entry.uom=line.uom AND entry.status='available'
        JOIN item_batches batch ON batch.tenant_id=entry.tenant_id
          AND batch.inventory_owner_id=entry.inventory_owner_id AND batch.id=entry.item_batch_id
          AND batch.load_id IS NOT NULL
        JOIN loads load ON load.tenant_id=batch.tenant_id AND load.id=batch.load_id
        JOIN inventory_balances balance ON balance.tenant_id=entry.tenant_id
          AND balance.inventory_owner_id=entry.inventory_owner_id
          AND balance.facility_id=entry.facility_id AND balance.location_id=entry.location_id
          AND balance.item_batch_id=entry.item_batch_id AND balance.uom=entry.uom
          AND balance.status=entry.status AND balance.license_plate_id IS NULL
          AND balance.deleted IS NULL
        JOIN locations source ON source.tenant_id=balance.tenant_id
          AND source.facility_id=balance.facility_id AND source.id=balance.location_id
          AND source.deleted IS NULL AND source.active AND source.receivable
          AND NULLIF(BTRIM(source.barcode),'') IS NOT NULL
        LEFT JOIN allocated ON allocated.reservation_id=reservation.id
        LEFT JOIN active_cross_dock ON active_cross_dock.reservation_id=reservation.id
        LEFT JOIN receipt_committed
          ON receipt_committed.source_receipt_inventory_transaction_id=transaction.id
        WHERE orders.tenant_id=$1 AND orders.status='open' AND orders.deleted IS NULL
          AND ($2 OR reservation.facility_id=ANY($3))
          AND ($4 OR reservation.inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR reservation.facility_id=$6)
          AND ($7::bigint IS NULL OR reservation.inventory_owner_id=$7)
          AND reservation.qty-COALESCE(allocated.quantity,0)-COALESCE(active_cross_dock.quantity,0)>0
          AND balance.qty_on_hand-balance.qty_reserved-balance.qty_held>0
          AND entry.quantity_delta-COALESCE(receipt_committed.quantity,0)>0
          AND (SELECT COUNT(*) FROM inventory_entries counted
               WHERE counted.tenant_id=transaction.tenant_id
                 AND counted.inventory_owner_id=transaction.inventory_owner_id
                 AND counted.transaction_id=transaction.id)=1
          AND NOT EXISTS (
            SELECT 1 FROM loose_inventory_movement_claims claim
            WHERE claim.tenant_id=balance.tenant_id
              AND claim.inventory_owner_id=balance.inventory_owner_id
              AND claim.source_inventory_balance_id=balance.id AND claim.released_at IS NULL)
          AND EXISTS (
            SELECT 1 FROM locations destination
            WHERE destination.tenant_id=reservation.tenant_id
              AND destination.facility_id=reservation.facility_id
              AND destination.deleted IS NULL AND destination.active
              AND destination.pickable AND NOT destination.receivable
              AND NULLIF(BTRIM(destination.barcode),'') IS NOT NULL)
        ORDER BY orders.rush DESC,orders.ship_by ASC NULLS LAST,
                 orders.id,line.line_number,line.id,transaction.id
        OFFSET $8 LIMIT $9
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(filter.inventory_owner_id.map(|id| id.get()))
    .bind(i64::try_from(filter.offset).map_err(|_| AppError::bad_request("invalid cross-dock planning cursor"))?)
    .bind(i64::from(filter.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(filter.limit);
    let mut items = Vec::with_capacity(rows.len().min(usize::from(filter.limit)));
    let mut destinations_by_facility = HashMap::<i64, Vec<CrossDockLocationReadModel>>::new();
    for row in rows.into_iter().take(usize::from(filter.limit)) {
        let facility_id = id(&row, "facility_id", FacilityId::new)?;
        if let std::collections::hash_map::Entry::Vacant(entry) =
            destinations_by_facility.entry(facility_id.get())
        {
            let destination_rows = sqlx::query(
                r#"SELECT id AS destination_location_id,barcode AS destination_barcode,
                          name AS destination_name
                   FROM locations
                   WHERE tenant_id=$1 AND facility_id=$2 AND deleted IS NULL AND active
                     AND pickable AND NOT receivable AND NULLIF(BTRIM(barcode),'') IS NOT NULL
                   ORDER BY name NULLS LAST,barcode,id"#,
            )
            .bind(access.tenant_id.get())
            .bind(facility_id.get())
            .fetch_all(&mut *tx)
            .await?;
            let destinations = destination_rows
                .iter()
                .map(|destination| location(destination, "destination"))
                .collect::<AppResult<Vec<_>>>()?;
            entry.insert(destinations);
        }
        let unallocated_quantity: i64 = row.try_get("unallocated_quantity")?;
        let source_free_quantity: i64 = row.try_get("source_free_quantity")?;
        let receipt_remaining_quantity: i64 = row.try_get("receipt_remaining_quantity")?;
        items.push(CrossDockPlanningOptionReadModel {
            order_id: id(&row, "order_id", OrderId::new)?,
            order_key: row.try_get("order_key")?,
            order_line_id: id(&row, "order_item_id", OrderLineId::new)?,
            order_line_key: row.try_get("line_key")?,
            order_revision: wareboxes_domain::OrderRevision::new(row.try_get("order_revision")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            inventory_owner_id: id(&row, "inventory_owner_id", InventoryOwnerId::new)?,
            inventory_owner_name: row.try_get("inventory_owner_name")?,
            facility_id,
            facility_name: row.try_get("facility_name")?,
            reservation_id: row.try_get("reservation_id")?,
            item_id: id(&row, "item_id", CatalogItemId::new)?,
            item_description: row.try_get("item_description")?,
            primary_sku: row.try_get("primary_sku")?,
            uom: CrossDockUom::new(row.try_get::<String, _>("uom")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            lot: row.try_get("lot")?,
            serial: row.try_get("serial")?,
            expiration: row.try_get("expiration")?,
            unallocated_quantity,
            source_receipt_inventory_transaction_id: row.try_get("receipt_transaction_id")?,
            inbound_load_id: id(&row, "inbound_load_id", InboundLoadId::new)?,
            inbound_load_reference: row.try_get("inbound_load_reference")?,
            source_inventory_balance_id: id(&row, "source_balance_id", InventoryBalanceId::new)?,
            source_receiving_location: location(&row, "source")?,
            source_free_quantity,
            receipt_remaining_quantity,
            maximum_plan_quantity: unallocated_quantity
                .min(source_free_quantity)
                .min(receipt_remaining_quantity),
            destination_pick_faces: destinations_by_facility
                .get(&facility_id.get())
                .cloned()
                .unwrap_or_default(),
        });
    }
    tx.commit().await?;
    Ok(CrossDockPlanningOptionPage {
        items,
        next_offset: has_more.then(|| filter.offset + u64::from(filter.limit)),
    })
}

pub async fn work_page(
    db: &Db,
    access: &TenantAccess,
    filter: CrossDockWorkPageFilter,
) -> AppResult<CrossDockWorkPage> {
    if filter.limit == 0 || filter.limit > 100 {
        return Err(AppError::bad_request(
            "cross-dock page limit must be 1 through 100",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    let rows = sqlx::query(
        r#"SELECT work.id AS work_id,detail.plan_run_id,work.status,
                  detail.inventory_owner_id,owner.name AS inventory_owner_name,
                  detail.facility_id,facility.name AS facility_name,detail.inbound_load_id,
                  detail.order_id,orders.order_key,orders.revision AS order_revision,
                  detail.order_item_id,line.line_key,
                  detail.reservation_id,work.priority,detail.item_id,item.description AS item_description,
                  (SELECT barcode.name FROM barcodes barcode WHERE barcode.tenant_id=detail.tenant_id
                     AND barcode.item_id=detail.item_id AND barcode.deleted IS NULL
                   ORDER BY barcode.id LIMIT 1) AS primary_sku,
                  detail.uom,detail.lot,detail.serial,detail.expiration,detail.planned_quantity,
                  detail.source_inventory_balance_id,
                  source.id AS source_location_id,source.barcode AS source_barcode,source.name AS source_name,
                  destination.id AS destination_location_id,destination.barcode AS destination_barcode,
                  destination.name AS destination_name,work.assigned_user_id,work.lease_expires_at,
                  work.due_at,work.created,work.completed_at
           FROM work_tasks work
           JOIN cross_dock_tasks detail ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id
           JOIN inventory_owners owner ON owner.tenant_id=detail.tenant_id AND owner.id=detail.inventory_owner_id
           JOIN facilities facility ON facility.tenant_id=detail.tenant_id AND facility.id=detail.facility_id
           JOIN orders ON orders.tenant_id=detail.tenant_id AND orders.inventory_owner_id=detail.inventory_owner_id
             AND orders.id=detail.order_id
           JOIN order_items line ON line.tenant_id=detail.tenant_id AND line.inventory_owner_id=detail.inventory_owner_id
             AND line.order_id=detail.order_id AND line.id=detail.order_item_id
           JOIN items item ON item.tenant_id=detail.tenant_id AND item.id=detail.item_id
           JOIN locations source ON source.tenant_id=detail.tenant_id AND source.id=detail.source_location_id
           JOIN locations destination ON destination.tenant_id=detail.tenant_id AND destination.id=detail.destination_location_id
           WHERE work.tenant_id=$1 AND work.task_type='cross_dock' AND work.deleted IS NULL
             AND ($2 OR detail.facility_id=ANY($3)) AND ($4 OR detail.inventory_owner_id=ANY($5))
             AND ($6::BIGINT IS NULL OR detail.facility_id=$6)
             AND ($7::BIGINT IS NULL OR detail.inventory_owner_id=$7)
             AND ($8::BIGINT IS NULL OR detail.order_id=$8)
             AND ($9::TEXT IS NULL OR CASE WHEN work.status IN ('open','assigned') THEN 'pending' ELSE work.status END=$9)
           ORDER BY CASE WHEN work.status IN ('open','assigned','in_progress') THEN 0 ELSE 1 END,
                    work.priority DESC,work.due_at ASC NULLS LAST,work.created,work.id
           OFFSET $10 LIMIT $11"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities).bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners).bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(filter.inventory_owner_id.map(|id| id.get()))
    .bind(filter.order_id.map(|id| id.get()))
    .bind(filter.status.map(|status| status.as_str()))
    .bind(i64::try_from(filter.offset).map_err(|_| AppError::bad_request("invalid cross-dock cursor"))?)
    .bind(i64::from(filter.limit) + 1)
    .fetch_all(&mut *tx).await?;
    let has_more = rows.len() > usize::from(filter.limit);
    let mut items = Vec::with_capacity(rows.len().min(usize::from(filter.limit)));
    for row in rows.into_iter().take(usize::from(filter.limit)) {
        items.push(map_row(&row)?);
    }
    tx.commit().await?;
    Ok(CrossDockWorkPage {
        next_offset: has_more.then(|| filter.offset + u64::from(filter.limit)),
        items,
    })
}

fn map_row(row: &sqlx::postgres::PgRow) -> AppResult<CrossDockWorkReadModel> {
    let status_text: String = row.try_get("status")?;
    Ok(CrossDockWorkReadModel {
        work_id: id(row, "work_id", CrossDockWorkId::new)?,
        plan_id: id(row, "plan_run_id", CrossDockPlanId::new)?,
        status: CrossDockWorkStatus::parse(&status_text)
            .ok_or_else(|| AppError::internal("invalid cross-dock work status"))?,
        inventory_owner_id: id(row, "inventory_owner_id", InventoryOwnerId::new)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: id(row, "facility_id", FacilityId::new)?,
        facility_name: row.try_get("facility_name")?,
        inbound_load_id: id(row, "inbound_load_id", InboundLoadId::new)?,
        order_id: id(row, "order_id", OrderId::new)?,
        order_key: row.try_get("order_key")?,
        order_revision: wareboxes_domain::OrderRevision::new(row.try_get("order_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_line_id: id(row, "order_item_id", OrderLineId::new)?,
        order_line_key: row.try_get("line_key")?,
        reservation_id: row.try_get("reservation_id")?,
        priority: row.try_get("priority")?,
        item_id: id(row, "item_id", CatalogItemId::new)?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        uom: CrossDockUom::new(row.try_get::<String, _>("uom")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        quantity: CrossDockQuantity::new(row.try_get("planned_quantity")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_inventory_balance_id: id(
            row,
            "source_inventory_balance_id",
            InventoryBalanceId::new,
        )?,
        source_receiving_location: location(row, "source")?,
        destination_pick_face: location(row, "destination")?,
        claimed_by: row
            .try_get::<Option<i64>, _>("assigned_user_id")?
            .map(wareboxes_domain::UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        due_at: row.try_get("due_at")?,
        created_at: row.try_get("created")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn location(row: &sqlx::postgres::PgRow, prefix: &str) -> AppResult<CrossDockLocationReadModel> {
    Ok(CrossDockLocationReadModel {
        location_id: LocationId::new(
            row.try_get::<i64, _>(format!("{prefix}_location_id").as_str())?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        barcode: CrossDockScanValue::new(
            row.try_get::<String, _>(format!("{prefix}_barcode").as_str())?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        name: row.try_get(format!("{prefix}_name").as_str())?,
    })
}

fn id<T, E>(
    row: &sqlx::postgres::PgRow,
    column: &str,
    constructor: fn(i64) -> Result<T, E>,
) -> AppResult<T>
where
    E: std::fmt::Display,
{
    constructor(row.try_get(column)?).map_err(|error| AppError::internal(error.to_string()))
}
