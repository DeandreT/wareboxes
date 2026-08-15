use axum::extract::State;
use axum::Json;
use wareboxes_api_contract::web::access::{
    AccessOwnerFacility, AccessScopeResource, AccessScopeWorkspace,
};

use crate::auth::CurrentTenant;
use crate::error::AppResult;
use crate::repo;
use crate::state::AppState;

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> AppResult<Json<AccessScopeWorkspace>> {
    Ok(Json(workspace_for_access(&state, &user.tenant).await?))
}

pub(crate) async fn workspace_for_access(
    state: &AppState,
    access: &wareboxes_core::models::TenantAccess,
) -> AppResult<AccessScopeWorkspace> {
    let facilities = wareboxes_persistence_postgres::facilities::get_facilities_in_scope(
        &state.db,
        access.tenant_id,
        &access.site_scope,
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
    let scoped_inventory_owners = repo::inventory_owners::get_inventory_owners_in_scope(
        &state.db,
        access.tenant_id,
        &access.owner_scope,
        &access.site_scope,
        false,
    )
    .await?;
    let mut owner_facilities = scoped_inventory_owners
        .iter()
        .flat_map(|owner| {
            owner
                .inventory_owner_facilities
                .iter()
                .map(move |facility| AccessOwnerFacility {
                    inventory_owner_id: owner.id,
                    facility_id: facility.id,
                })
        })
        .collect::<Vec<_>>();
    owner_facilities.sort_unstable_by_key(|link| (link.facility_id, link.inventory_owner_id));
    let inventory_owners = scoped_inventory_owners
        .into_iter()
        .map(|owner| AccessScopeResource {
            id: owner.id,
            name: owner.name,
        })
        .collect();
    Ok(AccessScopeWorkspace {
        facilities,
        inventory_owners,
        owner_facilities,
    })
}
