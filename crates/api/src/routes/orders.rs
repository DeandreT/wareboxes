use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use wareboxes_core::dto::{OrderIdRequest, OrderPage};
use wareboxes_core::models::{Order, OrderStatus};

use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "orders";
const DEFAULT_ORDER_LIMIT: i64 = 500;
const MAX_ORDER_LIMIT: i64 = 2500;

#[derive(Debug, Deserialize)]
pub struct OrderListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub status: Option<OrderStatus>,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<OrderListQuery>,
) -> AppResult<Json<OrderPage>> {
    user.require_permission(&state.db, PERM).await?;
    let limit = q
        .limit
        .unwrap_or(DEFAULT_ORDER_LIMIT)
        .clamp(1, MAX_ORDER_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let orders = repo::orders::get_orders_page_in_scope(
        &state.db,
        &user.tenant,
        limit,
        offset,
        q.status,
        q.search.as_deref(),
    )
    .await?;
    Ok(Json(orders))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(order_id): Path<i64>,
) -> AppResult<Json<Option<Order>>> {
    user.require_permission(&state.db, PERM).await?;
    let order = repo::orders::get_order_in_scope(&state.db, &user.tenant, order_id).await?;
    Ok(Json(order))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<OrderIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if !repo::access::order_is_accessible(&state.db, &user.tenant, body.order_id, false).await? {
        return Err(AppError::conflict(
            "order cannot be deleted because it is shipped, confirmed, closed, deleted, or not mutable",
        ));
    }
    let ok = repo::orders::delete_order(&state.db, user.tenant.tenant_id, body.order_id).await?;
    if !ok {
        return Err(AppError::conflict(
            "order cannot be deleted because it is shipped, confirmed, closed, deleted, or not mutable",
        ));
    }
    Ok(Json(ok))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<OrderIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if !repo::access::order_is_accessible(&state.db, &user.tenant, body.order_id, true).await? {
        return Ok(Json(false));
    }
    let ok = repo::orders::restore_order(&state.db, user.tenant.tenant_id, body.order_id).await?;
    Ok(Json(ok))
}
