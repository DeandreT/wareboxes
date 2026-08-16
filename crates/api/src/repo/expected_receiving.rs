//! Read model for scanner-first expected receiving sessions.

use sqlx::Row;
use wareboxes_application::receipt_policy::ReceiptPolicyReadModel;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedReceivingLoadStatus {
    Arrived,
    Receiving,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedReceivingLocation {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedReceiptLine {
    pub load_line_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub item_barcodes: Vec<String>,
    pub expected_quantity: i64,
    pub received_quantity: i64,
    pub rejected_quantity: i64,
    pub missing_quantity: i64,
    pub remaining_quantity: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedReceivingSession {
    pub load_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub reference_number: Option<String>,
    pub status: ExpectedReceivingLoadStatus,
    pub expected_seal: Option<String>,
    pub receiving_location: ExpectedReceivingLocation,
    pub receipt_policy: ReceiptPolicyReadModel,
    pub lines: Vec<ExpectedReceiptLine>,
}

/// Resolves a canonical load execution barcode inside the caller's complete
/// tenant, facility, and inventory-owner scope.
pub async fn get_expected_receiving_session_by_execution_barcode(
    db: &Db,
    access: &TenantAccess,
    execution_barcode: &str,
) -> AppResult<ExpectedReceivingSession> {
    let execution_barcode = crate::repo::loads::normalize_execution_barcode(execution_barcode)?;
    let scope = ScopeBindings::for_access(access);
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let load_id: i64 = sqlx::query_scalar(
        r#"
        SELECT load.id
        FROM loads load
        WHERE load.tenant_id = $1
          AND load.execution_barcode = $2
          AND load.deleted IS NULL
          AND ($3 OR load.facility_id = ANY($4))
          AND ($5 OR load.inventory_owner_id = ANY($6))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(execution_barcode)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("expected receiving load"))?;
    tx.commit().await?;

    get_expected_receiving_session(db, access, load_id).await
}

/// Loads the executable read projection for an expected receiving session.
///
/// Scope failures intentionally look identical to an unknown load. Readiness
/// failures are conflicts because the load exists for the caller but cannot be
/// executed safely by a scanner.
pub async fn get_expected_receiving_session(
    db: &Db,
    access: &TenantAccess,
    load_id: i64,
) -> AppResult<ExpectedReceivingSession> {
    let scope = ScopeBindings::for_access(access);
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let load = sqlx::query(
        r#"
        SELECT load.id,
               load.inventory_owner_id,
               load.facility_id,
               load.reference_number,
               load.status,
               load.type,
               load.seal_number,
               location.id AS location_id,
               location.barcode AS location_barcode,
               location.name AS location_name,
               location.deleted AS location_deleted,
               location.active AS location_active,
               location.receivable AS location_receivable
        FROM loads load
        LEFT JOIN locations location
          ON location.tenant_id = load.tenant_id
         AND location.facility_id = load.facility_id
         AND location.id = load.dock_door_location_id
        WHERE load.tenant_id = $1
          AND load.id = $2
          AND load.deleted IS NULL
          AND ($3 OR load.facility_id = ANY($4))
          AND ($5 OR load.inventory_owner_id = ANY($6))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("expected receiving load"))?;

    if load.try_get::<String, _>("type")? != "inbound" {
        return Err(AppError::conflict(
            "expected receiving requires an inbound load",
        ));
    }
    let status = match load.try_get::<String, _>("status")?.as_str() {
        "arrived" => ExpectedReceivingLoadStatus::Arrived,
        "receiving" => ExpectedReceivingLoadStatus::Receiving,
        "received" => ExpectedReceivingLoadStatus::Received,
        _ => {
            return Err(AppError::conflict(
                "load must be arrived, receiving, or received for receiving execution",
            ));
        }
    };

    let receiving_location = receiving_location(&load)?;
    let rows = sqlx::query(
        r#"
        SELECT line.id AS load_line_id,
               line.item_id,
               line.expected_qty,
               line.received_qty,
               line.rejected_qty,
               line.missing_qty,
               line.lot,
               line.serial,
               line.expiration,
               item.description AS item_description,
               item.packaging_unit AS uom,
               item.deleted AS item_deleted,
               COALESCE(
                   ARRAY_AGG(DISTINCT barcode.name ORDER BY barcode.name)
                       FILTER (
                           WHERE barcode.deleted IS NULL
                             AND NULLIF(BTRIM(barcode.name), '') IS NOT NULL
                       ),
                   ARRAY[]::TEXT[]
               ) AS item_barcodes
        FROM load_lines line
        INNER JOIN items item
          ON item.tenant_id = line.tenant_id
         AND item.id = line.item_id
        LEFT JOIN barcodes barcode
          ON barcode.tenant_id = item.tenant_id
         AND barcode.item_id = item.id
        WHERE line.tenant_id = $1
          AND line.load_id = $2
          AND line.deleted IS NULL
          AND line.status IN ('pending', 'partial')
        GROUP BY line.id,
                 line.item_id,
                 line.expected_qty,
                 line.received_qty,
                 line.rejected_qty,
                 line.missing_qty,
                 line.lot,
                 line.serial,
                 line.expiration,
                 item.description,
                 item.packaging_unit,
                 item.deleted
        ORDER BY line.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .fetch_all(&mut *tx)
    .await?;

    if rows.is_empty() && status != ExpectedReceivingLoadStatus::Received {
        return Err(AppError::conflict(
            "load has no open expected receiving lines",
        ));
    }
    let lines = rows.iter().map(map_line).collect::<AppResult<Vec<_>>>()?;
    let inventory_owner_id_raw = load.try_get("inventory_owner_id")?;
    let facility_id_raw = load.try_get("facility_id")?;
    let receipt_policy = crate::repo::receipt_policy::resolve_receipt_policy_tx(
        &mut tx,
        access.tenant_id,
        InventoryOwnerId::new(inventory_owner_id_raw)
            .map_err(|error| AppError::internal(error.to_string()))?,
        FacilityId::new(facility_id_raw).map_err(|error| AppError::internal(error.to_string()))?,
        crate::db::now_iso(),
        false,
    )
    .await?;
    tx.commit().await?;

    Ok(ExpectedReceivingSession {
        load_id: load.try_get("id")?,
        inventory_owner_id: inventory_owner_id_raw,
        facility_id: facility_id_raw,
        reference_number: load.try_get("reference_number")?,
        status,
        expected_seal: load.try_get("seal_number")?,
        receiving_location,
        receipt_policy,
        lines,
    })
}

fn receiving_location(row: &sqlx::postgres::PgRow) -> AppResult<ExpectedReceivingLocation> {
    let location_id: Option<i64> = row.try_get("location_id")?;
    let deleted: Option<chrono::DateTime<chrono::Utc>> = row.try_get("location_deleted")?;
    let active: Option<bool> = row.try_get("location_active")?;
    let receivable: Option<bool> = row.try_get("location_receivable")?;
    if location_id.is_none()
        || deleted.is_some()
        || active != Some(true)
        || receivable != Some(true)
    {
        return Err(AppError::conflict(
            "load requires an active receivable dock with a scannable barcode",
        ));
    }
    let location_id = location_id.ok_or_else(|| {
        AppError::internal("validated expected receiving location is missing its ID")
    })?;
    let barcode = row
        .try_get::<Option<String>, _>("location_barcode")?
        .filter(|barcode| !barcode.trim().is_empty())
        .ok_or_else(|| {
            AppError::conflict("load requires an active receivable dock with a scannable barcode")
        })?;

    Ok(ExpectedReceivingLocation {
        location_id,
        barcode,
        name: row.try_get("location_name")?,
    })
}

fn map_line(row: &sqlx::postgres::PgRow) -> AppResult<ExpectedReceiptLine> {
    let load_line_id: i64 = row.try_get("load_line_id")?;
    if row
        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("item_deleted")?
        .is_some()
    {
        return Err(AppError::conflict(format!(
            "expected receiving line {load_line_id} requires an active item"
        )));
    }
    let item_barcodes: Vec<String> = row.try_get("item_barcodes")?;
    if item_barcodes.is_empty() {
        return Err(AppError::conflict(format!(
            "expected receiving line {load_line_id} requires an active item barcode"
        )));
    }

    let expected_quantity: i64 = row.try_get("expected_qty")?;
    let received_quantity: i64 = row.try_get("received_qty")?;
    let rejected_quantity: i64 = row.try_get("rejected_qty")?;
    let missing_quantity: i64 = row.try_get("missing_qty")?;
    let remaining_quantity = expected_quantity
        .checked_sub(received_quantity)
        .and_then(|quantity| quantity.checked_sub(rejected_quantity))
        .and_then(|quantity| quantity.checked_sub(missing_quantity))
        .filter(|quantity| *quantity > 0)
        .ok_or_else(|| {
            AppError::conflict(format!(
                "expected receiving line {load_line_id} has no remaining quantity"
            ))
        })?;

    Ok(ExpectedReceiptLine {
        load_line_id,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        item_barcodes,
        expected_quantity,
        received_quantity,
        rejected_quantity,
        missing_quantity,
        remaining_quantity,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
    })
}
