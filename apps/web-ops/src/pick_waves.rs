use leptos::prelude::*;
use lucide_leptos::{Eye, Play, Plus, RefreshCw, RotateCcw, Trash2, X};
use wareboxes_api_contract::v1::{
    CancelPickWaveRequest, OpaqueCursor, PickWaveCancellationReason, PickWavePage,
    PickWaveResponse, PickWaveSort, PickWaveSortDirection, PickWaveStatus,
    PlanPickWaveOrderRequest, PlanPickWaveRequest, ReleasePickWaveRequest, Revision,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::dto::OrderPage;
use wareboxes_core::models::{Location, Order, OrderStatus};

use crate::api;
use crate::sorting::{SortDirection, SortableHeader};
use crate::toast::{use_toast_bus, ToastBus};
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, Copy)]
struct Signals {
    page: RwSignal<PickWavePage>,
    selected: RwSignal<Option<PickWaveResponse>>,
    loading: RwSignal<bool>,
    detail_loading: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    facility_id: RwSignal<Option<i64>>,
    status: RwSignal<Option<PickWaveStatus>>,
    sort: RwSignal<PickWaveSort>,
    direction: RwSignal<PickWaveSortDirection>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    cursor_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    generation: RwSignal<u64>,
    detail_generation: RwSignal<u64>,
    dialog: RwSignal<Option<Dialog>>,
    retry: RwSignal<Option<SavedCommand>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialog {
    Plan,
    Cancel,
}

#[derive(Clone)]
enum SavedCommand {
    Plan {
        request: PlanPickWaveRequest,
        key: String,
    },
    Release {
        wave_id: i64,
        request: ReleasePickWaveRequest,
        key: String,
    },
    Cancel {
        wave_id: i64,
        request: CancelPickWaveRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
struct Drafts {
    name: RwSignal<String>,
    facility_id: RwSignal<Option<i64>>,
    destination_id: RwSignal<Option<i64>>,
    selected_orders: RwSignal<Vec<i64>>,
    cancellation_reason: RwSignal<PickWaveCancellationReason>,
    cancellation_note: RwSignal<String>,
}

#[component]
pub(crate) fn PickWavesWorkspace(
    initial_page: PickWavePage,
    initial_orders: OrderPage,
    access: AccessScopeWorkspace,
    locations: Vec<Location>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let signals = Signals {
        page: RwSignal::new(initial_page),
        selected: RwSignal::new(None),
        loading: RwSignal::new(false),
        detail_loading: RwSignal::new(false),
        command_pending: RwSignal::new(false),
        error: RwSignal::new(None),
        facility_id: RwSignal::new(None),
        status: RwSignal::new(None),
        sort: RwSignal::new(PickWaveSort::PlannedAt),
        direction: RwSignal::new(PickWaveSortDirection::Desc),
        cursor: RwSignal::new(None),
        cursor_history: RwSignal::new(Vec::new()),
        generation: RwSignal::new(0),
        detail_generation: RwSignal::new(0),
        dialog: RwSignal::new(None),
        retry: RwSignal::new(None),
    };
    let drafts = Drafts {
        name: RwSignal::new(String::new()),
        facility_id: RwSignal::new(None),
        destination_id: RwSignal::new(None),
        selected_orders: RwSignal::new(Vec::new()),
        cancellation_reason: RwSignal::new(PickWaveCancellationReason::OperationalChange),
        cancellation_note: RwSignal::new(String::new()),
    };
    let layout = SplitPaneState::new("pick-waves", 690);
    let facilities = StoredValue::new(access.facilities);
    let locations = StoredValue::new(locations);
    let orders = StoredValue::new(initial_orders.page.items);
    let toasts = use_toast_bus();

    let refresh = move |_| {
        signals.cursor.set(None);
        signals.cursor_history.set(Vec::new());
        request_page(signals, on_unauthorized);
    };
    let open_plan = move |_| {
        drafts.name.set(String::new());
        drafts.facility_id.set(signals.facility_id.get_untracked());
        drafts.destination_id.set(None);
        drafts.selected_orders.set(Vec::new());
        signals.error.set(None);
        signals.dialog.set(Some(Dialog::Plan));
    };
    let submit_plan = Callback::new(move |_| {
        let Some(request) = build_plan_request(drafts, orders) else {
            signals.error.set(Some(
                "Name, facility, staging destination, and at least one order are required."
                    .to_owned(),
            ));
            return;
        };
        dispatch(
            SavedCommand::Plan {
                request,
                key: api::new_idempotency_key(),
            },
            signals,
            toasts,
            on_unauthorized,
        );
    });
    let submit_release = Callback::new(move |wave: PickWaveResponse| {
        dispatch(
            SavedCommand::Release {
                wave_id: wave.wave_id,
                request: ReleasePickWaveRequest {
                    expected_revision: wave.revision,
                },
                key: api::new_idempotency_key(),
            },
            signals,
            toasts,
            on_unauthorized,
        );
    });
    let open_cancel = Callback::new(move |_| {
        drafts
            .cancellation_reason
            .set(PickWaveCancellationReason::OperationalChange);
        drafts.cancellation_note.set(String::new());
        signals.error.set(None);
        signals.dialog.set(Some(Dialog::Cancel));
    });
    let submit_cancel = Callback::new(move |_| {
        let Some(wave) = signals.selected.get_untracked() else {
            return;
        };
        let note = drafts.cancellation_note.get_untracked().trim().to_owned();
        let reason = drafts.cancellation_reason.get_untracked();
        if reason == PickWaveCancellationReason::Other && note.is_empty() {
            signals
                .error
                .set(Some("A note is required for Other.".to_owned()));
            return;
        }
        dispatch(
            SavedCommand::Cancel {
                wave_id: wave.wave_id,
                request: CancelPickWaveRequest {
                    expected_revision: wave.revision,
                    reason,
                    note: (!note.is_empty()).then_some(note),
                },
                key: api::new_idempotency_key(),
            },
            signals,
            toasts,
            on_unauthorized,
        );
    });

    view! {
        <section class="pick-wave-page operations-page">
            <header class="page-heading pick-wave-heading">
                <div><span class="eyebrow">"Outbound execution"</span><h1>"Pick waves"</h1></div>
                <div class="page-actions">
                    <button type="button" class="icon-button" title="Refresh waves" aria-label="Refresh waves" disabled=move || signals.loading.get() on:click=refresh><RefreshCw size=16/></button>
                    <button type="button" class="button primary-action" disabled=move || signals.command_pending.get() on:click=open_plan><Plus size=15/>"Plan wave"</button>
                </div>
            </header>
            <Show when=move || signals.retry.get().is_some()>
                <div class="pick-wave-retry" role="status"><span>"The last command outcome is unknown."</span><button type="button" class="button secondary-action" disabled=move || signals.command_pending.get() on:click=move |_| { if let Some(command) = signals.retry.get_untracked() { dispatch(command, signals, toasts, on_unauthorized); } }><RotateCcw size=14/>"Retry exact command"</button></div>
            </Show>
            <div class="pick-wave-workspace split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
                <section class="pick-wave-master split-master data-section">
                    <div class="pick-wave-toolbar">
                        <div class="toolbar-summary"><strong>{move || signals.page.get().items.len()}</strong><span>"waves on page"</span><PaneControls layout master_label="wave list" detail_label="wave detail"/></div>
                        <label><span class="sr-only">"Facility"</span><select prop:value=move || signals.facility_id.get().map_or_else(String::new, |id| id.to_string()) on:change=move |event| { signals.facility_id.set(parse_optional_id(&event_target_value(&event))); signals.cursor.set(None); signals.cursor_history.set(Vec::new()); request_page(signals, on_unauthorized); }><option value="">"All facilities"</option>{facilities.with_value(|items| items.iter().map(|facility| view! { <option value=facility.id>{facility.name.clone()}</option> }).collect_view())}</select></label>
                        <label><span class="sr-only">"Wave status"</span><select on:change=move |event| { signals.status.set(parse_status(&event_target_value(&event))); signals.cursor.set(None); signals.cursor_history.set(Vec::new()); request_page(signals, on_unauthorized); }><option value="">"All states"</option><option value="planned">"Planned"</option><option value="released">"Released"</option><option value="cancelled">"Cancelled"</option></select></label>
                    </div>
                    <div class="table-scroll pick-wave-table-scroll"><table class="data-table pick-wave-table"><caption class="sr-only">"Pick waves matching the active scope and state filters"</caption><thead><tr>
                        <SortableHeader label="Wave" active=move || signals.sort.get() == PickWaveSort::Name direction=move || ui_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals, PickWaveSort::Name, on_unauthorized))/>
                        <SortableHeader label="State" active=move || signals.sort.get() == PickWaveSort::Status direction=move || ui_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals, PickWaveSort::Status, on_unauthorized))/>
                        <SortableHeader label="Orders" active=move || signals.sort.get() == PickWaveSort::Orders direction=move || ui_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals, PickWaveSort::Orders, on_unauthorized)) numeric=true/>
                        <SortableHeader label="Tasks" active=move || signals.sort.get() == PickWaveSort::Tasks direction=move || ui_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals, PickWaveSort::Tasks, on_unauthorized)) numeric=true/>
                        <SortableHeader label="Units" active=move || signals.sort.get() == PickWaveSort::Units direction=move || ui_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals, PickWaveSort::Units, on_unauthorized)) numeric=true/>
                        <SortableHeader label="Planned" active=move || signals.sort.get() == PickWaveSort::PlannedAt direction=move || ui_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals, PickWaveSort::PlannedAt, on_unauthorized))/>
                    </tr></thead><tbody>{move || { let entries=signals.page.get().items; if entries.is_empty() && !signals.loading.get() { view! { <tr><td colspan="6" class="table-empty-row">"No pick waves match this view."</td></tr> }.into_any() } else { entries.into_iter().map(|wave| { let row=wave.clone(); let button_row=row.clone(); let selected_id=wave.wave_id; let detail_label=format!("View details for wave {}", wave.name); view! { <tr class:selected=move || signals.selected.get().is_some_and(|selected| selected.wave_id == selected_id) on:click=move |_| { layout.show_detail(); request_detail(signals, row.wave_id, on_unauthorized); }><td><div class="pick-wave-name-cell"><span><strong>{wave.name}</strong><small class="cell-detail">{format!("Wave #{} · {}", wave.wave_id, wave.destination_location_name)}</small></span><button type="button" class="pick-wave-detail-button" title="View wave details" aria-label=detail_label on:click=move |event| { event.stop_propagation(); layout.show_detail(); request_detail(signals, button_row.wave_id, on_unauthorized); }><Eye size=13/></button></div></td><td><span class=wave_status_class(wave.status)>{wave_status_label(wave.status)}</span></td><td class="numeric">{wave.order_count}</td><td class="numeric">{wave.pick_task_count}</td><td class="numeric">{wave.released_quantity}</td><td>{compact_time(&wave.planned_at)}</td></tr> }}).collect_view().into_any() } }}</tbody></table></div>
                    <Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show>
                    <footer class="table-footer"><span>{move || if signals.loading.get() { "Loading waves..." } else { "Server-sorted results" }}</span><button type="button" class="button secondary-action" disabled=move || signals.loading.get() || signals.cursor_history.get().is_empty() on:click=move |_| previous_page(signals, on_unauthorized)>"Previous"</button><button type="button" class="button secondary-action" disabled=move || signals.loading.get() || !signals.page.get().has_more() on:click=move |_| next_page(signals, on_unauthorized)>"Next"</button></footer>
                </section>
                <SplitPaneHandle layout/>
                <section class="pick-wave-detail split-detail"><WaveDetail signals submit_release open_cancel/></section>
            </div>
        </section>
        <Show when=move || signals.dialog.get().is_some()>{move || signals.dialog.get().map(|dialog| view! { <WaveDialog dialog signals drafts facilities locations orders submit_plan submit_cancel/> })}</Show>
    }
}

#[component]
fn WaveDetail(
    signals: Signals,
    submit_release: Callback<PickWaveResponse>,
    open_cancel: Callback<()>,
) -> impl IntoView {
    view! {
        {move || if signals.detail_loading.get() { view! { <div class="workspace-state compact"><h2>"Loading wave"</h2></div> }.into_any() } else if let Some(wave)=signals.selected.get() { let for_release=wave.clone(); view! { <div class="pick-wave-detail-content"><header><div><span class="eyebrow">{format!("Wave #{}", wave.wave_id)}</span><h2>{wave.name.clone()}</h2></div><span class=wave_status_class(wave.status)>{wave_status_label(wave.status)}</span></header><div class="pick-wave-facts"><span><small>"Facility"</small><strong>{wave.facility_name.clone()}</strong></span><span><small>"Destination"</small><strong>{wave.destination_location_name.clone()}</strong></span><span><small>"Orders"</small><strong>{wave.order_count}</strong></span><span><small>"Tasks"</small><strong>{wave.pick_task_count}</strong></span><span><small>"Units"</small><strong>{wave.released_quantity}</strong></span><span><small>"Revision"</small><strong>{wave.revision.get()}</strong></span></div><div class="pick-wave-members table-scroll"><table class="data-table"><thead><tr><th>"Seq"</th><th>"Order"</th><th>"Client ID"</th><th>"State"</th><th class="numeric">"Tasks"</th><th class="numeric">"Units"</th></tr></thead><tbody>{wave.orders.into_iter().map(|order| view! { <tr><td>{order.sequence}</td><td><strong>{order.order_key}</strong><small class="cell-detail">{format!("Order #{} · rev {}", order.order_id, order.expected_revision.get())}</small></td><td>{order.inventory_owner_id}</td><td>{order.status}</td><td class="numeric">{order.pick_task_count}</td><td class="numeric">{order.released_quantity}</td></tr> }).collect_view()}</tbody></table></div>{(wave.status == PickWaveStatus::Planned).then(|| view! { <footer><button type="button" class="button danger-action" disabled=move || signals.command_pending.get() on:click=move |_| open_cancel.run(())><Trash2 size=14/>"Cancel"</button><button type="button" class="button primary-action" disabled=move || signals.command_pending.get() on:click=move |_| submit_release.run(for_release.clone())><Play size=14/>"Release wave"</button></footer> })}</div> }.into_any() } else { view! { <div class="workspace-empty"><h2>"Wave details"</h2><p>"Select a wave to inspect membership, work totals, and lifecycle evidence."</p></div> }.into_any() }}
    }
}

#[component]
fn WaveDialog(
    dialog: Dialog,
    signals: Signals,
    drafts: Drafts,
    facilities: StoredValue<Vec<wareboxes_api_contract::web::access::AccessScopeResource>>,
    locations: StoredValue<Vec<Location>>,
    orders: StoredValue<Vec<Order>>,
    submit_plan: Callback<()>,
    submit_cancel: Callback<()>,
) -> impl IntoView {
    let close = move |_| {
        if !signals.command_pending.get_untracked() {
            signals.dialog.set(None)
        }
    };
    let title = if dialog == Dialog::Plan {
        "Plan pick wave"
    } else {
        "Cancel pick wave"
    };
    view! { <div class="pick-wave-dialog-backdrop"><section class="pick-wave-dialog" class:wide=dialog == Dialog::Plan role="dialog" aria-modal="true" aria-labelledby="pick-wave-dialog-title"><header><div><span class="eyebrow">"Wave command"</span><h2 id="pick-wave-dialog-title">{title}</h2></div><button type="button" class="icon-button" aria-label="Close" on:click=close><X size=16/></button></header>
    {if dialog == Dialog::Plan { view! { <fieldset disabled=move || signals.command_pending.get()><div class="pick-wave-form-grid"><label><span>"Wave name"</span><input maxlength="100" prop:value=move || drafts.name.get() on:input=move |event| drafts.name.set(event_target_value(&event))/></label><label><span>"Facility"</span><select prop:value=move || drafts.facility_id.get().map_or_else(String::new, |id| id.to_string()) on:change=move |event| { drafts.facility_id.set(parse_optional_id(&event_target_value(&event))); drafts.destination_id.set(None); drafts.selected_orders.set(Vec::new()); }><option value="">"Select facility"</option>{facilities.with_value(|items| items.iter().map(|facility| view! { <option value=facility.id>{facility.name.clone()}</option> }).collect_view())}</select></label><label><span>"Staging destination"</span><select prop:value=move || drafts.destination_id.get().map_or_else(String::new, |id| id.to_string()) on:change=move |event| drafts.destination_id.set(parse_optional_id(&event_target_value(&event)))><option value="">"Select staging lane"</option>{move || locations.with_value(|items| items.iter().filter(|location| is_wave_destination(location, drafts.facility_id.get())).map(|location| view! { <option value=location.id>{location_label(location)}</option> }).collect_view())}</select></label></div><div class="pick-wave-order-picker"><header><h3>"Open orders"</h3><span>{move || format!("{} selected", drafts.selected_orders.get().len())}</span></header><div class="table-scroll"><table class="data-table"><thead><tr><th></th><th>"Order"</th><th>"Client"</th><th class="numeric">"Units"</th><th>"Ship by"</th></tr></thead><tbody>{move || orders.with_value(|items| items.iter().filter(|order| is_wave_candidate(order, drafts.facility_id.get())).map(|order| { let order_id=order.id; view! { <tr><td><input type="checkbox" aria-label=format!("Select order {}", order.order_key) checked=move || drafts.selected_orders.get().contains(&order_id) on:change=move |event| toggle_id(drafts.selected_orders, order_id, event_target_checked(&event))/></td><td><strong>{order.order_key.clone()}</strong><small class="cell-detail">{format!("Revision {}", order.revision)}</small></td><td>{order.inventory_owner_name.clone().unwrap_or_else(|| format!("Client #{}", order.inventory_owner_id))}</td><td class="numeric">{order.ordered_qty}</td><td>{order.ship_by.map(|value| value.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "Not set".to_owned())}</td></tr> }}).collect_view())}</tbody></table></div></div></fieldset> }.into_any() } else { view! { <fieldset disabled=move || signals.command_pending.get()><div class="pick-wave-form-grid single"><label><span>"Reason"</span><select on:change=move |event| drafts.cancellation_reason.set(parse_reason(&event_target_value(&event)))><option value="operational_change">"Operational change"</option><option value="capacity_constraint">"Capacity constraint"</option><option value="order_change">"Order change"</option><option value="other">"Other"</option></select></label><label><span>"Note"</span><textarea maxlength="500" prop:value=move || drafts.cancellation_note.get() on:input=move |event| drafts.cancellation_note.set(event_target_value(&event))></textarea></label></div></fieldset> }.into_any() }}
    <Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show><footer><button type="button" class="button secondary-action" on:click=close>"Back"</button><button type="button" class="button primary-action" disabled=move || signals.command_pending.get() on:click=move |_| if dialog == Dialog::Plan { submit_plan.run(()) } else { submit_cancel.run(()) }>{move || if signals.command_pending.get() { "Working" } else { title }}</button></footer></section></div> }
}

fn request_page(signals: Signals, on_unauthorized: Callback<()>) {
    if signals.loading.get_untracked() {
        return;
    }
    signals.loading.set(true);
    signals.error.set(None);
    let generation = signals.generation.get_untracked().saturating_add(1);
    signals.generation.set(generation);
    let facility = signals.facility_id.get_untracked();
    let status = signals.status.get_untracked();
    let sort = signals.sort.get_untracked();
    let direction = signals.direction.get_untracked();
    let cursor = signals.cursor.get_untracked();
    leptos::task::spawn_local(async move {
        match api::pick_waves(facility, status, sort, direction, cursor.as_ref()).await {
            Ok(page) if signals.generation.get_untracked() == generation => {
                signals.page.set(page);
                signals.loading.set(false);
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => {
                signals.error.set(Some(error.message));
                signals.loading.set(false);
            }
        }
    });
}

fn request_detail(signals: Signals, wave_id: i64, on_unauthorized: Callback<()>) {
    let generation = signals.detail_generation.get_untracked().saturating_add(1);
    signals.detail_generation.set(generation);
    signals.detail_loading.set(true);
    leptos::task::spawn_local(async move {
        match api::pick_wave(wave_id).await {
            Ok(wave) if signals.detail_generation.get_untracked() == generation => {
                signals.selected.set(Some(wave));
                signals.detail_loading.set(false);
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) if signals.detail_generation.get_untracked() == generation => {
                signals.error.set(Some(error.message));
                signals.detail_loading.set(false);
            }
            Err(_) => {}
        }
    });
}

fn dispatch(
    command: SavedCommand,
    signals: Signals,
    toasts: ToastBus,
    on_unauthorized: Callback<()>,
) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.error.set(None);
    signals.retry.set(Some(command.clone()));
    leptos::task::spawn_local(async move {
        let result = match &command {
            SavedCommand::Plan { request, key } => api::plan_pick_wave(request, key).await,
            SavedCommand::Release {
                wave_id,
                request,
                key,
            } => api::release_pick_wave(*wave_id, request, key).await,
            SavedCommand::Cancel {
                wave_id,
                request,
                key,
            } => api::cancel_pick_wave(*wave_id, request, key).await,
        };
        match result {
            Ok(wave) => {
                let message = match command {
                    SavedCommand::Plan { .. } => "Pick wave planned.",
                    SavedCommand::Release { .. } => "Pick wave released.",
                    SavedCommand::Cancel { .. } => "Pick wave cancelled.",
                };
                signals.retry.set(None);
                signals.dialog.set(None);
                signals.selected.set(Some(wave));
                signals.command_pending.set(false);
                toasts.success(message);
                signals.cursor.set(None);
                signals.cursor_history.set(Vec::new());
                request_page(signals, on_unauthorized);
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None);
                    if let Some(wave) = signals.selected.get_untracked() {
                        request_detail(signals, wave.wave_id, on_unauthorized);
                    }
                    request_page(signals, on_unauthorized);
                }
                signals.error.set(Some(error.message.clone()));
                signals.command_pending.set(false);
                toasts.error(error.message);
            }
        }
    });
}

fn select_sort(signals: Signals, key: PickWaveSort, on_unauthorized: Callback<()>) {
    if signals.loading.get_untracked() {
        return;
    }
    if signals.sort.get_untracked() == key {
        signals.direction.update(|direction| {
            *direction = match *direction {
                PickWaveSortDirection::Asc => PickWaveSortDirection::Desc,
                PickWaveSortDirection::Desc => PickWaveSortDirection::Asc,
            }
        });
    } else {
        signals.sort.set(key);
        signals.direction.set(PickWaveSortDirection::Asc);
    }
    signals.cursor.set(None);
    signals.cursor_history.set(Vec::new());
    request_page(signals, on_unauthorized);
}
fn next_page(signals: Signals, on_unauthorized: Callback<()>) {
    if let Some(next) = signals.page.get_untracked().next_cursor {
        signals
            .cursor_history
            .update(|history| history.push(signals.cursor.get_untracked()));
        signals.cursor.set(Some(next));
        request_page(signals, on_unauthorized);
    }
}
fn previous_page(signals: Signals, on_unauthorized: Callback<()>) {
    if let Some(previous) = signals
        .cursor_history
        .try_update(|history| history.pop())
        .flatten()
    {
        signals.cursor.set(previous);
        request_page(signals, on_unauthorized);
    }
}

fn build_plan_request(
    drafts: Drafts,
    orders: StoredValue<Vec<Order>>,
) -> Option<PlanPickWaveRequest> {
    let name = drafts.name.get_untracked().trim().to_owned();
    let facility_id = drafts.facility_id.get_untracked()?;
    let destination_location_id = drafts.destination_id.get_untracked()?;
    let selected = drafts.selected_orders.get_untracked();
    if name.is_empty() || selected.is_empty() {
        return None;
    }
    let mut members = orders.with_value(|items| {
        items
            .iter()
            .filter(|order| selected.contains(&order.id))
            .map(|order| (order.order_key.clone(), order.id, order.revision))
            .collect::<Vec<_>>()
    });
    members.sort_by(|left, right| left.0.cmp(&right.0));
    let orders = members
        .into_iter()
        .enumerate()
        .map(|(index, (_, order_id, revision))| {
            Some(PlanPickWaveOrderRequest {
                order_id,
                expected_revision: Revision::new(revision).ok()?,
                sequence: u32::try_from(index + 1).ok()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(PlanPickWaveRequest {
        facility_id,
        destination_location_id,
        name,
        orders,
    })
}
fn toggle_id(signal: RwSignal<Vec<i64>>, id: i64, checked: bool) {
    signal.update(|ids| {
        ids.retain(|candidate| *candidate != id);
        if checked {
            ids.push(id);
        }
    })
}
fn parse_optional_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|value| *value > 0)
}
fn parse_status(value: &str) -> Option<PickWaveStatus> {
    match value {
        "planned" => Some(PickWaveStatus::Planned),
        "released" => Some(PickWaveStatus::Released),
        "cancelled" => Some(PickWaveStatus::Cancelled),
        _ => None,
    }
}
fn parse_reason(value: &str) -> PickWaveCancellationReason {
    match value {
        "capacity_constraint" => PickWaveCancellationReason::CapacityConstraint,
        "order_change" => PickWaveCancellationReason::OrderChange,
        "other" => PickWaveCancellationReason::Other,
        _ => PickWaveCancellationReason::OperationalChange,
    }
}
fn ui_direction(value: PickWaveSortDirection) -> SortDirection {
    match value {
        PickWaveSortDirection::Asc => SortDirection::Ascending,
        PickWaveSortDirection::Desc => SortDirection::Descending,
    }
}
fn is_wave_destination(location: &Location, facility: Option<i64>) -> bool {
    facility == Some(location.facility_id)
        && location.active
        && location.deleted.is_none()
        && !location.pickable
        && !location.receivable
        && location
            .barcode
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
}
fn is_wave_candidate(order: &Order, facility: Option<i64>) -> bool {
    facility.is_some() && order.status == OrderStatus::Open && order.wave_id.is_none()
}
fn location_label(location: &Location) -> String {
    location
        .name
        .clone()
        .or_else(|| location.barcode.clone())
        .unwrap_or_else(|| format!("Location #{}", location.id))
}
fn wave_status_label(status: PickWaveStatus) -> &'static str {
    match status {
        PickWaveStatus::Planned => "Planned",
        PickWaveStatus::Released => "Released",
        PickWaveStatus::Cancelled => "Cancelled",
    }
}
fn wave_status_class(status: PickWaveStatus) -> &'static str {
    match status {
        PickWaveStatus::Planned => "status-chip status-open",
        PickWaveStatus::Released => "status-chip status-shipped",
        PickWaveStatus::Cancelled => "status-chip status-cancelled",
    }
}
fn compact_time(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_and_reason_parsers_fail_closed() {
        assert_eq!(parse_status("other"), None);
        assert_eq!(
            parse_reason("unknown"),
            PickWaveCancellationReason::OperationalChange
        );
    }
    #[test]
    fn server_sort_direction_maps_to_table_header() {
        assert_eq!(
            ui_direction(PickWaveSortDirection::Asc),
            SortDirection::Ascending
        );
        assert_eq!(
            ui_direction(PickWaveSortDirection::Desc),
            SortDirection::Descending
        );
    }
}
