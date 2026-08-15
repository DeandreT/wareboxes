//! Tenant-scoped labor execution, standards, certifications, and equipment.

mod candidates;
mod commands;
mod corrections;
mod models;
mod query;

pub use candidates::{
    reference_candidates, roster_candidates, LaborReferenceCandidateFilter, LaborRosterFilter,
    MAX_LABOR_CANDIDATE_PAGE_SIZE,
};
pub use commands::{
    cancel_activity, certify_employee, change_equipment_status, clock_in, clock_out,
    complete_activity, configure_equipment_class, configure_skill, configure_standard,
    create_equipment_asset, revoke_certification, start_activity,
};
pub use corrections::{correct_activity, correct_attendance};
pub use query::{workspace, LaborWorkspaceFilter};

use serde::Serialize;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    AttendanceStatus, EquipmentStatus, FacilityId, InventoryOwnerId, LaborActivityKind,
    LaborActivityStatus, LaborCorrectionReason, LaborExceptionReason, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;

pub(crate) const LABOR_VIEW_PERMISSION: &str = "labor_view";
const CONFIGURE_PERMISSION: &str = "labor_configure";
const CERTIFY_PERMISSION: &str = "labor_certify";
const EQUIPMENT_PERMISSION: &str = "labor_equipment";
const EXECUTE_PERMISSION: &str = "labor_execute";
const SUPERVISE_PERMISSION: &str = "labor_supervise";

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

fn require_facility(scope: &ScopeBindings, facility_id: FacilityId) -> AppResult<()> {
    if scope.includes_facility(facility_id.get()) {
        Ok(())
    } else {
        Err(AppError::not_found("labor record"))
    }
}

fn require_owner(scope: &ScopeBindings, owner_id: Option<InventoryOwnerId>) -> AppResult<()> {
    if owner_id.is_none_or(|owner_id| scope.includes_inventory_owner(owner_id.get())) {
        Ok(())
    } else {
        Err(AppError::not_found("labor record"))
    }
}

fn require_scope(
    scope: &ScopeBindings,
    facility_id: FacilityId,
    owner_id: Option<InventoryOwnerId>,
) -> AppResult<()> {
    require_facility(scope, facility_id)?;
    require_owner(scope, owner_id)
}

fn require_tenant_global_scope(scope: &ScopeBindings) -> AppResult<()> {
    if scope.all_facilities && scope.all_inventory_owners {
        Ok(())
    } else {
        Err(AppError::forbidden())
    }
}

fn parse_attendance_status(value: &str) -> AppResult<AttendanceStatus> {
    AttendanceStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored attendance status"))
}

fn parse_activity_kind(value: &str) -> AppResult<LaborActivityKind> {
    LaborActivityKind::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored labor activity kind"))
}

fn parse_activity_status(value: &str) -> AppResult<LaborActivityStatus> {
    LaborActivityStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored labor activity status"))
}

fn parse_equipment_status(value: &str) -> AppResult<EquipmentStatus> {
    EquipmentStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored equipment status"))
}

fn parse_exception_reason(value: &str) -> AppResult<LaborExceptionReason> {
    LaborExceptionReason::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored labor exception reason"))
}

fn parse_correction_reason(value: &str) -> AppResult<LaborCorrectionReason> {
    LaborCorrectionReason::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored labor correction reason"))
}

async fn lock_key_tx(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, key: &str) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

struct LaborOutboxEvent<'a> {
    tenant_id: TenantId,
    actor_id: UserId,
    facility_id: Option<FacilityId>,
    owner_id: Option<InventoryOwnerId>,
    aggregate_type: &'a str,
    aggregate_id: i64,
    transition: &'a str,
    occurred_at: Timestamp,
}

async fn enqueue_event_tx<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: LaborOutboxEvent<'_>,
    payload: &T,
) -> AppResult<()> {
    let ordering_key = format!("labor-{}:{}", event.aggregate_type, event.aggregate_id);
    let event_type = format!("labor.{}.{}", event.aggregate_type, event.transition);
    let aggregate_id = event.aggregate_id.to_string();
    let sequence = next_outbox_sequence_tx(tx, event.tenant_id, &ordering_key).await?;
    let event_key = format!("{ordering_key}:{}:{sequence}", event.transition);
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
