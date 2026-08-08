use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::models::Facility;

use crate::auth::CurrentTenant;
use crate::error::AppResult;
use crate::routes::users::ShowDeleted;
use crate::state::AppState;

const READ_PERMS: &[&str] = &["admin", "wms"];

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<Facility>>> {
    user.require_any_permission(&state.db, READ_PERMS).await?;
    let facilities = wareboxes_persistence_postgres::facilities::get_facilities_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        q.show_deleted,
    )
    .await?
    .into_iter()
    .map(|facility| Facility {
        id: facility.id,
        tenant_id: facility.tenant_id,
        created: facility.created,
        deleted: facility.deleted,
        name: facility.name,
        address_id: facility.address_id,
        revision: facility.revision,
    })
    .collect();
    Ok(Json(facilities))
}
