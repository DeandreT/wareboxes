//! License plate/container records for grouping inventory under a scannable ID.

use sqlx::Row;
use wareboxes_core::models::{
    InventoryStatus, InventoryTransactionType, LicensePlate, LicensePlateContent,
};
use wareboxes_domain::{InventoryOwnerId, TenantId};

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::inventory;
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry, JournalStart};

fn parse_inventory_status(s: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {s}")))
}

fn map(row: &sqlx::postgres::PgRow) -> AppResult<LicensePlate> {
    Ok(LicensePlate {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        barcode: row.try_get("barcode")?,
        location_id: row.try_get("location_id")?,
        dims_id: row.try_get("dims_id")?,
        contents: Vec::new(),
    })
}

fn map_content(row: &sqlx::postgres::PgRow) -> AppResult<LicensePlateContent> {
    Ok(LicensePlateContent {
        inventory_balance_id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        location_id: row.try_get("location_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        status: parse_inventory_status(row.try_get::<String, _>("status")?.as_str())?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
    })
}

async fn contents_by_license_plate(
    db: &Db,
    tenant_id: TenantId,
    ids: &[i64],
) -> AppResult<std::collections::HashMap<i64, Vec<LicensePlateContent>>> {
    let mut contents = std::collections::HashMap::<i64, Vec<LicensePlateContent>>::new();
    if ids.is_empty() {
        return Ok(contents);
    }
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, license_plate_id, location_id,
               item_batch_id, status, qty_on_hand, qty_reserved
        FROM inventory_balances
        WHERE tenant_id = $1
          AND deleted IS NULL
          AND license_plate_id = ANY($2)
          AND (qty_on_hand > 0 OR qty_reserved > 0)
        ORDER BY license_plate_id, item_batch_id, status
        "#,
    )
    .bind(tenant_id.get())
    .bind(ids)
    .fetch_all(db)
    .await?;
    for row in &rows {
        let license_plate_id: i64 = row.try_get("license_plate_id")?;
        contents
            .entry(license_plate_id)
            .or_default()
            .push(map_content(row)?);
    }
    Ok(contents)
}

pub async fn get_license_plates(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<LicensePlate>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, deleted, barcode, location_id, dims_id
        FROM license_plates
        WHERE tenant_id = $1 AND ($2 OR deleted IS NULL)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(db)
    .await?;
    let mut plates = rows.iter().map(map).collect::<AppResult<Vec<_>>>()?;
    let ids = plates.iter().map(|lp| lp.id).collect::<Vec<_>>();
    let mut contents = contents_by_license_plate(db, tenant_id, &ids).await?;
    for plate in &mut plates {
        plate.contents = contents.remove(&plate.id).unwrap_or_default();
    }
    Ok(plates)
}

pub async fn get_license_plate_by_barcode(
    db: &Db,
    tenant_id: TenantId,
    barcode: &str,
) -> AppResult<Option<LicensePlate>> {
    let Some(row) = sqlx::query(
        "SELECT id, tenant_id, inventory_owner_id, created, deleted, barcode, location_id, dims_id FROM license_plates WHERE tenant_id = $1 AND barcode = $2 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(barcode)
    .fetch_optional(db)
    .await? else {
        return Ok(None);
    };
    let mut plate = map(&row)?;
    let mut contents = contents_by_license_plate(db, tenant_id, &[plate.id]).await?;
    plate.contents = contents.remove(&plate.id).unwrap_or_default();
    Ok(Some(plate))
}

pub async fn add_license_plate(
    db: &Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    barcode: Option<&str>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO license_plates (tenant_id, inventory_owner_id, created, barcode) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(now_iso())
    .bind(barcode)
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn find_or_create_license_plate_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    barcode: Option<&str>,
    license_plate_id: Option<i64>,
    location_id: i64,
) -> AppResult<Option<i64>> {
    let now = now_iso();
    let id = match (license_plate_id, barcode) {
        (Some(id), _) => id,
        (None, Some(barcode)) => {
            if barcode.trim().is_empty() {
                return Err(AppError::bad_request(
                    "license plate barcode cannot be blank",
                ));
            }
            if let Some(id) = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM license_plates WHERE tenant_id = $1 AND inventory_owner_id = $2 AND barcode = $3 AND deleted IS NULL",
            )
            .bind(tenant_id.get())
            .bind(inventory_owner_id)
            .bind(barcode)
            .fetch_optional(&mut **tx)
            .await?
            {
                id
            } else {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO license_plates (tenant_id, inventory_owner_id, created, barcode, location_id) VALUES ($1, $2, $3, $4, $5) RETURNING id",
                )
                .bind(tenant_id.get())
                .bind(inventory_owner_id)
                .bind(now)
                .bind(barcode)
                .bind(location_id)
                .fetch_one(&mut **tx)
                .await?
            }
        }
        (None, None) => return Ok(None),
    };

    let current_location = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT location_id FROM license_plates WHERE tenant_id = $1 AND inventory_owner_id = $2 AND id = $3 AND deleted IS NULL FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("license plate"))?;
    if let Some(current_location) = current_location {
        if current_location != location_id {
            return Err(AppError::conflict(
                "license plate is already assigned to another location",
            ));
        }
    } else {
        sqlx::query("UPDATE license_plates SET location_id = $1 WHERE tenant_id = $2 AND inventory_owner_id = $3 AND id = $4 AND deleted IS NULL")
            .bind(location_id)
            .bind(tenant_id.get())
            .bind(inventory_owner_id)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }

    Ok(Some(id))
}

pub async fn update_license_plate(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    barcode: Option<&str>,
) -> AppResult<bool> {
    let res =
        sqlx::query("UPDATE license_plates SET barcode = COALESCE($1, barcode) WHERE tenant_id = $2 AND id = $3")
            .bind(barcode)
            .bind(tenant_id.get())
            .bind(id)
            .execute(db)
            .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn move_license_plate(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    id: i64,
    to_location_id: i64,
    reason: Option<&str>,
    idempotency_key: Option<&str>,
) -> AppResult<i64> {
    let now = now_iso();
    let mut tx = db.begin().await?;

    let request_hash = inventory_journal::request_hash(&(id, to_location_id, reason))?;
    if let Some(transaction_id) = inventory_journal::replayed_transaction(
        &mut tx,
        tenant_id,
        "move_license_plate",
        idempotency_key,
        &request_hash,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(transaction_id);
    }

    let plate = sqlx::query(
        "SELECT inventory_owner_id, location_id FROM license_plates WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("license plate"))?;
    let inventory_owner_id: i64 = plate.try_get("inventory_owner_id")?;
    let plate_location: Option<i64> = plate.try_get("location_id")?;

    let transaction_id = match inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id,
            inventory_owner_id,
            actor_user_id: user_id,
            transaction_type: InventoryTransactionType::Move,
            reason,
            reference_type: Some("license_plate"),
            reference_id: Some(id),
            correlation_id: None,
            operation: "move_license_plate",
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

    let destination_facility_id: i64 = sqlx::query_scalar(
        "SELECT facility_id FROM locations WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL AND active",
    )
    .bind(tenant_id.get())
    .bind(to_location_id)
    .fetch_one(&mut *tx)
    .await?;

    let content_rows = sqlx::query(
        r#"
        SELECT id, facility_id, location_id, item_batch_id, status, qty_on_hand, qty_reserved
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND license_plate_id = $3
          AND deleted IS NULL
          AND qty_on_hand > 0
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;

    let source_locations = content_rows
        .iter()
        .map(|row| row.try_get::<i64, _>("location_id"))
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    if source_locations.len() > 1 {
        return Err(AppError::conflict(
            "license plate inventory is split across multiple locations",
        ));
    }
    let from_location_id = source_locations
        .iter()
        .next()
        .copied()
        .or(plate_location)
        .ok_or_else(|| AppError::conflict("license plate has no current location"))?;
    if content_rows.is_empty() {
        return Err(AppError::conflict(
            "license plate does not contain inventory; use a container workflow",
        ));
    }
    if from_location_id == to_location_id {
        return Err(AppError::bad_request(
            "source and destination locations must differ",
        ));
    }
    let source_facility_id: i64 = content_rows[0].try_get("facility_id")?;
    if source_facility_id != destination_facility_id {
        return Err(AppError::conflict(
            "inventory moves cannot cross facilities; use an inventory transfer workflow",
        ));
    }
    for row in &content_rows {
        let qty_reserved: i64 = row.try_get("qty_reserved")?;
        if qty_reserved > 0 {
            return Err(AppError::conflict(
                "cannot move a license plate that contains reserved inventory",
            ));
        }
    }

    let mixed_content: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT a.id
        FROM inventory_balances a
        INNER JOIN inventory_balances b ON b.license_plate_id = a.license_plate_id AND b.id > a.id
        INNER JOIN item_batches batch_a ON batch_a.id = a.item_batch_id
        INNER JOIN item_batches batch_b ON batch_b.id = b.item_batch_id
        WHERE a.tenant_id = $1
          AND a.inventory_owner_id = $2
          AND a.license_plate_id = $3
          AND a.deleted IS NULL
          AND b.deleted IS NULL
          AND a.qty_on_hand > 0
          AND b.qty_on_hand > 0
          AND batch_a.item_id = batch_b.item_id
          AND (
              batch_a.lot IS DISTINCT FROM batch_b.lot
              OR batch_a.expiration IS DISTINCT FROM batch_b.expiration
          )
        LIMIT 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if mixed_content.is_some() {
        return Err(AppError::conflict(
            "license plate contains this item with multiple lots or expirations",
        ));
    }

    for row in &content_rows {
        let item_batch_id: i64 = row.try_get("item_batch_id")?;
        inventory::ensure_location_accepts_batch_tx(
            &mut tx,
            tenant_id,
            inventory_owner_id,
            to_location_id,
            item_batch_id,
        )
        .await?;
    }

    sqlx::query(
        r#"
        UPDATE inventory_balances
        SET facility_id = $1, location_id = $2, modified = $3
        WHERE tenant_id = $4 AND inventory_owner_id = $5
          AND license_plate_id = $6 AND deleted IS NULL
        "#,
    )
    .bind(destination_facility_id)
    .bind(to_location_id)
    .bind(now)
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE license_plates SET location_id = $1 WHERE tenant_id = $2 AND inventory_owner_id = $3 AND id = $4")
        .bind(to_location_id)
        .bind(tenant_id.get())
        .bind(inventory_owner_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;

    for row in &content_rows {
        let item_batch_id: i64 = row.try_get("item_batch_id")?;
        let status = parse_inventory_status(&row.try_get::<String, _>("status")?)?;
        let qty: i64 = row.try_get("qty_on_hand")?;
        for (location_id, quantity_delta) in [(from_location_id, -qty), (to_location_id, qty)] {
            inventory_journal::append_entry(
                &mut tx,
                tenant_id,
                inventory_owner_id,
                transaction_id,
                &JournalEntry {
                    facility_id: source_facility_id,
                    location_id,
                    license_plate_id: Some(id),
                    item_batch_id,
                    status,
                    quantity_delta,
                },
            )
            .await?;
        }
    }

    tx.commit().await?;
    Ok(transaction_id)
}

pub async fn set_license_plate_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM license_plates WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        return Ok(false);
    }

    if deleted {
        let stocked: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM inventory_balances WHERE tenant_id = $1 AND license_plate_id = $2 AND deleted IS NULL AND (qty_on_hand > 0 OR qty_reserved > 0) LIMIT 1",
        )
        .bind(tenant_id.get())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if stocked.is_some() {
            return Err(AppError::conflict(
                "cannot delete a license plate that still has inventory",
            ));
        }
    }
    let res =
        sqlx::query("UPDATE license_plates SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(if deleted { Some(now_iso()) } else { None })
            .bind(tenant_id.get())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}
