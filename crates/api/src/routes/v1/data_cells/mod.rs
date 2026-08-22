use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use wareboxes_api_contract::v1::{
    ChangeDataCellStatusRequest, DataCellEventPage as ApiEventPage, DataCellEventPageRequest,
    DataCellEventResponse, DataCellMode as ApiMode, DataCellPage as ApiPage, DataCellPageRequest,
    DataCellResponse, DataCellStatus as ApiStatus, OpaqueCursor, ReconfigureDataCellRequest,
    RegisterDataCellRequest,
};
use wareboxes_application::data_cell::{
    ChangeDataCellStatusCommand, DataCellCursor, DataCellEventCursor, DataCellEventPageQuery,
    DataCellEventReadModel, DataCellPageQuery, DataCellReadModel, ReconfigureDataCellCommand,
    RegisterDataCellCommand,
};
use wareboxes_domain::{
    DataCellCapacity, DataCellId, DataCellKey, DataCellMode, DataCellName, DataCellReason,
    DataCellRegion, DataCellRevision, DataCellStatus, DataResidencyCode,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const CURSOR_PREFIX: &str = "dcp1.";
const EVENT_CURSOR_PREFIX: &str = "dce1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<DataCellPageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_platform_administrator(&state.db).await?;
    let region = request
        .region
        .map(DataCellRegion::new)
        .transpose()
        .map_err(validation)?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, request.status, region.as_ref()))
        .transpose()?;
    let page = repo::data_cells::page(
        &state.db,
        &user.tenant,
        &DataCellPageQuery {
            status: request.status.map(map_status),
            region: region.as_ref().map(|value| value.as_str().to_owned()),
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_cursor(cursor, request.status, region.as_ref()))
        .transpose()?;
    Ok(Json(ApiPage::new(
        page.items
            .into_iter()
            .map(map_response)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(data_cell_id): Path<i64>,
) -> V1Result<Json<DataCellResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let result = repo::data_cells::by_id(
        &state.db,
        &user.tenant,
        DataCellId::new(data_cell_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn events(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(data_cell_id): Path<i64>,
    Query(request): Query<DataCellEventPageRequest>,
) -> V1Result<Json<ApiEventPage>> {
    user.require_platform_administrator(&state.db).await?;
    let cursor = request
        .cursor
        .as_ref()
        .map(decode_event_cursor)
        .transpose()?;
    let page = repo::data_cells::event_page(
        &state.db,
        &user.tenant,
        &DataCellEventPageQuery {
            data_cell_id: DataCellId::new(data_cell_id).map_err(validation)?,
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page.next_cursor.map(encode_event_cursor).transpose()?;
    Ok(Json(ApiEventPage::new(
        page.items
            .into_iter()
            .map(map_event)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn register(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RegisterDataCellRequest>,
) -> V1Result<Json<DataCellResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let mode = map_mode(body.mode);
    let max_tenants = DataCellCapacity::new(body.max_tenants).map_err(validation)?;
    mode.validate_capacity(max_tenants).map_err(validation)?;
    let command = RegisterDataCellCommand {
        key: DataCellKey::new(body.key).map_err(validation)?,
        name: DataCellName::new(body.name).map_err(validation)?,
        region: DataCellRegion::new(body.region).map_err(validation)?,
        residency: DataResidencyCode::new(body.residency).map_err(validation)?,
        mode,
        max_tenants,
    };
    let result = repo::data_cells::register(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn reconfigure(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(data_cell_id): Path<i64>,
    Json(body): Json<ReconfigureDataCellRequest>,
) -> V1Result<Json<DataCellResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = ReconfigureDataCellCommand {
        data_cell_id: DataCellId::new(data_cell_id).map_err(validation)?,
        expected_revision: DataCellRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        name: DataCellName::new(body.name).map_err(validation)?,
        max_tenants: DataCellCapacity::new(body.max_tenants).map_err(validation)?,
        reason: DataCellReason::new(body.reason).map_err(validation)?,
    };
    let result = repo::data_cells::reconfigure(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn change_status(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(data_cell_id): Path<i64>,
    Json(body): Json<ChangeDataCellStatusRequest>,
) -> V1Result<Json<DataCellResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = ChangeDataCellStatusCommand {
        data_cell_id: DataCellId::new(data_cell_id).map_err(validation)?,
        expected_revision: DataCellRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        status: map_status(body.status),
        reason: DataCellReason::new(body.reason).map_err(validation)?,
    };
    let result = repo::data_cells::change_status(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub(crate) fn map_response(value: DataCellReadModel) -> V1Result<DataCellResponse> {
    let placement_count = u32::try_from(value.placement_count)
        .map_err(|_| AppError::internal("data-cell placement count is invalid"))?;
    let reserved_inbound_move_count = u32::try_from(value.reserved_inbound_move_count)
        .map_err(|_| AppError::internal("data-cell inbound move reservation count is invalid"))?;
    let reserved_rollback_move_count = u32::try_from(value.reserved_rollback_move_count)
        .map_err(|_| AppError::internal("data-cell rollback reservation count is invalid"))?;
    let committed_slot_count = placement_count
        .checked_add(reserved_inbound_move_count)
        .and_then(|count| count.checked_add(reserved_rollback_move_count))
        .ok_or_else(|| AppError::internal("data-cell committed slot count overflow"))?;
    Ok(DataCellResponse {
        data_cell_id: value.data_cell_id.get(),
        key: value.key,
        name: value.name,
        region: value.region,
        residency: value.residency,
        mode: map_mode_to_api(value.mode),
        status: map_status_to_api(value.status),
        revision: wareboxes_api_contract::v1::Revision::new(value.revision.get())
            .map_err(invalid_result)?,
        max_tenants: value.max_tenants,
        placement_count: value.placement_count,
        reserved_inbound_move_count: value.reserved_inbound_move_count,
        reserved_rollback_move_count: value.reserved_rollback_move_count,
        available_tenant_slots: value.max_tenants.saturating_sub(committed_slot_count),
        created_at: value.created_at.to_rfc3339(),
        created_by: value.created_by.map(|value| value.get()),
        changed_at: value.changed_at.map(|value| value.to_rfc3339()),
        changed_by: value.changed_by.map(|value| value.get()),
        change_reason: value.change_reason,
    })
}

fn map_event(value: DataCellEventReadModel) -> V1Result<DataCellEventResponse> {
    Ok(DataCellEventResponse {
        event_id: value.event_id,
        data_cell_id: value.data_cell_id.get(),
        action: value.action,
        cell_revision: wareboxes_api_contract::v1::Revision::new(value.cell_revision.get())
            .map_err(invalid_result)?,
        previous_status: value.previous_status.map(map_status_to_api),
        resulting_status: map_status_to_api(value.resulting_status),
        actor_id: value.actor_id.map(|value| value.get()),
        occurred_at: value.occurred_at.to_rfc3339(),
        reason: value.reason,
        evidence: value.evidence,
    })
}

const fn map_status(value: ApiStatus) -> DataCellStatus {
    match value {
        ApiStatus::Provisioning => DataCellStatus::Provisioning,
        ApiStatus::Active => DataCellStatus::Active,
        ApiStatus::Draining => DataCellStatus::Draining,
        ApiStatus::Retired => DataCellStatus::Retired,
    }
}

const fn map_status_to_api(value: DataCellStatus) -> ApiStatus {
    match value {
        DataCellStatus::Provisioning => ApiStatus::Provisioning,
        DataCellStatus::Active => ApiStatus::Active,
        DataCellStatus::Draining => ApiStatus::Draining,
        DataCellStatus::Retired => ApiStatus::Retired,
    }
}

const fn map_mode(value: ApiMode) -> DataCellMode {
    match value {
        ApiMode::Shared => DataCellMode::Shared,
        ApiMode::Dedicated => DataCellMode::Dedicated,
    }
}

const fn map_mode_to_api(value: DataCellMode) -> ApiMode {
    match value {
        DataCellMode::Shared => ApiMode::Shared,
        DataCellMode::Dedicated => ApiMode::Dedicated,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPayload {
    created_at: String,
    data_cell_id: i64,
    status: Option<ApiStatus>,
    region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EventCursorPayload {
    occurred_at: String,
    event_id: i64,
}

fn encode_cursor(
    cursor: DataCellCursor,
    status: Option<ApiStatus>,
    region: Option<&DataCellRegion>,
) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&CursorPayload {
        created_at: cursor.after_created_at.to_rfc3339(),
        data_cell_id: cursor.after_data_cell_id.get(),
        status,
        region: region.map(|value| value.as_str().to_owned()),
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| AppError::internal(error.to_string()).into())
}

#[cfg(feature = "ssr")]
pub(crate) fn encode_cursor_for_web(
    cursor: DataCellCursor,
) -> crate::error::AppResult<OpaqueCursor> {
    encode_cursor(cursor, None, None)
        .map_err(|_| AppError::internal("could not encode data-cell cursor"))
}

#[cfg(feature = "ssr")]
pub(crate) fn encode_active_cursor_for_web(
    cursor: DataCellCursor,
) -> crate::error::AppResult<OpaqueCursor> {
    encode_cursor(cursor, Some(ApiStatus::Active), None)
        .map_err(|_| AppError::internal("could not encode data-cell cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    status: Option<ApiStatus>,
    region: Option<&DataCellRegion>,
) -> V1Result<DataCellCursor> {
    let payload: CursorPayload = decode_payload(cursor, CURSOR_PREFIX, "data-cell cursor")?;
    if payload.status != status || payload.region.as_deref() != region.map(DataCellRegion::as_str) {
        return Err(AppError::bad_request("data-cell cursor does not match filters").into());
    }
    Ok(DataCellCursor {
        after_created_at: chrono::DateTime::parse_from_rfc3339(&payload.created_at)
            .map_err(|_| AppError::bad_request("data-cell cursor is invalid"))?
            .with_timezone(&chrono::Utc),
        after_data_cell_id: DataCellId::new(payload.data_cell_id).map_err(validation)?,
    })
}

fn encode_event_cursor(cursor: DataCellEventCursor) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&EventCursorPayload {
        occurred_at: cursor.after_occurred_at.to_rfc3339(),
        event_id: cursor.after_event_id,
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{EVENT_CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| AppError::internal(error.to_string()).into())
}

fn decode_event_cursor(cursor: &OpaqueCursor) -> V1Result<DataCellEventCursor> {
    let payload: EventCursorPayload =
        decode_payload(cursor, EVENT_CURSOR_PREFIX, "data-cell event cursor")?;
    if payload.event_id <= 0 {
        return Err(AppError::bad_request("data-cell event cursor is invalid").into());
    }
    Ok(DataCellEventCursor {
        after_occurred_at: chrono::DateTime::parse_from_rfc3339(&payload.occurred_at)
            .map_err(|_| AppError::bad_request("data-cell event cursor is invalid"))?
            .with_timezone(&chrono::Utc),
        after_event_id: payload.event_id,
    })
}

fn decode_payload<T: for<'de> Deserialize<'de>>(
    cursor: &OpaqueCursor,
    prefix: &str,
    label: &str,
) -> V1Result<T> {
    let encoded = cursor
        .as_str()
        .strip_prefix(prefix)
        .ok_or_else(|| AppError::bad_request(format!("{label} is invalid")))?;
    serde_json::from_slice(
        &hex::decode(encoded).map_err(|_| AppError::bad_request(format!("{label} is invalid")))?,
    )
    .map_err(|_| AppError::bad_request(format!("{label} is invalid")).into())
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    AppError::internal(error.to_string()).into()
}
