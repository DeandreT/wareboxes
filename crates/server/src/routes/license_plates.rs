use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_core::dto::MoveLicensePlate;
use wareboxes_core::dto::{AddLicensePlate, LicensePlateIdRequest, LicensePlateUpdate};
use wareboxes_core::models::LicensePlate;
use wareboxes_domain::FacilityId;

use crate::auth::CurrentTenant;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";

async fn require_scoped_location(
    db: &Db,
    user: &CurrentTenant,
    location_id: i64,
) -> AppResult<FacilityId> {
    let facility_id = repo::locations::active_location_facility_in_scope(
        db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        location_id,
    )
    .await?
    .ok_or_else(|| AppError::bad_request("Destination location not found"))?;
    if repo::locations::location_active_state(db, user.tenant.tenant_id, location_id).await?
        != Some(true)
    {
        return Err(AppError::bad_request("Destination location is inactive"));
    }
    FacilityId::new(facility_id).map_err(|error| AppError::internal(error.to_string()))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<LicensePlate>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::license_plates::get_license_plates_in_scope(&state.db, &user.tenant, q.show_deleted)
            .await?,
    ))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddLicensePlate>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_inventory_owner(body.inventory_owner_id)?;
    user.require_facility(body.facility_id)?;
    if !repo::inventory_owners::active_inventory_owner_exists_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.owner_scope,
        body.inventory_owner_id,
    )
    .await?
    {
        return Err(AppError::bad_request("Inventory owner not found"));
    }
    if !repo::facilities::active_facility_exists_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        body.facility_id,
    )
    .await?
    {
        return Err(AppError::bad_request("Facility not found"));
    }
    let id = repo::license_plates::add_license_plate(
        &state.db,
        user.tenant.tenant_id,
        body.inventory_owner_id,
        body.facility_id,
        body.barcode.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn get_by_barcode(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(barcode): Path<String>,
) -> AppResult<Json<Option<LicensePlate>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::license_plates::get_license_plate_by_barcode_in_scope(
            &state.db,
            &user.tenant,
            &barcode,
        )
        .await?,
    ))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LicensePlateUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::license_plate_dimensions(&state.db, &user.tenant, body.license_plate_id, false)
        .await?
        .is_none()
    {
        return Ok(Json(false));
    }
    Ok(Json(
        repo::license_plates::update_license_plate(
            &state.db,
            user.tenant.tenant_id,
            body.license_plate_id,
            body.barcode.as_deref(),
        )
        .await?,
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LicensePlateIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::license_plate_dimensions(&state.db, &user.tenant, body.license_plate_id, false)
        .await?
        .is_none()
    {
        return Ok(Json(false));
    }
    Ok(Json(
        repo::license_plates::set_license_plate_deleted(
            &state.db,
            user.tenant.tenant_id,
            body.license_plate_id,
            true,
        )
        .await?,
    ))
}

pub async fn move_plate(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<MoveLicensePlate>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let source = repo::access::license_plate_dimensions(
        &state.db,
        &user.tenant,
        body.license_plate_id,
        false,
    )
    .await?
    .ok_or_else(|| AppError::not_found("license plate"))?;
    let destination_facility =
        require_scoped_location(&state.db, &user, body.to_location_id).await?;
    if source.facility_id != destination_facility {
        return Err(AppError::conflict(
            "license plate moves cannot cross facilities; use an inventory transfer workflow",
        ));
    }
    Ok(Json(
        repo::license_plates::move_license_plate(
            &state.db,
            user.tenant.tenant_id,
            user.user.id,
            body.license_plate_id,
            body.to_location_id,
            body.reason.as_deref(),
            Some(&body.idempotency_key),
        )
        .await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LicensePlateIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::license_plate_dimensions(&state.db, &user.tenant, body.license_plate_id, true)
        .await?
        .is_none()
    {
        return Ok(Json(false));
    }
    Ok(Json(
        repo::license_plates::set_license_plate_deleted(
            &state.db,
            user.tenant.tenant_id,
            body.license_plate_id,
            false,
        )
        .await?,
    ))
}
