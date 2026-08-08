use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{AddLocation, LocationIdRequest, LocationUpdate};
use wareboxes_core::models::Location;

use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";
const READ_PERMS: &[&str] = &["admin", "wms", "orders"];

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<Location>>> {
    user.require_any_permission(&state.db, READ_PERMS).await?;
    Ok(Json(
        list_for_access(&state, &user.tenant, q.show_deleted).await?,
    ))
}

pub(crate) async fn list_for_access(
    state: &AppState,
    access: &wareboxes_core::models::TenantAccess,
    show_deleted: bool,
) -> AppResult<Vec<Location>> {
    let locations = wareboxes_persistence_postgres::locations::get_locations_in_scope(
        &state.db,
        access.tenant_id,
        &access.site_scope,
        show_deleted,
    )
    .await?
    .into_iter()
    .map(|location| Location {
        id: location.id,
        tenant_id: location.tenant_id,
        created: location.created,
        deleted: location.deleted,
        facility_id: location.facility_id,
        facility_name: location.facility_name,
        parent_location_id: location.parent_location_id,
        barcode: location.barcode,
        name: location.name,
        r#type: location.r#type,
        active: location.active,
        pickable: location.pickable,
        receivable: location.receivable,
    })
    .collect();
    Ok(locations)
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddLocation>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_facility(body.facility_id)?;
    if !wareboxes_persistence_postgres::facilities::active_facility_exists_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        body.facility_id,
    )
    .await?
    {
        return Err(AppError::bad_request("Facility not found"));
    }
    if let Some(parent_location_id) = body.parent_location_id {
        if !wareboxes_persistence_postgres::locations::active_location_exists_in_facility(
            &state.db,
            user.tenant.tenant_id,
            body.facility_id,
            parent_location_id,
        )
        .await?
        {
            return Err(AppError::bad_request("Parent location not found"));
        }
    }
    let id = wareboxes_persistence_postgres::locations::add_location(
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
    let Some(facility_id) =
        wareboxes_persistence_postgres::locations::active_location_facility_in_scope(
            &state.db,
            user.tenant.tenant_id,
            &user.tenant.site_scope,
            body.location_id,
        )
        .await?
    else {
        return Ok(Json(false));
    };
    if let Some(parent_location_id) = body.parent_location_id {
        if !wareboxes_persistence_postgres::locations::active_location_exists_in_facility(
            &state.db,
            user.tenant.tenant_id,
            facility_id,
            parent_location_id,
        )
        .await?
        {
            return Err(AppError::bad_request("Parent location not found"));
        }
    }
    let ok = wareboxes_persistence_postgres::locations::update_location_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
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
        wareboxes_persistence_postgres::locations::set_location_deleted_in_scope(
            &state.db,
            user.tenant.tenant_id,
            &user.tenant.site_scope,
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
        wareboxes_persistence_postgres::locations::set_location_deleted_in_scope(
            &state.db,
            user.tenant.tenant_id,
            &user.tenant.site_scope,
            body.location_id,
            false,
        )
        .await?,
    ))
}
