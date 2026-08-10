use leptos::prelude::*;
use lucide_leptos::{Boxes, ClipboardList, RefreshCw, RotateCcw};
use wareboxes_api_contract::v1::{
    CreateLicensePlatePutawayTaskRequest, CreatePutawayTaskRequest, OpaqueCursor,
    PutawayCandidatePage, PutawayCandidateResponse, PutawayCandidateSort, PutawaySortDirection,
    PutawayWorkPage, PutawayWorkResponse, PutawayWorkSort, PutawayWorkStatus, PutawayWorkflow,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::Location;

use crate::api;
use crate::sorting::{SortDirection, SortableHeader};
use crate::toast::{use_toast_bus, ToastBus};
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Candidates,
    Work,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingCreate {
    Loose {
        request: CreatePutawayTaskRequest,
        key: String,
    },
    LicensePlate {
        request: CreateLicensePlatePutawayTaskRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
struct Signals {
    mode: RwSignal<ViewMode>,
    facility_id: RwSignal<Option<i64>>,
    inventory_owner_id: RwSignal<Option<i64>>,
    workflow: RwSignal<Option<PutawayWorkflow>>,
    work_status: RwSignal<Option<PutawayWorkStatus>>,
    candidate_sort: RwSignal<PutawayCandidateSort>,
    candidate_direction: RwSignal<PutawaySortDirection>,
    work_sort: RwSignal<PutawayWorkSort>,
    work_direction: RwSignal<PutawaySortDirection>,
    candidates: RwSignal<PutawayCandidatePage>,
    candidate_cursor: RwSignal<Option<OpaqueCursor>>,
    candidate_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    candidate_generation: RwSignal<u64>,
    work: RwSignal<PutawayWorkPage>,
    work_cursor: RwSignal<Option<OpaqueCursor>>,
    work_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    work_generation: RwSignal<u64>,
    loading: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    retry: RwSignal<Option<PendingCreate>>,
    selected_candidate: RwSignal<Option<PutawayCandidateResponse>>,
    selected_work: RwSignal<Option<PutawayWorkResponse>>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[derive(Clone, Copy)]
struct Drafts {
    destination_id: RwSignal<Option<i64>>,
    quantity: RwSignal<i64>,
    priority: RwSignal<i64>,
    instructions: RwSignal<String>,
}

#[component]
pub(crate) fn PutawayWorkspace(
    initial_candidates: PutawayCandidatePage,
    initial_work: PutawayWorkPage,
    access: AccessScopeWorkspace,
    locations: Vec<Location>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let facilities = StoredValue::new(access.facilities);
    let owners = StoredValue::new(access.inventory_owners);
    let locations = StoredValue::new(locations);
    let layout = SplitPaneState::new("putaway", 700);
    let signals = Signals {
        mode: RwSignal::new(ViewMode::Candidates),
        facility_id: RwSignal::new(None),
        inventory_owner_id: RwSignal::new(None),
        workflow: RwSignal::new(None),
        work_status: RwSignal::new(None),
        candidate_sort: RwSignal::new(PutawayCandidateSort::ReceivedAt),
        candidate_direction: RwSignal::new(PutawaySortDirection::Asc),
        work_sort: RwSignal::new(PutawayWorkSort::CreatedAt),
        work_direction: RwSignal::new(PutawaySortDirection::Asc),
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
        error: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = Drafts {
        destination_id: RwSignal::new(None),
        quantity: RwSignal::new(0),
        priority: RwSignal::new(50),
        instructions: RwSignal::new(String::new()),
    };

    let select_candidate = Callback::new(move |candidate: PutawayCandidateResponse| {
        drafts.destination_id.set(None);
        drafts.quantity.set(candidate.available_quantity);
        drafts.priority.set(50);
        drafts.instructions.set(String::new());
        signals.selected_work.set(None);
        signals.selected_candidate.set(Some(candidate));
        signals.error.set(None);
        layout.show_detail();
    });
    let select_work = Callback::new(move |work: PutawayWorkResponse| {
        signals.selected_candidate.set(None);
        signals.selected_work.set(Some(work));
        signals.error.set(None);
        layout.show_detail();
    });
    let submit = Callback::new(move |_| prepare_create(signals, drafts));
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch_create(signals, command);
        }
    });
    let refresh = Callback::new(move |_| refresh_active(signals));
    let reset_filters = Callback::new(move |_| {
        signals.facility_id.set(None);
        signals.inventory_owner_id.set(None);
        signals.workflow.set(None);
        signals.work_status.set(None);
        reset_and_load(signals);
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
        <section class="putaway-workspace">
            <header class="putaway-header">
                <div class="putaway-heading"><Boxes size=16/><div><h1>"Putaway control"</h1><span>"Receiving inventory and directed RF work"</span></div></div>
                <div class="putaway-header-actions">
                    <PaneControls layout master_label="putaway queue" detail_label="putaway detail"/>
                    <button type="button" class="icon-button" title="Reset filters" aria-label="Reset putaway filters" on:click=move |_| reset_filters.run(())><RotateCcw size=14/></button>
                    <button type="button" class="icon-button" title="Refresh" aria-label="Refresh putaway" disabled=move || signals.loading.get() on:click=move |_| refresh.run(())><RefreshCw size=14/></button>
                </div>
            </header>
            <div class="putaway-toolbar">
                <div class="segmented-control" role="tablist" aria-label="Putaway views">
                    <button type="button" role="tab" aria-selected=move || (signals.mode.get()==ViewMode::Candidates).to_string() class:active=move || signals.mode.get()==ViewMode::Candidates on:click=move |_| { signals.mode.set(ViewMode::Candidates); signals.selected_work.set(None); }><Boxes size=13/>"To plan"</button>
                    <button type="button" role="tab" aria-selected=move || (signals.mode.get()==ViewMode::Work).to_string() class:active=move || signals.mode.get()==ViewMode::Work on:click=move |_| { signals.mode.set(ViewMode::Work); signals.selected_candidate.set(None); }><ClipboardList size=13/>"Work"</button>
                </div>
                <label><span>"Facility"</span><select prop:value=move || option_value(signals.facility_id.get()) on:change=move |event| { signals.facility_id.set(parse_optional_id(&event_target_value(&event))); reset_and_load(signals); }><option value="">"All facilities"</option>{facility_options}</select></label>
                <label><span>"Client"</span><select prop:value=move || option_value(signals.inventory_owner_id.get()) on:change=move |event| { signals.inventory_owner_id.set(parse_optional_id(&event_target_value(&event))); reset_and_load(signals); }><option value="">"All clients"</option>{owner_options}</select></label>
                <label><span>"Workflow"</span><select prop:value=move || workflow_value(signals.workflow.get()) on:change=move |event| { signals.workflow.set(parse_workflow(&event_target_value(&event))); reset_and_load(signals); }><option value="">"All workflows"</option><option value="loose">"Loose stock"</option><option value="license_plate">"License plate"</option></select></label>
                <Show when=move || signals.mode.get()==ViewMode::Work>
                    <label><span>"Status"</span><select prop:value=move || status_value(signals.work_status.get()) on:change=move |event| { signals.work_status.set(parse_status(&event_target_value(&event))); reset_work(signals); }><option value="">"Open work"</option><option value="pending">"Pending"</option><option value="claimed">"Claimed"</option><option value="completed">"Completed"</option><option value="cancelled">"Cancelled"</option></select></label>
                </Show>
            </div>
            <div class="putaway-body split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
                <section class="putaway-master split-master">
                    <Show when=move || signals.mode.get()==ViewMode::Candidates fallback=move || view! { <WorkTable signals select=select_work/> }>
                        <CandidateTable signals select=select_candidate/>
                    </Show>
                </section>
                <SplitPaneHandle layout/>
                <aside class="putaway-detail split-detail">
                    {move || {
                        if let Some(candidate)=signals.selected_candidate.get() {
                            view! { <PlanningPanel candidate drafts locations signals submit retry/> }.into_any()
                        } else if let Some(work)=signals.selected_work.get() {
                            view! { <WorkDetail work/> }.into_any()
                        } else {
                            view! { <div class="putaway-empty"><Boxes size=24/><h2>"Putaway detail"</h2><p>"Select receiving stock to plan work, or select a task to review execution."</p></div> }.into_any()
                        }
                    }}
                </aside>
            </div>
        </section>
    }
}

#[component]
fn CandidateTable(signals: Signals, select: Callback<PutawayCandidateResponse>) -> impl IntoView {
    view! {
        <div class="putaway-table-region">
            <table class="data-table putaway-table"><caption class="sr-only">"Receiving inventory eligible for directed putaway"</caption><thead><tr>
                <SortableHeader label="Received" active=move || signals.candidate_sort.get()==PutawayCandidateSort::ReceivedAt direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,PutawayCandidateSort::ReceivedAt))/>
                <SortableHeader label="Workflow" active=move || signals.candidate_sort.get()==PutawayCandidateSort::Workflow direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,PutawayCandidateSort::Workflow))/>
                <SortableHeader label="Client" active=move || signals.candidate_sort.get()==PutawayCandidateSort::Client direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,PutawayCandidateSort::Client))/>
                <SortableHeader label="Facility" active=move || signals.candidate_sort.get()==PutawayCandidateSort::Facility direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,PutawayCandidateSort::Facility))/>
                <SortableHeader label="Source" active=move || signals.candidate_sort.get()==PutawayCandidateSort::Source direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,PutawayCandidateSort::Source))/>
                <SortableHeader label="Item / LPN" active=move || signals.candidate_sort.get()==PutawayCandidateSort::Item direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,PutawayCandidateSort::Item))/>
                <SortableHeader label="Qty" active=move || signals.candidate_sort.get()==PutawayCandidateSort::Quantity direction=move || display_direction(signals.candidate_direction.get()) on_sort=Callback::new(move |_| candidate_sort(signals,PutawayCandidateSort::Quantity)) numeric=true/>
            </tr></thead><tbody>
                {move || signals.candidates.get().items.into_iter().map(|candidate| { let row=candidate.clone(); let selected=signals.selected_candidate.get().as_ref().is_some_and(|value| candidate_key(value)==candidate_key(&candidate)); let owner=candidate.inventory_owner_name.clone(); let facility=candidate.facility_name.clone(); let source_barcode=candidate.source_location.barcode.clone(); view! { <tr class:selected=selected on:click=move |_| select.run(row.clone())><td>{compact_time(&candidate.received_at)}</td><td><span class="status processing">{workflow_label(candidate.workflow)}</span></td><td>{owner}</td><td>{facility}</td><td><strong>{location_label(&candidate.source_location)}</strong><small class="cell-detail">{source_barcode}</small></td><td><strong>{candidate_item(&candidate)}</strong><small class="cell-detail">{candidate_trace(&candidate)}</small></td><td class="numeric strong">{format_quantity(candidate.available_quantity)}<small class="cell-detail">{candidate.uom.clone().unwrap_or_else(|| format!("{} balances",candidate.balance_count))}</small></td></tr> } }).collect_view()}
            </tbody></table>
            <PageFooter label="positions" count=move || signals.candidates.get().items.len() loading=signals.loading history=signals.candidate_history has_more=Signal::derive(move || signals.candidates.get().has_more()) previous=Callback::new(move |_| previous_candidates(signals)) next=Callback::new(move |_| next_candidates(signals))/>
        </div>
    }
}

#[component]
fn WorkTable(signals: Signals, select: Callback<PutawayWorkResponse>) -> impl IntoView {
    view! {
        <div class="putaway-table-region"><table class="data-table putaway-table"><caption class="sr-only">"Directed putaway work across the active scope"</caption><thead><tr>
            <SortableHeader label="Priority" active=move || signals.work_sort.get()==PutawayWorkSort::Priority direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,PutawayWorkSort::Priority)) numeric=true/>
            <SortableHeader label="Created" active=move || signals.work_sort.get()==PutawayWorkSort::CreatedAt direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,PutawayWorkSort::CreatedAt))/>
            <SortableHeader label="Status" active=move || signals.work_sort.get()==PutawayWorkSort::Status direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,PutawayWorkSort::Status))/>
            <SortableHeader label="Workflow" active=move || signals.work_sort.get()==PutawayWorkSort::Workflow direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,PutawayWorkSort::Workflow))/>
            <SortableHeader label="Client" active=move || signals.work_sort.get()==PutawayWorkSort::Client direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,PutawayWorkSort::Client))/>
            <SortableHeader label="Source" active=move || signals.work_sort.get()==PutawayWorkSort::Source direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,PutawayWorkSort::Source))/>
            <SortableHeader label="Destination" active=move || signals.work_sort.get()==PutawayWorkSort::Destination direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,PutawayWorkSort::Destination))/>
            <SortableHeader label="Qty" active=move || signals.work_sort.get()==PutawayWorkSort::Quantity direction=move || display_direction(signals.work_direction.get()) on_sort=Callback::new(move |_| work_sort(signals,PutawayWorkSort::Quantity)) numeric=true/>
        </tr></thead><tbody>
            {move || signals.work.get().items.into_iter().map(|work| { let row=work.clone(); let selected=signals.selected_work.get().as_ref().is_some_and(|value| value.task_id==work.task_id); view! { <tr class:selected=selected on:click=move |_| select.run(row.clone())><td class="numeric strong">{work.priority}</td><td>{compact_time(&work.created_at)}<small class="cell-detail">{format!("Task #{}",work.task_id)}</small></td><td><span class=work_status_class(work.status)>{work_status_label(work.status)}</span></td><td>{workflow_label(work.workflow)}</td><td>{work.inventory_owner_name}<small class="cell-detail">{work.facility_name}</small></td><td><strong>{location_label(&work.source_location)}</strong><small class="cell-detail">{work.source_location.barcode}</small></td><td><strong>{location_label(&work.destination_location)}</strong><small class="cell-detail">{work.destination_location.barcode}</small></td><td class="numeric strong">{format_quantity(work.planned_quantity)}<small class="cell-detail">{work.uom.clone().unwrap_or_else(|| format!("{} balances",work.balance_count))}</small></td></tr> } }).collect_view()}
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
    candidate: PutawayCandidateResponse,
    drafts: Drafts,
    locations: StoredValue<Vec<Location>>,
    signals: Signals,
    submit: Callback<()>,
    retry: Callback<()>,
) -> impl IntoView {
    let facility_id = candidate.facility_id;
    let source_id = candidate.source_location.location_id;
    let destinations = locations.with_value(|values| {
        values
            .iter()
            .filter(|value| {
                value.facility_id == facility_id
                    && value.id != source_id
                    && value.active
                    && !value.receivable
                    && value.deleted.is_none()
                    && value
                        .barcode
                        .as_ref()
                        .is_some_and(|barcode| !barcode.trim().is_empty())
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    let is_loose = candidate.workflow == PutawayWorkflow::Loose;
    view! { <div class="putaway-planning-panel"><header><span class="eyebrow">"Directed work"</span><h2>{if is_loose { "Plan loose putaway" } else { "Plan whole-LP putaway" }}</h2><p>{format!("{} / {}",candidate.inventory_owner_name,candidate.facility_name)}</p></header><dl class="putaway-facts"><div><dt>"Source"</dt><dd>{location_label(&candidate.source_location)}</dd></div><div><dt>"Inventory"</dt><dd>{candidate_item(&candidate)}</dd></div><div><dt>"Available"</dt><dd>{format!("{} {}",format_quantity(candidate.available_quantity),candidate.uom.clone().unwrap_or_else(|| "units".into()))}</dd></div><div><dt>"Trace"</dt><dd>{candidate_trace(&candidate)}</dd></div></dl><fieldset disabled=move || signals.command_pending.get()><label><span>"Destination"</span><select required prop:value=move || option_value(drafts.destination_id.get()) on:change=move |event| drafts.destination_id.set(parse_optional_id(&event_target_value(&event)))><option value="">"Select storage location"</option>{destinations.into_iter().map(|value| { let label=value.name.clone().or(value.barcode.clone()).unwrap_or_else(|| format!("Location #{}",value.id)); view! { <option value=value.id>{label}</option> } }).collect_view()}</select></label><Show when=move || is_loose><label><span>"Quantity"</span><input type="number" min="1" max=candidate.available_quantity prop:value=move || drafts.quantity.get() on:input=move |event| { if let Ok(value)=event_target_value(&event).parse() { drafts.quantity.set(value); } }/></label></Show><label><span>"Priority"</span><input type="number" min="0" max="999" prop:value=move || drafts.priority.get() on:input=move |event| { if let Ok(value)=event_target_value(&event).parse() { drafts.priority.set(value); } }/></label><label><span>"Instructions"</span><textarea maxlength="1000" prop:value=move || drafts.instructions.get() on:input=move |event| drafts.instructions.set(event_target_value(&event))></textarea></label></fieldset><Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show><footer><Show when=move || signals.retry.get().is_some()><button type="button" class="button secondary-action" disabled=move || signals.command_pending.get() on:click=move |_| retry.run(())>"Retry exact command"</button></Show><button type="button" class="button primary-action" disabled=move || signals.command_pending.get() on:click=move |_| submit.run(())>{move || if signals.command_pending.get() { "Planning..." } else { "Create putaway work" }}</button></footer></div> }
}

#[component]
fn WorkDetail(work: PutawayWorkResponse) -> impl IntoView {
    view! { <div class="putaway-work-panel"><header><span class="eyebrow">"RF execution"</span><h2>{format!("Putaway task #{}",work.task_id)}</h2><span class=work_status_class(work.status)>{work_status_label(work.status)}</span></header><dl class="putaway-facts"><div><dt>"Workflow"</dt><dd>{workflow_label(work.workflow)}</dd></div><div><dt>"Client / facility"</dt><dd>{format!("{} / {}",work.inventory_owner_name,work.facility_name)}</dd></div><div><dt>"Source"</dt><dd>{format!("{} / {}",location_label(&work.source_location),work.source_location.barcode)}</dd></div><div><dt>"Destination"</dt><dd>{format!("{} / {}",location_label(&work.destination_location),work.destination_location.barcode)}</dd></div><div><dt>"Quantity"</dt><dd>{format!("{} {}",format_quantity(work.planned_quantity),work.uom.unwrap_or_else(|| "units".into()))}</dd></div><div><dt>"Assignment"</dt><dd>{work.assigned_user_id.map_or_else(|| "Unassigned".into(),|id| format!("User #{id}"))}</dd></div><div><dt>"Created"</dt><dd>{compact_time(&work.created_at)}</dd></div><div><dt>"Lease / due"</dt><dd>{work.lease_expires_at.or(work.due_at).map_or_else(|| "None".into(),|value| compact_time(&value))}</dd></div></dl>{work.instructions.map(|value| view! { <section class="putaway-instructions"><span>"Instructions"</span><p>{value}</p></section> })}</div> }
}

fn prepare_create(signals: Signals, drafts: Drafts) {
    let Some(candidate) = signals.selected_candidate.get_untracked() else {
        return;
    };
    let Some(destination_location_id) = drafts.destination_id.get_untracked() else {
        signals
            .error
            .set(Some("Select a storage destination.".into()));
        return;
    };
    let priority = drafts.priority.get_untracked();
    if priority < 0 {
        signals
            .error
            .set(Some("Priority cannot be negative.".into()));
        return;
    }
    let instructions = optional_text(&drafts.instructions.get_untracked());
    let key = api::new_idempotency_key();
    let command = match candidate.workflow {
        PutawayWorkflow::Loose => {
            let quantity = drafts.quantity.get_untracked();
            if quantity <= 0 || quantity > candidate.available_quantity {
                signals.error.set(Some(
                    "Quantity must be within the currently available amount.".into(),
                ));
                return;
            }
            let Some(source_inventory_balance_id) = candidate.source_inventory_balance_id else {
                signals
                    .error
                    .set(Some("The selected loose balance is incomplete.".into()));
                return;
            };
            PendingCreate::Loose {
                request: CreatePutawayTaskRequest {
                    source_inventory_balance_id,
                    destination_location_id,
                    quantity,
                    priority: Some(priority),
                    assigned_user_id: None,
                    scheduled_for: None,
                    due_at: None,
                    instructions,
                },
                key,
            }
        }
        PutawayWorkflow::LicensePlate => {
            let Some(license_plate_id) = candidate.license_plate_id else {
                signals
                    .error
                    .set(Some("The selected license plate is incomplete.".into()));
                return;
            };
            PendingCreate::LicensePlate {
                request: CreateLicensePlatePutawayTaskRequest {
                    license_plate_id,
                    destination_location_id,
                    priority: Some(priority),
                    assigned_user_id: None,
                    scheduled_for: None,
                    due_at: None,
                    instructions,
                },
                key,
            }
        }
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
        let result = match &command {
            PendingCreate::Loose { request, key } => api::create_putaway(request, key)
                .await
                .map(|value| value.task_id),
            PendingCreate::LicensePlate { request, key } => {
                api::create_license_plate_putaway(request, key)
                    .await
                    .map(|value| value.task_id)
            }
        };
        signals.command_pending.set(false);
        match result {
            Ok(task_id) => {
                signals.selected_candidate.set(None);
                signals.mode.set(ViewMode::Work);
                signals
                    .toasts
                    .success(format!("Putaway task #{task_id} created."));
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
    let Some(next) = signals.candidates.get_untracked().next_cursor else {
        return;
    };
    signals
        .candidate_history
        .update(|history| history.push(signals.candidate_cursor.get_untracked()));
    request_candidates(signals, Some(next));
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
    let Some(next) = signals.work.get_untracked().next_cursor else {
        return;
    };
    signals
        .work_history
        .update(|history| history.push(signals.work_cursor.get_untracked()));
    request_work(signals, Some(next));
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
        let result = api::putaway_candidates(
            signals.facility_id.get_untracked(),
            signals.inventory_owner_id.get_untracked(),
            signals.workflow.get_untracked(),
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
        let result = api::putaway_work(
            signals.facility_id.get_untracked(),
            signals.inventory_owner_id.get_untracked(),
            signals.workflow.get_untracked(),
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

fn candidate_sort(signals: Signals, key: PutawayCandidateSort) {
    if signals.candidate_sort.get_untracked() == key {
        signals
            .candidate_direction
            .update(|value| *value = toggle_direction(*value));
    } else {
        signals.candidate_sort.set(key);
        signals.candidate_direction.set(PutawaySortDirection::Asc);
    }
    reset_candidates(signals);
}
fn work_sort(signals: Signals, key: PutawayWorkSort) {
    if signals.work_sort.get_untracked() == key {
        signals
            .work_direction
            .update(|value| *value = toggle_direction(*value));
    } else {
        signals.work_sort.set(key);
        signals.work_direction.set(PutawaySortDirection::Asc);
    }
    reset_work(signals);
}
const fn toggle_direction(value: PutawaySortDirection) -> PutawaySortDirection {
    match value {
        PutawaySortDirection::Asc => PutawaySortDirection::Desc,
        PutawaySortDirection::Desc => PutawaySortDirection::Asc,
    }
}
const fn display_direction(value: PutawaySortDirection) -> SortDirection {
    match value {
        PutawaySortDirection::Asc => SortDirection::Ascending,
        PutawaySortDirection::Desc => SortDirection::Descending,
    }
}
fn parse_optional_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}
fn option_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}
fn parse_workflow(value: &str) -> Option<PutawayWorkflow> {
    match value {
        "loose" => Some(PutawayWorkflow::Loose),
        "license_plate" => Some(PutawayWorkflow::LicensePlate),
        _ => None,
    }
}
fn workflow_value(value: Option<PutawayWorkflow>) -> &'static str {
    match value {
        None => "",
        Some(PutawayWorkflow::Loose) => "loose",
        Some(PutawayWorkflow::LicensePlate) => "license_plate",
    }
}
fn parse_status(value: &str) -> Option<PutawayWorkStatus> {
    match value {
        "pending" => Some(PutawayWorkStatus::Pending),
        "claimed" => Some(PutawayWorkStatus::Claimed),
        "completed" => Some(PutawayWorkStatus::Completed),
        "cancelled" => Some(PutawayWorkStatus::Cancelled),
        _ => None,
    }
}
fn status_value(value: Option<PutawayWorkStatus>) -> &'static str {
    match value {
        None => "",
        Some(PutawayWorkStatus::Pending) => "pending",
        Some(PutawayWorkStatus::Claimed) => "claimed",
        Some(PutawayWorkStatus::Completed) => "completed",
        Some(PutawayWorkStatus::Cancelled) => "cancelled",
    }
}
fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn workflow_label(value: PutawayWorkflow) -> &'static str {
    match value {
        PutawayWorkflow::Loose => "Loose",
        PutawayWorkflow::LicensePlate => "License plate",
    }
}
fn work_status_label(value: PutawayWorkStatus) -> &'static str {
    match value {
        PutawayWorkStatus::Pending => "Pending",
        PutawayWorkStatus::Claimed => "Claimed",
        PutawayWorkStatus::Completed => "Completed",
        PutawayWorkStatus::Cancelled => "Cancelled",
    }
}
fn work_status_class(value: PutawayWorkStatus) -> &'static str {
    match value {
        PutawayWorkStatus::Pending => "status open",
        PutawayWorkStatus::Claimed => "status processing",
        PutawayWorkStatus::Completed => "status shipped",
        PutawayWorkStatus::Cancelled => "status cancelled",
    }
}
fn location_label(value: &wareboxes_api_contract::v1::PutawayLocationResponse) -> String {
    value.name.clone().unwrap_or_else(|| value.barcode.clone())
}
fn candidate_item(value: &PutawayCandidateResponse) -> String {
    value
        .license_plate_barcode
        .clone()
        .or_else(|| value.item_description.clone())
        .or_else(|| value.primary_sku.clone())
        .unwrap_or_else(|| format!("{} items", value.item_count))
}
fn candidate_trace(value: &PutawayCandidateResponse) -> String {
    [
        value.primary_sku.clone(),
        value.lot.as_ref().map(|v| format!("Lot {v}")),
        value.serial.as_ref().map(|v| format!("Serial {v}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" / ")
}
fn candidate_key(value: &PutawayCandidateResponse) -> (PutawayWorkflow, Option<i64>, Option<i64>) {
    (
        value.workflow,
        value.source_inventory_balance_id,
        value.license_plate_id,
    )
}
fn compact_time(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorting_changes_are_server_requests() {
        assert_eq!(
            toggle_direction(PutawaySortDirection::Asc),
            PutawaySortDirection::Desc
        );
        assert_eq!(
            workflow_value(Some(PutawayWorkflow::LicensePlate)),
            "license_plate"
        );
    }
    #[test]
    fn notes_are_trimmed() {
        assert_eq!(
            optional_text("  Scan upper rack  ").as_deref(),
            Some("Scan upper rack")
        );
        assert_eq!(optional_text("   "), None);
    }
}
