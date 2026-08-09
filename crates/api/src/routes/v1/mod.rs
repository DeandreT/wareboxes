//! Version 1 public HTTP routes.

mod backorders;
mod cycle_count;
mod error;
mod expected_receiving;
mod facility_shipping_origins;
pub(crate) mod inventory_balances;
mod inventory_holds;
mod inventory_relocation;
mod inventory_rollups;
mod inventory_status_transitions;
mod license_plate_putaway;
mod order_allocations;
mod order_cancellations;
mod order_holds;
mod order_releases;
mod orders;
pub(crate) mod outbound_loads;
mod outbound_qa;
pub(crate) mod packing;
mod pick_shortages;
pub(crate) mod pick_waves;
mod picking;
mod putaway;
mod putaway_claim_lifecycle;
mod putaway_claims;
pub(crate) mod replenishment;
mod rf_sessions;
mod shipping;
pub(crate) mod shipping_queue;

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
        .route(
            "/facilities/{facility_id}/shipping-origin-configurations",
            post(facility_shipping_origins::configure),
        )
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
        .route("/orders/{order_id}/amendments", post(orders::amend))
        .route(
            "/orders/{order_id}/line-amendments",
            post(orders::replace_lines),
        )
        .route("/orders/{order_id}/shipments", post(shipping::create))
        .route("/shipments/{shipment_id}", get(shipping::get))
        .route(
            "/shipments/{shipment_id}/documents",
            get(shipping::list_documents),
        )
        .route(
            "/shipments/{shipment_id}/documents/packing-slips",
            post(shipping::generate_packing_slip),
        )
        .route(
            "/shipments/{shipment_id}/documents/carton-label-sets",
            post(shipping::generate_carton_label_set),
        )
        .route(
            "/shipment-documents/{document_id}/content",
            get(shipping::download_document),
        )
        .route(
            "/shipments/{shipment_id}/manifests",
            post(shipping::record_manifest),
        )
        .route(
            "/shipments/{shipment_id}/departures",
            post(shipping::confirm_departure),
        )
        .route("/shipping-queue", get(shipping_queue::queue))
        .route("/outbound-qa-policies", post(outbound_qa::configure_policy))
        .route(
            "/packing-sessions/{session_id}/outbound-qa-sessions",
            post(outbound_qa::start),
        )
        .route("/outbound-qa-sessions/{session_id}", get(outbound_qa::get))
        .route(
            "/outbound-qa-sessions/{session_id}/carton-verifications",
            post(outbound_qa::verify_carton),
        )
        .route(
            "/outbound-qa-sessions/{session_id}/completions",
            post(outbound_qa::complete),
        )
        .route(
            "/outbound-loads",
            get(outbound_loads::list).post(outbound_loads::plan),
        )
        .route(
            "/outbound-loads/by-barcode/{load_barcode}",
            get(outbound_loads::get_by_barcode),
        )
        .route("/outbound-loads/{load_id}", get(outbound_loads::get))
        .route(
            "/outbound-loads/{load_id}/releases",
            post(outbound_loads::release),
        )
        .route(
            "/outbound-loads/{load_id}/loading-starts",
            post(outbound_loads::start_loading),
        )
        .route(
            "/outbound-loads/{load_id}/loading-completions",
            post(outbound_loads::complete_loading),
        )
        .route(
            "/outbound-loads/{load_id}/departures",
            post(outbound_loads::depart),
        )
        .route(
            "/outbound-loads/{load_id}/cancellations",
            post(outbound_loads::cancel),
        )
        .route(
            "/outbound-loads/{load_id}/cartons/{carton_id}/staging-movements",
            post(outbound_loads::stage),
        )
        .route(
            "/outbound-loads/{load_id}/cartons/{carton_id}/loading-movements",
            post(outbound_loads::load_carton),
        )
        .route(
            "/outbound-loads/{load_id}/cartons/{carton_id}/unloading-movements",
            post(outbound_loads::unload),
        )
        .route(
            "/outbound-loads/{load_id}/cartons/{carton_id}/unstaging-movements",
            post(outbound_loads::unstage),
        )
        .route(
            "/packed-cartons/{carton_id}/position",
            get(outbound_loads::position),
        )
        .route("/packing-queue", get(packing::queue))
        .route(
            "/orders/{order_id}/allocation-runs",
            post(order_allocations::plan),
        )
        .route(
            "/backorder-policies",
            get(backorders::get_policy).post(backorders::configure_policy),
        )
        .route(
            "/orders/{order_id}/backorder-splits",
            post(backorders::split_shortage),
        )
        .route(
            "/orders/{order_id}/allocation-readiness",
            get(order_allocations::readiness),
        )
        .route(
            "/orders/{order_id}/cancellations",
            post(order_cancellations::create),
        )
        .route("/orders/{order_id}/releases", post(order_releases::create))
        .route(
            "/orders/{order_id}/packing-session",
            get(packing::for_order),
        )
        .route("/orders/{order_id}/packing-sessions", post(packing::open))
        .route("/packing-sessions/{session_id}", get(packing::get))
        .route(
            "/packing-sessions/{session_id}/cartons",
            post(packing::create_carton),
        )
        .route(
            "/packing-sessions/{session_id}/cartons/{carton_id}/contents",
            post(packing::pack_content),
        )
        .route(
            "/packing-sessions/{session_id}/cartons/{carton_id}/closures",
            post(packing::close_carton),
        )
        .route(
            "/packing-sessions/{session_id}/cartons/{carton_id}/voids",
            post(packing::void_carton),
        )
        .route("/picking-claims/next", post(picking::claim_next))
        .route("/picking-claims/current", get(picking::current))
        .route("/picking-claims/{task_id}", post(picking::claim_by_id))
        .route(
            "/picking-claims/{task_id}/heartbeats",
            post(picking::heartbeat),
        )
        .route("/picking-claims/{task_id}/releases", post(picking::release))
        .route(
            "/picking-tasks/{task_id}/contents/{content_id}/confirmations",
            post(picking::confirm),
        )
        .route(
            "/pick-confirmations/{confirmation_id}/reversals",
            post(picking::reverse_confirmation),
        )
        .route(
            "/orders/{order_id}/pick-confirmations",
            get(picking::list_confirmation_history),
        )
        .route(
            "/picking-tasks/{task_id}/contents/{content_id}/short-picks",
            post(pick_shortages::report),
        )
        .route("/pick-waves", get(pick_waves::list).post(pick_waves::plan))
        .route("/pick-waves/{wave_id}", get(pick_waves::get))
        .route("/pick-waves/{wave_id}/releases", post(pick_waves::release))
        .route(
            "/pick-waves/{wave_id}/cancellations",
            post(pick_waves::cancel),
        )
        .route("/pick-shortages", get(pick_shortages::list))
        .route("/pick-shortages/{shortage_id}", get(pick_shortages::get))
        .route(
            "/pick-shortages/{shortage_id}/reallocations",
            post(pick_shortages::reallocate),
        )
        .route(
            "/pick-shortages/{shortage_id}/short-ship-dispositions",
            post(pick_shortages::accept_short_shipment),
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
        .route(
            "/replenishment-policies",
            get(replenishment::policy_page).post(replenishment::configure_policy),
        )
        .route(
            "/replenishment-policies/{policy_id}/retirements",
            post(replenishment::retire_policy),
        )
        .route(
            "/replenishment-policies/{policy_id}/plan-runs",
            post(replenishment::plan_policy),
        )
        .route("/replenishment-queue", get(replenishment::work_page))
        .route(
            "/replenishment-claims/next",
            post(replenishment::claim_next),
        )
        .route(
            "/replenishment-claims/current",
            get(replenishment::current_claim),
        )
        .route(
            "/replenishment-claims/{work_id}",
            post(replenishment::claim_by_id),
        )
        .route(
            "/replenishment-claims/{work_id}/heartbeats",
            post(replenishment::heartbeat_claim),
        )
        .route(
            "/replenishment-claims/{work_id}/releases",
            post(replenishment::release_claim),
        )
        .route(
            "/replenishment-tasks/{work_id}/confirmations",
            post(replenishment::confirm_work),
        )
        .route(
            "/replenishment-tasks/{work_id}/cancellations",
            post(replenishment::cancel_work),
        )
        .layer(middleware::map_response(error::normalize_error_response))
}
