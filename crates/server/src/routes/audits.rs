use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_core::dto::{AddAuditWave, AuditWaveIdRequest, AuditWaveUpdate};
use wareboxes_core::models::{AuditLocationCount, AuditWave};

use crate::auth::CurrentUser;
use crate::error::AppResult;
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "admin";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<AuditWave>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::audits::get_audit_waves(&state.db, q.show_deleted).await?,
    ))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddAuditWave>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id =
        repo::audits::add_audit_wave(&state.db, &body.name, body.description.as_deref()).await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AuditWaveUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::audits::update_audit_wave(
        &state.db,
        body.audit_wave_id,
        body.name.as_deref(),
        body.description.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AuditWaveIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::audits::set_audit_wave_deleted(&state.db, body.audit_wave_id, true).await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AuditWaveIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::audits::set_audit_wave_deleted(&state.db, body.audit_wave_id, false).await?,
    ))
}

pub async fn counts(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(audit_id): Path<i64>,
) -> AppResult<Json<Vec<AuditLocationCount>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::audits::get_location_counts(&state.db, audit_id).await?,
    ))
}
