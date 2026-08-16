use wareboxes_application::picking::ConfirmPickContentCommand;
use wareboxes_domain::{PickScanValue, TenantId};

use super::PickTarget;
use crate::error::{AppError, AppResult};

pub(super) async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    command: &ConfirmPickContentCommand,
) -> AppResult<()> {
    let source_barcode: Option<String> = sqlx::query_scalar(
        r#"SELECT barcode FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
          AND deleted IS NULL AND active AND pickable FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(target.facility_id)
    .bind(target.source_location_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let source_barcode = source_barcode.ok_or_else(|| {
        AppError::conflict("directed source location is no longer available for picking")
    })?;
    match command.source_location_barcode.as_ref() {
        Some(scanned) if source_barcode != scanned.as_str() => {
            return Err(AppError::bad_request(
                "scanned source location does not match the directed pick",
            ));
        }
        None if target.pick_policy.require_source_location_scan => {
            return Err(AppError::bad_request(
                "the effective Pick policy requires a source location scan",
            ));
        }
        _ => {}
    }
    if let Some(item_barcode) = command.item_barcode.as_ref() {
        let item_matches: bool = sqlx::query_scalar(
            r#"SELECT EXISTS (SELECT 1 FROM barcodes
            WHERE tenant_id=$1 AND item_id=$2 AND deleted IS NULL AND name=$3)"#,
        )
        .bind(tenant_id.get())
        .bind(target.item_id)
        .bind(item_barcode.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if !item_matches {
            return Err(AppError::bad_request(
                "scanned item does not match the directed pick",
            ));
        }
    } else if target.pick_policy.require_item_scan {
        return Err(AppError::bad_request(
            "the effective Pick policy requires an item scan",
        ));
    }
    match (
        target.source_license_plate_id,
        command.source_license_plate_barcode.as_ref(),
    ) {
        (None, None) => {}
        (Some(plate_id), Some(scanned)) => {
            let barcode: Option<String> = sqlx::query_scalar(
                r#"SELECT barcode FROM license_plates
                WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
                  AND id=$4 AND location_id=$5 AND deleted IS NULL FOR UPDATE"#,
            )
            .bind(tenant_id.get())
            .bind(target.inventory_owner_id.get())
            .bind(target.facility_id)
            .bind(plate_id.get())
            .bind(target.source_location_id.get())
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
            let barcode = barcode.ok_or_else(|| {
                AppError::conflict("directed source license plate is no longer available")
            })?;
            if barcode != scanned.as_str() {
                return Err(AppError::bad_request(
                    "scanned source license plate does not match the directed pick",
                ));
            }
        }
        _ => {
            return Err(AppError::bad_request(
                "source license plate scan does not match the directed pick",
            ));
        }
    }
    Ok(())
}

pub(super) async fn resolve_destination_barcode_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    command: &ConfirmPickContentCommand,
) -> AppResult<PickScanValue> {
    if let Some(scanned) = command.destination_license_plate_barcode.as_ref() {
        return Ok(scanned.clone());
    }
    if target.pick_policy.require_destination_container_scan {
        return Err(AppError::bad_request(
            "the effective Pick policy requires a destination container scan",
        ));
    }
    let barcodes: Vec<String> = sqlx::query_scalar(
        r#"SELECT plate.barcode FROM outbound_order_containers container
        INNER JOIN license_plates plate
          ON plate.tenant_id=container.tenant_id
         AND plate.inventory_owner_id=container.inventory_owner_id
         AND plate.facility_id=container.facility_id
         AND plate.id=container.license_plate_id AND plate.deleted IS NULL
        WHERE container.tenant_id=$1 AND container.inventory_owner_id=$2
          AND container.facility_id=$3 AND container.order_release_id=$4
          AND container.order_id=$5 AND container.destination_location_id=$6
          AND container.released_at IS NULL ORDER BY container.id LIMIT 2"#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.release_id)
    .bind(target.order_id.get())
    .bind(target.destination_location_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if barcodes.len() != 1 {
        return Err(AppError::conflict(
            "destination scan can only be omitted when one staged order container is available",
        ));
    }
    PickScanValue::new(barcodes[0].clone()).map_err(|error| AppError::internal(error.to_string()))
}
