use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::shipping::{
    CreateShipmentCommand, CreateShipmentResult, CREATE_SHIPMENT_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    create_shipment as validate_creation, CartonId, OrderRevision, PackSessionStatus,
    ShipmentCartonIdentity, ShipmentId, ShipmentScanValue, TenantId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::inventory_locking;
use crate::repo::orders::insert_order_activity_tx;

use super::read_model::load_shipment_tx;
use super::{
    enqueue_order_event_tx, lock_order_tx, order_hint_for_session_tx, positive,
    require_replayed_shipment_visible_tx,
};

#[derive(Debug)]
struct ReadySession {
    inventory_owner_id: i64,
    facility_id: i64,
    order_release_id: i64,
    state: PackSessionStatus,
    revision: OrderRevision,
    carton_count: i64,
    content_count: i64,
    shipped_qty: i64,
}

#[derive(Debug)]
struct CartonSnapshot {
    carton_id: CartonId,
    license_plate_id: i64,
    carton_barcode: ShipmentScanValue,
    sequence: i64,
    content_count: i64,
    packed_qty: i64,
    weight_g: Option<i64>,
    length_mm: Option<i64>,
    width_mm: Option<i64>,
    height_mm: Option<i64>,
}

pub async fn create_shipment(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateShipmentCommand,
) -> AppResult<CreateShipmentResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_SHIPMENT_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared.replayed::<CreateShipmentResult>(&mut tx).await? {
        require_replayed_shipment_visible_tx(&mut tx, access.tenant_id, &result.shipment, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let hinted_order_id =
        order_hint_for_session_tx(&mut tx, access.tenant_id, command.packing_session_id).await?;
    if hinted_order_id != command.order_id {
        return Err(AppError::not_found("packing session"));
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "shipment-order:{}:{}",
            access.tenant_id, command.order_id
        ))
        .execute(&mut *tx)
        .await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, command.order_id, &scope).await?;
    let session = lock_ready_session_tx(&mut tx, access.tenant_id, command).await?;
    if session.inventory_owner_id != order.inventory_owner_id.get()
        || !scope.includes_facility(session.facility_id)
    {
        return Err(AppError::not_found("packing session"));
    }
    if order.revision != command.expected_revision || session.revision != command.expected_revision
    {
        return Err(AppError::conflict("shipment creation revision is stale"));
    }
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM shipments WHERE tenant_id = $1 AND order_id = $2)",
    )
    .bind(access.tenant_id.get())
    .bind(command.order_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if existing {
        return Err(AppError::conflict("order already has a shipment"));
    }

    let cartons =
        lock_carton_snapshots_tx(&mut tx, access.tenant_id, command.packing_session_id.get())
            .await?;
    let identities = cartons
        .iter()
        .map(|carton| ShipmentCartonIdentity::new(carton.carton_id, carton.carton_barcode.clone()))
        .collect::<Vec<_>>();
    let shipment_status = validate_creation(order.status, session.state, &identities)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    validate_session_totals(&session, &cartons)?;
    let (origin_address_id, destination_address_id) = lock_shipping_addresses_tx(
        &mut tx,
        access.tenant_id,
        session.facility_id,
        command.order_id.get(),
    )
    .await?;

    let resulting_revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let created_at = now_iso();
    let shipment_id_raw: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO shipments (
            tenant_id, inventory_owner_id, facility_id, packing_session_id,
            order_release_id, order_id, state, revision,
            creation_expected_order_revision, creation_resulting_order_revision,
            carton_count, content_count, shipped_qty,
            created_by_user_id, created_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, 1, $8, $9, $10, $11, $12, $13, $14
        ) RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order.inventory_owner_id.get())
    .bind(session.facility_id)
    .bind(command.packing_session_id.get())
    .bind(session.order_release_id)
    .bind(command.order_id.get())
    .bind(shipment_status.as_str())
    .bind(order.revision.get())
    .bind(resulting_revision.get())
    .bind(session.carton_count)
    .bind(session.content_count)
    .bind(session.shipped_qty)
    .bind(context.actor_id.get())
    .bind(created_at)
    .fetch_one(&mut *tx)
    .await?;
    let shipment_id = positive(shipment_id_raw, ShipmentId::new)?;
    insert_address_snapshot_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id.get(),
        session.facility_id,
        shipment_id,
        "origin",
        origin_address_id,
    )
    .await?;
    insert_address_snapshot_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id.get(),
        session.facility_id,
        shipment_id,
        "destination",
        destination_address_id,
    )
    .await?;
    insert_carton_snapshots_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id.get(),
        session.facility_id,
        shipment_id,
        command.packing_session_id.get(),
        &cartons,
    )
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE orders SET revision = $1
        WHERE tenant_id = $2 AND id = $3 AND status = $4 AND revision = $5
        "#,
    )
    .bind(resulting_revision.get())
    .bind(access.tenant_id.get())
    .bind(order.id.get())
    .bind(order.status.as_str())
    .bind(order.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "order changed while creating its shipment",
        ));
    }
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        order.id.get(),
        Some(context.actor_id.get()),
        &format!(
            "created shipment {shipment_id} from {} closed carton(s)",
            session.carton_count
        ),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        positive(session.facility_id, wareboxes_domain::FacilityId::new)?,
        context.actor_id.get(),
        order.id,
        "shipping.shipment_created",
        &format!("shipment:{}:created", shipment_id.get()),
        serde_json::json!({
            "shipment_id": shipment_id,
            "packing_session_id": command.packing_session_id,
            "order_id": order.id,
            "order_key": order.order_key,
            "facility_id": session.facility_id,
            "carton_count": session.carton_count,
            "shipped_quantity": session.shipped_qty,
            "expected_order_revision": order.revision,
            "order_revision": resulting_revision,
            "created_at": created_at,
        }),
        created_at,
    )
    .await?;
    let shipment = load_shipment_tx(&mut tx, access.tenant_id, shipment_id, &scope).await?;
    Ok(prepared
        .commit(
            tx,
            CreateShipmentResult {
                shipment,
                order_status: order.status,
                order_revision: resulting_revision,
            },
        )
        .await?)
}

async fn lock_ready_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &CreateShipmentCommand,
) -> AppResult<ReadySession> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, order_release_id, state, revision,
               closed_carton_count, packed_allocation_count, packed_qty
        FROM packing_sessions
        WHERE tenant_id = $1 AND id = $2 AND order_id = $3
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.packing_session_id.get())
    .bind(command.order_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing session"))?;
    let state = match row.try_get::<String, _>("state")?.as_str() {
        "open" => PackSessionStatus::Open,
        "ready_to_manifest" => PackSessionStatus::ReadyToManifest,
        _ => return Err(AppError::internal("packing session has an invalid state")),
    };
    Ok(ReadySession {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        order_release_id: row.try_get("order_release_id")?,
        state,
        revision: positive(row.try_get("revision")?, OrderRevision::new)?,
        carton_count: row.try_get("closed_carton_count")?,
        content_count: row.try_get("packed_allocation_count")?,
        shipped_qty: row.try_get("packed_qty")?,
    })
}

async fn lock_carton_snapshots_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: i64,
) -> AppResult<Vec<CartonSnapshot>> {
    let carton_rows = sqlx::query(
        r#"
        SELECT carton.id, carton.license_plate_id
        FROM cartons carton
        WHERE carton.tenant_id = $1 AND carton.packing_session_id = $2
          AND carton.state = 'closed'
        ORDER BY carton.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id)
    .fetch_all(&mut **tx)
    .await?;
    let carton_ids = carton_rows
        .iter()
        .map(|row| row.try_get("id"))
        .collect::<Result<Vec<i64>, _>>()?;
    let license_plate_ids = carton_rows
        .iter()
        .map(|row| row.try_get("license_plate_id"))
        .collect::<Result<Vec<i64>, _>>()?;
    inventory_locking::lock_license_plates(tx, tenant_id, license_plate_ids).await?;
    let locked_carton_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM cartons
        WHERE tenant_id = $1 AND id = ANY($2)
        ORDER BY id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(&carton_ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked_carton_ids != carton_ids {
        return Err(AppError::conflict(
            "packing cartons changed before shipment creation",
        ));
    }
    let rows = sqlx::query(
        r#"
        SELECT carton.id, carton.license_plate_id, plate.barcode,
               ROW_NUMBER() OVER (ORDER BY carton.id)::BIGINT AS sequence,
               COUNT(content.id)::BIGINT AS content_count,
               COALESCE(SUM(content.packed_qty), 0)::BIGINT AS packed_qty,
               carton.weight_g, carton.length_mm, carton.width_mm, carton.height_mm
        FROM cartons carton
        INNER JOIN license_plates plate
          ON plate.tenant_id = carton.tenant_id
         AND plate.inventory_owner_id = carton.inventory_owner_id
         AND plate.facility_id = carton.facility_id
         AND plate.id = carton.license_plate_id
        LEFT JOIN carton_contents content
          ON content.tenant_id = carton.tenant_id
         AND content.inventory_owner_id = carton.inventory_owner_id
         AND content.facility_id = carton.facility_id
         AND content.packing_session_id = carton.packing_session_id
         AND content.carton_id = carton.id
        WHERE carton.tenant_id = $1 AND carton.packing_session_id = $2
          AND carton.state = 'closed' AND plate.deleted IS NULL
          AND plate.barcode IS NOT NULL
        GROUP BY carton.id, plate.id, plate.barcode
        ORDER BY carton.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != carton_ids.len() {
        return Err(AppError::conflict(
            "packing cartons changed before shipment creation",
        ));
    }
    rows.into_iter()
        .map(|row| {
            Ok(CartonSnapshot {
                carton_id: positive(row.try_get("id")?, CartonId::new)?,
                license_plate_id: row.try_get("license_plate_id")?,
                carton_barcode: ShipmentScanValue::new(row.try_get::<String, _>("barcode")?)
                    .map_err(|error| AppError::conflict(error.to_string()))?,
                sequence: row.try_get("sequence")?,
                content_count: row.try_get("content_count")?,
                packed_qty: row.try_get("packed_qty")?,
                weight_g: row.try_get("weight_g")?,
                length_mm: row.try_get("length_mm")?,
                width_mm: row.try_get("width_mm")?,
                height_mm: row.try_get("height_mm")?,
            })
        })
        .collect()
}

fn validate_session_totals(session: &ReadySession, cartons: &[CartonSnapshot]) -> AppResult<()> {
    let carton_count = i64::try_from(cartons.len())
        .map_err(|_| AppError::internal("shipment carton count exceeds i64"))?;
    let (content_count, shipped_qty) =
        cartons
            .iter()
            .try_fold((0_i64, 0_i64), |(content_total, quantity_total), carton| {
                Ok::<_, AppError>((
                    content_total
                        .checked_add(carton.content_count)
                        .ok_or_else(|| AppError::internal("shipment content count exceeds i64"))?,
                    quantity_total
                        .checked_add(carton.packed_qty)
                        .ok_or_else(|| AppError::internal("shipment quantity exceeds i64"))?,
                ))
            })?;
    if carton_count != session.carton_count
        || content_count != session.content_count
        || shipped_qty != session.shipped_qty
        || cartons
            .iter()
            .any(|carton| carton.content_count <= 0 || carton.packed_qty <= 0)
    {
        return Err(AppError::conflict(
            "packing session carton contents changed before shipment creation",
        ));
    }
    Ok(())
}

async fn lock_shipping_addresses_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: i64,
    order_id: i64,
) -> AppResult<(i64, i64)> {
    let row = sqlx::query(
        r#"
        SELECT facility.address_id AS origin_address_id,
               order_header.address_id AS destination_address_id
        FROM facilities facility
        INNER JOIN orders order_header
          ON order_header.tenant_id = facility.tenant_id
         AND order_header.id = $3 AND order_header.deleted IS NULL
        WHERE facility.tenant_id = $1 AND facility.id = $2
          AND facility.deleted IS NULL
        FOR SHARE OF facility
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("shipment facility or order is no longer active"))?;
    let origin_address_id = row
        .try_get::<Option<i64>, _>("origin_address_id")?
        .ok_or_else(|| AppError::conflict("shipping facility has no origin address"))?;
    let destination_address_id: i64 = row.try_get("destination_address_id")?;
    let mut address_ids = vec![origin_address_id, destination_address_id];
    address_ids.sort_unstable();
    address_ids.dedup();
    let complete_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM addresses
        WHERE tenant_id = $1 AND id = ANY($2) AND deleted IS NULL
          AND (NULLIF(btrim(name), '') IS NOT NULL
               OR NULLIF(btrim(company), '') IS NOT NULL)
          AND line1 IS NOT NULL AND btrim(line1) <> ''
          AND city IS NOT NULL AND btrim(city) <> ''
          AND postal_code IS NOT NULL AND btrim(postal_code) <> ''
          AND country IS NOT NULL AND btrim(country) <> ''
        ORDER BY id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(&address_ids)
    .fetch_all(&mut **tx)
    .await?;
    if complete_ids != address_ids {
        return Err(AppError::conflict(
            "shipment origin and destination addresses must be complete",
        ));
    }
    Ok((origin_address_id, destination_address_id))
}

#[allow(clippy::too_many_arguments)]
async fn insert_address_snapshot_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    shipment_id: ShipmentId,
    role: &str,
    source_address_id: i64,
) -> AppResult<()> {
    let inserted = sqlx::query(
        r#"
        INSERT INTO shipment_address_snapshots (
            tenant_id, inventory_owner_id, facility_id, shipment_id,
            address_role, source_address_id, name, company, line1, line2,
            postal_code, country, phone, email, state, county, city,
            territory, district
        )
        SELECT $1, $2, $3, $4, $5, address.id, address.name, address.company,
               address.line1, address.line2, address.postal_code, address.country,
               address.phone, address.email, address.state, address.county,
               address.city, address.territory, address.district
        FROM addresses address
        WHERE address.tenant_id = $1 AND address.id = $6 AND address.deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(shipment_id.get())
    .bind(role)
    .bind(source_address_id)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(AppError::conflict(
            "shipment address changed before snapshot",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_carton_snapshots_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    shipment_id: ShipmentId,
    packing_session_id: i64,
    cartons: &[CartonSnapshot],
) -> AppResult<()> {
    for carton in cartons {
        sqlx::query(
            r#"
            INSERT INTO shipment_cartons (
                tenant_id, inventory_owner_id, facility_id, shipment_id,
                packing_session_id, carton_id, license_plate_id, carton_barcode,
                sequence, content_count, packed_qty, weight_g,
                length_mm, width_mm, height_mm
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15
            )
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(shipment_id.get())
        .bind(packing_session_id)
        .bind(carton.carton_id.get())
        .bind(carton.license_plate_id)
        .bind(carton.carton_barcode.as_str())
        .bind(carton.sequence)
        .bind(carton.content_count)
        .bind(carton.packed_qty)
        .bind(carton.weight_g)
        .bind(carton.length_mm)
        .bind(carton.width_mm)
        .bind(carton.height_mm)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
