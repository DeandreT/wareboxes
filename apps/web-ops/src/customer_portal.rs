use std::collections::BTreeMap;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CustomerPortalDocumentType, CustomerPortalOrderStatus, CustomerPortalShipmentStatus,
    CustomerPortalWorkspaceResponse,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::components::{Icon, UiIcon};

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "the browser build consumes unauthorized callbacks"
    )
)]
struct Signals {
    workspace: RwSignal<CustomerPortalWorkspaceResponse>,
    scope_options: RwSignal<PortalScopeOptions>,
    owner_filter: RwSignal<Option<i64>>,
    facility_filter: RwSignal<Option<i64>>,
    search_draft: RwSignal<String>,
    search: RwSignal<Option<String>>,
    include_history: RwSignal<bool>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    on_unauthorized: Callback<()>,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct PortalScopeOptions {
    owners: BTreeMap<i64, String>,
    facilities: BTreeMap<i64, String>,
}

impl From<AccessScopeWorkspace> for PortalScopeOptions {
    fn from(access: AccessScopeWorkspace) -> Self {
        Self {
            owners: access
                .inventory_owners
                .into_iter()
                .map(|owner| (owner.id, owner.name))
                .collect(),
            facilities: access
                .facilities
                .into_iter()
                .map(|facility| (facility.id, facility.name))
                .collect(),
        }
    }
}

#[component]
pub(crate) fn CustomerPortal(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let signals = Signals {
        workspace: RwSignal::new(empty_workspace()),
        scope_options: RwSignal::new(access.into()),
        owner_filter: RwSignal::new(None),
        facility_filter: RwSignal::new(None),
        search_draft: RwSignal::new(String::new()),
        search: RwSignal::new(None),
        include_history: RwSignal::new(false),
        loading: RwSignal::new(true),
        error: RwSignal::new(None),
        generation: RwSignal::new(0),
        on_unauthorized,
    };
    Effect::new(move || {
        let _ = (
            signals.owner_filter.get(),
            signals.facility_filter.get(),
            signals.search.get(),
            signals.include_history.get(),
        );
        load(signals);
    });
    let apply_search = move |_| {
        signals
            .search
            .set(text_filter(&signals.search_draft.get_untracked()).map(ToOwned::to_owned))
    };
    let report_path = move || api::customer_portal_inventory_report_path(&filters(signals));

    view! {
        <section class="customer-portal" aria-busy=move || signals.loading.get()>
            <header class="portal-toolbar">
                <div class="portal-heading">
                    <span class="portal-heading-icon"><Icon icon=UiIcon::Clients/></span>
                    <div>
                        <h1>"Client portal"</h1>
                        <span>"Inventory, orders, shipments, and documents"</span>
                    </div>
                </div>
                <div class="portal-filters" role="group" aria-label="Portal filters">
                    <label class="portal-field">
                        <span>"Client"</span>
                        <select prop:value=move ||option_value(signals.owner_filter.get()) on:change=move |event|signals.owner_filter.set(parse_id(&event_target_value(&event)))>
                            <option value="">"All clients"</option>
                            {move ||owner_options(signals.scope_options.get())}
                        </select>
                    </label>
                    <label class="portal-field">
                        <span>"Facility"</span>
                        <select prop:value=move ||option_value(signals.facility_filter.get()) on:change=move |event|signals.facility_filter.set(parse_id(&event_target_value(&event)))>
                            <option value="">"All facilities"</option>
                            {move ||facility_options(signals.scope_options.get())}
                        </select>
                    </label>
                    <form class="portal-search" on:submit=move |event|{event.prevent_default();apply_search(())}>
                        <label class="portal-field">
                            <span>"Search"</span>
                            <input type="search" placeholder="Order, item, lot, or tracking" prop:value=move ||signals.search_draft.get() on:input=move |event|signals.search_draft.set(event_target_value(&event))/>
                        </label>
                        <button class="button secondary-action compact" type="submit"><Icon icon=UiIcon::Search/>"Apply"</button>
                    </form>
                    <label class="portal-history">
                        <input type="checkbox" prop:checked=move ||signals.include_history.get() on:change=move |event|signals.include_history.set(event_target_checked(&event))/>
                        <span>"Include history"</span>
                    </label>
                </div>
                <div class="portal-actions">
                    <a class="button secondary-action compact" href=report_path><Icon icon=UiIcon::Download/>"Inventory CSV"</a>
                    <button class="icon-button" type="button" title="Refresh portal" aria-label="Refresh portal" disabled=move ||signals.loading.get() on:click=move |_|load(signals)><Icon icon=UiIcon::Refresh/></button>
                    <Show when=move || signals.loading.get() && !signals.workspace.get().generated_at.is_empty()>
                        <span class="sr-only" role="status">"Refreshing client portal data"</span>
                    </Show>
                </div>
            </header>
            {move ||if signals.loading.get()&&signals.workspace.get().generated_at.is_empty(){view!{<div class="portal-state" role="status"><span class="loading-line"></span><h2>"Loading customer visibility"</h2></div>}.into_any()}else if let Some(message)=signals.error.get(){view!{<div class="portal-state error" role="alert"><h2>"Portal data unavailable"</h2><p>{message}</p><button class="button secondary-action" type="button" on:click=move |_|load(signals)>"Try again"</button></div>}.into_any()}else{portal_body(signals).into_any()}}
        </section>
    }
}

fn portal_body(signals: Signals) -> AnyView {
    view! {
        <div class="portal-body">
            {move ||metrics(signals.workspace.get())}
            <div class="portal-grid">
                <section class="portal-panel inventory-panel"><header><div><h2>"Inventory availability"</h2><span>{move ||format!("{} grouped positions",signals.workspace.get().inventory.len())}</span></div></header><div class="portal-table-scroll"><table><caption class="sr-only">"Inventory availability in the current client portal view"</caption><thead><tr><th>"Item"</th><th>"Lot / expiry"</th><th>"Client / facility"</th><th>"Status"</th><th class="numeric">"On hand"</th><th class="numeric">"Committed"</th><th class="numeric">"Available"</th></tr></thead><tbody>{move ||signals.workspace.get().inventory.into_iter().map(|line|view!{<tr><td><strong>{line.primary_sku.unwrap_or_else(||format!("Item #{}",line.item_id))}</strong><small>{line.item_description.unwrap_or_else(||"No description".into())}</small></td><td>{line.lot.unwrap_or_else(||"Untracked".into())}<small>{line.expiration.as_deref().map(short_timestamp).unwrap_or_else(||"No expiry".into())}</small></td><td>{line.inventory_owner_name}<small>{line.facility_name}</small></td><td><span class="status-badge neutral">{title_case(&line.status)}</span></td><td class="numeric">{line.on_hand}</td><td class="numeric">{line.reserved+line.held}</td><td class="numeric"><strong>{line.available}</strong></td></tr>}).collect_view()}</tbody></table>{move ||signals.workspace.get().inventory.is_empty().then(||view!{<p class="portal-empty">"No inventory matches the current scope and filters."</p>})}</div></section>
                <div class="portal-columns">
                    <section class="portal-panel"><header><div><h2>"Order status"</h2><span>{move ||format!("{} orders",signals.workspace.get().orders.len())}</span></div></header><div class="portal-table-scroll"><table><caption class="sr-only">"Orders in the current client portal view"</caption><thead><tr><th>"Order"</th><th>"State"</th><th>"Destination"</th><th>"Facility"</th><th class="numeric">"Units"</th><th>"Ship by"</th></tr></thead><tbody>{move ||signals.workspace.get().orders.into_iter().map(|order|view!{<tr><td><strong>{order.order_key}</strong><small>{order.inventory_owner_name}</small></td><td><span class=order_status_class(order.status)>{order_status(order.status)}</span></td><td>{order.destination_company.unwrap_or_else(||"Recipient".into())}<small>{destination(&order.destination_city,&order.destination_region,&order.destination_country)}</small></td><td>{order.facility_name.unwrap_or_else(||"Not assigned".into())}</td><td class="numeric">{order.ordered_quantity}</td><td>{order.ship_by.as_deref().map(short_timestamp).unwrap_or_else(||"Not scheduled".into())}</td></tr>}).collect_view()}</tbody></table>{move ||signals.workspace.get().orders.is_empty().then(||view!{<p class="portal-empty">"No orders match the current view."</p>})}</div></section>
                    <div class="portal-side-stack">
                        <section class="portal-panel"><header><div><h2>"Shipments"</h2><span>{move ||format!("{} shipments",signals.workspace.get().shipments.len())}</span></div></header><div class="portal-table-scroll"><table><caption class="sr-only">"Shipments in the current client portal view"</caption><thead><tr><th>"Shipment"</th><th>"State"</th><th>"Carrier"</th><th>"Tracking"</th><th class="numeric">"Cartons"</th><th>"Updated"</th></tr></thead><tbody>{move ||signals.workspace.get().shipments.into_iter().map(|shipment|{let updated=shipment.departed_at.clone().or(shipment.manifested_at.clone()).unwrap_or(shipment.created_at.clone());view!{<tr><td><strong>{format!("Shipment #{}",shipment.shipment_id)}</strong><small>{format!("{} · {}",shipment.order_key,shipment.facility_name)}</small></td><td><span class=shipment_status_class(shipment.status)>{shipment_status(shipment.status)}</span></td><td>{shipment.carrier.unwrap_or_else(||"Not manifested".into())}<small>{shipment.service.unwrap_or_default()}</small></td><td>{if shipment.tracking_numbers.is_empty(){"Pending".into()}else{shipment.tracking_numbers.join(", ")}}</td><td class="numeric">{shipment.carton_count}</td><td>{short_timestamp(&updated)}</td></tr>}}).collect_view()}</tbody></table>{move ||signals.workspace.get().shipments.is_empty().then(||view!{<p class="portal-empty">"No shipments match the current view."</p>})}</div></section>
                        <section class="portal-panel"><header><div><h2>"Shipment documents"</h2><span>{move ||format!("{} files",signals.workspace.get().documents.len())}</span></div></header><div class="portal-documents">{move ||signals.workspace.get().documents.into_iter().map(|document|view!{<article><div class="portal-document-icon"><Icon icon=UiIcon::Print/></div><div><strong>{document.file_name}</strong><span>{format!("{} · {} · {}",document_type(document.document_type),document.order_key,short_timestamp(&document.generated_at))}</span><small>{format!("{} bytes · SHA-256 {}…",document.content_length,&document.content_sha256[..12.min(document.content_sha256.len())])}</small></div><a class="button secondary-action compact" href=document.download_path><Icon icon=UiIcon::Download/>"Download"</a></article>}).collect_view()}<Show when=move ||signals.workspace.get().documents.is_empty()><p class="portal-empty">"No shipment documents are available in this scope."</p></Show></div></section>
                    </div>
                </div>
            </div>
        </div>
    }.into_any()
}

fn metrics(workspace: CustomerPortalWorkspaceResponse) -> AnyView {
    let on_hand: i64 = workspace.inventory.iter().map(|line| line.on_hand).sum();
    let available: i64 = workspace.inventory.iter().map(|line| line.available).sum();
    let active_orders = workspace
        .orders
        .iter()
        .filter(|order| {
            !matches!(
                order.status,
                CustomerPortalOrderStatus::Shipped
                    | CustomerPortalOrderStatus::Cancelled
                    | CustomerPortalOrderStatus::Void
            )
        })
        .count();
    let in_transit = workspace
        .shipments
        .iter()
        .filter(|shipment| {
            matches!(
                shipment.status,
                CustomerPortalShipmentStatus::Manifested
                    | CustomerPortalShipmentStatus::PartiallyDeparted
            )
        })
        .count();
    view!{<section class="portal-metrics" aria-label="Current view totals"><article><span>"On hand"</span><strong>{on_hand}</strong><small>"units in the current view"</small></article><article><span>"Available"</span><strong>{available}</strong><small>"visible after commitments"</small></article><article><span>"Active orders"</span><strong>{active_orders}</strong><small>"in the current view"</small></article><article><span>"In transit"</span><strong>{in_transit}</strong><small>"visible shipments moving"</small></article></section>}.into_any()
}

fn load(signals: Signals) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::customer_portal_workspace(filters(signals)).await {
            Ok(workspace) if signals.generation.get_untracked() == generation => {
                record_scope_options(signals.scope_options, &workspace);
                signals.workspace.set(workspace);
                signals.error.set(None)
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
        if signals.generation.get_untracked() == generation {
            signals.loading.set(false)
        }
    });
}

fn filters(signals: Signals) -> api::CustomerPortalFilters {
    api::CustomerPortalFilters {
        inventory_owner_id: signals.owner_filter.get_untracked(),
        facility_id: signals.facility_filter.get_untracked(),
        search: signals.search.get_untracked(),
        include_history: signals.include_history.get_untracked(),
    }
}
fn empty_workspace() -> CustomerPortalWorkspaceResponse {
    CustomerPortalWorkspaceResponse {
        generated_at: String::new(),
        inventory: Vec::new(),
        orders: Vec::new(),
        shipments: Vec::new(),
        documents: Vec::new(),
        inventory_report_path: String::new(),
    }
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}
fn option_value(value: Option<i64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}
fn text_filter(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}
fn owner_options(options: PortalScopeOptions) -> AnyView {
    options
        .owners
        .into_iter()
        .map(|(id, name)| view! {<option value=id>{name}</option>})
        .collect_view()
        .into_any()
}
fn facility_options(options: PortalScopeOptions) -> AnyView {
    options
        .facilities
        .into_iter()
        .map(|(id, name)| view! {<option value=id>{name}</option>})
        .collect_view()
        .into_any()
}

#[cfg(target_arch = "wasm32")]
fn record_scope_options(
    options: RwSignal<PortalScopeOptions>,
    workspace: &CustomerPortalWorkspaceResponse,
) {
    options.update(|known| {
        for line in &workspace.inventory {
            known
                .owners
                .insert(line.inventory_owner_id, line.inventory_owner_name.clone());
            known
                .facilities
                .insert(line.facility_id, line.facility_name.clone());
        }
        for order in &workspace.orders {
            known
                .owners
                .insert(order.inventory_owner_id, order.inventory_owner_name.clone());
            if let (Some(id), Some(name)) = (order.facility_id, order.facility_name.as_ref()) {
                known.facilities.insert(id, name.clone());
            }
        }
        for shipment in &workspace.shipments {
            known.owners.insert(
                shipment.inventory_owner_id,
                shipment.inventory_owner_name.clone(),
            );
            known
                .facilities
                .insert(shipment.facility_id, shipment.facility_name.clone());
        }
    });
}
fn short_timestamp(value: &str) -> String {
    value
        .replace('T', " ")
        .get(..16)
        .unwrap_or(value)
        .to_owned()
}
fn title_case(value: &str) -> String {
    let mut value = value.replace('_', " ");
    if let Some(first) = value.get_mut(..1) {
        first.make_ascii_uppercase()
    }
    value
}
fn destination(city: &Option<String>, region: &Option<String>, country: &Option<String>) -> String {
    [city.as_deref(), region.as_deref(), country.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ")
}
fn order_status(value: CustomerPortalOrderStatus) -> &'static str {
    match value {
        CustomerPortalOrderStatus::Open => "Open",
        CustomerPortalOrderStatus::Held => "Held",
        CustomerPortalOrderStatus::Processing => "Processing",
        CustomerPortalOrderStatus::AwaitingPacking => "Awaiting packing",
        CustomerPortalOrderStatus::Packing => "Packing",
        CustomerPortalOrderStatus::AwaitingShipment => "Awaiting shipment",
        CustomerPortalOrderStatus::Shipped => "Shipped",
        CustomerPortalOrderStatus::Cancelled => "Cancelled",
        CustomerPortalOrderStatus::Void => "Void",
    }
}
fn order_status_class(value: CustomerPortalOrderStatus) -> &'static str {
    match value {
        CustomerPortalOrderStatus::Shipped => "status-badge success",
        CustomerPortalOrderStatus::Held => "status-badge warning",
        CustomerPortalOrderStatus::Cancelled | CustomerPortalOrderStatus::Void => {
            "status-badge neutral"
        }
        _ => "status-badge info",
    }
}
fn shipment_status(value: CustomerPortalShipmentStatus) -> &'static str {
    match value {
        CustomerPortalShipmentStatus::AwaitingManifest => "Awaiting manifest",
        CustomerPortalShipmentStatus::Manifested => "Manifested",
        CustomerPortalShipmentStatus::PartiallyDeparted => "Partially departed",
        CustomerPortalShipmentStatus::Departed => "Departed",
        CustomerPortalShipmentStatus::Cancelled => "Cancelled",
    }
}
fn shipment_status_class(value: CustomerPortalShipmentStatus) -> &'static str {
    match value {
        CustomerPortalShipmentStatus::Departed => "status-badge success",
        CustomerPortalShipmentStatus::Cancelled => "status-badge neutral",
        CustomerPortalShipmentStatus::PartiallyDeparted => "status-badge warning",
        _ => "status-badge info",
    }
}
fn document_type(value: CustomerPortalDocumentType) -> &'static str {
    match value {
        CustomerPortalDocumentType::PackingSlip => "Packing slip",
        CustomerPortalDocumentType::CartonLabelSet => "Carton labels",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::web::access::AccessScopeResource;

    #[test]
    fn status_and_search_labels_are_customer_safe() {
        assert_eq!(
            order_status(CustomerPortalOrderStatus::AwaitingShipment),
            "Awaiting shipment"
        );
        assert_eq!(
            shipment_status(CustomerPortalShipmentStatus::PartiallyDeparted),
            "Partially departed"
        );
        assert_eq!(text_filter("  SO-1 "), Some("SO-1"));
    }

    #[test]
    fn filter_options_come_from_authorized_scope_not_filtered_rows() {
        let options = PortalScopeOptions::from(AccessScopeWorkspace {
            facilities: vec![AccessScopeResource {
                id: 7,
                name: "Reno DC".into(),
            }],
            inventory_owners: vec![AccessScopeResource {
                id: 11,
                name: "Northstar".into(),
            }],
            owner_facilities: Vec::new(),
        });

        assert_eq!(
            options.facilities.get(&7).map(String::as_str),
            Some("Reno DC")
        );
        assert_eq!(
            options.owners.get(&11).map(String::as_str),
            Some("Northstar")
        );
    }
}
