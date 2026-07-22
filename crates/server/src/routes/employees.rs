use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{AddEmployee, EmployeeIdRequest, EmployeeUpdate};
use wareboxes_core::models::Employee;

use crate::auth::CurrentTenant;
use crate::db::now_iso;
use crate::error::AppResult;
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "admin";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<Employee>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::employees::get_employees(&state.db, user.tenant.tenant_id, q.show_deleted).await?,
    ))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddEmployee>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let hired = body.hired.unwrap_or_else(now_iso);
    let id = repo::employees::add_employee(
        &state.db,
        user.tenant.tenant_id,
        &body.first_name,
        &body.last_name,
        &body.title,
        &body.r#type,
        body.email.as_deref(),
        body.phone.as_deref(),
        hired,
    )
    .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<EmployeeUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let ok = repo::employees::update_employee(
        &state.db,
        user.tenant.tenant_id,
        body.employee_id,
        body.first_name.as_deref(),
        body.last_name.as_deref(),
        body.title.as_deref(),
        body.r#type.as_deref(),
        body.email.as_deref(),
        body.phone.as_deref(),
        body.terminated,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<EmployeeIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::employees::set_employee_deleted(
            &state.db,
            user.tenant.tenant_id,
            body.employee_id,
            true,
        )
        .await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<EmployeeIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::employees::set_employee_deleted(
            &state.db,
            user.tenant.tenant_id,
            body.employee_id,
            false,
        )
        .await?,
    ))
}
