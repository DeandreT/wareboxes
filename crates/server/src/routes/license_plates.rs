use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_core::dto::MoveLicensePlate;
use wareboxes_core::dto::{AddLicensePlate, LicensePlateIdRequest, LicensePlateUpdate};
use wareboxes_core::models::LicensePlate;

use crate::auth::{CurrentTenant, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<LicensePlate>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::license_plates::get_license_plates(&state.db, q.show_deleted).await?,
    ))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddLicensePlate>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::license_plates::add_license_plate(&state.db, body.barcode.as_deref()).await?;
    Ok(Json(id))
}

pub async fn get_by_barcode(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(barcode): Path<String>,
) -> AppResult<Json<Option<LicensePlate>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::license_plates::get_license_plate_by_barcode(&state.db, &barcode).await?,
    ))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<LicensePlateUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::license_plates::update_license_plate(
            &state.db,
            body.license_plate_id,
            body.barcode.as_deref(),
        )
        .await?,
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<LicensePlateIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::license_plates::set_license_plate_deleted(&state.db, body.license_plate_id, true)
            .await?,
    ))
}

pub async fn move_plate(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<MoveLicensePlate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    match repo::locations::location_active_state(
        &state.db,
        user.tenant.tenant_id,
        body.to_location_id,
    )
    .await?
    {
        None => return Err(AppError::bad_request("Destination location not found")),
        Some(false) => return Err(AppError::bad_request("Destination location is inactive")),
        Some(true) => {}
    }
    Ok(Json(
        repo::license_plates::move_license_plate(
            &state.db,
            user.user.id,
            body.license_plate_id,
            body.to_location_id,
            body.reason.as_deref(),
            body.idempotency_key.as_deref(),
        )
        .await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<LicensePlateIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::license_plates::set_license_plate_deleted(&state.db, body.license_plate_id, false)
            .await?,
    ))
}
