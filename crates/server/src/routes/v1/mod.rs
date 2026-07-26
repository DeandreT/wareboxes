//! Version 1 public HTTP routes.

mod error;
mod inventory_balances;
mod putaway;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/inventory/balances", get(inventory_balances::list))
        .route("/putaway-tasks", post(putaway::create))
        .route(
            "/putaway-tasks/:task_id/confirmations",
            post(putaway::confirm),
        )
        .layer(middleware::map_response(error::normalize_error_response))
}
