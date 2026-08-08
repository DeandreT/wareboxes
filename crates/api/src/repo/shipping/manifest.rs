use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::shipping::{
    RecordManualManifestCommand, RecordManualManifestResult, RECORD_MANUAL_MANIFEST_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    record_manual_manifest as validate_manifest, CarrierManifestId, CartonId,
    CartonTrackingAssignment, OrderStatus, ShipmentCartonIdentity, ShipmentScanValue,
    ShippingError, TenantId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::orders::insert_order_activity_tx;

use super::read_model::load_shipment_tx;
use super::{
    enqueue_order_event_tx, lock_order_tx, lock_shipment_tx, order_hint_for_shipment_tx, positive,
    require_replayed_shipment_id_visible_tx,
};

#[derive(Debug)]
struct ManifestCarton {
    shipment_carton_id: i64,
    carton_id: CartonId,
    license_plate_id: i64,
    carton_barcode: ShipmentScanValue,
    sequence: i64,
    weight_g: Option<i64>,
    length_mm: Option<i64>,
    width_mm: Option<i64>,
    height_mm: Option<i64>,
}

pub async fn record_manual_manifest(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RecordManualManifestCommand,
) -> AppResult<RecordManualManifestResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RECORD_MANUAL_MANIFEST_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<RecordManualManifestResult>(&mut tx)
        .await?
    {
        require_replayed_shipment_id_visible_tx(
            &mut tx,
            access.tenant_id,
            result.shipment_id,
            result.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id =
        order_hint_for_shipment_tx(&mut tx, access.tenant_id, command.shipment_id).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    let shipment = lock_shipment_tx(&mut tx, access.tenant_id, command.shipment_id, &scope).await?;
    if shipment.order_id != order.id || shipment.inventory_owner_id != order.inventory_owner_id {
        return Err(AppError::not_found("shipment"));
    }
    if !matches!(order.status, OrderStatus::AwaitingShipment) {
        return Err(AppError::conflict(
            "shipment order is no longer awaiting shipment",
        ));
    }
    if shipment.revision != command.expected_revision {
        return Err(AppError::conflict("shipment manifest revision is stale"));
    }
    let cartons = lock_manifest_cartons_tx(&mut tx, access.tenant_id, command.shipment_id).await?;
    let identities = cartons
        .iter()
        .map(|carton| ShipmentCartonIdentity::new(carton.carton_id, carton.carton_barcode.clone()))
        .collect::<Vec<_>>();
    let next_status = validate_manifest(
        shipment.status,
        &identities,
        &command.carton_tracking_assignments,
    )
    .map_err(manifest_validation_error)?;
    let next_revision = shipment
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("shipment revision overflow"))?;

    lock_manifest_natural_keys_tx(&mut tx, access.tenant_id, command).await?;
    require_manifest_keys_available_tx(&mut tx, access.tenant_id, command).await?;
    let manifested_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE shipments
        SET state = $1, revision = $2, carrier = $3, service = $4,
            manifested_at = $5
        WHERE tenant_id = $6 AND id = $7 AND state = $8 AND revision = $9
        "#,
    )
    .bind(next_status.as_str())
    .bind(next_revision.get())
    .bind(command.carrier_code.as_str())
    .bind(command.service_code.as_ref().map(|code| code.as_str()))
    .bind(manifested_at)
    .bind(access.tenant_id.get())
    .bind(shipment.id.get())
    .bind(shipment.status.as_str())
    .bind(shipment.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "shipment changed while recording its manifest",
        ));
    }
    let manifest_id_raw: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO shipment_manifests (
            tenant_id, inventory_owner_id, facility_id, shipment_id,
            packing_session_id, order_release_id, order_id,
            manifest_number, carrier, service, expected_revision,
            resulting_revision, package_count, manifested_by_user_id, manifested_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15
        ) RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment.inventory_owner_id.get())
    .bind(shipment.facility_id.get())
    .bind(shipment.id.get())
    .bind(shipment.packing_session_id.get())
    .bind(shipment.order_release_id)
    .bind(shipment.order_id.get())
    .bind(command.manifest_reference.as_str())
    .bind(command.carrier_code.as_str())
    .bind(command.service_code.as_ref().map(|code| code.as_str()))
    .bind(shipment.revision.get())
    .bind(next_revision.get())
    .bind(shipment.carton_count)
    .bind(context.actor_id.get())
    .bind(manifested_at)
    .fetch_one(&mut *tx)
    .await?;
    let manifest_id = positive(manifest_id_raw, CarrierManifestId::new)?;
    insert_manifest_packages_tx(
        &mut tx,
        access.tenant_id,
        &shipment,
        manifest_id,
        command,
        &cartons,
        manifested_at,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "manifested shipment {} with {} via {}",
            shipment.id, command.manifest_reference, command.carrier_code
        ),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
        context.actor_id.get(),
        shipment.order_id,
        "shipping.shipment_manifested",
        &format!("shipment:{}:manifested", shipment.id.get()),
        serde_json::json!({
            "shipment_id": shipment.id,
            "order_id": shipment.order_id,
            "manifest_id": manifest_id,
            "manifest_reference": command.manifest_reference,
            "carrier_code": command.carrier_code,
            "service_code": command.service_code,
            "package_count": shipment.carton_count,
            "expected_revision": shipment.revision,
            "revision": next_revision,
            "manifested_at": manifested_at,
        }),
        manifested_at,
    )
    .await?;
    let read_model = load_shipment_tx(&mut tx, access.tenant_id, shipment.id, &scope).await?;
    let manifest = read_model
        .manifest
        .ok_or_else(|| AppError::internal("manifested shipment has no manifest"))?;
    Ok(prepared
        .commit(
            tx,
            RecordManualManifestResult {
                shipment_id: shipment.id,
                order_id: shipment.order_id,
                status: next_status,
                revision: next_revision,
                manifest,
            },
        )
        .await?)
}

fn manifest_validation_error(error: ShippingError) -> AppError {
    match error {
        ShippingError::TrackingAssignmentSetMismatch | ShippingError::DuplicateTrackingNumber => {
            AppError::bad_request(error.to_string())
        }
        _ => AppError::conflict(error.to_string()),
    }
}

async fn lock_manifest_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: wareboxes_domain::ShipmentId,
) -> AppResult<Vec<ManifestCarton>> {
    let rows = sqlx::query(
        r#"
        SELECT id, carton_id, license_plate_id, carton_barcode, sequence,
               weight_g, length_mm, width_mm, height_mm
        FROM shipment_cartons
        WHERE tenant_id = $1 AND shipment_id = $2
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ManifestCarton {
                shipment_carton_id: row.try_get("id")?,
                carton_id: positive(row.try_get("carton_id")?, CartonId::new)?,
                license_plate_id: row.try_get("license_plate_id")?,
                carton_barcode: ShipmentScanValue::new(row.try_get::<String, _>("carton_barcode")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                sequence: row.try_get("sequence")?,
                weight_g: row.try_get("weight_g")?,
                length_mm: row.try_get("length_mm")?,
                width_mm: row.try_get("width_mm")?,
                height_mm: row.try_get("height_mm")?,
            })
        })
        .collect()
}

async fn lock_manifest_natural_keys_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &RecordManualManifestCommand,
) -> AppResult<()> {
    let mut keys = command
        .carton_tracking_assignments
        .iter()
        .map(|assignment| {
            format!(
                "shipment-tracking:{}:{}:{}",
                tenant_id,
                command.carrier_code,
                assignment.tracking_number()
            )
        })
        .collect::<Vec<_>>();
    keys.push(format!(
        "shipment-manifest:{}:{}:{}",
        tenant_id, command.carrier_code, command.manifest_reference
    ));
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn require_manifest_keys_available_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &RecordManualManifestCommand,
) -> AppResult<()> {
    let manifest_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM shipment_manifests
            WHERE tenant_id = $1 AND carrier = $2 AND manifest_number = $3
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.carrier_code.as_str())
    .bind(command.manifest_reference.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if manifest_exists {
        return Err(AppError::conflict(
            "carrier manifest reference is already in use",
        ));
    }
    let tracking_numbers = command
        .carton_tracking_assignments
        .iter()
        .map(|assignment| assignment.tracking_number().as_str())
        .collect::<Vec<_>>();
    let tracking_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM shipment_manifest_packages
            WHERE tenant_id = $1 AND carrier = $2 AND tracking_number = ANY($3)
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.carrier_code.as_str())
    .bind(&tracking_numbers)
    .fetch_one(&mut **tx)
    .await?;
    if tracking_exists {
        return Err(AppError::conflict(
            "carrier tracking number is already in use",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_manifest_packages_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment: &super::LockedShipment,
    manifest_id: CarrierManifestId,
    command: &RecordManualManifestCommand,
    cartons: &[ManifestCarton],
    manifested_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    let assignments = command
        .carton_tracking_assignments
        .iter()
        .map(|assignment| (assignment.carton_id(), assignment))
        .collect::<HashMap<CartonId, &CartonTrackingAssignment>>();
    for carton in cartons {
        let assignment = assignments
            .get(&carton.carton_id)
            .ok_or_else(|| AppError::internal("validated manifest assignment is missing"))?;
        sqlx::query(
            r#"
            INSERT INTO shipment_manifest_packages (
                tenant_id, inventory_owner_id, facility_id, shipment_id,
                manifest_id, shipment_carton_id, carton_id, license_plate_id,
                sequence, carrier, service, tracking_number, weight_g,
                length_mm, width_mm, height_mm, created_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, $16, $17
            )
            "#,
        )
        .bind(tenant_id.get())
        .bind(shipment.inventory_owner_id.get())
        .bind(shipment.facility_id.get())
        .bind(shipment.id.get())
        .bind(manifest_id.get())
        .bind(carton.shipment_carton_id)
        .bind(carton.carton_id.get())
        .bind(carton.license_plate_id)
        .bind(carton.sequence)
        .bind(command.carrier_code.as_str())
        .bind(command.service_code.as_ref().map(|code| code.as_str()))
        .bind(assignment.tracking_number().as_str())
        .bind(carton.weight_g)
        .bind(carton.length_mm)
        .bind(carton.width_mm)
        .bind(carton.height_mm)
        .bind(manifested_at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
