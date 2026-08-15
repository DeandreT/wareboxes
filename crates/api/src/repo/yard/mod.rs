//! Tenant- and owner-scoped yard execution.

mod commands;
mod models;
mod query;

pub use commands::{
    assign_door, cancel_appointment, complete_operation, configure_location, create_appointment,
    gate_in, gate_out, mark_no_show, register_asset, reject_visit, spot_visit, start_operation,
};
pub use query::workspace;

use serde::Serialize;
use wareboxes_application::yard::YardVisitEventKind;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, TenantId, Timestamp, UserId, YardAppointmentStatus,
    YardAssetKind, YardDirection, YardLocationKind, YardOperation, YardVisitStatus,
};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;

const PERMISSION: &str = "wms";

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

fn require_access_actor(
    access: &TenantAccess,
    context: &wareboxes_application::CommandContext,
) -> AppResult<()> {
    context.require_actor(access.tenant_id, access.user_id)?;
    Ok(())
}

fn require_scope(
    scope: &ScopeBindings,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id.get()) && scope.includes_facility(facility_id.get())
    {
        Ok(())
    } else {
        Err(AppError::not_found("yard record"))
    }
}

fn require_facility(scope: &ScopeBindings, facility_id: FacilityId) -> AppResult<()> {
    if scope.includes_facility(facility_id.get()) {
        Ok(())
    } else {
        Err(AppError::not_found("yard record"))
    }
}

const fn direction_name(value: YardDirection) -> &'static str {
    value.as_str()
}

fn parse_direction(value: &str) -> AppResult<YardDirection> {
    YardDirection::parse(value).ok_or_else(|| AppError::internal("invalid stored yard direction"))
}

const fn asset_kind_name(value: YardAssetKind) -> &'static str {
    value.as_str()
}

fn parse_asset_kind(value: &str) -> AppResult<YardAssetKind> {
    YardAssetKind::parse(value).ok_or_else(|| AppError::internal("invalid stored yard asset kind"))
}

const fn location_kind_name(value: YardLocationKind) -> &'static str {
    value.as_str()
}

fn parse_location_kind(value: &str) -> AppResult<YardLocationKind> {
    YardLocationKind::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored yard location kind"))
}

fn parse_appointment_status(value: &str) -> AppResult<YardAppointmentStatus> {
    YardAppointmentStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored yard appointment status"))
}

fn parse_visit_status(value: &str) -> AppResult<YardVisitStatus> {
    YardVisitStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored yard visit status"))
}

fn parse_operation(value: &str) -> AppResult<YardOperation> {
    YardOperation::parse(value).ok_or_else(|| AppError::internal("invalid stored yard operation"))
}

fn parse_event_kind(value: &str) -> AppResult<YardVisitEventKind> {
    YardVisitEventKind::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored yard event kind"))
}

struct YardOutboxEvent<'a> {
    tenant_id: TenantId,
    actor_id: UserId,
    owner_id: Option<InventoryOwnerId>,
    facility_id: Option<FacilityId>,
    aggregate_type: &'a str,
    aggregate_id: i64,
    transition: &'a str,
    occurred_at: Timestamp,
}

async fn enqueue_event_tx<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: YardOutboxEvent<'_>,
    payload: &T,
) -> AppResult<()> {
    let ordering_key = format!("yard-{}:{}", event.aggregate_type, event.aggregate_id);
    let event_key = format!("{ordering_key}:{}", event.transition);
    let event_type = format!("yard.{}.{}", event.aggregate_type, event.transition);
    let aggregate_id = event.aggregate_id.to_string();
    let sequence = next_outbox_sequence_tx(tx, event.tenant_id, &ordering_key).await?;
    let payload = serde_json::to_value(payload).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: event.owner_id,
            facility_id: event.facility_id,
            actor_user_id: Some(event.actor_id.get()),
            event_key: &event_key,
            aggregate_type: event.aggregate_type,
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}
