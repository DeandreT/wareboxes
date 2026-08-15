use sqlx::Row;
use wareboxes_application::yard::{
    YardAppointmentReadModel, YardAssetReadModel, YardDetentionReadModel, YardLocationReadModel,
    YardVisitEventReadModel, YardVisitReadModel,
};
use wareboxes_domain::{
    BillableEventId, FacilityId, InboundLoadId, InventoryOwnerId, OutboundLoadId, TenantId, UserId,
    YardAppointmentId, YardAppointmentWindow, YardAssetId, YardDetentionId, YardFreeMinutes,
    YardLocationId, YardRevision, YardVisitEventId, YardVisitId,
};

use super::{
    internal, parse_appointment_status, parse_asset_kind, parse_direction, parse_event_kind,
    parse_location_kind, parse_operation, parse_visit_status,
};
use crate::error::{AppError, AppResult};

fn optional_id<T>(
    value: Option<i64>,
    constructor: impl Fn(i64) -> Result<T, wareboxes_domain::InvalidId>,
) -> AppResult<Option<T>> {
    value.map(constructor).transpose().map_err(internal)
}

fn u64_value(value: i64, name: &str) -> AppResult<u64> {
    u64::try_from(value).map_err(|_| AppError::internal(format!("invalid stored {name}")))
}

pub(super) fn location(row: &sqlx::postgres::PgRow) -> AppResult<YardLocationReadModel> {
    Ok(YardLocationReadModel {
        location_id: YardLocationId::new(row.try_get("id")?).map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        facility_name: row.try_get("facility_name")?,
        code: row.try_get("code")?,
        name: row.try_get("name")?,
        kind: parse_location_kind(&row.try_get::<String, _>("kind")?)?,
        active: row.try_get("active")?,
        revision: YardRevision::new(row.try_get("revision")?).map_err(internal)?,
    })
}

pub(super) fn asset(row: &sqlx::postgres::PgRow) -> AppResult<YardAssetReadModel> {
    Ok(YardAssetReadModel {
        asset_id: YardAssetId::new(row.try_get("id")?).map_err(internal)?,
        kind: parse_asset_kind(&row.try_get::<String, _>("kind")?)?,
        asset_number: row.try_get("asset_number")?,
        carrier: row.try_get("carrier")?,
        active: row.try_get("active")?,
        revision: YardRevision::new(row.try_get("revision")?).map_err(internal)?,
    })
}

pub(super) fn appointment(row: &sqlx::postgres::PgRow) -> AppResult<YardAppointmentReadModel> {
    Ok(YardAppointmentReadModel {
        appointment_id: YardAppointmentId::new(row.try_get("id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        facility_name: row.try_get("facility_name")?,
        direction: parse_direction(&row.try_get::<String, _>("direction")?)?,
        appointment_number: row.try_get("appointment_number")?,
        window: YardAppointmentWindow::new(
            row.try_get("scheduled_from")?,
            row.try_get("scheduled_until")?,
        )
        .map_err(internal)?,
        carrier: row.try_get("carrier")?,
        expected_asset_kind: parse_asset_kind(&row.try_get::<String, _>("expected_asset_kind")?)?,
        expected_asset_number: row.try_get("expected_asset_number")?,
        inbound_load_id: optional_id(row.try_get("inbound_load_id")?, InboundLoadId::new)?,
        outbound_load_id: optional_id(row.try_get("outbound_load_id")?, OutboundLoadId::new)?,
        free_minutes: YardFreeMinutes::new(
            u32::try_from(row.try_get::<i32, _>("free_minutes")?)
                .map_err(|_| AppError::internal("invalid stored yard free minutes"))?,
        )
        .map_err(internal)?,
        status: parse_appointment_status(&row.try_get::<String, _>("status")?)?,
        revision: YardRevision::new(row.try_get("revision")?).map_err(internal)?,
        note: row.try_get("note")?,
        visit_id: optional_id(row.try_get("visit_id")?, YardVisitId::new)?,
        created_by: UserId::new(row.try_get("created_by_user_id")?).map_err(internal)?,
        created_at: row.try_get("created_at")?,
        updated_by: optional_id(row.try_get("updated_by_user_id")?, UserId::new)?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn visit_event(row: &sqlx::postgres::PgRow) -> AppResult<YardVisitEventReadModel> {
    Ok(YardVisitEventReadModel {
        event_id: YardVisitEventId::new(row.try_get("id")?).map_err(internal)?,
        kind: parse_event_kind(&row.try_get::<String, _>("event_kind")?)?,
        from_status: row
            .try_get::<Option<String>, _>("from_status")?
            .map(|value| parse_visit_status(&value))
            .transpose()?,
        to_status: parse_visit_status(&row.try_get::<String, _>("to_status")?)?,
        from_location_id: optional_id(row.try_get("from_location_id")?, YardLocationId::new)?,
        to_location_id: optional_id(row.try_get("to_location_id")?, YardLocationId::new)?,
        operation: row
            .try_get::<Option<String>, _>("operation")?
            .map(|value| parse_operation(&value))
            .transpose()?,
        note: row.try_get("note")?,
        resulting_revision: YardRevision::new(row.try_get("resulting_revision")?)
            .map_err(internal)?,
        actor_id: UserId::new(row.try_get("actor_user_id")?).map_err(internal)?,
        occurred_at: row.try_get("occurred_at")?,
    })
}

fn detention(row: &sqlx::postgres::PgRow) -> AppResult<YardDetentionReadModel> {
    Ok(YardDetentionReadModel {
        detention_id: YardDetentionId::new(row.try_get("id")?).map_err(internal)?,
        total_minutes: u64_value(row.try_get("total_minutes")?, "yard total minutes")?,
        free_minutes: u32::try_from(row.try_get::<i32, _>("free_minutes")?)
            .map_err(|_| AppError::internal("invalid stored yard free minutes"))?,
        detention_minutes: u64_value(row.try_get("detention_minutes")?, "yard detention minutes")?,
        billable_hours: u64_value(row.try_get("billable_hours")?, "yard billable hours")?,
        billable_event_id: optional_id(row.try_get("billable_event_id")?, BillableEventId::new)?,
        calculated_at: row.try_get("calculated_at")?,
    })
}

pub(super) async fn read_location_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    location_id: YardLocationId,
) -> AppResult<YardLocationReadModel> {
    let row = sqlx::query(
        r#"SELECT location.*,facility.name AS facility_name
           FROM yard_locations location JOIN facilities facility
             ON facility.tenant_id=location.tenant_id AND facility.id=location.facility_id
           WHERE location.tenant_id=$1 AND location.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(location_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("yard location"))?;
    location(&row)
}

pub(super) async fn read_asset_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    asset_id: YardAssetId,
) -> AppResult<YardAssetReadModel> {
    let row = sqlx::query("SELECT * FROM yard_assets WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(asset_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("yard asset"))?;
    asset(&row)
}

pub(super) async fn read_appointment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    appointment_id: YardAppointmentId,
) -> AppResult<YardAppointmentReadModel> {
    let row = sqlx::query(
        r#"SELECT appointment.*,owner.name AS inventory_owner_name,
                  facility.name AS facility_name
           FROM yard_appointments appointment
           JOIN inventory_owners owner ON owner.tenant_id=appointment.tenant_id
             AND owner.id=appointment.inventory_owner_id
           JOIN facilities facility ON facility.tenant_id=appointment.tenant_id
             AND facility.id=appointment.facility_id
           WHERE appointment.tenant_id=$1 AND appointment.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(appointment_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("yard appointment"))?;
    appointment(&row)
}

pub(super) async fn read_visit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    visit_id: YardVisitId,
) -> AppResult<YardVisitReadModel> {
    let row = sqlx::query(
        r#"SELECT visit.*,owner.name AS inventory_owner_name,facility.name AS facility_name,
                  appointment.appointment_number,asset.kind AS asset_kind,
                  asset.asset_number,asset.carrier,
                  current_location.code AS current_location_code,
                  dock_door.code AS dock_door_code
           FROM yard_visits visit
           JOIN inventory_owners owner ON owner.tenant_id=visit.tenant_id
             AND owner.id=visit.inventory_owner_id
           JOIN facilities facility ON facility.tenant_id=visit.tenant_id
             AND facility.id=visit.facility_id
           JOIN yard_assets asset ON asset.tenant_id=visit.tenant_id AND asset.id=visit.asset_id
           LEFT JOIN yard_appointments appointment ON appointment.tenant_id=visit.tenant_id
             AND appointment.id=visit.appointment_id
           LEFT JOIN yard_locations current_location ON current_location.tenant_id=visit.tenant_id
             AND current_location.facility_id=visit.facility_id
             AND current_location.id=visit.current_location_id
           LEFT JOIN yard_locations dock_door ON dock_door.tenant_id=visit.tenant_id
             AND dock_door.facility_id=visit.facility_id
             AND dock_door.id=visit.dock_door_location_id
           WHERE visit.tenant_id=$1 AND visit.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(visit_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("yard visit"))?;
    let event_rows = sqlx::query(
        "SELECT * FROM yard_visit_events WHERE tenant_id=$1 AND visit_id=$2 ORDER BY resulting_revision",
    )
    .bind(tenant_id.get())
    .bind(visit_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let detention_row =
        sqlx::query("SELECT * FROM yard_detention_records WHERE tenant_id=$1 AND visit_id=$2")
            .bind(tenant_id.get())
            .bind(visit_id.get())
            .fetch_optional(&mut **tx)
            .await?;
    Ok(YardVisitReadModel {
        visit_id,
        appointment_id: optional_id(row.try_get("appointment_id")?, YardAppointmentId::new)?,
        appointment_number: row.try_get("appointment_number")?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        facility_name: row.try_get("facility_name")?,
        direction: parse_direction(&row.try_get::<String, _>("direction")?)?,
        asset_id: YardAssetId::new(row.try_get("asset_id")?).map_err(internal)?,
        asset_kind: parse_asset_kind(&row.try_get::<String, _>("asset_kind")?)?,
        asset_number: row.try_get("asset_number")?,
        carrier: row.try_get("carrier")?,
        driver_name: row.try_get("driver_name")?,
        status: parse_visit_status(&row.try_get::<String, _>("status")?)?,
        revision: YardRevision::new(row.try_get("revision")?).map_err(internal)?,
        current_location_id: optional_id(row.try_get("current_location_id")?, YardLocationId::new)?,
        current_location_code: row.try_get("current_location_code")?,
        dock_door_location_id: optional_id(
            row.try_get("dock_door_location_id")?,
            YardLocationId::new,
        )?,
        dock_door_code: row.try_get("dock_door_code")?,
        inbound_load_id: optional_id(row.try_get("inbound_load_id")?, InboundLoadId::new)?,
        outbound_load_id: optional_id(row.try_get("outbound_load_id")?, OutboundLoadId::new)?,
        gated_in_at: row.try_get("gated_in_at")?,
        operation_started_at: row.try_get("operation_started_at")?,
        operation_completed_at: row.try_get("operation_completed_at")?,
        gated_out_at: row.try_get("gated_out_at")?,
        rejected_at: row.try_get("rejected_at")?,
        detention: detention_row.as_ref().map(detention).transpose()?,
        events: event_rows
            .iter()
            .map(visit_event)
            .collect::<AppResult<Vec<_>>>()?,
    })
}
