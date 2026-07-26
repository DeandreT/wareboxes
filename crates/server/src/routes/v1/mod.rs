//! Version 1 public HTTP routes.

mod error;
mod inventory_balances;

use axum::middleware;
use axum::routing::get;
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/inventory/balances", get(inventory_balances::list))
        .layer(middleware::map_response(error::normalize_error_response))
}
