//! Version 1 public HTTP routes.

mod cycle_count;
mod error;
mod expected_receiving;
pub(crate) mod inventory_balances;
mod inventory_holds;
mod inventory_relocation;
mod inventory_rollups;
mod inventory_status_transitions;
mod license_plate_putaway;
mod order_allocations;
mod order_holds;
mod orders;
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
        .route("/cycle-count-claims/next", post(cycle_count::claim_next))
        .route("/cycle-count-claims/current", get(cycle_count::current))
        .route(
            "/cycle-count-claims/{task_id}",
            post(cycle_count::claim_by_id),
        )
        .route(
            "/cycle-count-claims/{task_id}/heartbeats",
            post(cycle_count::heartbeat),
        )
        .route(
            "/cycle-count-claims/{task_id}/releases",
            post(cycle_count::release),
        )
        .route(
            "/cycle-count-tasks/{task_id}/confirmations",
            post(cycle_count::confirm),
        )
        .route("/inventory/balances", get(inventory_balances::list))
        .route(
            "/inventory/rollups/by-location",
            get(inventory_rollups::list_by_location),
        )
        .route(
            "/inventory/rollups/by-facility",
            get(inventory_rollups::list_by_facility),
        )
        .route(
            "/inventory/rollups/by-item",
            get(inventory_rollups::list_by_item),
        )
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
            "/inventory-relocation-tasks",
            post(inventory_relocation::create),
        )
        .route(
            "/inventory-relocation-tasks/{task_id}/confirmations",
            post(inventory_relocation::confirm),
        )
        .route(
            "/inventory-relocation-claims/next",
            post(inventory_relocation::claim_next),
        )
        .route(
            "/inventory-relocation-claims/current",
            get(inventory_relocation::current),
        )
        .route(
            "/inventory-relocation-claims/{task_id}",
            post(inventory_relocation::claim_by_id),
        )
        .route(
            "/inventory-relocation-claims/{task_id}/heartbeats",
            post(inventory_relocation::heartbeat),
        )
        .route(
            "/inventory-relocation-claims/{task_id}/releases",
            post(inventory_relocation::release),
        )
        .route(
            "/license-plate-putaway-tasks",
            post(license_plate_putaway::create),
        )
        .route(
            "/license-plate-putaway-tasks/{task_id}/confirmations",
            post(license_plate_putaway::confirm),
        )
        .route("/orders", post(orders::create))
        .route(
            "/orders/{order_id}/allocation-runs",
            post(order_allocations::plan),
        )
        .route(
            "/orders/{order_id}/allocation-readiness",
            get(order_allocations::readiness),
        )
        .route(
            "/inventory-owners/{inventory_owner_id}/order-entry-items",
            get(orders::entry_items),
        )
        .route("/orders/{order_id}/holds", post(order_holds::place))
        .route(
            "/orders/{order_id}/holds/{hold_id}/releases",
            post(order_holds::release),
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
