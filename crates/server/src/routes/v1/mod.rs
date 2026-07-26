//! Version 1 public HTTP routes.

mod error;
mod inventory_balances;
mod license_plate_putaway;
mod putaway;
mod putaway_claim_lifecycle;
mod putaway_claims;

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
        .route("/putaway-claims/next", post(putaway_claims::claim_next))
        .route("/putaway-claims/current", get(putaway_claims::current))
        .route(
            "/putaway-claims/:task_id",
            post(putaway_claims::claim_by_id),
        )
        .route(
            "/putaway-claims/:task_id/heartbeats",
            post(putaway_claim_lifecycle::heartbeat),
        )
        .route(
            "/putaway-claims/:task_id/releases",
            post(putaway_claim_lifecycle::release),
        )
        .layer(middleware::map_response(error::normalize_error_response))
}
