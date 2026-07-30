use axum::extract::State;
use axum::Json;
use wareboxes_core::dto::{AccessScopeResource, AccessScopeWorkspace};

use crate::auth::CurrentTenant;
use crate::error::AppResult;
use crate::repo;
use crate::state::AppState;

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> AppResult<Json<AccessScopeWorkspace>> {
    let facilities = repo::facilities::get_facilities_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        false,
    )
    .await?
    .into_iter()
    .map(|facility| AccessScopeResource {
        id: facility.id,
        name: facility
            .name
            .unwrap_or_else(|| format!("Facility {}", facility.id)),
    })
    .collect();
    let inventory_owners = repo::inventory_owners::get_inventory_owners_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.owner_scope,
        &user.tenant.site_scope,
        false,
    )
    .await?
    .into_iter()
    .map(|owner| AccessScopeResource {
        id: owner.id,
        name: owner.name,
    })
    .collect();
    Ok(Json(AccessScopeWorkspace {
        facilities,
        inventory_owners,
    }))
}
