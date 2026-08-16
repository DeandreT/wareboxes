//! Version 1 public HTTP routes.

mod backorders;
mod billing;
mod configurations;
pub(crate) mod cross_dock;
pub(crate) mod customer_portal;
mod customer_returns;
pub(crate) mod cycle_count;
mod error;
mod expected_receiving;
mod facility_shipping_origins;
mod inbound_asns;
mod inbound_inspections;
mod inbound_loads;
mod integration_mappings;
mod integration_monitor;
pub(crate) mod integration_order_intake;
pub(crate) mod inventory_balances;
mod inventory_holds;
mod inventory_integrity;
mod inventory_recalls;
mod inventory_relocation;
mod inventory_rollups;
mod inventory_status_transitions;
mod item_storage_policies;
mod item_substitutions;
mod item_traceability_policies;
mod labor;
mod license_plate_putaway;
mod license_plates;
mod order_allocations;
mod order_cancellations;
mod order_holds;
mod order_releases;
mod order_streams;
mod orders;
pub(crate) mod outbound_loads;
mod outbound_qa;
pub(crate) mod packing;
mod pick_clusters;
mod pick_shortages;
pub(crate) mod pick_waves;
mod pick_zones;
mod picking;
mod purchase_orders;
pub(crate) mod putaway;
mod putaway_claim_lifecycle;
mod putaway_claims;
pub(crate) mod replenishment;
mod rf_sessions;
mod service_accounts;
mod shipping;
pub(crate) mod shipping_queue;
mod slotting;
mod storage_zones;
pub(crate) mod tenant_lifecycle;
mod transfer_orders;
mod value_added_work;
mod vendor_returns;
mod work_orchestration;
mod workforce_identity;
mod x12_940;
mod yard;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/platform/tenants",
            get(tenant_lifecycle::list).post(tenant_lifecycle::create),
        )
        .route("/platform/tenants/{tenant_id}", get(tenant_lifecycle::get))
        .route(
            "/platform/tenants/{tenant_id}/events",
            get(tenant_lifecycle::events),
        )
        .route(
            "/platform/tenants/{tenant_id}/status-changes",
            post(tenant_lifecycle::change_status),
        )
        .route(
            "/service-accounts",
            get(service_accounts::list).post(service_accounts::create),
        )
        .route(
            "/service-accounts/{service_account_id}",
            get(service_accounts::get),
        )
        .route(
            "/service-accounts/{service_account_id}/events",
            get(service_accounts::events),
        )
        .route(
            "/service-account-options",
            get(service_accounts::options),
        )
        .route(
            "/service-accounts/{service_account_id}/access-changes",
            post(service_accounts::update_access),
        )
        .route(
            "/service-accounts/{service_account_id}/status-changes",
            post(service_accounts::change_status),
        )
        .route(
            "/service-accounts/{service_account_id}/credentials",
            post(service_accounts::issue_credential),
        )
        .route(
            "/service-accounts/{service_account_id}/credentials/{credential_id}/revocations",
            post(service_accounts::revoke_credential),
        )
        .route("/portal/workspace", get(customer_portal::workspace))
        .route(
            "/portal/documents/{document_id}/content",
            get(customer_portal::download_document),
        )
        .route(
            "/portal/reports/inventory.csv",
            get(customer_portal::inventory_report),
        )
        .route("/billing/workspace", get(billing::workspace))
        .route("/labor/workspace", get(labor::workspace))
        .route("/labor/roster", get(labor::roster_candidates))
        .route(
            "/labor/reference-candidates",
            get(labor::reference_candidates),
        )
        .route(
            "/slotting/profiles",
            get(slotting::list_profiles).post(slotting::configure_profile),
        )
        .route("/slotting/runs", post(slotting::run))
        .route(
            "/slotting/recommendations",
            get(slotting::list_recommendations),
        )
        .route(
            "/slotting/recommendations/{recommendation_id}/acceptances",
            post(slotting::accept),
        )
        .route(
            "/slotting/recommendations/{recommendation_id}/dismissals",
            post(slotting::dismiss),
        )
        .route(
            "/work-orchestration/policies",
            get(work_orchestration::list_policies).post(work_orchestration::configure_policy),
        )
        .route(
            "/work-orchestration/signals",
            get(work_orchestration::signals),
        )
        .route(
            "/work-orchestration/signals/congestion",
            post(work_orchestration::record_zone_signal),
        )
        .route(
            "/work-orchestration/signals/resources",
            post(work_orchestration::record_resource_signal),
        )
        .route(
            "/work-orchestration/plans",
            get(work_orchestration::list_plans).post(work_orchestration::generate_plan),
        )
        .route(
            "/work-orchestration/plans/{plan_id}",
            get(work_orchestration::get_plan),
        )
        .route(
            "/work-orchestration/plans/{plan_id}/dispatches",
            post(work_orchestration::activate),
        )
        .route(
            "/work-orchestration/dispatches/{dispatch_id}/cancellations",
            post(work_orchestration::cancel),
        )
        .route(
            "/work-orchestration/workers",
            get(work_orchestration::list_workers),
        )
        .route(
            "/workforce/employees/{employee_id}/identity-links",
            post(workforce_identity::link),
        )
        .route(
            "/workforce/employees/{employee_id}/identity-unlinks",
            post(workforce_identity::unlink),
        )
        .route(
            "/license-plates/{license_plate_id}/hierarchy",
            get(license_plates::hierarchy),
        )
        .route(
            "/license-plates/{license_plate_id}/parent-changes",
            post(license_plates::change_parent),
        )
        .route("/labor/skills", post(labor::configure_skill))
        .route("/labor/certifications", post(labor::certify_employee))
        .route(
            "/labor/certifications/{certification_id}/revocations",
            post(labor::revoke_certification),
        )
        .route(
            "/labor/equipment-classes",
            post(labor::configure_equipment_class),
        )
        .route(
            "/labor/equipment-assets",
            post(labor::create_equipment_asset),
        )
        .route(
            "/labor/equipment-assets/{equipment_asset_id}/status-changes",
            post(labor::change_equipment_status),
        )
        .route("/labor/standards", post(labor::configure_standard))
        .route("/labor/attendance", post(labor::clock_in))
        .route(
            "/labor/attendance/{attendance_interval_id}/clock-outs",
            post(labor::clock_out),
        )
        .route(
            "/labor/attendance/{attendance_interval_id}/corrections",
            post(labor::correct_attendance),
        )
        .route("/labor/activities", post(labor::start_activity))
        .route(
            "/labor/activities/{labor_activity_id}/completions",
            post(labor::complete_activity),
        )
        .route(
            "/labor/activities/{labor_activity_id}/cancellations",
            post(labor::cancel_activity),
        )
        .route(
            "/labor/activities/{labor_activity_id}/corrections",
            post(labor::correct_activity),
        )
        .route("/yard/workspace", get(yard::workspace))
        .route(
            "/value-added-work",
            get(value_added_work::list).post(value_added_work::create),
        )
        .route(
            "/value-added-work/{work_id}",
            get(value_added_work::get),
        )
        .route(
            "/value-added-work/{work_id}/releases",
            post(value_added_work::release),
        )
        .route(
            "/value-added-work/{work_id}/completions",
            post(value_added_work::complete),
        )
        .route(
            "/value-added-work/{work_id}/cancellations",
            post(value_added_work::cancel),
        )
        .route(
            "/vendor-returns",
            get(vendor_returns::list).post(vendor_returns::create),
        )
        .route(
            "/vendor-returns/{vendor_return_id}",
            get(vendor_returns::get),
        )
        .route(
            "/vendor-returns/{vendor_return_id}/releases",
            post(vendor_returns::release),
        )
        .route(
            "/vendor-returns/{vendor_return_id}/shipments",
            post(vendor_returns::ship),
        )
        .route(
            "/vendor-returns/{vendor_return_id}/cancellations",
            post(vendor_returns::cancel),
        )
        .route("/yard/locations", post(yard::configure_location))
        .route("/yard/assets", post(yard::register_asset))
        .route("/yard/appointments", post(yard::create_appointment))
        .route(
            "/yard/appointments/{appointment_id}/cancellations",
            post(yard::cancel_appointment),
        )
        .route(
            "/yard/appointments/{appointment_id}/no-shows",
            post(yard::mark_no_show),
        )
        .route("/yard/visits", post(yard::gate_in))
        .route("/yard/visits/{visit_id}/spot-moves", post(yard::spot_visit))
        .route(
            "/yard/visits/{visit_id}/door-assignments",
            post(yard::assign_door),
        )
        .route(
            "/yard/visits/{visit_id}/operation-starts",
            post(yard::start_operation),
        )
        .route(
            "/yard/visits/{visit_id}/operation-completions",
            post(yard::complete_operation),
        )
        .route(
            "/yard/visits/{visit_id}/rejections",
            post(yard::reject_visit),
        )
        .route(
            "/yard/visits/{visit_id}/gate-outs",
            post(yard::gate_out),
        )
        .route("/billing/contracts", post(billing::create_contract))
        .route(
            "/billing/contracts/{contract_id}/activations",
            post(billing::activate_contract),
        )
        .route(
            "/billing/contracts/{contract_id}/closures",
            post(billing::close_contract),
        )
        .route(
            "/billing/contracts/{contract_id}/rates",
            post(billing::configure_rate),
        )
        .route(
            "/billing/contracts/{contract_id}/billable-events",
            post(billing::capture_event),
        )
        .route(
            "/billing/contracts/{contract_id}/storage-snapshots",
            post(billing::capture_snapshot),
        )
        .route(
            "/billing/contracts/{contract_id}/reconciliation-runs",
            post(billing::generate_run),
        )
        .route(
            "/billing/reconciliation-runs/{run_id}/reviews",
            post(billing::review_run),
        )
        .route(
            "/billing/reconciliation-runs/{run_id}/exports",
            post(billing::export_run),
        )
        .route(
            "/configurations",
            get(configurations::list).post(configurations::create),
        )
        .route(
            "/configurations/{configuration_id}/submissions",
            post(configurations::submit),
        )
        .route(
            "/configurations/{configuration_id}/approvals",
            post(configurations::approve),
        )
        .route(
            "/configurations/{configuration_id}/activations",
            post(configurations::activate),
        )
        .route(
            "/configurations/{configuration_id}/retirements",
            post(configurations::retire),
        )
        .route(
            "/configurations/{configuration_id}/rollbacks",
            post(configurations::rollback),
        )
        .route("/configuration-simulations", post(configurations::simulate))
        .route("/inbound-asns", get(inbound_asns::list).post(inbound_asns::create))
        .route("/inbound-asns/{asn_id}", get(inbound_asns::get))
        .route(
            "/inbound-asns/{asn_id}/cancellations",
            post(inbound_asns::cancel),
        )
        .route(
            "/inbound-asns/{asn_id}/load-plans",
            post(inbound_asns::plan_load),
        )
        .route(
            "/customer-returns",
            get(customer_returns::list).post(customer_returns::create),
        )
        .route(
            "/customer-returns/{customer_return_id}",
            get(customer_returns::get),
        )
        .route(
            "/customer-returns/{customer_return_id}/load-plans",
            post(customer_returns::plan_load),
        )
        .route(
            "/customer-returns/{customer_return_id}/cancellations",
            post(customer_returns::cancel),
        )
        .route(
            "/orders/{order_id}/cross-dock-tasks",
            post(cross_dock::plan_work),
        )
        .route("/cross-dock-queue", get(cross_dock::work_page))
        .route(
            "/cross-dock-planning-options",
            get(cross_dock::planning_options),
        )
        .route("/cross-dock-claims/next", post(cross_dock::claim_next))
        .route("/cross-dock-claims/current", get(cross_dock::current_claim))
        .route(
            "/cross-dock-claims/{work_id}",
            post(cross_dock::claim_by_id),
        )
        .route(
            "/cross-dock-claims/{work_id}/heartbeats",
            post(cross_dock::heartbeat_claim),
        )
        .route(
            "/cross-dock-claims/{work_id}/releases",
            post(cross_dock::release_claim),
        )
        .route(
            "/cross-dock-tasks/{work_id}/confirmations",
            post(cross_dock::confirm_work),
        )
        .route(
            "/cross-dock-tasks/{work_id}/cancellations",
            post(cross_dock::cancel_work),
        )
        .route(
            "/purchase-orders",
            get(purchase_orders::list).post(purchase_orders::create),
        )
        .route("/purchase-orders/{purchase_order_id}", get(purchase_orders::get))
        .route(
            "/purchase-orders/{purchase_order_id}/releases",
            post(purchase_orders::release),
        )
        .route(
            "/purchase-orders/{purchase_order_id}/cancellations",
            post(purchase_orders::cancel),
        )
        .route(
            "/purchase-orders/{purchase_order_id}/asns",
            post(purchase_orders::create_asn),
        )
        .route(
            "/transfer-orders",
            get(transfer_orders::list).post(transfer_orders::create),
        )
        .route("/transfer-orders/{transfer_order_id}", get(transfer_orders::get))
        .route(
            "/transfer-orders/{transfer_order_id}/releases",
            post(transfer_orders::release),
        )
        .route(
            "/transfer-orders/{transfer_order_id}/cancellations",
            post(transfer_orders::cancel),
        )
        .route(
            "/transfer-orders/{transfer_order_id}/execution-readiness",
            get(transfer_orders::execution_readiness),
        )
        .route(
            "/transfer-orders/{transfer_order_id}/dispatches",
            post(transfer_orders::dispatch),
        )
        .route(
            "/transfer-orders/{transfer_order_id}/receipts",
            post(transfer_orders::receive),
        )
        .route("/inbound-loads", post(inbound_loads::plan))
        .route(
            "/inbound-loads/{load_id}/appointments",
            post(inbound_loads::schedule),
        )
        .route(
            "/inbound-loads/{load_id}/appointment-reschedules",
            post(inbound_loads::reschedule_appointment),
        )
        .route(
            "/inbound-loads/{load_id}/cancellations",
            post(inbound_loads::cancel),
        )
        .route(
            "/inbound-loads/{load_id}/rejections",
            post(inbound_loads::reject),
        )
        .route(
            "/inbound-loads/{load_id}/arrivals",
            post(inbound_loads::arrive),
        )
        .route(
            "/inbound-loads/{load_id}/unloading-starts",
            post(inbound_loads::start_unloading),
        )
        .route(
            "/inbound-loads/{load_id}/closures",
            post(inbound_loads::close),
        )
        .route(
            "/inventory-owners/{inventory_owner_id}/inbound-load-entry-items",
            get(inbound_loads::entry_items),
        )
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
        .route(
            "/expected-receiving/loads/{load_id}/unexpected-receipts",
            post(expected_receiving::confirm_unexpected),
        )
        .route("/rf/sessions", post(rf_sessions::create))
        .route(
            "/facilities/{facility_id}/shipping-origin-configurations",
            post(facility_shipping_origins::configure),
        )
        .route("/cycle-count-candidates", get(cycle_count::candidates))
        .route(
            "/cycle-count-policies",
            get(cycle_count::policies).post(cycle_count::configure_policy),
        )
        .route("/cycle-count-variances", get(cycle_count::variances))
        .route(
            "/cycle-count-variances/{variance_id}/decisions",
            post(cycle_count::decide_variance),
        )
        .route(
            "/cycle-count-tasks",
            get(cycle_count::list).post(cycle_count::create),
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
            "/integration-monitor/inbound",
            get(integration_monitor::inbound),
        )
        .route(
            "/integration-monitor/inbound/{receipt_id}",
            get(integration_monitor::inbound_detail),
        )
        .route(
            "/integration-monitor/inbound/{receipt_id}/payload",
            get(integration_monitor::download_inbound_payload),
        )
        .route(
            "/integrations/order-intake/{source_key}/inventory-owners/{external_inventory_owner_key}/orders",
            post(integration_order_intake::receive_order),
        )
        .route(
            "/integrations/x12-940/{source_key}/inventory-owners/{external_inventory_owner_key}/orders",
            post(integration_order_intake::receive_x12_940_order),
        )
        .route(
            "/integration-monitor/inbound/{receipt_id}/reprocessings",
            post(integration_order_intake::reprocess_order),
        )
        .route(
            "/integration-monitor/inbound/{receipt_id}/corrections",
            post(integration_order_intake::correct_order),
        )
        .route(
            "/integration-order-item-mappings",
            get(integration_mappings::list).post(integration_mappings::configure),
        )
        .route(
            "/integration-order-item-mappings/{mapping_id}/retirements",
            post(integration_mappings::retire),
        )
        .route(
            "/integration-order-owner-mappings",
            get(integration_mappings::list_owners).post(integration_mappings::configure_owner),
        )
        .route(
            "/integration-order-owner-mappings/{mapping_id}/retirements",
            post(integration_mappings::retire_owner),
        )
        .route(
            "/integration-monitor/outbound",
            get(integration_monitor::outbound),
        )
        .route(
            "/integration-monitor/outbound/{event_id}",
            get(integration_monitor::outbound_detail),
        )
        .route(
            "/integration-monitor/outbound/{event_id}/replays",
            post(integration_monitor::replay_outbound_dead_letter),
        )
        .route(
            "/integration-monitor/outbound/{event_id}/discards",
            post(integration_monitor::discard_outbound_dead_letter),
        )
        .route("/inventory/journal", get(inventory_integrity::journal))
        .route("/inventory/aging", get(inventory_integrity::aging))
        .route(
            "/inventory/recalls",
            get(inventory_recalls::list).post(inventory_recalls::create),
        )
        .route(
            "/inventory/recalls/{recall_id}/releases",
            post(inventory_recalls::release),
        )
        .route(
            "/inventory/integrity-issues",
            get(inventory_integrity::issues),
        )
        .route(
            "/inventory/reconciliation/status",
            get(inventory_integrity::reconciliation_status),
        )
        .route(
            "/storage-zones",
            get(storage_zones::list).post(storage_zones::configure),
        )
        .route(
            "/storage-zones/{storage_zone_id}/retirements",
            post(storage_zones::retire),
        )
        .route(
            "/item-storage-policies",
            get(item_storage_policies::list).post(item_storage_policies::configure),
        )
        .route(
            "/item-storage-policies/{policy_id}/retirements",
            post(item_storage_policies::retire),
        )
        .route(
            "/item-traceability-policies",
            get(item_traceability_policies::list).post(item_traceability_policies::configure),
        )
        .route(
            "/item-traceability-policies/{policy_id}/retirements",
            post(item_traceability_policies::retire),
        )
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
            "/inbound-inspections/{hold_id}/dispositions",
            post(inbound_inspections::dispose),
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
            "/shipments/{shipment_id}/cancellations",
            post(shipping::cancel),
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
            "/outbound-qa-sessions/{session_id}/cancellations",
            post(outbound_qa::cancel),
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
            "/item-substitution-policies",
            get(item_substitutions::list).post(item_substitutions::configure),
        )
        .route(
            "/item-substitution-policies/{policy_id}/retirements",
            post(item_substitutions::retire),
        )
        .route(
            "/pick-shortages/{shortage_id}/substitutions",
            post(item_substitutions::substitute_shortage),
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
        .route("/orders/{order_id}/streams", post(order_streams::create))
        .route(
            "/orders/{order_id}/packing-session",
            get(packing::for_order),
        )
        .route("/orders/{order_id}/packing-sessions", post(packing::open))
        .route("/packing-sessions/{session_id}", get(packing::get))
        .route(
            "/packing-sessions/{session_id}/abandonments",
            post(packing::abandon_session),
        )
        .route(
            "/packing-sessions/{session_id}/cartons",
            post(packing::create_carton),
        )
        .route(
            "/packing-sessions/{session_id}/cartons/{carton_id}/contents",
            post(packing::pack_content),
        )
        .route(
            "/packing-sessions/{session_id}/cartons/{carton_id}/contents/{content_id}/removals",
            post(packing::remove_content),
        )
        .route(
            "/packing-sessions/{session_id}/cartons/{carton_id}/closures",
            post(packing::close_carton),
        )
        .route(
            "/packing-sessions/{session_id}/cartons/{carton_id}/reopenings",
            post(packing::reopen_carton),
        )
        .route(
            "/packing-sessions/{session_id}/cartons/{carton_id}/voids",
            post(packing::void_carton),
        )
        .route("/picking-claims/next", post(picking::claim_next))
        .route("/picking-claims/current", get(picking::current))
        .route(
            "/pick-clusters/workspace",
            get(pick_clusters::workspace),
        )
        .route("/pick-carts", post(pick_clusters::create_cart))
        .route(
            "/pick-carts/{cart_id}/status-changes",
            post(pick_clusters::change_cart_status),
        )
        .route("/pick-clusters", post(pick_clusters::plan))
        .route(
            "/pick-clusters/{cluster_id}/claims/next",
            post(pick_clusters::claim_next),
        )
        .route(
            "/pick-clusters/{cluster_id}/cancellations",
            post(pick_clusters::cancel),
        )
        .route("/pick-zones/workspace", get(pick_zones::workspace))
        .route(
            "/pick-zones/{storage_zone_id}/claims/next",
            post(pick_zones::claim_next),
        )
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
        .route(
            "/pick-waves/policy-resolutions",
            post(pick_waves::resolve_policies),
        )
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
        .route("/putaway-candidates", get(putaway::candidates))
        .route("/putaway-tasks", get(putaway::list).post(putaway::create))
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
