use sqlx::Row;
use wareboxes_domain::{LicensePlateId, TenantId};
use wareboxes_persistence_postgres::db::now_iso;

use crate::error::{AppError, AppResult};

use super::{DestinationPlate, PickTarget};

pub(super) async fn lock_full_pallet_source_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    scanned_destination_barcode: &str,
) -> AppResult<Option<DestinationPlate>> {
    let Some(source_plate_id) = target.source_license_plate_id else {
        return Ok(None);
    };
    let row = sqlx::query(
        r#"SELECT plate.barcode,plate.location_id,plate.parent_license_plate_id,
          balance.qty_on_hand,balance.qty_reserved,balance.qty_held,
          EXISTS(SELECT 1 FROM license_plates child
            WHERE child.tenant_id=plate.tenant_id
              AND child.inventory_owner_id=plate.inventory_owner_id
              AND child.facility_id=plate.facility_id
              AND child.parent_license_plate_id=plate.id AND child.deleted IS NULL) AS has_child,
          (SELECT COUNT(*) FROM inventory_balances position
            WHERE position.tenant_id=plate.tenant_id
              AND position.inventory_owner_id=plate.inventory_owner_id
              AND position.facility_id=plate.facility_id
              AND position.license_plate_id=plate.id
              AND position.deleted IS NULL AND position.qty_on_hand>0) AS position_count
        FROM license_plates plate
        JOIN inventory_balances balance ON balance.tenant_id=plate.tenant_id
          AND balance.inventory_owner_id=plate.inventory_owner_id
          AND balance.facility_id=plate.facility_id
          AND balance.license_plate_id=plate.id AND balance.id=$4
          AND balance.deleted IS NULL
        WHERE plate.tenant_id=$1 AND plate.inventory_owner_id=$2
          AND plate.facility_id=$3 AND plate.id=$5 AND plate.deleted IS NULL"#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.source_balance_id.get())
    .bind(source_plate_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("source license plate is no longer available"))?;
    let source_barcode: String = row.try_get("barcode")?;
    let eligible = row.try_get::<i64, _>("location_id")? == target.source_location_id.get()
        && row
            .try_get::<Option<i64>, _>("parent_license_plate_id")?
            .is_none()
        && !row.try_get::<bool, _>("has_child")?
        && row.try_get::<i64, _>("position_count")? == 1
        && row.try_get::<i64, _>("qty_on_hand")? == target.quantity.get()
        && row.try_get::<i64, _>("qty_reserved")? == target.quantity.get()
        && row.try_get::<i64, _>("qty_held")? == 0;
    if eligible {
        if scanned_destination_barcode != source_barcode {
            return Err(AppError::bad_request(
                "a full pallet pick must retain its directed source license plate",
            ));
        }
        return Ok(Some(DestinationPlate {
            id: source_plate_id,
        }));
    }
    if scanned_destination_barcode == source_barcode {
        return Err(AppError::conflict(
            "source license plate is no longer eligible for a full pallet pick",
        ));
    }
    Ok(None)
}

pub(super) async fn lock_destination_plate_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    barcode: &str,
) -> AppResult<DestinationPlate> {
    let row = sqlx::query(
        r#"
        SELECT plate.id, plate.location_id,
               location.active, location.pickable, location.barcode, location.type
        FROM license_plates plate
        INNER JOIN locations location
          ON location.tenant_id = plate.tenant_id
         AND location.facility_id = plate.facility_id
         AND location.id = plate.location_id AND location.deleted IS NULL
        WHERE plate.tenant_id = $1 AND plate.inventory_owner_id = $2
          AND plate.facility_id = $3 AND plate.barcode = $4
          AND plate.deleted IS NULL
        FOR UPDATE OF plate
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(barcode)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        AppError::bad_request("scanned destination license plate is not available in this facility")
    })?;
    let location_id: i64 = row.try_get("location_id")?;
    if location_id != target.destination_location_id.get()
        || !row.try_get::<bool, _>("active")?
        || row.try_get::<bool, _>("pickable")?
        || !matches!(
            row.try_get::<String, _>("type")?
                .to_ascii_lowercase()
                .as_str(),
            "staging" | "packing"
        )
        || row
            .try_get::<Option<String>, _>("barcode")?
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(AppError::conflict(
            "destination license plate is not at the directed staging location",
        ));
    }
    let id = LicensePlateId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    if Some(id) == target.source_license_plate_id {
        return Err(AppError::conflict(
            "source and destination license plates must differ",
        ));
    }
    Ok(DestinationPlate { id })
}

pub(super) async fn move_full_pallet_header_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    target: &PickTarget,
    pallet_id: LicensePlateId,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE license_plates SET location_id=$1
        WHERE tenant_id=$2 AND inventory_owner_id=$3 AND facility_id=$4
          AND id=$5 AND location_id=$6 AND parent_license_plate_id IS NULL
          AND deleted IS NULL AND NOT EXISTS(
            SELECT 1 FROM license_plates child
            WHERE child.tenant_id=license_plates.tenant_id
              AND child.inventory_owner_id=license_plates.inventory_owner_id
              AND child.facility_id=license_plates.facility_id
              AND child.parent_license_plate_id=license_plates.id
              AND child.deleted IS NULL)"#,
    )
    .bind(target.destination_location_id.get())
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(pallet_id.get())
    .bind(target.source_location_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "full pallet location changed during pick confirmation",
        ));
    }
    Ok(())
}

pub(super) async fn bind_outbound_order_container_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    target: &PickTarget,
    destination_plate_id: LicensePlateId,
    full_pallet_pick: bool,
) -> AppResult<()> {
    let existing = sqlx::query(
        r#"
        SELECT order_release_id, order_id, destination_location_id
        FROM outbound_order_containers
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND facility_id = $3 AND license_plate_id = $4
          AND released_at IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(destination_plate_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(existing) = existing {
        let matches_order = existing.try_get::<i64, _>("order_release_id")? == target.release_id
            && existing.try_get::<i64, _>("order_id")? == target.order_id.get()
            && existing.try_get::<i64, _>("destination_location_id")?
                == target.destination_location_id.get();
        return if matches_order {
            Ok(())
        } else {
            Err(AppError::conflict(
                "destination license plate is assigned to another outbound order",
            ))
        };
    }

    let occupied_balance_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM inventory_balances
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND facility_id = $3 AND license_plate_id = $4
          AND deleted IS NULL AND qty_on_hand > 0
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(destination_plate_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if (!full_pallet_pick && !occupied_balance_ids.is_empty())
        || (full_pallet_pick && occupied_balance_ids.as_slice() != [target.source_balance_id.get()])
    {
        return Err(AppError::conflict(
            "outbound license plate contents do not match the directed pick",
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO outbound_order_containers (
            tenant_id, inventory_owner_id, facility_id, order_release_id,
            order_id, destination_location_id, license_plate_id,
            created_by_user_id, created_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id.get())
    .bind(target.facility_id)
    .bind(target.release_id)
    .bind(target.order_id.get())
    .bind(target.destination_location_id.get())
    .bind(destination_plate_id.get())
    .bind(actor_user_id)
    .bind(now_iso())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
