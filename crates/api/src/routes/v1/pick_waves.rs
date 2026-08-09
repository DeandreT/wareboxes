use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CancelPickWaveRequest, OpaqueCursor, PickWaveCancellationReason as ApiCancellationReason,
    PickWaveOrderResponse, PickWavePage as ApiPickWavePage, PickWavePageRequest, PickWaveResponse,
    PickWaveSort as ApiPickWaveSort, PickWaveSortDirection as ApiSortDirection,
    PickWaveStatus as ApiPickWaveStatus, PlanPickWaveOrderRequest, PlanPickWaveRequest,
    ReleasePickWaveRequest, Revision,
};
use wareboxes_application::pick_wave::{
    CancelPickWaveCommand, PickWaveCursor, PickWavePage, PickWaveQuery, PickWaveReadModel,
    PickWaveSort, PickWaveSortDirection, PlanPickWaveCommand, PlanPickWaveOrder,
    ReleasePickWaveCommand,
};
use wareboxes_domain::{
    FacilityId, LocationId, OrderId, OrderRevision, PickWaveCancellationNote,
    PickWaveCancellationReason, PickWaveId, PickWaveName, PickWaveRevision, PickWaveStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const READ_PERMISSION: &str = "orders";
const MUTATE_PERMISSION: &str = "wms_supervisor";
const CURSOR_PREFIX: &str = "pw1.";

pub async fn plan(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<PlanPickWaveRequest>,
) -> V1Result<Json<PickWaveResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    user.require_facility(body.facility_id)?;
    let command = plan_command(body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::pick_wave::plan_wave(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_wave(result)?))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(wave_id): Path<i64>,
    Json(body): Json<ReleasePickWaveRequest>,
) -> V1Result<Json<PickWaveResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let command = ReleasePickWaveCommand {
        wave_id: wave_id_value(wave_id)?,
        expected_revision: wave_revision(body.expected_revision)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::pick_wave::release_wave(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_wave(result)?))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(wave_id): Path<i64>,
    Json(body): Json<CancelPickWaveRequest>,
) -> V1Result<Json<PickWaveResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let command = CancelPickWaveCommand {
        wave_id: wave_id_value(wave_id)?,
        expected_revision: wave_revision(body.expected_revision)?,
        reason: map_reason_to_domain(body.reason),
        note: body
            .note
            .map(PickWaveCancellationNote::new)
            .transpose()
            .map_err(domain_validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::pick_wave::cancel_wave(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_wave(result)?))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(wave_id): Path<i64>,
) -> V1Result<Json<PickWaveResponse>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    Ok(Json(map_wave(
        repo::pick_wave::get_wave(&state.db, &user.tenant, wave_id_value(wave_id)?).await?,
    )?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<PickWavePageRequest>,
) -> V1Result<Json<ApiPickWavePage>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    if query.limit.get() > 100 {
        return Err(AppError::bad_request("pick wave page limit must be 1..=100").into());
    }
    let facility_id = query
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let status = query.status.map(map_status_to_domain);
    let sort = map_sort_to_application(query.sort);
    let direction = map_direction_to_application(query.direction);
    let scoped_cursor = query.cursor.as_ref().map(decode_cursor).transpose()?;
    if scoped_cursor.as_ref().is_some_and(|cursor| {
        cursor.facility_id != facility_id.map(FacilityId::get)
            || cursor.status != status
            || cursor.sort != sort
            || cursor.direction != direction
    }) {
        return Err(V1Error::invalid_cursor_for("pick waves"));
    }
    let page = repo::pick_wave::list_waves(
        &state.db,
        &user.tenant,
        &PickWaveQuery {
            facility_id,
            status,
            limit: query.limit.get(),
            sort,
            direction,
            cursor: scoped_cursor.map(|cursor| cursor.cursor),
        },
    )
    .await?;
    Ok(Json(map_page(page, facility_id, status, sort, direction)?))
}

#[cfg_attr(not(feature = "ssr"), allow(dead_code))]
pub(crate) async fn page_for_access(
    state: &AppState,
    access: &wareboxes_core::models::TenantAccess,
    facility_id: Option<FacilityId>,
    status: Option<PickWaveStatus>,
    limit: u16,
) -> AppResult<ApiPickWavePage> {
    let sort = PickWaveSort::default();
    let direction = PickWaveSortDirection::default();
    let page = repo::pick_wave::list_waves(
        &state.db,
        access,
        &PickWaveQuery {
            facility_id,
            status,
            limit,
            sort,
            direction,
            cursor: None,
        },
    )
    .await?;
    map_page(page, facility_id, status, sort, direction)
}

fn plan_command(body: PlanPickWaveRequest) -> V1Result<PlanPickWaveCommand> {
    Ok(PlanPickWaveCommand {
        facility_id: FacilityId::new(body.facility_id).map_err(domain_validation)?,
        destination_location_id: LocationId::new(body.destination_location_id)
            .map_err(domain_validation)?,
        name: PickWaveName::new(body.name).map_err(domain_validation)?,
        orders: body
            .orders
            .into_iter()
            .map(map_plan_order)
            .collect::<V1Result<Vec<_>>>()?,
    })
}

fn map_plan_order(order: PlanPickWaveOrderRequest) -> V1Result<PlanPickWaveOrder> {
    Ok(PlanPickWaveOrder {
        order_id: OrderId::new(order.order_id).map_err(domain_validation)?,
        expected_revision: OrderRevision::new(order.expected_revision.get())
            .map_err(domain_validation)?,
        sequence: order.sequence,
    })
}

fn map_page(
    page: PickWavePage,
    facility_id: Option<FacilityId>,
    status: Option<PickWaveStatus>,
    sort: PickWaveSort,
    direction: PickWaveSortDirection,
) -> AppResult<ApiPickWavePage> {
    let entries = page
        .entries
        .into_iter()
        .map(map_wave)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| {
            encode_cursor(ScopedCursor {
                facility_id: facility_id.map(FacilityId::get),
                status,
                sort,
                direction,
                cursor,
            })
        })
        .transpose()?;
    Ok(ApiPickWavePage::new(entries, next_cursor))
}

fn map_wave(wave: PickWaveReadModel) -> AppResult<PickWaveResponse> {
    Ok(PickWaveResponse {
        wave_id: wave.wave_id.get(),
        facility_id: wave.facility_id.get(),
        facility_name: wave.facility_name,
        destination_location_id: wave.destination_location_id.get(),
        destination_location_name: wave.destination_location_name,
        name: wave.name.into_inner(),
        status: map_status(wave.status),
        revision: revision(wave.revision.get())?,
        order_count: wave.order_count,
        allocation_count: wave.allocation_count,
        pick_task_count: wave.pick_task_count,
        released_quantity: wave.released_quantity,
        orders: wave
            .orders
            .into_iter()
            .map(|order| {
                Ok(PickWaveOrderResponse {
                    order_id: order.order_id.get(),
                    inventory_owner_id: order.inventory_owner_id.get(),
                    order_key: order.order_key,
                    sequence: order.sequence,
                    expected_revision: revision(order.expected_revision.get())?,
                    resulting_revision: order
                        .resulting_revision
                        .map(|value| revision(value.get()))
                        .transpose()?,
                    release_id: order.release_id.map(|value| value.get()),
                    status: order.status.as_str().to_owned(),
                    allocation_count: order.allocation_count,
                    pick_task_count: order.pick_task_count,
                    released_quantity: order.released_quantity,
                })
            })
            .collect::<AppResult<Vec<_>>>()?,
        planned_by: wave.planned_by.get(),
        planned_at: wave.planned_at.to_rfc3339(),
        released_by: wave.released_by.map(|value| value.get()),
        released_at: wave.released_at.map(|value| value.to_rfc3339()),
        cancelled_by: wave.cancelled_by.map(|value| value.get()),
        cancelled_at: wave.cancelled_at.map(|value| value.to_rfc3339()),
        cancellation_reason: wave.cancellation_reason.map(map_reason),
        cancellation_note: wave
            .cancellation_note
            .map(|value| value.as_str().to_owned()),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScopedCursor {
    facility_id: Option<i64>,
    status: Option<PickWaveStatus>,
    sort: PickWaveSort,
    direction: PickWaveSortDirection,
    cursor: PickWaveCursor,
}

fn decode_cursor(cursor: &OpaqueCursor) -> V1Result<ScopedCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick waves"))?;
    let parts = encoded.split('.').collect::<Vec<_>>();
    if parts.len() != 5 {
        return Err(V1Error::invalid_cursor_for("pick waves"));
    }
    let status = match parts[1] {
        "a" => None,
        "p" => Some(PickWaveStatus::Planned),
        "r" => Some(PickWaveStatus::Released),
        "c" => Some(PickWaveStatus::Cancelled),
        _ => return Err(V1Error::invalid_cursor_for("pick waves")),
    };
    Ok(ScopedCursor {
        facility_id: parse_optional_id(parts[0])?,
        status,
        sort: parse_sort(parts[2])?,
        direction: parse_direction(parts[3])?,
        cursor: PickWaveCursor {
            offset: parse_hex_u64(parts[4])?,
        },
    })
}

fn encode_cursor(cursor: ScopedCursor) -> AppResult<OpaqueCursor> {
    let status = match cursor.status {
        None => "a",
        Some(PickWaveStatus::Planned) => "p",
        Some(PickWaveStatus::Released) => "r",
        Some(PickWaveStatus::Cancelled) => "c",
    };
    let facility = cursor
        .facility_id
        .map_or_else(|| "a".to_owned(), |value| format!("{value:016x}"));
    let sort = cursor.sort.as_str();
    let direction = cursor.direction.as_str();
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{facility}.{status}.{sort}.{direction}.{:016x}",
        cursor.cursor.offset
    ))
    .map_err(|_| AppError::internal("generated an invalid pick wave cursor"))
}

fn parse_optional_id(value: &str) -> V1Result<Option<i64>> {
    if value == "a" {
        Ok(None)
    } else {
        parse_hex_i64(value).map(Some)
    }
}

fn parse_hex_i64(value: &str) -> V1Result<i64> {
    if value.len() != 16 {
        return Err(V1Error::invalid_cursor_for("pick waves"));
    }
    i64::from_str_radix(value, 16)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick waves"))
}

fn parse_hex_u64(value: &str) -> V1Result<u64> {
    if value.len() != 16 {
        return Err(V1Error::invalid_cursor_for("pick waves"));
    }
    u64::from_str_radix(value, 16).map_err(|_| V1Error::invalid_cursor_for("pick waves"))
}

fn parse_sort(value: &str) -> V1Result<PickWaveSort> {
    match value {
        "name" => Ok(PickWaveSort::Name),
        "status" => Ok(PickWaveSort::Status),
        "orders" => Ok(PickWaveSort::Orders),
        "tasks" => Ok(PickWaveSort::Tasks),
        "units" => Ok(PickWaveSort::Units),
        "planned_at" => Ok(PickWaveSort::PlannedAt),
        _ => Err(V1Error::invalid_cursor_for("pick waves")),
    }
}

fn parse_direction(value: &str) -> V1Result<PickWaveSortDirection> {
    match value {
        "asc" => Ok(PickWaveSortDirection::Ascending),
        "desc" => Ok(PickWaveSortDirection::Descending),
        _ => Err(V1Error::invalid_cursor_for("pick waves")),
    }
}

fn wave_id_value(value: i64) -> V1Result<PickWaveId> {
    PickWaveId::new(value).map_err(domain_validation)
}

fn wave_revision(value: Revision) -> V1Result<PickWaveRevision> {
    PickWaveRevision::new(value.get()).map_err(domain_validation)
}

fn revision(value: i64) -> AppResult<Revision> {
    Revision::new(value).map_err(|error| AppError::internal(error.to_string()))
}

const fn map_status(status: PickWaveStatus) -> ApiPickWaveStatus {
    match status {
        PickWaveStatus::Planned => ApiPickWaveStatus::Planned,
        PickWaveStatus::Released => ApiPickWaveStatus::Released,
        PickWaveStatus::Cancelled => ApiPickWaveStatus::Cancelled,
    }
}

const fn map_status_to_domain(status: ApiPickWaveStatus) -> PickWaveStatus {
    match status {
        ApiPickWaveStatus::Planned => PickWaveStatus::Planned,
        ApiPickWaveStatus::Released => PickWaveStatus::Released,
        ApiPickWaveStatus::Cancelled => PickWaveStatus::Cancelled,
    }
}

const fn map_sort_to_application(sort: ApiPickWaveSort) -> PickWaveSort {
    match sort {
        ApiPickWaveSort::Name => PickWaveSort::Name,
        ApiPickWaveSort::Status => PickWaveSort::Status,
        ApiPickWaveSort::Orders => PickWaveSort::Orders,
        ApiPickWaveSort::Tasks => PickWaveSort::Tasks,
        ApiPickWaveSort::Units => PickWaveSort::Units,
        ApiPickWaveSort::PlannedAt => PickWaveSort::PlannedAt,
    }
}

const fn map_direction_to_application(direction: ApiSortDirection) -> PickWaveSortDirection {
    match direction {
        ApiSortDirection::Asc => PickWaveSortDirection::Ascending,
        ApiSortDirection::Desc => PickWaveSortDirection::Descending,
    }
}

const fn map_reason(reason: PickWaveCancellationReason) -> ApiCancellationReason {
    match reason {
        PickWaveCancellationReason::OperationalChange => ApiCancellationReason::OperationalChange,
        PickWaveCancellationReason::CapacityConstraint => ApiCancellationReason::CapacityConstraint,
        PickWaveCancellationReason::OrderChange => ApiCancellationReason::OrderChange,
        PickWaveCancellationReason::Other => ApiCancellationReason::Other,
    }
}

const fn map_reason_to_domain(reason: ApiCancellationReason) -> PickWaveCancellationReason {
    match reason {
        ApiCancellationReason::OperationalChange => PickWaveCancellationReason::OperationalChange,
        ApiCancellationReason::CapacityConstraint => PickWaveCancellationReason::CapacityConstraint,
        ApiCancellationReason::OrderChange => PickWaveCancellationReason::OrderChange,
        ApiCancellationReason::Other => PickWaveCancellationReason::Other,
    }
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_filters_time_and_identity() {
        let expected = ScopedCursor {
            facility_id: Some(11),
            status: Some(PickWaveStatus::Planned),
            sort: PickWaveSort::Units,
            direction: PickWaveSortDirection::Ascending,
            cursor: PickWaveCursor { offset: 100 },
        };
        let encoded = encode_cursor(expected).unwrap();
        assert_eq!(decode_cursor(&encoded).unwrap(), expected);
    }

    #[test]
    fn cursor_rejects_other_resources_and_invalid_status() {
        for value in [
            "ps1.a.a.units.asc.0000000000000001",
            "pw1.a.x.units.asc.0000000000000001",
            "pw1.a.p.bad.asc.0000000000000001",
        ] {
            let cursor = OpaqueCursor::new(value).unwrap();
            assert!(decode_cursor(&cursor).is_err(), "{value}");
        }
    }
}
