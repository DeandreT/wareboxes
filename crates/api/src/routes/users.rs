use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use wareboxes_core::dto::{AddDeleteUserRole, UpdateUserAccessScope, UserIdRequest, UserUpdate};
use wareboxes_core::models::User;
use wareboxes_domain::UserId;

use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::identity::tenant_user_response;
use crate::repo;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "admin";

fn requested_user_id(value: i64) -> AppResult<UserId> {
    UserId::new(value).map_err(|_| AppError::bad_request("user ID must be positive"))
}

#[derive(Debug, Deserialize, Default)]
pub struct ShowDeleted {
    #[serde(default)]
    pub show_deleted: bool,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<User>>> {
    user.require_permission(&state.db, PERM).await?;
    let users = wareboxes_persistence_postgres::users::get_tenant_users(
        &state.db,
        user.tenant.tenant_id,
        q.show_deleted,
    )
    .await?
    .into_iter()
    .map(tenant_user_response)
    .collect();
    Ok(Json(users))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<UserUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::users::update_user(
        &state.db,
        user.tenant.tenant_id,
        requested_user_id(body.user_id)?,
        body.first_name.as_deref(),
        body.last_name.as_deref(),
        body.nick_name.as_deref(),
        body.phone.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<UserIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::users::set_user_membership_deleted(
        &state.db,
        user.tenant.tenant_id,
        requested_user_id(body.user_id)?,
        true,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<UserIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::users::set_user_membership_deleted(
        &state.db,
        user.tenant.tenant_id,
        requested_user_id(body.user_id)?,
        false,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn add_role(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddDeleteUserRole>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::add_role_to_user(
        &state.db,
        user.tenant.tenant_id,
        body.user_id,
        body.role_id,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn remove_role(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddDeleteUserRole>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::delete_user_role(
        &state.db,
        user.tenant.tenant_id,
        body.user_id,
        body.role_id,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn update_access_scope(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<UpdateUserAccessScope>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_scope_delegation(&body)?;
    let updated =
        repo::tenants::update_user_access_scope(&state.db, user.tenant.tenant_id, &body).await?;
    Ok(Json(updated))
}
