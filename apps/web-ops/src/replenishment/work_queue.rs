use leptos::prelude::*;
use lucide_leptos::{ArchiveX, Eye};
use wareboxes_api_contract::v1::{ReplenishmentQueueEntryResponse, ReplenishmentWorkStatus};

use super::model::{
    compact_timestamp, work_status_class, work_status_label, WorkPageSignals, WorkSort,
};
use crate::sorting::SortableHeader;
use crate::view_model::format_quantity;

#[component]
pub(super) fn WorkQueuePanel(
    signals: WorkPageSignals,
    on_previous: Callback<()>,
    on_next: Callback<()>,
    on_cancel: Callback<ReplenishmentQueueEntryResponse>,
    on_sort: Callback<WorkSort>,
) -> impl IntoView {
    let sort = signals.sort;
    let selected_id = RwSignal::new(None::<i64>);
    let selected = move || {
        let selected_id = selected_id.get()?;
        signals
            .page
            .get()?
            .items
            .into_iter()
            .find(|work| work.work_id == selected_id)
    };

    view! {
        <section class="data-section replenishment-work-section">
            <div class="replenishment-work-summary" aria-label="Work queue summary">
                <span><small>"Work on page"</small><strong>{move || signals.page.get().map_or(0, |page| page.items.len())}</strong></span>
                <span><small>"Pending"</small><strong>{move || count_status(signals, ReplenishmentWorkStatus::Pending)}</strong></span>
                <span><small>"Claimed"</small><strong>{move || count_status(signals, ReplenishmentWorkStatus::Claimed)}</strong></span>
                <span><small>"Quantity"</small><strong>{move || format_quantity(signals.page.get().map_or(0_i64, |page| page.items.iter().map(|work| work.quantity).sum()))}</strong></span>
                <span class="replenishment-poll-state">{move || if signals.loading.get() { "Refreshing" } else { "Live monitor" }}</span>
            </div>
            <div class="table-scroll replenishment-work-scroll">
                <table class="data-table replenishment-work-table">
                    <caption class="sr-only">"Replenishment execution work matching the active scope"</caption>
                    <thead>
                        <tr>
                            <SortableHeader label="Created" active=move || sort.get().key == WorkSort::Created direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Created))/>
                            <SortableHeader label="Pri" active=move || sort.get().key == WorkSort::Priority direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Priority)) numeric=true/>
                            <SortableHeader label="Client" active=move || sort.get().key == WorkSort::Client direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Client))/>
                            <SortableHeader label="Facility" active=move || sort.get().key == WorkSort::Facility direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Facility))/>
                            <SortableHeader label="Item" active=move || sort.get().key == WorkSort::Item direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Item))/>
                            <SortableHeader label="Source" active=move || sort.get().key == WorkSort::Source direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Source))/>
                            <SortableHeader label="Pick face" active=move || sort.get().key == WorkSort::Destination direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Destination))/>
                            <SortableHeader label="Qty" active=move || sort.get().key == WorkSort::Quantity direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Quantity)) numeric=true/>
                            <th>"Trace"</th>
                            <SortableHeader label="Status" active=move || sort.get().key == WorkSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Status))/>
                            <SortableHeader label="Lease / due" active=move || sort.get().key == WorkSort::Lease direction=move || sort.get().direction on_sort=Callback::new(move |_| on_sort.run(WorkSort::Lease))/>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let current_selected = selected_id.get();
                            let work = signals.page.get().map_or_else(Vec::new, |page| page.items);
                            if work.is_empty() && !signals.loading.get() {
                                view! {
                                    <tr><td class="table-empty-row" colspan="11">"No replenishment work matches this scope."</td></tr>
                                }.into_any()
                            } else {
                                work.into_iter().map(|entry| {
                                    let work_id = entry.work_id;
                                    let item = entry.item_description.clone().unwrap_or_else(|| format!("Item #{}", entry.item_id));
                                    let sku = entry.primary_sku.clone().unwrap_or_else(|| format!("ID {}", entry.item_id));
                                    let source = location_name(&entry.source_location);
                                    let destination = location_name(&entry.destination_pick_face);
                                    let trace = trace_label(&entry);
                                    let lease = lease_label(&entry);
                                    view! {
                                        <tr
                                            class:active-row=move || current_selected == Some(work_id)
                                            on:click=move |_| selected_id.set(Some(work_id))
                                        >
                                            <td>
                                                <div class="replenishment-work-primary">
                                                    <button
                                                        type="button"
                                                        class="replenishment-work-detail-button"
                                                        class:active=move || selected_id.get() == Some(work_id)
                                                        title="View work detail"
                                                        aria-label=format!("View detail for replenishment work {work_id}")
                                                        aria-pressed=move || selected_id.get() == Some(work_id)
                                                        on:click=move |_| selected_id.set(Some(work_id))
                                                    >
                                                        <Eye size=13/>
                                                    </button>
                                                    <span>
                                                        {compact_timestamp(&entry.created_at)}
                                                        <small class="cell-detail">{format!("Work #{} / Plan #{}", entry.work_id, entry.plan_id)}</small>
                                                    </span>
                                                </div>
                                            </td>
                                            <td class="numeric strong">{entry.priority}</td>
                                            <td>{entry.inventory_owner_name}</td>
                                            <td>{entry.facility_name}</td>
                                            <td><strong>{item}</strong><small class="cell-detail">{format!("{} / {}", sku, entry.uom)}</small></td>
                                            <td><span class="mono">{source}</span><small class="cell-detail">{entry.source_location.barcode}</small></td>
                                            <td><span class="mono">{destination}</span><small class="cell-detail">{entry.destination_pick_face.barcode}</small></td>
                                            <td class="numeric strong">{format_quantity(entry.quantity)}</td>
                                            <td>{trace}</td>
                                            <td><span class=work_status_class(entry.status)>{work_status_label(entry.status)}</span><small class="cell-detail">{entry.claimed_by.map_or_else(|| "Unassigned".to_owned(), |user| format!("User #{user}"))}</small></td>
                                            <td>{lease}</td>
                                        </tr>
                                    }
                                }).collect_view().into_any()
                            }
                        }}
                    </tbody>
                </table>
                <Show when=move || signals.loading.get()>
                    <div class="replenishment-table-loading" role="status">"Refreshing work monitor..."</div>
                </Show>
            </div>
            <Show when=move || selected().is_some()>
                {move || selected().map(|entry| view! { <WorkDetail entry on_cancel/> })}
            </Show>
            <Show when=move || signals.error.get().is_some()>
                <p class="inline-command-error replenishment-page-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p>
            </Show>
            <div class="table-footer">
                <span>{move || signals.page.get().map_or_else(|| "Loading work...".to_owned(), |page| format!("{} tasks on this page", page.items.len()))}</span>
                <button type="button" class="button secondary-action" disabled=move || signals.loading.get() || signals.cursor_history.get().is_empty() on:click=move |_| on_previous.run(())>"Previous"</button>
                <button type="button" class="button secondary-action" disabled=move || signals.loading.get() || !signals.page.get().is_some_and(|page| page.has_more()) on:click=move |_| on_next.run(())>"Next"</button>
            </div>
        </section>
    }
}

#[component]
fn WorkDetail(
    entry: ReplenishmentQueueEntryResponse,
    on_cancel: Callback<ReplenishmentQueueEntryResponse>,
) -> impl IntoView {
    let cancellation_entry = StoredValue::new(entry.clone());
    let pending = entry.status == ReplenishmentWorkStatus::Pending;
    view! {
        <section class="replenishment-work-detail" aria-label=format!("Work {} detail", entry.work_id)>
            <div>
                <span>"Work / plan / policy"</span>
                <strong>{format!("#{} / #{} / #{}", entry.work_id, entry.plan_id, entry.policy_id)}</strong>
            </div>
            <div>
                <span>"Source balance / batch"</span>
                <strong>{format!("#{} / #{}", entry.source_inventory_balance_id, entry.item_batch_id)}</strong>
            </div>
            <div>
                <span>"Sequence / policy rev"</span>
                <strong>{format!("{} / {}", entry.sequence, entry.policy_revision.get())}</strong>
            </div>
            <div>
                <span>"Expiration"</span>
                <strong>{entry.expiration.as_deref().map_or_else(|| "Uncontrolled".to_owned(), compact_timestamp)}</strong>
            </div>
            <div>
                <span>"Completed"</span>
                <strong>{entry.completed_at.as_deref().map_or_else(|| "Open".to_owned(), compact_timestamp)}</strong>
            </div>
            <div class="replenishment-work-actions">
                <Show when=move || pending>
                    <button
                        type="button"
                        class="button danger-action"
                        on:click=move |_| on_cancel.run(cancellation_entry.get_value())
                    >
                        <ArchiveX size=14/>
                        "Cancel"
                    </button>
                </Show>
            </div>
        </section>
    }
}

fn count_status(signals: WorkPageSignals, status: ReplenishmentWorkStatus) -> usize {
    signals.page.get().map_or(0, |page| {
        page.items
            .iter()
            .filter(|work| work.status == status)
            .count()
    })
}

fn location_name(location: &wareboxes_api_contract::v1::ReplenishmentLocationResponse) -> String {
    location
        .name
        .clone()
        .unwrap_or_else(|| location.barcode.clone())
}

fn trace_label(entry: &ReplenishmentQueueEntryResponse) -> String {
    let trace = [
        entry.lot.as_deref().map(|lot| format!("Lot {lot}")),
        entry
            .serial
            .as_deref()
            .map(|serial| format!("Serial {serial}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if trace.is_empty() {
        "Uncontrolled".to_owned()
    } else {
        trace.join(" / ")
    }
}

fn lease_label(entry: &ReplenishmentQueueEntryResponse) -> String {
    entry.lease_expires_at.as_deref().map_or_else(
        || {
            entry
                .due_at
                .as_deref()
                .map_or_else(|| "No lease".to_owned(), compact_timestamp)
        },
        |lease| format!("Lease {}", compact_timestamp(lease)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_timestamps_are_safe_for_live_partial_values() {
        assert_eq!(compact_timestamp("pending"), "pending");
        assert_eq!(
            compact_timestamp("2026-08-08T12:34:56Z"),
            "2026-08-08 12:34"
        );
    }
}
