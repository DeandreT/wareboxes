use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::models::Warehouse;

use crate::auth::CurrentTenant;
use crate::error::AppResult;
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::state::AppState;

const READ_PERMS: &[&str] = &["admin", "wms"];

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<Warehouse>>> {
    user.require_any_permission(&state.db, READ_PERMS).await?;
    let warehouses =
        repo::warehouses::get_warehouses(&state.db, user.tenant.tenant_id, q.show_deleted).await?;
    Ok(Json(warehouses))
}
