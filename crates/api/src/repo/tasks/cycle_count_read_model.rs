//! Scope-bound supervisor views for cycle-count planning and execution.

use sqlx::Row;
use wareboxes_application::cycle_count::{
    CycleCountCandidatePage, CycleCountCandidateQuery, CycleCountCandidateReadModel,
    CycleCountCandidateSort, CycleCountCursor, CycleCountLocationReadModel,
    CycleCountSortDirection, CycleCountStockReadModel, CycleCountWorkPage, CycleCountWorkQuery,
    CycleCountWorkReadModel, CycleCountWorkSort, CycleCountWorkStatus,
};
use wareboxes_application::inventory::InventoryBalanceStatus;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::{bind_tenant_context, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

const PERMISSION: &str = "wms_supervisor";

pub async fn cycle_count_candidate_page(
    db: &Db,
    access: &TenantAccess,
    query: CycleCountCandidateQuery,
) -> AppResult<CycleCountCandidatePage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;

    let sort = candidate_sort_expression(query.sort);
    let direction = sort_direction(query.direction);
    let sql = format!(
        r#"
        SELECT balance.id AS inventory_balance_id,
               balance.inventory_owner_id,
               owner.name AS inventory_owner_name,
               balance.facility_id,
               facility.name AS facility_name,
               balance.location_id,
               location.barcode AS location_barcode,
               location.name AS location_name,
               balance.item_batch_id,
               balance.item_id,
               item.description AS item_description,
               sku.name AS primary_sku,
               balance.license_plate_id,
               plate.barcode AS license_plate_barcode,
               balance.uom,
               batch.lot,
               batch.expiration,
               batch.serial,
               balance.status AS inventory_status,
               balance.qty_on_hand,
               balance.qty_reserved,
               balance.qty_held,
               last_count.confirmed_at AS last_counted_at,
               last_count.variance_qty AS last_variance_quantity
        FROM inventory_balances balance
        JOIN inventory_owners owner
          ON owner.tenant_id=balance.tenant_id
         AND owner.id=balance.inventory_owner_id
         AND owner.deleted IS NULL
        JOIN facilities facility
          ON facility.tenant_id=balance.tenant_id
         AND facility.id=balance.facility_id
         AND facility.deleted IS NULL
        JOIN locations location
          ON location.tenant_id=balance.tenant_id
         AND location.facility_id=balance.facility_id
         AND location.id=balance.location_id
         AND location.deleted IS NULL
         AND location.active
        JOIN item_batches batch
          ON batch.tenant_id=balance.tenant_id
         AND batch.inventory_owner_id=balance.inventory_owner_id
         AND batch.id=balance.item_batch_id
         AND batch.deleted IS NULL
        JOIN items item
          ON item.tenant_id=balance.tenant_id
         AND item.id=balance.item_id
         AND item.deleted IS NULL
        LEFT JOIN license_plates plate
          ON plate.tenant_id=balance.tenant_id
         AND plate.inventory_owner_id=balance.inventory_owner_id
         AND plate.facility_id=balance.facility_id
         AND plate.id=balance.license_plate_id
        LEFT JOIN LATERAL (
            SELECT barcode.name
            FROM barcodes barcode
            WHERE barcode.tenant_id=balance.tenant_id
              AND barcode.item_id=balance.item_id
              AND barcode.deleted IS NULL
            ORDER BY barcode.id
            LIMIT 1
        ) sku ON true
        LEFT JOIN LATERAL (
            SELECT result.confirmed_at, result.variance_qty
            FROM cycle_count_item_location_results result
            WHERE result.tenant_id=balance.tenant_id
              AND result.inventory_owner_id=balance.inventory_owner_id
              AND result.inventory_balance_id=balance.id
            ORDER BY result.confirmed_at DESC, result.task_id DESC
            LIMIT 1
        ) last_count ON true
        WHERE balance.tenant_id=$1
          AND balance.deleted IS NULL
          AND balance.qty_on_hand>0
          AND ($2 OR balance.facility_id=ANY($3))
          AND ($4 OR balance.inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR balance.facility_id=$6)
          AND ($7::bigint IS NULL OR balance.inventory_owner_id=$7)
          AND ($8::text IS NULL OR balance.status=$8)
          AND NOT EXISTS (
              SELECT 1
              FROM cycle_count_item_location_tasks detail
              JOIN work_tasks task
                ON task.tenant_id=detail.tenant_id
               AND task.id=detail.task_id
              WHERE detail.tenant_id=balance.tenant_id
                AND detail.inventory_owner_id=balance.inventory_owner_id
                AND detail.inventory_balance_id=balance.id
                AND task.deleted IS NULL
                AND task.task_type='cycle_count_item_location'
                AND task.status IN ('open','assigned','in_progress')
          )
        ORDER BY {sort} {direction} NULLS FIRST, balance.id
        OFFSET $9 LIMIT $10
        "#,
    );
    let offset = checked_offset(query.cursor, "cycle-count candidate")?;
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.facility_id.map(FacilityId::get))
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.inventory_status.map(inventory_status_value))
        .bind(offset)
        .bind(i64::from(query.limit) + 1)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .into_iter()
        .take(usize::from(query.limit))
        .map(map_candidate)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = next_cursor(has_more, offset, query.limit, "cycle-count candidate")?;
    tx.commit().await?;
    Ok(CycleCountCandidatePage { items, next_cursor })
}

pub async fn cycle_count_work_page(
    db: &Db,
    access: &TenantAccess,
    query: CycleCountWorkQuery,
) -> AppResult<CycleCountWorkPage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;

    let sort = work_sort_expression(query.sort);
    let direction = sort_direction(query.direction);
    let sql = format!(
        r#"
        WITH work AS (
          SELECT task.id AS task_id,
                 CASE task.status
                   WHEN 'open' THEN 'pending'
                   WHEN 'assigned' THEN 'pending'
                   WHEN 'in_progress' THEN 'claimed'
                   ELSE task.status
                 END::text AS public_status,
                 detail.inventory_owner_id,
                 owner.name AS inventory_owner_name,
                 detail.facility_id,
                 facility.name AS facility_name,
                 detail.location_id,
                 location.barcode AS location_barcode,
                 location.name AS location_name,
                 detail.inventory_balance_id,
                 COALESCE(result.item_batch_id,balance.item_batch_id) AS item_batch_id,
                 detail.item_id,
                 item.description AS item_description,
                 sku.name AS primary_sku,
                 COALESCE(result.license_plate_id,balance.license_plate_id) AS license_plate_id,
                 plate.barcode AS license_plate_barcode,
                 COALESCE(result.uom,balance.uom) AS uom,
                 COALESCE(result.lot,batch.lot) AS lot,
                 COALESCE(result.expiration,batch.expiration) AS expiration,
                 COALESCE(result.serial,batch.serial) AS serial,
                 COALESCE(result.status,balance.status) AS inventory_status,
                 CASE WHEN balance.deleted IS NULL THEN balance.qty_on_hand END
                   AS current_qty_on_hand,
                 CASE WHEN balance.deleted IS NULL THEN balance.qty_reserved END
                   AS current_qty_reserved,
                 CASE WHEN balance.deleted IS NULL THEN balance.qty_held END
                   AS current_qty_held,
                 result.system_qty_on_hand,
                 result.system_qty_reserved,
                 result.system_qty_held,
                 result.counted_qty,
                 result.variance_qty,
                 result.inventory_transaction_id,
                 task.priority,
                 detail.note,
                 task.assigned_user_id,
                 task.lease_expires_at,
                 task.due_at,
                 task.created,
                 task.completed_at,
                 result.confirmed_by,
                 result.confirmed_at
          FROM work_tasks task
          JOIN cycle_count_item_location_tasks detail
            ON detail.tenant_id=task.tenant_id
           AND detail.task_id=task.id
          JOIN inventory_owners owner
            ON owner.tenant_id=detail.tenant_id
           AND owner.id=detail.inventory_owner_id
          JOIN facilities facility
            ON facility.tenant_id=detail.tenant_id
           AND facility.id=detail.facility_id
          JOIN locations location
            ON location.tenant_id=detail.tenant_id
           AND location.facility_id=detail.facility_id
           AND location.id=detail.location_id
          LEFT JOIN inventory_balances balance
            ON balance.tenant_id=detail.tenant_id
           AND balance.inventory_owner_id=detail.inventory_owner_id
           AND balance.facility_id=detail.facility_id
           AND balance.id=detail.inventory_balance_id
          LEFT JOIN cycle_count_item_location_results result
            ON result.tenant_id=detail.tenant_id
           AND result.task_id=detail.task_id
          LEFT JOIN item_batches batch
            ON batch.tenant_id=detail.tenant_id
           AND batch.inventory_owner_id=detail.inventory_owner_id
           AND batch.id=COALESCE(result.item_batch_id,balance.item_batch_id)
          JOIN items item
            ON item.tenant_id=detail.tenant_id
           AND item.id=detail.item_id
          LEFT JOIN license_plates plate
            ON plate.tenant_id=detail.tenant_id
           AND plate.inventory_owner_id=detail.inventory_owner_id
           AND plate.facility_id=detail.facility_id
           AND plate.id=COALESCE(result.license_plate_id,balance.license_plate_id)
          LEFT JOIN LATERAL (
              SELECT barcode.name
              FROM barcodes barcode
              WHERE barcode.tenant_id=detail.tenant_id
                AND barcode.item_id=detail.item_id
                AND barcode.deleted IS NULL
              ORDER BY barcode.id LIMIT 1
          ) sku ON true
          WHERE task.tenant_id=$1
            AND task.deleted IS NULL
            AND task.task_type='cycle_count_item_location'
        )
        SELECT * FROM work
        WHERE ($2 OR facility_id=ANY($3))
          AND ($4 OR inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR facility_id=$6)
          AND ($7::bigint IS NULL OR inventory_owner_id=$7)
          AND (($8::text IS NULL AND public_status IN ('pending','claimed'))
               OR public_status=$8)
        ORDER BY {sort} {direction} NULLS LAST, task_id
        OFFSET $9 LIMIT $10
        "#,
    );
    let offset = checked_offset(query.cursor, "cycle-count work")?;
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.facility_id.map(FacilityId::get))
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.status.map(CycleCountWorkStatus::as_str))
        .bind(offset)
        .bind(i64::from(query.limit) + 1)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .into_iter()
        .take(usize::from(query.limit))
        .map(map_work)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = next_cursor(has_more, offset, query.limit, "cycle-count work")?;
    tx.commit().await?;
    Ok(CycleCountWorkPage { items, next_cursor })
}

fn map_candidate(row: sqlx::postgres::PgRow) -> AppResult<CycleCountCandidateReadModel> {
    Ok(CycleCountCandidateReadModel {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|_| AppError::internal("cycle-count candidate has invalid owner ID"))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|_| AppError::internal("cycle-count candidate has invalid facility ID"))?,
        facility_name: row.try_get("facility_name")?,
        location: map_location(&row)?,
        stock: map_stock(&row)?,
        quantity_on_hand: row.try_get("qty_on_hand")?,
        quantity_reserved: row.try_get("qty_reserved")?,
        quantity_held: row.try_get("qty_held")?,
        last_counted_at: row.try_get("last_counted_at")?,
        last_variance_quantity: row.try_get("last_variance_quantity")?,
    })
}

fn map_work(row: sqlx::postgres::PgRow) -> AppResult<CycleCountWorkReadModel> {
    Ok(CycleCountWorkReadModel {
        task_id: row.try_get("task_id")?,
        status: parse_work_status(&row.try_get::<String, _>("public_status")?)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|_| AppError::internal("cycle-count task has invalid owner ID"))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|_| AppError::internal("cycle-count task has invalid facility ID"))?,
        facility_name: row.try_get("facility_name")?,
        location: map_location(&row)?,
        stock: map_stock(&row)?,
        current_quantity_on_hand: row.try_get("current_qty_on_hand")?,
        current_quantity_reserved: row.try_get("current_qty_reserved")?,
        current_quantity_held: row.try_get("current_qty_held")?,
        system_quantity_on_hand: row.try_get("system_qty_on_hand")?,
        system_quantity_reserved: row.try_get("system_qty_reserved")?,
        system_quantity_held: row.try_get("system_qty_held")?,
        counted_quantity: row.try_get("counted_qty")?,
        variance_quantity: row.try_get("variance_qty")?,
        inventory_transaction_id: row.try_get("inventory_transaction_id")?,
        priority: row.try_get("priority")?,
        note: row.try_get("note")?,
        assigned_user_id: row.try_get("assigned_user_id")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        due_at: row.try_get("due_at")?,
        created_at: row.try_get("created")?,
        completed_at: row.try_get("completed_at")?,
        confirmed_by: row.try_get("confirmed_by")?,
        confirmed_at: row.try_get("confirmed_at")?,
    })
}

fn map_location(row: &sqlx::postgres::PgRow) -> AppResult<CycleCountLocationReadModel> {
    Ok(CycleCountLocationReadModel {
        location_id: row.try_get("location_id")?,
        barcode: row.try_get("location_barcode")?,
        name: row.try_get("location_name")?,
    })
}

fn map_stock(row: &sqlx::postgres::PgRow) -> AppResult<CycleCountStockReadModel> {
    Ok(CycleCountStockReadModel {
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        license_plate_id: row.try_get("license_plate_id")?,
        license_plate_barcode: row.try_get("license_plate_barcode")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get("serial")?,
        inventory_status: parse_inventory_status(&row.try_get::<String, _>("inventory_status")?)?,
    })
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryBalanceStatus> {
    InventoryBalanceStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

fn parse_work_status(value: &str) -> AppResult<CycleCountWorkStatus> {
    match value {
        "pending" => Ok(CycleCountWorkStatus::Pending),
        "claimed" => Ok(CycleCountWorkStatus::Claimed),
        "completed" => Ok(CycleCountWorkStatus::Completed),
        "cancelled" => Ok(CycleCountWorkStatus::Cancelled),
        _ => Err(AppError::internal(format!(
            "invalid cycle-count work status in database: {value}"
        ))),
    }
}

const fn inventory_status_value(value: InventoryBalanceStatus) -> &'static str {
    match value {
        InventoryBalanceStatus::Available => "available",
        InventoryBalanceStatus::Hold => "hold",
        InventoryBalanceStatus::Damaged => "damaged",
        InventoryBalanceStatus::Quarantine => "quarantine",
    }
}

const fn candidate_sort_expression(sort: CycleCountCandidateSort) -> &'static str {
    match sort {
        CycleCountCandidateSort::LastCounted => "last_counted_at",
        CycleCountCandidateSort::Client => "inventory_owner_name",
        CycleCountCandidateSort::Facility => "facility_name",
        CycleCountCandidateSort::Location => "location_barcode",
        CycleCountCandidateSort::Item => "COALESCE(primary_sku,item_description)",
        CycleCountCandidateSort::Quantity => "qty_on_hand",
        CycleCountCandidateSort::InventoryStatus => "inventory_status",
    }
}

const fn work_sort_expression(sort: CycleCountWorkSort) -> &'static str {
    match sort {
        CycleCountWorkSort::Priority => "priority",
        CycleCountWorkSort::CreatedAt => "created",
        CycleCountWorkSort::Client => "inventory_owner_name",
        CycleCountWorkSort::Facility => "facility_name",
        CycleCountWorkSort::Location => "location_barcode",
        CycleCountWorkSort::Item => "COALESCE(primary_sku,item_description)",
        CycleCountWorkSort::Quantity => "COALESCE(counted_qty,current_qty_on_hand)",
        CycleCountWorkSort::Variance => "variance_qty",
        CycleCountWorkSort::Status => "public_status",
    }
}

const fn sort_direction(direction: CycleCountSortDirection) -> &'static str {
    match direction {
        CycleCountSortDirection::Asc => "ASC",
        CycleCountSortDirection::Desc => "DESC",
    }
}

fn checked_offset(cursor: Option<CycleCountCursor>, label: &str) -> AppResult<i64> {
    i64::try_from(cursor.map_or(0, |value| value.offset))
        .map_err(|_| AppError::bad_request(format!("{label} cursor overflow")))
}

fn next_cursor(
    has_more: bool,
    offset: i64,
    limit: u16,
    label: &str,
) -> AppResult<Option<CycleCountCursor>> {
    has_more
        .then(|| {
            Ok(CycleCountCursor {
                offset: u64::try_from(offset)
                    .map_err(|_| AppError::internal(format!("{label} cursor is negative")))?
                    + u64::from(limit),
            })
        })
        .transpose()
}
