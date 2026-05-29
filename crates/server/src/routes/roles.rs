use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use wareboxes_core::dto::{
    AddDeleteChildRole, AddDeleteRolePermission, AddRole, RoleIdRequest, UpdateRole,
};
use wareboxes_core::models::Role;

use crate::auth::CurrentUser;
use crate::error::AppResult;
use crate::repo;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "admin";

#[derive(Debug, Deserialize, Default)]
pub struct RoleQuery {
    #[serde(default)]
    pub show_deleted: bool,
    #[serde(default)]
    pub show_self: bool,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<RoleQuery>,
) -> AppResult<Json<Vec<Role>>> {
    user.require_permission(&state.db, PERM).await?;
    let roles = repo::roles::get_roles(&state.db, q.show_deleted, q.show_self).await?;
    Ok(Json(roles))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddRole>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::roles::add_role(&state.db, &body.name, body.description.as_deref()).await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<UpdateRole>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::roles::update_role(
        &state.db,
        body.role_id,
        body.name.as_deref(),
        body.description.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<RoleIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::roles::set_role_deleted(&state.db, body.role_id, true).await?;
    Ok(Json(ok))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<RoleIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::roles::set_role_deleted(&state.db, body.role_id, false).await?;
    Ok(Json(ok))
}

pub async fn add_child(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddDeleteChildRole>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok =
        repo::roles::add_role_relationship(&state.db, body.role_id, body.child_role_id).await?;
    Ok(Json(ok))
}

pub async fn remove_child(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddDeleteChildRole>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::roles::delete_role_relationship(&state.db, body.child_role_id).await?;
    Ok(Json(ok))
}

pub async fn add_permission(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddDeleteRolePermission>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::roles::add_role_permission(&state.db, body.role_id, body.permission_id).await?;
    Ok(Json(ok))
}

pub async fn remove_permission(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddDeleteRolePermission>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok =
        repo::roles::delete_role_permission(&state.db, body.role_id, body.permission_id).await?;
    Ok(Json(ok))
}
