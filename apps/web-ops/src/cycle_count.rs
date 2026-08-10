use leptos::prelude::*;
use lucide_leptos::{ClipboardList, Eye, RefreshCw, RotateCcw};
use wareboxes_api_contract::v1::{
    CreateCycleCountTaskRequest, CycleCountCandidatePage, CycleCountCandidateResponse,
    CycleCountCandidateSort, CycleCountPolicyPage, CycleCountSortDirection, CycleCountVariancePage,
    CycleCountWorkPage, CycleCountWorkResponse, CycleCountWorkSort, CycleCountWorkStatus,
    InventoryBalanceStatus, OpaqueCursor,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::sorting::{SortDirection, SortableHeader};
use crate::toast::{use_toast_bus, ToastBus};
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

mod control;
use control::{CycleCountPolicyControl, CycleCountVarianceControl};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Candidates,
    Work,
    Variances,
    Policies,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingCreate {
    request: CreateCycleCountTaskRequest,
    key: String,
}

#[derive(Clone, Copy)]
struct Signals {
    mode: RwSignal<ViewMode>,
    facility_id: RwSignal<Option<i64>>,
    inventory_owner_id: RwSignal<Option<i64>>,
    inventory_status: RwSignal<Option<InventoryBalanceStatus>>,
    work_status: RwSignal<Option<CycleCountWorkStatus>>,
    candidate_sort: RwSignal<CycleCountCandidateSort>,
    candidate_direction: RwSignal<CycleCountSortDirection>,
    work_sort: RwSignal<CycleCountWorkSort>,
    work_direction: RwSignal<CycleCountSortDirection>,
    candidates: RwSignal<CycleCountCandidatePage>,
    candidate_cursor: RwSignal<Option<OpaqueCursor>>,
    candidate_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    candidate_generation: RwSignal<u64>,
    work: RwSignal<CycleCountWorkPage>,
    work_cursor: RwSignal<Option<OpaqueCursor>>,
    work_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    work_generation: RwSignal<u64>,
    loading: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    retry: RwSignal<Option<PendingCreate>>,
    selected_candidate: RwSignal<Option<CycleCountCandidateResponse>>,
    selected_work: RwSignal<Option<CycleCountWorkResponse>>,
    note: RwSignal<String>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn CycleCountWorkspace(
    initial_candidates: CycleCountCandidatePage,
    initial_work: CycleCountWorkPage,
    initial_policies: CycleCountPolicyPage,
    initial_variances: CycleCountVariancePage,
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let control_access = access.clone();
    let facilities = StoredValue::new(access.facilities);
    let owners = StoredValue::new(access.inventory_owners);
    let layout = SplitPaneState::new("cycle-count", 720);
    let signals = Signals {
        mode: RwSignal::new(ViewMode::Candidates),
        facility_id: RwSignal::new(None),
        inventory_owner_id: RwSignal::new(None),
        inventory_status: RwSignal::new(None),
        work_status: RwSignal::new(None),
        candidate_sort: RwSignal::new(CycleCountCandidateSort::LastCounted),
        candidate_direction: RwSignal::new(CycleCountSortDirection::Asc),
        work_sort: RwSignal::new(CycleCountWorkSort::CreatedAt),
        work_direction: RwSignal::new(CycleCountSortDirection::Desc),
        candidates: RwSignal::new(initial_candidates),
        candidate_cursor: RwSignal::new(None),
        candidate_history: RwSignal::new(Vec::new()),
        candidate_generation: RwSignal::new(0),
        work: RwSignal::new(initial_work),
        work_cursor: RwSignal::new(None),
        work_history: RwSignal::new(Vec::new()),
        work_generation: RwSignal::new(0),
        loading: RwSignal::new(false),
        command_pending: RwSignal::new(false),
        retry: RwSignal::new(None),
        selected_candidate: RwSignal::new(None),
        selected_work: RwSignal::new(None),
        note: RwSignal::new(String::new()),
        error: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let select_candidate = Callback::new(move |candidate: CycleCountCandidateResponse| {
        signals.selected_work.set(None);
        signals.selected_candidate.set(Some(candidate));
        signals.note.set(String::new());
        signals.error.set(None);
        layout.show_detail();
    });
    let select_work = Callback::new(move |work: CycleCountWorkResponse| {
        signals.selected_candidate.set(None);
        signals.selected_work.set(Some(work));
        signals.error.set(None);
        layout.show_detail();
    });
    let refresh = Callback::new(move |_| refresh_active(signals));
    let reset_filters = Callback::new(move |_| {
        signals.facility_id.set(None);
        signals.inventory_owner_id.set(None);
        signals.inventory_status.set(None);
        signals.work_status.set(None);
        reset_and_load(signals);
    });
    let submit = Callback::new(move |_| prepare_create(signals));
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch_create(signals, command);
        }
    });

    let facility_options = move || {
        facilities.with_value(|values| {
            values
                .iter()
                .map(|value| view! { <option value=value.id>{value.name.clone()}</option> })
                .collect_view()
        })
    };
    let owner_options = move || {
        owners.with_value(|values| {
            values
                .iter()
                .map(|value| view! { <option value=value.id>{value.name.clone()}</option> })
                .collect_view()
        })
    };

    view! {
        <section class="cycle-count-workspace">
            <header class="cycle-count-header">
                <div class="cycle-count-heading"><ClipboardList size=16/><div><h1>"Cycle count control"</h1><span>"Blind RF counts and immutable variance results"</span></div></div>
                <div class="cycle-count-header-actions">
                    <Show when=move || matches!(signals.mode.get(), ViewMode::Candidates | ViewMode::Work)>
                        <PaneControls layout master_label="count queue" detail_label="count detail"/>
                        <button type="button" class="icon-button" title="Reset filters" aria-label="Reset cycle-count filters" on:click=move |_| reset_filters.run(())><RotateCcw size=14/></button>
                        <button type="button" class="icon-button" title="Refresh" aria-label="Refresh cycle counts" disabled=move || signals.loading.get() on:click=move |_| refresh.run(())><RefreshCw size=14/></button>
                    </Show>
                </div>
            </header>
            <div class="cycle-count-toolbar">
                <div class="segmented-control" role="tablist" aria-label="Cycle count views">
                    <button type="button" role="tab" aria-selected=move || (signals.mode.get()==ViewMode::Candidates).to_string() class:active=move || signals.mode.get()==ViewMode::Candidates on:click=move |_| { signals.mode.set(ViewMode::Candidates); signals.selected_work.set(None); }><ClipboardList size=13/>"To count"</button>
                    <button type="button" role="tab" aria-selected=move || (signals.mode.get()==ViewMode::Work).to_string() class:active=move || signals.mode.get()==ViewMode::Work on:click=move |_| { signals.mode.set(ViewMode::Work); signals.selected_candidate.set(None); }><ClipboardList size=13/>"Work"</button>
                    <button type="button" role="tab" aria-selected=move || (signals.mode.get()==ViewMode::Variances).to_string() class:active=move || signals.mode.get()==ViewMode::Variances on:click=move |_| { signals.mode.set(ViewMode::Variances); signals.selected_candidate.set(None); signals.selected_work.set(None); }><ClipboardList size=13/>"Variance review"</button>
                    <button type="button" role="tab" aria-selected=move || (signals.mode.get()==ViewMode::Policies).to_string() class:active=move || signals.mode.get()==ViewMode::Policies on:click=move |_| { signals.mode.set(ViewMode::Policies); signals.selected_candidate.set(None); signals.selected_work.set(None); }><ClipboardList size=13/>"Policies"</button>
                </div>
                <Show when=move || matches!(signals.mode.get(), ViewMode::Candidates | ViewMode::Work)>
                    <label><span>"Facility"</span><select prop:value=move || option_value(signals.facility_id.get()) on:change=move |event| { signals.facility_id.set(parse_optional_id(&event_target_value(&event))); reset_and_load(signals); }><option value="">"All facilities"</option>{facility_options}</select></label>
                    <label><span>"Client"</span><select prop:value=move || option_value(signals.inventory_owner_id.get()) on:change=move |event| { signals.inventory_owner_id.set(parse_optional_id(&event_target_value(&event))); reset_and_load(signals); }><option value="">"All clients"</option>{owner_options}</select></label>
                </Show>
                <Show when=move || signals.mode.get()==ViewMode::Candidates>
                    <label><span>"Inventory status"</span><select prop:value=move || inventory_status_value(signals.inventory_status.get()) on:change=move |event| { signals.inventory_status.set(parse_inventory_status(&event_target_value(&event))); reset_candidates(signals); }><option value="">"All statuses"</option><option value="available">"Available"</option><option value="hold">"Hold"</option><option value="damaged">"Damaged"</option><option value="quarantine">"Quarantine"</option></select></label>
                </Show>
                <Show when=move || signals.mode.get()==ViewMode::Work>
                    <label><span>"Work status"</span><select prop:value=move || work_status_value(signals.work_status.get()) on:change=move |event| { signals.work_status.set(parse_work_status(&event_target_value(&event))); reset_work(signals); }><option value="">"Open work"</option><option value="pending">"Pending"</option><option value="claimed">"Claimed"</option><option value="completed">"Completed"</option><option value="cancelled">"Cancelled"</option></select></label>
                </Show>
            </div>
            {move || match signals.mode.get() {
                ViewMode::Candidates | ViewMode::Work => view! {
                    <div class="cycle-count-body split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
                        <section class="cycle-count-master split-master">
                            <Show when=move || signals.mode.get()==ViewMode::Candidates fallback=move || view! { <WorkTable signals select=select_work/> }>
                                <CandidateTable signals select=select_candidate/>
                            </Show>
                        </section>
                        <SplitPaneHandle layout/>
                        <aside class="cycle-count-detail split-detail">
                            {move || {
                                if let Some(candidate)=signals.selected_candidate.get() {
                                    view! { <PlanningPanel candidate signals submit retry/> }.into_any()
                                } else if let Some(work)=signals.selected_work.get() {
                                    view! { <WorkDetail work/> }.into_any()
                                } else {
                                    view! { <div class="cycle-count-empty"><ClipboardList size=24/><h2>"Cycle count detail"</h2><p>"Select stock to schedule a blind RF count, or select work to inspect execution and variance."</p></div> }.into_any()
                                }
                            }}
                        </aside>
                    </div>
                }.into_any(),
                ViewMode::Variances => view! { <CycleCountVarianceControl initial_page=initial_variances.clone() access=control_access.clone() on_unauthorized/> }.into_any(),
                ViewMode::Policies => view! { <CycleCountPolicyControl initial_page=initial_policies.clone() access=control_access.clone() on_unauthorized/> }.into_any(),
            }}
        </section>
    }
}

#[component]
fn CandidateTable(
    signals: Signals,
    select: Callback<CycleCountCandidateResponse>,
) -> impl IntoView {
    view! {
        <div class="cycle-count-table-region"><table class="data-table cycle-count-table"><caption class="sr-only">"Inventory balances eligible for blind cycle count"</caption><thead><tr>
            <SortableHeader label="Last count" active=move || signals.candidate_sort.get()==CycleCountCandidateSort::LastCounted direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,CycleCountCandidateSort::LastCounted))/>
            <SortableHeader label="Client" active=move || signals.candidate_sort.get()==CycleCountCandidateSort::Client direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,CycleCountCandidateSort::Client))/>
            <SortableHeader label="Facility" active=move || signals.candidate_sort.get()==CycleCountCandidateSort::Facility direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,CycleCountCandidateSort::Facility))/>
            <SortableHeader label="Location" active=move || signals.candidate_sort.get()==CycleCountCandidateSort::Location direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,CycleCountCandidateSort::Location))/>
            <SortableHeader label="Item / LPN" active=move || signals.candidate_sort.get()==CycleCountCandidateSort::Item direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,CycleCountCandidateSort::Item))/>
            <SortableHeader label="Status" active=move || signals.candidate_sort.get()==CycleCountCandidateSort::InventoryStatus direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,CycleCountCandidateSort::InventoryStatus))/>
            <SortableHeader label="On hand" active=move || signals.candidate_sort.get()==CycleCountCandidateSort::Quantity direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,CycleCountCandidateSort::Quantity)) numeric=true/>
            <th class="icon-column"><span class="sr-only">"View"</span></th>
        </tr></thead><tbody>
            {move || signals.candidates.get().items.into_iter().map(|candidate| { let row=candidate.clone(); let selected=signals.selected_candidate.get().as_ref().is_some_and(|value| value.stock.inventory_balance_id==candidate.stock.inventory_balance_id); let button_row=row.clone(); view! { <tr class:selected=selected on:click=move |_| select.run(row.clone())><td>{candidate.last_counted_at.as_deref().map(compact_time).unwrap_or_else(|| "Never".into())}<small class="cell-detail">{candidate.last_variance_quantity.map_or_else(|| "No variance history".into(),|value| format!("Last variance {value:+}"))}</small></td><td>{candidate.inventory_owner_name}</td><td>{candidate.facility_name}</td><td><strong>{location_label(&candidate.location)}</strong><small class="cell-detail">{candidate.location.barcode}</small></td><td><strong>{item_label(&candidate.item,candidate.stock.license_plate_barcode.as_deref())}</strong><small class="cell-detail">{trace_label(&candidate.stock)}</small></td><td><span class=inventory_status_class(candidate.stock.inventory_status)>{inventory_status_label(candidate.stock.inventory_status)}</span></td><td class="numeric strong">{format_quantity(candidate.quantity.on_hand)}<small class="cell-detail">{format!("{} / R{} / H{}",candidate.stock.uom,candidate.quantity.reserved,candidate.quantity.held)}</small></td><td class="icon-column"><button type="button" class="icon-button compact" title="View count target" aria-label=format!("View balance {}",candidate.stock.inventory_balance_id) aria-pressed=selected on:click=move |event| { event.stop_propagation(); select.run(button_row.clone()); }><Eye size=13/></button></td></tr> } }).collect_view()}
        </tbody></table><PageFooter label="balances" count=move || signals.candidates.get().items.len() loading=signals.loading history=signals.candidate_history has_more=Signal::derive(move || signals.candidates.get().has_more()) previous=Callback::new(move |_| previous_candidates(signals)) next=Callback::new(move |_| next_candidates(signals))/></div>
    }
}

#[component]
fn WorkTable(signals: Signals, select: Callback<CycleCountWorkResponse>) -> impl IntoView {
    view! {
        <div class="cycle-count-table-region"><table class="data-table cycle-count-table"><caption class="sr-only">"Cycle count execution work and results"</caption><thead><tr>
            <SortableHeader label="Created" active=move || signals.work_sort.get()==CycleCountWorkSort::CreatedAt direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,CycleCountWorkSort::CreatedAt))/>
            <SortableHeader label="Status" active=move || signals.work_sort.get()==CycleCountWorkSort::Status direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,CycleCountWorkSort::Status))/>
            <SortableHeader label="Client" active=move || signals.work_sort.get()==CycleCountWorkSort::Client direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,CycleCountWorkSort::Client))/>
            <SortableHeader label="Location" active=move || signals.work_sort.get()==CycleCountWorkSort::Location direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,CycleCountWorkSort::Location))/>
            <SortableHeader label="Item / LPN" active=move || signals.work_sort.get()==CycleCountWorkSort::Item direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,CycleCountWorkSort::Item))/>
            <SortableHeader label="Qty" active=move || signals.work_sort.get()==CycleCountWorkSort::Quantity direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,CycleCountWorkSort::Quantity)) numeric=true/>
            <SortableHeader label="Variance" active=move || signals.work_sort.get()==CycleCountWorkSort::Variance direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,CycleCountWorkSort::Variance)) numeric=true/>
            <th class="icon-column"><span class="sr-only">"View"</span></th>
        </tr></thead><tbody>
            {move || signals.work.get().items.into_iter().map(|work| { let row=work.clone(); let selected=signals.selected_work.get().as_ref().is_some_and(|value| value.task_id==work.task_id); let button_row=row.clone(); let quantity=work.counted_quantity.or_else(|| work.current_quantity.as_ref().map(|value| value.on_hand)); view! { <tr class:selected=selected on:click=move |_| select.run(row.clone())><td>{compact_time(&work.created_at)}<small class="cell-detail">{format!("Task #{} / P{}",work.task_id,work.priority)}</small></td><td><span class=work_status_class(work.status)>{work_status_label(work.status)}</span></td><td>{work.inventory_owner_name}<small class="cell-detail">{work.facility_name}</small></td><td><strong>{location_label(&work.location)}</strong><small class="cell-detail">{work.location.barcode}</small></td><td><strong>{item_label(&work.item,work.stock.license_plate_barcode.as_deref())}</strong><small class="cell-detail">{trace_label(&work.stock)}</small></td><td class="numeric strong">{quantity.map_or_else(|| "-".into(),format_quantity)}<small class="cell-detail">{work.stock.uom}</small></td><td class=variance_class(work.variance_quantity)>{work.variance_quantity.map_or_else(|| "-".into(),|value| format!("{value:+}"))}</td><td class="icon-column"><button type="button" class="icon-button compact" title="View count work" aria-label=format!("View task {}",work.task_id) aria-pressed=selected on:click=move |event| { event.stop_propagation(); select.run(button_row.clone()); }><Eye size=13/></button></td></tr> } }).collect_view()}
        </tbody></table><PageFooter label="tasks" count=move || signals.work.get().items.len() loading=signals.loading history=signals.work_history has_more=Signal::derive(move || signals.work.get().has_more()) previous=Callback::new(move |_| previous_work(signals)) next=Callback::new(move |_| next_work(signals))/></div>
    }
}

#[component]
fn PageFooter(
    label: &'static str,
    count: impl Fn() -> usize + Copy + Send + 'static,
    loading: RwSignal<bool>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
    has_more: Signal<bool>,
    previous: Callback<()>,
    next: Callback<()>,
) -> impl IntoView {
    view! { <footer class="table-footer"><span>{move || if loading.get() { "Refreshing...".into() } else { format!("{} {label} on this page",count()) }}</span><button type="button" class="button secondary-action" disabled=move || loading.get() || history.get().is_empty() on:click=move |_| previous.run(())>"Previous"</button><button type="button" class="button secondary-action" disabled=move || loading.get() || !has_more.get() on:click=move |_| next.run(())>"Next"</button></footer> }
}

#[component]
fn PlanningPanel(
    candidate: CycleCountCandidateResponse,
    signals: Signals,
    submit: Callback<()>,
    retry: Callback<()>,
) -> impl IntoView {
    view! { <div class="cycle-count-panel"><header><span class="eyebrow">"Blind execution"</span><h2>"Schedule cycle count"</h2><p>{format!("{} / {}",candidate.inventory_owner_name,candidate.facility_name)}</p></header><dl class="cycle-count-facts"><div><dt>"Location"</dt><dd>{format!("{} / {}",location_label(&candidate.location),candidate.location.barcode)}</dd></div><div><dt>"Inventory"</dt><dd>{item_label(&candidate.item,candidate.stock.license_plate_barcode.as_deref())}</dd></div><div><dt>"System quantity"</dt><dd>{format!("{} {}",format_quantity(candidate.quantity.on_hand),candidate.stock.uom)}</dd></div><div><dt>"Commitments"</dt><dd>{format!("Reserved {} / Held {}",candidate.quantity.reserved,candidate.quantity.held)}</dd></div><div><dt>"Trace"</dt><dd>{trace_label(&candidate.stock)}</dd></div><div><dt>"Last count"</dt><dd>{candidate.last_counted_at.as_deref().map(compact_time).unwrap_or_else(|| "Never".into())}</dd></div></dl><p class="cycle-count-blind-note">"The RF operator receives identity and scan requirements only. This system quantity is not disclosed in the claim."</p><fieldset disabled=move || signals.command_pending.get()><label><span>"Instructions (optional)"</span><textarea maxlength="1000" prop:value=move || signals.note.get() on:input=move |event| signals.note.set(event_target_value(&event))></textarea></label></fieldset><Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show><footer><Show when=move || signals.retry.get().is_some()><button type="button" class="button secondary-action" disabled=move || signals.command_pending.get() on:click=move |_| retry.run(())>"Retry exact command"</button></Show><button type="button" class="button primary-action" disabled=move || signals.command_pending.get() on:click=move |_| submit.run(())>{move || if signals.command_pending.get() { "Scheduling..." } else { "Create blind count" }}</button></footer></div> }
}

#[component]
fn WorkDetail(work: CycleCountWorkResponse) -> impl IntoView {
    let quantity = work
        .current_quantity
        .clone()
        .or(work.system_quantity.clone());
    view! { <div class="cycle-count-panel"><header><span class="eyebrow">"RF execution"</span><h2>{format!("Cycle count task #{}",work.task_id)}</h2><span class=work_status_class(work.status)>{work_status_label(work.status)}</span></header><dl class="cycle-count-facts"><div><dt>"Client / facility"</dt><dd>{format!("{} / {}",work.inventory_owner_name,work.facility_name)}</dd></div><div><dt>"Location"</dt><dd>{format!("{} / {}",location_label(&work.location),work.location.barcode)}</dd></div><div><dt>"Inventory"</dt><dd>{item_label(&work.item,work.stock.license_plate_barcode.as_deref())}</dd></div><div><dt>"Trace"</dt><dd>{trace_label(&work.stock)}</dd></div><div><dt>"System quantity"</dt><dd>{quantity.as_ref().map_or_else(|| "Unavailable".into(),|value| format!("{} {}",format_quantity(value.on_hand),work.stock.uom))}</dd></div><div><dt>"Counted quantity"</dt><dd>{work.counted_quantity.map_or_else(|| "Pending blind count".into(),|value| format!("{} {}",format_quantity(value),work.stock.uom))}</dd></div><div><dt>"Variance"</dt><dd class=variance_class(work.variance_quantity)>{work.variance_quantity.map_or_else(|| "Pending".into(),|value| format!("{value:+}"))}</dd></div><div><dt>"Inventory transaction"</dt><dd>{work.inventory_transaction_id.map_or_else(|| "None".into(),|id| format!("Transaction #{id}"))}</dd></div><div><dt>"Assignment"</dt><dd>{work.assigned_user_id.map_or_else(|| "Unassigned".into(),|id| format!("User #{id}"))}</dd></div><div><dt>"Completed"</dt><dd>{work.confirmed_at.as_deref().map(compact_time).unwrap_or_else(|| "Not completed".into())}</dd></div></dl>{work.note.map(|value| view! { <section class="cycle-count-instructions"><span>"Instructions / note"</span><p>{value}</p></section> })}</div> }
}

fn prepare_create(signals: Signals) {
    let Some(candidate) = signals.selected_candidate.get_untracked() else {
        return;
    };
    let note = optional_text(&signals.note.get_untracked());
    let command = PendingCreate {
        request: CreateCycleCountTaskRequest {
            inventory_balance_id: candidate.stock.inventory_balance_id,
            note,
        },
        key: api::new_idempotency_key(),
    };
    dispatch_create(signals, command);
}

fn dispatch_create(signals: Signals, command: PendingCreate) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.retry.set(None);
    signals.error.set(None);
    leptos::task::spawn_local(async move {
        let result = api::create_cycle_count_task(&command.request, &command.key).await;
        signals.command_pending.set(false);
        match result {
            Ok(response) => {
                signals.selected_candidate.set(None);
                signals.mode.set(ViewMode::Work);
                signals
                    .toasts
                    .success(format!("Cycle count task #{} created.", response.task_id));
                reset_candidates(signals);
                reset_work(signals);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if error.ambiguous_outcome {
                    signals.retry.set(Some(command));
                }
                signals.error.set(Some(error.message));
                reset_and_load(signals);
            }
        }
    });
}

fn refresh_active(signals: Signals) {
    match signals.mode.get_untracked() {
        ViewMode::Candidates => {
            request_candidates(signals, signals.candidate_cursor.get_untracked())
        }
        ViewMode::Work => request_work(signals, signals.work_cursor.get_untracked()),
        ViewMode::Variances | ViewMode::Policies => {}
    }
}
fn reset_and_load(signals: Signals) {
    reset_candidates(signals);
    reset_work(signals);
}
fn reset_candidates(signals: Signals) {
    signals.candidate_history.set(Vec::new());
    signals.candidate_cursor.set(None);
    request_candidates(signals, None);
}
fn reset_work(signals: Signals) {
    signals.work_history.set(Vec::new());
    signals.work_cursor.set(None);
    request_work(signals, None);
}
fn next_candidates(signals: Signals) {
    if let Some(next) = signals.candidates.get_untracked().next_cursor {
        signals
            .candidate_history
            .update(|history| history.push(signals.candidate_cursor.get_untracked()));
        request_candidates(signals, Some(next));
    }
}
fn previous_candidates(signals: Signals) {
    let previous = signals
        .candidate_history
        .try_update(|history| history.pop())
        .flatten()
        .flatten();
    request_candidates(signals, previous);
}
fn next_work(signals: Signals) {
    if let Some(next) = signals.work.get_untracked().next_cursor {
        signals
            .work_history
            .update(|history| history.push(signals.work_cursor.get_untracked()));
        request_work(signals, Some(next));
    }
}
fn previous_work(signals: Signals) {
    let previous = signals
        .work_history
        .try_update(|history| history.pop())
        .flatten()
        .flatten();
    request_work(signals, previous);
}

fn request_candidates(signals: Signals, cursor: Option<OpaqueCursor>) {
    let generation = signals.candidate_generation.get_untracked().wrapping_add(1);
    signals.candidate_generation.set(generation);
    signals.loading.set(true);
    leptos::task::spawn_local(async move {
        let result = api::cycle_count_candidates(
            signals.facility_id.get_untracked(),
            signals.inventory_owner_id.get_untracked(),
            signals.inventory_status.get_untracked(),
            signals.candidate_sort.get_untracked(),
            signals.candidate_direction.get_untracked(),
            cursor.as_ref(),
        )
        .await;
        if signals.candidate_generation.get_untracked() != generation {
            return;
        }
        signals.loading.set(false);
        match result {
            Ok(page) => {
                signals.candidate_cursor.set(cursor);
                signals.candidates.set(page);
                signals.error.set(None);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
    });
}
fn request_work(signals: Signals, cursor: Option<OpaqueCursor>) {
    let generation = signals.work_generation.get_untracked().wrapping_add(1);
    signals.work_generation.set(generation);
    signals.loading.set(true);
    leptos::task::spawn_local(async move {
        let result = api::cycle_count_work(
            signals.facility_id.get_untracked(),
            signals.inventory_owner_id.get_untracked(),
            signals.work_status.get_untracked(),
            signals.work_sort.get_untracked(),
            signals.work_direction.get_untracked(),
            cursor.as_ref(),
        )
        .await;
        if signals.work_generation.get_untracked() != generation {
            return;
        }
        signals.loading.set(false);
        match result {
            Ok(page) => {
                signals.work_cursor.set(cursor);
                signals.work.set(page);
                signals.error.set(None);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
    });
}

fn candidate_sort(signals: Signals, key: CycleCountCandidateSort) {
    if signals.candidate_sort.get_untracked() == key {
        signals
            .candidate_direction
            .update(|value| *value = toggle_direction(*value));
    } else {
        signals.candidate_sort.set(key);
        signals
            .candidate_direction
            .set(CycleCountSortDirection::Asc);
    }
    reset_candidates(signals);
}
fn work_sort(signals: Signals, key: CycleCountWorkSort) {
    if signals.work_sort.get_untracked() == key {
        signals
            .work_direction
            .update(|value| *value = toggle_direction(*value));
    } else {
        signals.work_sort.set(key);
        signals.work_direction.set(CycleCountSortDirection::Asc);
    }
    reset_work(signals);
}
const fn toggle_direction(value: CycleCountSortDirection) -> CycleCountSortDirection {
    match value {
        CycleCountSortDirection::Asc => CycleCountSortDirection::Desc,
        CycleCountSortDirection::Desc => CycleCountSortDirection::Asc,
    }
}
const fn display_direction(value: CycleCountSortDirection) -> SortDirection {
    match value {
        CycleCountSortDirection::Asc => SortDirection::Ascending,
        CycleCountSortDirection::Desc => SortDirection::Descending,
    }
}
fn parse_optional_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}
fn option_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}
fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn parse_inventory_status(value: &str) -> Option<InventoryBalanceStatus> {
    match value {
        "available" => Some(InventoryBalanceStatus::Available),
        "hold" => Some(InventoryBalanceStatus::Hold),
        "damaged" => Some(InventoryBalanceStatus::Damaged),
        "quarantine" => Some(InventoryBalanceStatus::Quarantine),
        _ => None,
    }
}
fn inventory_status_value(value: Option<InventoryBalanceStatus>) -> &'static str {
    match value {
        None => "",
        Some(InventoryBalanceStatus::Available) => "available",
        Some(InventoryBalanceStatus::Hold) => "hold",
        Some(InventoryBalanceStatus::Damaged) => "damaged",
        Some(InventoryBalanceStatus::Quarantine) => "quarantine",
    }
}
fn parse_work_status(value: &str) -> Option<CycleCountWorkStatus> {
    match value {
        "pending" => Some(CycleCountWorkStatus::Pending),
        "claimed" => Some(CycleCountWorkStatus::Claimed),
        "completed" => Some(CycleCountWorkStatus::Completed),
        "cancelled" => Some(CycleCountWorkStatus::Cancelled),
        _ => None,
    }
}
fn work_status_value(value: Option<CycleCountWorkStatus>) -> &'static str {
    match value {
        None => "",
        Some(CycleCountWorkStatus::Pending) => "pending",
        Some(CycleCountWorkStatus::Claimed) => "claimed",
        Some(CycleCountWorkStatus::Completed) => "completed",
        Some(CycleCountWorkStatus::Cancelled) => "cancelled",
    }
}
fn work_status_label(value: CycleCountWorkStatus) -> &'static str {
    match value {
        CycleCountWorkStatus::Pending => "Pending",
        CycleCountWorkStatus::Claimed => "Claimed",
        CycleCountWorkStatus::Completed => "Completed",
        CycleCountWorkStatus::Cancelled => "Cancelled",
    }
}
fn work_status_class(value: CycleCountWorkStatus) -> &'static str {
    match value {
        CycleCountWorkStatus::Pending => "status open",
        CycleCountWorkStatus::Claimed => "status processing",
        CycleCountWorkStatus::Completed => "status shipped",
        CycleCountWorkStatus::Cancelled => "status cancelled",
    }
}
fn inventory_status_label(value: InventoryBalanceStatus) -> &'static str {
    match value {
        InventoryBalanceStatus::Available => "Available",
        InventoryBalanceStatus::Hold => "Hold",
        InventoryBalanceStatus::Damaged => "Damaged",
        InventoryBalanceStatus::Quarantine => "Quarantine",
    }
}
fn inventory_status_class(value: InventoryBalanceStatus) -> &'static str {
    match value {
        InventoryBalanceStatus::Available => "status shipped",
        InventoryBalanceStatus::Hold | InventoryBalanceStatus::Damaged => "status held",
        InventoryBalanceStatus::Quarantine => "status processing",
    }
}
fn variance_class(value: Option<i64>) -> &'static str {
    match value {
        Some(0) => "numeric variance-zero",
        Some(_) => "numeric variance-nonzero",
        None => "numeric",
    }
}
fn location_label(value: &wareboxes_api_contract::v1::CycleCountLocation) -> String {
    value.name.clone().unwrap_or_else(|| value.barcode.clone())
}
fn item_label(value: &wareboxes_api_contract::v1::CycleCountItem, plate: Option<&str>) -> String {
    plate
        .map(str::to_owned)
        .or_else(|| value.description.clone())
        .or_else(|| value.barcodes.first().cloned())
        .unwrap_or_else(|| format!("Item #{}", value.item_id))
}
fn trace_label(value: &wareboxes_api_contract::v1::CycleCountStock) -> String {
    [
        value.lot.as_ref().map(|v| format!("Lot {v}")),
        value.serial.as_ref().map(|v| format!("Serial {v}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" / ")
    .pipe(|value| {
        if value.is_empty() {
            "No lot / serial".into()
        } else {
            value
        }
    })
}
fn compact_time(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorting_is_server_directed() {
        assert_eq!(
            toggle_direction(CycleCountSortDirection::Asc),
            CycleCountSortDirection::Desc
        );
        assert_eq!(
            work_status_value(Some(CycleCountWorkStatus::Completed)),
            "completed"
        );
    }
    #[test]
    fn notes_are_trimmed() {
        assert_eq!(
            optional_text("  upper rack  ").as_deref(),
            Some("upper rack")
        );
        assert_eq!(optional_text("  "), None);
    }
}
