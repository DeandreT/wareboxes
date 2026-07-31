use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{AddPermission, PermissionIdRequest, UpdatePermission};
use wareboxes_core::models::Permission;

use crate::auth::CurrentTenant;
use crate::error::AppResult;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "admin";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<Permission>>> {
    user.require_permission(&state.db, PERM).await?;
    let perms = wareboxes_persistence_postgres::permissions::get_permissions(
        &state.db,
        user.tenant.tenant_id,
        q.show_deleted,
    )
    .await?
    .into_iter()
    .map(|permission| Permission {
        id: permission.id,
        created: permission.created,
        deleted: permission.deleted,
        name: permission.name,
        description: permission.description,
    })
    .collect();
    Ok(Json(perms))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddPermission>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = wareboxes_persistence_postgres::permissions::add_permission(
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
    Json(body): Json<UpdatePermission>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::permissions::update_permission(
        &state.db,
        user.tenant.tenant_id,
        body.permission_id,
        body.name.as_deref(),
        body.description.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<PermissionIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::permissions::set_deleted(
        &state.db,
        user.tenant.tenant_id,
        body.permission_id,
        true,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<PermissionIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = wareboxes_persistence_postgres::permissions::set_deleted(
        &state.db,
        user.tenant.tenant_id,
        body.permission_id,
        false,
    )
    .await?;
    Ok(Json(ok))
}
