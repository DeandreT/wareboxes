mod display;
mod documents;
mod request_state;

use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{
    ConfigureFacilityShippingOriginResponse, ConfirmShipmentDepartureRequest,
    CreateShipmentRequest, CreateShipmentResponse, ManualCartonTrackingRequest, OpaqueCursor,
    RecordManualManifestRequest, RecordManualManifestResponse, ShipmentResponse, ShipmentStatus,
    ShippingQueueEntryResponse, ShippingQueuePage,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::facility_shipping_origin::FacilityShippingOriginDialog;
use crate::toast::{use_toast_bus, ToastBus};

use display::{
    compact_timestamp, departure_action_label, dimensions_label, optional_text,
    shipment_status_label,
};
use documents::ShipmentDocumentsPanel;
use request_state::{
    queue_refresh_action, queue_response_is_current, shipment_request_is_current,
    QueueRefreshAction, ShipmentRequestToken, ShipmentVersion,
};

#[cfg(target_arch = "wasm32")]
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
struct TrackingDraft {
    carton_id: i64,
    carton_barcode: String,
    tracking_number: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingShippingCommand {
    Create {
        order_id: i64,
        request: CreateShipmentRequest,
        idempotency_key: String,
    },
    Manifest {
        shipment_id: i64,
        request: RecordManualManifestRequest,
        idempotency_key: String,
    },
    Depart {
        shipment_id: i64,
        request: ConfirmShipmentDepartureRequest,
        idempotency_key: String,
    },
}

#[derive(Clone, Copy)]
struct QueueSignals {
    entries: RwSignal<Vec<ShippingQueueEntryResponse>>,
    next_cursor: RwSignal<Option<OpaqueCursor>>,
    facility_id: RwSignal<Option<i64>>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
}

#[derive(Clone, Copy)]
struct ShippingSignals {
    selected_order_id: RwSignal<Option<i64>>,
    shipment: RwSignal<Option<ShipmentResponse>>,
    pending: RwSignal<bool>,
    message: RwSignal<String>,
    error: RwSignal<bool>,
    retry: RwSignal<Option<PendingShippingCommand>>,
    tracking: RwSignal<Vec<TrackingDraft>>,
    carrier: RwSignal<String>,
    service: RwSignal<String>,
    manifest_reference: RwSignal<String>,
    departure_scan: RwSignal<String>,
    scanned_cartons: RwSignal<Vec<String>>,
    shipment_generation: RwSignal<u64>,
    focus_epoch: RwSignal<u64>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

impl ShippingSignals {
    fn blocked(self) -> bool {
        self.pending.get_untracked() || self.retry.get_untracked().is_some()
    }

    fn refocus(self) {
        self.focus_epoch
            .update(|epoch| *epoch = epoch.saturating_add(1));
    }
}

#[component]
pub(crate) fn ShippingWorkspace(
    initial_queue: ShippingQueuePage,
    access: AccessScopeWorkspace,
    can_configure_origins: bool,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let facilities = StoredValue::new(access.facilities);
    let queue = QueueSignals {
        entries: RwSignal::new(initial_queue.items),
        next_cursor: RwSignal::new(initial_queue.next_cursor),
        facility_id: RwSignal::new(None),
        pending: RwSignal::new(false),
        error: RwSignal::new(None),
        generation: RwSignal::new(0),
    };
    let signals = ShippingSignals {
        selected_order_id: RwSignal::new(None),
        shipment: RwSignal::new(None),
        pending: RwSignal::new(false),
        message: RwSignal::new("Select an order ready to ship.".to_owned()),
        error: RwSignal::new(false),
        retry: RwSignal::new(None),
        tracking: RwSignal::new(Vec::new()),
        carrier: RwSignal::new(String::new()),
        service: RwSignal::new(String::new()),
        manifest_reference: RwSignal::new(String::new()),
        departure_scan: RwSignal::new(String::new()),
        scanned_cartons: RwSignal::new(Vec::new()),
        shipment_generation: RwSignal::new(0),
        focus_epoch: RwSignal::new(0),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let origin_dialog_open = RwSignal::new(false);
    let scan_input = NodeRef::<html::Input>::new();

    #[cfg(target_arch = "wasm32")]
    install_queue_poll(queue, signals);

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        signals.focus_epoch.get();
        if let Some(input) = scan_input.get() {
            let _ = input.focus();
        }
    });

    let select_order = Callback::new(move |order_id: i64| {
        if signals.blocked() {
            return;
        }
        let Some(entry) = queue_entry(queue, order_id) else {
            set_error(signals, "That order is no longer in the shipping queue.");
            return;
        };
        signals.selected_order_id.set(Some(order_id));
        signals.scanned_cartons.set(Vec::new());
        signals.tracking.set(Vec::new());
        if let Some(shipment) = entry.shipment {
            load_shipment(order_id, shipment.shipment_id, signals);
        } else {
            invalidate_shipment_request(signals);
            signals.shipment.set(None);
            signals.error.set(false);
            signals
                .message
                .set("Review shipping readiness, then create the shipment.".to_owned());
        }
    });
    let clear_selection = Callback::new(move |_| {
        if signals.blocked() {
            return;
        }
        clear_shipping_selection(signals, "Select an order ready to ship.");
    });
    let refresh = Callback::new(move |_| request_queue(queue, signals, false));
    let load_more = Callback::new(move |_| request_queue(queue, signals, true));
    let create = Callback::new(move |_| {
        let Some(entry) = selected_entry_untracked(queue, signals) else {
            set_error(signals, "Select an order before creating a shipment.");
            return;
        };
        if !entry.origin_ready {
            if can_configure_origins {
                origin_dialog_open.set(true);
            } else {
                set_error(signals, "The facility shipping origin is not configured.");
            }
            return;
        }
        if !entry.destination_ready {
            set_error(signals, "The order has an incomplete ship-to address.");
            return;
        }
        dispatch_command(
            PendingShippingCommand::Create {
                order_id: entry.order_id,
                request: CreateShipmentRequest {
                    packing_session_id: entry.packing_session_id,
                    expected_revision: entry.order_revision,
                },
                idempotency_key: api::new_idempotency_key(),
            },
            queue,
            signals,
        );
    });
    let manifest = Callback::new(move |_| submit_manifest(queue, signals));
    let scan = Callback::new(move |_| submit_departure_scan(signals));
    let depart = Callback::new(move |_| submit_departure(queue, signals));
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch_command(command, queue, signals);
        }
    });
    let on_origin_configured =
        Callback::new(move |result: ConfigureFacilityShippingOriginResponse| {
            queue.entries.update(|entries| {
                for entry in entries {
                    if entry.facility_id == result.facility_id {
                        entry.origin_ready = true;
                        entry.facility_revision = result.revision;
                    }
                }
            });
            origin_dialog_open.set(false);
            signals.error.set(false);
            signals
                .message
                .set("Facility origin is ready. Create the shipment.".to_owned());
        });

    view! {
        <section class="shipping-workspace">
            <header class="shipping-toolbar">
                <div class="shipping-heading">
                    <Icon icon=UiIcon::Shipping/>
                    <div>
                        <h1>"Shipping"</h1>
                        <span>"Manifest and confirm packed orders"</span>
                    </div>
                </div>
                <label class="shipping-facility-filter">
                    <span>"Facility"</span>
                    <select
                        on:change=move |event| {
                            invalidate_queue_request(queue);
                            let value = event_target_value(&event);
                            queue.facility_id.set(value.parse::<i64>().ok());
                            queue.entries.set(Vec::new());
                            queue.next_cursor.set(None);
                            clear_shipping_selection(signals, "Select an order ready to ship.");
                            request_queue(queue, signals, false);
                        }
                        disabled=move || queue.pending.get() || signals.blocked()
                    >
                        <option value="">"All facilities"</option>
                        {facilities.get_value().into_iter().map(|facility| view! {
                            <option value=facility.id.to_string()>{facility.name}</option>
                        }).collect_view()}
                    </select>
                </label>
                <button
                    type="button"
                    class="icon-button"
                    title="Refresh shipping queue"
                    aria-label="Refresh shipping queue"
                    disabled=move || queue.pending.get() || signals.blocked()
                    on:click=move |_| refresh.run(())
                >
                    <Icon icon=UiIcon::Refresh/>
                </button>
            </header>

            <div class="shipping-body">
                <ShippingQueue
                    queue
                    selected_order_id=signals.selected_order_id
                    on_select=select_order
                    on_load_more=load_more
                />
                <ShippingDetail
                    queue
                    signals
                    scan_input
                    can_configure_origins
                    on_clear=clear_selection
                    on_create=create
                    on_manifest=manifest
                    on_scan=scan
                    on_depart=depart
                    on_retry=retry
                    on_configure_origin=Callback::new(move |_| origin_dialog_open.set(true))
                />
            </div>
        </section>
        <Show when=move || origin_dialog_open.get() && selected_entry(queue, signals).is_some()>
            <FacilityShippingOriginDialog
                facility_id=Signal::derive(move || selected_entry(queue, signals).map_or(0, |entry| entry.facility_id))
                facility_name=Signal::derive(move || selected_entry(queue, signals).map_or_else(String::new, |entry| entry.facility_name))
                current_revision=Signal::derive(move || selected_entry(queue, signals).map_or(0, |entry| entry.facility_revision.get()))
                on_close=Callback::new(move |_| origin_dialog_open.set(false))
                on_configured=on_origin_configured
            />
        </Show>
    }
}

#[component]
fn ShippingQueue(
    queue: QueueSignals,
    selected_order_id: RwSignal<Option<i64>>,
    on_select: Callback<i64>,
    on_load_more: Callback<()>,
) -> impl IntoView {
    view! {
        <aside class="shipping-queue" aria-label="Shipping queue">
            <header>
                <div>
                    <h2>"Ready orders"</h2>
                    <span>{move || format!("{} visible", queue.entries.get().len())}</span>
                </div>
                <Show when=move || queue.pending.get()>
                    <span class="status pending">"Refreshing"</span>
                </Show>
            </header>
            <Show when=move || queue.error.get().is_some()>
                <p class="shipping-queue-error" role="alert">{move || queue.error.get().unwrap_or_default()}</p>
            </Show>
            <div class="shipping-queue-list">
                <For
                    each=move || queue.entries.get()
                    key=|entry| (
                        entry.order_id,
                        entry.order_revision.get(),
                        entry.facility_revision.get(),
                        entry.shipment.as_ref().map_or(0, |shipment| shipment.revision.get()),
                    )
                    children=move |entry| {
                        let order_id = entry.order_id;
                        let state = entry.shipment.as_ref().map_or_else(
                            || "Ready".to_owned(),
                            |shipment| match shipment.status {
                                ShipmentStatus::AwaitingManifest => "Needs manifest".to_owned(),
                                ShipmentStatus::Manifested => "Ready to depart".to_owned(),
                                ShipmentStatus::PartiallyDeparted => format!(
                                    "{} / {} departed",
                                    shipment.departed_carton_count,
                                    shipment.carton_count,
                                ),
                                ShipmentStatus::Departed => "Departed".to_owned(),
                            },
                        );
                        let blocker = if !entry.origin_ready {
                            Some("Origin missing")
                        } else if !entry.destination_ready {
                            Some("Ship-to incomplete")
                        } else {
                            None
                        };
                        view! {
                            <button
                                type="button"
                                class="shipping-queue-row"
                                class:selected=move || selected_order_id.get() == Some(order_id)
                                on:click=move |_| on_select.run(order_id)
                            >
                                <span class="shipping-queue-primary">
                                    <strong>{entry.order_key}</strong>
                                    <small>{format!("{} · {}", entry.inventory_owner_name, entry.facility_name)}</small>
                                </span>
                                <span class="shipping-queue-state">
                                    <span class="status">{state}</span>
                                    {entry.rush.then(|| view! { <span class="status danger">"Rush"</span> })}
                                    {blocker.map(|label| view! { <span class="status warning">{label}</span> })}
                                </span>
                            </button>
                        }
                    }
                />
                <Show when=move || queue.entries.get().is_empty() && !queue.pending.get()>
                    <p class="shipping-empty">"No packed orders are ready in this scope."</p>
                </Show>
            </div>
            <Show when=move || queue.next_cursor.get().is_some()>
                <button
                    type="button"
                    class="button secondary-action shipping-load-more"
                    disabled=move || queue.pending.get()
                    on:click=move |_| on_load_more.run(())
                >"Load more"</button>
            </Show>
        </aside>
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn ShippingDetail(
    queue: QueueSignals,
    signals: ShippingSignals,
    scan_input: NodeRef<html::Input>,
    can_configure_origins: bool,
    on_clear: Callback<()>,
    on_create: Callback<()>,
    on_manifest: Callback<()>,
    on_scan: Callback<()>,
    on_depart: Callback<()>,
    on_retry: Callback<()>,
    on_configure_origin: Callback<()>,
) -> impl IntoView {
    view! {
        <main class="shipping-detail">
            <Show
                when=move || selected_entry(queue, signals).is_some()
                fallback=move || view! {
                    <div class="shipping-detail-empty">
                        <Icon icon=UiIcon::Shipping/>
                        <h2>"Select a ready order"</h2>
                        <p>"The selected order’s cartons, manifest, tracking, and departure controls appear here."</p>
                    </div>
                }
            >
                {move || selected_entry(queue, signals).map(|entry| {
                    let order_scope = format!(
                        "{} · {}",
                        entry.inventory_owner_name,
                        entry.facility_name,
                    );
                    let order_key = entry.order_key.clone();
                    let order_revision = entry.order_revision.get();
                    let ship_by = entry
                        .ship_by
                        .as_deref()
                        .map_or_else(|| "Not set".into(), compact_timestamp);
                    let readiness_entry = entry.clone();
                    view! {
                    <header class="shipping-order-header">
                        <div>
                            <span>{order_scope}</span>
                            <h2>{order_key}</h2>
                        </div>
                        <dl>
                            <div><dt>"Order rev"</dt><dd>{order_revision}</dd></div>
                            <div><dt>"Ship by"</dt><dd>{ship_by}</dd></div>
                        </dl>
                        <button type="button" class="button secondary-action" disabled=move || signals.blocked() on:click=move |_| on_clear.run(())>"Close"</button>
                    </header>
                    <CommandStatus signals on_retry/>
                    <Show
                        when=move || signals.shipment.get().is_some()
                        fallback=move || view! {
                            <ShipmentReadiness
                                entry=readiness_entry.clone()
                                can_configure_origins
                                pending=signals.pending
                                on_create
                                on_configure_origin
                            />
                        }
                    >
                        {move || signals.shipment.get().map(|shipment| view! {
                            <ShipmentExecution
                                shipment
                                signals
                                scan_input
                                on_manifest
                                on_scan
                                on_depart
                            />
                        })}
                    </Show>
                }})}
            </Show>
        </main>
    }
}

#[component]
fn CommandStatus(signals: ShippingSignals, on_retry: Callback<()>) -> impl IntoView {
    view! {
        <div
            class="shipping-command-status"
            class:error=move || signals.error.get()
            class:pending=move || signals.pending.get()
            role=move || signals.error.get().then_some("alert")
        >
            {move || if signals.error.get() {
                view! { <Icon icon=UiIcon::Alert/> }.into_any()
            } else {
                view! { <Icon icon=UiIcon::Shipping/> }.into_any()
            }}
            <span>{move || signals.message.get()}</span>
            <Show when=move || signals.retry.get().is_some()>
                <button type="button" class="button secondary-action" disabled=move || signals.pending.get() on:click=move |_| on_retry.run(())>"Retry exact command"</button>
            </Show>
        </div>
    }
}

#[component]
fn ShipmentReadiness(
    entry: ShippingQueueEntryResponse,
    can_configure_origins: bool,
    pending: RwSignal<bool>,
    on_create: Callback<()>,
    on_configure_origin: Callback<()>,
) -> impl IntoView {
    let ready = entry.origin_ready && entry.destination_ready;
    view! {
        <section class="shipping-readiness">
            <header><h3>"Shipment readiness"</h3><span class=if ready { "status success" } else { "status warning" }>{if ready { "Ready" } else { "Blocked" }}</span></header>
            <div class="shipping-readiness-grid">
                <div><span>"Packing"</span><strong>"Complete"</strong><small>{format!("Session {}", entry.packing_session_id)}</small></div>
                <div><span>"Facility origin"</span><strong>{if entry.origin_ready { "Complete" } else { "Missing" }}</strong><small>{format!("Revision {}", entry.facility_revision.get())}</small></div>
                <div><span>"Ship-to"</span><strong>{if entry.destination_ready { "Complete" } else { "Incomplete" }}</strong><small>"Order snapshot"</small></div>
            </div>
            <div class="shipping-readiness-actions">
                {(!entry.origin_ready && can_configure_origins).then(|| view! {
                    <button type="button" class="button secondary-action" disabled=move || pending.get() on:click=move |_| on_configure_origin.run(())>"Configure origin"</button>
                })}
                <button type="button" class="button primary-action" disabled=move || pending.get() || !ready on:click=move |_| on_create.run(())>"Create shipment"</button>
            </div>
        </section>
    }
}

#[component]
fn ShipmentExecution(
    shipment: ShipmentResponse,
    signals: ShippingSignals,
    scan_input: NodeRef<html::Input>,
    on_manifest: Callback<()>,
    on_scan: Callback<()>,
    on_depart: Callback<()>,
) -> impl IntoView {
    let carton_count = shipment.cartons.len();
    let packed_quantity = shipment
        .cartons
        .iter()
        .map(|carton| carton.packed_quantity)
        .sum::<i64>();
    let shipment_id = shipment.shipment_id;
    let shipment_revision = shipment.revision;
    view! {
        <div class="shipping-execution">
            <section class="shipping-cartons">
                <header>
                    <div><h3>"Cartons"</h3><span>{format!("{carton_count} cartons · {packed_quantity} units")}</span></div>
                    <span class="status success">{shipment_status_label(shipment.status)}</span>
                </header>
                <div class="table-scroll shipping-carton-scroll">
                    <table class="data-table shipping-carton-table">
                        <thead><tr><th>"#"</th><th>"Carton"</th><th>"Lines/qty"</th><th>"Weight"</th><th>"Dimensions"</th><th>"Tracking"</th><th>"Departure"</th></tr></thead>
                        <tbody>
                            {shipment.cartons.clone().into_iter().map(|carton| {
                                let carton_barcode = carton.carton_barcode;
                                let carton_title = carton_barcode.clone();
                                let packed = format!("{} / {}", carton.content_count, carton.packed_quantity);
                                let dimensions = dimensions_label(carton.length_mm, carton.width_mm, carton.height_mm);
                                let dimensions_title = dimensions.clone();
                                let tracking = carton.tracking_number.unwrap_or_else(|| "Unassigned".into());
                                let tracking_title = tracking.clone();
                                let departure = carton.departed_at.as_ref().map_or("Remaining", |_| "Departed");
                                view! {
                                <tr>
                                    <td>{carton.sequence}</td>
                                    <td class="mono" title=carton_title>{carton_barcode}</td>
                                    <td>{packed}</td>
                                    <td>{carton.weight_grams.map_or_else(|| "—".into(), |value| format!("{value} g"))}</td>
                                    <td title=dimensions_title>{dimensions}</td>
                                    <td class="mono" title=tracking_title>{tracking}</td>
                                    <td><span class=if carton.departed_at.is_some() { "status success" } else { "status" }>{departure}</span></td>
                                </tr>
                            }}).collect_view()}
                        </tbody>
                    </table>
                </div>
                <ShipmentDocumentsPanel
                    shipment_id
                    shipment_revision
                    shipment_status=shipment.status
                    on_unauthorized=signals.on_unauthorized
                />
            </section>
            <aside class="shipping-command-panel">
                {match shipment.status {
                    ShipmentStatus::AwaitingManifest => view! {
                        <ManifestPanel signals on_manifest/>
                    }.into_any(),
                    ShipmentStatus::Manifested => view! {
                        <DeparturePanel shipment signals scan_input on_scan on_depart/>
                    }.into_any(),
                    ShipmentStatus::PartiallyDeparted => view! {
                        <DeparturePanel shipment signals scan_input on_scan on_depart/>
                    }.into_any(),
                    ShipmentStatus::Departed => view! {
                        <div class="shipping-complete"><Icon icon=UiIcon::Shipping/><h3>"Shipment departed"</h3><p>"Inventory and the order are posted as shipped."</p></div>
                    }.into_any(),
                }}
            </aside>
        </div>
    }
}

#[component]
fn ManifestPanel(signals: ShippingSignals, on_manifest: Callback<()>) -> impl IntoView {
    view! {
        <form class="shipping-manifest" on:submit=move |event| { event.prevent_default(); on_manifest.run(()); }>
            <header><h3>"Carrier manifest"</h3><span>"Assign every carton"</span></header>
            <div class="shipping-manifest-fields">
                <label><span>"Carrier"</span><input required prop:value=move || signals.carrier.get() on:input=move |event| signals.carrier.set(event_target_value(&event)) /></label>
                <label><span>"Service"</span><input prop:value=move || signals.service.get() on:input=move |event| signals.service.set(event_target_value(&event)) /></label>
                <label class="wide"><span>"Manifest reference"</span><input required prop:value=move || signals.manifest_reference.get() on:input=move |event| signals.manifest_reference.set(event_target_value(&event)) /></label>
            </div>
            <div class="shipping-tracking-list">
                <For each=move || signals.tracking.get() key=|draft| draft.carton_id children=move |draft| {
                    let carton_id = draft.carton_id;
                    view! {
                        <label><span class="mono">{draft.carton_barcode}</span><input required placeholder="Tracking number" prop:value=draft.tracking_number on:input=move |event| {
                            let value = event_target_value(&event);
                            signals.tracking.update(|drafts| if let Some(row) = drafts.iter_mut().find(|row| row.carton_id == carton_id) { row.tracking_number = value; });
                        } /></label>
                    }
                }/>
            </div>
            <button type="submit" class="button primary-action" disabled=move || signals.blocked()>"Record manifest"</button>
        </form>
    }
}

#[component]
fn DeparturePanel(
    shipment: ShipmentResponse,
    signals: ShippingSignals,
    scan_input: NodeRef<html::Input>,
    on_scan: Callback<()>,
    on_depart: Callback<()>,
) -> impl IntoView {
    let expected = shipment
        .cartons
        .iter()
        .filter(|carton| carton.departed_at.is_none())
        .count();
    view! {
        <form class="shipping-departure" on:submit=move |event| { event.prevent_default(); on_scan.run(()); }>
            <header><h3>"Departure scan"</h3><span>{move || format!("{} of {expected}", signals.scanned_cartons.get().len())}</span></header>
            <label class="shipping-scan-input"><Icon icon=UiIcon::Scan/><input node_ref=scan_input autocomplete="off" placeholder="Scan carton barcode" prop:value=move || signals.departure_scan.get() on:input=move |event| signals.departure_scan.set(event_target_value(&event)) disabled=move || signals.blocked() /></label>
            <div class="shipping-scan-list">
                {shipment.cartons.into_iter().filter(|carton| carton.departed_at.is_none()).map(|carton| {
                    let barcode = carton.carton_barcode;
                    let barcode_for_class = barcode.clone();
                    let barcode_for_label = barcode.clone();
                    view! { <div class:verified=move || signals.scanned_cartons.get().iter().any(|scan| scan == &barcode_for_class)><span class="mono">{barcode}</span><strong>{move || if signals.scanned_cartons.get().iter().any(|scan| scan == &barcode_for_label) { "Verified" } else { "Pending" }}</strong></div> }
                }).collect_view()}
            </div>
            <button type="button" class="button primary-action" disabled=move || signals.blocked() || signals.scanned_cartons.get().is_empty() on:click=move |_| on_depart.run(())>{move || {
                let count = signals.scanned_cartons.get().len();
                departure_action_label(count)
            }}</button>
        </form>
    }
}

fn submit_manifest(queue: QueueSignals, signals: ShippingSignals) {
    let Some(shipment) = signals.shipment.get_untracked() else {
        return;
    };
    let carrier_code = signals.carrier.get_untracked().trim().to_owned();
    let manifest_reference = signals.manifest_reference.get_untracked().trim().to_owned();
    let tracking = signals.tracking.get_untracked();
    if carrier_code.is_empty()
        || manifest_reference.is_empty()
        || tracking
            .iter()
            .any(|row| row.tracking_number.trim().is_empty())
    {
        set_error(
            signals,
            "Carrier, manifest reference, and every tracking number are required.",
        );
        return;
    }
    dispatch_command(
        PendingShippingCommand::Manifest {
            shipment_id: shipment.shipment_id,
            request: RecordManualManifestRequest {
                carrier_code,
                service_code: optional_text(&signals.service.get_untracked()),
                manifest_reference,
                carton_tracking_assignments: tracking
                    .into_iter()
                    .map(|row| ManualCartonTrackingRequest {
                        carton_id: row.carton_id,
                        tracking_number: row.tracking_number.trim().to_owned(),
                    })
                    .collect(),
                expected_revision: shipment.revision,
            },
            idempotency_key: api::new_idempotency_key(),
        },
        queue,
        signals,
    );
}

fn submit_departure_scan(signals: ShippingSignals) {
    if signals.blocked() {
        return;
    }
    let scan = signals.departure_scan.get_untracked().trim().to_owned();
    let Some(shipment) = signals.shipment.get_untracked() else {
        return;
    };
    if scan.is_empty() {
        set_error(signals, "Scan a carton barcode.");
        return;
    }
    let Some(expected) = shipment
        .cartons
        .iter()
        .find(|carton| {
            carton.departed_at.is_none() && carton.carton_barcode.eq_ignore_ascii_case(&scan)
        })
        .map(|carton| carton.carton_barcode.clone())
    else {
        set_error(signals, "That carton is not remaining on this shipment.");
        signals.departure_scan.set(String::new());
        signals.refocus();
        return;
    };
    if signals
        .scanned_cartons
        .get_untracked()
        .iter()
        .any(|current| current == &expected)
    {
        set_error(signals, "That carton is already verified.");
    } else {
        signals.scanned_cartons.update(|scans| scans.push(expected));
        signals.error.set(false);
        signals.message.set("Carton verified.".to_owned());
    }
    signals.departure_scan.set(String::new());
    signals.refocus();
}

fn submit_departure(queue: QueueSignals, signals: ShippingSignals) {
    let Some(shipment) = signals.shipment.get_untracked() else {
        return;
    };
    let scanned = signals.scanned_cartons.get_untracked();
    if scanned.is_empty() {
        set_error(
            signals,
            "Verify at least one remaining carton before departure.",
        );
        return;
    }
    dispatch_command(
        PendingShippingCommand::Depart {
            shipment_id: shipment.shipment_id,
            request: ConfirmShipmentDepartureRequest {
                scanned_carton_barcodes: scanned,
                expected_shipment_revision: shipment.revision,
                expected_order_revision: shipment.order_revision,
            },
            idempotency_key: api::new_idempotency_key(),
        },
        queue,
        signals,
    );
}

fn dispatch_command(
    command: PendingShippingCommand,
    queue: QueueSignals,
    signals: ShippingSignals,
) {
    if signals.pending.get_untracked() {
        return;
    }
    signals.pending.set(true);
    signals.retry.set(None);
    signals.error.set(false);
    signals
        .message
        .set(command_pending_label(&command).to_owned());
    leptos::task::spawn_local(async move {
        let result = match &command {
            PendingShippingCommand::Create {
                order_id,
                request,
                idempotency_key,
            } => api::internal_post_idempotent::<_, CreateShipmentResponse>(
                &format!("/api/v1/orders/{order_id}/shipments"),
                request,
                idempotency_key,
            )
            .await
            .map(|result| CommandResult::Created(Box::new(result))),
            PendingShippingCommand::Manifest {
                shipment_id,
                request,
                idempotency_key,
            } => api::internal_post_idempotent::<_, RecordManualManifestResponse>(
                &format!("/api/v1/shipments/{shipment_id}/manifests"),
                request,
                idempotency_key,
            )
            .await
            .map(|result| CommandResult::Refresh {
                order_id: result.order_id,
                shipment_id: result.shipment_id,
            }),
            PendingShippingCommand::Depart {
                shipment_id,
                request,
                idempotency_key,
            } => api::internal_post_idempotent::<
                _,
                wareboxes_api_contract::v1::ConfirmShipmentDepartureResponse,
            >(
                &format!("/api/v1/shipments/{shipment_id}/departures"),
                request,
                idempotency_key,
            )
            .await
            .map(CommandResult::Departed),
        };
        signals.pending.set(false);
        match result {
            Ok(CommandResult::Created(result)) => {
                let result = *result;
                signals.shipment.set(Some(result.shipment.clone()));
                initialize_shipment(result.shipment, signals);
                signals.toasts.success("Shipment created.");
                refresh_shipping_queue(queue, signals);
            }
            Ok(CommandResult::Refresh {
                order_id,
                shipment_id,
            }) => {
                signals.toasts.success("Carrier manifest recorded.");
                load_shipment(order_id, shipment_id, signals);
                refresh_shipping_queue(queue, signals);
            }
            Ok(CommandResult::Departed(result)) => {
                if matches!(result.shipment_status, ShipmentStatus::Departed) {
                    signals
                        .toasts
                        .success(format!("Shipment {} departed.", result.shipment_id));
                    clear_shipping_selection(signals, "Select an order ready to ship.");
                } else {
                    signals.toasts.success(format!(
                        "{} carton(s) departed; {} remain.",
                        result.scanned_carton_count, result.remaining_carton_count
                    ));
                    load_shipment(result.order_id, result.shipment_id, signals);
                }
                refresh_shipping_queue(queue, signals);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) if error.ambiguous_outcome => {
                signals.retry.set(Some(command));
                set_error(
                    signals,
                    format!(
                        "{} Retry the exact command before continuing.",
                        error.message
                    ),
                );
            }
            Err(error) => {
                set_error(signals, error.message);
                refresh_shipping_queue(queue, signals);
            }
        }
    });
}

enum CommandResult {
    Created(Box<CreateShipmentResponse>),
    Refresh { order_id: i64, shipment_id: i64 },
    Departed(wareboxes_api_contract::v1::ConfirmShipmentDepartureResponse),
}

fn load_shipment(order_id: i64, shipment_id: i64, signals: ShippingSignals) {
    let generation = signals
        .shipment_generation
        .get_untracked()
        .saturating_add(1);
    signals.shipment_generation.set(generation);
    let token = ShipmentRequestToken {
        generation,
        order_id,
        shipment_id,
    };
    signals.pending.set(true);
    signals.error.set(false);
    signals.message.set("Loading shipment...".to_owned());
    leptos::task::spawn_local(async move {
        let result = api::internal_get::<ShipmentResponse>(&format!(
            "/api/v1/shipments/{}",
            token.shipment_id
        ))
        .await;
        if !shipment_request_is_current(
            token,
            signals.shipment_generation.get_untracked(),
            signals.selected_order_id.get_untracked(),
        ) {
            return;
        }
        signals.pending.set(false);
        match result {
            Ok(shipment)
                if shipment.order_id == token.order_id
                    && shipment.shipment_id == token.shipment_id =>
            {
                initialize_shipment(shipment, signals);
            }
            Ok(_) => set_error(
                signals,
                "The shipment response did not match the selected order.",
            ),
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => set_error(signals, error.message),
        }
    });
}

fn initialize_shipment(shipment: ShipmentResponse, signals: ShippingSignals) {
    signals.tracking.set(
        shipment
            .cartons
            .iter()
            .map(|carton| TrackingDraft {
                carton_id: carton.carton_id,
                carton_barcode: carton.carton_barcode.clone(),
                tracking_number: carton.tracking_number.clone().unwrap_or_default(),
            })
            .collect(),
    );
    if let Some(manifest) = shipment.manifest.as_ref() {
        signals.carrier.set(manifest.carrier_code.clone());
        signals
            .service
            .set(manifest.service_code.clone().unwrap_or_default());
        signals
            .manifest_reference
            .set(manifest.manifest_reference.clone());
    } else {
        signals.carrier.set(String::new());
        signals.service.set(String::new());
        signals.manifest_reference.set(String::new());
    }
    let status = shipment.status;
    signals.shipment.set(Some(shipment));
    signals.scanned_cartons.set(Vec::new());
    signals.error.set(false);
    signals.message.set(
        match status {
            ShipmentStatus::AwaitingManifest => "Assign tracking and record the carrier manifest.",
            ShipmentStatus::Manifested => "Scan one or more cartons for physical departure.",
            ShipmentStatus::PartiallyDeparted => {
                "Scan one or more remaining cartons for the next departure."
            }
            ShipmentStatus::Departed => "Shipment departure is complete.",
        }
        .to_owned(),
    );
    signals.refocus();
}

fn invalidate_shipment_request(signals: ShippingSignals) {
    signals
        .shipment_generation
        .update(|generation| *generation = generation.saturating_add(1));
    signals.pending.set(false);
}

fn invalidate_queue_request(queue: QueueSignals) {
    queue
        .generation
        .update(|generation| *generation = generation.saturating_add(1));
    queue.pending.set(false);
}

fn refresh_shipping_queue(queue: QueueSignals, signals: ShippingSignals) {
    invalidate_queue_request(queue);
    request_queue(queue, signals, false);
}

fn clear_shipping_selection(signals: ShippingSignals, message: &str) {
    invalidate_shipment_request(signals);
    signals.selected_order_id.set(None);
    signals.shipment.set(None);
    signals.retry.set(None);
    signals.tracking.set(Vec::new());
    signals.carrier.set(String::new());
    signals.service.set(String::new());
    signals.manifest_reference.set(String::new());
    signals.departure_scan.set(String::new());
    signals.scanned_cartons.set(Vec::new());
    signals.message.set(message.to_owned());
    signals.error.set(false);
}

fn reconcile_selected_queue_entry(queue: QueueSignals, signals: ShippingSignals) {
    let selected_order_id = signals.selected_order_id.get_untracked();
    let entry = selected_order_id.and_then(|order_id| queue_entry(queue, order_id));
    let queued = entry.as_ref().and_then(|entry| {
        entry.shipment.as_ref().map(|shipment| ShipmentVersion {
            shipment_id: shipment.shipment_id,
            shipment_revision: shipment.revision.get(),
            order_revision: entry.order_revision.get(),
        })
    });
    let current = signals
        .shipment
        .get_untracked()
        .map(|shipment| ShipmentVersion {
            shipment_id: shipment.shipment_id,
            shipment_revision: shipment.revision.get(),
            order_revision: shipment.order_revision.get(),
        });
    match queue_refresh_action(
        selected_order_id.is_some(),
        entry.is_some(),
        queued,
        current,
    ) {
        QueueRefreshAction::Keep => {}
        QueueRefreshAction::ClearSelection => clear_shipping_selection(
            signals,
            "The selected order is no longer in the shipping queue.",
        ),
        QueueRefreshAction::ClearShipment => {
            invalidate_shipment_request(signals);
            signals.shipment.set(None);
            signals.tracking.set(Vec::new());
            signals.scanned_cartons.set(Vec::new());
            signals
                .message
                .set("Review shipping readiness, then create the shipment.".to_owned());
            signals.error.set(false);
        }
        QueueRefreshAction::Load(version) => {
            if let Some(order_id) = selected_order_id {
                load_shipment(order_id, version.shipment_id, signals);
            }
        }
    }
}

fn request_queue(queue: QueueSignals, signals: ShippingSignals, append: bool) {
    if queue.pending.get_untracked() {
        return;
    }
    let cursor = if append {
        queue.next_cursor.get_untracked()
    } else {
        None
    };
    if append && cursor.is_none() {
        return;
    }
    let generation = queue.generation.get_untracked().saturating_add(1);
    queue.generation.set(generation);
    queue.pending.set(true);
    queue.error.set(None);
    let facility_id = queue.facility_id.get_untracked();
    leptos::task::spawn_local(async move {
        let mut path = "/api/v1/shipping-queue?limit=100".to_owned();
        if let Some(facility_id) = facility_id {
            path.push_str(&format!("&facility_id={facility_id}"));
        }
        if let Some(cursor) = cursor.as_ref() {
            path.push_str("&cursor=");
            path.push_str(&urlencoding::encode(cursor.as_str()));
        }
        let result = api::internal_get::<ShippingQueuePage>(&path).await;
        if !queue_response_is_current(
            generation,
            facility_id,
            queue.generation.get_untracked(),
            queue.facility_id.get_untracked(),
        ) {
            return;
        }
        queue.pending.set(false);
        match result {
            Ok(page) => {
                if append {
                    queue.entries.update(|entries| {
                        for item in page.items {
                            if !entries.iter().any(|entry| entry.order_id == item.order_id) {
                                entries.push(item);
                            }
                        }
                    });
                } else {
                    queue.entries.set(page.items);
                }
                queue.next_cursor.set(page.next_cursor);
                if !append {
                    reconcile_selected_queue_entry(queue, signals);
                }
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => queue.error.set(Some(error.message)),
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn install_queue_poll(queue: QueueSignals, signals: ShippingSignals) {
    let Some(owner) = Owner::current() else {
        return;
    };
    let Ok(handle) = set_interval_with_handle(
        move || {
            if signals.selected_order_id.get_untracked().is_none()
                && !signals.blocked()
                && !queue.pending.get_untracked()
            {
                owner.with(|| request_queue(queue, signals, false));
            }
        },
        Duration::from_secs(15),
    ) else {
        return;
    };
    on_cleanup(move || handle.clear());
}

fn selected_entry(
    queue: QueueSignals,
    signals: ShippingSignals,
) -> Option<ShippingQueueEntryResponse> {
    signals.selected_order_id.get().and_then(|order_id| {
        queue
            .entries
            .get()
            .into_iter()
            .find(|entry| entry.order_id == order_id)
    })
}

fn selected_entry_untracked(
    queue: QueueSignals,
    signals: ShippingSignals,
) -> Option<ShippingQueueEntryResponse> {
    signals
        .selected_order_id
        .get_untracked()
        .and_then(|order_id| queue_entry(queue, order_id))
}

fn queue_entry(queue: QueueSignals, order_id: i64) -> Option<ShippingQueueEntryResponse> {
    queue
        .entries
        .get_untracked()
        .into_iter()
        .find(|entry| entry.order_id == order_id)
}

fn set_error(signals: ShippingSignals, message: impl Into<String>) {
    signals.error.set(true);
    signals.message.set(message.into());
}

fn command_pending_label(command: &PendingShippingCommand) -> &'static str {
    match command {
        PendingShippingCommand::Create { .. } => "Creating shipment...",
        PendingShippingCommand::Manifest { .. } => "Recording carrier manifest...",
        PendingShippingCommand::Depart { .. } => "Confirming shipment departure...",
    }
}
