use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use wareboxes_application::authorization::{PermissionReadModel, RoleReadModel};
use wareboxes_core::dto::{
    AddDeleteChildRole, AddDeleteRolePermission, AddRole, RoleIdRequest, UpdateRole,
};
use wareboxes_core::models::{Permission, Role};

use crate::auth::CurrentTenant;
use crate::error::AppResult;
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

fn permission_response(permission: PermissionReadModel) -> Permission {
    Permission {
        id: permission.id,
        created: permission.created,
        deleted: permission.deleted,
        name: permission.name,
        description: permission.description,
    }
}

fn role_response(role: RoleReadModel) -> Role {
    Role {
        id: role.id,
        created: role.created,
        deleted: role.deleted,
        name: role.name,
        description: role.description,
        parent_id: role.parent_id,
        self_user_id: role.self_user_id,
        parent_roles: role.parent_roles.into_iter().map(role_response).collect(),
        child_roles: role.child_roles.into_iter().map(role_response).collect(),
        role_permissions: role
            .role_permissions
            .into_iter()
            .map(permission_response)
            .collect(),
    }
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<RoleQuery>,
) -> AppResult<Json<Vec<Role>>> {
    user.require_permission(&state.db, PERM).await?;
    let roles = wareboxes_persistence_postgres::roles::get_roles(
        &state.db,
        user.tenant.tenant_id,
        q.show_deleted,
        q.show_self,
    )
    .await?
    .into_iter()
    .map(role_response)
    .collect();
    Ok(Json(roles))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddRole>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = wareboxes_persistence_postgres::roles::add_role(
        &state.db,
        user.tenant.tenant_id,
        &body.name,
        body.description.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<UpdateRole>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::update_role(
        &state.db,
        user.tenant.tenant_id,
        body.role_id,
        body.name.as_deref(),
        body.description.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<RoleIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::set_role_deleted(
        &state.db,
        user.tenant.tenant_id,
        body.role_id,
        true,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<RoleIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::set_role_deleted(
        &state.db,
        user.tenant.tenant_id,
        body.role_id,
        false,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn add_child(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddDeleteChildRole>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::add_role_relationship(
        &state.db,
        user.tenant.tenant_id,
        body.role_id,
        body.child_role_id,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn remove_child(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddDeleteChildRole>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::delete_role_relationship(
        &state.db,
        user.tenant.tenant_id,
        body.child_role_id,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn add_permission(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddDeleteRolePermission>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::add_role_permission(
        &state.db,
        user.tenant.tenant_id,
        body.role_id,
        body.permission_id,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn remove_permission(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddDeleteRolePermission>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::roles::delete_role_permission(
        &state.db,
        user.tenant.tenant_id,
        body.role_id,
        body.permission_id,
    )
    .await?;
    Ok(Json(ok))
}
