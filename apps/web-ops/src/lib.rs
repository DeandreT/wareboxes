#![recursion_limit = "512"]

mod administration;
pub mod api;
pub mod app;
mod app_frame;
mod catalog;
mod components;
mod cross_dock;
mod customer_portal;
mod customer_returns;
mod cycle_count;
pub mod facility_shipping_origin;
mod fulfillment;
mod fulfillment_load_detail;
mod fulfillment_loads;
mod fulfillment_order_allocation;
mod fulfillment_order_cancellation;
mod fulfillment_orders;
mod fulfillment_pick_shortages;
mod fulfillment_shared;
mod inbound_asns;
mod inventory;
mod inventory_disposition;
mod inventory_holds;
mod inventory_integrity;
mod inventory_rollups;
mod labor;
mod orders;
mod outbound_loads;
mod packing;
mod pick_waves;
mod preferences;
mod purchase_orders;
mod putaway;
mod replenishment;
mod service_accounts;
mod shipping;
mod slotting;
mod sorting;
mod tenant_lifecycle;
mod toast;
mod transfer_orders;
mod value_added_work;
mod vendor_returns;
mod view_model;
mod work_orchestration;
mod workspace_layout;
mod yard;

#[cfg(all(feature = "hydrate", target_arch = "wasm32"))]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::App;

    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
