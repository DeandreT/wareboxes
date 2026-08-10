use leptos::prelude::*;
use lucide_leptos::Eye;
use wareboxes_api_contract::v1::{
    CreateInventoryRecallRequest, InventoryAgingPage, InventoryAgingResponse, InventoryRecallPage,
    InventoryRecallReason, InventoryRecallResponse, InventoryRecallStatus, OpaqueCursor,
    ReleaseInventoryRecallRequest,
};
#[cfg(target_arch = "wasm32")]
use wareboxes_api_contract::v1::{InventoryAgingSort, InventorySortDirection};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};

#[cfg(target_arch = "wasm32")]
use crate::api;
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "hydration dispatches retained recall commands")
)]
enum SavedRecallCommand {
    Create {
        request: CreateInventoryRecallRequest,
        key: String,
    },
    Release {
        recall_id: i64,
        request: ReleaseInventoryRecallRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
struct RecallSignals {
    page: RwSignal<InventoryRecallPage>,
    selected: RwSignal<Option<InventoryRecallResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(dead_code, reason = "hydration guards asynchronous recall pages")
    )]
    generation: RwSignal<u64>,
    status: RwSignal<Option<InventoryRecallStatus>>,
    facility_filter: RwSignal<String>,
    owner_filter: RwSignal<String>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
    create_open: RwSignal<bool>,
    facility_id: RwSignal<String>,
    batch_query: RwSignal<String>,
    candidate_page: RwSignal<InventoryAgingPage>,
    selected_candidate: RwSignal<Option<InventoryAgingResponse>>,
    candidate_loading: RwSignal<bool>,
    candidate_error: RwSignal<Option<String>>,
    candidate_generation: RwSignal<u64>,
    reason: RwSignal<InventoryRecallReason>,
    note: RwSignal<String>,
    release_confirm: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    retry: RwSignal<Option<SavedRecallCommand>>,
}

impl RecallSignals {
    fn new() -> Self {
        Self {
            page: RwSignal::new(InventoryRecallPage::new(Vec::new(), None)),
            selected: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            generation: RwSignal::new(0),
            status: RwSignal::new(Some(InventoryRecallStatus::Active)),
            facility_filter: RwSignal::new(String::new()),
            owner_filter: RwSignal::new(String::new()),
            cursor: RwSignal::new(None),
            history: RwSignal::new(Vec::new()),
            create_open: RwSignal::new(false),
            facility_id: RwSignal::new(String::new()),
            batch_query: RwSignal::new(String::new()),
            candidate_page: RwSignal::new(InventoryAgingPage::new(Vec::new(), None)),
            selected_candidate: RwSignal::new(None),
            candidate_loading: RwSignal::new(false),
            candidate_error: RwSignal::new(None),
            candidate_generation: RwSignal::new(0),
            reason: RwSignal::new(InventoryRecallReason::SupplierNotice),
            note: RwSignal::new(String::new()),
            release_confirm: RwSignal::new(false),
            command_pending: RwSignal::new(false),
            retry: RwSignal::new(None),
        }
    }
}

#[component]
pub(super) fn RecallView(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
    target: RwSignal<Option<InventoryAgingResponse>>,
) -> impl IntoView {
    let signals = RecallSignals::new();
    let access = StoredValue::new(access);
    let layout = SplitPaneState::new("inventory-recalls", 760);
    let toasts = use_toast_bus();
    Effect::new(move || request_recalls(signals, on_unauthorized));
    Effect::new(move || {
        let Some(candidate) = target.get() else {
            return;
        };
        signals.facility_id.set(candidate.facility_id.to_string());
        signals.batch_query.set(trace_search_value(&candidate));
        signals
            .candidate_page
            .set(InventoryAgingPage::new(vec![candidate.clone()], None));
        signals.selected_candidate.set(Some(candidate));
        signals.create_open.set(true);
        signals.selected.set(None);
        signals.release_confirm.set(false);
        target.set(None);
        layout.show_detail();
    });

    let refresh = move |_| {
        reset_page(signals);
        request_recalls(signals, on_unauthorized);
    };
    let next = move |_| {
        let Some(cursor) = signals.page.get_untracked().next_cursor else {
            return;
        };
        signals
            .history
            .update(|history| history.push(signals.cursor.get_untracked()));
        signals.cursor.set(Some(cursor));
        request_recalls(signals, on_unauthorized);
    };
    let previous = move |_| {
        let Some(cursor) = signals
            .history
            .with_untracked(|history| history.last().cloned())
        else {
            return;
        };
        signals.history.update(|history| {
            history.pop();
        });
        signals.cursor.set(cursor);
        request_recalls(signals, on_unauthorized);
    };
    let open_create = move |_| {
        let default_facility = access.with_value(|scope| {
            (scope.facilities.len() == 1).then(|| scope.facilities[0].id.to_string())
        });
        signals
            .facility_id
            .set(default_facility.unwrap_or_default());
        signals.batch_query.set(String::new());
        signals
            .candidate_page
            .set(InventoryAgingPage::new(Vec::new(), None));
        signals.selected_candidate.set(None);
        signals.candidate_error.set(None);
        signals.create_open.set(true);
        signals.selected.set(None);
        signals.release_confirm.set(false);
        signals.retry.set(None);
        signals.error.set(None);
        layout.show_detail();
    };
    let search_batches = move |_| {
        reset_candidates(signals);
        request_candidates(signals, false, on_unauthorized);
    };
    let more_batches = move |_| request_candidates(signals, true, on_unauthorized);
    let submit_create = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if signals.command_pending.get_untracked() {
            return;
        }
        let Some(candidate) = signals.selected_candidate.get_untracked() else {
            signals.error.set(Some(
                "Select an inventory batch from the search results.".into(),
            ));
            return;
        };
        let note = optional_text(&signals.note.get_untracked());
        if signals.reason.get_untracked() == InventoryRecallReason::Other && note.is_none() {
            signals.error.set(Some(
                "A note is required when the recall reason is Other.".into(),
            ));
            return;
        }
        let request = CreateInventoryRecallRequest {
            facility_id: candidate.facility_id,
            item_batch_id: candidate.item_batch_id,
            reason: signals.reason.get_untracked(),
            note,
        };
        let key = api_key();
        dispatch_command(
            signals,
            SavedRecallCommand::Create { request, key },
            on_unauthorized,
            toasts,
        );
    };
    let release = move |_| {
        let Some(recall) = signals.selected.get_untracked() else {
            return;
        };
        let key = api_key();
        dispatch_command(
            signals,
            SavedRecallCommand::Release {
                recall_id: recall.recall_id,
                request: ReleaseInventoryRecallRequest {
                    expected_revision: recall.revision,
                },
                key,
            },
            on_unauthorized,
            toasts,
        );
    };
    let retry = move |_| {
        let Some(command) = signals.retry.get_untracked() else {
            return;
        };
        dispatch_command(signals, command, on_unauthorized, toasts);
    };

    view! {
        <section class="integrity-read-view recall-view">
            <div class="recall-control-stack">
              <div class="integrity-query-bar recall">
                <label>
                    <span>"State"</span>
                    <select
                        prop:value=move || status_value(signals.status.get())
                        on:change=move |event| {
                            signals.status.set(parse_status(&event_target_value(&event)));
                            reset_page(signals);
                            request_recalls(signals, on_unauthorized);
                        }
                    >
                        <option value="active">"Active recalls"</option>
                        <option value="released">"Released history"</option>
                        <option value="all">"All cases"</option>
                    </select>
                </label>
                <label><span>"Facility"</span><select prop:value=move || signals.facility_filter.get() on:change=move |event| signals.facility_filter.set(event_target_value(&event))><option value="">"All facilities"</option>{access.with_value(|scope| scope_options(&scope.facilities))}</select></label>
                <label><span>"Client"</span><select prop:value=move || signals.owner_filter.get() on:change=move |event| signals.owner_filter.set(event_target_value(&event))><option value="">"All clients"</option>{access.with_value(|scope| scope_options(&scope.inventory_owners))}</select></label>
                <button type="button" class="button secondary-action" disabled=move || signals.loading.get() on:click=refresh>"Apply"</button>
                <button type="button" class="button primary-action" on:click=open_create>"New recall"</button>
                <div class="integrity-health" class:attention=move || signals.page.get().items.iter().any(|item| item.status==InventoryRecallStatus::Active)>
                    <strong>{move || signals.page.get().items.len()}</strong><span>"cases on page"</span>
                </div>
                <PaneControls layout master_label="recall case table" detail_label="recall case detail"/>
              </div>
              <Show when=move || signals.error.get().is_some()>
                <div class="inline-command-error recall-error" role="alert">
                    <span>{move || signals.error.get().unwrap_or_default()}</span>
                    <Show when=move || signals.retry.get().is_some()>
                        <button type="button" class="button secondary-action" disabled=move || signals.command_pending.get() on:click=retry>"Retry exact command"</button>
                    </Show>
                </div>
              </Show>
            </div>
            <div class="integrity-read-split split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
                <section class="data-section split-master integrity-read-master">
                    <div class="table-toolbar compact"><div class="toolbar-summary"><strong>{move || signals.page.get().items.len()}</strong><span>"recall cases"</span></div><span>{move || if signals.loading.get(){"Loading recalls"}else{"Newest cases first"}}</span></div>
                    <div class="table-scroll"><table class="data-table recall-table">
                        <thead><tr><th>"Case"</th><th>"Item / trace"</th><th>"State"</th><th class="numeric">"Positions"</th><th class="numeric">"Held"</th><th>"Client"</th><th>"Facility"</th><th class="icon-column"><span class="sr-only">"Detail"</span></th></tr></thead>
                        <tbody>{move || recall_rows(signals,layout)}</tbody>
                    </table></div>
                    <div class="table-footer"><span>{move || if signals.page.get().next_cursor.is_some(){"More recall cases available"}else{"End of recall results"}}</span><div><button type="button" class="button secondary-action" disabled=move || signals.history.get().is_empty()||signals.loading.get() on:click=previous>"Previous"</button><button type="button" class="button secondary-action" disabled=move || signals.page.get().next_cursor.is_none()||signals.loading.get() on:click=next>"Next"</button></div></div>
                </section>
                <SplitPaneHandle layout/>
                <aside class="data-section split-detail recall-detail">
                    {move || if signals.create_open.get() {
                        view! { <RecallCreateForm access=access.get_value() signals on_search=Callback::new(search_batches) on_more=Callback::new(more_batches) on_submit=Callback::new(submit_create)/> }.into_any()
                    } else {
                        signals.selected.get().map_or_else(
                            || view! { <div class="journal-detail-empty"><h2>"Recall detail"</h2><p>"Select a case or open a facility batch recall."</p></div> }.into_any(),
                            |recall| view! { <RecallDetail recall signals on_release=Callback::new(release)/> }.into_any(),
                        )
                    }}
                </aside>
            </div>
        </section>
    }
}

fn recall_rows(signals: RecallSignals, layout: SplitPaneState) -> AnyView {
    let rows = signals.page.get().items;
    if rows.is_empty() && !signals.loading.get() {
        return view! { <tr><td colspan="8" class="table-empty-row">"No recall cases match the current scope and state."</td></tr> }.into_any();
    }
    rows.into_iter().map(|recall| {
        let row=recall.clone(); let action=recall.clone(); let id=recall.recall_id;
        let selected=signals.selected.get().is_some_and(|value|value.recall_id==id);
        let item=item_label(&recall); let trace=trace_label(&recall);
        view! { <tr class:selected=selected on:click=move |_| {signals.selected.set(Some(row.clone()));signals.create_open.set(false);signals.release_confirm.set(false);layout.show_detail();}>
            <td><strong>{format!("#{id}")}</strong><small class="cell-detail">{compact_date(&recall.created_at)}</small></td>
            <td><strong>{item}</strong><small class="cell-detail">{trace}</small></td>
            <td><span class=status_class(recall.status)>{status_label(recall.status)}</span></td><td class="numeric">{recall.affected_position_count}</td><td class="numeric strong">{format_quantity(recall.held_quantity)}</td><td>{recall.inventory_owner_name}</td><td>{recall.facility_name}</td>
            <td class="icon-column"><button type="button" class="icon-button compact" title="View recall detail" aria-label=format!("View recall case {id}") aria-pressed=selected on:click=move |event| {event.stop_propagation();signals.selected.set(Some(action.clone()));signals.create_open.set(false);signals.release_confirm.set(false);layout.show_detail();}><Eye size=13/></button></td>
        </tr> }
    }).collect_view().into_any()
}

fn candidate_rows(signals: RecallSignals) -> AnyView {
    let rows = signals.candidate_page.get().items;
    if rows.is_empty() {
        let message = if signals.candidate_loading.get() {
            "Searching current inventory"
        } else if signals.batch_query.get().trim().is_empty() {
            "Choose a facility, then search by SKU, description, lot, or serial."
        } else {
            "No current inventory batches match this search."
        };
        return view! { <p class="recall-candidate-empty">{message}</p> }.into_any();
    }
    rows.into_iter()
        .map(|candidate| {
            let selection = candidate.clone();
            let batch_id = candidate.item_batch_id;
            let selected = signals.selected_candidate.get().is_some_and(|value| {
                value.facility_id == candidate.facility_id && value.item_batch_id == batch_id
            });
            let committed = candidate.reserved_quantity + candidate.held_quantity;
            view! {
                <button
                    type="button"
                    class="recall-candidate"
                    class:selected=selected
                    class:blocked=committed != 0
                    disabled=committed != 0
                    aria-pressed=selected
                    on:click=move |_| {
                        signals.selected_candidate.set(Some(selection.clone()));
                        signals.error.set(None);
                        signals.retry.set(None);
                    }
                >
                    <span><strong>{aging_item_label(&candidate)}</strong><small>{aging_trace_label(&candidate)}</small></span>
                    <span><strong>{format!("{} {}", format_quantity(candidate.on_hand_quantity), candidate.uom)}</strong><small>{if committed == 0 { "No visible commitments".to_owned() } else { format!("{} committed; unavailable", format_quantity(committed)) }}</small></span>
                    <span><strong>{candidate.inventory_owner_name}</strong><small>{format!("Batch #{batch_id}")}</small></span>
                </button>
            }
        })
        .collect_view()
        .into_any()
}

#[component]
fn RecallCreateForm(
    access: AccessScopeWorkspace,
    signals: RecallSignals,
    on_search: Callback<()>,
    on_more: Callback<()>,
    on_submit: Callback<leptos::ev::SubmitEvent>,
) -> impl IntoView {
    view! { <form class="recall-create-form" on:submit=move |event| on_submit.run(event)>
        <header><div><p class="eyebrow">"Facility batch containment"</p><h2>"Open inventory recall"</h2></div><span class="status held">"Full hold"</span></header>
        <p class="detail-note">"Every positive position for this item batch at the facility must be unreserved and unheld. The command places a full-quantity hold on the exact set atomically."</p>
        <div class="recall-form-grid">
            <label><span>"Facility"</span><select required prop:value=move || signals.facility_id.get() on:change=move |event| {signals.facility_id.set(event_target_value(&event));reset_candidates(signals);signals.retry.set(None);}><option value="">"Select facility"</option>{scope_options(&access.facilities)}</select></label>
            <label class="recall-batch-search"><span>"Find inventory batch"</span><div><input type="search" placeholder="SKU, description, lot or serial" prop:value=move || signals.batch_query.get() on:input=move |event| {signals.batch_query.set(event_target_value(&event));signals.retry.set(None);}/><button type="button" class="button secondary-action" disabled=move || signals.candidate_loading.get()||positive_id(&signals.facility_id.get()).is_none() on:click=move |_| on_search.run(())>{move || if signals.candidate_loading.get(){"Searching"}else{"Search"}}</button></div></label>
            <div class="recall-candidate-picker">
                <Show when=move || signals.candidate_error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.candidate_error.get().unwrap_or_default()}</p></Show>
                <div class="recall-candidate-results">{move || candidate_rows(signals)}</div>
                <Show when=move || signals.candidate_page.get().next_cursor.is_some()><button type="button" class="button secondary-action recall-more-candidates" disabled=move || signals.candidate_loading.get() on:click=move |_| on_more.run(())>"Load more matches"</button></Show>
            </div>
            <label><span>"Reason"</span><select prop:value=move || reason_value(signals.reason.get()) on:change=move |event| {signals.reason.set(parse_reason(&event_target_value(&event)));signals.retry.set(None);}>
                <option value="supplier_notice">"Supplier notice"</option><option value="regulatory">"Regulatory"</option><option value="customer_request">"Customer request"</option><option value="quality_concern">"Quality concern"</option><option value="other">"Other"</option>
            </select></label>
            <label class="recall-note"><span>"Note"</span><textarea maxlength="500" placeholder="Bulletin, authority, or investigation context" prop:value=move || signals.note.get() on:input=move |event| {signals.note.set(event_target_value(&event));signals.retry.set(None);}></textarea></label>
        </div>
        <footer><button type="button" class="button secondary-action" on:click=move |_| {signals.create_open.set(false);signals.error.set(None);signals.retry.set(None);}>"Cancel"</button><button type="submit" class="button danger-action" disabled=move || signals.command_pending.get()||signals.selected_candidate.get().is_none()>{move || if signals.command_pending.get(){"Opening recall"}else{"Hold selected batch"}}</button></footer>
    </form> }
}

#[component]
fn RecallDetail(
    recall: InventoryRecallResponse,
    signals: RecallSignals,
    on_release: Callback<()>,
) -> impl IntoView {
    let active = recall.status == InventoryRecallStatus::Active;
    let trace = trace_label(&recall);
    let item = item_label(&recall);
    let note = recall.note.clone();
    let has_note = note.is_some();
    view! { <div class="inventory-trace-detail recall-case-detail">
        <header><div><p class="eyebrow">{format!("Recall case #{}",recall.recall_id)}</p><h2>{item}</h2></div><span class=status_class(recall.status)>{status_label(recall.status)}</span></header>
        <dl class="journal-facts recall-facts"><div><dt>"Client"</dt><dd>{recall.inventory_owner_name}</dd></div><div><dt>"Facility"</dt><dd>{recall.facility_name}</dd></div><div><dt>"Trace"</dt><dd>{trace}</dd></div><div><dt>"Batch"</dt><dd>{format!("#{} · {}",recall.item_batch_id,recall.uom)}</dd></div><div><dt>"Reason"</dt><dd>{reason_label(recall.reason)}</dd></div><div><dt>"Revision"</dt><dd>{recall.revision.get()}</dd></div><div><dt>"Created"</dt><dd>{compact_date(&recall.created_at)}</dd></div><div><dt>"Released"</dt><dd>{recall.released_at.as_deref().map(compact_date).unwrap_or_else(||"Active".into())}</dd></div></dl>
        <div class="recall-metrics"><div><span>"Positions"</span><strong>{recall.affected_position_count}</strong></div><div><span>{if active{"Quantity held"}else{"Quantity released"}}</span><strong>{format!("{} {}",format_quantity(recall.held_quantity),recall.uom)}</strong></div></div>
        <Show when=move || has_note><p class="detail-note recall-case-note">{note.clone().unwrap_or_default()}</p></Show>
        <Show when=move || active fallback=|| view! { <p class="detail-note">"Hold evidence and recall history are immutable. This case is released."</p> }>
            <Show when=move || signals.release_confirm.get() fallback=move || view! { <button type="button" class="button danger-action recall-release-action" on:click=move |_| signals.release_confirm.set(true)>"Release recall holds"</button> }>
                <div class="recall-release-confirm"><strong>"Release every linked hold?"</strong><p>"The case remains in history and all current positions return to their prior availability."</p><div><button type="button" class="button secondary-action" on:click=move |_| signals.release_confirm.set(false)>"Keep active"</button><button type="button" class="button danger-action" disabled=move || signals.command_pending.get() on:click=move |_| on_release.run(())>{move || if signals.command_pending.get(){"Releasing"}else{"Release all holds"}}</button></div></div>
            </Show>
        </Show>
    </div> }
}

fn reset_page(signals: RecallSignals) {
    signals.cursor.set(None);
    signals.history.set(Vec::new());
    signals.selected.set(None);
}

fn reset_candidates(signals: RecallSignals) {
    signals
        .candidate_generation
        .update(|value| *value = value.wrapping_add(1));
    signals
        .candidate_page
        .set(InventoryAgingPage::new(Vec::new(), None));
    signals.selected_candidate.set(None);
    signals.candidate_error.set(None);
    signals.candidate_loading.set(false);
}

#[cfg(target_arch = "wasm32")]
fn request_candidates(signals: RecallSignals, append: bool, on_unauthorized: Callback<()>) {
    let Some(facility_id) = positive_id(&signals.facility_id.get_untracked()) else {
        signals
            .candidate_error
            .set(Some("Choose a facility before searching inventory.".into()));
        return;
    };
    let cursor = if append {
        let Some(cursor) = signals.candidate_page.get_untracked().next_cursor else {
            return;
        };
        Some(cursor)
    } else {
        None
    };
    let generation = signals.candidate_generation.get_untracked().wrapping_add(1);
    signals.candidate_generation.set(generation);
    signals.candidate_loading.set(true);
    signals.candidate_error.set(None);
    let query = optional_text(&signals.batch_query.get_untracked());
    leptos::task::spawn_local(async move {
        let result = api::inventory_aging(
            api::AgingFilters {
                query,
                facility_id: Some(facility_id),
                ..Default::default()
            },
            InventoryAgingSort::Age,
            InventorySortDirection::Descending,
            cursor.as_ref(),
        )
        .await;
        if signals.candidate_generation.get_untracked() != generation {
            return;
        }
        match result {
            Ok(page) => {
                if append {
                    let current = signals.candidate_page.get_untracked().items;
                    signals.candidate_page.set(InventoryAgingPage::new(
                        merge_candidates(current, page.items),
                        page.next_cursor,
                    ));
                } else {
                    signals.candidate_page.set(InventoryAgingPage::new(
                        merge_candidates(Vec::new(), page.items),
                        page.next_cursor,
                    ));
                }
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => signals.candidate_error.set(Some(error.message)),
        }
        signals.candidate_loading.set(false);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn request_candidates(_signals: RecallSignals, _append: bool, _on_unauthorized: Callback<()>) {}

#[cfg(target_arch = "wasm32")]
fn request_recalls(signals: RecallSignals, on_unauthorized: Callback<()>) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    signals.error.set(None);
    let facility_id = positive_id(&signals.facility_filter.get_untracked());
    let owner_id = positive_id(&signals.owner_filter.get_untracked());
    let status = signals.status.get_untracked();
    let cursor = signals.cursor.get_untracked();
    leptos::task::spawn_local(async move {
        let result = api::inventory_recalls(facility_id, owner_id, status, cursor.as_ref()).await;
        if signals.generation.get_untracked() != generation {
            return;
        }
        match result {
            Ok(page) => signals.page.set(page),
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
        signals.loading.set(false);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn request_recalls(_signals: RecallSignals, _on_unauthorized: Callback<()>) {}

#[cfg(target_arch = "wasm32")]
fn dispatch_command(
    signals: RecallSignals,
    command: SavedRecallCommand,
    on_unauthorized: Callback<()>,
    toasts: crate::toast::ToastBus,
) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.error.set(None);
    let saved = command.clone();
    leptos::task::spawn_local(async move {
        let result = match command {
            SavedRecallCommand::Create { request, key } => {
                api::create_inventory_recall(&request, &key).await
            }
            SavedRecallCommand::Release {
                recall_id,
                request,
                key,
            } => api::release_inventory_recall(recall_id, &request, &key).await,
        };
        match result {
            Ok(recall) => {
                let was_release = recall.status == InventoryRecallStatus::Released;
                signals.selected.set(Some(recall));
                signals.create_open.set(false);
                signals.release_confirm.set(false);
                signals.retry.set(None);
                signals.cursor.set(None);
                signals.history.set(Vec::new());
                request_recalls(signals, on_unauthorized);
                if was_release {
                    toasts.success("Inventory recall released.")
                } else {
                    toasts.success("Inventory recall opened and batch positions held.")
                }
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => {
                if error.ambiguous_outcome {
                    signals.retry.set(Some(saved));
                } else {
                    signals.retry.set(None);
                }
                signals.error.set(Some(error.message));
            }
        }
        signals.command_pending.set(false);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_command(
    _signals: RecallSignals,
    _command: SavedRecallCommand,
    _on_unauthorized: Callback<()>,
    _toasts: crate::toast::ToastBus,
) {
}

#[cfg(target_arch = "wasm32")]
fn api_key() -> String {
    api::new_idempotency_key()
}
#[cfg(not(target_arch = "wasm32"))]
fn api_key() -> String {
    "ssr-recall-command".into()
}

fn positive_id(value: &str) -> Option<i64> {
    value.trim().parse().ok().filter(|id| *id > 0)
}
fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn scope_options(values: &[AccessScopeResource]) -> AnyView {
    values
        .iter()
        .map(|value| {
            view! { <option value=value.id.to_string()>{value.name.clone()}</option> }
        })
        .collect_view()
        .into_any()
}
#[cfg(any(target_arch = "wasm32", test))]
fn merge_candidates(
    mut current: Vec<InventoryAgingResponse>,
    next: Vec<InventoryAgingResponse>,
) -> Vec<InventoryAgingResponse> {
    for candidate in next {
        if let Some(existing) = current.iter_mut().find(|existing| {
            existing.facility_id == candidate.facility_id
                && existing.item_batch_id == candidate.item_batch_id
        }) {
            existing.on_hand_quantity = existing
                .on_hand_quantity
                .saturating_add(candidate.on_hand_quantity);
            existing.reserved_quantity = existing
                .reserved_quantity
                .saturating_add(candidate.reserved_quantity);
            existing.held_quantity = existing
                .held_quantity
                .saturating_add(candidate.held_quantity);
            existing.available_quantity = existing
                .available_quantity
                .saturating_add(candidate.available_quantity);
        } else {
            current.push(candidate);
        }
    }
    current
}
fn aging_item_label(value: &InventoryAgingResponse) -> String {
    value
        .primary_sku
        .clone()
        .or_else(|| value.item_description.clone())
        .unwrap_or_else(|| format!("Item #{}", value.item_id))
}
fn aging_trace_label(value: &InventoryAgingResponse) -> String {
    let mut parts = Vec::new();
    if let Some(lot) = value.lot.as_deref() {
        parts.push(format!("Lot {lot}"));
    }
    if let Some(serial) = value.serial.as_deref() {
        parts.push(format!("Serial {serial}"));
    }
    if let Some(expiration) = value.expiration.as_deref() {
        parts.push(format!("Exp {}", compact_date(expiration)));
    }
    if parts.is_empty() {
        "Untracked inventory".into()
    } else {
        parts.join(" / ")
    }
}
fn trace_search_value(value: &InventoryAgingResponse) -> String {
    value
        .lot
        .clone()
        .or_else(|| value.serial.clone())
        .or_else(|| value.primary_sku.clone())
        .or_else(|| value.item_description.clone())
        .unwrap_or_default()
}
fn status_value(value: Option<InventoryRecallStatus>) -> &'static str {
    match value {
        Some(InventoryRecallStatus::Active) => "active",
        Some(InventoryRecallStatus::Released) => "released",
        None => "all",
    }
}
fn parse_status(value: &str) -> Option<InventoryRecallStatus> {
    match value {
        "active" => Some(InventoryRecallStatus::Active),
        "released" => Some(InventoryRecallStatus::Released),
        _ => None,
    }
}
fn reason_value(value: InventoryRecallReason) -> &'static str {
    match value {
        InventoryRecallReason::Regulatory => "regulatory",
        InventoryRecallReason::SupplierNotice => "supplier_notice",
        InventoryRecallReason::CustomerRequest => "customer_request",
        InventoryRecallReason::QualityConcern => "quality_concern",
        InventoryRecallReason::Other => "other",
    }
}
fn parse_reason(value: &str) -> InventoryRecallReason {
    match value {
        "regulatory" => InventoryRecallReason::Regulatory,
        "customer_request" => InventoryRecallReason::CustomerRequest,
        "quality_concern" => InventoryRecallReason::QualityConcern,
        "other" => InventoryRecallReason::Other,
        _ => InventoryRecallReason::SupplierNotice,
    }
}
fn reason_label(value: InventoryRecallReason) -> &'static str {
    match value {
        InventoryRecallReason::Regulatory => "Regulatory",
        InventoryRecallReason::SupplierNotice => "Supplier notice",
        InventoryRecallReason::CustomerRequest => "Customer request",
        InventoryRecallReason::QualityConcern => "Quality concern",
        InventoryRecallReason::Other => "Other",
    }
}
fn status_label(value: InventoryRecallStatus) -> &'static str {
    match value {
        InventoryRecallStatus::Active => "Active",
        InventoryRecallStatus::Released => "Released",
    }
}
fn status_class(value: InventoryRecallStatus) -> &'static str {
    match value {
        InventoryRecallStatus::Active => "status held",
        InventoryRecallStatus::Released => "status muted",
    }
}
fn item_label(value: &InventoryRecallResponse) -> String {
    value
        .primary_sku
        .clone()
        .or_else(|| value.item_description.clone())
        .unwrap_or_else(|| format!("Item #{}", value.item_id))
}
fn trace_label(value: &InventoryRecallResponse) -> String {
    let mut parts = Vec::new();
    if let Some(lot) = value.lot.as_deref() {
        parts.push(format!("Lot {lot}"));
    }
    if let Some(serial) = value.serial.as_deref() {
        parts.push(format!("Serial {serial}"));
    }
    if parts.is_empty() {
        "Untracked".into()
    } else {
        parts.join(" / ")
    }
}
fn compact_date(value: &str) -> String {
    value.get(..10).unwrap_or(value).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recall_values_round_trip() {
        for status in [
            Some(InventoryRecallStatus::Active),
            Some(InventoryRecallStatus::Released),
            None,
        ] {
            assert_eq!(parse_status(status_value(status)), status);
        }
        for reason in [
            InventoryRecallReason::Regulatory,
            InventoryRecallReason::SupplierNotice,
            InventoryRecallReason::CustomerRequest,
            InventoryRecallReason::QualityConcern,
            InventoryRecallReason::Other,
        ] {
            assert_eq!(parse_reason(reason_value(reason)), reason);
        }
    }
    #[test]
    fn other_requires_context_in_submit_policy() {
        assert_eq!(optional_text("   "), None);
        assert_eq!(optional_text(" bulletin "), Some("bulletin".into()));
    }

    #[test]
    fn candidate_pages_collapse_positions_for_the_same_facility_batch() {
        let first = candidate(11, 41, 5, 1, 0);
        let second = candidate(11, 41, 7, 0, 2);
        let other_facility = candidate(12, 41, 3, 0, 0);
        let merged = merge_candidates(vec![first], vec![second, other_facility]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].on_hand_quantity, 12);
        assert_eq!(merged[0].reserved_quantity, 1);
        assert_eq!(merged[0].held_quantity, 2);
        assert_eq!(merged[1].facility_id, 12);
    }

    fn candidate(
        facility_id: i64,
        item_batch_id: i64,
        on_hand_quantity: i64,
        reserved_quantity: i64,
        held_quantity: i64,
    ) -> InventoryAgingResponse {
        InventoryAgingResponse {
            inventory_balance_id: item_batch_id,
            inventory_owner_id: 7,
            inventory_owner_name: "Client".into(),
            facility_id,
            facility_name: "Facility".into(),
            location_id: 9,
            location_name: Some("Reserve".into()),
            location_barcode: Some("R-01".into()),
            license_plate_id: None,
            license_plate_barcode: None,
            item_batch_id,
            item_id: 3,
            primary_sku: Some("SKU-3".into()),
            item_description: Some("Widget".into()),
            uom: "case".into(),
            lot: Some("LOT-1".into()),
            serial: None,
            received_at: "2026-01-01T00:00:00Z".into(),
            age_days: 10,
            expiration: None,
            days_to_expiration: None,
            bucket: wareboxes_api_contract::v1::InventoryAgingBucket::NoExpiration,
            status: wareboxes_api_contract::v1::InventoryBalanceStatus::Available,
            on_hand_quantity,
            reserved_quantity,
            held_quantity,
            available_quantity: on_hand_quantity - reserved_quantity - held_quantity,
        }
    }
}
