use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use wareboxes_core::dto::{
    AddLoad, AddLoadFile, AddLoadLine, AddLoadNote, ArriveLoad, LoadFileIdRequest, LoadIdRequest,
    LoadNoteIdRequest, LoadUpdate, ReceiveInboundLine, ReceiveLoadLine,
};
use wareboxes_core::models::{Load, LoadFileCategory, LoadStatus, LoadType};

use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::permissions;
use crate::repo;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";
const DEFAULT_LOAD_LIMIT: i64 = 500;
const MAX_LOAD_LIMIT: i64 = 2_000;

#[derive(Debug, Deserialize)]
pub struct LoadListQuery {
    #[serde(default)]
    pub show_deleted: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<LoadListQuery>,
) -> AppResult<Json<Vec<Load>>> {
    user.require_permission(&state.db, PERM).await?;
    let limit = q
        .limit
        .unwrap_or(DEFAULT_LOAD_LIMIT)
        .clamp(1, MAX_LOAD_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    Ok(Json(
        repo::loads::get_load_summaries(&state.db, q.show_deleted, limit, offset).await?,
    ))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(load_id): Path<i64>,
) -> AppResult<Json<Option<Load>>> {
    user.require_permission(&state.db, PERM).await?;
    let show_deleted_notes =
        permissions::user_has_permission(&state.db, user.user.id, "admin").await?;
    Ok(Json(
        repo::loads::get_load(&state.db, load_id, show_deleted_notes).await?,
    ))
}

pub async fn mobile_inbound_list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<Load>>> {
    user.require_permission(&state.db, PERM).await?;
    let loads = repo::loads::get_load_summaries(&state.db, false, MAX_LOAD_LIMIT, 0)
        .await?
        .into_iter()
        .filter(|load| load.r#type == LoadType::Inbound)
        .collect();
    Ok(Json(loads))
}

pub async fn mobile_inbound_get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(load_id): Path<i64>,
) -> AppResult<Json<Option<Load>>> {
    user.require_permission(&state.db, PERM).await?;
    let load = repo::loads::get_load(&state.db, load_id, false)
        .await?
        .filter(|load| load.r#type == LoadType::Inbound);
    Ok(Json(load))
}

pub async fn mobile_arrive(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(load_id): Path<i64>,
    Json(body): Json<ArriveLoad>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if body
        .arrival
        .is_some_and(|arrival| arrival > chrono::Utc::now())
    {
        return Err(crate::error::AppError::bad_request(
            "arrival time cannot be in the future",
        ));
    }
    let ok = repo::loads::update_load(
        &state.db,
        user.user.id,
        load_id,
        Some(LoadStatus::Arrived),
        None,
        None,
        body.invoice_number.as_deref(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        body.arrival,
        None,
        None,
        None,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn mobile_receive_line(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(load_line_id): Path<i64>,
    Json(body): Json<ReceiveInboundLine>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::loads::receive_line(
        &state.db,
        user.user.id,
        load_line_id,
        body.to_location_id,
        body.received_qty,
        body.rejected_qty,
        body.missing_qty.unwrap_or(0),
        body.license_plate_id,
        body.license_plate_barcode.as_deref(),
        body.lot.as_deref(),
        body.serial.as_deref(),
        body.expiration,
        body.reason.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddLoad>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if !repo::warehouses::active_warehouse_exists(&state.db, body.warehouse_id).await? {
        return Err(AppError::bad_request("Warehouse not found"));
    }
    if !repo::accounts::active_account_exists(&state.db, body.account_id).await? {
        return Err(AppError::bad_request("Account not found"));
    }
    if let Some(location_id) = body.dock_door_location_id {
        match repo::locations::location_active_state(&state.db, location_id).await? {
            None => return Err(AppError::bad_request("Dock door location not found")),
            Some(false) => return Err(AppError::bad_request("Dock door location is inactive")),
            Some(true) => {}
        }
    }
    let id = repo::loads::add_load(
        &state.db,
        user.user.id,
        body.warehouse_id,
        body.account_id,
        body.r#type,
        body.reference_number.as_deref(),
        body.invoice_number.as_deref(),
        body.carrier.as_deref(),
        body.trailer_number.as_deref(),
        body.seal_number.as_deref(),
        body.dock_door_location_id,
        body.expected_time,
        body.appointment_time,
    )
    .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<LoadUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::loads::update_load(
        &state.db,
        user.user.id,
        body.load_id,
        body.status,
        body.r#type,
        body.reference_number.as_deref(),
        body.invoice_number.as_deref(),
        body.carrier.as_deref(),
        body.trailer_number.as_deref(),
        body.seal_number.as_deref(),
        body.dock_door_location_id,
        body.expected_time,
        body.appointment_time,
        body.actual_time,
        body.arrival,
        body.departure,
        body.rejected,
        body.receive_completed,
        body.closed,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<LoadIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::loads::set_load_deleted(&state.db, body.load_id, true).await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<LoadIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::loads::set_load_deleted(&state.db, body.load_id, false).await?,
    ))
}

pub async fn add_note(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddLoadNote>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::loads::add_note(&state.db, user.user.id, body.load_id, &body.note).await?;
    Ok(Json(id))
}

pub async fn delete_note(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<LoadNoteIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::loads::set_load_note_deleted(&state.db, user.user.id, body.load_note_id, true)
            .await?,
    ))
}

pub async fn add_line(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddLoadLine>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::loads::add_line(
        &state.db,
        user.user.id,
        body.load_id,
        body.item_id,
        body.sku_id,
        body.expected_qty,
        body.lot.as_deref(),
        body.serial.as_deref(),
        body.expiration,
    )
    .await?;
    Ok(Json(id))
}

pub async fn receive_line(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ReceiveLoadLine>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::loads::receive_line(
        &state.db,
        user.user.id,
        body.load_line_id,
        body.to_location_id,
        body.received_qty,
        body.rejected_qty,
        body.missing_qty.unwrap_or(0),
        body.license_plate_id,
        body.license_plate_barcode.as_deref(),
        body.lot.as_deref(),
        body.serial.as_deref(),
        body.expiration,
        body.reason.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn add_file(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddLoadFile>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::loads::add_file(
        &state.db,
        body.load_id,
        &body.original_name,
        &body.name,
        &body.path,
        body.content_type.as_deref(),
        body.category.unwrap_or(LoadFileCategory::General),
    )
    .await?;
    Ok(Json(id))
}

pub async fn delete_file(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<LoadFileIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::loads::delete_file(&state.db, body.file_id).await?,
    ))
}
