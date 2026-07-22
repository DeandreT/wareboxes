//! Inventory commands, immutable journal reads, and balance projections.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_core::models::{
    InventoryBalance, InventoryEntry, InventoryReconciliationIssue, InventoryReservation,
    InventoryStatus, InventoryTransaction, InventoryTransactionType, ItemBatch, ReservationStatus,
    Timestamp,
};
use wareboxes_domain::{InventoryOwnerId, TenantId};

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry, JournalStart};

fn parse_inventory_status(s: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {s}")))
}

fn parse_transaction_type(s: &str) -> AppResult<InventoryTransactionType> {
    InventoryTransactionType::parse(s).ok_or_else(|| {
        AppError::internal(format!(
            "invalid inventory transaction type in database: {s}"
        ))
    })
}

fn parse_reservation_status(s: &str) -> AppResult<ReservationStatus> {
    ReservationStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid reservation status in database: {s}")))
}

fn map_batch(row: &sqlx::postgres::PgRow) -> AppResult<ItemBatch> {
    Ok(ItemBatch {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
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
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        modified: row.try_get("modified")?,
        deleted: row.try_get("deleted")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get::<Option<i64>, _>("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        status: parse_inventory_status(row.try_get::<String, _>("status")?.as_str())?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
    })
}

fn map_transaction(row: &sqlx::postgres::PgRow) -> AppResult<InventoryTransaction> {
    Ok(InventoryTransaction {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        actor_user_id: row.try_get::<Option<i64>, _>("actor_user_id")?,
        transaction_type: parse_transaction_type(
            row.try_get::<String, _>("transaction_type")?.as_str(),
        )?,
        reason: row.try_get::<Option<String>, _>("reason")?,
        reference_type: row.try_get::<Option<String>, _>("reference_type")?,
        reference_id: row.try_get::<Option<i64>, _>("reference_id")?,
        correlation_id: row.try_get::<Option<String>, _>("correlation_id")?,
        operation: row.try_get("operation")?,
        idempotency_key: row.try_get::<Option<String>, _>("idempotency_key")?,
        entries: Vec::new(),
    })
}

fn map_entry(row: &sqlx::postgres::PgRow) -> AppResult<InventoryEntry> {
    Ok(InventoryEntry {
        id: row.try_get("id")?,
        transaction_id: row.try_get("transaction_id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get::<Option<i64>, _>("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get("serial")?,
        status: parse_inventory_status(row.try_get::<String, _>("status")?.as_str())?,
        quantity_delta: row.try_get("quantity_delta")?,
    })
}

fn map_reservation(row: &sqlx::postgres::PgRow) -> AppResult<InventoryReservation> {
    Ok(InventoryReservation {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        modified: row.try_get("modified")?,
        deleted: row.try_get("deleted")?,
        order_id: row.try_get("order_id")?,
        order_item_id: row.try_get::<Option<i64>, _>("order_item_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        facility_id: row.try_get("facility_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        location_id: row.try_get("location_id")?,
        qty: row.try_get("qty")?,
        status: parse_reservation_status(row.try_get::<String, _>("status")?.as_str())?,
    })
}

pub(crate) async fn ensure_location_accepts_batch_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    location_id: i64,
    item_batch_id: i64,
) -> AppResult<()> {
    let incoming_item_id: i64 = sqlx::query_scalar(
        "SELECT item_id FROM item_batches WHERE tenant_id = $1 AND inventory_owner_id = $2 AND id = $3 AND deleted IS NULL",
    )
            .bind(tenant_id.get())
            .bind(inventory_owner_id)
            .bind(item_batch_id)
            .fetch_one(&mut **tx)
            .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "inventory-location-item:{tenant_id}:{inventory_owner_id}:{location_id}:{incoming_item_id}"
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
          AND ib.tenant_id = $4
          AND ib.inventory_owner_id = $5
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
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .fetch_optional(&mut **tx)
    .await?;

    if conflict.is_some() {
        return Err(AppError::conflict(
            "location already contains this item with a different lot or expiration",
        ));
    }

    Ok(())
}

pub async fn get_item_batches(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<ItemBatch>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, deleted, item_id, uom,
               lot, load_id, order_id, expiration, serial
        FROM item_batches
        WHERE tenant_id = $1 AND ($2 OR deleted IS NULL)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(db)
    .await?;
    rows.iter().map(map_batch).collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn add_item_batch(
    db: &Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    item_id: i64,
    load_id: Option<i64>,
    lot: Option<&str>,
    serial: Option<&str>,
    expiration: Option<Timestamp>,
) -> AppResult<i64> {
    let mut tx = db.begin().await?;
    let owner_item_link = sqlx::query(
        r#"
        INSERT INTO inventory_owner_items
            (tenant_id, created, inventory_owner_id, item_id)
        SELECT $1, $2, owner.id, item.id
        FROM inventory_owners owner
        INNER JOIN items item ON item.tenant_id = owner.tenant_id AND item.id = $4
        WHERE owner.tenant_id = $1 AND owner.id = $3
          AND owner.deleted IS NULL AND item.deleted IS NULL
        ON CONFLICT (tenant_id, inventory_owner_id, item_id)
        DO UPDATE SET deleted = NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(inventory_owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await?;
    if owner_item_link.rows_affected() == 0 {
        return Err(AppError::bad_request(
            "item or inventory owner is outside the selected tenant",
        ));
    }

    let id: Option<i64> = sqlx::query_scalar(
        r#"
        INSERT INTO item_batches
            (tenant_id, inventory_owner_id, created, item_id, uom, load_id, lot, serial, expiration)
        SELECT $1, $2, $3, i.id, i.packaging_unit, $5, $6, $7, $8
        FROM items i
        INNER JOIN inventory_owners owner
            ON owner.tenant_id = i.tenant_id AND owner.id = $2 AND owner.deleted IS NULL
        WHERE i.tenant_id = $1 AND i.id = $4 AND i.deleted IS NULL
          AND ($5::BIGINT IS NULL OR EXISTS (
              SELECT 1 FROM loads l
              INNER JOIN inventory_owners load_owner ON load_owner.id = l.inventory_owner_id
              WHERE l.id = $5 AND l.inventory_owner_id = $2
                AND load_owner.tenant_id = $1 AND l.deleted IS NULL
          ))
        RETURNING item_batches.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(now_iso())
    .bind(item_id)
    .bind(load_id)
    .bind(lot)
    .bind(serial)
    .bind(expiration)
    .fetch_optional(&mut *tx)
    .await?;
    let id = id.ok_or_else(|| {
        AppError::bad_request("item, owner, or load is outside the selected scope")
    })?;
    tx.commit().await?;
    Ok(id)
}

pub async fn set_item_batch_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    if deleted {
        let stocked: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM inventory_balances WHERE tenant_id = $1 AND item_batch_id = $2 AND deleted IS NULL AND (qty_on_hand > 0 OR qty_reserved > 0) LIMIT 1",
        )
        .bind(tenant_id.get())
        .bind(id)
        .fetch_optional(db)
        .await?;
        if stocked.is_some() {
            return Err(AppError::conflict(
                "cannot delete an item batch that still has stock",
            ));
        }
    }
    let res = sqlx::query("UPDATE item_batches SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(tenant_id.get())
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn get_balances(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<InventoryBalance>> {
    let rows = sqlx::query(
        r#"
        SELECT ib.id, ib.tenant_id, ib.inventory_owner_id, ib.created, ib.modified,
               ib.deleted, ib.facility_id, facility.name AS facility_name,
               ib.location_id, ib.license_plate_id, ib.item_batch_id, ib.item_id,
               ib.uom, ib.status, ib.qty_on_hand, ib.qty_reserved
        FROM inventory_balances ib
        INNER JOIN facilities facility
            ON facility.tenant_id = ib.tenant_id AND facility.id = ib.facility_id
        WHERE ib.tenant_id = $1 AND ($2 OR ib.deleted IS NULL)
        ORDER BY ib.location_id, ib.item_batch_id, ib.status
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(db)
    .await?;
    rows.iter().map(map_balance).collect()
}

pub async fn get_reconciliation_issues(
    db: &Db,
    tenant_id: TenantId,
) -> AppResult<Vec<InventoryReconciliationIssue>> {
    let rows = sqlx::query(
        r#"
        SELECT tenant_id, inventory_owner_id, facility_id, location_id,
               license_plate_id, item_batch_id, item_id, uom, status,
               journal_qty, projected_qty, variance
        FROM inventory_reconciliation
        WHERE tenant_id = $1
        ORDER BY inventory_owner_id, facility_id, location_id, item_batch_id, status
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(InventoryReconciliationIssue {
                tenant_id: TenantId::new(row.try_get("tenant_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                facility_id: row.try_get("facility_id")?,
                location_id: row.try_get("location_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
                journal_qty: row.try_get("journal_qty")?,
                projected_qty: row.try_get("projected_qty")?,
                variance: row.try_get("variance")?,
            })
        })
        .collect()
}

pub async fn get_transactions(
    db: &Db,
    tenant_id: TenantId,
) -> AppResult<Vec<InventoryTransaction>> {
    let transaction_rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, actor_user_id,
               transaction_type, reason, reference_type, reference_id, correlation_id,
               operation, idempotency_key
        FROM inventory_transactions
        WHERE tenant_id = $1
        ORDER BY id DESC
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let mut transactions = transaction_rows
        .iter()
        .map(map_transaction)
        .collect::<AppResult<Vec<_>>>()?;
    if transactions.is_empty() {
        return Ok(transactions);
    }

    let transaction_ids = transactions.iter().map(|row| row.id).collect::<Vec<_>>();
    let entry_rows = sqlx::query(
        r#"
        SELECT id, transaction_id, tenant_id, inventory_owner_id, created, facility_id,
               location_id, license_plate_id, item_batch_id, item_id, uom, lot,
               expiration, serial, status, quantity_delta
        FROM inventory_entries
        WHERE tenant_id = $1 AND transaction_id = ANY($2)
        ORDER BY transaction_id, id
        "#,
    )
    .bind(tenant_id.get())
    .bind(&transaction_ids)
    .fetch_all(db)
    .await?;
    let mut entries = HashMap::<i64, Vec<InventoryEntry>>::new();
    for row in &entry_rows {
        let entry = map_entry(row)?;
        entries.entry(entry.transaction_id).or_default().push(entry);
    }
    for transaction in &mut transactions {
        transaction.entries = entries.remove(&transaction.id).unwrap_or_default();
    }
    Ok(transactions)
}

pub async fn get_reservations(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<InventoryReservation>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, modified, deleted, order_id,
               order_item_id, inventory_balance_id, facility_id, item_batch_id,
               location_id, qty, status
        FROM inventory_reservations
        WHERE tenant_id = $1 AND ($2 OR deleted IS NULL)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(db)
    .await?;
    rows.iter().map(map_reservation).collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn receive_inventory(
    db: &Db,
    tenant_id: TenantId,
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

    let request_hash = inventory_journal::request_hash(&(
        item_batch_id,
        to_location_id,
        qty,
        status.as_str(),
        reason,
        reference_type,
        reference_id,
    ))?;
    if let Some(transaction_id) = inventory_journal::replayed_transaction(
        &mut tx,
        tenant_id,
        "receive_inventory",
        idempotency_key,
        &request_hash,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(transaction_id);
    }

    let batch = sqlx::query(
        r#"
        SELECT inventory_owner_id, item_id, uom
        FROM item_batches
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(item_batch_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("item batch was not found in the selected tenant"))?;
    let inventory_owner_id: i64 = batch.try_get("inventory_owner_id")?;
    let item_id: i64 = batch.try_get("item_id")?;
    let uom: String = batch.try_get("uom")?;

    let facility_id: i64 = sqlx::query_scalar(
        "SELECT facility_id FROM locations WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL AND active",
    )
    .bind(tenant_id.get())
    .bind(to_location_id)
    .fetch_one(&mut *tx)
    .await?;

    let transaction_id = match inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id,
            inventory_owner_id,
            actor_user_id: user_id,
            transaction_type: InventoryTransactionType::Receive,
            reason,
            reference_type,
            reference_id,
            correlation_id: None,
            operation: "receive_inventory",
            idempotency_key,
            request_hash: &request_hash,
            record_idempotency: true,
        },
    )
    .await?
    {
        JournalStart::New(id) => id,
        JournalStart::Replay(id) => {
            tx.commit().await?;
            return Ok(id);
        }
    };

    ensure_location_accepts_batch_tx(
        &mut tx,
        tenant_id,
        inventory_owner_id,
        to_location_id,
        item_batch_id,
    )
    .await?;

    sqlx::query(
        r#"
        INSERT INTO inventory_balances
            (tenant_id, inventory_owner_id, created, modified, facility_id, location_id,
             license_plate_id, item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved)
        VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9, $10, $11, 0)
        ON CONFLICT (tenant_id, inventory_owner_id, location_id, item_batch_id, uom, status)
            WHERE license_plate_id IS NULL DO UPDATE
        SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
            modified = excluded.modified,
            deleted = NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(now)
    .bind(now)
    .bind(facility_id)
    .bind(to_location_id)
    .bind(item_batch_id)
    .bind(item_id)
    .bind(&uom)
    .bind(status.as_str())
    .bind(qty)
    .execute(&mut *tx)
    .await?;

    inventory_journal::append_entry(
        &mut tx,
        tenant_id,
        inventory_owner_id,
        transaction_id,
        &JournalEntry {
            facility_id,
            location_id: to_location_id,
            license_plate_id: None,
            item_batch_id,
            status,
            quantity_delta: qty,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(transaction_id)
}

#[allow(clippy::too_many_arguments)]
pub async fn move_inventory(
    db: &Db,
    tenant_id: TenantId,
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

    let request_hash = inventory_journal::request_hash(&(
        item_batch_id,
        from_location_id,
        to_location_id,
        qty,
        status.as_str(),
        reason,
        reference_type,
        reference_id,
    ))?;
    if let Some(transaction_id) = inventory_journal::replayed_transaction(
        &mut tx,
        tenant_id,
        "move_inventory",
        idempotency_key,
        &request_hash,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(transaction_id);
    }

    let source = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, item_id, uom, qty_on_hand, qty_reserved
        FROM inventory_balances
        WHERE tenant_id = $1
          AND location_id = $2
          AND item_batch_id = $3
          AND status = $4
          AND license_plate_id IS NULL
          AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(from_location_id)
    .bind(item_batch_id)
    .bind(status.as_str())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("source inventory balance was not found"))?;
    let inventory_owner_id: i64 = source.try_get("inventory_owner_id")?;
    let source_facility_id: i64 = source.try_get("facility_id")?;
    let item_id: i64 = source.try_get("item_id")?;
    let uom: String = source.try_get("uom")?;
    let qty_on_hand: i64 = source.try_get("qty_on_hand")?;
    let qty_reserved: i64 = source.try_get("qty_reserved")?;
    if qty_on_hand - qty_reserved < qty {
        return Err(AppError::conflict(
            "insufficient available inventory at source location",
        ));
    }

    let destination_facility_id: i64 = sqlx::query_scalar(
        "SELECT facility_id FROM locations WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL AND active",
    )
    .bind(tenant_id.get())
    .bind(to_location_id)
    .fetch_one(&mut *tx)
    .await?;
    if source_facility_id != destination_facility_id {
        return Err(AppError::conflict(
            "inventory moves cannot cross facilities; use an inventory transfer workflow",
        ));
    }

    let transaction_id = match inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id,
            inventory_owner_id,
            actor_user_id: user_id,
            transaction_type: InventoryTransactionType::Move,
            reason,
            reference_type,
            reference_id,
            correlation_id: None,
            operation: "move_inventory",
            idempotency_key,
            request_hash: &request_hash,
            record_idempotency: true,
        },
    )
    .await?
    {
        JournalStart::New(id) => id,
        JournalStart::Replay(id) => {
            tx.commit().await?;
            return Ok(id);
        }
    };

    ensure_location_accepts_batch_tx(
        &mut tx,
        tenant_id,
        inventory_owner_id,
        to_location_id,
        item_batch_id,
    )
    .await?;

    let res = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1, modified = $2
        WHERE tenant_id = $3
          AND inventory_owner_id = $4
          AND location_id = $5
          AND item_batch_id = $6
          AND status = $7
          AND license_plate_id IS NULL
          AND deleted IS NULL
          AND qty_on_hand - qty_reserved >= $8
        "#,
    )
    .bind(qty)
    .bind(now)
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
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

    sqlx::query(
        r#"
        INSERT INTO inventory_balances
            (tenant_id, inventory_owner_id, created, modified, facility_id, location_id,
             license_plate_id, item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved)
        VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9, $10, $11, 0)
        ON CONFLICT (tenant_id, inventory_owner_id, location_id, item_batch_id, uom, status)
            WHERE license_plate_id IS NULL DO UPDATE
        SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
            modified = excluded.modified,
            deleted = NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(now)
    .bind(now)
    .bind(destination_facility_id)
    .bind(to_location_id)
    .bind(item_batch_id)
    .bind(item_id)
    .bind(&uom)
    .bind(status.as_str())
    .bind(qty)
    .execute(&mut *tx)
    .await?;

    for (location_id, quantity_delta) in [(from_location_id, -qty), (to_location_id, qty)] {
        inventory_journal::append_entry(
            &mut tx,
            tenant_id,
            inventory_owner_id,
            transaction_id,
            &JournalEntry {
                facility_id: source_facility_id,
                location_id,
                license_plate_id: None,
                item_batch_id,
                status,
                quantity_delta,
            },
        )
        .await?;
    }

    tx.commit().await?;
    Ok(transaction_id)
}

#[allow(clippy::too_many_arguments)]
pub async fn split_move_inventory(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    from_inventory_balance_id: i64,
    destinations: &[(i64, i64)],
    reason: Option<&str>,
    reference_type: Option<&str>,
    reference_id: Option<i64>,
    idempotency_key: Option<&str>,
) -> AppResult<i64> {
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

    let request_hash = inventory_journal::request_hash(&(
        from_inventory_balance_id,
        destinations,
        reason,
        reference_type,
        reference_id,
    ))?;
    if let Some(transaction_id) = inventory_journal::replayed_transaction(
        &mut tx,
        tenant_id,
        "split_move_inventory",
        idempotency_key,
        &request_hash,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(transaction_id);
    }

    let Some(source) = sqlx::query(
        r#"
        SELECT id, inventory_owner_id, facility_id, location_id, license_plate_id,
               item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(from_inventory_balance_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(AppError::conflict("source inventory balance was not found"));
    };

    let from_location_id: i64 = source.try_get("location_id")?;
    let inventory_owner_id: i64 = source.try_get("inventory_owner_id")?;
    let source_facility_id: i64 = source.try_get("facility_id")?;
    let license_plate_id: Option<i64> = source.try_get("license_plate_id")?;
    let item_batch_id: i64 = source.try_get("item_batch_id")?;
    let item_id: i64 = source.try_get("item_id")?;
    let uom: String = source.try_get("uom")?;
    let status: String = source.try_get("status")?;
    let parsed_status = parse_inventory_status(&status)?;
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

    let destination_rows = sqlx::query(
        r#"
        SELECT id, facility_id
        FROM locations
        WHERE tenant_id = $1 AND id = ANY($2) AND deleted IS NULL AND active
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(destination_ids.iter().copied().collect::<Vec<_>>())
    .fetch_all(&mut *tx)
    .await?;
    if destination_rows.len() != destinations.len() {
        return Err(AppError::bad_request(
            "one or more destination locations were not found",
        ));
    }
    for row in &destination_rows {
        if row.try_get::<i64, _>("facility_id")? != source_facility_id {
            return Err(AppError::conflict(
                "inventory moves cannot cross facilities; use an inventory transfer workflow",
            ));
        }
    }

    let transaction_id = match inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id,
            inventory_owner_id,
            actor_user_id: user_id,
            transaction_type: InventoryTransactionType::Move,
            reason,
            reference_type,
            reference_id,
            correlation_id: None,
            operation: "split_move_inventory",
            idempotency_key,
            request_hash: &request_hash,
            record_idempotency: true,
        },
    )
    .await?
    {
        JournalStart::New(id) => id,
        JournalStart::Replay(id) => {
            tx.commit().await?;
            return Ok(id);
        }
    };

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
    .bind(now)
    .bind(from_inventory_balance_id)
    .bind(total_qty)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::conflict(
            "insufficient available inventory at source location",
        ));
    }

    inventory_journal::append_entry(
        &mut tx,
        tenant_id,
        inventory_owner_id,
        transaction_id,
        &JournalEntry {
            facility_id: source_facility_id,
            location_id: from_location_id,
            license_plate_id: None,
            item_batch_id,
            status: parsed_status,
            quantity_delta: -total_qty,
        },
    )
    .await?;

    for (to_location_id, qty) in destinations {
        ensure_location_accepts_batch_tx(
            &mut tx,
            tenant_id,
            inventory_owner_id,
            *to_location_id,
            item_batch_id,
        )
        .await?;

        sqlx::query(
            r#"
            INSERT INTO inventory_balances
                (tenant_id, inventory_owner_id, created, modified, facility_id, location_id,
                 license_plate_id, item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved)
            VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9, $10, $11, 0)
            ON CONFLICT (tenant_id, inventory_owner_id, location_id, item_batch_id, uom, status)
                WHERE license_plate_id IS NULL DO UPDATE
            SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                modified = excluded.modified,
                deleted = NULL
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id)
        .bind(now)
        .bind(now)
        .bind(source_facility_id)
        .bind(*to_location_id)
        .bind(item_batch_id)
        .bind(item_id)
        .bind(&uom)
        .bind(&status)
        .bind(*qty)
        .execute(&mut *tx)
        .await?;

        inventory_journal::append_entry(
            &mut tx,
            tenant_id,
            inventory_owner_id,
            transaction_id,
            &JournalEntry {
                facility_id: source_facility_id,
                location_id: *to_location_id,
                license_plate_id: None,
                item_batch_id,
                status: parsed_status,
                quantity_delta: *qty,
            },
        )
        .await?;
    }

    tx.commit().await?;
    Ok(transaction_id)
}

pub async fn reserve_inventory(
    db: &Db,
    tenant_id: TenantId,
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
        SELECT id, inventory_owner_id, facility_id, item_batch_id, item_id,
               location_id, qty_on_hand, qty_reserved
        FROM inventory_balances
        WHERE tenant_id = $1
          AND id = $2
          AND status = 'available'
          AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(AppError::conflict(
            "insufficient available inventory to reserve",
        ));
    };
    let resolved_balance_id: i64 = balance_row.try_get("id")?;
    let inventory_owner_id: i64 = balance_row.try_get("inventory_owner_id")?;
    let facility_id: i64 = balance_row.try_get("facility_id")?;
    let resolved_item_batch_id: i64 = balance_row.try_get("item_batch_id")?;
    let resolved_item_id: i64 = balance_row.try_get("item_id")?;
    let resolved_location_id: i64 = balance_row.try_get("location_id")?;
    let qty_on_hand: i64 = balance_row.try_get("qty_on_hand")?;
    let qty_reserved: i64 = balance_row.try_get("qty_reserved")?;

    if qty_on_hand - qty_reserved < qty {
        return Err(AppError::conflict(
            "insufficient available inventory to reserve",
        ));
    }

    let order_matches: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM orders o
            INNER JOIN inventory_owners owner ON owner.id = o.inventory_owner_id
            WHERE o.id = $1 AND o.inventory_owner_id = $2
              AND owner.tenant_id = $3 AND o.deleted IS NULL
        )
        "#,
    )
    .bind(order_id)
    .bind(inventory_owner_id)
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !order_matches {
        return Err(AppError::conflict(
            "order and inventory balance must have the same tenant and inventory owner",
        ));
    }

    if let Some(order_item_id) = order_item_id {
        let item_matches: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM order_items WHERE id = $1 AND order_id = $2 AND item_id = $3 AND deleted IS NULL)",
        )
        .bind(order_item_id)
        .bind(order_id)
        .bind(resolved_item_id)
        .fetch_one(&mut *tx)
        .await?;
        if !item_matches {
            return Err(AppError::conflict(
                "order line does not match the reserved inventory item",
            ));
        }
    }

    let res = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_reserved = qty_reserved + $1, modified = $2
        WHERE tenant_id = $3
          AND inventory_owner_id = $4
          AND id = $5
          AND qty_on_hand - qty_reserved >= $6
        "#,
    )
    .bind(qty)
    .bind(now)
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
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
            (tenant_id, inventory_owner_id, created, modified, order_id, order_item_id,
             inventory_balance_id, facility_id, item_batch_id, location_id, qty, status)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'reserved')
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(now)
    .bind(now)
    .bind(order_id)
    .bind(order_item_id)
    .bind(resolved_balance_id)
    .bind(facility_id)
    .bind(resolved_item_batch_id)
    .bind(resolved_location_id)
    .bind(qty)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(reservation_id)
}

pub async fn cancel_reservation(
    db: &Db,
    tenant_id: TenantId,
    reservation_id: i64,
) -> AppResult<bool> {
    let now = now_iso();
    let mut tx = db.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT inventory_balance_id, qty
        FROM inventory_reservations
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL AND status = 'reserved'
        "#,
    )
    .bind(tenant_id.get())
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
        WHERE tenant_id = $3 AND id = $4 AND qty_reserved >= $5
        "#,
    )
    .bind(qty)
    .bind(now)
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .bind(qty)
    .execute(&mut *tx)
    .await?;

    let res = sqlx::query(
        "UPDATE inventory_reservations SET deleted = $1, modified = $2, status = 'cancelled' WHERE tenant_id = $3 AND id = $4",
    )
    .bind(now)
    .bind(now)
    .bind(tenant_id.get())
    .bind(reservation_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}
