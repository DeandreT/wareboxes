use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    AssignYardVisitDoorRequest, ConfigureYardLocationRequest, CreateYardAppointmentRequest,
    GateInYardVisitRequest, MoveYardVisitRequest, OpaqueCursor, RegisterYardAssetRequest, Revision,
    YardAppointmentResponse, YardAppointmentStatus as ApiAppointmentStatus,
    YardAssetKind as ApiAssetKind, YardAssetResponse, YardDetentionResponse,
    YardDirection as ApiDirection, YardDockOperationRequest, YardLifecycleRequest,
    YardLocationKind as ApiLocationKind, YardLocationResponse, YardOperation as ApiOperation,
    YardVisitEventKind as ApiEventKind, YardVisitEventResponse, YardVisitResponse,
    YardVisitStatus as ApiVisitStatus, YardWorkspaceRequest, YardWorkspaceResponse,
};
use wareboxes_application::yard::{
    AssignYardVisitDoorCommand, ConfigureYardLocationCommand, CreateYardAppointmentCommand,
    GateInYardVisitCommand, MoveYardVisitCommand, RegisterYardAssetCommand,
    YardAppointmentLifecycleCommand, YardAppointmentReadModel, YardAssetReadModel,
    YardDetentionReadModel, YardDockOperationCommand, YardLocationReadModel, YardVisitEventKind,
    YardVisitEventReadModel, YardVisitLifecycleCommand, YardVisitReadModel, YardWorkspaceFilter,
};
use wareboxes_domain::{
    FacilityId, InboundLoadId, InventoryOwnerId, OutboundLoadId, Timestamp, YardAppointmentId,
    YardAppointmentNumber, YardAppointmentStatus, YardAppointmentWindow, YardAssetId,
    YardAssetKind, YardAssetNumber, YardDirection, YardFreeMinutes, YardLocationCode,
    YardLocationId, YardLocationKind, YardName, YardNote, YardOperation, YardRevision, YardVisitId,
    YardVisitStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "yard1.";

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<YardWorkspaceRequest>,
) -> V1Result<Json<YardWorkspaceResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let facility_id = request
        .facility_id
        .map(FacilityId::new)
        .transpose()
        .map_err(validation)?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(validation)?;
    let before_visit_id = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let result = repo::yard::workspace(
        &state.db,
        &user.tenant,
        &YardWorkspaceFilter {
            facility_id,
            inventory_owner_id,
            include_completed: request.include_completed,
            before_visit_id,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = result
        .next_visit_id
        .map(|visit_id| encode_cursor(visit_id, &request))
        .transpose()?;
    Ok(Json(YardWorkspaceResponse {
        locations: result
            .locations
            .into_iter()
            .map(map_location)
            .collect::<V1Result<Vec<_>>>()?,
        assets: result
            .assets
            .into_iter()
            .map(map_asset)
            .collect::<V1Result<Vec<_>>>()?,
        appointments: result
            .appointments
            .into_iter()
            .map(map_appointment)
            .collect::<V1Result<Vec<_>>>()?,
        visits: result
            .visits
            .into_iter()
            .map(map_visit)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    }))
}

pub async fn configure_location(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureYardLocationRequest>,
) -> V1Result<Json<YardLocationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ConfigureYardLocationCommand {
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        code: YardLocationCode::new(body.code).map_err(validation)?,
        name: YardName::new(body.name).map_err(validation)?,
        kind: location_kind_from_api(body.kind),
    };
    let result = repo::yard::configure_location(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_location(result)?))
}

pub async fn register_asset(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RegisterYardAssetRequest>,
) -> V1Result<Json<YardAssetResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = RegisterYardAssetCommand {
        kind: asset_kind_from_api(body.kind),
        asset_number: YardAssetNumber::new(body.asset_number).map_err(validation)?,
        carrier: YardName::new(body.carrier).map_err(validation)?,
    };
    let result = repo::yard::register_asset(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_asset(result)?))
}

pub async fn create_appointment(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateYardAppointmentRequest>,
) -> V1Result<Json<YardAppointmentResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = CreateYardAppointmentCommand {
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        direction: direction_from_api(body.direction),
        appointment_number: YardAppointmentNumber::new(body.appointment_number)
            .map_err(validation)?,
        window: YardAppointmentWindow::new(
            parse_timestamp(&body.scheduled_from, "scheduled_from")?,
            parse_timestamp(&body.scheduled_until, "scheduled_until")?,
        )
        .map_err(validation)?,
        carrier: YardName::new(body.carrier).map_err(validation)?,
        expected_asset_kind: asset_kind_from_api(body.expected_asset_kind),
        expected_asset_number: body
            .expected_asset_number
            .map(YardAssetNumber::new)
            .transpose()
            .map_err(validation)?,
        inbound_load_id: body
            .inbound_load_id
            .map(InboundLoadId::new)
            .transpose()
            .map_err(validation)?,
        outbound_load_id: body
            .outbound_load_id
            .map(OutboundLoadId::new)
            .transpose()
            .map_err(validation)?,
        free_minutes: YardFreeMinutes::new(body.free_minutes).map_err(validation)?,
        note: body
            .note
            .map(YardNote::new)
            .transpose()
            .map_err(validation)?,
    };
    let result = repo::yard::create_appointment(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_appointment(result)?))
}

pub async fn cancel_appointment(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(appointment_id): Path<i64>,
    Json(body): Json<YardLifecycleRequest>,
) -> V1Result<Json<YardAppointmentResponse>> {
    appointment_lifecycle(state, user, idempotency_key, appointment_id, body, false).await
}

pub async fn mark_no_show(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(appointment_id): Path<i64>,
    Json(body): Json<YardLifecycleRequest>,
) -> V1Result<Json<YardAppointmentResponse>> {
    appointment_lifecycle(state, user, idempotency_key, appointment_id, body, true).await
}

async fn appointment_lifecycle(
    state: AppState,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    appointment_id: i64,
    body: YardLifecycleRequest,
    no_show: bool,
) -> V1Result<Json<YardAppointmentResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = YardAppointmentLifecycleCommand {
        appointment_id: YardAppointmentId::new(appointment_id).map_err(validation)?,
        expected_revision: YardRevision::new(body.expected_revision.get()).map_err(validation)?,
        note: YardNote::new(body.note).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = if no_show {
        repo::yard::mark_no_show(&state.db, &user.tenant, &context, &command).await?
    } else {
        repo::yard::cancel_appointment(&state.db, &user.tenant, &context, &command).await?
    };
    Ok(Json(map_appointment(result)?))
}

pub async fn gate_in(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<GateInYardVisitRequest>,
) -> V1Result<Json<YardVisitResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = GateInYardVisitCommand {
        appointment_id: body
            .appointment_id
            .map(YardAppointmentId::new)
            .transpose()
            .map_err(validation)?,
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        direction: direction_from_api(body.direction),
        asset_id: YardAssetId::new(body.asset_id).map_err(validation)?,
        driver_name: YardName::new(body.driver_name).map_err(validation)?,
        gate_location_id: YardLocationId::new(body.gate_location_id).map_err(validation)?,
        note: body
            .note
            .map(YardNote::new)
            .transpose()
            .map_err(validation)?,
    };
    let result = repo::yard::gate_in(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_visit(result)?))
}

pub async fn spot_visit(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(visit_id): Path<i64>,
    Json(body): Json<MoveYardVisitRequest>,
) -> V1Result<Json<YardVisitResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = MoveYardVisitCommand {
        visit_id: YardVisitId::new(visit_id).map_err(validation)?,
        expected_revision: YardRevision::new(body.expected_revision.get()).map_err(validation)?,
        destination_location_id: YardLocationId::new(body.destination_location_id)
            .map_err(validation)?,
        note: YardNote::new(body.note).map_err(validation)?,
    };
    let result = repo::yard::spot_visit(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_visit(result)?))
}

pub async fn assign_door(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(visit_id): Path<i64>,
    Json(body): Json<AssignYardVisitDoorRequest>,
) -> V1Result<Json<YardVisitResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = AssignYardVisitDoorCommand {
        visit_id: YardVisitId::new(visit_id).map_err(validation)?,
        expected_revision: YardRevision::new(body.expected_revision.get()).map_err(validation)?,
        door_location_id: YardLocationId::new(body.door_location_id).map_err(validation)?,
        note: YardNote::new(body.note).map_err(validation)?,
    };
    let result = repo::yard::assign_door(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_visit(result)?))
}

pub async fn start_operation(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(visit_id): Path<i64>,
    Json(body): Json<YardDockOperationRequest>,
) -> V1Result<Json<YardVisitResponse>> {
    dock_operation(state, user, idempotency_key, visit_id, body, false).await
}

pub async fn complete_operation(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(visit_id): Path<i64>,
    Json(body): Json<YardDockOperationRequest>,
) -> V1Result<Json<YardVisitResponse>> {
    dock_operation(state, user, idempotency_key, visit_id, body, true).await
}

async fn dock_operation(
    state: AppState,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    visit_id: i64,
    body: YardDockOperationRequest,
    complete: bool,
) -> V1Result<Json<YardVisitResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = YardDockOperationCommand {
        visit_id: YardVisitId::new(visit_id).map_err(validation)?,
        expected_revision: YardRevision::new(body.expected_revision.get()).map_err(validation)?,
        operation: operation_from_api(body.operation),
        note: YardNote::new(body.note).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = if complete {
        repo::yard::complete_operation(&state.db, &user.tenant, &context, &command).await?
    } else {
        repo::yard::start_operation(&state.db, &user.tenant, &context, &command).await?
    };
    Ok(Json(map_visit(result)?))
}

pub async fn reject_visit(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(visit_id): Path<i64>,
    Json(body): Json<YardLifecycleRequest>,
) -> V1Result<Json<YardVisitResponse>> {
    visit_lifecycle(state, user, idempotency_key, visit_id, body, false).await
}

pub async fn gate_out(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(visit_id): Path<i64>,
    Json(body): Json<YardLifecycleRequest>,
) -> V1Result<Json<YardVisitResponse>> {
    visit_lifecycle(state, user, idempotency_key, visit_id, body, true).await
}

async fn visit_lifecycle(
    state: AppState,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    visit_id: i64,
    body: YardLifecycleRequest,
    gate_out: bool,
) -> V1Result<Json<YardVisitResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = YardVisitLifecycleCommand {
        visit_id: YardVisitId::new(visit_id).map_err(validation)?,
        expected_revision: YardRevision::new(body.expected_revision.get()).map_err(validation)?,
        note: YardNote::new(body.note).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = if gate_out {
        repo::yard::gate_out(&state.db, &user.tenant, &context, &command).await?
    } else {
        repo::yard::reject_visit(&state.db, &user.tenant, &context, &command).await?
    };
    Ok(Json(map_visit(result)?))
}

fn map_location(value: YardLocationReadModel) -> V1Result<YardLocationResponse> {
    Ok(YardLocationResponse {
        location_id: value.location_id.get(),
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        code: value.code,
        name: value.name,
        kind: location_kind_to_api(value.kind),
        active: value.active,
        revision: revision(value.revision)?,
    })
}

fn map_asset(value: YardAssetReadModel) -> V1Result<YardAssetResponse> {
    Ok(YardAssetResponse {
        asset_id: value.asset_id.get(),
        kind: asset_kind_to_api(value.kind),
        asset_number: value.asset_number,
        carrier: value.carrier,
        active: value.active,
        revision: revision(value.revision)?,
    })
}

fn map_appointment(value: YardAppointmentReadModel) -> V1Result<YardAppointmentResponse> {
    Ok(YardAppointmentResponse {
        appointment_id: value.appointment_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        direction: direction_to_api(value.direction),
        appointment_number: value.appointment_number,
        scheduled_from: value.window.scheduled_from.to_rfc3339(),
        scheduled_until: value.window.scheduled_until.to_rfc3339(),
        carrier: value.carrier,
        expected_asset_kind: asset_kind_to_api(value.expected_asset_kind),
        expected_asset_number: value.expected_asset_number,
        inbound_load_id: value.inbound_load_id.map(|id| id.get()),
        outbound_load_id: value.outbound_load_id.map(|id| id.get()),
        free_minutes: value.free_minutes.get(),
        status: appointment_status_to_api(value.status),
        revision: revision(value.revision)?,
        note: value.note,
        visit_id: value.visit_id.map(|id| id.get()),
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        updated_by: value.updated_by.map(|id| id.get()),
        updated_at: value.updated_at.map(|value| value.to_rfc3339()),
    })
}

fn map_event(value: YardVisitEventReadModel) -> V1Result<YardVisitEventResponse> {
    Ok(YardVisitEventResponse {
        event_id: value.event_id.get(),
        kind: event_kind_to_api(value.kind),
        from_status: value.from_status.map(visit_status_to_api),
        to_status: visit_status_to_api(value.to_status),
        from_location_id: value.from_location_id.map(|id| id.get()),
        to_location_id: value.to_location_id.map(|id| id.get()),
        operation: value.operation.map(operation_to_api),
        note: value.note,
        resulting_revision: revision(value.resulting_revision)?,
        actor_id: value.actor_id.get(),
        occurred_at: value.occurred_at.to_rfc3339(),
    })
}

fn map_detention(value: YardDetentionReadModel) -> YardDetentionResponse {
    YardDetentionResponse {
        detention_id: value.detention_id.get(),
        total_minutes: value.total_minutes,
        free_minutes: value.free_minutes,
        detention_minutes: value.detention_minutes,
        billable_hours: value.billable_hours,
        billable_event_id: value.billable_event_id.map(|id| id.get()),
        calculated_at: value.calculated_at.to_rfc3339(),
    }
}

fn map_visit(value: YardVisitReadModel) -> V1Result<YardVisitResponse> {
    Ok(YardVisitResponse {
        visit_id: value.visit_id.get(),
        appointment_id: value.appointment_id.map(|id| id.get()),
        appointment_number: value.appointment_number,
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        direction: direction_to_api(value.direction),
        asset_id: value.asset_id.get(),
        asset_kind: asset_kind_to_api(value.asset_kind),
        asset_number: value.asset_number,
        carrier: value.carrier,
        driver_name: value.driver_name,
        status: visit_status_to_api(value.status),
        revision: revision(value.revision)?,
        current_location_id: value.current_location_id.map(|id| id.get()),
        current_location_code: value.current_location_code,
        dock_door_location_id: value.dock_door_location_id.map(|id| id.get()),
        dock_door_code: value.dock_door_code,
        inbound_load_id: value.inbound_load_id.map(|id| id.get()),
        outbound_load_id: value.outbound_load_id.map(|id| id.get()),
        gated_in_at: value.gated_in_at.to_rfc3339(),
        operation_started_at: value.operation_started_at.map(|value| value.to_rfc3339()),
        operation_completed_at: value.operation_completed_at.map(|value| value.to_rfc3339()),
        gated_out_at: value.gated_out_at.map(|value| value.to_rfc3339()),
        rejected_at: value.rejected_at.map(|value| value.to_rfc3339()),
        detention: value.detention.map(map_detention),
        events: value
            .events
            .into_iter()
            .map(map_event)
            .collect::<V1Result<Vec<_>>>()?,
    })
}

const fn direction_from_api(value: ApiDirection) -> YardDirection {
    match value {
        ApiDirection::Inbound => YardDirection::Inbound,
        ApiDirection::Outbound => YardDirection::Outbound,
    }
}

const fn direction_to_api(value: YardDirection) -> ApiDirection {
    match value {
        YardDirection::Inbound => ApiDirection::Inbound,
        YardDirection::Outbound => ApiDirection::Outbound,
    }
}

const fn asset_kind_from_api(value: ApiAssetKind) -> YardAssetKind {
    match value {
        ApiAssetKind::Trailer => YardAssetKind::Trailer,
        ApiAssetKind::Container => YardAssetKind::Container,
    }
}

const fn asset_kind_to_api(value: YardAssetKind) -> ApiAssetKind {
    match value {
        YardAssetKind::Trailer => ApiAssetKind::Trailer,
        YardAssetKind::Container => ApiAssetKind::Container,
    }
}

const fn location_kind_from_api(value: ApiLocationKind) -> YardLocationKind {
    match value {
        ApiLocationKind::Gate => YardLocationKind::Gate,
        ApiLocationKind::Parking => YardLocationKind::Parking,
        ApiLocationKind::DockDoor => YardLocationKind::DockDoor,
        ApiLocationKind::Inspection => YardLocationKind::Inspection,
        ApiLocationKind::Staging => YardLocationKind::Staging,
    }
}

const fn location_kind_to_api(value: YardLocationKind) -> ApiLocationKind {
    match value {
        YardLocationKind::Gate => ApiLocationKind::Gate,
        YardLocationKind::Parking => ApiLocationKind::Parking,
        YardLocationKind::DockDoor => ApiLocationKind::DockDoor,
        YardLocationKind::Inspection => ApiLocationKind::Inspection,
        YardLocationKind::Staging => ApiLocationKind::Staging,
    }
}

const fn operation_from_api(value: ApiOperation) -> YardOperation {
    match value {
        ApiOperation::Loading => YardOperation::Loading,
        ApiOperation::Unloading => YardOperation::Unloading,
    }
}

const fn operation_to_api(value: YardOperation) -> ApiOperation {
    match value {
        YardOperation::Loading => ApiOperation::Loading,
        YardOperation::Unloading => ApiOperation::Unloading,
    }
}

const fn appointment_status_to_api(value: YardAppointmentStatus) -> ApiAppointmentStatus {
    match value {
        YardAppointmentStatus::Scheduled => ApiAppointmentStatus::Scheduled,
        YardAppointmentStatus::CheckedIn => ApiAppointmentStatus::CheckedIn,
        YardAppointmentStatus::Completed => ApiAppointmentStatus::Completed,
        YardAppointmentStatus::Cancelled => ApiAppointmentStatus::Cancelled,
        YardAppointmentStatus::NoShow => ApiAppointmentStatus::NoShow,
    }
}

const fn visit_status_to_api(value: YardVisitStatus) -> ApiVisitStatus {
    match value {
        YardVisitStatus::GatedIn => ApiVisitStatus::GatedIn,
        YardVisitStatus::InYard => ApiVisitStatus::InYard,
        YardVisitStatus::AtDoor => ApiVisitStatus::AtDoor,
        YardVisitStatus::Loading => ApiVisitStatus::Loading,
        YardVisitStatus::Unloading => ApiVisitStatus::Unloading,
        YardVisitStatus::ReadyToDepart => ApiVisitStatus::ReadyToDepart,
        YardVisitStatus::Rejected => ApiVisitStatus::Rejected,
        YardVisitStatus::GatedOut => ApiVisitStatus::GatedOut,
    }
}

const fn event_kind_to_api(value: YardVisitEventKind) -> ApiEventKind {
    match value {
        YardVisitEventKind::GatedIn => ApiEventKind::GatedIn,
        YardVisitEventKind::Spotted => ApiEventKind::Spotted,
        YardVisitEventKind::DoorAssigned => ApiEventKind::DoorAssigned,
        YardVisitEventKind::OperationStarted => ApiEventKind::OperationStarted,
        YardVisitEventKind::OperationCompleted => ApiEventKind::OperationCompleted,
        YardVisitEventKind::Rejected => ApiEventKind::Rejected,
        YardVisitEventKind::GatedOut => ApiEventKind::GatedOut,
    }
}

fn revision(value: YardRevision) -> V1Result<Revision> {
    Revision::new(value.get()).map_err(invalid_result)
}

fn parse_timestamp(value: &str, field: &str) -> V1Result<Timestamp> {
    value
        .parse::<Timestamp>()
        .map_err(|error| AppError::bad_request(format!("{field} is invalid: {error}")).into())
}

fn cursor_filter(request: &YardWorkspaceRequest) -> String {
    format!(
        "{}.{}.{}",
        request
            .facility_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        u8::from(request.include_completed)
    )
}

fn encode_cursor(visit_id: YardVisitId, request: &YardWorkspaceRequest) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        visit_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid yard cursor"))
}

fn decode_cursor(cursor: &OpaqueCursor, request: &YardWorkspaceRequest) -> V1Result<YardVisitId> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("yard workspace"))?;
    let (filter, visit_id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("yard workspace"))?;
    if filter != cursor_filter(request) || visit_id.len() != 16 {
        return Err(V1Error::invalid_cursor_for("yard workspace"));
    }
    let visit_id = i64::from_str_radix(visit_id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("yard workspace"))?;
    YardVisitId::new(visit_id).map_err(|_| V1Error::invalid_cursor_for("yard workspace"))
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}
