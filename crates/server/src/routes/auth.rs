use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use wareboxes_core::dto::{LoginRequest, RegisterRequest, SessionUser, UserSettings};
use wareboxes_core::models::{TenantAccess, User};

use crate::auth::{self, CurrentTenant, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::routes::validate;
use crate::state::AppState;

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> AppResult<Json<SessionUser>> {
    validate(&body)?;
    let user = auth::verify_credentials(&state.db, &body.email, &body.password)
        .await?
        .ok_or_else(AppError::unauthorized)?;
    let active_tenant = crate::repo::tenants::default_for_user(&state.db, user.id)
        .await?
        .ok_or_else(AppError::forbidden)?;
    let token = auth::create_session(&state.db, user.id).await?;
    let settings = crate::repo::settings::get_user_settings(&state.db, user.id).await?;
    Ok(Json(SessionUser {
        token,
        user,
        active_tenant,
        settings,
    }))
}

pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> AppResult<Json<SessionUser>> {
    if !state.security.allow_public_registration {
        return Err(AppError::forbidden());
    }
    validate(&body)?;
    let user = auth::register_user(
        &state.db,
        &body.email,
        &body.password,
        body.first_name.as_deref(),
        body.last_name.as_deref(),
    )
    .await?;
    let active_tenant = crate::repo::tenants::default_for_user(&state.db, user.id)
        .await?
        .ok_or_else(|| AppError::internal("registered user has no active tenant"))?;
    let token = auth::create_session(&state.db, user.id).await?;
    let settings = crate::repo::settings::get_user_settings(&state.db, user.id).await?;
    Ok(Json(SessionUser {
        token,
        user,
        active_tenant,
        settings,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    _user: CurrentUser,
) -> AppResult<Json<()>> {
    if let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        auth::destroy_session(&state.db, token).await?;
    }
    Ok(Json(()))
}

pub async fn me(user: CurrentUser) -> AppResult<Json<User>> {
    Ok(Json(user.user))
}

pub async fn tenants(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<TenantAccess>>> {
    Ok(Json(
        crate::repo::tenants::list_for_user(&state.db, user.user.id).await?,
    ))
}

pub async fn context(tenant: CurrentTenant) -> AppResult<Json<TenantAccess>> {
    Ok(Json(tenant.tenant))
}

pub async fn update_settings(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<UserSettings>,
) -> AppResult<Json<UserSettings>> {
    validate(&body)?;
    Ok(Json(
        crate::repo::settings::upsert_user_settings(&state.db, user.user.id, &body).await?,
    ))
}
