use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{AddPermission, PermissionIdRequest, UpdatePermission};
use wareboxes_core::models::Permission;

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
) -> AppResult<Json<Vec<Permission>>> {
    user.require_permission(&state.db, PERM).await?;
    let perms = repo::permissions::get_permissions(&state.db, q.show_deleted).await?;
    Ok(Json(perms))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddPermission>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::permissions::add_permission(&state.db, &body.name, body.description.as_deref())
        .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<UpdatePermission>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::permissions::update_permission(
        &state.db,
        body.permission_id,
        body.name.as_deref(),
        body.description.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<PermissionIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::permissions::set_deleted(&state.db, body.permission_id, true).await?;
    Ok(Json(ok))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<PermissionIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::permissions::set_deleted(&state.db, body.permission_id, false).await?;
    Ok(Json(ok))
}
