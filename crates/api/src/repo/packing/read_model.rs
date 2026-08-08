use sqlx::Row;
use wareboxes_application::packing::{
    PackAllocationDisposition, PackCarton, PackCartonLifecycle, PackSessionQuery,
    PackSessionReadModel, PackableAllocation,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CartonContentId, CartonDimensions, CartonId, CartonMeasurements, DimensionMillimeters,
    FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryOwnerId, ItemBatchId,
    LicensePlateId, LocationId, OrderId, OrderLineId, OrderRevision, PackQuantity, PackScanValue,
    PackSessionId, PackingProgress, TenantId, Timestamp, UserId, WeightGrams,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, ScopeBindings};

pub async fn packing_session(
    db: &Db,
    access: &TenantAccess,
    query: PackSessionQuery,
) -> AppResult<PackSessionReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let result = load_session_tx(&mut tx, access.tenant_id, query.session_id, &scope).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn packing_session_for_order(
    db: &Db,
    access: &TenantAccess,
    order_id: OrderId,
) -> AppResult<Option<PackSessionReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let session_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM packing_sessions
        WHERE tenant_id = $1 AND order_id = $2
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let result = match session_id {
        Some(id) => Some(
            load_session_tx(
                &mut tx,
                access.tenant_id,
                PackSessionId::new(id).map_err(|error| AppError::internal(error.to_string()))?,
                &scope,
            )
            .await?,
        ),
        None => None,
    };
    tx.commit().await?;
    Ok(result)
}

pub(super) async fn load_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
    scope: &ScopeBindings,
) -> AppResult<PackSessionReadModel> {
    let row = sqlx::query(
        r#"
        SELECT session.id, session.order_id, session.inventory_owner_id,
               session.facility_id, session.packing_location_id,
               session.revision, session.expected_allocation_count,
               session.packed_allocation_count, session.expected_qty,
               session.packed_qty, session.open_carton_count,
               session.closed_carton_count, session.started_by_user_id,
               session.started_at, orders.order_key,
               location.barcode AS station_location_barcode,
               location.name AS station_location_name
        FROM packing_sessions session
        INNER JOIN orders
          ON orders.tenant_id = session.tenant_id
         AND orders.inventory_owner_id = session.inventory_owner_id
         AND orders.id = session.order_id AND orders.deleted IS NULL
        INNER JOIN locations location
          ON location.tenant_id = session.tenant_id
         AND location.facility_id = session.facility_id
         AND location.id = session.packing_location_id
         AND location.deleted IS NULL
        WHERE session.tenant_id = $1 AND session.id = $2
          AND ($3 OR session.facility_id = ANY($4))
          AND ($5 OR session.inventory_owner_id = ANY($6))
        FOR SHARE OF session
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing session"))?;

    let progress = PackingProgress::new(
        row.try_get("expected_allocation_count")?,
        row.try_get("packed_allocation_count")?,
        row.try_get("expected_qty")?,
        row.try_get("packed_qty")?,
        row.try_get("open_carton_count")?,
        row.try_get("closed_carton_count")?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let cartons = load_cartons_tx(tx, tenant_id, session_id).await?;
    let allocations = load_allocations_tx(tx, tenant_id, session_id).await?;
    Ok(PackSessionReadModel {
        session_id,
        order_id: positive(row.try_get("order_id")?, OrderId::new)?,
        inventory_owner_id: positive(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        station_location_id: positive(row.try_get("packing_location_id")?, LocationId::new)?,
        station_location_barcode: scan(row.try_get("station_location_barcode")?)?,
        station_location_name: row.try_get("station_location_name")?,
        order_key: row.try_get("order_key")?,
        revision: positive(row.try_get("revision")?, OrderRevision::new)?,
        progress,
        cartons,
        allocations,
        started_by: positive(row.try_get("started_by_user_id")?, UserId::new)?,
        started_at: row.try_get("started_at")?,
    })
}

async fn load_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
) -> AppResult<Vec<PackCarton>> {
    let rows = sqlx::query(
        r#"
        SELECT carton.id, plate.barcode, carton.state, carton.created_by_user_id,
               carton.created_at, carton.closed_by_user_id, carton.closed_at,
               carton.voided_by_user_id, carton.voided_at,
               carton.weight_g, carton.length_mm, carton.width_mm, carton.height_mm,
               COUNT(content.id) AS content_count
        FROM cartons carton
        INNER JOIN license_plates plate
          ON plate.tenant_id = carton.tenant_id
         AND plate.inventory_owner_id = carton.inventory_owner_id
         AND plate.facility_id = carton.facility_id
         AND plate.id = carton.license_plate_id
        LEFT JOIN carton_contents content
          ON content.tenant_id = carton.tenant_id AND content.carton_id = carton.id
        WHERE carton.tenant_id = $1 AND carton.packing_session_id = $2
        GROUP BY carton.id, plate.barcode
        ORDER BY carton.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter().map(map_carton).collect()
}

fn map_carton(row: sqlx::postgres::PgRow) -> AppResult<PackCarton> {
    let lifecycle = match row.try_get::<String, _>("state")?.as_str() {
        "open" => PackCartonLifecycle::Open,
        "closed" => PackCartonLifecycle::Closed {
            measurements: measurements(&row)?,
            closed_by: positive(
                row.try_get::<Option<i64>, _>("closed_by_user_id")?
                    .ok_or_else(|| AppError::internal("closed carton has no actor"))?,
                UserId::new,
            )?,
            closed_at: row
                .try_get::<Option<Timestamp>, _>("closed_at")?
                .ok_or_else(|| AppError::internal("closed carton has no timestamp"))?,
        },
        "voided" => PackCartonLifecycle::Voided {
            voided_by: positive(
                row.try_get::<Option<i64>, _>("voided_by_user_id")?
                    .ok_or_else(|| AppError::internal("voided carton has no actor"))?,
                UserId::new,
            )?,
            voided_at: row
                .try_get::<Option<Timestamp>, _>("voided_at")?
                .ok_or_else(|| AppError::internal("voided carton has no timestamp"))?,
        },
        _ => return Err(AppError::internal("carton has an invalid state")),
    };
    Ok(PackCarton {
        carton_id: positive(row.try_get("id")?, CartonId::new)?,
        carton_barcode: scan(row.try_get("barcode")?)?,
        lifecycle,
        content_count: row.try_get("content_count")?,
        created_by: positive(row.try_get("created_by_user_id")?, UserId::new)?,
        created_at: row.try_get("created_at")?,
    })
}

async fn load_allocations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
) -> AppResult<Vec<PackableAllocation>> {
    let rows = sqlx::query(
        r#"
        SELECT snapshot.source_inventory_allocation_id, snapshot.order_item_id,
               snapshot.source_inventory_balance_id, snapshot.source_location_id,
               location.barcode AS source_location_barcode,
               location.name AS source_location_name,
               snapshot.source_license_plate_id, plate.barcode AS plate_barcode,
               snapshot.item_batch_id, snapshot.item_id, item.description,
               snapshot.uom, batch.lot, batch.serial, batch.expiration,
               snapshot.planned_qty, content.id AS content_id,
               content.carton_id, content.packed_by_user_id, content.packed_at,
               ARRAY(
                   SELECT barcode.name FROM barcodes barcode
                   WHERE barcode.tenant_id = snapshot.tenant_id
                     AND barcode.item_id = snapshot.item_id
                     AND barcode.deleted IS NULL AND btrim(barcode.name) <> ''
                   ORDER BY barcode.name
               ) AS item_barcodes
        FROM packing_session_allocations snapshot
        INNER JOIN locations location
          ON location.tenant_id = snapshot.tenant_id
         AND location.facility_id = snapshot.facility_id
         AND location.id = snapshot.source_location_id
        INNER JOIN license_plates plate
          ON plate.tenant_id = snapshot.tenant_id
         AND plate.inventory_owner_id = snapshot.inventory_owner_id
         AND plate.facility_id = snapshot.facility_id
         AND plate.id = snapshot.source_license_plate_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = snapshot.tenant_id
         AND batch.inventory_owner_id = snapshot.inventory_owner_id
         AND batch.id = snapshot.item_batch_id
        INNER JOIN items item
          ON item.tenant_id = snapshot.tenant_id AND item.id = snapshot.item_id
        LEFT JOIN carton_contents content
          ON content.tenant_id = snapshot.tenant_id
         AND content.packing_session_allocation_id = snapshot.id
        WHERE snapshot.tenant_id = $1 AND snapshot.packing_session_id = $2
        ORDER BY snapshot.sequence, snapshot.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter().map(map_allocation).collect()
}

fn map_allocation(row: sqlx::postgres::PgRow) -> AppResult<PackableAllocation> {
    let disposition = match row.try_get::<Option<i64>, _>("content_id")? {
        Some(content_id) => PackAllocationDisposition::Packed {
            content_id: positive(content_id, CartonContentId::new)?,
            carton_id: positive(row.try_get("carton_id")?, CartonId::new)?,
            packed_by: positive(row.try_get("packed_by_user_id")?, UserId::new)?,
            packed_at: row.try_get("packed_at")?,
        },
        None => PackAllocationDisposition::Available,
    };
    Ok(PackableAllocation {
        inventory_allocation_id: positive(
            row.try_get("source_inventory_allocation_id")?,
            InventoryAllocationId::new,
        )?,
        order_line_id: positive(row.try_get("order_item_id")?, OrderLineId::new)?,
        inventory_balance_id: positive(
            row.try_get("source_inventory_balance_id")?,
            InventoryBalanceId::new,
        )?,
        source_location_id: positive(row.try_get("source_location_id")?, LocationId::new)?,
        source_location_barcode: scan(row.try_get("source_location_barcode")?)?,
        source_location_name: row.try_get("source_location_name")?,
        license_plate_id: positive(row.try_get("source_license_plate_id")?, LicensePlateId::new)?,
        license_plate_barcode: scan(row.try_get("plate_barcode")?)?,
        item_batch_id: positive(row.try_get("item_batch_id")?, ItemBatchId::new)?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("description")?,
        item_barcodes: row
            .try_get::<Vec<String>, _>("item_barcodes")?
            .into_iter()
            .map(scan)
            .collect::<AppResult<Vec<_>>>()?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        quantity: PackQuantity::new(row.try_get("planned_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        disposition,
    })
}

fn measurements(row: &sqlx::postgres::PgRow) -> AppResult<CartonMeasurements> {
    let weight = row
        .try_get::<Option<i64>, _>("weight_g")?
        .map(WeightGrams::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let dimensions = match (
        row.try_get::<Option<i64>, _>("length_mm")?,
        row.try_get::<Option<i64>, _>("width_mm")?,
        row.try_get::<Option<i64>, _>("height_mm")?,
    ) {
        (None, None, None) => None,
        (Some(length), Some(width), Some(height)) => Some(CartonDimensions::new(
            DimensionMillimeters::new(length)
                .map_err(|error| AppError::internal(error.to_string()))?,
            DimensionMillimeters::new(width)
                .map_err(|error| AppError::internal(error.to_string()))?,
            DimensionMillimeters::new(height)
                .map_err(|error| AppError::internal(error.to_string()))?,
        )),
        _ => return Err(AppError::internal("carton has partial dimensions")),
    };
    Ok(CartonMeasurements::new(weight, dimensions))
}

fn scan(value: String) -> AppResult<PackScanValue> {
    PackScanValue::new(value).map_err(|error| AppError::internal(error.to_string()))
}

fn positive<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>) -> AppResult<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(|error| AppError::internal(error.to_string()))
}
