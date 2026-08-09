use leptos::html;
use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    PackAllocationDispositionResponse, PackCartonLifecycleResponse, PackSessionResponse,
    PackingQueueOrderStatus,
};
use wareboxes_api_contract::web::access::AccessScopeResource;
use wareboxes_core::models::Location;

use crate::components::{Icon, UiIcon};
use crate::view_model::format_quantity;

use super::removal::PendingContentRemoval;
use super::{
    CartonMeasurementSignals, IdentityScanStage, PackingLocation, PackingQueueSignals,
    PackingSignals, PendingItemIdentity,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceSummary {
    barcode: String,
    location: String,
    allocation_count: i64,
    quantity: i64,
    packed_count: i64,
}

#[component]
pub(super) fn PackingIdle(
    queue: PackingQueueSignals,
    scan: RwSignal<String>,
    scan_input: NodeRef<html::Input>,
    selected_location_id: RwSignal<String>,
    station_locations: StoredValue<Vec<PackingLocation>>,
    signals: PackingSignals,
    start_order: Callback<i64>,
    on_retry: Callback<()>,
    on_load_more: Callback<()>,
    submit_idle: impl Fn(leptos::ev::SubmitEvent) + 'static,
) -> impl IntoView {
    let visible_orders = move || {
        let selected_facility_id = selected_location(station_locations, selected_location_id)
            .map(|location| location.facility_id);
        queue
            .entries
            .get()
            .into_iter()
            .filter(|order| Some(order.facility_id) == selected_facility_id)
            .collect::<Vec<_>>()
    };
    view! {
        <div class="packing-idle">
            <form class="packing-idle-main" on:submit=submit_idle>
                <header>
                    <h2>"Start or resume packing"</h2>
                    <p>"Scan the order at the station or select it from the ready queue."</p>
                </header>
                <div class="packing-order-scan">
                    <label>
                        <span class="sr-only">"Order number"</span>
                        <Icon icon=UiIcon::Scan/>
                        <input
                            node_ref=scan_input
                            autofocus
                            autocomplete="off"
                            placeholder="SCAN ORDER"
                            prop:value=move || scan.get()
                            disabled=move || signals.blocked()
                            on:input=move |event| scan.set(event_target_value(&event))
                        />
                    </label>
                    <button
                        class="button primary-action"
                        type="submit"
                        disabled=move || signals.blocked() || selected_location_id.get().is_empty()
                    >
                        {move || if signals.pending.get() { "Opening" } else { "Open order" }}
                    </button>
                </div>
                <CommandStatus signals idle=true/>
                <Show when=move || signals.retry.get().is_some() || signals.refresh_order_id.get().is_some()>
                    <button
                        class="button primary-action packing-idle-retry"
                        type="button"
                        disabled=move || signals.pending.get()
                        on:click=move |_| on_retry.run(())
                    >
                        {move || if signals.refresh_order_id.get().is_some() { "Refresh session" } else { "Retry saved command" }}
                    </button>
                </Show>
                <Show when=move || station_locations.get_value().is_empty()>
                    <p class="inline-command-error" role="alert">
                        "No active, barcoded packing location is available in your facility scope."
                    </p>
                </Show>
            </form>
            <aside class="packing-idle-queue">
                <header>
                    <h3>"Ready queue"</h3>
                    <span>{move || format!("{} orders", visible_orders().len())}</span>
                </header>
                <div class="packing-order-queue">
                    {move || visible_orders()
                        .into_iter()
                        .map(|order| {
                            let order_id = order.order_id;
                            let order_key = order.order_key;
                            let client = order.inventory_owner_name;
                            let ship_by = order
                                .ship_by
                                .map_or_else(|| "No ship-by".to_owned(), |value| compact_time(&value));
                            let status = match order.status {
                                PackingQueueOrderStatus::AwaitingPacking => "Ready",
                                PackingQueueOrderStatus::Packing => "In progress",
                            };
                            view! {
                                <button
                                    type="button"
                                    class="packing-order-choice"
                                    disabled=move || {
                                        signals.blocked()
                                            || signals.completed_order_ids.get().contains(&order_id)
                                    }
                                    on:click=move |_| start_order.run(order_id)
                                >
                                    <span>
                                        <strong>{order_key}</strong>
                                        <small>{format!("{client} - {ship_by}")}</small>
                                    </span>
                                    <span>{status}</span>
                                </button>
                            }
                        })
                        .collect_view()}
                    <Show when=move || visible_orders().is_empty()>
                        <p class="packing-empty">"No orders are waiting at this facility."</p>
                    </Show>
                </div>
                <div class="packing-queue-footer">
                    <Show when=move || queue.error.get().is_some()>
                        <span role="alert">{move || queue.error.get().unwrap_or_default()}</span>
                    </Show>
                    <Show when=move || queue.next_cursor.get().is_some()>
                        <button
                            class="button secondary-action"
                            type="button"
                            disabled=move || queue.pending.get() || signals.blocked()
                            on:click=move |_| on_load_more.run(())
                        >
                            {move || if queue.pending.get() { "Loading" } else { "Load more" }}
                        </button>
                    </Show>
                </div>
            </aside>
        </div>
    }
}

#[component]
pub(super) fn PackingActive(
    current: PackSessionResponse,
    scan: RwSignal<String>,
    scan_input: NodeRef<html::Input>,
    source_plate: RwSignal<Option<String>>,
    signals: PackingSignals,
    on_submit: Callback<()>,
    on_retry: Callback<()>,
    on_change_source: Callback<()>,
    on_close: Callback<()>,
    on_void: Callback<()>,
    on_remove: Callback<PendingContentRemoval>,
    on_next_order: Callback<()>,
) -> impl IntoView {
    let order_id = current.order_id;
    let open_carton = current
        .cartons
        .iter()
        .find(|carton| matches!(carton.lifecycle, PackCartonLifecycleResponse::Open))
        .cloned();
    let ready =
        current.progress.status == wareboxes_api_contract::v1::PackSessionStatus::ReadyToManifest;
    let awaiting_carton_close = current.progress.expected_allocation_count > 0
        && current.progress.packed_allocation_count >= current.progress.expected_allocation_count
        && open_carton.is_some()
        && !ready;
    let prompt = move || {
        scan_prompt(
            signals.session.get(),
            source_plate.get(),
            signals.item_identity.get(),
        )
    };
    let submit_scan = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        on_submit.run(());
    };
    let source_summaries = source_summaries(&current);
    let packed_count = current.progress.packed_allocation_count;
    let expected_count = current.progress.expected_allocation_count;
    let order_key = current.order_key.clone();
    let station_name = current
        .station_location_name
        .clone()
        .unwrap_or_else(|| current.station_location_barcode.clone());
    let station_title = station_name.clone();
    let allocations = current.allocations.clone();
    let session_id = current.session_id;
    let expected_revision = current.revision;
    let removable_carton_id = open_carton.as_ref().map(|carton| carton.carton_id);
    let removable_carton_barcode = open_carton
        .as_ref()
        .map(|carton| carton.carton_barcode.clone());
    let cartons = current.cartons.clone();
    let active_carton_for_form = StoredValue::new(open_carton.clone());
    let has_active_carton = active_carton_for_form.get_value().is_some();
    let active_carton_for_summary = open_carton.clone();

    view! {
        <div class="packing-active">
            <form class="packing-command-bar" on:submit=submit_scan>
                <label class="packing-scan-field">
                    <span class="sr-only">{prompt}</span>
                    <Icon icon=UiIcon::Scan/>
                    <input
                        node_ref=scan_input
                        autofocus
                        autocomplete="off"
                        placeholder=prompt
                        prop:value=move || scan.get()
                        disabled=move || signals.blocked() || ready || awaiting_carton_close
                        on:input=move |event| scan.set(event_target_value(&event))
                    />
                </label>
                <CommandStatus signals idle=false/>
                <Show
                    when=move || signals.retry.get().is_some() || signals.refresh_order_id.get().is_some()
                    fallback=move || view! {
                        <button
                            class="button secondary-action packing-retry-action"
                            type="button"
                            disabled=move || source_plate.get().is_none() || signals.blocked()
                            on:click=move |_| on_change_source.run(())
                        >
                            "Change tote"
                        </button>
                    }
                >
                    <button
                        class="button primary-action packing-retry-action"
                        type="button"
                        disabled=move || signals.pending.get()
                        on:click=move |_| on_retry.run(())
                    >
                        {move || if signals.refresh_order_id.get().is_some() { "Refresh" } else { "Retry" }}
                    </button>
                </Show>
            </form>
            <div class="packing-layout">
                <section class="packing-panel">
                    <header class="packing-panel-header">
                        <h2>"Order"</h2>
                        <span>{format!("#{order_id}")}</span>
                    </header>
                    <dl class="packing-facts">
                        <div><dt>"Order"</dt><dd>{order_key}</dd></div>
                        <div><dt>"Revision"</dt><dd>{current.revision.get()}</dd></div>
                        <div><dt>"Station"</dt><dd title=station_title>{station_name}</dd></div>
                        <div><dt>"Progress"</dt><dd>{format!("{packed_count}/{expected_count}")}</dd></div>
                    </dl>
                    <header class="packing-panel-header">
                        <h2>"Source totes"</h2>
                        <span>{source_summaries.len()}</span>
                    </header>
                    <div class="packing-source-list">
                        {source_summaries
                            .into_iter()
                            .map(|source| {
                                let source_barcode = source.barcode.clone();
                                view! {
                                    <div
                                        class="packing-source-row"
                                        class:active-row=move || source_plate.get().as_deref()
                                            == Some(source_barcode.as_str())
                                    >
                                        <strong>{source.barcode}</strong>
                                        <small>{format!(
                                            "{} - {}/{} allocations - {} units",
                                            source.location,
                                            source.packed_count,
                                            source.allocation_count,
                                            format_quantity(source.quantity)
                                        )}</small>
                                    </div>
                                }
                            })
                            .collect_view()}
                    </div>
                </section>

                <section class="packing-panel">
                    <header class="packing-panel-header">
                        <h2>"Items"</h2>
                        <span>{format!(
                            "{} / {} units packed",
                            format_quantity(current.progress.packed_quantity),
                            format_quantity(current.progress.expected_quantity)
                        )}</span>
                    </header>
                    <div class="packing-items-scroll">
                        <table class="data-table packing-items-table">
                            <thead><tr>
                                <th>"Item"</th><th>"Description"</th><th>"Lot / serial"</th>
                                <th>"UOM"</th><th class="numeric">"Qty"</th><th>"State"</th>
                            </tr></thead>
                            <tbody>
                                {allocations
                                    .into_iter()
                                    .map(|allocation| {
                                        let packed = matches!(&allocation.disposition, PackAllocationDispositionResponse::Packed { .. });
                                        let removable_content = match &allocation.disposition {
                                            PackAllocationDispositionResponse::Packed { content_id, carton_id, .. }
                                                if Some(*carton_id) == removable_carton_id => {
                                                    removable_carton_barcode.clone().map(|carton_barcode| PendingContentRemoval {
                                                        session_id,
                                                        order_id,
                                                        carton_id: *carton_id,
                                                        carton_barcode,
                                                        content_id: *content_id,
                                                        item_barcodes: allocation.item_barcodes.clone(),
                                                        item_description: allocation.item_description.clone(),
                                                        lot: allocation.lot.clone(),
                                                        serial: allocation.serial.clone(),
                                                        destination_tote_barcode: allocation.picked_tote_license_plate_barcode.clone(),
                                                        quantity: allocation.quantity,
                                                        uom: allocation.uom.clone(),
                                                        expected_revision,
                                                    })
                                                }
                                            _ => None,
                                        };
                                        let trace = allocation.lot.clone().or(allocation.serial.clone())
                                            .unwrap_or_else(|| "-".to_owned());
                                        let item = allocation.item_barcodes.first().cloned()
                                            .unwrap_or_else(|| format!("Item #{}", allocation.item_id));
                                        let description = allocation.item_description
                                            .unwrap_or_else(|| "-".to_owned());
                                        let description_title = description.clone();
                                        view! {
                                            <tr class:packed>
                                                <td><strong>{item}</strong><small class="cell-detail">{format!("Allocation #{}", allocation.inventory_allocation_id)}</small></td>
                                                <td title=description_title>{description}</td>
                                                <td>{trace}</td>
                                                <td>{allocation.uom}</td>
                                                <td class="numeric strong">{format_quantity(allocation.quantity)}</td>
                                                <td>
                                                    <div class="packing-item-state">
                                                        <span class=if packed { "status shipped" } else { "status open" }>{if packed { "Packed" } else { "Ready" }}</span>
                                                        {removable_content.map(|selection| view! {
                                                            <button
                                                                type="button"
                                                                class="icon-button packing-remove-content"
                                                                title="Return content to picked tote"
                                                                aria-label="Return packed content to picked tote"
                                                                disabled=move || signals.blocked()
                                                                on:click=move |_| on_remove.run(selection.clone())
                                                            >
                                                                <Icon icon=UiIcon::Reverse/>
                                                            </button>
                                                        })}
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </div>
                </section>

                <section class="packing-panel">
                    <header class="packing-panel-header">
                        <h2>"Cartons"</h2>
                        <span>{current.cartons.len()}</span>
                    </header>
                    <Show when=move || ready>
                        <div class="packing-carton-close">
                            <strong>"Packing complete"</strong>
                            <button
                                class="button primary-action packing-carton-action"
                                type="button"
                                on:click=move |_| on_next_order.run(())
                            >
                                <Icon icon=UiIcon::Packing/>
                                "Pack next order"
                            </button>
                        </div>
                    </Show>
                    <Show
                        when=move || has_active_carton && !ready
                        fallback=move || {
                            (!ready).then(|| view! {
                                <div class="packing-carton-create">
                                    <strong>"No open carton"</strong>
                                    <span class="packing-empty">"Scan the next carton barcode to open it."</span>
                                </div>
                            })
                        }
                    >
                        {move || active_carton_for_form.get_value().map(|carton| view! {
                            <CloseCartonPanel carton on_close on_void signals/>
                        })}
                    </Show>
                    <div class="packing-carton-list">
                        {cartons
                            .into_iter()
                            .rev()
                            .map(|carton| {
                                let (state, detail) = match carton.lifecycle {
                                    PackCartonLifecycleResponse::Open => (
                                        "Open",
                                        format!("{} contents", carton.content_count),
                                    ),
                                    PackCartonLifecycleResponse::Closed { closed_at, .. } => (
                                        "Closed",
                                        format!("{} contents - {}", carton.content_count, compact_time(&closed_at)),
                                    ),
                                    PackCartonLifecycleResponse::Voided { voided_at, .. } => (
                                        "Voided",
                                        format!("Empty - {}", compact_time(&voided_at)),
                                    ),
                                };
                                view! {
                                    <div class="packing-carton-row">
                                        <strong>{carton.carton_barcode}</strong>
                                        <small>{format!("{state} - {detail}")}</small>
                                    </div>
                                }
                            })
                            .collect_view()}
                        {current.cartons.is_empty().then(|| view! {
                            <p class="packing-empty">"No cartons have been opened."</p>
                        })}
                    </div>
                    {active_carton_for_summary.map(|carton| view! {
                        <p class="packing-empty">{format!(
                            "Active carton {} contains {} allocation(s).",
                            carton.carton_barcode,
                            carton.content_count
                        )}</p>
                    })}
                </section>
            </div>
        </div>
    }
}

#[component]
fn CommandStatus(signals: PackingSignals, idle: bool) -> impl IntoView {
    let class = move || {
        let base = if idle {
            "packing-idle-status"
        } else {
            "packing-command-status"
        };
        if signals.error.get() {
            format!("{base} error")
        } else if signals.pending.get()
            || signals.retry.get().is_some()
            || signals.refresh_order_id.get().is_some()
        {
            format!("{base} pending")
        } else {
            base.to_owned()
        }
    };
    view! {
        <div class=class role=move || if signals.error.get() { "alert" } else { "status" }>
            {move || if signals.error.get() {
                view! { <Icon icon=UiIcon::Alert/> }.into_any()
            } else {
                view! { <Icon icon=UiIcon::Packing/> }.into_any()
            }}
            <span>{move || signals.message.get()}</span>
        </div>
    }
}

#[component]
fn CloseCartonPanel(
    carton: wareboxes_api_contract::v1::PackCartonResponse,
    on_close: Callback<()>,
    on_void: Callback<()>,
    signals: PackingSignals,
) -> impl IntoView {
    let CartonMeasurementSignals {
        weight,
        length,
        width,
        height,
    } = signals.measurements;
    let empty = carton.content_count == 0;
    view! {
        <div class="packing-carton-close">
            <strong>{carton.carton_barcode}</strong>
            <Show when=move || !empty>
                <div class="packing-measurement-grid">
                    <label class="wide"><span>"Weight (g)"</span><input inputmode="numeric" prop:value=move || weight.get() on:input=move |event| weight.set(event_target_value(&event))/></label>
                    <label><span>"Length (mm)"</span><input inputmode="numeric" prop:value=move || length.get() on:input=move |event| length.set(event_target_value(&event))/></label>
                    <label><span>"Width (mm)"</span><input inputmode="numeric" prop:value=move || width.get() on:input=move |event| width.set(event_target_value(&event))/></label>
                    <label><span>"Height (mm)"</span><input inputmode="numeric" prop:value=move || height.get() on:input=move |event| height.set(event_target_value(&event))/></label>
                </div>
                <button
                    class="button primary-action packing-carton-action"
                    type="button"
                    disabled=move || signals.blocked() || signals.item_identity.get().is_some()
                    on:click=move |_| on_close.run(())
                >
                    <Icon icon=UiIcon::Release/>
                    {move || if signals.pending.get() { "Closing" } else { "Close carton" }}
                </button>
            </Show>
            <Show when=move || empty>
                <button
                    class="button secondary-action packing-void-action"
                    type="button"
                    disabled=move || signals.blocked() || signals.item_identity.get().is_some()
                    on:click=move |_| on_void.run(())
                >
                    <Icon icon=UiIcon::Remove/>
                    {move || if signals.pending.get() { "Voiding" } else { "Void empty carton" }}
                </button>
            </Show>
        </div>
    }
}

pub(super) fn packing_locations(locations: Vec<Location>) -> Vec<PackingLocation> {
    let mut locations = locations
        .into_iter()
        .filter(|location| {
            location.deleted.is_none()
                && location.active
                && !location.pickable
                && location.r#type.eq_ignore_ascii_case("packing")
        })
        .filter_map(|location| {
            let barcode = location.barcode?.trim().to_owned();
            if barcode.is_empty() {
                return None;
            }
            let label = location.name.as_deref().map(str::trim).map_or_else(
                || barcode.clone(),
                |name| {
                    if name.is_empty() || name == barcode {
                        barcode.clone()
                    } else {
                        format!("{name} ({barcode})")
                    }
                },
            );
            Some(PackingLocation {
                id: location.id,
                facility_id: location.facility_id,
                label,
            })
        })
        .collect::<Vec<_>>();
    locations.sort_by(|left, right| {
        left.label
            .to_ascii_lowercase()
            .cmp(&right.label.to_ascii_lowercase())
            .then(left.id.cmp(&right.id))
    });
    locations
}

pub(super) fn selected_location(
    locations: StoredValue<Vec<PackingLocation>>,
    selected_id: RwSignal<String>,
) -> Option<PackingLocation> {
    let id = selected_id.get_untracked().parse::<i64>().ok()?;
    locations.with_value(|locations| locations.iter().find(|location| location.id == id).cloned())
}

pub(super) fn station_label(
    session: Option<PackSessionResponse>,
    locations: StoredValue<Vec<PackingLocation>>,
) -> String {
    session.map_or_else(
        || format!("{} available locations", locations.get_value().len()),
        |current| {
            current
                .station_location_name
                .unwrap_or(current.station_location_barcode)
        },
    )
}

pub(super) fn facility_label(
    session: Option<PackSessionResponse>,
    locations: StoredValue<Vec<PackingLocation>>,
    facilities: StoredValue<Vec<AccessScopeResource>>,
    selected_location_id: String,
) -> String {
    let facility_id = session.map(|current| current.facility_id).or_else(|| {
        let selected_location_id = selected_location_id.parse::<i64>().ok()?;
        locations
            .with_value(|locations| {
                locations
                    .iter()
                    .find(|location| location.id == selected_location_id)
                    .cloned()
            })
            .map(|location| location.facility_id)
    });
    let Some(facility_id) = facility_id else {
        return "No facility".to_owned();
    };
    facilities.with_value(|facilities| {
        facilities
            .iter()
            .find(|facility| facility.id == facility_id)
            .map_or_else(
                || format!("Facility #{facility_id}"),
                |facility| facility.name.clone(),
            )
    })
}

pub(super) fn packing_progress_label(session: Option<PackSessionResponse>) -> String {
    session.map_or_else(
        || "Idle".to_owned(),
        |current| {
            format!(
                "{} / {} allocations",
                current.progress.packed_allocation_count,
                current.progress.expected_allocation_count
            )
        },
    )
}

fn scan_prompt(
    session: Option<PackSessionResponse>,
    source_plate: Option<String>,
    item_identity: Option<PendingItemIdentity>,
) -> &'static str {
    let Some(current) = session else {
        return "SCAN ORDER";
    };
    if current.progress.status == wareboxes_api_contract::v1::PackSessionStatus::ReadyToManifest {
        return "PACKING COMPLETE";
    }
    if current.progress.expected_allocation_count > 0
        && current.progress.packed_allocation_count >= current.progress.expected_allocation_count
        && current
            .cartons
            .iter()
            .any(|carton| matches!(carton.lifecycle, PackCartonLifecycleResponse::Open))
    {
        return "ALL ITEMS PACKED - CLOSE CARTON";
    }
    if !current
        .cartons
        .iter()
        .any(|carton| matches!(carton.lifecycle, PackCartonLifecycleResponse::Open))
    {
        "SCAN NEW CARTON"
    } else if source_plate.is_none() {
        "SCAN SOURCE TOTE"
    } else if let Some(item_identity) = item_identity {
        match item_identity.stage {
            IdentityScanStage::Lot => "SCAN LOT",
            IdentityScanStage::Serial => "SCAN SERIAL",
        }
    } else {
        "SCAN ITEM"
    }
}

fn source_summaries(session: &PackSessionResponse) -> Vec<SourceSummary> {
    let mut summaries = Vec::<SourceSummary>::new();
    for allocation in &session.allocations {
        let packed = matches!(
            allocation.disposition,
            PackAllocationDispositionResponse::Packed { .. }
        );
        if let Some(existing) = summaries
            .iter_mut()
            .find(|source| source.barcode == allocation.picked_tote_license_plate_barcode)
        {
            existing.allocation_count += 1;
            existing.quantity += allocation.quantity;
            existing.packed_count += i64::from(packed);
        } else {
            summaries.push(SourceSummary {
                barcode: allocation.picked_tote_license_plate_barcode.clone(),
                location: allocation
                    .picked_tote_location_name
                    .clone()
                    .unwrap_or_else(|| allocation.picked_tote_location_barcode.clone()),
                allocation_count: 1,
                quantity: allocation.quantity,
                packed_count: i64::from(packed),
            });
        }
    }
    summaries.sort_by(|left, right| left.barcode.cmp(&right.barcode));
    summaries
}

fn compact_time(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::packing_locations;
    use wareboxes_core::models::Location;

    fn location(value: serde_json::Value) -> Location {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn station_choices_require_active_scannable_non_pickable_packing_locations() {
        let base = serde_json::json!({
            "id": 1,
            "tenant_id": 1,
            "created": "2026-08-08T20:00:00Z",
            "deleted": null,
            "facility_id": 2,
            "facility_name": "North",
            "parent_location_id": null,
            "barcode": "PACK-01",
            "name": "Packing lane 1",
            "type": "packing",
            "active": true,
            "pickable": false,
            "receivable": false
        });
        let valid = location(base.clone());
        let mut staging_value = base.clone();
        staging_value["id"] = serde_json::json!(2);
        staging_value["type"] = serde_json::json!("staging");
        let mut no_barcode_value = base;
        no_barcode_value["id"] = serde_json::json!(3);
        no_barcode_value["barcode"] = serde_json::Value::Null;

        let choices = packing_locations(vec![
            location(staging_value),
            location(no_barcode_value),
            valid,
        ]);

        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, 1);
        assert_eq!(choices[0].label, "Packing lane 1 (PACK-01)");
    }
}
