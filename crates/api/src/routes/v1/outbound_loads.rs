mod mapping;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use wareboxes_api_contract::v1::{
    CancelOutboundLoadRequest, CancelOutboundLoadResponse, CompleteOutboundLoadLoadingRequest,
    CompleteOutboundLoadLoadingResponse, ConfirmOutboundLoadDepartureRequest,
    ConfirmOutboundLoadDepartureResponse, LoadOutboundCartonRequest, MovePackedCartonResponse,
    OpaqueCursor, OutboundLoadQueuePage, OutboundLoadQueuePageRequest, OutboundLoadQueueSort,
    OutboundLoadQueueSortDirection, OutboundLoadResponse, OutboundLoadStatus as ApiStatus,
    PackedCartonPositionResponse, PlanOutboundLoadRequest, PlanOutboundLoadResponse,
    ReleaseOutboundLoadRequest, ReleaseOutboundLoadResponse, StageOutboundCartonRequest,
    StartOutboundLoadLoadingRequest, StartOutboundLoadLoadingResponse, UnloadOutboundCartonRequest,
    UnstageOutboundCartonRequest,
};
use wareboxes_application::outbound_load::{
    CancelOutboundLoadCommand, CompleteOutboundLoadLoadingCommand,
    ConfirmOutboundLoadDepartureCommand, LoadPackedCartonCommand, OutboundLoadQuery,
    OutboundLoadQueueQuery, OutboundLoadQueueSort as ApplicationQueueSort,
    OutboundLoadQueueSortDirection as ApplicationSortDirection, PackedCartonPositionQuery,
    PlanOutboundLoadCarton, PlanOutboundLoadCommand, PlanOutboundLoadShipment,
    ReleaseOutboundLoadCommand, StagePackedCartonCommand, StartOutboundLoadLoadingCommand,
    UnloadPackedCartonCommand, UnstagePackedCartonCommand,
};
#[cfg(feature = "ssr")]
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CarrierCode, CartonId, FacilityId, LocationId, OrderRevision, OutboundLoadCancellationDetails,
    OutboundLoadCancellationNote, OutboundLoadCancellationReason, OutboundLoadId,
    OutboundLoadReference, OutboundLoadRevision, OutboundLoadScanValue, OutboundLoadStatus,
    PackedCartonPositionRevision, SealNumber, ShipmentId, ShipmentRevision, Timestamp,
    TrailerNumber,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const CURSOR_PREFIX: &str = "ol2.";

pub async fn plan(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<PlanOutboundLoadRequest>,
) -> V1Result<Json<PlanOutboundLoadResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = PlanOutboundLoadCommand {
        facility_id: id(body.facility_id, FacilityId::new, "facility ID")?,
        load_reference: OutboundLoadReference::new(body.load_reference).map_err(invalid)?,
        carrier_code: CarrierCode::new(body.carrier_code).map_err(invalid)?,
        staging_location_id: id(
            body.staging_location_id,
            LocationId::new,
            "staging location ID",
        )?,
        scheduled_departure_at: parse_optional_timestamp(
            body.scheduled_departure_at.as_deref(),
            "scheduled_departure_at",
        )?,
        shipments: body
            .shipments
            .into_iter()
            .map(|shipment| {
                Ok(PlanOutboundLoadShipment {
                    shipment_id: id(shipment.shipment_id, ShipmentId::new, "shipment ID")?,
                    expected_shipment_revision: shipment_revision(
                        shipment.expected_shipment_revision.get(),
                    )?,
                    expected_order_revision: order_revision(
                        shipment.expected_order_revision.get(),
                    )?,
                    shipment_sequence: shipment.shipment_sequence,
                    cartons: shipment
                        .cartons
                        .into_iter()
                        .map(|carton| {
                            Ok(PlanOutboundLoadCarton {
                                carton_id: id(carton.carton_id, CartonId::new, "carton ID")?,
                                load_sequence: carton.load_sequence,
                            })
                        })
                        .collect::<V1Result<Vec<_>>>()?,
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
    };
    let result = repo::outbound_load::plan(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::plan_result(result)?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<OutboundLoadQueuePageRequest>,
) -> V1Result<Json<OutboundLoadQueuePage>> {
    user.require_permission(&state.db, "wms").await?;
    let filters = CursorFilters {
        facility_id: query
            .facility_id
            .map(|value| id(value, FacilityId::new, "facility ID"))
            .transpose()?,
        status: query.status.map(map_status),
        scheduled_from: parse_optional_timestamp(
            query.scheduled_from.as_deref(),
            "scheduled_from",
        )?,
        scheduled_to: parse_optional_timestamp(query.scheduled_to.as_deref(), "scheduled_to")?,
        sort: query.sort,
        direction: query.direction,
    };
    if filters
        .scheduled_from
        .zip(filters.scheduled_to)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(invalid("scheduled_from must not be after scheduled_to"));
    }
    let offset = decode_bound_cursor(query.cursor.as_ref(), filters)?;
    let page = repo::outbound_load::list(
        &state.db,
        &user.tenant,
        &OutboundLoadQueueQuery {
            facility_id: filters.facility_id,
            status: filters.status,
            scheduled_from: filters.scheduled_from,
            scheduled_to: filters.scheduled_to,
            offset,
            limit: u32::from(query.limit.get()),
            sort: map_queue_sort(query.sort),
            direction: map_queue_direction(query.direction),
        },
    )
    .await?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(BoundCursor { filters, offset }))
        .transpose()?;
    Ok(Json(mapping::queue_page(page.entries, next_cursor)?))
}

#[cfg(feature = "ssr")]
pub(crate) async fn page_for_access(
    state: &AppState,
    access: &TenantAccess,
    facility_id: Option<i64>,
    status: Option<ApiStatus>,
    limit: u16,
) -> AppResult<OutboundLoadQueuePage> {
    let filters = CursorFilters {
        facility_id: facility_id
            .map(|value| {
                FacilityId::new(value).map_err(|error| AppError::bad_request(error.to_string()))
            })
            .transpose()?,
        status: status.map(map_status),
        scheduled_from: None,
        scheduled_to: None,
        sort: OutboundLoadQueueSort::ScheduledDeparture,
        direction: OutboundLoadQueueSortDirection::Ascending,
    };
    let page = repo::outbound_load::list(
        &state.db,
        access,
        &OutboundLoadQueueQuery {
            facility_id: filters.facility_id,
            status: filters.status,
            scheduled_from: None,
            scheduled_to: None,
            offset: 0,
            limit: u32::from(limit),
            sort: ApplicationQueueSort::ScheduledDeparture,
            direction: ApplicationSortDirection::Ascending,
        },
    )
    .await?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(BoundCursor { filters, offset }))
        .transpose()?;
    mapping::queue_page(page.entries, next_cursor)
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
) -> V1Result<Json<OutboundLoadResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let result = repo::outbound_load::get(
        &state.db,
        &user.tenant,
        OutboundLoadQuery {
            outbound_load_id: id(load_id, OutboundLoadId::new, "outbound load ID")?,
        },
    )
    .await?;
    Ok(Json(mapping::load(result)?))
}

pub async fn get_by_barcode(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_barcode): Path<String>,
) -> V1Result<Json<OutboundLoadResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let barcode = scan(load_barcode)?;
    let result = repo::outbound_load::get_by_barcode(&state.db, &user.tenant, &barcode).await?;
    Ok(Json(mapping::load(result)?))
}

pub async fn position(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(carton_id): Path<i64>,
) -> V1Result<Json<PackedCartonPositionResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let result = repo::outbound_load::packed_carton_position(
        &state.db,
        &user.tenant,
        PackedCartonPositionQuery {
            carton_id: id(carton_id, CartonId::new, "carton ID")?,
        },
    )
    .await?;
    Ok(Json(mapping::position(result)?))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(load_id): Path<i64>,
    Json(body): Json<ReleaseOutboundLoadRequest>,
) -> V1Result<Json<ReleaseOutboundLoadResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = ReleaseOutboundLoadCommand {
        outbound_load_id: load_id_value(load_id)?,
        expected_revision: load_revision(body.expected_revision.get())?,
    };
    let result = repo::outbound_load::release(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::release(result)?))
}

pub async fn start_loading(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(load_id): Path<i64>,
    Json(body): Json<StartOutboundLoadLoadingRequest>,
) -> V1Result<Json<StartOutboundLoadLoadingResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = StartOutboundLoadLoadingCommand {
        outbound_load_id: load_id_value(load_id)?,
        expected_revision: load_revision(body.expected_revision.get())?,
        load_barcode: scan(body.load_barcode)?,
        staging_location_barcode: scan(body.staging_location_barcode)?,
        dock_location_barcode: scan(body.dock_location_barcode)?,
        trailer_number: TrailerNumber::new(body.trailer_number).map_err(invalid)?,
    };
    let result = repo::outbound_load::start_loading(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::start(result)?))
}

pub async fn complete_loading(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(load_id): Path<i64>,
    Json(body): Json<CompleteOutboundLoadLoadingRequest>,
) -> V1Result<Json<CompleteOutboundLoadLoadingResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = CompleteOutboundLoadLoadingCommand {
        outbound_load_id: load_id_value(load_id)?,
        expected_revision: load_revision(body.expected_revision.get())?,
        load_barcode: scan(body.load_barcode)?,
        dock_location_barcode: scan(body.dock_location_barcode)?,
        trailer_number: TrailerNumber::new(body.trailer_number).map_err(invalid)?,
        seal_number: SealNumber::new(body.seal_number).map_err(invalid)?,
    };
    let result = repo::outbound_load::complete_loading(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::complete(result)?))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(load_id): Path<i64>,
    Json(body): Json<CancelOutboundLoadRequest>,
) -> V1Result<Json<CancelOutboundLoadResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let reason = map_cancellation_reason(body.reason);
    let note = body
        .note
        .map(OutboundLoadCancellationNote::new)
        .transpose()
        .map_err(invalid)?;
    let command = CancelOutboundLoadCommand {
        outbound_load_id: load_id_value(load_id)?,
        expected_revision: load_revision(body.expected_revision.get())?,
        details: OutboundLoadCancellationDetails::new(reason, note).map_err(invalid)?,
    };
    let result = repo::outbound_load::cancel(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::cancel(result)?))
}

pub async fn stage(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((load_id, carton_id)): Path<(i64, i64)>,
    Json(body): Json<StageOutboundCartonRequest>,
) -> V1Result<Json<MovePackedCartonResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = StagePackedCartonCommand {
        outbound_load_id: load_id_value(load_id)?,
        carton_id: carton_id_value(carton_id)?,
        expected_load_revision: load_revision(body.expected_load_revision.get())?,
        expected_position_revision: position_revision(body.expected_position_revision.get())?,
        source_location_barcode: scan(body.source_location_barcode)?,
        carton_barcode: scan(body.carton_barcode)?,
        staging_location_barcode: scan(body.staging_location_barcode)?,
    };
    let result = repo::outbound_load::stage_carton(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::movement_result(result)?))
}

pub async fn load_carton(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((load_id, carton_id)): Path<(i64, i64)>,
    Json(body): Json<LoadOutboundCartonRequest>,
) -> V1Result<Json<MovePackedCartonResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = LoadPackedCartonCommand {
        outbound_load_id: load_id_value(load_id)?,
        carton_id: carton_id_value(carton_id)?,
        expected_load_revision: load_revision(body.expected_load_revision.get())?,
        expected_position_revision: position_revision(body.expected_position_revision.get())?,
        staging_location_barcode: scan(body.staging_location_barcode)?,
        carton_barcode: scan(body.carton_barcode)?,
        trailer_number: TrailerNumber::new(body.trailer_number).map_err(invalid)?,
    };
    let result = repo::outbound_load::load_carton(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::movement_result(result)?))
}

pub async fn unload(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((load_id, carton_id)): Path<(i64, i64)>,
    Json(body): Json<UnloadOutboundCartonRequest>,
) -> V1Result<Json<MovePackedCartonResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = UnloadPackedCartonCommand {
        outbound_load_id: load_id_value(load_id)?,
        carton_id: carton_id_value(carton_id)?,
        expected_load_revision: load_revision(body.expected_load_revision.get())?,
        expected_position_revision: position_revision(body.expected_position_revision.get())?,
        trailer_number: TrailerNumber::new(body.trailer_number).map_err(invalid)?,
        carton_barcode: scan(body.carton_barcode)?,
        staging_location_barcode: scan(body.staging_location_barcode)?,
    };
    let result = repo::outbound_load::unload_carton(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::movement_result(result)?))
}

pub async fn unstage(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((load_id, carton_id)): Path<(i64, i64)>,
    Json(body): Json<UnstageOutboundCartonRequest>,
) -> V1Result<Json<MovePackedCartonResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = UnstagePackedCartonCommand {
        outbound_load_id: load_id_value(load_id)?,
        carton_id: carton_id_value(carton_id)?,
        expected_load_revision: load_revision(body.expected_load_revision.get())?,
        expected_position_revision: position_revision(body.expected_position_revision.get())?,
        staging_location_barcode: scan(body.staging_location_barcode)?,
        carton_barcode: scan(body.carton_barcode)?,
        return_location_barcode: scan(body.return_location_barcode)?,
    };
    let result = repo::outbound_load::unstage_carton(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::movement_result(result)?))
}

pub async fn depart(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(load_id): Path<i64>,
    Json(body): Json<ConfirmOutboundLoadDepartureRequest>,
) -> V1Result<Json<ConfirmOutboundLoadDepartureResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = ConfirmOutboundLoadDepartureCommand {
        outbound_load_id: load_id_value(load_id)?,
        expected_revision: load_revision(body.expected_revision.get())?,
        load_barcode: scan(body.load_barcode)?,
        dock_location_barcode: scan(body.dock_location_barcode)?,
        trailer_number: TrailerNumber::new(body.trailer_number).map_err(invalid)?,
        seal_number: SealNumber::new(body.seal_number).map_err(invalid)?,
    };
    let result = repo::outbound_load::confirm_departure(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(mapping::departure(result)?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorFilters {
    facility_id: Option<FacilityId>,
    status: Option<OutboundLoadStatus>,
    scheduled_from: Option<Timestamp>,
    scheduled_to: Option<Timestamp>,
    sort: OutboundLoadQueueSort,
    direction: OutboundLoadQueueSortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoundCursor {
    filters: CursorFilters,
    offset: u64,
}

fn encode_cursor(cursor: BoundCursor) -> AppResult<OpaqueCursor> {
    let time = |value: Option<Timestamp>| {
        value.map_or_else(
            || "a".into(),
            |value| format!("{:016x}", (value.timestamp_micros() as u64) ^ (1_u64 << 63)),
        )
    };
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{}.{}.{}.{}.{}.{:016x}",
        cursor
            .filters
            .facility_id
            .map_or_else(|| "a".into(), |id| format!("{:016x}", id.get())),
        cursor.filters.status.map_or("a", status_code),
        time(cursor.filters.scheduled_from),
        time(cursor.filters.scheduled_to),
        queue_sort_code(cursor.filters.sort),
        queue_direction_code(cursor.filters.direction),
        cursor.offset,
    ))
    .map_err(|_| AppError::internal("generated an invalid outbound load cursor"))
}

fn decode_cursor(cursor: &OpaqueCursor) -> V1Result<BoundCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("outbound loads"))?;
    let parts = encoded.split('.').collect::<Vec<_>>();
    if parts.len() != 7 {
        return Err(V1Error::invalid_cursor_for("outbound loads"));
    }
    Ok(BoundCursor {
        filters: CursorFilters {
            facility_id: parse_optional_id(parts[0], FacilityId::new)?,
            status: parse_status_code(parts[1])?,
            scheduled_from: parse_cursor_time(parts[2])?,
            scheduled_to: parse_cursor_time(parts[3])?,
            sort: parse_queue_sort_code(parts[4])?,
            direction: parse_queue_direction_code(parts[5])?,
        },
        offset: parse_offset(parts[6])?,
    })
}

fn decode_bound_cursor(cursor: Option<&OpaqueCursor>, filters: CursorFilters) -> V1Result<u64> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let decoded = decode_cursor(cursor)?;
    if decoded.filters != filters {
        return Err(V1Error::invalid_cursor_for("outbound loads"));
    }
    Ok(decoded.offset)
}

fn parse_offset(value: &str) -> V1Result<u64> {
    if value.len() != 16 {
        return Err(V1Error::invalid_cursor_for("outbound loads"));
    }
    u64::from_str_radix(value, 16).map_err(|_| V1Error::invalid_cursor_for("outbound loads"))
}

fn parse_cursor_time(value: &str) -> V1Result<Option<Timestamp>> {
    if value == "a" {
        return Ok(None);
    }
    if value.len() != 16 {
        return Err(V1Error::invalid_cursor_for("outbound loads"));
    }
    let sortable = u64::from_str_radix(value, 16)
        .map_err(|_| V1Error::invalid_cursor_for("outbound loads"))?;
    DateTime::<Utc>::from_timestamp_micros((sortable ^ (1_u64 << 63)) as i64)
        .ok_or_else(|| V1Error::invalid_cursor_for("outbound loads"))
        .map(Some)
}

fn parse_optional_id<T, E>(
    value: &str,
    constructor: impl FnOnce(i64) -> Result<T, E>,
) -> V1Result<Option<T>> {
    if value == "a" {
        Ok(None)
    } else {
        parse_id(value, constructor).map(Some)
    }
}

fn parse_id<T, E>(value: &str, constructor: impl FnOnce(i64) -> Result<T, E>) -> V1Result<T> {
    if value.len() != 16 {
        return Err(V1Error::invalid_cursor_for("outbound loads"));
    }
    let value = i64::from_str_radix(value, 16)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| V1Error::invalid_cursor_for("outbound loads"))?;
    constructor(value).map_err(|_| V1Error::invalid_cursor_for("outbound loads"))
}

fn status_code(status: OutboundLoadStatus) -> &'static str {
    match status {
        OutboundLoadStatus::Planned => "p",
        OutboundLoadStatus::Staging => "s",
        OutboundLoadStatus::Loading => "l",
        OutboundLoadStatus::ReadyToDepart => "r",
        OutboundLoadStatus::Departed => "d",
        OutboundLoadStatus::Cancelled => "c",
    }
}
fn parse_status_code(value: &str) -> V1Result<Option<OutboundLoadStatus>> {
    Ok(match value {
        "a" => None,
        "p" => Some(OutboundLoadStatus::Planned),
        "s" => Some(OutboundLoadStatus::Staging),
        "l" => Some(OutboundLoadStatus::Loading),
        "r" => Some(OutboundLoadStatus::ReadyToDepart),
        "d" => Some(OutboundLoadStatus::Departed),
        "c" => Some(OutboundLoadStatus::Cancelled),
        _ => return Err(V1Error::invalid_cursor_for("outbound loads")),
    })
}
const fn queue_sort_code(sort: OutboundLoadQueueSort) -> &'static str {
    match sort {
        OutboundLoadQueueSort::Reference => "r",
        OutboundLoadQueueSort::Status => "s",
        OutboundLoadQueueSort::Progress => "p",
        OutboundLoadQueueSort::Facility => "f",
        OutboundLoadQueueSort::Trailer => "t",
        OutboundLoadQueueSort::ScheduledDeparture => "d",
    }
}
fn parse_queue_sort_code(value: &str) -> V1Result<OutboundLoadQueueSort> {
    match value {
        "r" => Ok(OutboundLoadQueueSort::Reference),
        "s" => Ok(OutboundLoadQueueSort::Status),
        "p" => Ok(OutboundLoadQueueSort::Progress),
        "f" => Ok(OutboundLoadQueueSort::Facility),
        "t" => Ok(OutboundLoadQueueSort::Trailer),
        "d" => Ok(OutboundLoadQueueSort::ScheduledDeparture),
        _ => Err(V1Error::invalid_cursor_for("outbound loads")),
    }
}
const fn queue_direction_code(direction: OutboundLoadQueueSortDirection) -> &'static str {
    match direction {
        OutboundLoadQueueSortDirection::Ascending => "a",
        OutboundLoadQueueSortDirection::Descending => "d",
    }
}
fn parse_queue_direction_code(value: &str) -> V1Result<OutboundLoadQueueSortDirection> {
    match value {
        "a" => Ok(OutboundLoadQueueSortDirection::Ascending),
        "d" => Ok(OutboundLoadQueueSortDirection::Descending),
        _ => Err(V1Error::invalid_cursor_for("outbound loads")),
    }
}
const fn map_queue_sort(sort: OutboundLoadQueueSort) -> ApplicationQueueSort {
    match sort {
        OutboundLoadQueueSort::Reference => ApplicationQueueSort::Reference,
        OutboundLoadQueueSort::Status => ApplicationQueueSort::Status,
        OutboundLoadQueueSort::Progress => ApplicationQueueSort::Progress,
        OutboundLoadQueueSort::Facility => ApplicationQueueSort::Facility,
        OutboundLoadQueueSort::Trailer => ApplicationQueueSort::Trailer,
        OutboundLoadQueueSort::ScheduledDeparture => ApplicationQueueSort::ScheduledDeparture,
    }
}
const fn map_queue_direction(
    direction: OutboundLoadQueueSortDirection,
) -> ApplicationSortDirection {
    match direction {
        OutboundLoadQueueSortDirection::Ascending => ApplicationSortDirection::Ascending,
        OutboundLoadQueueSortDirection::Descending => ApplicationSortDirection::Descending,
    }
}
fn map_status(status: ApiStatus) -> OutboundLoadStatus {
    match status {
        ApiStatus::Planned => OutboundLoadStatus::Planned,
        ApiStatus::Staging => OutboundLoadStatus::Staging,
        ApiStatus::Loading => OutboundLoadStatus::Loading,
        ApiStatus::ReadyToDepart => OutboundLoadStatus::ReadyToDepart,
        ApiStatus::Departed => OutboundLoadStatus::Departed,
        ApiStatus::Cancelled => OutboundLoadStatus::Cancelled,
    }
}
fn map_cancellation_reason(
    reason: wareboxes_api_contract::v1::OutboundLoadCancellationReason,
) -> OutboundLoadCancellationReason {
    match reason {
        wareboxes_api_contract::v1::OutboundLoadCancellationReason::RouteCancelled => {
            OutboundLoadCancellationReason::RouteCancelled
        }
        wareboxes_api_contract::v1::OutboundLoadCancellationReason::CarrierCancelled => {
            OutboundLoadCancellationReason::CarrierCancelled
        }
        wareboxes_api_contract::v1::OutboundLoadCancellationReason::EquipmentUnavailable => {
            OutboundLoadCancellationReason::EquipmentUnavailable
        }
        wareboxes_api_contract::v1::OutboundLoadCancellationReason::PlanningError => {
            OutboundLoadCancellationReason::PlanningError
        }
        wareboxes_api_contract::v1::OutboundLoadCancellationReason::Other => {
            OutboundLoadCancellationReason::Other
        }
    }
}
fn parse_optional_timestamp(value: Option<&str>, field: &str) -> V1Result<Option<Timestamp>> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|_| invalid(format!("{field} must be RFC 3339")))
        })
        .transpose()
}
fn scan(value: String) -> V1Result<OutboundLoadScanValue> {
    OutboundLoadScanValue::new(value).map_err(invalid)
}
fn load_id_value(value: i64) -> V1Result<OutboundLoadId> {
    id(value, OutboundLoadId::new, "outbound load ID")
}
fn carton_id_value(value: i64) -> V1Result<CartonId> {
    id(value, CartonId::new, "carton ID")
}
fn load_revision(value: i64) -> V1Result<OutboundLoadRevision> {
    OutboundLoadRevision::new(value).map_err(invalid)
}
fn position_revision(value: i64) -> V1Result<PackedCartonPositionRevision> {
    PackedCartonPositionRevision::new(value).map_err(invalid)
}
fn shipment_revision(value: i64) -> V1Result<ShipmentRevision> {
    ShipmentRevision::new(value).map_err(invalid)
}
fn order_revision(value: i64) -> V1Result<OrderRevision> {
    OrderRevision::new(value).map_err(invalid)
}
fn id<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>, field: &str) -> V1Result<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(|error| invalid(format!("{field}: {error}")))
}
fn invalid(message: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(message.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_round_trips_all_filters() {
        let expected = BoundCursor {
            filters: CursorFilters {
                facility_id: Some(FacilityId::new(2).unwrap()),
                status: Some(OutboundLoadStatus::Loading),
                scheduled_from: Some("2026-08-01T00:00:00Z".parse().unwrap()),
                scheduled_to: None,
                sort: OutboundLoadQueueSort::Progress,
                direction: OutboundLoadQueueSortDirection::Descending,
            },
            offset: 100,
        };
        assert_eq!(
            decode_cursor(&encode_cursor(expected.clone()).unwrap()).unwrap(),
            expected
        );
    }

    #[test]
    fn cursor_rejects_a_different_sort() {
        let filters = CursorFilters {
            facility_id: None,
            status: Some(OutboundLoadStatus::Staging),
            scheduled_from: None,
            scheduled_to: None,
            sort: OutboundLoadQueueSort::Reference,
            direction: OutboundLoadQueueSortDirection::Ascending,
        };
        let cursor = encode_cursor(BoundCursor {
            filters,
            offset: 100,
        })
        .unwrap();
        let changed = CursorFilters {
            sort: OutboundLoadQueueSort::Progress,
            ..filters
        };

        assert!(decode_bound_cursor(Some(&cursor), changed).is_err());
    }
}
