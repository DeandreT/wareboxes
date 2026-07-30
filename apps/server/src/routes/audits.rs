use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_core::dto::{
    AddAuditLocationCount, AddAuditWave, AuditLocationCountIdRequest, AuditLocationCountUpdate,
    AuditWaveIdRequest, AuditWaveUpdate,
};
use wareboxes_core::models::{AuditLocationCount, AuditWave};

use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "admin";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<AuditWave>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::audits::get_audit_waves(&state.db, &user.tenant, q.show_deleted).await?,
    ))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddAuditWave>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_facility(body.facility_id)?;
    user.require_inventory_owner(body.inventory_owner_id)?;
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
    let id = repo::audits::add_audit_wave(
        &state.db,
        &user.tenant,
        user.user.id,
        body.facility_id,
        body.inventory_owner_id,
        &body.name,
        body.description.as_deref(),
    )
    .await?
    .ok_or_else(|| AppError::bad_request("Inventory owner is not assigned to the facility"))?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AuditWaveUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::audits::update_audit_wave(
        &state.db,
        &user.tenant,
        body.audit_wave_id,
        body.name.as_deref(),
        body.description.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AuditWaveIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::audits::set_audit_wave_deleted(&state.db, &user.tenant, body.audit_wave_id, true)
            .await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AuditWaveIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::audits::set_audit_wave_deleted(&state.db, &user.tenant, body.audit_wave_id, false)
            .await?,
    ))
}

pub async fn counts(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(audit_id): Path<i64>,
) -> AppResult<Json<Vec<AuditLocationCount>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::audits::get_location_counts(&state.db, &user.tenant, audit_id).await?,
    ))
}

pub async fn add_count(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddAuditLocationCount>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::audits::add_location_count(&state.db, &user.tenant, &body)
        .await?
        .ok_or_else(|| AppError::not_found("audit wave, location, or owner item"))?;
    Ok(Json(id))
}

pub async fn update_count(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AuditLocationCountUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::audits::update_location_count(&state.db, &user.tenant, &body).await?,
    ))
}

pub async fn delete_count(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AuditLocationCountIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::audits::set_location_count_deleted(
            &state.db,
            &user.tenant,
            body.audit_location_count_id,
            body.expected_revision,
            true,
        )
        .await?,
    ))
}

pub async fn restore_count(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AuditLocationCountIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::audits::set_location_count_deleted(
            &state.db,
            &user.tenant,
            body.audit_location_count_id,
            body.expected_revision,
            false,
        )
        .await?,
    ))
}
