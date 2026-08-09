use std::collections::{BTreeMap, BTreeSet};

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbound_load::{
    PlanOutboundLoadCommand, PlanOutboundLoadResult, PLAN_OUTBOUND_LOAD_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{FacilityId, OutboundLoadId, Timestamp};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

use super::{positive, read_model::load_read_model_tx, require_load_visible_tx};

#[derive(Debug)]
struct ShipmentPlanRow {
    shipment_id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    order_id: i64,
    shipment_state: String,
    shipment_revision: i64,
    order_state: String,
    order_revision: i64,
    carton_count: i64,
    shipped_qty: i64,
    carrier: String,
}

#[derive(Debug)]
struct CartonPlanRow {
    shipment_id: i64,
    shipment_carton_id: i64,
    carton_id: i64,
    license_plate_id: i64,
    carton_barcode: String,
    content_count: i64,
    packed_qty: i64,
    original_location_id: i64,
}

pub async fn plan(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanOutboundLoadCommand,
) -> AppResult<PlanOutboundLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, PLAN_OUTBOUND_LOAD_OPERATION, command)?;
    validate_plan_shape(command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        "wms_supervisor",
    )
    .await?;
    if let Some(result) = prepared.replayed::<PlanOutboundLoadResult>(&mut tx).await? {
        require_load_visible_tx(
            &mut tx,
            access.tenant_id,
            result.outbound_load.outbound_load_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }
    if !scope.includes_facility(command.facility_id.get()) {
        return Err(AppError::not_found("facility"));
    }
    lock_reference_tx(&mut tx, access.tenant_id, command.load_reference.as_str()).await?;
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM outbound_loads WHERE tenant_id=$1 AND load_reference=$2)",
    )
    .bind(access.tenant_id.get())
    .bind(command.load_reference.as_str())
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(AppError::conflict(
            "outbound load reference is already in use",
        ));
    }
    lock_staging_location_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.staging_location_id.get(),
    )
    .await?;

    let shipment_ids = command
        .shipments
        .iter()
        .map(|shipment| shipment.shipment_id.get())
        .collect::<Vec<_>>();
    let shipment_rows = lock_shipments_tx(&mut tx, access.tenant_id, &shipment_ids).await?;
    if shipment_rows.len() != command.shipments.len() {
        return Err(AppError::not_found("shipment"));
    }
    let shipment_by_id = shipment_rows
        .into_iter()
        .map(|row| (row.shipment_id, row))
        .collect::<BTreeMap<_, _>>();
    validate_shipments(command, &shipment_by_id, &scope)?;
    if sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM outbound_load_shipments
            WHERE tenant_id=$1 AND shipment_id=ANY($2) AND closed_at IS NULL
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(&shipment_ids)
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(AppError::conflict(
            "shipment is already assigned to an active outbound load",
        ));
    }

    let carton_rows = lock_cartons_tx(&mut tx, access.tenant_id, &shipment_ids).await?;
    validate_carton_sets(command, &carton_rows)?;
    let planned_at = now_iso();
    let load_barcode = format!("OUTBOUND:{}", command.load_reference.as_str());
    let virtual_location_id = create_virtual_trailer_location_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        &load_barcode,
        command.load_reference.as_str(),
        planned_at,
    )
    .await?;
    let shipment_count = i64::try_from(command.shipments.len())
        .map_err(|_| AppError::bad_request("outbound load has too many shipments"))?;
    let carton_count = i64::try_from(carton_rows.len())
        .map_err(|_| AppError::bad_request("outbound load has too many cartons"))?;
    let load_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO outbound_loads (
            tenant_id, facility_id, load_reference, load_barcode, carrier,
            state, revision, staging_lane_location_id,
            virtual_trailer_location_id, scheduled_departure_at,
            shipment_count, carton_count, planned_by_user_id, planned_at
        ) VALUES ($1,$2,$3,$4,$5,'planned',1,$6,$7,$8,$9,$10,$11,$12)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.load_reference.as_str())
    .bind(&load_barcode)
    .bind(command.carrier_code.as_str())
    .bind(command.staging_location_id.get())
    .bind(virtual_location_id)
    .bind(command.scheduled_departure_at)
    .bind(shipment_count)
    .bind(carton_count)
    .bind(context.actor_id.get())
    .bind(planned_at)
    .fetch_one(&mut *tx)
    .await?;
    let load_id = positive(load_id, OutboundLoadId::new)?;
    let carton_by_shipment = carton_rows.into_iter().fold(
        BTreeMap::<i64, Vec<CartonPlanRow>>::new(),
        |mut rows, carton| {
            rows.entry(carton.shipment_id).or_default().push(carton);
            rows
        },
    );
    for planned_shipment in &command.shipments {
        let shipment = shipment_by_id
            .get(&planned_shipment.shipment_id.get())
            .ok_or_else(|| AppError::not_found("shipment"))?;
        let link_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO outbound_load_shipments (
                tenant_id, inventory_owner_id, facility_id, outbound_load_id,
                shipment_id, order_id, shipment_sequence,
                expected_shipment_revision, expected_order_revision,
                carton_count, shipped_qty, carrier
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(shipment.inventory_owner_id)
        .bind(shipment.facility_id)
        .bind(load_id.get())
        .bind(shipment.shipment_id)
        .bind(shipment.order_id)
        .bind(i64::from(planned_shipment.shipment_sequence))
        .bind(planned_shipment.expected_shipment_revision.get())
        .bind(planned_shipment.expected_order_revision.get())
        .bind(shipment.carton_count)
        .bind(shipment.shipped_qty)
        .bind(&shipment.carrier)
        .fetch_one(&mut *tx)
        .await?;
        let plan_sequences = planned_shipment
            .cartons
            .iter()
            .map(|carton| (carton.carton_id.get(), carton.load_sequence))
            .collect::<BTreeMap<_, _>>();
        for carton in carton_by_shipment
            .get(&shipment.shipment_id)
            .ok_or_else(|| AppError::conflict("shipment has no packed cartons"))?
        {
            let load_sequence = plan_sequences
                .get(&carton.carton_id)
                .copied()
                .ok_or_else(|| AppError::bad_request("load carton set does not match shipment"))?;
            sqlx::query(
                r#"
                INSERT INTO outbound_load_cartons (
                    tenant_id, inventory_owner_id, facility_id, outbound_load_id,
                    outbound_load_shipment_id, shipment_id, shipment_carton_id,
                    carton_id, license_plate_id, carton_barcode,
                    shipment_sequence, load_sequence, original_location_id,
                    content_count, packed_qty, state, revision
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,'planned',1)
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(shipment.inventory_owner_id)
            .bind(shipment.facility_id)
            .bind(load_id.get())
            .bind(link_id)
            .bind(shipment.shipment_id)
            .bind(carton.shipment_carton_id)
            .bind(carton.carton_id)
            .bind(carton.license_plate_id)
            .bind(&carton.carton_barcode)
            .bind(i64::from(planned_shipment.shipment_sequence))
            .bind(i64::from(load_sequence))
            .bind(carton.original_location_id)
            .bind(carton.content_count)
            .bind(carton.packed_qty)
            .execute(&mut *tx)
            .await?;
        }
    }
    let outbound_load = load_read_model_tx(&mut tx, access.tenant_id, load_id).await?;
    let result = PlanOutboundLoadResult { outbound_load };
    super::enqueue_load_event_tx(
        &mut tx,
        super::LoadEvent {
            tenant_id: access.tenant_id,
            facility_id: command.facility_id,
            actor_user_id: context.actor_id.get(),
            load_id,
            event_type: "outbound.load.planned",
            event_key: "planned",
            payload: serde_json::to_value(&result)
                .map_err(|error| AppError::internal(error.to_string()))?,
            occurred_at: planned_at,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

fn validate_plan_shape(command: &PlanOutboundLoadCommand) -> AppResult<()> {
    if command.shipments.is_empty() {
        return Err(AppError::bad_request("outbound load requires shipments"));
    }
    let shipment_sequences = command
        .shipments
        .iter()
        .map(|shipment| shipment.shipment_sequence)
        .collect::<BTreeSet<_>>();
    let shipment_count = u32::try_from(command.shipments.len())
        .map_err(|_| AppError::bad_request("outbound load has too many shipments"))?;
    if shipment_sequences.len() != command.shipments.len()
        || !shipment_sequences.iter().copied().eq(1..=shipment_count)
    {
        return Err(AppError::bad_request(
            "shipment sequences must be unique and contiguous",
        ));
    }
    let mut shipment_ids = BTreeSet::new();
    let mut carton_ids = BTreeSet::new();
    let mut load_sequences = BTreeSet::new();
    for shipment in &command.shipments {
        if !shipment_ids.insert(shipment.shipment_id.get()) || shipment.cartons.is_empty() {
            return Err(AppError::bad_request(
                "outbound load shipment set is invalid",
            ));
        }
        for carton in &shipment.cartons {
            if carton.load_sequence == 0
                || !carton_ids.insert(carton.carton_id.get())
                || !load_sequences.insert(carton.load_sequence)
            {
                return Err(AppError::bad_request("outbound load carton set is invalid"));
            }
        }
    }
    let carton_count = u32::try_from(load_sequences.len())
        .map_err(|_| AppError::bad_request("outbound load has too many cartons"))?;
    if !load_sequences.iter().copied().eq(1..=carton_count) {
        return Err(AppError::bad_request(
            "load sequences must be unique and contiguous",
        ));
    }
    Ok(())
}

async fn lock_reference_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    reference: &str,
) -> AppResult<()> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('outbound-load:' || $1 || ':' || $2, 0))",
    )
    .bind(tenant_id.get().to_string())
    .bind(reference)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn lock_staging_location_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    facility_id: FacilityId,
    location_id: i64,
) -> AppResult<()> {
    let valid: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT active AND deleted IS NULL AND barcode IS NOT NULL
               AND lower(type)='staging' AND NOT pickable AND NOT receivable
        FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(location_id)
    .fetch_optional(&mut **tx)
    .await?;
    if valid != Some(true) {
        return Err(AppError::conflict("staging lane is not available"));
    }
    Ok(())
}

async fn lock_shipments_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    shipment_ids: &[i64],
) -> AppResult<Vec<ShipmentPlanRow>> {
    let order_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT order_id FROM shipments WHERE tenant_id=$1 AND id=ANY($2) ORDER BY order_id",
    )
    .bind(tenant_id.get())
    .bind(shipment_ids)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query("SELECT id FROM orders WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE")
        .bind(tenant_id.get())
        .bind(&order_ids)
        .fetch_all(&mut **tx)
        .await?;
    let rows = sqlx::query(
        r#"
        SELECT shipment.id AS shipment_id, shipment.inventory_owner_id,
               shipment.facility_id, shipment.order_id,
               shipment.state AS shipment_state, shipment.revision AS shipment_revision,
               orders.status AS order_state, orders.revision AS order_revision,
               shipment.carton_count, shipment.shipped_qty, manifest.carrier
        FROM shipments shipment
        JOIN orders
          ON orders.tenant_id=shipment.tenant_id
         AND orders.inventory_owner_id=shipment.inventory_owner_id
         AND orders.id=shipment.order_id AND orders.deleted IS NULL
        JOIN shipment_manifests manifest
          ON manifest.tenant_id=shipment.tenant_id
         AND manifest.inventory_owner_id=shipment.inventory_owner_id
         AND manifest.shipment_id=shipment.id
        JOIN inventory_owners owner
          ON owner.tenant_id=shipment.tenant_id
         AND owner.id=shipment.inventory_owner_id AND owner.deleted IS NULL
        JOIN inventory_owner_facilities assignment
          ON assignment.tenant_id=shipment.tenant_id
         AND assignment.inventory_owner_id=shipment.inventory_owner_id
         AND assignment.facility_id=shipment.facility_id
         AND assignment.deleted IS NULL
        WHERE shipment.tenant_id=$1 AND shipment.id=ANY($2)
        ORDER BY shipment.id
        FOR UPDATE OF shipment
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ShipmentPlanRow {
                shipment_id: row.try_get("shipment_id")?,
                inventory_owner_id: row.try_get("inventory_owner_id")?,
                facility_id: row.try_get("facility_id")?,
                order_id: row.try_get("order_id")?,
                shipment_state: row.try_get("shipment_state")?,
                shipment_revision: row.try_get("shipment_revision")?,
                order_state: row.try_get("order_state")?,
                order_revision: row.try_get("order_revision")?,
                carton_count: row.try_get("carton_count")?,
                shipped_qty: row.try_get("shipped_qty")?,
                carrier: row.try_get("carrier")?,
            })
        })
        .collect()
}

fn validate_shipments(
    command: &PlanOutboundLoadCommand,
    rows: &BTreeMap<i64, ShipmentPlanRow>,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<()> {
    for planned in &command.shipments {
        let row = rows
            .get(&planned.shipment_id.get())
            .ok_or_else(|| AppError::not_found("shipment"))?;
        if row.facility_id != command.facility_id.get()
            || !scope.includes_inventory_owner(row.inventory_owner_id)
        {
            return Err(AppError::not_found("shipment"));
        }
        if row.shipment_state != "manifested"
            || row.order_state != "awaiting shipment"
            || row.shipment_revision != planned.expected_shipment_revision.get()
            || row.order_revision != planned.expected_order_revision.get()
            || row.carrier != command.carrier_code.as_str()
        {
            return Err(AppError::conflict(
                "shipment is not ready for outbound loading",
            ));
        }
    }
    Ok(())
}

async fn lock_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    shipment_ids: &[i64],
) -> AppResult<Vec<CartonPlanRow>> {
    let shipment_carton_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM shipment_cartons
        WHERE tenant_id=$1 AND shipment_id=ANY($2)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_ids)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        SELECT position.id
        FROM packed_inventory_positions position
        JOIN shipment_cartons shipment_carton
          ON shipment_carton.tenant_id=position.tenant_id
         AND shipment_carton.inventory_owner_id=position.inventory_owner_id
         AND shipment_carton.facility_id=position.facility_id
         AND shipment_carton.carton_id=position.carton_id
        WHERE position.tenant_id=$1 AND shipment_carton.id=ANY($2)
          AND position.state='packed'
        ORDER BY position.id
        FOR UPDATE OF position
        "#,
    )
    .bind(tenant_id.get())
    .bind(&shipment_carton_ids)
    .fetch_all(&mut **tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT shipment_carton.shipment_id, shipment_carton.id AS shipment_carton_id,
               shipment_carton.carton_id, shipment_carton.license_plate_id,
               shipment_carton.carton_barcode, shipment_carton.content_count,
               shipment_carton.packed_qty,
               MIN(position.current_location_id) AS original_location_id,
               COUNT(position.id)::BIGINT AS position_count,
               BOOL_AND(position.state='packed' AND position.revision=1
                        AND position.current_license_plate_id=shipment_carton.license_plate_id)
                   AS positions_valid
        FROM shipment_cartons shipment_carton
        JOIN packed_inventory_positions position
          ON position.tenant_id=shipment_carton.tenant_id
         AND position.inventory_owner_id=shipment_carton.inventory_owner_id
         AND position.facility_id=shipment_carton.facility_id
         AND position.carton_id=shipment_carton.carton_id
         AND position.state='packed'
        WHERE shipment_carton.tenant_id=$1 AND shipment_carton.shipment_id=ANY($2)
        GROUP BY shipment_carton.id
        ORDER BY shipment_carton.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            if !row.try_get::<bool, _>("positions_valid")?
                || row.try_get::<i64, _>("position_count")?
                    != row.try_get::<i64, _>("content_count")?
            {
                return Err(AppError::conflict("shipment carton position changed"));
            }
            Ok(CartonPlanRow {
                shipment_id: row.try_get("shipment_id")?,
                shipment_carton_id: row.try_get("shipment_carton_id")?,
                carton_id: row.try_get("carton_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                carton_barcode: row.try_get("carton_barcode")?,
                content_count: row.try_get("content_count")?,
                packed_qty: row.try_get("packed_qty")?,
                original_location_id: row.try_get("original_location_id")?,
            })
        })
        .collect()
}

fn validate_carton_sets(
    command: &PlanOutboundLoadCommand,
    rows: &[CartonPlanRow],
) -> AppResult<()> {
    for shipment in &command.shipments {
        let expected = shipment
            .cartons
            .iter()
            .map(|carton| carton.carton_id.get())
            .collect::<BTreeSet<_>>();
        let actual = rows
            .iter()
            .filter(|carton| carton.shipment_id == shipment.shipment_id.get())
            .map(|carton| carton.carton_id)
            .collect::<BTreeSet<_>>();
        if expected != actual {
            return Err(AppError::bad_request(
                "planned cartons must exactly match each shipment",
            ));
        }
    }
    Ok(())
}

async fn create_virtual_trailer_location_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    facility_id: FacilityId,
    barcode: &str,
    reference: &str,
    created_at: Timestamp,
) -> AppResult<i64> {
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM locations WHERE tenant_id=$1 AND barcode=$2)",
    )
    .bind(tenant_id.get())
    .bind(barcode)
    .fetch_one(&mut **tx)
    .await?;
    if existing {
        return Err(AppError::conflict(
            "outbound load barcode is already in use",
        ));
    }
    Ok(sqlx::query_scalar(
        r#"
        INSERT INTO locations (
            tenant_id, created, facility_id, barcode, name, type,
            active, pickable, receivable
        ) VALUES ($1,$2,$3,$4,$5,'outbound_trailer',TRUE,FALSE,FALSE)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(created_at)
    .bind(facility_id.get())
    .bind(barcode)
    .bind(format!("Outbound load {reference}"))
    .fetch_one(&mut **tx)
    .await?)
}
