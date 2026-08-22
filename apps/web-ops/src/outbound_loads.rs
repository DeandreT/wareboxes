use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CancelOutboundLoadRequest, CompleteOutboundLoadLoadingRequest,
    CompleteOutboundLoadLoadingResponse, ConfirmOutboundLoadDepartureRequest,
    ConfirmOutboundLoadDepartureResponse, OpaqueCursor, OutboundLoadCancellationReason,
    OutboundLoadQueueEntryResponse, OutboundLoadQueuePage, OutboundLoadResponse,
    OutboundLoadStatus, PlanOutboundLoadCartonRequest, PlanOutboundLoadRequest,
    PlanOutboundLoadResponse, PlanOutboundLoadShipmentRequest, ReleaseOutboundLoadRequest,
    ReleaseOutboundLoadResponse, ShipmentResponse, ShipmentStatus, ShippingQueueEntryResponse,
    ShippingQueuePage, StartOutboundLoadLoadingRequest, StartOutboundLoadLoadingResponse,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::Location;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::{use_toast_bus, ToastBus};
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[cfg(target_arch = "wasm32")]
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingCommand {
    Plan {
        request: PlanOutboundLoadRequest,
        key: String,
    },
    Release {
        load_id: i64,
        request: ReleaseOutboundLoadRequest,
        key: String,
    },
    Start {
        load_id: i64,
        request: StartOutboundLoadLoadingRequest,
        key: String,
    },
    Complete {
        load_id: i64,
        request: CompleteOutboundLoadLoadingRequest,
        key: String,
    },
    Depart {
        load_id: i64,
        request: ConfirmOutboundLoadDepartureRequest,
        key: String,
    },
    Cancel {
        load_id: i64,
        request: CancelOutboundLoadRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
struct Signals {
    entries: RwSignal<Vec<OutboundLoadQueueEntryResponse>>,
    next_cursor: RwSignal<Option<OpaqueCursor>>,
    queue_generation: RwSignal<u64>,
    facility_id: RwSignal<Option<i64>>,
    status: RwSignal<Option<OutboundLoadStatus>>,
    sort: RwSignal<SortSpec<LoadSort>>,
    queue_pending: RwSignal<bool>,
    shipping_entries: RwSignal<Vec<ShippingQueueEntryResponse>>,
    shipping_next_cursor: RwSignal<Option<OpaqueCursor>>,
    shipping_generation: RwSignal<u64>,
    shipping_facility_id: RwSignal<Option<i64>>,
    shipping_pending: RwSignal<bool>,
    selected_id: RwSignal<Option<i64>>,
    detail: RwSignal<Option<OutboundLoadResponse>>,
    detail_generation: RwSignal<u64>,
    command_pending: RwSignal<bool>,
    retry: RwSignal<Option<PendingCommand>>,
    message: RwSignal<String>,
    error: RwSignal<bool>,
    dialog: RwSignal<Option<Dialog>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadSort {
    Reference,
    Status,
    Progress,
    Facility,
    Trailer,
    ScheduledDeparture,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct QueueRequestIdentity {
    generation: u64,
    facility_id: Option<i64>,
    status: Option<OutboundLoadStatus>,
    sort: SortSpec<LoadSort>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialog {
    Plan,
    Start,
    Complete,
    Depart,
    Cancel,
}

#[derive(Clone, Copy)]
struct Drafts {
    reference: RwSignal<String>,
    carrier: RwSignal<String>,
    staging_id: RwSignal<Option<i64>>,
    scheduled_at: RwSignal<String>,
    selected_shipments: RwSignal<Vec<i64>>,
    dock_barcode: RwSignal<String>,
    trailer: RwSignal<String>,
    seal: RwSignal<String>,
    cancellation_reason: RwSignal<OutboundLoadCancellationReason>,
    cancellation_note: RwSignal<String>,
}

#[component]
pub(crate) fn OutboundLoadsWorkspace(
    initial_queue: OutboundLoadQueuePage,
    shipping_queue: ShippingQueuePage,
    access: AccessScopeWorkspace,
    locations: Vec<Location>,
    can_supervise: bool,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let facilities = StoredValue::new(access.facilities);
    let locations = StoredValue::new(locations);
    let signals = Signals {
        entries: RwSignal::new(initial_queue.items),
        next_cursor: RwSignal::new(initial_queue.next_cursor),
        queue_generation: RwSignal::new(0),
        facility_id: RwSignal::new(None),
        status: RwSignal::new(None),
        sort: RwSignal::new(SortSpec {
            key: LoadSort::ScheduledDeparture,
            direction: SortDirection::Ascending,
        }),
        queue_pending: RwSignal::new(false),
        shipping_entries: RwSignal::new(shipping_queue.items),
        shipping_next_cursor: RwSignal::new(shipping_queue.next_cursor),
        shipping_generation: RwSignal::new(0),
        shipping_facility_id: RwSignal::new(None),
        shipping_pending: RwSignal::new(false),
        selected_id: RwSignal::new(None),
        detail: RwSignal::new(None),
        detail_generation: RwSignal::new(0),
        command_pending: RwSignal::new(false),
        retry: RwSignal::new(None),
        message: RwSignal::new("Select a load to review execution.".into()),
        error: RwSignal::new(false),
        dialog: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = Drafts {
        reference: RwSignal::new(String::new()),
        carrier: RwSignal::new(String::new()),
        staging_id: RwSignal::new(None),
        scheduled_at: RwSignal::new(String::new()),
        selected_shipments: RwSignal::new(Vec::new()),
        dock_barcode: RwSignal::new(String::new()),
        trailer: RwSignal::new(String::new()),
        seal: RwSignal::new(String::new()),
        cancellation_reason: RwSignal::new(OutboundLoadCancellationReason::PlanningError),
        cancellation_note: RwSignal::new(String::new()),
    };
    let layout = SplitPaneState::new("outbound-loads", 620);

    #[cfg(target_arch = "wasm32")]
    install_poll(signals);

    let refresh = Callback::new(move |_| request_queue(signals, false));
    let load_more = Callback::new(move |_| request_queue(signals, true));
    let change_sort = Callback::new(move |key: LoadSort| {
        SortSpec::select(signals.sort, key);
        invalidate_queue(signals);
        request_queue(signals, false);
    });
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(command, signals);
        }
    });
    let select = Callback::new(move |load_id: i64| {
        if !signals.command_pending.get_untracked() {
            layout.show_detail();
            signals.selected_id.set(Some(load_id));
            signals.detail.set(None);
            load_detail(load_id, signals);
        }
    });
    let release = Callback::new(move |_| {
        let Some(load) = signals.detail.get_untracked() else {
            return;
        };
        dispatch(
            PendingCommand::Release {
                load_id: load.outbound_load_id,
                request: ReleaseOutboundLoadRequest {
                    expected_revision: load.revision,
                },
                key: api::new_idempotency_key(),
            },
            signals,
        );
    });
    let submit_plan = Callback::new(move |_| {
        prepare_plan(drafts, signals);
    });
    let submit_phase = Callback::new(move |dialog: Dialog| {
        let Some(load) = signals.detail.get_untracked() else {
            return;
        };
        let trailer = drafts.trailer.get_untracked().trim().to_owned();
        let dock = drafts.dock_barcode.get_untracked().trim().to_owned();
        let seal = drafts.seal.get_untracked().trim().to_owned();
        let command = match dialog {
            Dialog::Start if !dock.is_empty() && !trailer.is_empty() => PendingCommand::Start {
                load_id: load.outbound_load_id,
                request: StartOutboundLoadLoadingRequest {
                    expected_revision: load.revision,
                    load_barcode: load.load_barcode.clone(),
                    staging_location_barcode: load.staging_location_barcode.clone(),
                    dock_location_barcode: dock,
                    trailer_number: trailer,
                },
                key: api::new_idempotency_key(),
            },
            Dialog::Complete if !dock.is_empty() && !trailer.is_empty() && !seal.is_empty() => {
                PendingCommand::Complete {
                    load_id: load.outbound_load_id,
                    request: CompleteOutboundLoadLoadingRequest {
                        expected_revision: load.revision,
                        load_barcode: load.load_barcode.clone(),
                        dock_location_barcode: dock,
                        trailer_number: trailer,
                        seal_number: seal,
                    },
                    key: api::new_idempotency_key(),
                }
            }
            Dialog::Depart if !dock.is_empty() && !trailer.is_empty() && !seal.is_empty() => {
                PendingCommand::Depart {
                    load_id: load.outbound_load_id,
                    request: ConfirmOutboundLoadDepartureRequest {
                        expected_revision: load.revision,
                        load_barcode: load.load_barcode.clone(),
                        dock_location_barcode: dock,
                        trailer_number: trailer,
                        seal_number: seal,
                    },
                    key: api::new_idempotency_key(),
                }
            }
            Dialog::Cancel => {
                let reason = drafts.cancellation_reason.get_untracked();
                let note = optional_text(&drafts.cancellation_note.get_untracked());
                if reason == OutboundLoadCancellationReason::Other && note.is_none() {
                    set_error(
                        signals,
                        "A note is required when the cancellation reason is Other.",
                    );
                    return;
                }
                PendingCommand::Cancel {
                    load_id: load.outbound_load_id,
                    request: CancelOutboundLoadRequest {
                        expected_revision: load.revision,
                        reason,
                        note,
                    },
                    key: api::new_idempotency_key(),
                }
            }
            _ => {
                set_error(signals, "Complete every required scan field.");
                return;
            }
        };
        dispatch(command, signals);
    });

    let facility_options = move || {
        facilities.with_value(|values| {
            values
                .iter()
                .map(
                    |facility| view! { <option value=facility.id>{facility.name.clone()}</option> },
                )
                .collect_view()
        })
    };

    view! {
        <section class="outbound-loads-workspace">
            <header class="outbound-loads-toolbar">
                <div class="outbound-loads-heading">
                    <Icon icon=UiIcon::Shipping/>
                    <div><h1>"Outbound loads"</h1></div>
                </div>
                <PaneControls layout master_label="load queue" detail_label="load detail"/>
                <label><span>"Facility"</span><select on:change=move |event| {
                    signals.facility_id.set(parse_optional_id(&event_target_value(&event)));
                    invalidate_queue(signals);
                    request_queue(signals, false);
                }><option value="">"All facilities"</option>{facility_options}</select></label>
                <label><span>"State"</span><select on:change=move |event| {
                    signals.status.set(parse_status(&event_target_value(&event)));
                    invalidate_queue(signals);
                    request_queue(signals, false);
                }>
                    <option value="">"Active loads"</option>
                    <option value="planned">"Planned"</option><option value="staging">"Staging"</option>
                    <option value="loading">"Loading"</option><option value="ready_to_depart">"Ready"</option>
                    <option value="departed">"Departed"</option><option value="cancelled">"Cancelled"</option>
                </select></label>
                <button class="icon-button" type="button" title="Refresh loads" aria-label="Refresh loads" disabled=move || signals.queue_pending.get() on:click=move |_| refresh.run(())><Icon icon=UiIcon::Refresh/></button>
                {can_supervise.then(|| view! { <button class="button primary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=move |_| open_plan(drafts, signals)><Icon icon=UiIcon::Add/><span>"Plan load"</span></button> })}
            </header>
            <div
                class="outbound-loads-body split-workspace"
                style=move || layout.style()
                data-pane-mode=move || layout.mode_attribute()
            >
                <section class="outbound-loads-queue split-master">
                    <header><h2>"Load queue"</h2><span>{move || format!("{} loaded", signals.entries.get().len())}</span></header>
                    <div class="outbound-loads-table-scroll" aria-busy=move || signals.queue_pending.get().to_string()>
                        <table><caption class="sr-only">"Outbound loads matching the active facility and state filters"</caption><thead><tr>
                            <SortableHeader label="Load" active=move || signals.sort.get().key == LoadSort::Reference direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| change_sort.run(LoadSort::Reference))/>
                            <SortableHeader label="State" active=move || signals.sort.get().key == LoadSort::Status direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| change_sort.run(LoadSort::Status))/>
                            <SortableHeader label="Progress" active=move || signals.sort.get().key == LoadSort::Progress direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| change_sort.run(LoadSort::Progress)) numeric=true/>
                            <SortableHeader label="Facility" active=move || signals.sort.get().key == LoadSort::Facility direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| change_sort.run(LoadSort::Facility))/>
                            <SortableHeader label="Trailer" active=move || signals.sort.get().key == LoadSort::Trailer direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| change_sort.run(LoadSort::Trailer))/>
                            <SortableHeader label="Depart" active=move || signals.sort.get().key == LoadSort::ScheduledDeparture direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| change_sort.run(LoadSort::ScheduledDeparture))/>
                            <th><span class="sr-only">"Open detail"</span></th>
                        </tr></thead>
                        <tbody>{move || {
                            let entries=signals.entries.get();
                            if entries.is_empty() {
                                let message=if signals.queue_pending.get() { "Loading outbound loads..." } else { "No outbound loads match the active filters." };
                                view! { <tr><td colspan="7" class="table-empty-row" role="status" aria-live="polite">{message}</td></tr> }.into_any()
                            } else {
                                entries.into_iter().map(|entry| {
                                    let id = entry.outbound_load_id;
                                    let selected = signals.selected_id.get() == Some(id);
                                    view! { <tr class:selected=selected>
                                        <td><strong class="mono">{entry.load_reference}</strong><small>{entry.carrier_code}</small></td>
                                        <td><span class=format!("status-chip {}", status_class(entry.status))>{status_label(entry.status)}</span></td>
                                        <td><strong>{format!("{}/{} loaded", entry.progress.loaded_carton_count, entry.progress.planned_carton_count)}</strong><small>{format!("{} staged", entry.progress.staged_carton_count)}</small></td>
                                        <td>{entry.facility_name}</td><td>{entry.trailer_number.unwrap_or_else(|| "Not assigned".into())}</td>
                                        <td>{entry.scheduled_departure_at.as_deref().map(compact_timestamp).unwrap_or_else(|| "Not scheduled".into())}</td>
                                        <td><button type="button" class="icon-button" title="Open load detail" aria-label=format!("Open load {}", id) aria-pressed=selected on:click=move |_| select.run(id)><Icon icon=UiIcon::Search/></button></td>
                                    </tr> }
                                }).collect_view().into_any()
                            }
                        }}</tbody></table>
                    </div>
                    {move || signals.next_cursor.get().map(|_| view! { <button class="button quiet-action outbound-loads-more" type="button" disabled=move || signals.queue_pending.get() on:click=move |_| load_more.run(())>"Load more"</button> })}
                </section>
                <SplitPaneHandle layout/>
                <section class="outbound-loads-detail split-detail">
                    <div class:error=move || signals.error.get() class="outbound-loads-status" role=move || if signals.error.get() { "alert" } else { "status" } aria-live="polite" aria-atomic="true"><span>{move || signals.message.get()}</span>{move || signals.retry.get().map(|_| view! { <button type="button" class="button quiet-action compact" on:click=move |_| retry.run(())>"Retry exact command"</button> })}</div>
                    {move || signals.detail.get().map(|load| detail_view(load, signals, drafts, can_supervise, release)).unwrap_or_else(|| view! { <div class="outbound-loads-empty"><Icon icon=UiIcon::Shipping/><h2>"No load selected"</h2></div> }.into_any())}
                </section>
            </div>
        </section>
        {move || signals.dialog.get().map(|dialog| dialog_view(dialog, signals, drafts, locations, submit_plan, submit_phase))}
    }
}

fn detail_view(
    load: OutboundLoadResponse,
    signals: Signals,
    drafts: Drafts,
    can_supervise: bool,
    release: Callback<()>,
) -> AnyView {
    let all_staged = load.progress.staged_carton_count == load.progress.planned_carton_count;
    let all_loaded = load.progress.loaded_carton_count == load.progress.planned_carton_count;
    let can_cancel = can_cancel_load(
        load.status,
        load.progress.staged_carton_count,
        load.progress.loaded_carton_count,
    );
    let status = load.status;
    let trailer = load.trailer_number.clone().unwrap_or_default();
    let dock = load.dock_location_barcode.clone().unwrap_or_default();
    let seal = load.seal_number.clone().unwrap_or_default();
    let complete_trailer = trailer.clone();
    let complete_dock = dock.clone();
    let depart_trailer = trailer.clone();
    let depart_dock = dock.clone();
    let reference = load.load_reference.clone();
    view! {
        <div class="outbound-loads-detail-scroll">
            <header class="outbound-loads-detail-header">
                <div><span class="eyebrow">"Outbound load"</span><h2>{reference}</h2><small class="mono">{load.load_barcode.clone()}</small></div>
                <div class="outbound-loads-progress"><span class=format!("status-chip {}", status_class(status))>{status_label(status)}</span><strong>{format!("{}/{} loaded", load.progress.loaded_carton_count, load.progress.planned_carton_count)}</strong><small>{format!("{} shipments · revision {}", load.progress.planned_shipment_count, load.revision.get())}</small></div>
            </header>
            <dl class="outbound-loads-facts"><div><dt>"Staging lane"</dt><dd>{load.staging_location_name}</dd></div><div><dt>"Dock"</dt><dd>{load.dock_location_name.unwrap_or_else(|| "Not assigned".into())}</dd></div><div><dt>"Trailer"</dt><dd>{if trailer.is_empty() { "Not assigned".into() } else { trailer.clone() }}</dd></div><div><dt>"Seal"</dt><dd>{if seal.is_empty() { "Not sealed".into() } else { seal.clone() }}</dd></div></dl>
            <div class="outbound-loads-membership">
                <section><header><h3>"Shipments"</h3><span>{load.shipments.len()}</span></header><div class="outbound-loads-table-scroll"><table><caption class="sr-only">"Shipments assigned to this outbound load"</caption><thead><tr><th>"Stop"</th><th>"Order"</th><th>"Client"</th><th>"Shipment"</th><th>"Demand"</th></tr></thead><tbody>{load.shipments.into_iter().map(|shipment| view! { <tr><td>{shipment.shipment_sequence}</td><td class="mono">{shipment.order_key}</td><td>{shipment.inventory_owner_name}</td><td>{format!("#{} · {:?}", shipment.shipment_id, shipment.shipment_status)}</td><td>{format!("{} ship / {} ordered", shipment.demand.shipped_quantity, shipment.demand.ordered_quantity)}</td></tr> }).collect_view()}</tbody></table></div></section>
                <section><header><h3>"Carton execution"</h3><span>{load.cartons.len()}</span></header><div class="outbound-loads-table-scroll"><table><caption class="sr-only">"Cartons assigned to this outbound load"</caption><thead><tr><th>"Seq"</th><th>"Carton"</th><th>"State"</th><th>"Qty"</th><th>"Position rev"</th></tr></thead><tbody>{load.cartons.into_iter().map(|carton| view! { <tr><td>{carton.load_sequence}</td><td class="mono">{carton.carton_barcode}</td><td>{position_label(&carton.state)}</td><td>{carton.packed_quantity}</td><td>{carton.position_revision.get()}</td></tr> }).collect_view()}</tbody></table></div></section>
            </div>
            {can_supervise.then(|| view! { <footer class="outbound-loads-actions">
                {matches!(status, OutboundLoadStatus::Planned).then(|| view! { <button class="button primary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| release.run(())>"Release to staging"</button> })}
                {(status == OutboundLoadStatus::Staging && all_staged).then(|| view! { <button class="button primary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| { drafts.dock_barcode.set(String::new()); drafts.trailer.set(String::new()); signals.dialog.set(Some(Dialog::Start)); }>"Start loading"</button> })}
                {(status == OutboundLoadStatus::Loading && all_loaded).then(|| view! { <button class="button primary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| { drafts.dock_barcode.set(complete_dock.clone()); drafts.trailer.set(complete_trailer.clone()); drafts.seal.set(String::new()); signals.dialog.set(Some(Dialog::Complete)); }>"Complete loading"</button> })}
                {(status == OutboundLoadStatus::ReadyToDepart).then(|| view! { <button class="button primary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| { drafts.dock_barcode.set(depart_dock.clone()); drafts.trailer.set(depart_trailer.clone()); drafts.seal.set(seal.clone()); signals.dialog.set(Some(Dialog::Depart)); }>"Confirm departure"</button> })}
                {can_cancel.then(|| view! { <button class="button danger-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(Some(Dialog::Cancel))>"Cancel load"</button> })}
            </footer> })}
        </div>
    }.into_any()
}

fn dialog_view(
    dialog: Dialog,
    signals: Signals,
    drafts: Drafts,
    locations: StoredValue<Vec<Location>>,
    submit_plan: Callback<()>,
    submit_phase: Callback<Dialog>,
) -> AnyView {
    let close = move |_| {
        if !signals.command_pending.get_untracked() {
            signals.dialog.set(None)
        }
    };
    if dialog == Dialog::Plan {
        let select_staging = move |event| {
            let staging_id = parse_optional_id(&event_target_value(&event));
            let facility_id = staging_id.and_then(|location_id| {
                locations.with_value(|all| {
                    all.iter()
                        .find(|location| location.id == location_id)
                        .map(|location| location.facility_id)
                })
            });
            drafts.staging_id.set(staging_id);
            drafts.selected_shipments.set(Vec::new());
            invalidate_shipping(signals, facility_id);
            request_shipping(signals, false);
        };
        return view! { <div class="outbound-load-dialog-backdrop"><section class="outbound-load-dialog wide" role="dialog" aria-modal="true" aria-labelledby="plan-load-title"><header><div><span class="eyebrow">"Supervisor planning"</span><h2 id="plan-load-title">"Plan outbound load"</h2></div><button class="icon-button" type="button" aria-label="Close" on:click=close><Icon icon=UiIcon::Close/></button></header>
            <fieldset class="outbound-load-plan-fields" disabled=move || signals.command_pending.get()>
                <div class="outbound-load-form-grid"><label><span>"Load reference"</span><input prop:value=move || drafts.reference.get() on:input=move |event| drafts.reference.set(event_target_value(&event))/></label><label><span>"Carrier code"</span><input prop:value=move || drafts.carrier.get() on:input=move |event| drafts.carrier.set(event_target_value(&event))/></label><label><span>"Staging lane"</span><select on:change=select_staging><option value="">"Select staging lane"</option>{locations.with_value(|all| all.iter().filter(|location| is_staging_location(location)).map(|location| view! { <option value=location.id>{format!("{} · {}", location.facility_name.clone().unwrap_or_default(), location.name.clone().unwrap_or_else(|| location.barcode.clone().unwrap_or_default()))}</option> }).collect_view())}</select></label><label><span>"Scheduled departure"</span><input type="datetime-local" prop:value=move || drafts.scheduled_at.get() on:input=move |event| drafts.scheduled_at.set(event_target_value(&event))/></label></div>
                <div class="outbound-load-shipment-picker">
                    <header><h3>"Manifested shipments"</h3><span>{move || format!("{} selected", drafts.selected_shipments.get().len())}</span></header>
                    <div class="outbound-loads-table-scroll" aria-busy=move || signals.shipping_pending.get().to_string()><table><caption class="sr-only">"Manifested shipments eligible for this outbound load"</caption><thead><tr><th><span class="sr-only">"Select"</span></th><th>"Order"</th><th>"Client"</th><th>"Facility"</th><th>"Carrier"</th><th>"Cartons"</th></tr></thead><tbody>
                        {move || {
                            signals.shipping_entries.get().into_iter().filter_map(|entry| {
                                let shipment = entry.shipment?;
                                (shipment.status == ShipmentStatus::Manifested).then(|| {
                                    let shipment_id = shipment.shipment_id;
                                    view! { <tr><td><input type="checkbox" aria-label=format!("Select shipment {}", shipment_id) checked=move || drafts.selected_shipments.get().contains(&shipment_id) on:change=move |event| toggle_id(drafts.selected_shipments, shipment_id, event_target_checked(&event))/></td><td class="mono">{entry.order_key}</td><td>{entry.inventory_owner_name}</td><td>{entry.facility_name}</td><td>{shipment.carrier_code.unwrap_or_else(|| "Unassigned".into())}</td><td>{shipment.carton_count}</td></tr> }
                                })
                            }).collect_view()
                        }}
                    </tbody></table></div>
                    {move || signals.shipping_next_cursor.get().map(|_| view! { <button class="button quiet-action outbound-loads-more" type="button" disabled=move || signals.shipping_pending.get() on:click=move |_| request_shipping(signals, true)>"Load more shipments"</button> })}
                </div>
            </fieldset>
            <footer><button class="button quiet-action" type="button" on:click=close>"Cancel"</button><button class="button primary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| submit_plan.run(())>"Plan load"</button></footer></section></div> }.into_any();
    }
    let title = match dialog {
        Dialog::Start => "Start loading",
        Dialog::Complete => "Complete and seal",
        Dialog::Depart => "Confirm load departure",
        Dialog::Cancel => "Cancel outbound load",
        Dialog::Plan => unreachable!(),
    };
    view! { <div class="outbound-load-dialog-backdrop"><section class="outbound-load-dialog" role="dialog" aria-modal="true" aria-labelledby="load-command-title"><header><div><span class="eyebrow">"Load command"</span><h2 id="load-command-title">{title}</h2></div><button class="icon-button" type="button" aria-label="Close" on:click=close><Icon icon=UiIcon::Close/></button></header>
        <fieldset class="outbound-load-command-fields" disabled=move || signals.command_pending.get()>{if dialog == Dialog::Cancel { view! { <div class="outbound-load-form-grid single"><label><span>"Reason"</span><select on:change=move |event| drafts.cancellation_reason.set(parse_cancellation_reason(&event_target_value(&event)))><option value="planning_error">"Planning error"</option><option value="route_cancelled">"Route cancelled"</option><option value="carrier_cancelled">"Carrier cancelled"</option><option value="equipment_unavailable">"Equipment unavailable"</option><option value="other">"Other"</option></select></label><label><span>"Note"</span><textarea maxlength="500" prop:value=move || drafts.cancellation_note.get() on:input=move |event| drafts.cancellation_note.set(event_target_value(&event))></textarea></label></div> }.into_any() } else { view! { <div class="outbound-load-form-grid single"><label><span>"Dock barcode"</span><input prop:value=move || drafts.dock_barcode.get() on:input=move |event| drafts.dock_barcode.set(event_target_value(&event))/></label><label><span>"Trailer number"</span><input prop:value=move || drafts.trailer.get() on:input=move |event| drafts.trailer.set(event_target_value(&event))/></label>{matches!(dialog, Dialog::Complete | Dialog::Depart).then(|| view! { <label><span>"Seal number"</span><input prop:value=move || drafts.seal.get() on:input=move |event| drafts.seal.set(event_target_value(&event))/></label> })}</div> }.into_any() }}</fieldset>
        <footer><button class="button quiet-action" type="button" on:click=close>"Back"</button><button class=if dialog == Dialog::Cancel { "button danger-action" } else { "button primary-action" } type="button" disabled=move || signals.command_pending.get() on:click=move |_| submit_phase.run(dialog)>{title}</button></footer>
    </section></div> }.into_any()
}

fn open_plan(drafts: Drafts, signals: Signals) {
    drafts.reference.set(String::new());
    drafts.carrier.set(String::new());
    drafts.staging_id.set(None);
    drafts.scheduled_at.set(String::new());
    drafts.selected_shipments.set(Vec::new());
    invalidate_shipping(signals, None);
    request_shipping(signals, false);
    signals.dialog.set(Some(Dialog::Plan));
}

fn prepare_plan(drafts: Drafts, signals: Signals) {
    let reference = drafts.reference.get_untracked().trim().to_owned();
    let carrier = drafts.carrier.get_untracked().trim().to_owned();
    let selected = drafts.selected_shipments.get_untracked();
    let Some(staging_location_id) = drafts.staging_id.get_untracked() else {
        set_error(signals, "Select a staging lane.");
        return;
    };
    if reference.is_empty() || carrier.is_empty() || selected.is_empty() {
        set_error(
            signals,
            "Load reference, carrier, and at least one shipment are required.",
        );
        return;
    }
    let entries = signals
        .shipping_entries
        .get_untracked()
        .into_iter()
        .filter(|entry| {
            entry
                .shipment
                .as_ref()
                .is_some_and(|shipment| selected.contains(&shipment.shipment_id))
        })
        .collect::<Vec<_>>();
    if entries.len() != selected.len() {
        set_error(signals, "One selected shipment is no longer available.");
        return;
    }
    let Some(facility_id) = entries.first().map(|entry| entry.facility_id) else {
        return;
    };
    if entries.iter().any(|entry| entry.facility_id != facility_id) {
        set_error(
            signals,
            "Every shipment on a load must use the same facility.",
        );
        return;
    }
    if entries.iter().any(|entry| {
        entry
            .shipment
            .as_ref()
            .and_then(|shipment| shipment.carrier_code.as_deref())
            .is_none_or(|value| !value.eq_ignore_ascii_case(&carrier))
    }) {
        set_error(
            signals,
            "Every selected shipment must use the load carrier.",
        );
        return;
    }
    signals.command_pending.set(true);
    signals.error.set(false);
    signals
        .message
        .set("Loading exact shipment plans...".into());
    let scheduled_departure_at = parse_local_timestamp(&drafts.scheduled_at.get_untracked());
    leptos::task::spawn_local(async move {
        let mut shipments = Vec::with_capacity(entries.len());
        let mut next_carton_sequence = 1_u32;
        for (index, entry) in entries.into_iter().enumerate() {
            let Some(summary) = entry.shipment else {
                continue;
            };
            let shipment = match api::internal_get::<ShipmentResponse>(&format!(
                "/api/v1/shipments/{}",
                summary.shipment_id
            ))
            .await
            {
                Ok(value) => value,
                Err(error) => {
                    signals.command_pending.set(false);
                    if error.unauthorized {
                        signals.on_unauthorized.run(());
                    } else {
                        set_error(signals, error.message);
                    }
                    return;
                }
            };
            let cartons = shipment
                .cartons
                .into_iter()
                .map(|carton| {
                    let request = PlanOutboundLoadCartonRequest {
                        carton_id: carton.carton_id,
                        load_sequence: next_carton_sequence,
                    };
                    next_carton_sequence = next_carton_sequence.saturating_add(1);
                    request
                })
                .collect();
            shipments.push(PlanOutboundLoadShipmentRequest {
                shipment_id: shipment.shipment_id,
                expected_shipment_revision: shipment.revision,
                expected_order_revision: shipment.order_revision,
                shipment_sequence: u32::try_from(index + 1).unwrap_or(u32::MAX),
                cartons,
            });
        }
        signals.command_pending.set(false);
        dispatch(
            PendingCommand::Plan {
                request: PlanOutboundLoadRequest {
                    facility_id,
                    load_reference: reference,
                    carrier_code: carrier,
                    staging_location_id,
                    scheduled_departure_at,
                    shipments,
                },
                key: api::new_idempotency_key(),
            },
            signals,
        );
    });
}

fn dispatch(command: PendingCommand, signals: Signals) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.retry.set(None);
    signals.error.set(false);
    signals.message.set(command_label(&command).into());
    leptos::task::spawn_local(async move {
        let result = match &command {
            PendingCommand::Plan { request, key } => api::internal_post_idempotent::<
                _,
                PlanOutboundLoadResponse,
            >(
                "/api/v1/outbound-loads", request, key
            )
            .await
            .map(|response| response.outbound_load),
            PendingCommand::Release {
                load_id,
                request,
                key,
            } => match api::internal_post_idempotent::<_, ReleaseOutboundLoadResponse>(
                &format!("/api/v1/outbound-loads/{load_id}/releases"),
                request,
                key,
            )
            .await
            {
                Ok(_) => refresh_result(*load_id).await,
                Err(error) => Err(error),
            },
            PendingCommand::Start {
                load_id,
                request,
                key,
            } => match api::internal_post_idempotent::<_, StartOutboundLoadLoadingResponse>(
                &format!("/api/v1/outbound-loads/{load_id}/loading-starts"),
                request,
                key,
            )
            .await
            {
                Ok(_) => refresh_result(*load_id).await,
                Err(error) => Err(error),
            },
            PendingCommand::Complete {
                load_id,
                request,
                key,
            } => match api::internal_post_idempotent::<_, CompleteOutboundLoadLoadingResponse>(
                &format!("/api/v1/outbound-loads/{load_id}/loading-completions"),
                request,
                key,
            )
            .await
            {
                Ok(_) => refresh_result(*load_id).await,
                Err(error) => Err(error),
            },
            PendingCommand::Depart {
                load_id,
                request,
                key,
            } => match api::internal_post_idempotent::<_, ConfirmOutboundLoadDepartureResponse>(
                &format!("/api/v1/outbound-loads/{load_id}/departures"),
                request,
                key,
            )
            .await
            {
                Ok(_) => refresh_result(*load_id).await,
                Err(error) => Err(error),
            },
            PendingCommand::Cancel {
                load_id,
                request,
                key,
            } => match api::internal_post_idempotent::<
                _,
                wareboxes_api_contract::v1::CancelOutboundLoadResponse,
            >(
                &format!("/api/v1/outbound-loads/{load_id}/cancellations"),
                request,
                key,
            )
            .await
            {
                Ok(_) => refresh_result(*load_id).await,
                Err(error) => Err(error),
            },
        };
        signals.command_pending.set(false);
        match result {
            Ok(load) => {
                let planned_shipments = load
                    .shipments
                    .iter()
                    .map(|shipment| shipment.shipment_id)
                    .collect::<Vec<_>>();
                signals.shipping_entries.update(|entries| {
                    entries.retain(|entry| {
                        entry.shipment.as_ref().is_none_or(|shipment| {
                            !planned_shipments.contains(&shipment.shipment_id)
                        })
                    });
                });
                signals.dialog.set(None);
                signals.selected_id.set(Some(load.outbound_load_id));
                signals.detail.set(Some(load));
                signals.message.set("Load state is current.".into());
                signals.toasts.success(command_success_label(&command));
                request_queue(signals, false);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) if error.ambiguous_outcome => {
                signals.retry.set(Some(command));
                signals.dialog.set(None);
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
                if let Some(load_id) = signals.selected_id.get_untracked() {
                    load_detail(load_id, signals);
                }
                request_queue(signals, false);
            }
        }
    });
}

async fn refresh_result(load_id: i64) -> Result<OutboundLoadResponse, api::ApiError> {
    api::internal_get(&format!("/api/v1/outbound-loads/{load_id}")).await
}

fn load_detail(load_id: i64, signals: Signals) {
    let generation = signals.detail_generation.get_untracked().saturating_add(1);
    signals.detail_generation.set(generation);
    signals.detail.set(None);
    signals.message.set("Loading outbound load...".into());
    leptos::task::spawn_local(async move {
        let result =
            api::internal_get::<OutboundLoadResponse>(&format!("/api/v1/outbound-loads/{load_id}"))
                .await;
        if !accept_detail_response(
            signals.detail_generation.get_untracked(),
            generation,
            signals.selected_id.get_untracked(),
            load_id,
        ) {
            return;
        }
        match result {
            Ok(load) if load.outbound_load_id == load_id => {
                signals.detail.set(Some(load));
                signals.error.set(false);
                signals.message.set("Load state is current.".into());
            }
            Ok(_) => set_error(signals, "The load response did not match the selection."),
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => set_error(signals, error.message),
        }
    });
}

fn request_queue(signals: Signals, append: bool) {
    if signals.queue_pending.get_untracked() {
        return;
    }
    let cursor = append
        .then(|| signals.next_cursor.get_untracked())
        .flatten();
    if append && cursor.is_none() {
        return;
    }
    signals.queue_pending.set(true);
    let generation = signals.queue_generation.get_untracked().saturating_add(1);
    signals.queue_generation.set(generation);
    let facility_id = signals.facility_id.get_untracked();
    let status = signals.status.get_untracked();
    let sort = signals.sort.get_untracked();
    let request_identity = QueueRequestIdentity {
        generation,
        facility_id,
        status,
        sort,
    };
    leptos::task::spawn_local(async move {
        let path = queue_path(facility_id, status, sort, cursor.as_ref());
        let result = api::internal_get::<OutboundLoadQueuePage>(&path).await;
        if !accept_queue_response(current_queue_identity(signals), request_identity) {
            return;
        }
        signals.queue_pending.set(false);
        match result {
            Ok(page) => {
                if append {
                    signals.entries.update(|entries| {
                        for item in page.items {
                            if !entries
                                .iter()
                                .any(|entry| entry.outbound_load_id == item.outbound_load_id)
                            {
                                entries.push(item);
                            }
                        }
                    });
                } else {
                    signals.entries.set(page.items);
                }
                signals.next_cursor.set(page.next_cursor);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => set_error(signals, error.message),
        }
    });
}

fn request_shipping(signals: Signals, append: bool) {
    if signals.shipping_pending.get_untracked() {
        return;
    }
    let cursor = append
        .then(|| signals.shipping_next_cursor.get_untracked())
        .flatten();
    if append && cursor.is_none() {
        return;
    }
    signals.shipping_pending.set(true);
    let generation = signals
        .shipping_generation
        .get_untracked()
        .saturating_add(1);
    signals.shipping_generation.set(generation);
    let facility_id = signals.shipping_facility_id.get_untracked();
    leptos::task::spawn_local(async move {
        let path = shipping_queue_path(facility_id, cursor.as_ref());
        let result = api::internal_get::<ShippingQueuePage>(&path).await;
        if !accept_shipping_response(
            signals.shipping_generation.get_untracked(),
            generation,
            signals.shipping_facility_id.get_untracked(),
            facility_id,
        ) {
            return;
        }
        signals.shipping_pending.set(false);
        match result {
            Ok(page) => {
                if append {
                    signals.shipping_entries.update(|entries| {
                        for item in page.items {
                            if !entries.iter().any(|entry| entry.order_id == item.order_id) {
                                entries.push(item);
                            }
                        }
                    });
                } else {
                    signals.shipping_entries.set(page.items);
                }
                signals.shipping_next_cursor.set(page.next_cursor);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => set_error(signals, error.message),
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn install_poll(signals: Signals) {
    let Some(owner) = Owner::current() else {
        return;
    };
    let Ok(handle) = set_interval_with_handle(
        move || {
            if signals.dialog.get_untracked().is_none()
                && !signals.command_pending.get_untracked()
                && !signals.queue_pending.get_untracked()
            {
                owner.with(|| request_queue(signals, false));
            }
        },
        Duration::from_secs(15),
    ) else {
        return;
    };
    on_cleanup(move || handle.clear());
}

fn invalidate_queue(signals: Signals) {
    signals
        .queue_generation
        .update(|generation| *generation = generation.saturating_add(1));
    signals.queue_pending.set(false);
    signals.entries.set(Vec::new());
    signals.next_cursor.set(None);
}
fn invalidate_shipping(signals: Signals, facility_id: Option<i64>) {
    signals
        .shipping_generation
        .update(|generation| *generation = generation.saturating_add(1));
    signals.shipping_facility_id.set(facility_id);
    signals.shipping_pending.set(false);
    signals.shipping_entries.set(Vec::new());
    signals.shipping_next_cursor.set(None);
}
fn accept_detail_response(
    current_generation: u64,
    response_generation: u64,
    selected_id: Option<i64>,
    requested_id: i64,
) -> bool {
    current_generation == response_generation && selected_id == Some(requested_id)
}
fn current_queue_identity(signals: Signals) -> QueueRequestIdentity {
    QueueRequestIdentity {
        generation: signals.queue_generation.get_untracked(),
        facility_id: signals.facility_id.get_untracked(),
        status: signals.status.get_untracked(),
        sort: signals.sort.get_untracked(),
    }
}
fn accept_queue_response(current: QueueRequestIdentity, requested: QueueRequestIdentity) -> bool {
    current == requested
}
fn accept_shipping_response(
    current_generation: u64,
    response_generation: u64,
    current_facility: Option<i64>,
    requested_facility: Option<i64>,
) -> bool {
    current_generation == response_generation && current_facility == requested_facility
}
fn set_error(signals: Signals, message: impl Into<String>) {
    signals.error.set(true);
    signals.message.set(message.into());
}
fn toggle_id(signal: RwSignal<Vec<i64>>, id: i64, selected: bool) {
    signal.update(|ids| {
        ids.retain(|value| *value != id);
        if selected {
            ids.push(id);
        }
    });
}
fn parse_optional_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}
fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn compact_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
fn parse_local_timestamp(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| format!("{value}:00Z"))
}
fn is_staging_location(location: &Location) -> bool {
    location.deleted.is_none()
        && location.active
        && !location.pickable
        && !location.receivable
        && location.barcode.is_some()
        && location.r#type.eq_ignore_ascii_case("staging")
}
fn position_label(
    state: &wareboxes_api_contract::v1::PackedCartonPositionStateResponse,
) -> &'static str {
    match state {
        wareboxes_api_contract::v1::PackedCartonPositionStateResponse::Packed { .. } => "Planned",
        wareboxes_api_contract::v1::PackedCartonPositionStateResponse::Staged { .. } => "Staged",
        wareboxes_api_contract::v1::PackedCartonPositionStateResponse::Loaded { .. } => "Loaded",
        wareboxes_api_contract::v1::PackedCartonPositionStateResponse::Departed { .. } => {
            "Departed"
        }
    }
}
fn parse_status(value: &str) -> Option<OutboundLoadStatus> {
    match value {
        "planned" => Some(OutboundLoadStatus::Planned),
        "staging" => Some(OutboundLoadStatus::Staging),
        "loading" => Some(OutboundLoadStatus::Loading),
        "ready_to_depart" => Some(OutboundLoadStatus::ReadyToDepart),
        "departed" => Some(OutboundLoadStatus::Departed),
        "cancelled" => Some(OutboundLoadStatus::Cancelled),
        _ => None,
    }
}
fn status_label(status: OutboundLoadStatus) -> &'static str {
    match status {
        OutboundLoadStatus::Planned => "Planned",
        OutboundLoadStatus::Staging => "Staging",
        OutboundLoadStatus::Loading => "Loading",
        OutboundLoadStatus::ReadyToDepart => "Ready",
        OutboundLoadStatus::Departed => "Departed",
        OutboundLoadStatus::Cancelled => "Cancelled",
    }
}
fn status_class(status: OutboundLoadStatus) -> &'static str {
    match status {
        OutboundLoadStatus::Planned => "neutral",
        OutboundLoadStatus::Staging => "warning",
        OutboundLoadStatus::Loading => "active",
        OutboundLoadStatus::ReadyToDepart => "success",
        OutboundLoadStatus::Departed => "success",
        OutboundLoadStatus::Cancelled => "danger",
    }
}

fn can_cancel_load(status: OutboundLoadStatus, staged: u32, loaded: u32) -> bool {
    status == OutboundLoadStatus::Planned
        || (matches!(
            status,
            OutboundLoadStatus::Staging | OutboundLoadStatus::Loading
        ) && staged == 0
            && loaded == 0)
}
fn parse_cancellation_reason(value: &str) -> OutboundLoadCancellationReason {
    match value {
        "route_cancelled" => OutboundLoadCancellationReason::RouteCancelled,
        "carrier_cancelled" => OutboundLoadCancellationReason::CarrierCancelled,
        "equipment_unavailable" => OutboundLoadCancellationReason::EquipmentUnavailable,
        "other" => OutboundLoadCancellationReason::Other,
        _ => OutboundLoadCancellationReason::PlanningError,
    }
}
fn command_label(command: &PendingCommand) -> &'static str {
    match command {
        PendingCommand::Plan { .. } => "Planning outbound load...",
        PendingCommand::Release { .. } => "Releasing load to staging...",
        PendingCommand::Start { .. } => "Starting trailer loading...",
        PendingCommand::Complete { .. } => "Sealing outbound load...",
        PendingCommand::Depart { .. } => "Confirming load departure...",
        PendingCommand::Cancel { .. } => "Cancelling outbound load...",
    }
}
fn command_success_label(command: &PendingCommand) -> &'static str {
    match command {
        PendingCommand::Plan { .. } => "Outbound load planned.",
        PendingCommand::Release { .. } => "Load released to staging.",
        PendingCommand::Start { .. } => "Trailer loading started.",
        PendingCommand::Complete { .. } => "Load sealed and ready to depart.",
        PendingCommand::Depart { .. } => "Outbound load departed.",
        PendingCommand::Cancel { .. } => "Outbound load cancelled.",
    }
}
fn queue_path(
    facility_id: Option<i64>,
    status: Option<OutboundLoadStatus>,
    sort: SortSpec<LoadSort>,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/outbound-loads?limit=100&sort={}&direction={}",
        load_sort_wire(sort.key),
        sort_direction_wire(sort.direction),
    );
    if let Some(id) = facility_id {
        path.push_str(&format!("&facility_id={id}"));
    }
    if let Some(status) = status {
        path.push_str("&status=");
        path.push_str(match status {
            OutboundLoadStatus::Planned => "planned",
            OutboundLoadStatus::Staging => "staging",
            OutboundLoadStatus::Loading => "loading",
            OutboundLoadStatus::ReadyToDepart => "ready_to_depart",
            OutboundLoadStatus::Departed => "departed",
            OutboundLoadStatus::Cancelled => "cancelled",
        });
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}
const fn load_sort_wire(sort: LoadSort) -> &'static str {
    match sort {
        LoadSort::Reference => "reference",
        LoadSort::Status => "status",
        LoadSort::Progress => "progress",
        LoadSort::Facility => "facility",
        LoadSort::Trailer => "trailer",
        LoadSort::ScheduledDeparture => "scheduled_departure",
    }
}
const fn sort_direction_wire(direction: SortDirection) -> &'static str {
    match direction {
        SortDirection::Ascending => "ascending",
        SortDirection::Descending => "descending",
    }
}
fn shipping_queue_path(facility_id: Option<i64>, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = "/api/v1/shipping-queue?limit=100".to_owned();
    if let Some(id) = facility_id {
        path.push_str(&format!("&facility_id={id}"));
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_path_binds_scope_state_and_cursor() {
        let sort = SortSpec {
            key: LoadSort::Progress,
            direction: SortDirection::Descending,
        };
        let cursor = OpaqueCursor::new("ol2.a.a.a.a.p.d.0000000000000064").unwrap();
        let path = queue_path(
            Some(4),
            Some(OutboundLoadStatus::Loading),
            sort,
            Some(&cursor),
        );
        assert!(path.contains("facility_id=4"));
        assert!(path.contains("status=loading"));
        assert!(path.contains("sort=progress"));
        assert!(path.contains("direction=descending"));
        assert!(path.contains("cursor=ol2."));
    }
    #[test]
    fn async_responses_are_bound_to_selection_generation_and_filters() {
        let sort = SortSpec {
            key: LoadSort::ScheduledDeparture,
            direction: SortDirection::Ascending,
        };
        let requested = QueueRequestIdentity {
            generation: 8,
            facility_id: Some(3),
            status: Some(OutboundLoadStatus::Loading),
            sort,
        };
        assert!(accept_detail_response(4, 4, Some(22), 22));
        assert!(!accept_detail_response(5, 4, Some(22), 22));
        assert!(!accept_detail_response(4, 4, Some(23), 22));
        assert!(accept_queue_response(requested, requested));
        assert!(!accept_queue_response(
            QueueRequestIdentity {
                generation: 9,
                ..requested
            },
            requested,
        ));
        assert!(!accept_queue_response(
            QueueRequestIdentity {
                sort: SortSpec {
                    key: LoadSort::Progress,
                    direction: SortDirection::Descending,
                },
                ..requested
            },
            requested,
        ));
        assert!(!accept_shipping_response(3, 2, Some(7), Some(7)));
        assert!(!accept_shipping_response(3, 3, Some(8), Some(7)));
    }
    #[test]
    fn shipment_page_path_binds_the_selected_staging_facility() {
        let cursor = OpaqueCursor::new("sq1.a.0.n.0000000000000001").unwrap();
        let path = shipping_queue_path(Some(7), Some(&cursor));
        assert!(path.contains("facility_id=7"));
        assert!(path.contains("cursor=sq1."));
        assert!(!shipping_queue_path(None, None).contains("facility_id="));
    }
    #[test]
    fn staging_location_requires_typed_active_scannable_lane() {
        let location: Location = serde_json::from_value(serde_json::json!({
            "id": 1,
            "tenant_id": 1,
            "created": "2026-08-08T00:00:00Z",
            "deleted": null,
            "facility_id": 2,
            "facility_name": "Reno",
            "parent_location_id": null,
            "barcode": "STAGE",
            "name": "Stage",
            "type": "staging",
            "active": true,
            "pickable": false,
            "receivable": false
        }))
        .unwrap();
        assert!(is_staging_location(&location));
    }
    #[test]
    fn other_cancellation_reason_is_typed() {
        assert_eq!(
            parse_cancellation_reason("other"),
            OutboundLoadCancellationReason::Other
        );
        assert_eq!(
            parse_cancellation_reason("bad"),
            OutboundLoadCancellationReason::PlanningError
        );
    }

    #[test]
    fn restored_loading_load_can_be_cancelled() {
        assert!(can_cancel_load(OutboundLoadStatus::Loading, 0, 0));
        assert!(!can_cancel_load(OutboundLoadStatus::Loading, 1, 0));
        assert!(!can_cancel_load(OutboundLoadStatus::Loading, 0, 1));
        assert!(!can_cancel_load(OutboundLoadStatus::ReadyToDepart, 0, 0));
    }
}
