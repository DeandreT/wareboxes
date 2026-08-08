use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    OpaqueCursor, OrderAllocationStrategy, PickShortagePage, PickShortageReason,
    PickShortageResponse, PickShortageStatus, ReallocatePickShortageRequest,
};
use wareboxes_api_contract::web::access::AccessScopeResource;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::{use_toast_bus, ToastBus};
use crate::view_model::format_quantity;

type ReallocationRetry = (i64, ReallocatePickShortageRequest, String);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShortageSort {
    Reported,
    Order,
    Client,
    Facility,
    Item,
    Short,
    Remaining,
    Status,
}

#[derive(Clone, Copy)]
struct QueueSignals {
    page: RwSignal<Option<PickShortagePage>>,
    current_cursor: RwSignal<Option<OpaqueCursor>>,
    cursor_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    selected: RwSignal<Option<PickShortageResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    facility_id: RwSignal<String>,
    inventory_owner_id: RwSignal<String>,
    order_id: RwSignal<String>,
    status: RwSignal<String>,
    on_unauthorized: Callback<()>,
}

#[derive(Clone, Copy)]
struct DetailSignals {
    selected: RwSignal<Option<PickShortageResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    command_pending: RwSignal<Option<i64>>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<ReallocationRetry>>,
    queue: QueueSignals,
    toasts: ToastBus,
    on_unauthorized: Callback<()>,
}

#[component]
pub(super) fn PickShortageWorkbench(
    facilities: Vec<AccessScopeResource>,
    inventory_owners: Vec<AccessScopeResource>,
    on_close: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let page = RwSignal::new(None::<PickShortagePage>);
    let selected = RwSignal::new(None::<PickShortageResponse>);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let facility_id = RwSignal::new(String::new());
    let inventory_owner_id = RwSignal::new(String::new());
    let order_id = RwSignal::new(String::new());
    let status = RwSignal::new(String::new());
    let generation = RwSignal::new(0_u64);
    let current_cursor = RwSignal::new(None::<OpaqueCursor>);
    let cursor_history = RwSignal::new(Vec::<Option<OpaqueCursor>>::new());
    let detail_loading = RwSignal::new(false);
    let detail_error = RwSignal::new(None::<String>);
    let detail_generation = RwSignal::new(0_u64);
    let command_pending = RwSignal::new(None::<i64>);
    let command_error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<ReallocationRetry>);
    let sort = RwSignal::new(SortSpec {
        key: ShortageSort::Reported,
        direction: SortDirection::Descending,
    });
    let scoped_facilities = StoredValue::new(facilities);
    let scoped_owners = StoredValue::new(inventory_owners);
    let toasts = use_toast_bus();
    let queue = QueueSignals {
        page,
        current_cursor,
        cursor_history,
        selected,
        loading,
        error,
        generation,
        facility_id,
        inventory_owner_id,
        order_id,
        status,
        on_unauthorized,
    };
    let detail = DetailSignals {
        selected,
        loading: detail_loading,
        error: detail_error,
        generation: detail_generation,
        command_pending,
        command_error,
        retry,
        queue,
        toasts,
        on_unauthorized,
    };

    Effect::new(move |_| {
        request_queue(queue, None, Vec::new());
    });

    let apply_filters = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if let Err(message) = validate_order_filter(&order_id.get_untracked()) {
            error.set(Some(message));
            return;
        }
        selected.set(None);
        detail_error.set(None);
        command_error.set(None);
        request_queue(queue, None, Vec::new());
    };

    let refresh = move |_| {
        request_queue(
            queue,
            current_cursor.get_untracked(),
            cursor_history.get_untracked(),
        );
        if let Some(shortage_id) = selected
            .get_untracked()
            .map(|shortage| shortage.shortage_id)
        {
            request_detail(shortage_id, detail);
        }
    };

    let select_shortage = move |shortage: PickShortageResponse| {
        let shortage_id = shortage.shortage_id;
        selected.set(Some(shortage));
        detail_error.set(None);
        command_error.set(None);
        request_detail(shortage_id, detail);
    };

    let previous_page = move |_| {
        if loading.get_untracked() {
            return;
        }
        let mut history = cursor_history.get_untracked();
        let Some(previous) = history.pop() else {
            return;
        };
        request_queue(queue, previous, history);
    };

    let next_page = move |_| {
        if loading.get_untracked() {
            return;
        }
        let Some(next) = page.get_untracked().and_then(|current| current.next_cursor) else {
            return;
        };
        let mut history = cursor_history.get_untracked();
        history.push(current_cursor.get_untracked());
        request_queue(queue, Some(next), history);
    };

    view! {
        <div class="fulfillment-workbench pick-shortage-workbench">
            <section class="data-section fulfillment-list shortage-list">
                <form class="table-toolbar fulfillment-toolbar shortage-toolbar" on:submit=apply_filters>
                    <div class="toolbar-summary">
                        <strong>{move || page.get().map_or(0, |value| value.items.len())}</strong>
                        <span>"pick exceptions"</span>
                    </div>
                    <div class="fulfillment-filters shortage-filters">
                        <label>
                            <span class="sr-only">"Facility"</span>
                            <select
                                aria-label="Facility"
                                prop:value=move || facility_id.get()
                                on:change=move |event| facility_id.set(event_target_value(&event))
                            >
                                <option value="">"All facilities"</option>
                                {scoped_facilities
                                    .get_value()
                                    .into_iter()
                                    .map(|facility| view! {
                                        <option value=facility.id.to_string()>{facility.name}</option>
                                    })
                                    .collect_view()}
                            </select>
                        </label>
                        <label>
                            <span class="sr-only">"Client"</span>
                            <select
                                aria-label="Client"
                                prop:value=move || inventory_owner_id.get()
                                on:change=move |event| inventory_owner_id.set(event_target_value(&event))
                            >
                                <option value="">"All clients"</option>
                                {scoped_owners
                                    .get_value()
                                    .into_iter()
                                    .map(|owner| view! {
                                        <option value=owner.id.to_string()>{owner.name}</option>
                                    })
                                    .collect_view()}
                            </select>
                        </label>
                        <label class="shortage-order-filter">
                            <span class="sr-only">"Order ID"</span>
                            <input
                                type="number"
                                min="1"
                                step="1"
                                placeholder="Order ID"
                                aria-label="Order ID"
                                prop:value=move || order_id.get()
                                on:input=move |event| order_id.set(event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span class="sr-only">"Exception status"</span>
                            <select
                                aria-label="Exception status"
                                prop:value=move || status.get()
                                on:change=move |event| status.set(event_target_value(&event))
                            >
                                <option value="">"Open exceptions"</option>
                                <option value="awaiting_inventory">"Awaiting inventory"</option>
                                <option value="recovery_in_progress">"Recovery in progress"</option>
                                <option value="resolved">"Resolved"</option>
                            </select>
                        </label>
                        <button class="button secondary-action" type="submit" disabled=move || loading.get()>
                            {move || if loading.get() { "Loading" } else { "Apply" }}
                        </button>
                        <button
                            class="button secondary-action shortage-icon-action"
                            type="button"
                            title="Refresh pick exceptions"
                            aria-label="Refresh pick exceptions"
                            on:click=refresh
                            disabled=move || loading.get()
                        >
                            <Icon icon=UiIcon::Refresh/>
                        </button>
                        <button
                            class="button secondary-action shortage-icon-action"
                            type="button"
                            title="Back to orders"
                            aria-label="Back to orders"
                            on:click=move |_| on_close.run(())
                        >
                            <Icon icon=UiIcon::Back/>
                        </button>
                    </div>
                </form>
                <div class="table-scroll shortage-table-scroll">
                    <table class="data-table fulfillment-table shortage-table">
                        <caption class="sr-only">"Pick shortages matching the active scope filters"</caption>
                        <thead>
                            <tr>
                                <SortableHeader label="Reported" active=move || sort.get().key == ShortageSort::Reported direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, ShortageSort::Reported))/>
                                <SortableHeader label="Order" active=move || sort.get().key == ShortageSort::Order direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, ShortageSort::Order))/>
                                <SortableHeader label="Status" active=move || sort.get().key == ShortageSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, ShortageSort::Status))/>
                                <SortableHeader label="Short" active=move || sort.get().key == ShortageSort::Short direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, ShortageSort::Short)) numeric=true/>
                                <SortableHeader label="Open" active=move || sort.get().key == ShortageSort::Remaining direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, ShortageSort::Remaining)) numeric=true/>
                                <SortableHeader label="Client" active=move || sort.get().key == ShortageSort::Client direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, ShortageSort::Client))/>
                                <SortableHeader label="Item" active=move || sort.get().key == ShortageSort::Item direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, ShortageSort::Item))/>
                                <SortableHeader label="Facility" active=move || sort.get().key == ShortageSort::Facility direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, ShortageSort::Facility))/>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let spec = sort.get();
                                let selected_id = selected.get().map(|value| value.shortage_id);
                                let mut shortages = page.get().map_or_else(Vec::new, |value| value.items);
                                shortages.sort_by(|left, right| shortage_ordering(left, right, spec));
                                shortages
                                    .into_iter()
                                    .map(|shortage| {
                                        let row_shortage = shortage.clone();
                                        let shortage_id = shortage.shortage_id;
                                        view! {
                                            <tr
                                                class:active-row=move || selected_id == Some(shortage_id)
                                                on:click=move |_| select_shortage(row_shortage.clone())
                                            >
                                                <td>{compact_timestamp(&shortage.reported_at)}</td>
                                                <td><strong>{shortage.order_key}</strong><small class="cell-detail">{format!("#{} / Line {}", shortage.order_id, shortage.order_line_id)}</small></td>
                                                <td><span class=shortage_status_class(shortage.status)>{shortage_status_label(shortage.status)}</span></td>
                                                <td class="numeric strong">{format_quantity(shortage.quantities.short)}</td>
                                                <td class="numeric">{format_quantity(shortage.remaining_to_allocate_quantity)}</td>
                                                <td>{shortage.inventory_owner_name}</td>
                                                <td>{shortage.item_description.unwrap_or_else(|| format!("Item #{}", shortage.item_id))}<small class="cell-detail">{shortage.uom.clone()}</small></td>
                                                <td>{shortage.facility_name}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </tbody>
                    </table>
                    <Show when=move || !loading.get() && page.get().is_some_and(|value| value.items.is_empty())>
                        <p class="empty-state">"No pick exceptions match the active scope."</p>
                    </Show>
                </div>
                <div class="table-footer">
                    <span>{move || page.get().map_or_else(|| "Loading exceptions...".to_owned(), |value| format!("{} exceptions on this page", value.items.len()))}</span>
                    <button type="button" class="button secondary-action" disabled=move || cursor_history.get().is_empty() || loading.get() on:click=previous_page>"Previous"</button>
                    <button type="button" class="button secondary-action" disabled=move || !page.get().is_some_and(|value| value.has_more()) || loading.get() on:click=next_page>"Next"</button>
                </div>
                <Show when=move || error.get().is_some()>
                    <p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
                </Show>
            </section>

            <aside class="command-panel fulfillment-detail shortage-detail">
                <Show
                    when=move || selected.get().is_some()
                    fallback=move || view! {
                        <div class="command-placeholder">
                            <Icon icon=UiIcon::Alert/>
                            <h2>"Pick exception"</h2>
                            <p>"Select a shortage to inspect evidence, held stock, and recovery progress."</p>
                        </div>
                    }
                >
                    {move || selected.get().map(|shortage| view! {
                        <PickShortageDetail shortage signals=detail/>
                    })}
                </Show>
            </aside>
        </div>
    }
}

#[component]
fn PickShortageDetail(shortage: PickShortageResponse, signals: DetailSignals) -> impl IntoView {
    let shortage_id = shortage.shortage_id;
    let pending = move || signals.command_pending.get() == Some(shortage_id);
    let retry_for_shortage = move || {
        signals
            .retry
            .get()
            .is_some_and(|(retry_id, _, _)| retry_id == shortage_id)
    };
    let unresolved_retry_elsewhere = move || {
        signals
            .retry
            .get()
            .is_some_and(|(retry_id, _, _)| retry_id != shortage_id)
    };
    let can_reallocate = shortage.status != PickShortageStatus::Resolved
        && shortage.remaining_to_allocate_quantity > 0;
    let recover = move |_| dispatch_reallocation(shortage_id, signals);
    let item_label = shortage
        .item_description
        .clone()
        .unwrap_or_else(|| format!("Item #{}", shortage.item_id));
    let source_label = shortage.source_location_name.clone().map_or_else(
        || shortage.source_location_barcode.clone(),
        |name| format!("{name} / {}", shortage.source_location_barcode),
    );
    let source_plate = shortage
        .source_license_plate_barcode
        .clone()
        .unwrap_or_else(|| "Loose stock".to_owned());
    let trace = trace_label(&shortage);
    let evidence = evidence_label(&shortage);
    let progress = recovery_percent(&shortage);

    view! {
        <div class="fulfillment-detail-content shortage-detail-content">
            <div class="detail-heading">
                <div>
                    <span class="eyebrow">{format!("Pick exception #{}", shortage.shortage_id)}</span>
                    <h2>{shortage.order_key.clone()}</h2>
                </div>
                <span class=shortage_status_class(shortage.status)>{shortage_status_label(shortage.status)}</span>
            </div>
            <dl class="detail-facts four-column">
                <div><dt>"Client"</dt><dd>{shortage.inventory_owner_name.clone()}</dd></div>
                <div><dt>"Facility"</dt><dd>{shortage.facility_name.clone()}</dd></div>
                <div><dt>"Item"</dt><dd>{item_label}</dd></div>
                <div><dt>"Reported"</dt><dd>{compact_timestamp(&shortage.reported_at)}</dd></div>
            </dl>
            <Show when=move || signals.loading.get()>
                <div class="detail-loading" role="status">"Refreshing exception..."</div>
            </Show>
            <Show when=move || signals.error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p>
            </Show>

            <section class="detail-section shortage-progress-section">
                <div class="detail-section-title">
                    <div><h3>"Recovery progress"</h3><span>{format!("Revision {}", shortage.shortage_revision.get())}</span></div>
                    <strong>{format!("{progress}%")}</strong>
                </div>
                <div class="shortage-progress" role="progressbar" aria-label="Recovery progress" aria-valuenow=progress aria-valuemin="0" aria-valuemax="100">
                    <span style=format!("width: {progress}%")></span>
                </div>
                <dl class="shortage-quantity-grid">
                    <div><dt>"Planned"</dt><dd>{format_quantity(shortage.quantities.planned)}</dd></div>
                    <div><dt>"Picked"</dt><dd>{format_quantity(shortage.quantities.picked)}</dd></div>
                    <div><dt>"Short"</dt><dd>{format_quantity(shortage.quantities.short)}</dd></div>
                    <div><dt>"Reallocated"</dt><dd>{format_quantity(shortage.reallocated_quantity)}</dd></div>
                    <div><dt>"RF terminal"</dt><dd>{format_quantity(shortage.recovery_terminal_quantity)}</dd></div>
                    <div><dt>"Open"</dt><dd>{format_quantity(shortage.remaining_to_allocate_quantity)}</dd></div>
                </dl>
            </section>

            <section class="detail-section">
                <div class="detail-section-title"><h3>"Physical exception"</h3><span>{shortage_reason_label(shortage.details.reason)}</span></div>
                <dl class="shortage-record-grid">
                    <div><dt>"Source"</dt><dd>{source_label}</dd></div>
                    <div><dt>"License plate"</dt><dd class="mono">{source_plate}</dd></div>
                    <div><dt>"Trace"</dt><dd>{trace}</dd></div>
                    <div><dt>"Observed"</dt><dd>{evidence}</dd></div>
                    <div><dt>"Hold"</dt><dd>{format!("#{} / {} {}", shortage.hold.hold_id, format_quantity(shortage.hold.held_quantity), shortage.uom)}</dd></div>
                    <div><dt>"Operator"</dt><dd>{format!("User #{}", shortage.reported_by)}</dd></div>
                </dl>
                {shortage.details.note.clone().map(|note| view! { <p class="shortage-note">{note}</p> })}
            </section>

            <section class="detail-section shortage-action-section">
                <div class="detail-section-title">
                    <div><h3>"FEFO recovery"</h3><span>{format!("Order revision {}", shortage.order_revision.get())}</span></div>
                </div>
                <Show when=retry_for_shortage>
                    <p class="shortage-retry-note" role="status">"The previous result is unknown. Retry sends the exact saved command and idempotency key."</p>
                </Show>
                <Show when=unresolved_retry_elsewhere>
                    <p class="shortage-retry-note" role="alert">"Resolve the unknown recovery result on the previously selected exception before starting another recovery."</p>
                </Show>
                <Show when=move || signals.command_error.get().is_some()>
                    <p class="inline-command-error" role="alert">{move || signals.command_error.get().unwrap_or_default()}</p>
                </Show>
                <div class="form-actions">
                    <button
                        type="button"
                        class="button primary-action"
                        disabled=move || pending() || !can_reallocate || unresolved_retry_elsewhere()
                        on:click=recover
                    >
                        <Icon icon=UiIcon::Release/>
                        {move || if pending() { "Recovering" } else if retry_for_shortage() { "Retry recovery" } else { "Allocate replacement" }}
                    </button>
                    <button
                        type="button"
                        class="button secondary-action"
                        disabled=move || signals.loading.get() || pending()
                        on:click=move |_| request_detail(shortage_id, signals)
                    >
                        <Icon icon=UiIcon::Refresh/>
                        "Refresh"
                    </button>
                </div>
                {(!can_reallocate).then(|| view! {
                    <p class="shortage-action-state">{if shortage.status == PickShortageStatus::Resolved { "Recovery is complete." } else { "Replacement work is already allocated and awaiting RF execution." }}</p>
                })}
            </section>
        </div>
    }
}

fn request_queue(
    signals: QueueSignals,
    cursor: Option<OpaqueCursor>,
    next_history: Vec<Option<OpaqueCursor>>,
) {
    signals
        .generation
        .update(|generation| *generation = generation.saturating_add(1));
    let generation = signals.generation.get_untracked();
    let facility_id = parse_filter_id(&signals.facility_id.get_untracked());
    let inventory_owner_id = parse_filter_id(&signals.inventory_owner_id.get_untracked());
    let order_id = parse_filter_id(&signals.order_id.get_untracked());
    let status = parse_shortage_status(&signals.status.get_untracked());
    signals.loading.set(true);
    signals.error.set(None);

    leptos::task::spawn_local(async move {
        let response = api::pick_shortages(
            facility_id,
            inventory_owner_id,
            order_id,
            status,
            cursor.as_ref(),
        )
        .await;
        if signals.generation.get_untracked() != generation {
            return;
        }
        match response {
            Ok(next) => {
                let selected_id = signals
                    .selected
                    .get_untracked()
                    .map(|shortage| shortage.shortage_id);
                let next_selected = selected_id
                    .and_then(|selected_id| {
                        next.items
                            .iter()
                            .find(|shortage| shortage.shortage_id == selected_id)
                            .cloned()
                    })
                    .or_else(|| next.items.first().cloned());
                signals.current_cursor.set(cursor);
                signals.cursor_history.set(next_history);
                signals.page.set(Some(next));
                signals.selected.set(next_selected);
                signals.loading.set(false);
            }
            Err(api_error) if api_error.unauthorized => {
                signals.loading.set(false);
                signals.on_unauthorized.run(());
            }
            Err(api_error) => {
                signals.loading.set(false);
                signals.error.set(Some(api_error.message));
            }
        }
    });
}

fn request_detail(shortage_id: i64, signals: DetailSignals) {
    signals
        .generation
        .update(|generation| *generation = generation.saturating_add(1));
    let generation = signals.generation.get_untracked();
    signals.loading.set(true);
    signals.error.set(None);

    leptos::task::spawn_local(async move {
        let response = api::pick_shortage(shortage_id).await;
        let is_current = signals.generation.get_untracked() == generation
            && signals
                .selected
                .get_untracked()
                .is_some_and(|selected| selected.shortage_id == shortage_id);
        if !is_current {
            return;
        }
        match response {
            Ok(shortage) => {
                signals.selected.set(Some(shortage));
                signals.loading.set(false);
            }
            Err(api_error) if api_error.unauthorized => {
                signals.loading.set(false);
                signals.on_unauthorized.run(());
            }
            Err(api_error) => {
                signals.loading.set(false);
                signals.error.set(Some(api_error.message));
            }
        }
    });
}

fn dispatch_reallocation(shortage_id: i64, signals: DetailSignals) {
    if signals.command_pending.get_untracked().is_some() {
        return;
    }
    let (request, idempotency_key) = match signals.retry.get_untracked() {
        Some((retry_id, request, key)) if retry_id == shortage_id => (request, key),
        Some((retry_id, _, _)) => {
            signals.command_error.set(Some(format!(
                "Resolve the unknown result for pick exception #{retry_id} first."
            )));
            return;
        }
        None => {
            let Some(shortage) = signals.selected.get_untracked() else {
                return;
            };
            (
                ReallocatePickShortageRequest {
                    expected_shortage_revision: shortage.shortage_revision,
                    expected_order_revision: shortage.order_revision,
                    strategy: OrderAllocationStrategy::Fefo,
                },
                api::new_idempotency_key(),
            )
        }
    };
    signals
        .retry
        .set(Some((shortage_id, request, idempotency_key.clone())));
    signals.command_pending.set(Some(shortage_id));
    signals.command_error.set(None);

    leptos::task::spawn_local(async move {
        let response = api::reallocate_pick_shortage(shortage_id, &request, &idempotency_key).await;
        if signals.command_pending.get_untracked() == Some(shortage_id) {
            signals.command_pending.set(None);
        }
        match response {
            Ok(result) => {
                if signals
                    .retry
                    .get_untracked()
                    .is_some_and(|(retry_id, _, _)| retry_id == shortage_id)
                {
                    signals.retry.set(None);
                }
                signals.command_error.set(None);
                signals.toasts.success(format!(
                    "Pick exception #{shortage_id}: {} {} allocated, {} still open.",
                    format_quantity(result.newly_allocated_quantity),
                    result
                        .new_allocations
                        .first()
                        .map_or("units", |allocation| allocation.execution_stage_label()),
                    format_quantity(result.remaining_to_allocate_quantity)
                ));
                refresh_after_reallocation(shortage_id, signals);
            }
            Err(api_error) if api_error.unauthorized => {
                signals.retry.set(None);
                signals.on_unauthorized.run(());
            }
            Err(api_error) if api_error.ambiguous_outcome => {
                let message = format!(
                    "{} The result is unknown; retry the saved recovery command.",
                    api_error.message
                );
                signals.command_error.set(Some(message));
                signals.toasts.error(api_error.message);
            }
            Err(api_error) => {
                signals.retry.set(None);
                signals.command_error.set(Some(format!(
                    "{} Authoritative shortage revisions were refreshed.",
                    api_error.message
                )));
                signals.toasts.error(api_error.message);
                refresh_after_reallocation(shortage_id, signals);
            }
        }
    });
}

fn refresh_after_reallocation(shortage_id: i64, signals: DetailSignals) {
    request_queue(
        signals.queue,
        signals.queue.current_cursor.get_untracked(),
        signals.queue.cursor_history.get_untracked(),
    );
    if signals
        .selected
        .get_untracked()
        .is_some_and(|selected| selected.shortage_id == shortage_id)
    {
        request_detail(shortage_id, signals);
    }
}

fn shortage_ordering(
    left: &PickShortageResponse,
    right: &PickShortageResponse,
    spec: SortSpec<ShortageSort>,
) -> std::cmp::Ordering {
    let ordering = match spec.key {
        ShortageSort::Reported => left.reported_at.cmp(&right.reported_at),
        ShortageSort::Order => left
            .order_key
            .to_ascii_lowercase()
            .cmp(&right.order_key.to_ascii_lowercase()),
        ShortageSort::Client => left
            .inventory_owner_name
            .to_ascii_lowercase()
            .cmp(&right.inventory_owner_name.to_ascii_lowercase()),
        ShortageSort::Facility => left
            .facility_name
            .to_ascii_lowercase()
            .cmp(&right.facility_name.to_ascii_lowercase()),
        ShortageSort::Item => left.item_id.cmp(&right.item_id),
        ShortageSort::Short => left.quantities.short.cmp(&right.quantities.short),
        ShortageSort::Remaining => left
            .remaining_to_allocate_quantity
            .cmp(&right.remaining_to_allocate_quantity),
        ShortageSort::Status => {
            shortage_status_wire(left.status).cmp(shortage_status_wire(right.status))
        }
    }
    .then_with(|| left.shortage_id.cmp(&right.shortage_id));
    if spec.direction == SortDirection::Ascending {
        ordering
    } else {
        ordering.reverse()
    }
}

fn validate_order_filter(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.parse::<i64>().is_ok_and(|id| id > 0) {
        Ok(())
    } else {
        Err("Order ID must be a positive whole number.".to_owned())
    }
}

fn parse_filter_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|id| *id > 0)
}

fn parse_shortage_status(value: &str) -> Option<PickShortageStatus> {
    match value {
        "awaiting_inventory" => Some(PickShortageStatus::AwaitingInventory),
        "recovery_in_progress" => Some(PickShortageStatus::RecoveryInProgress),
        "resolved" => Some(PickShortageStatus::Resolved),
        _ => None,
    }
}

const fn shortage_status_wire(status: PickShortageStatus) -> &'static str {
    match status {
        PickShortageStatus::AwaitingInventory => "awaiting_inventory",
        PickShortageStatus::RecoveryInProgress => "recovery_in_progress",
        PickShortageStatus::Resolved => "resolved",
    }
}

const fn shortage_status_label(status: PickShortageStatus) -> &'static str {
    match status {
        PickShortageStatus::AwaitingInventory => "Awaiting inventory",
        PickShortageStatus::RecoveryInProgress => "Recovery in progress",
        PickShortageStatus::Resolved => "Resolved",
    }
}

const fn shortage_status_class(status: PickShortageStatus) -> &'static str {
    match status {
        PickShortageStatus::AwaitingInventory => "status held",
        PickShortageStatus::RecoveryInProgress => "status processing",
        PickShortageStatus::Resolved => "status shipped",
    }
}

const fn shortage_reason_label(reason: PickShortageReason) -> &'static str {
    match reason {
        PickShortageReason::InventoryMissing => "Inventory missing",
        PickShortageReason::InsufficientQuantity => "Insufficient quantity",
        PickShortageReason::DamagedInventory => "Damaged inventory",
        PickShortageReason::WrongInventory => "Wrong inventory",
        PickShortageReason::LotOrSerialMismatch => "Lot or serial mismatch",
        PickShortageReason::Other => "Other",
    }
}

fn compact_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

fn trace_label(shortage: &PickShortageResponse) -> String {
    let values = [
        shortage.lot.as_deref().map(|lot| format!("Lot {lot}")),
        shortage
            .serial
            .as_deref()
            .map(|serial| format!("Serial {serial}")),
        shortage
            .expiration
            .as_deref()
            .map(|expiration| format!("Exp. {expiration}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if values.is_empty() {
        "Uncontrolled".to_owned()
    } else {
        values.join(" / ")
    }
}

fn evidence_label(shortage: &PickShortageResponse) -> String {
    let values = [
        shortage.observed_item_barcode.as_deref(),
        shortage.observed_lot.as_deref(),
        shortage.observed_serial.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if values.is_empty() {
        "No scanned evidence".to_owned()
    } else {
        values.join(" / ")
    }
}

fn recovery_percent(shortage: &PickShortageResponse) -> i64 {
    if shortage.quantities.short == 0 {
        return 100;
    }
    shortage
        .recovery_terminal_quantity
        .saturating_mul(100)
        .checked_div(shortage.quantities.short)
        .unwrap_or(0)
        .clamp(0, 100)
}

trait ExecutionStageLabel {
    fn execution_stage_label(&self) -> &'static str;
}

impl ExecutionStageLabel for wareboxes_api_contract::v1::PickShortageAllocationResponse {
    fn execution_stage_label(&self) -> &'static str {
        match self.execution_stage {
            wareboxes_api_contract::v1::AllocationExecutionStage::PickSource => "units",
            wareboxes_api_contract::v1::AllocationExecutionStage::Staged => "staged units",
            wareboxes_api_contract::v1::AllocationExecutionStage::Packed => "packed units",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_filter_parsing_fails_closed_for_invalid_ids() {
        assert_eq!(parse_filter_id(" 41 "), Some(41));
        assert_eq!(parse_filter_id("0"), None);
        assert_eq!(parse_filter_id("bad"), None);
        assert!(validate_order_filter("41").is_ok());
        assert!(validate_order_filter("bad").is_err());
    }

    #[test]
    fn shortage_status_filter_uses_contract_wire_values() {
        assert_eq!(
            parse_shortage_status("recovery_in_progress"),
            Some(PickShortageStatus::RecoveryInProgress)
        );
        assert_eq!(parse_shortage_status(""), None);
        assert_eq!(
            shortage_status_wire(PickShortageStatus::AwaitingInventory),
            "awaiting_inventory"
        );
    }

    #[test]
    fn compact_timestamps_do_not_panic_on_short_server_values() {
        assert_eq!(
            compact_timestamp("2026-08-08T12:34:56Z"),
            "2026-08-08 12:34"
        );
        assert_eq!(compact_timestamp("pending"), "pending");
    }
}
