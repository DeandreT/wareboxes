mod setup;
mod visit;

pub use setup::{
    cancel_appointment, configure_location, create_appointment, mark_no_show, register_asset,
};
pub use visit::{
    assign_door, complete_operation, gate_in, gate_out, reject_visit, spot_visit, start_operation,
};

use wareboxes_application::yard::{YardVisitEventKind, YardVisitReadModel};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, TenantId, Timestamp, UserId, YardAppointmentId,
    YardAppointmentStatus, YardLocationId, YardLocationKind, YardOperation, YardRevision,
    YardVisitId, YardVisitStatus,
};

use super::models::{read_location_tx, read_visit_tx};
use super::{internal, location_kind_name, require_scope};
use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;

async fn lock_key_tx(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, key: String) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(key)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn validate_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let valid = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(
          SELECT 1 FROM inventory_owner_facilities link
          JOIN inventory_owners owner ON owner.tenant_id=link.tenant_id
            AND owner.id=link.inventory_owner_id AND owner.deleted IS NULL
          JOIN facilities facility ON facility.tenant_id=link.tenant_id
            AND facility.id=link.facility_id AND facility.deleted IS NULL
          WHERE link.tenant_id=$1 AND link.inventory_owner_id=$2
            AND link.facility_id=$3 AND link.deleted IS NULL)"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(AppError::not_found("owner-facility assignment"))
    }
}

async fn validate_location_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    location_id: YardLocationId,
    expected_kind: Option<YardLocationKind>,
) -> AppResult<wareboxes_application::yard::YardLocationReadModel> {
    let location = read_location_tx(tx, tenant_id, location_id).await?;
    if location.facility_id != facility_id
        || !location.active
        || expected_kind.is_some_and(|kind| location.kind != kind)
    {
        return Err(AppError::not_found("yard location"));
    }
    Ok(location)
}

async fn lock_visit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    visit_id: YardVisitId,
) -> AppResult<YardVisitReadModel> {
    lock_key_tx(
        tx,
        format!("yard-visit:{}:{}", tenant_id.get(), visit_id.get()),
    )
    .await?;
    sqlx::query("SELECT id FROM yard_visits WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(tenant_id.get())
        .bind(visit_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("yard visit"))?;
    read_visit_tx(tx, tenant_id, visit_id).await
}

struct NewAppointmentEvent<'a> {
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    appointment_id: YardAppointmentId,
    kind: &'a str,
    from_status: Option<YardAppointmentStatus>,
    to_status: YardAppointmentStatus,
    note: Option<&'a str>,
    revision: YardRevision,
    actor_id: UserId,
    occurred_at: Timestamp,
}

async fn insert_appointment_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: NewAppointmentEvent<'_>,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO yard_appointment_events
           (tenant_id,inventory_owner_id,facility_id,appointment_id,event_kind,from_status,
            to_status,note,resulting_revision,actor_user_id,occurred_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,
    )
    .bind(event.tenant_id.get())
    .bind(event.owner_id.get())
    .bind(event.facility_id.get())
    .bind(event.appointment_id.get())
    .bind(event.kind)
    .bind(event.from_status.map(YardAppointmentStatus::as_str))
    .bind(event.to_status.as_str())
    .bind(event.note)
    .bind(event.revision.get())
    .bind(event.actor_id.get())
    .bind(event.occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

struct NewVisitEvent<'a> {
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    visit_id: YardVisitId,
    kind: YardVisitEventKind,
    from_status: Option<YardVisitStatus>,
    to_status: YardVisitStatus,
    from_location_id: Option<YardLocationId>,
    to_location_id: Option<YardLocationId>,
    operation: Option<YardOperation>,
    note: Option<&'a str>,
    revision: YardRevision,
    actor_id: UserId,
    occurred_at: Timestamp,
}

async fn insert_visit_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: NewVisitEvent<'_>,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO yard_visit_events
           (tenant_id,inventory_owner_id,facility_id,visit_id,event_kind,from_status,to_status,
            from_location_id,to_location_id,operation,note,resulting_revision,actor_user_id,occurred_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)"#,
    )
    .bind(event.tenant_id.get())
    .bind(event.owner_id.get())
    .bind(event.facility_id.get())
    .bind(event.visit_id.get())
    .bind(event.kind.as_str())
    .bind(event.from_status.map(YardVisitStatus::as_str))
    .bind(event.to_status.as_str())
    .bind(event.from_location_id.map(YardLocationId::get))
    .bind(event.to_location_id.map(YardLocationId::get))
    .bind(event.operation.map(YardOperation::as_str))
    .bind(event.note)
    .bind(event.revision.get())
    .bind(event.actor_id.get())
    .bind(event.occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn verify_visit_replay(scope: &ScopeBindings, visit: &YardVisitReadModel) -> AppResult<()> {
    require_scope(scope, visit.inventory_owner_id, visit.facility_id)
}

fn conflict(error: impl std::fmt::Display) -> AppError {
    AppError::conflict(error.to_string())
}

fn validate_non_door_destination(kind: YardLocationKind) -> AppResult<()> {
    if matches!(
        kind,
        YardLocationKind::Parking | YardLocationKind::Inspection | YardLocationKind::Staging
    ) {
        Ok(())
    } else {
        Err(AppError::bad_request(format!(
            "{} is not a spot-move destination",
            location_kind_name(kind)
        )))
    }
}

fn next_revision(current: YardRevision) -> AppResult<YardRevision> {
    current.next().map_err(internal)
}
