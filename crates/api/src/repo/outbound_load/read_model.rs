use sqlx::Row;
use wareboxes_application::outbound_load::{
    OutboundLoadCartonReadModel, OutboundLoadCursor, OutboundLoadProgressReadModel,
    OutboundLoadQuery, OutboundLoadQueueEntryReadModel, OutboundLoadQueuePage,
    OutboundLoadQueueQuery, OutboundLoadReadModel, OutboundLoadShipmentReadModel,
    PackedCartonContentPositionReadModel, PackedCartonPositionQuery, PackedCartonPositionReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    ActualPickQuantity, CarrierCode, CartonContentId, CartonId, FacilityId, InventoryAllocationId,
    InventoryBalanceId, InventoryOwnerId, LicensePlateId, LocationId, OrderId, OrderRevision,
    OrderStatus, OutboundLoadCartonId, OutboundLoadId, OutboundLoadReference, OutboundLoadRevision,
    OutboundLoadScanValue, OutboundLoadShipmentId, OutboundLoadStatus, PackedCartonMovementId,
    PackedCartonPositionId, PackedCartonPositionRevision, PackedCartonPositionState, PickQuantity,
    ShipmentId, ShipmentRevision, ShipmentScanValue, ShipmentStatus, ShortShipDemandQuantities,
    TrailerNumber, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

use super::{parse_status, positive, require_load_visible_tx};

pub async fn get(
    db: &Db,
    access: &TenantAccess,
    query: OutboundLoadQuery,
) -> AppResult<OutboundLoadReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    require_load_visible_tx(&mut tx, access.tenant_id, query.outbound_load_id, &scope).await?;
    let result = load_read_model_tx(&mut tx, access.tenant_id, query.outbound_load_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn get_by_barcode(
    db: &Db,
    access: &TenantAccess,
    barcode: &OutboundLoadScanValue,
) -> AppResult<OutboundLoadReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let load_id = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id
        FROM outbound_loads
        WHERE tenant_id = $1 AND load_barcode = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(barcode.as_str())
    .fetch_optional(&mut *tx)
    .await?
    .map(|value| positive(value, OutboundLoadId::new))
    .transpose()?
    .ok_or_else(|| AppError::not_found("outbound load"))?;
    require_load_visible_tx(&mut tx, access.tenant_id, load_id, &scope).await?;
    let result = load_read_model_tx(&mut tx, access.tenant_id, load_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn packed_carton_position(
    db: &Db,
    access: &TenantAccess,
    query: PackedCartonPositionQuery,
) -> AppResult<PackedCartonPositionReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let position =
        position_for_carton_tx(&mut tx, access.tenant_id, query.carton_id, Some(&scope)).await?;
    tx.commit().await?;
    Ok(position)
}

pub async fn list(
    db: &Db,
    access: &TenantAccess,
    query: &OutboundLoadQueueQuery,
) -> AppResult<OutboundLoadQueuePage> {
    if query.limit == 0 || query.limit > 100 {
        return Err(AppError::bad_request("outbound load page limit is invalid"));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    if let Some(facility_id) = query.facility_id {
        if !scope.includes_facility(facility_id.get()) {
            return Err(AppError::not_found("facility"));
        }
    }
    let status = query.status.map(OutboundLoadStatus::as_str);
    let cursor_present = query.cursor.is_some();
    let cursor_time = query
        .cursor
        .as_ref()
        .and_then(|cursor| cursor.scheduled_departure_at);
    let cursor_id = query
        .cursor
        .as_ref()
        .map(|cursor| cursor.outbound_load_id.get());
    let fetch_limit = i64::from(query.limit) + 1;
    let rows = sqlx::query(
        r#"
        SELECT load.id, load.load_reference, load.carrier, load.facility_id,
               facility.name AS facility_name, load.state, load.revision,
               load.shipment_count, load.carton_count,
               COUNT(carton.id) FILTER (WHERE carton.state = 'staged')::BIGINT AS staged_count,
               COUNT(carton.id) FILTER (WHERE carton.state = 'loaded')::BIGINT AS loaded_count,
               staging.name AS staging_name, dock.name AS dock_name,
               load.trailer_number, load.scheduled_departure_at
        FROM outbound_loads load
        JOIN facilities facility
          ON facility.tenant_id = load.tenant_id AND facility.id = load.facility_id
         AND facility.deleted IS NULL
        JOIN locations staging
          ON staging.tenant_id = load.tenant_id AND staging.id = load.staging_lane_location_id
        LEFT JOIN locations dock
          ON dock.tenant_id = load.tenant_id AND dock.id = load.dock_door_location_id
        LEFT JOIN outbound_load_cartons carton
          ON carton.tenant_id = load.tenant_id AND carton.outbound_load_id = load.id
        WHERE load.tenant_id = $1
          AND ($2::BIGINT IS NULL OR load.facility_id = $2)
          AND ($3 OR load.facility_id = ANY($4))
          AND (($5::TEXT IS NULL AND load.state NOT IN ('departed', 'cancelled')) OR load.state = $5)
          AND ($6::TIMESTAMPTZ IS NULL OR load.scheduled_departure_at >= $6)
          AND ($7::TIMESTAMPTZ IS NULL OR load.scheduled_departure_at <= $7)
          AND NOT EXISTS (
              SELECT 1 FROM outbound_load_shipments link
              WHERE link.tenant_id = load.tenant_id AND link.outbound_load_id = load.id
                AND (
                    NOT ($8 OR link.inventory_owner_id = ANY($9))
                    OR NOT EXISTS (
                        SELECT 1 FROM inventory_owners owner
                        WHERE owner.tenant_id=link.tenant_id
                          AND owner.id=link.inventory_owner_id
                          AND owner.deleted IS NULL)
                    OR NOT EXISTS (
                        SELECT 1 FROM inventory_owner_facilities assignment
                        WHERE assignment.tenant_id=link.tenant_id
                          AND assignment.inventory_owner_id=link.inventory_owner_id
                          AND assignment.facility_id=link.facility_id
                          AND assignment.deleted IS NULL)
                )
          )
          AND (
              NOT $10
              OR ($11::TIMESTAMPTZ IS NOT NULL AND
                  (load.scheduled_departure_at > $11
                   OR load.scheduled_departure_at IS NULL
                   OR (load.scheduled_departure_at = $11 AND load.id > $12)))
              OR ($11::TIMESTAMPTZ IS NULL
                  AND load.scheduled_departure_at IS NULL AND load.id > $12)
          )
        GROUP BY load.id, facility.name, staging.name, dock.name
        ORDER BY load.scheduled_departure_at ASC NULLS LAST, load.id ASC
        LIMIT $13
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(query.facility_id.map(FacilityId::get))
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(status)
    .bind(query.scheduled_from)
    .bind(query.scheduled_to)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(cursor_present)
    .bind(cursor_time)
    .bind(cursor_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > query.limit as usize;
    let rows = rows
        .into_iter()
        .take(query.limit as usize)
        .collect::<Vec<_>>();
    let next_cursor = if has_more {
        rows.last()
            .map(|row| {
                Ok::<OutboundLoadCursor, AppError>(OutboundLoadCursor {
                    scheduled_departure_at: row.try_get("scheduled_departure_at")?,
                    outbound_load_id: positive(row.try_get("id")?, OutboundLoadId::new)?,
                })
            })
            .transpose()?
    } else {
        None
    };
    let entries = rows
        .into_iter()
        .map(queue_entry)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(OutboundLoadQueuePage {
        entries,
        next_cursor,
    })
}

fn queue_entry(row: sqlx::postgres::PgRow) -> AppResult<OutboundLoadQueueEntryReadModel> {
    let status = parse_status(&row.try_get::<String, _>("state")?)?;
    Ok(OutboundLoadQueueEntryReadModel {
        outbound_load_id: positive(row.try_get("id")?, OutboundLoadId::new)?,
        load_reference: OutboundLoadReference::new(row.try_get::<String, _>("load_reference")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        carrier_code: CarrierCode::new(row.try_get::<String, _>("carrier")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        facility_name: row.try_get("facility_name")?,
        status,
        revision: positive(row.try_get("revision")?, OutboundLoadRevision::new)?,
        progress: progress_from_row(&row)?,
        staging_location_name: row.try_get("staging_name")?,
        dock_location_name: row.try_get("dock_name")?,
        trailer_number: row
            .try_get::<Option<String>, _>("trailer_number")?
            .map(TrailerNumber::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        scheduled_departure_at: row.try_get("scheduled_departure_at")?,
    })
}

fn progress_from_row(row: &sqlx::postgres::PgRow) -> AppResult<OutboundLoadProgressReadModel> {
    Ok(OutboundLoadProgressReadModel {
        planned_shipment_count: u32::try_from(row.try_get::<i64, _>("shipment_count")?)
            .map_err(|_| AppError::internal("outbound load shipment count is invalid"))?,
        planned_carton_count: u32::try_from(row.try_get::<i64, _>("carton_count")?)
            .map_err(|_| AppError::internal("outbound load carton count is invalid"))?,
        staged_carton_count: u32::try_from(row.try_get::<i64, _>("staged_count")?)
            .map_err(|_| AppError::internal("outbound load staged count is invalid"))?,
        loaded_carton_count: u32::try_from(row.try_get::<i64, _>("loaded_count")?)
            .map_err(|_| AppError::internal("outbound load loaded count is invalid"))?,
    })
}

pub(super) async fn load_read_model_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    load_id: OutboundLoadId,
) -> AppResult<OutboundLoadReadModel> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM outbound_loads WHERE tenant_id=$1 AND id=$2 FOR SHARE",
    )
    .bind(tenant_id.get())
    .bind(load_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound load"))?;
    let row = sqlx::query(
        r#"
        SELECT load.*, staging.barcode AS staging_barcode,
               staging.name AS staging_name, dock.barcode AS dock_barcode,
               dock.name AS dock_name,
               COUNT(carton.id) FILTER (WHERE carton.state = 'staged')::BIGINT AS staged_count,
               COUNT(carton.id) FILTER (WHERE carton.state = 'loaded')::BIGINT AS loaded_count
        FROM outbound_loads load
        JOIN locations staging
          ON staging.tenant_id = load.tenant_id AND staging.id = load.staging_lane_location_id
        LEFT JOIN locations dock
          ON dock.tenant_id = load.tenant_id AND dock.id = load.dock_door_location_id
        LEFT JOIN outbound_load_cartons carton
          ON carton.tenant_id = load.tenant_id AND carton.outbound_load_id = load.id
        WHERE load.tenant_id = $1 AND load.id = $2
        GROUP BY load.id, staging.barcode, staging.name, dock.barcode, dock.name
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound load"))?;
    let status = parse_status(&row.try_get::<String, _>("state")?)?;
    let shipments = shipment_rows_tx(tx, tenant_id, load_id).await?;
    let cartons = carton_rows_tx(tx, tenant_id, load_id).await?;
    let read = OutboundLoadReadModel {
        outbound_load_id: load_id,
        load_reference: OutboundLoadReference::new(row.try_get::<String, _>("load_reference")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        load_barcode: OutboundLoadScanValue::new(row.try_get::<String, _>("load_barcode")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        carrier_code: CarrierCode::new(row.try_get::<String, _>("carrier")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        status,
        revision: positive(row.try_get("revision")?, OutboundLoadRevision::new)?,
        progress: progress_from_row(&row)?,
        staging_location_id: positive(row.try_get("staging_lane_location_id")?, LocationId::new)?,
        staging_location_barcode: row.try_get("staging_barcode")?,
        staging_location_name: row.try_get("staging_name")?,
        dock_location_id: row
            .try_get::<Option<i64>, _>("dock_door_location_id")?
            .map(|id| positive(id, LocationId::new))
            .transpose()?,
        dock_location_barcode: row.try_get("dock_barcode")?,
        dock_location_name: row.try_get("dock_name")?,
        virtual_trailer_location_id: positive(
            row.try_get("virtual_trailer_location_id")?,
            LocationId::new,
        )?,
        trailer_number: row
            .try_get::<Option<String>, _>("trailer_number")?
            .map(TrailerNumber::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        seal_number: row
            .try_get::<Option<String>, _>("seal_number")?
            .map(wareboxes_domain::SealNumber::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        scheduled_departure_at: row.try_get("scheduled_departure_at")?,
        shipments,
        cartons,
        planned_by: positive(row.try_get("planned_by_user_id")?, UserId::new)?,
        planned_at: row.try_get("planned_at")?,
        released_by: optional_positive(row.try_get("released_by_user_id")?, UserId::new)?,
        released_at: row.try_get("released_at")?,
        loading_started_by: optional_positive(
            row.try_get("loading_started_by_user_id")?,
            UserId::new,
        )?,
        loading_started_at: row.try_get("loading_started_at")?,
        ready_to_depart_by: optional_positive(
            row.try_get("ready_to_depart_by_user_id")?,
            UserId::new,
        )?,
        ready_to_depart_at: row.try_get("ready_to_depart_at")?,
        departed_by: optional_positive(row.try_get("departed_by_user_id")?, UserId::new)?,
        departed_at: row.try_get("departed_at")?,
        cancelled_by: optional_positive(row.try_get("cancelled_by_user_id")?, UserId::new)?,
        cancelled_at: row.try_get("cancelled_at")?,
    };
    if !read.is_consistent() {
        return Err(AppError::internal(
            "outbound load read model is inconsistent",
        ));
    }
    Ok(read)
}

async fn shipment_rows_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    load_id: OutboundLoadId,
) -> AppResult<Vec<OutboundLoadShipmentReadModel>> {
    let rows = sqlx::query(
        r#"
        SELECT link.id, link.inventory_owner_id, owner.name AS inventory_owner_name,
               link.shipment_id, link.order_id,
               link.shipment_sequence, shipment.state AS shipment_state,
               shipment.revision AS shipment_revision, orders.order_key,
               orders.status AS order_status, orders.revision AS order_revision,
               COALESCE(SUM(demand.original_qty), 0)::BIGINT AS ordered_qty,
               COALESCE(SUM(demand.accepted_short_qty), 0)::BIGINT AS accepted_short_qty,
               COALESCE(SUM(demand.accepted_substitute_qty), 0)::BIGINT
                   AS accepted_substitute_qty
        FROM outbound_load_shipments link
        JOIN shipments shipment
          ON shipment.tenant_id = link.tenant_id
         AND shipment.inventory_owner_id = link.inventory_owner_id
         AND shipment.id = link.shipment_id
        JOIN orders
          ON orders.tenant_id = link.tenant_id
         AND orders.inventory_owner_id = link.inventory_owner_id
         AND orders.id = link.order_id
        JOIN inventory_owners owner
          ON owner.tenant_id = link.tenant_id
         AND owner.id = link.inventory_owner_id
         AND owner.deleted IS NULL
        JOIN outbound_effective_demand demand
          ON demand.tenant_id = link.tenant_id
         AND demand.inventory_owner_id = link.inventory_owner_id
         AND demand.order_id = link.order_id
        WHERE link.tenant_id = $1 AND link.outbound_load_id = $2
        GROUP BY link.id, owner.name, shipment.state, shipment.revision, orders.order_key,
                 orders.status, orders.revision
        ORDER BY link.shipment_sequence, link.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let ordered = PickQuantity::new(row.try_get("ordered_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?;
            let accepted = ActualPickQuantity::new(row.try_get("accepted_short_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?;
            let accepted_substitute =
                ActualPickQuantity::new(row.try_get("accepted_substitute_qty")?)
                    .map_err(|error| AppError::internal(error.to_string()))?;
            Ok(OutboundLoadShipmentReadModel {
                outbound_load_shipment_id: positive(
                    row.try_get("id")?,
                    OutboundLoadShipmentId::new,
                )?,
                shipment_id: positive(row.try_get("shipment_id")?, ShipmentId::new)?,
                order_id: positive(row.try_get("order_id")?, OrderId::new)?,
                order_key: row.try_get("order_key")?,
                inventory_owner_id: positive(
                    row.try_get("inventory_owner_id")?,
                    InventoryOwnerId::new,
                )?,
                inventory_owner_name: row.try_get("inventory_owner_name")?,
                shipment_sequence: u32::try_from(row.try_get::<i64, _>("shipment_sequence")?)
                    .map_err(|_| AppError::internal("shipment sequence is invalid"))?,
                shipment_status: ShipmentStatus::parse(
                    &row.try_get::<String, _>("shipment_state")?,
                )
                .ok_or_else(|| AppError::internal("shipment status is invalid"))?,
                shipment_revision: positive(
                    row.try_get("shipment_revision")?,
                    ShipmentRevision::new,
                )?,
                order_status: OrderStatus::parse(&row.try_get::<String, _>("order_status")?)
                    .ok_or_else(|| AppError::internal("order status is invalid"))?,
                order_revision: positive(row.try_get("order_revision")?, OrderRevision::new)?,
                demand: ShortShipDemandQuantities::with_substitution(
                    ordered,
                    accepted,
                    accepted_substitute,
                )
                .map_err(|error| AppError::internal(error.to_string()))?,
            })
        })
        .collect()
}

async fn carton_rows_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    load_id: OutboundLoadId,
) -> AppResult<Vec<OutboundLoadCartonReadModel>> {
    let rows = sqlx::query(
        r#"
        SELECT carton.id, carton.shipment_id, carton.carton_id,
               carton.carton_barcode, carton.license_plate_id,
               carton.load_sequence, carton.original_location_id,
               load.staging_lane_location_id, carton.state, carton.revision,
               carton.content_count, carton.packed_qty,
               carton.last_move_confirmation_id
        FROM outbound_load_cartons carton
        JOIN outbound_loads load
          ON load.tenant_id = carton.tenant_id AND load.id = carton.outbound_load_id
        WHERE carton.tenant_id = $1 AND carton.outbound_load_id = $2
        ORDER BY carton.load_sequence, carton.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let state = carton_state(
                &row.try_get::<String, _>("state")?,
                Some(load_id),
                row.try_get("original_location_id")?,
                row.try_get("staging_lane_location_id")?,
                Some(row.try_get("load_sequence")?),
            )?;
            Ok(OutboundLoadCartonReadModel {
                outbound_load_carton_id: positive(row.try_get("id")?, OutboundLoadCartonId::new)?,
                shipment_id: positive(row.try_get("shipment_id")?, ShipmentId::new)?,
                carton_id: positive(row.try_get("carton_id")?, CartonId::new)?,
                carton_barcode: ShipmentScanValue::new(row.try_get::<String, _>("carton_barcode")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                license_plate_id: positive(row.try_get("license_plate_id")?, LicensePlateId::new)?,
                load_sequence: u32::try_from(row.try_get::<i64, _>("load_sequence")?)
                    .map_err(|_| AppError::internal("load sequence is invalid"))?,
                state,
                position_revision: positive(
                    row.try_get("revision")?,
                    PackedCartonPositionRevision::new,
                )?,
                content_count: row.try_get("content_count")?,
                packed_quantity: row.try_get("packed_qty")?,
                last_movement_id: row
                    .try_get::<Option<i64>, _>("last_move_confirmation_id")?
                    .map(|id| positive(id, PackedCartonMovementId::new))
                    .transpose()?,
            })
        })
        .collect()
}

pub(super) async fn position_for_carton_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    carton_id: CartonId,
    scope: Option<&ScopeBindings>,
) -> AppResult<PackedCartonPositionReadModel> {
    let rows = sqlx::query(
        r#"
        SELECT position.*, plate.barcode AS carton_barcode,
               content.destination_location_id AS original_location_id
        FROM packed_inventory_positions position
        JOIN carton_contents content
          ON content.tenant_id = position.tenant_id
         AND content.inventory_owner_id = position.inventory_owner_id
         AND content.id = position.carton_content_id
        JOIN cartons carton
          ON carton.tenant_id = position.tenant_id
         AND carton.inventory_owner_id = position.inventory_owner_id
         AND carton.id = position.carton_id
        JOIN license_plates plate
          ON plate.tenant_id = carton.tenant_id
         AND plate.inventory_owner_id = carton.inventory_owner_id
         AND plate.id = carton.license_plate_id
        WHERE position.tenant_id = $1 AND position.carton_id = $2
        ORDER BY position.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(carton_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let first = rows
        .first()
        .ok_or_else(|| AppError::not_found("packed carton"))?;
    let owner_id: i64 = first.try_get("inventory_owner_id")?;
    let facility_id: i64 = first.try_get("facility_id")?;
    if scope.is_some_and(|scope| {
        !scope.includes_inventory_owner(owner_id) || !scope.includes_facility(facility_id)
    }) {
        return Err(AppError::not_found("packed carton"));
    }
    let state_text: String = first.try_get("state")?;
    let revision: i64 = first.try_get("revision")?;
    let load_id: Option<i64> = first.try_get("outbound_load_id")?;
    let load_sequence: Option<i64> = first.try_get("load_sequence")?;
    let original_location_id: i64 = first.try_get("original_location_id")?;
    let current_location_id: Option<i64> = first.try_get("current_location_id")?;
    if rows.iter().any(|row| {
        row.try_get::<String, _>("state").ok().as_deref() != Some(state_text.as_str())
            || row.try_get::<i64, _>("revision").ok() != Some(revision)
            || row.try_get::<Option<i64>, _>("outbound_load_id").ok() != Some(load_id)
            || row.try_get::<Option<i64>, _>("load_sequence").ok() != Some(load_sequence)
    }) {
        return Err(AppError::internal(
            "packed carton content positions are inconsistent",
        ));
    }
    let state = carton_state(
        &state_text,
        load_id
            .map(|id| positive(id, OutboundLoadId::new))
            .transpose()?,
        original_location_id,
        current_location_id.unwrap_or(original_location_id),
        load_sequence,
    )?;
    let contents = rows
        .iter()
        .map(|row| {
            Ok(PackedCartonContentPositionReadModel {
                position_id: positive(row.try_get("id")?, PackedCartonPositionId::new)?,
                carton_content_id: positive(
                    row.try_get("carton_content_id")?,
                    CartonContentId::new,
                )?,
                current_inventory_allocation_id: optional_positive(
                    row.try_get("current_inventory_allocation_id")?,
                    InventoryAllocationId::new,
                )?,
                current_inventory_balance_id: optional_positive(
                    row.try_get("current_inventory_balance_id")?,
                    InventoryBalanceId::new,
                )?,
                current_location_id: optional_positive(
                    row.try_get("current_location_id")?,
                    LocationId::new,
                )?,
                current_license_plate_id: optional_positive(
                    row.try_get("current_license_plate_id")?,
                    LicensePlateId::new,
                )?,
                packed_quantity: row.try_get("packed_qty")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(PackedCartonPositionReadModel {
        carton_id,
        carton_barcode: ShipmentScanValue::new(first.try_get::<String, _>("carton_barcode")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: positive(owner_id, InventoryOwnerId::new)?,
        facility_id: positive(facility_id, FacilityId::new)?,
        state,
        revision: positive(revision, PackedCartonPositionRevision::new)?,
        contents,
        positioned_at: first.try_get("positioned_at")?,
        departed_at: first.try_get("departed_at")?,
    })
}

pub(super) fn carton_state(
    state: &str,
    load_id: Option<OutboundLoadId>,
    original_location_id: i64,
    current_location_id: i64,
    load_sequence: Option<i64>,
) -> AppResult<PackedCartonPositionState> {
    match state {
        "planned" | "packed" => Ok(PackedCartonPositionState::Packed {
            location_id: positive(original_location_id, LocationId::new)?,
        }),
        "staged" => Ok(PackedCartonPositionState::Staged {
            outbound_load_id: load_id
                .ok_or_else(|| AppError::internal("staged carton has no outbound load"))?,
            staging_location_id: positive(current_location_id, LocationId::new)?,
        }),
        "loaded" => Ok(PackedCartonPositionState::Loaded {
            outbound_load_id: load_id
                .ok_or_else(|| AppError::internal("loaded carton has no outbound load"))?,
            load_sequence: sequence(load_sequence)?,
        }),
        "departed" => Ok(PackedCartonPositionState::Departed {
            outbound_load_id: load_id,
            load_sequence: load_sequence.map(|_| sequence(load_sequence)).transpose()?,
        }),
        _ => Err(AppError::internal("packed carton state is invalid")),
    }
}

fn sequence(value: Option<i64>) -> AppResult<u32> {
    u32::try_from(value.ok_or_else(|| AppError::internal("load sequence is missing"))?)
        .map_err(|_| AppError::internal("load sequence is invalid"))
}

fn optional_positive<T, E>(
    value: Option<i64>,
    constructor: impl Fn(i64) -> Result<T, E>,
) -> AppResult<Option<T>>
where
    E: std::fmt::Display,
{
    value.map(|value| positive(value, constructor)).transpose()
}
