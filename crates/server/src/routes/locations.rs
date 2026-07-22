use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{AddLocation, LocationIdRequest, LocationUpdate};
use wareboxes_core::models::Location;

use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<Location>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::locations::get_locations(&state.db, user.tenant.tenant_id, q.show_deleted).await?,
    ))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddLocation>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if !repo::facilities::active_facility_exists(&state.db, user.tenant.tenant_id, body.facility_id)
        .await?
    {
        return Err(AppError::bad_request("Facility not found"));
    }
    if let Some(parent_location_id) = body.parent_location_id {
        if !repo::locations::active_location_exists(
            &state.db,
            user.tenant.tenant_id,
            parent_location_id,
        )
        .await?
        {
            return Err(AppError::bad_request("Parent location not found"));
        }
    }
    let id = repo::locations::add_location(
        &state.db,
        user.tenant.tenant_id,
        body.facility_id,
        body.parent_location_id,
        body.barcode.as_deref(),
        body.name.as_deref(),
        &body.r#type,
        body.active.unwrap_or(true),
        body.pickable.unwrap_or(false),
        body.receivable.unwrap_or(false),
    )
    .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LocationUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::locations::update_location(
        &state.db,
        user.tenant.tenant_id,
        body.location_id,
        body.parent_location_id,
        body.barcode.as_deref(),
        body.name.as_deref(),
        body.r#type.as_deref(),
        body.active,
        body.pickable,
        body.receivable,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LocationIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::locations::set_location_deleted(
            &state.db,
            user.tenant.tenant_id,
            body.location_id,
            true,
        )
        .await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LocationIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::locations::set_location_deleted(
            &state.db,
            user.tenant.tenant_id,
            body.location_id,
            false,
        )
        .await?,
    ))
}
