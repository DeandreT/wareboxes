//! Inventory ledger and balances. Balances are derived and guarded here;
//! callers create stock movements/reservations instead of editing quantities.

use sqlx::Row;
use wareboxes_core::models::{
    InventoryBalance, InventoryReservation, InventoryStatus, ItemBatch, Movement, MovementType,
    ReservationStatus, Timestamp,
};

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};

fn parse_inventory_status(s: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {s}")))
}

fn parse_movement_type(s: &str) -> AppResult<MovementType> {
    MovementType::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid movement type in database: {s}")))
}

fn parse_reservation_status(s: &str) -> AppResult<ReservationStatus> {
    ReservationStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid reservation status in database: {s}")))
}

fn map_batch(row: &sqlx::postgres::PgRow) -> AppResult<ItemBatch> {
    Ok(ItemBatch {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        item_id: row.try_get("item_id")?,
        lot: row.try_get::<Option<String>, _>("lot")?,
        load_id: row.try_get::<Option<i64>, _>("load_id")?,
        order_id: row.try_get::<Option<i64>, _>("order_id")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get::<Option<String>, _>("serial")?,
    })
}

fn map_balance(row: &sqlx::postgres::PgRow) -> AppResult<InventoryBalance> {
    Ok(InventoryBalance {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        modified: row.try_get("modified")?,
        deleted: row.try_get("deleted")?,
        warehouse_id: row.try_get("warehouse_id")?,
        warehouse_name: row.try_get("warehouse_name")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get::<Option<i64>, _>("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        status: parse_inventory_status(row.try_get::<String, _>("status")?.as_str())?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
    })
}

fn map_movement(row: &sqlx::postgres::PgRow) -> AppResult<Movement> {
    Ok(Movement {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        user_id: row.try_get::<Option<i64>, _>("user_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        license_plate_id: row.try_get::<Option<i64>, _>("license_plate_id")?,
        from_location_id: row.try_get::<Option<i64>, _>("from_location_id")?,
        to_location_id: row.try_get::<Option<i64>, _>("to_location_id")?,
        qty: row.try_get("qty")?,
        movement_type: parse_movement_type(row.try_get::<String, _>("movement_type")?.as_str())?,
        status: parse_inventory_status(row.try_get::<String, _>("status")?.as_str())?,
        reason: row.try_get::<Option<String>, _>("reason")?,
        reference_type: row.try_get::<Option<String>, _>("reference_type")?,
        reference_id: row.try_get::<Option<i64>, _>("reference_id")?,
        idempotency_key: row.try_get::<Option<String>, _>("idempotency_key")?,
    })
}

fn map_reservation(row: &sqlx::postgres::PgRow) -> AppResult<InventoryReservation> {
    Ok(InventoryReservation {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        modified: row.try_get("modified")?,
        deleted: row.try_get("deleted")?,
        order_id: row.try_get("order_id")?,
        order_item_id: row.try_get::<Option<i64>, _>("order_item_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        location_id: row.try_get("location_id")?,
        qty: row.try_get("qty")?,
        status: parse_reservation_status(row.try_get::<String, _>("status")?.as_str())?,
    })
}

pub(crate) async fn ensure_location_accepts_batch_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    location_id: i64,
    item_batch_id: i64,
) -> AppResult<()> {
    let incoming_item_id: i64 =
        sqlx::query_scalar("SELECT item_id FROM item_batches WHERE id = $1 AND deleted IS NULL")
            .bind(item_batch_id)
            .fetch_one(&mut **tx)
            .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "inventory-location-item:{location_id}:{incoming_item_id}"
        ))
        .execute(&mut **tx)
        .await?;

    let conflict: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT ib.id
        FROM inventory_balances ib
        INNER JOIN item_batches existing_batch ON existing_batch.id = ib.item_batch_id
        INNER JOIN item_batches incoming_batch ON incoming_batch.id = $2
        WHERE ib.location_id = $1
          AND ib.deleted IS NULL
          AND ib.qty_on_hand > 0
          AND ib.item_batch_id <> $2
          AND existing_batch.item_id = $3
          AND (
              existing_batch.lot IS DISTINCT FROM incoming_batch.lot
              OR existing_batch.expiration IS DISTINCT FROM incoming_batch.expiration
          )
        LIMIT 1
        FOR UPDATE OF ib
        "#,
    )
    .bind(location_id)
    .bind(item_batch_id)
    .bind(incoming_item_id)
    .fetch_optional(&mut **tx)
    .await?;

    if conflict.is_some() {
        return Err(AppError::conflict(
            "location already contains this item with a different lot or expiration",
        ));
    }

    Ok(())
}

pub async fn get_item_batches(db: &Db, show_deleted: bool) -> AppResult<Vec<ItemBatch>> {
    let sql = if show_deleted {
        "SELECT id, created, deleted, item_id, lot, load_id, order_id, expiration, serial FROM item_batches ORDER BY id"
    } else {
        "SELECT id, created, deleted, item_id, lot, load_id, order_id, expiration, serial FROM item_batches WHERE deleted IS NULL ORDER BY id"
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    rows.iter().map(map_batch).collect()
}

pub async fn add_item_batch(
    db: &Db,
    item_id: i64,
    load_id: Option<i64>,
    lot: Option<&str>,
    serial: Option<&str>,
    expiration: Option<Timestamp>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO item_batches (created, item_id, load_id, lot, serial, expiration) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(now_iso())
    .bind(item_id)
    .bind(load_id)
    .bind(lot)
    .bind(serial)
    .bind(expiration)
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn set_item_batch_deleted(db: &Db, id: i64, deleted: bool) -> AppResult<bool> {
    if deleted {
        let stocked: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM inventory_balances WHERE item_batch_id = $1 AND deleted IS NULL AND (qty_on_hand > 0 OR qty_reserved > 0) LIMIT 1",
        )
        .bind(id)
        .fetch_optional(db)
        .await?;
        if stocked.is_some() {
            return Err(AppError::conflict(
                "cannot delete an item batch that still has stock",
            ));
        }
    }
    let res = sqlx::query("UPDATE item_batches SET deleted = $1 WHERE id = $2")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn get_balances(db: &Db, show_deleted: bool) -> AppResult<Vec<InventoryBalance>> {
    let sql = if show_deleted {
        r#"
        SELECT ib.id, ib.created, ib.modified, ib.deleted, ib.warehouse_id, w.name AS warehouse_name,
               ib.location_id, ib.license_plate_id, ib.item_batch_id, ib.status, ib.qty_on_hand, ib.qty_reserved
        FROM inventory_balances ib
        LEFT JOIN warehouses w ON w.id = ib.warehouse_id
        ORDER BY ib.location_id, ib.item_batch_id, ib.status
        "#
    } else {
        r#"
        SELECT ib.id, ib.created, ib.modified, ib.deleted, ib.warehouse_id, w.name AS warehouse_name,
               ib.location_id, ib.license_plate_id, ib.item_batch_id, ib.status, ib.qty_on_hand, ib.qty_reserved
        FROM inventory_balances ib
        LEFT JOIN warehouses w ON w.id = ib.warehouse_id
        WHERE ib.deleted IS NULL
        ORDER BY ib.location_id, ib.item_batch_id, ib.status
        "#
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    rows.iter().map(map_balance).collect()
}

pub async fn get_movements(db: &Db) -> AppResult<Vec<Movement>> {
    let rows = sqlx::query(
        r#"
        SELECT id, created, deleted, user_id, item_batch_id, license_plate_id, from_location_id, to_location_id,
               qty, movement_type, status, reason, reference_type, reference_id, idempotency_key
        FROM stock_movements
        WHERE deleted IS NULL
        ORDER BY id DESC
        "#,
    )
    .fetch_all(db)
    .await?;
    rows.iter().map(map_movement).collect()
}

pub async fn get_reservations(db: &Db, show_deleted: bool) -> AppResult<Vec<InventoryReservation>> {
    let sql = if show_deleted {
        "SELECT id, created, modified, deleted, order_id, order_item_id, inventory_balance_id, item_batch_id, location_id, qty, status FROM inventory_reservations ORDER BY id"
    } else {
        "SELECT id, created, modified, deleted, order_id, order_item_id, inventory_balance_id, item_batch_id, location_id, qty, status FROM inventory_reservations WHERE deleted IS NULL ORDER BY id"
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    rows.iter().map(map_reservation).collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn receive_inventory(
    db: &Db,
    user_id: i64,
    item_batch_id: i64,
    to_location_id: i64,
    qty: i64,
    status: Option<InventoryStatus>,
    reason: Option<&str>,
    reference_type: Option<&str>,
    reference_id: Option<i64>,
    idempotency_key: Option<&str>,
) -> AppResult<i64> {
    if qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }
    let status = status.unwrap_or_default();
    let now = now_iso();
    let mut tx = db.begin().await?;

    let warehouse_id: i64 = sqlx::query_scalar(
        "SELECT warehouse_id FROM locations WHERE id = $1 AND deleted IS NULL AND active",
    )
    .bind(to_location_id)
    .fetch_one(&mut *tx)
    .await?;

    ensure_location_accepts_batch_tx(&mut tx, to_location_id, item_batch_id).await?;

    sqlx::query(
        r#"
        INSERT INTO inventory_balances
            (created, modified, warehouse_id, location_id, license_plate_id, item_batch_id, status, qty_on_hand, qty_reserved)
        VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, 0)
        ON CONFLICT (location_id, item_batch_id, status) WHERE license_plate_id IS NULL DO UPDATE
        SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
            modified = excluded.modified,
            deleted = NULL
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(warehouse_id)
    .bind(to_location_id)
    .bind(item_batch_id)
    .bind(status.as_str())
    .bind(qty)
    .execute(&mut *tx)
    .await?;

    let movement_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO stock_movements
            (created, user_id, item_batch_id, license_plate_id, from_location_id, to_location_id, qty, movement_type,
             status, reason, reference_type, reference_id, idempotency_key)
        VALUES ($1, $2, $3, NULL, NULL, $4, $5, 'receive', $6, $7, $8, $9, $10)
        RETURNING id
        "#,
    )
    .bind(&now)
    .bind(user_id)
    .bind(item_batch_id)
    .bind(to_location_id)
    .bind(qty)
    .bind(status.as_str())
    .bind(reason)
    .bind(reference_type)
    .bind(reference_id)
    .bind(idempotency_key)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(movement_id)
}

#[allow(clippy::too_many_arguments)]
pub async fn move_inventory(
    db: &Db,
    user_id: i64,
    item_batch_id: i64,
    from_location_id: i64,
    to_location_id: i64,
    qty: i64,
    status: Option<InventoryStatus>,
    reason: Option<&str>,
    reference_type: Option<&str>,
    reference_id: Option<i64>,
    idempotency_key: Option<&str>,
) -> AppResult<i64> {
    if qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }
    if from_location_id == to_location_id {
        return Err(AppError::bad_request(
            "source and destination locations must differ",
        ));
    }

    let status = status.unwrap_or_default();
    let now = now_iso();
    let mut tx = db.begin().await?;

    ensure_location_accepts_batch_tx(&mut tx, to_location_id, item_batch_id).await?;

    let res = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1, modified = $2
        WHERE location_id = $3
          AND item_batch_id = $4
          AND status = $5
          AND license_plate_id IS NULL
          AND deleted IS NULL
          AND qty_on_hand - qty_reserved >= $6
        "#,
    )
    .bind(qty)
    .bind(&now)
    .bind(from_location_id)
    .bind(item_batch_id)
    .bind(status.as_str())
    .bind(qty)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::conflict(
            "insufficient available inventory at source location",
        ));
    }

    let warehouse_id: i64 = sqlx::query_scalar(
        "SELECT warehouse_id FROM locations WHERE id = $1 AND deleted IS NULL AND active",
    )
    .bind(to_location_id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO inventory_balances
            (created, modified, warehouse_id, location_id, license_plate_id, item_batch_id, status, qty_on_hand, qty_reserved)
        VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, 0)
        ON CONFLICT (location_id, item_batch_id, status) WHERE license_plate_id IS NULL DO UPDATE
        SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
            modified = excluded.modified,
            deleted = NULL
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(warehouse_id)
    .bind(to_location_id)
    .bind(item_batch_id)
    .bind(status.as_str())
    .bind(qty)
    .execute(&mut *tx)
    .await?;

    let movement_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO stock_movements
            (created, user_id, item_batch_id, license_plate_id, from_location_id, to_location_id, qty, movement_type,
             status, reason, reference_type, reference_id, idempotency_key)
        VALUES ($1, $2, $3, NULL, $4, $5, $6, 'move', $7, $8, $9, $10, $11)
        RETURNING id
        "#,
    )
    .bind(&now)
    .bind(user_id)
    .bind(item_batch_id)
    .bind(from_location_id)
    .bind(to_location_id)
    .bind(qty)
    .bind(status.as_str())
    .bind(reason)
    .bind(reference_type)
    .bind(reference_id)
    .bind(idempotency_key)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(movement_id)
}

#[allow(clippy::too_many_arguments)]
pub async fn split_move_inventory(
    db: &Db,
    user_id: i64,
    from_inventory_balance_id: i64,
    destinations: &[(i64, i64)],
    reason: Option<&str>,
    reference_type: Option<&str>,
    reference_id: Option<i64>,
    idempotency_key: Option<&str>,
) -> AppResult<Vec<i64>> {
    if destinations.is_empty() {
        return Err(AppError::bad_request(
            "at least one destination is required",
        ));
    }

    let mut destination_ids = std::collections::BTreeSet::new();
    let mut total_qty = 0_i64;
    for (to_location_id, qty) in destinations {
        if *to_location_id <= 0 {
            return Err(AppError::bad_request("destination location is required"));
        }
        if *qty <= 0 {
            return Err(AppError::bad_request("quantity must be positive"));
        }
        if !destination_ids.insert(*to_location_id) {
            return Err(AppError::bad_request(
                "destination locations must be unique",
            ));
        }
        total_qty = total_qty
            .checked_add(*qty)
            .ok_or_else(|| AppError::bad_request("move quantity is too large"))?;
    }

    let now = now_iso();
    let mut tx = db.begin().await?;

    let Some(source) = sqlx::query(
        r#"
        SELECT id, warehouse_id, location_id, license_plate_id, item_batch_id, status, qty_on_hand, qty_reserved
        FROM inventory_balances
        WHERE id = $1 AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(from_inventory_balance_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(AppError::conflict("source inventory balance was not found"));
    };

    let from_location_id: i64 = source.try_get("location_id")?;
    let license_plate_id: Option<i64> = source.try_get("license_plate_id")?;
    let item_batch_id: i64 = source.try_get("item_batch_id")?;
    let status: String = source.try_get("status")?;
    let qty_on_hand: i64 = source.try_get("qty_on_hand")?;
    let qty_reserved: i64 = source.try_get("qty_reserved")?;
    if license_plate_id.is_some() {
        return Err(AppError::conflict(
            "use the License Plates panel to move license-plated stock",
        ));
    }
    if destinations
        .iter()
        .any(|(to_location_id, _)| *to_location_id == from_location_id)
    {
        return Err(AppError::bad_request(
            "source and destination locations must differ",
        ));
    }
    if qty_on_hand - qty_reserved < total_qty {
        return Err(AppError::conflict(
            "insufficient available inventory at source location",
        ));
    }

    let res = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1, modified = $2
        WHERE id = $3
          AND license_plate_id IS NULL
          AND deleted IS NULL
          AND qty_on_hand - qty_reserved >= $4
        "#,
    )
    .bind(total_qty)
    .bind(&now)
    .bind(from_inventory_balance_id)
    .bind(total_qty)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::conflict(
            "insufficient available inventory at source location",
        ));
    }

    let mut movement_ids = Vec::with_capacity(destinations.len());
    for (idx, (to_location_id, qty)) in destinations.iter().enumerate() {
        let warehouse_id: i64 = sqlx::query_scalar(
            "SELECT warehouse_id FROM locations WHERE id = $1 AND deleted IS NULL AND active",
        )
        .bind(*to_location_id)
        .fetch_one(&mut *tx)
        .await?;

        ensure_location_accepts_batch_tx(&mut tx, *to_location_id, item_batch_id).await?;

        sqlx::query(
            r#"
            INSERT INTO inventory_balances
                (created, modified, warehouse_id, location_id, license_plate_id, item_batch_id, status, qty_on_hand, qty_reserved)
            VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, 0)
            ON CONFLICT (location_id, item_batch_id, status) WHERE license_plate_id IS NULL DO UPDATE
            SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                modified = excluded.modified,
                deleted = NULL
            "#,
        )
        .bind(&now)
        .bind(&now)
        .bind(warehouse_id)
        .bind(*to_location_id)
        .bind(item_batch_id)
        .bind(&status)
        .bind(*qty)
        .execute(&mut *tx)
        .await?;

        let movement_key = idempotency_key.map(|key| format!("{key}:{idx}:{to_location_id}"));
        let movement_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO stock_movements
                (created, user_id, item_batch_id, license_plate_id, from_location_id, to_location_id, qty, movement_type,
                 status, reason, reference_type, reference_id, idempotency_key)
            VALUES ($1, $2, $3, NULL, $4, $5, $6, 'move', $7, $8, $9, $10, $11)
            RETURNING id
            "#,
        )
        .bind(&now)
        .bind(user_id)
        .bind(item_batch_id)
        .bind(from_location_id)
        .bind(*to_location_id)
        .bind(*qty)
        .bind(&status)
        .bind(reason)
        .bind(reference_type)
        .bind(reference_id)
        .bind(movement_key.as_deref())
        .fetch_one(&mut *tx)
        .await?;
        movement_ids.push(movement_id);
    }

    tx.commit().await?;
    Ok(movement_ids)
}

pub async fn reserve_inventory(
    db: &Db,
    user_id: i64,
    order_id: i64,
    order_item_id: Option<i64>,
    inventory_balance_id: i64,
    qty: i64,
) -> AppResult<i64> {
    if qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }
    let now = now_iso();
    let mut tx = db.begin().await?;

    let Some(balance_row) = sqlx::query(
        r#"
        SELECT id, item_batch_id, location_id, license_plate_id, qty_on_hand, qty_reserved
        FROM inventory_balances
        WHERE id = $1
          AND status = 'available'
          AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(inventory_balance_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(AppError::conflict(
            "insufficient available inventory to reserve",
        ));
    };
    let resolved_balance_id: i64 = balance_row.try_get("id")?;
    let resolved_item_batch_id: i64 = balance_row.try_get("item_batch_id")?;
    let resolved_location_id: i64 = balance_row.try_get("location_id")?;
    let resolved_license_plate_id: Option<i64> = balance_row.try_get("license_plate_id")?;
    let qty_on_hand: i64 = balance_row.try_get("qty_on_hand")?;
    let qty_reserved: i64 = balance_row.try_get("qty_reserved")?;

    if qty_on_hand - qty_reserved < qty {
        return Err(AppError::conflict(
            "insufficient available inventory to reserve",
        ));
    }

    let res = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_reserved = qty_reserved + $1, modified = $2
        WHERE id = $3
          AND qty_on_hand - qty_reserved >= $4
        "#,
    )
    .bind(qty)
    .bind(&now)
    .bind(resolved_balance_id)
    .bind(qty)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::conflict(
            "insufficient available inventory to reserve",
        ));
    }

    let reservation_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_reservations
            (created, modified, order_id, order_item_id, inventory_balance_id, item_batch_id, location_id, qty, status)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'reserved')
        RETURNING id
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(order_id)
    .bind(order_item_id)
    .bind(resolved_balance_id)
    .bind(resolved_item_batch_id)
    .bind(resolved_location_id)
    .bind(qty)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO stock_movements
            (created, user_id, item_batch_id, license_plate_id, from_location_id, to_location_id, qty, movement_type,
             status, reason, reference_type, reference_id, idempotency_key)
        VALUES ($1, $2, $3, $4, $5, NULL, $6, 'reserve', 'available', 'order reservation', 'order', $7, NULL)
        "#,
    )
    .bind(&now)
    .bind(user_id)
    .bind(resolved_item_batch_id)
    .bind(resolved_license_plate_id)
    .bind(resolved_location_id)
    .bind(qty)
    .bind(order_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(reservation_id)
}

pub async fn cancel_reservation(db: &Db, reservation_id: i64) -> AppResult<bool> {
    let now = now_iso();
    let mut tx = db.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT inventory_balance_id, qty
        FROM inventory_reservations
        WHERE id = $1 AND deleted IS NULL AND status = 'reserved'
        "#,
    )
    .bind(reservation_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        return Ok(false);
    };
    let inventory_balance_id: i64 = row.try_get("inventory_balance_id")?;
    let qty: i64 = row.try_get("qty")?;

    sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_reserved = qty_reserved - $1, modified = $2
        WHERE id = $3 AND qty_reserved >= $4
        "#,
    )
    .bind(qty)
    .bind(&now)
    .bind(inventory_balance_id)
    .bind(qty)
    .execute(&mut *tx)
    .await?;

    let res = sqlx::query(
        "UPDATE inventory_reservations SET deleted = $1, modified = $2, status = 'cancelled' WHERE id = $3",
    )
    .bind(&now)
    .bind(&now)
    .bind(reservation_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}
