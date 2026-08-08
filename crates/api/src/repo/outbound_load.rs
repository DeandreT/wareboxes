//! Multi-owner outbound-load planning and physical execution.

mod departure;
mod movement;
mod phase;
mod planning;
mod read_model;

pub use departure::confirm_departure;
pub use movement::{load_carton, stage_carton, unload_carton, unstage_carton};
pub use phase::{cancel, complete_loading, release, start_loading};
pub use planning::plan;
pub use read_model::{get, get_by_barcode, list, packed_carton_position};

use sqlx::Row;
use wareboxes_application::outbound_load::OutboundLoadProgressReadModel;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{
    FacilityId, OutboundLoadId, OutboundLoadProgress, OutboundLoadRevision, OutboundLoadStatus,
    TenantId, Timestamp,
};
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;

#[derive(Debug, Clone)]
struct LockedLoad {
    id: OutboundLoadId,
    facility_id: FacilityId,
    state: OutboundLoadStatus,
    revision: OutboundLoadRevision,
    staging_location_id: i64,
    dock_location_id: Option<i64>,
    virtual_trailer_location_id: i64,
    load_barcode: String,
    trailer_number: Option<String>,
    seal_number: Option<String>,
    shipment_count: i64,
    carton_count: i64,
}

async fn require_load_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load_id: OutboundLoadId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM outbound_loads load
            WHERE load.tenant_id = $1 AND load.id = $2
              AND ($3 OR load.facility_id = ANY($4))
              AND NOT EXISTS (
                  SELECT 1
                  FROM outbound_load_shipments link
                  WHERE link.tenant_id = load.tenant_id
                    AND link.outbound_load_id = load.id
                    AND (
                        NOT ($5 OR link.inventory_owner_id = ANY($6))
                        OR NOT EXISTS (
                            SELECT 1 FROM inventory_owners owner
                            WHERE owner.tenant_id = link.tenant_id
                              AND owner.id = link.inventory_owner_id
                              AND owner.deleted IS NULL
                        )
                        OR NOT EXISTS (
                            SELECT 1 FROM inventory_owner_facilities assignment
                            WHERE assignment.tenant_id = link.tenant_id
                              AND assignment.inventory_owner_id = link.inventory_owner_id
                              AND assignment.facility_id = link.facility_id
                              AND assignment.deleted IS NULL
                        )
                    )
              )
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("outbound load"))
    }
}

async fn lock_load_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load_id: OutboundLoadId,
    scope: &ScopeBindings,
) -> AppResult<LockedLoad> {
    require_load_visible_tx(tx, tenant_id, load_id, scope).await?;
    let row = sqlx::query(
        r#"
        SELECT id, facility_id, state, revision, staging_lane_location_id,
               dock_door_location_id, virtual_trailer_location_id, load_barcode,
               trailer_number, seal_number, shipment_count, carton_count
        FROM outbound_loads
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound load"))?;
    Ok(LockedLoad {
        id: positive(row.try_get("id")?, OutboundLoadId::new)?,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        state: parse_status(&row.try_get::<String, _>("state")?)?,
        revision: positive(row.try_get("revision")?, OutboundLoadRevision::new)?,
        staging_location_id: row.try_get("staging_lane_location_id")?,
        dock_location_id: row.try_get("dock_door_location_id")?,
        virtual_trailer_location_id: row.try_get("virtual_trailer_location_id")?,
        load_barcode: row.try_get("load_barcode")?,
        trailer_number: row.try_get("trailer_number")?,
        seal_number: row.try_get("seal_number")?,
        shipment_count: row.try_get("shipment_count")?,
        carton_count: row.try_get("carton_count")?,
    })
}

async fn load_progress_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load: &LockedLoad,
) -> AppResult<OutboundLoadProgress> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) FILTER (WHERE state = 'staged')::BIGINT AS staged,
               COUNT(*) FILTER (WHERE state = 'loaded')::BIGINT AS loaded
        FROM outbound_load_cartons
        WHERE tenant_id = $1 AND outbound_load_id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(load.id.get())
    .fetch_one(&mut **tx)
    .await?;
    let shipment_count = u32::try_from(load.shipment_count)
        .map_err(|_| AppError::internal("outbound load shipment count is invalid"))?;
    let carton_count = u32::try_from(load.carton_count)
        .map_err(|_| AppError::internal("outbound load carton count is invalid"))?;
    let staged = u32::try_from(row.try_get::<i64, _>("staged")?)
        .map_err(|_| AppError::internal("outbound load staged count is invalid"))?;
    let loaded = u32::try_from(row.try_get::<i64, _>("loaded")?)
        .map_err(|_| AppError::internal("outbound load loaded count is invalid"))?;
    OutboundLoadProgress::restore(shipment_count, carton_count, staged, loaded, load.state)
        .map_err(|error| AppError::internal(error.to_string()))
}

fn progress_read(progress: OutboundLoadProgress) -> OutboundLoadProgressReadModel {
    progress.into()
}

struct LoadEvent<'a> {
    tenant_id: TenantId,
    facility_id: FacilityId,
    actor_user_id: i64,
    load_id: OutboundLoadId,
    event_type: &'a str,
    event_key: &'a str,
    payload: serde_json::Value,
    occurred_at: Timestamp,
}

async fn enqueue_load_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: LoadEvent<'_>,
) -> AppResult<()> {
    let aggregate_id = event.load_id.get().to_string();
    let ordering_key = format!("outbound-load:{}", event.load_id.get());
    let aggregate_sequence = next_outbox_sequence_tx(tx, event.tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: None,
            facility_id: Some(event.facility_id),
            actor_user_id: Some(event.actor_user_id),
            event_key: event.event_key,
            aggregate_type: "outbound_load",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: event.event_type,
            schema_version: 1,
            payload: &event.payload,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}

fn parse_status(value: &str) -> AppResult<OutboundLoadStatus> {
    OutboundLoadStatus::parse(value)
        .ok_or_else(|| AppError::internal("outbound load has an invalid status"))
}

fn positive<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>) -> AppResult<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(|error| AppError::internal(error.to_string()))
}
