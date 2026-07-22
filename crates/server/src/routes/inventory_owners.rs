use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{AddInventoryOwner, InventoryOwnerIdRequest, InventoryOwnerUpdate};
use wareboxes_core::models::InventoryOwner;

use crate::auth::CurrentTenant;
use crate::error::AppResult;
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "admin";
const READ_PERMS: &[&str] = &["admin", "wms"];

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<InventoryOwner>>> {
    user.require_any_permission(&state.db, READ_PERMS).await?;
    let inventory_owners = repo::inventory_owners::get_inventory_owners_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.owner_scope,
        &user.tenant.site_scope,
        q.show_deleted,
    )
    .await?;
    Ok(Json(inventory_owners))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddInventoryOwner>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if !user.tenant.owner_scope.all_inventory_owners {
        return Err(wareboxes_core::CoreError::Forbidden.into());
    }
    let id = repo::inventory_owners::add_inventory_owner(
        &state.db,
        user.tenant.tenant_id,
        &body.name,
        &body.email,
    )
    .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<InventoryOwnerUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_inventory_owner(body.inventory_owner_id)?;
    let ok = repo::inventory_owners::update_inventory_owner_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.owner_scope,
        body.inventory_owner_id,
        body.name.as_deref(),
        body.email.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<InventoryOwnerIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_inventory_owner(body.inventory_owner_id)?;
    let ok = repo::inventory_owners::delete_inventory_owner_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.owner_scope,
        body.inventory_owner_id,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<InventoryOwnerIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_inventory_owner(body.inventory_owner_id)?;
    let ok = repo::inventory_owners::restore_inventory_owner_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.owner_scope,
        body.inventory_owner_id,
    )
    .await?;
    Ok(Json(ok))
}
