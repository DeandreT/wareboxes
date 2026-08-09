use sqlx::Row;
use wareboxes_application::shipping::{
    ManualCarrierManifestReadModel, ShipmentCancellationReadModel, ShipmentCartonReadModel,
    ShipmentCartonTrackingReadModel, ShipmentDepartureProgress, ShipmentQuery, ShipmentReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CarrierCode, CarrierManifestId, CarrierServiceCode, CartonId, FacilityId, InventoryOwnerId,
    ManifestReference, OrderId, OrderRevision, OrderStatus, PackSessionId,
    ShipmentCancellationDetails, ShipmentCancellationId, ShipmentCancellationNote,
    ShipmentCancellationReason, ShipmentId, ShipmentRevision, ShipmentScanValue, ShipmentStatus,
    ShipmentTrackingAssignmentId, TenantId, Timestamp, TrackingNumber, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

use super::{order_demand_tx, positive};

pub async fn get_shipment(
    db: &Db,
    access: &TenantAccess,
    query: ShipmentQuery,
) -> AppResult<ShipmentReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let shipment = load_shipment_tx(&mut tx, access.tenant_id, query.shipment_id, &scope).await?;
    tx.commit().await?;
    Ok(shipment)
}

pub(super) async fn load_shipment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
    scope: &ScopeBindings,
) -> AppResult<ShipmentReadModel> {
    let row = sqlx::query(
        r#"
        SELECT shipment.id, shipment.attempt, shipment.packing_session_id, shipment.order_id,
               order_header.order_key, order_header.status AS order_status,
               order_header.revision AS order_revision, shipment.inventory_owner_id,
               shipment.facility_id, shipment.state, shipment.revision,
               shipment.carton_count, shipment.shipped_qty,
               shipment.departed_carton_count, shipment.departed_qty,
               shipment.created_by_user_id, shipment.created_at,
               confirmation.confirmed_by_user_id AS departed_by_user_id,
               shipment.departed_at,
               cancellation.id AS cancellation_id,
               cancellation.reason_code AS cancellation_reason,
               cancellation.note AS cancellation_note,
               cancellation.cancelled_by_user_id,
               cancellation.cancelled_at
        FROM shipments shipment
        INNER JOIN orders order_header
          ON order_header.tenant_id = shipment.tenant_id
         AND order_header.inventory_owner_id = shipment.inventory_owner_id
         AND order_header.id = shipment.order_id
        LEFT JOIN shipment_confirmations confirmation
          ON confirmation.tenant_id = shipment.tenant_id
         AND confirmation.inventory_owner_id = shipment.inventory_owner_id
         AND confirmation.facility_id = shipment.facility_id
         AND confirmation.shipment_id = shipment.id
         AND confirmation.resulting_shipment_state = 'departed'
         AND confirmation.confirmed_at = shipment.departed_at
        LEFT JOIN shipment_cancellations cancellation
          ON cancellation.tenant_id = shipment.tenant_id
         AND cancellation.inventory_owner_id = shipment.inventory_owner_id
         AND cancellation.facility_id = shipment.facility_id
         AND cancellation.shipment_id = shipment.id
        WHERE shipment.tenant_id = $1 AND shipment.id = $2
          AND ($3 OR shipment.facility_id = ANY($4))
          AND ($5 OR shipment.inventory_owner_id = ANY($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("shipment"))?;
    let cartons = load_cartons_tx(tx, tenant_id, shipment_id).await?;
    let manifest = load_manifest_tx(tx, tenant_id, shipment_id).await?;
    let status_text: String = row.try_get("state")?;
    let order_status_text: String = row.try_get("order_status")?;
    let order_id = positive(row.try_get("order_id")?, OrderId::new)?;
    let inventory_owner_id = positive(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?;
    let demand = order_demand_tx(tx, tenant_id, inventory_owner_id, order_id).await?;
    if demand.effective().get() != row.try_get::<i64, _>("shipped_qty")? {
        return Err(AppError::internal(
            "shipment quantity does not match effective order demand",
        ));
    }
    let shipment = ShipmentReadModel {
        shipment_id: positive(row.try_get("id")?, ShipmentId::new)?,
        attempt: row.try_get("attempt")?,
        packing_session_id: positive(row.try_get("packing_session_id")?, PackSessionId::new)?,
        order_id,
        order_key: row.try_get("order_key")?,
        inventory_owner_id,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        status: ShipmentStatus::parse(&status_text)
            .ok_or_else(|| AppError::internal("shipment has an invalid status"))?,
        revision: positive(row.try_get("revision")?, ShipmentRevision::new)?,
        order_status: OrderStatus::parse(&order_status_text)
            .ok_or_else(|| AppError::internal("shipment order has an invalid status"))?,
        order_revision: positive(row.try_get("order_revision")?, OrderRevision::new)?,
        demand,
        departure_progress: ShipmentDepartureProgress {
            total_carton_count: row.try_get("carton_count")?,
            departed_carton_count: row.try_get("departed_carton_count")?,
            remaining_carton_count: row.try_get::<i64, _>("carton_count")?
                - row.try_get::<i64, _>("departed_carton_count")?,
            total_quantity: row.try_get("shipped_qty")?,
            departed_quantity: row.try_get("departed_qty")?,
            remaining_quantity: row.try_get::<i64, _>("shipped_qty")?
                - row.try_get::<i64, _>("departed_qty")?,
        },
        cartons,
        manifest,
        cancellation: cancellation_from_row(&row)?,
        created_by: positive(row.try_get("created_by_user_id")?, UserId::new)?,
        created_at: row.try_get("created_at")?,
        departed_by: row
            .try_get::<Option<i64>, _>("departed_by_user_id")?
            .map(|id| positive(id, UserId::new))
            .transpose()?,
        departed_at: row.try_get("departed_at")?,
    };
    if !shipment.is_consistent() {
        return Err(AppError::internal("shipment read model is inconsistent"));
    }
    Ok(shipment)
}

fn cancellation_from_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<Option<ShipmentCancellationReadModel>> {
    let Some(id) = row.try_get::<Option<i64>, _>("cancellation_id")? else {
        return Ok(None);
    };
    let reason = match row.try_get::<String, _>("cancellation_reason")?.as_str() {
        "packing_correction" => ShipmentCancellationReason::PackingCorrection,
        "shipping_data_correction" => ShipmentCancellationReason::ShippingDataCorrection,
        "duplicate_shipment" => ShipmentCancellationReason::DuplicateShipment,
        "operator_error" => ShipmentCancellationReason::OperatorError,
        "other" => ShipmentCancellationReason::Other,
        _ => {
            return Err(AppError::internal(
                "shipment cancellation reason is invalid",
            ))
        }
    };
    let note = row
        .try_get::<Option<String>, _>("cancellation_note")?
        .map(ShipmentCancellationNote::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let details = ShipmentCancellationDetails::new(reason, note)
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(Some(ShipmentCancellationReadModel {
        cancellation_id: positive(id, ShipmentCancellationId::new)?,
        details,
        cancelled_by: positive(row.try_get("cancelled_by_user_id")?, UserId::new)?,
        cancelled_at: row.try_get("cancelled_at")?,
    }))
}

async fn load_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
) -> AppResult<Vec<ShipmentCartonReadModel>> {
    let rows = sqlx::query(
        r#"
        SELECT shipment_carton.carton_id, shipment_carton.carton_barcode,
               shipment_carton.sequence, shipment_carton.content_count,
               shipment_carton.packed_qty, shipment_carton.weight_g,
               shipment_carton.length_mm, shipment_carton.width_mm,
               shipment_carton.height_mm,
               package.id AS tracking_assignment_id, package.tracking_number,
               departed.departed_at
        FROM shipment_cartons shipment_carton
        LEFT JOIN shipment_manifest_packages package
          ON package.tenant_id = shipment_carton.tenant_id
         AND package.inventory_owner_id = shipment_carton.inventory_owner_id
         AND package.facility_id = shipment_carton.facility_id
         AND package.shipment_id = shipment_carton.shipment_id
         AND package.shipment_carton_id = shipment_carton.id
        LEFT JOIN shipment_confirmation_cartons departed
          ON departed.tenant_id = shipment_carton.tenant_id
         AND departed.inventory_owner_id = shipment_carton.inventory_owner_id
         AND departed.facility_id = shipment_carton.facility_id
         AND departed.shipment_id = shipment_carton.shipment_id
         AND departed.shipment_carton_id = shipment_carton.id
        WHERE shipment_carton.tenant_id = $1
          AND shipment_carton.shipment_id = $2
        ORDER BY shipment_carton.sequence, shipment_carton.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ShipmentCartonReadModel {
                carton_id: positive(row.try_get("carton_id")?, CartonId::new)?,
                carton_barcode: ShipmentScanValue::new(row.try_get::<String, _>("carton_barcode")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                sequence: row.try_get("sequence")?,
                content_count: row.try_get("content_count")?,
                packed_quantity: row.try_get("packed_qty")?,
                weight_grams: row.try_get("weight_g")?,
                length_mm: row.try_get("length_mm")?,
                width_mm: row.try_get("width_mm")?,
                height_mm: row.try_get("height_mm")?,
                tracking_assignment_id: row
                    .try_get::<Option<i64>, _>("tracking_assignment_id")?
                    .map(|id| positive(id, ShipmentTrackingAssignmentId::new))
                    .transpose()?,
                tracking_number: row
                    .try_get::<Option<String>, _>("tracking_number")?
                    .map(TrackingNumber::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
                departed_at: row.try_get("departed_at")?,
            })
        })
        .collect()
}

async fn load_manifest_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
) -> AppResult<Option<ManualCarrierManifestReadModel>> {
    let row = sqlx::query(
        r#"
        SELECT id, carrier, service, manifest_number,
               manifested_by_user_id, manifested_at
        FROM shipment_manifests
        WHERE tenant_id = $1 AND shipment_id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let manifest_id = positive(row.try_get("id")?, CarrierManifestId::new)?;
    let assignments = load_tracking_assignments_tx(tx, tenant_id, shipment_id, manifest_id).await?;
    Ok(Some(ManualCarrierManifestReadModel {
        manifest_id,
        carrier_code: CarrierCode::new(row.try_get::<String, _>("carrier")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        service_code: row
            .try_get::<Option<String>, _>("service")?
            .map(CarrierServiceCode::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        manifest_reference: ManifestReference::new(row.try_get::<String, _>("manifest_number")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        carton_tracking_assignments: assignments,
        manifested_by: positive(row.try_get("manifested_by_user_id")?, UserId::new)?,
        manifested_at: row.try_get::<Timestamp, _>("manifested_at")?,
    }))
}

async fn load_tracking_assignments_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
    manifest_id: CarrierManifestId,
) -> AppResult<Vec<ShipmentCartonTrackingReadModel>> {
    let rows = sqlx::query(
        r#"
        SELECT id, carton_id, tracking_number
        FROM shipment_manifest_packages
        WHERE tenant_id = $1 AND shipment_id = $2 AND manifest_id = $3
        ORDER BY sequence, id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(manifest_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ShipmentCartonTrackingReadModel {
                tracking_assignment_id: positive(
                    row.try_get("id")?,
                    ShipmentTrackingAssignmentId::new,
                )?,
                carton_id: positive(row.try_get("carton_id")?, CartonId::new)?,
                tracking_number: TrackingNumber::new(row.try_get::<String, _>("tracking_number")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            })
        })
        .collect()
}
