//! Version 1 public HTTP routes.

mod error;
mod expected_receiving;
mod inventory_balances;
mod inventory_holds;
mod inventory_status_transitions;
mod license_plate_putaway;
mod putaway;
mod putaway_claim_lifecycle;
mod putaway_claims;
mod rf_sessions;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/expected-receiving/loads/by-barcode/{execution_barcode}",
            get(expected_receiving::get_session_by_execution_barcode),
        )
        .route(
            "/expected-receiving/loads/{load_id}",
            get(expected_receiving::get_session),
        )
        .route(
            "/expected-receiving/lines/{load_line_id}/confirmations",
            post(expected_receiving::confirm),
        )
        .route("/rf/sessions", post(rf_sessions::create))
        .route("/inventory/balances", get(inventory_balances::list))
        .route(
            "/inventory/holds",
            get(inventory_holds::list).post(inventory_holds::place),
        )
        .route(
            "/inventory/holds/{hold_id}/releases",
            post(inventory_holds::release),
        )
        .route(
            "/inventory/balances/{balance_id}/status-transitions",
            post(inventory_status_transitions::create),
        )
        .route(
            "/license-plate-putaway-tasks",
            post(license_plate_putaway::create),
        )
        .route(
            "/license-plate-putaway-tasks/{task_id}/confirmations",
            post(license_plate_putaway::confirm),
        )
        .route("/putaway-tasks", post(putaway::create))
        .route(
            "/putaway-tasks/{task_id}/confirmations",
            post(putaway::confirm),
        )
        .route("/putaway-claims/next", post(putaway_claims::claim_next))
        .route("/putaway-claims/current", get(putaway_claims::current))
        .route(
            "/putaway-claims/{task_id}",
            post(putaway_claims::claim_by_id),
        )
        .route(
            "/putaway-claims/{task_id}/heartbeats",
            post(putaway_claim_lifecycle::heartbeat),
        )
        .route(
            "/putaway-claims/{task_id}/releases",
            post(putaway_claim_lifecycle::release),
        )
        .layer(middleware::map_response(error::normalize_error_response))
}
