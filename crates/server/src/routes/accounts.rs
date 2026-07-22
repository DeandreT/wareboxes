use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{AccountIdRequest, AccountUpdate, AddAccount};
use wareboxes_core::models::Account;

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
) -> AppResult<Json<Vec<Account>>> {
    user.require_any_permission(&state.db, READ_PERMS).await?;
    let accounts =
        repo::accounts::get_accounts(&state.db, user.tenant.tenant_id, q.show_deleted).await?;
    Ok(Json(accounts))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddAccount>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::accounts::add_account(&state.db, user.tenant.tenant_id, &body.name, &body.email)
        .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AccountUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::accounts::update_account(
        &state.db,
        user.tenant.tenant_id,
        body.account_id,
        body.name.as_deref(),
        body.email.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AccountIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok =
        repo::accounts::delete_account(&state.db, user.tenant.tenant_id, body.account_id).await?;
    Ok(Json(ok))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AccountIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok =
        repo::accounts::restore_account(&state.db, user.tenant.tenant_id, body.account_id).await?;
    Ok(Json(ok))
}
