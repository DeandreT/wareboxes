//! Version 1 public HTTP routes.

mod error;
mod inventory_balances;
mod license_plate_putaway;
mod putaway;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/inventory/balances", get(inventory_balances::list))
        .route(
            "/license-plate-putaway-tasks",
            post(license_plate_putaway::create),
        )
        .route(
            "/license-plate-putaway-tasks/:task_id/confirmations",
            post(license_plate_putaway::confirm),
        )
        .route("/putaway-tasks", post(putaway::create))
        .route(
            "/putaway-tasks/:task_id/confirmations",
            post(putaway::confirm),
        )
        .layer(middleware::map_response(error::normalize_error_response))
}
