use leptos::prelude::*;
use lucide_leptos::Eye;
use wareboxes_api_contract::v1::{
    InventoryAgingBucket, InventoryAgingPage, InventoryAgingResponse, InventoryAgingSort,
    InventorySortDirection, OpaqueCursor,
};

#[cfg(target_arch = "wasm32")]
use crate::api;
use crate::api::AgingFilters;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortableHeader};
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, Copy)]
struct AgingSignals {
    page: RwSignal<InventoryAgingPage>,
    selected: RwSignal<Option<InventoryAgingResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(dead_code, reason = "hydration guards asynchronous aging responses")
    )]
    generation: RwSignal<u64>,
    search: RwSignal<String>,
    applied_search: RwSignal<String>,
    item_id: RwSignal<String>,
    bucket: RwSignal<Option<InventoryAgingBucket>>,
    sort: RwSignal<InventoryAgingSort>,
    direction: RwSignal<InventorySortDirection>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
}

impl AgingSignals {
    fn new() -> Self {
        Self {
            page: RwSignal::new(InventoryAgingPage::new(Vec::new(), None)),
            selected: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            generation: RwSignal::new(0),
            search: RwSignal::new(String::new()),
            applied_search: RwSignal::new(String::new()),
            item_id: RwSignal::new(String::new()),
            bucket: RwSignal::new(None),
            sort: RwSignal::new(InventoryAgingSort::Age),
            direction: RwSignal::new(InventorySortDirection::Descending),
            cursor: RwSignal::new(None),
            history: RwSignal::new(Vec::new()),
        }
    }

    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(dead_code, reason = "hydration builds the inventory aging request")
    )]
    fn filters(self) -> AgingFilters {
        let search = self.applied_search.get_untracked();
        AgingFilters {
            query: (!search.is_empty()).then_some(search),
            item_id: positive_id(&self.item_id.get_untracked()),
            bucket: self.bucket.get_untracked(),
            ..Default::default()
        }
    }
}

#[component]
pub(super) fn AgingView(on_unauthorized: Callback<()>) -> impl IntoView {
    let signals = AgingSignals::new();
    let layout = SplitPaneState::new("inventory-aging", 820);
    Effect::new(move || request_aging(signals, on_unauthorized));

    let apply = move |_| {
        signals
            .applied_search
            .set(signals.search.get_untracked().trim().to_owned());
        reset_page(signals);
        request_aging(signals, on_unauthorized);
    };
    let next = move |_| {
        let Some(cursor) = signals.page.get_untracked().next_cursor else {
            return;
        };
        signals
            .history
            .update(|history| history.push(signals.cursor.get_untracked()));
        signals.cursor.set(Some(cursor));
        request_aging(signals, on_unauthorized);
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
        request_aging(signals, on_unauthorized);
    };

    view! {
        <section class="integrity-read-view aging-view">
            <div class="integrity-query-bar aging">
                <SearchField
                    label="Search aging inventory trace dimensions".to_owned()
                    placeholder="SKU, lot, serial, location, client"
                    value=signals.search
                />
                <label>
                    <span>"Expiry risk"</span>
                    <select
                        prop:value=move || bucket_value(signals.bucket.get())
                        on:change=move |event| {
                            signals.bucket.set(parse_bucket(&event_target_value(&event)));
                            reset_page(signals);
                            request_aging(signals, on_unauthorized);
                        }
                    >
                        <option value="all">"All inventory"</option>
                        <option value="expired">"Expired"</option>
                        <option value="due_within_7_days">"Due in 7 days"</option>
                        <option value="due_within_30_days">"Due in 30 days"</option>
                        <option value="due_within_90_days">"Due in 90 days"</option>
                        <option value="beyond_90_days">"Beyond 90 days"</option>
                        <option value="no_expiration">"No expiration"</option>
                    </select>
                </label>
                <label>
                    <span>"Item ID"</span>
                    <input
                        inputmode="numeric"
                        placeholder="Any"
                        prop:value=move || signals.item_id.get()
                        on:input=move |event| signals.item_id.set(event_target_value(&event))
                    />
                </label>
                <button
                    type="button"
                    class="button primary-action"
                    disabled=move || signals.loading.get()
                    on:click=apply
                >
                    "Apply"
                </button>
                <div class="integrity-health" class:attention=move || {
                    signals.page.get().items.iter().any(|row| {
                        matches!(row.bucket, InventoryAgingBucket::Expired | InventoryAgingBucket::DueWithin7Days)
                    })
                }>
                    <strong>{move || signals.page.get().items.len()}</strong>
                    <span>"positions on page"</span>
                </div>
                <PaneControls layout master_label="aging table" detail_label="aging detail"/>
            </div>
            <Show when=move || signals.error.get().is_some()>
                <p class="inline-command-error" role="alert">
                    {move || signals.error.get().unwrap_or_default()}
                </p>
            </Show>
            <div
                class="integrity-read-split split-workspace"
                style=move || layout.style()
                data-pane-mode=move || layout.mode_attribute()
            >
                <section class="data-section split-master integrity-read-master">
                    <div class="table-toolbar compact">
                        <div class="toolbar-summary">
                            <strong>{move || signals.page.get().items.len()}</strong>
                            <span>"current positions"</span>
                        </div>
                        <span>{move || if signals.loading.get() { "Loading aging inventory" } else { "Server-sorted aging" }}</span>
                    </div>
                    <div class="table-scroll">
                        <table class="data-table aging-table">
                            <thead><tr>
                                <SortableHeader label="Item" active=move || signals.sort.get()==InventoryAgingSort::Item direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals,InventoryAgingSort::Item,on_unauthorized))/>
                                <SortableHeader label="Client" active=move || signals.sort.get()==InventoryAgingSort::Client direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals,InventoryAgingSort::Client,on_unauthorized))/>
                                <SortableHeader label="Facility" active=move || signals.sort.get()==InventoryAgingSort::Facility direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals,InventoryAgingSort::Facility,on_unauthorized))/>
                                <th>"Trace"</th>
                                <SortableHeader label="Age" active=move || signals.sort.get()==InventoryAgingSort::Age direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals,InventoryAgingSort::Age,on_unauthorized)) numeric=true/>
                                <SortableHeader label="Expiration" active=move || signals.sort.get()==InventoryAgingSort::Expiration direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals,InventoryAgingSort::Expiration,on_unauthorized))/>
                                <SortableHeader label="On hand" active=move || signals.sort.get()==InventoryAgingSort::Quantity direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_sort(signals,InventoryAgingSort::Quantity,on_unauthorized)) numeric=true/>
                                <th class="icon-column"><span class="sr-only">"Detail"</span></th>
                            </tr></thead>
                            <tbody>{move || aging_rows(signals, layout)}</tbody>
                        </table>
                    </div>
                    <div class="table-footer">
                        <span>{move || if signals.page.get().next_cursor.is_some() { "More aging positions available" } else { "End of aging results" }}</span>
                        <div>
                            <button type="button" class="button secondary-action" disabled=move || signals.history.get().is_empty()||signals.loading.get() on:click=previous>"Previous"</button>
                            <button type="button" class="button secondary-action" disabled=move || signals.page.get().next_cursor.is_none()||signals.loading.get() on:click=next>"Next"</button>
                        </div>
                    </div>
                </section>
                <SplitPaneHandle layout/>
                <aside class="data-section split-detail journal-detail">
                    {move || signals.selected.get().map_or_else(
                        || view! { <div class="journal-detail-empty"><h2>"Aging detail"</h2><p>"Select a position to inspect age, expiry, commitments, and trace identity."</p></div> }.into_any(),
                        |position| view! { <AgingDetail position/> }.into_any(),
                    )}
                </aside>
            </div>
        </section>
    }
}

fn aging_rows(signals: AgingSignals, layout: SplitPaneState) -> AnyView {
    let rows = signals.page.get().items;
    if rows.is_empty() && !signals.loading.get() {
        return view! { <tr><td colspan="8" class="table-empty-row">"No current inventory matches the aging filters."</td></tr> }.into_any();
    }
    rows.into_iter()
        .map(|position| {
            let row = position.clone();
            let action = position.clone();
            let id = position.inventory_balance_id;
            let selected = signals.selected.get().is_some_and(|value| value.inventory_balance_id == id);
            let item = item_label(&position);
            let trace = trace_label(&position);
            let risk = bucket_label(position.bucket);
            let risk_class = bucket_class(position.bucket);
            let client = position.inventory_owner_name.clone();
            let facility = position.facility_name.clone();
            let location = position
                .location_barcode
                .clone()
                .or_else(|| position.location_name.clone())
                .unwrap_or_else(|| format!("#{}", position.location_id));
            let batch_id = position.item_batch_id;
            let uom = position.uom.clone();
            let age_days = position.age_days;
            let expiration = expiration_label(&position);
            let on_hand = format_quantity(position.on_hand_quantity);
            let available = format_quantity(position.available_quantity);
            view! {
                <tr class:selected=selected on:click=move |_| { signals.selected.set(Some(row.clone())); layout.show_detail(); }>
                    <td><strong>{item}</strong><small class="cell-detail">{format!("Batch #{batch_id} · {uom}")}</small></td>
                    <td>{client}</td>
                    <td>{facility}<small class="cell-detail">{location}</small></td>
                    <td>{trace}</td>
                    <td class="numeric strong">{format!("{age_days}d")}</td>
                    <td><span class=risk_class>{risk}</span><small class="cell-detail">{expiration}</small></td>
                    <td class="numeric strong">{on_hand}<small class="cell-detail">{format!("{available} free")}</small></td>
                    <td class="icon-column"><button type="button" class="icon-button compact" title="View aging detail" aria-label=format!("View aging detail for balance {id}") aria-pressed=selected on:click=move |event| { event.stop_propagation(); signals.selected.set(Some(action.clone())); layout.show_detail(); }><Eye size=13/></button></td>
                </tr>
            }
        })
        .collect_view()
        .into_any()
}

#[component]
fn AgingDetail(position: InventoryAgingResponse) -> impl IntoView {
    let item = item_label(&position);
    let trace = trace_label(&position);
    let location = position
        .location_barcode
        .clone()
        .or_else(|| position.location_name.clone())
        .unwrap_or_else(|| format!("#{}", position.location_id));
    let license_plate = position
        .license_plate_barcode
        .clone()
        .unwrap_or_else(|| "Loose stock".into());
    let expiration = expiration_label(&position);
    view! {
        <div class="inventory-trace-detail aging-detail">
            <header>
                <div><p class="eyebrow">"Current inventory age"</p><h2>{item}</h2></div>
                <span class=bucket_class(position.bucket)>{bucket_label(position.bucket)}</span>
            </header>
            <dl class="journal-facts aging-facts">
                <div><dt>"Client"</dt><dd>{position.inventory_owner_name}</dd></div>
                <div><dt>"Facility"</dt><dd>{position.facility_name}</dd></div>
                <div><dt>"Location"</dt><dd>{location}</dd></div>
                <div><dt>"License plate"</dt><dd>{license_plate}</dd></div>
                <div><dt>"Trace identity"</dt><dd>{trace}</dd></div>
                <div><dt>"Received"</dt><dd>{compact_date(&position.received_at)}</dd></div>
                <div><dt>"Storage age"</dt><dd>{format!("{} days",position.age_days)}</dd></div>
                <div><dt>"Expiration"</dt><dd>{expiration}</dd></div>
            </dl>
            <div class="aging-quantity-grid">
                <div><span>"On hand"</span><strong>{format_quantity(position.on_hand_quantity)}</strong></div>
                <div><span>"Reserved"</span><strong>{format_quantity(position.reserved_quantity)}</strong></div>
                <div><span>"Held"</span><strong>{format_quantity(position.held_quantity)}</strong></div>
                <div><span>"Available"</span><strong>{format_quantity(position.available_quantity)}</strong></div>
            </div>
            <p class="detail-note">{format!("Balance #{} · Batch #{} · {}",position.inventory_balance_id,position.item_batch_id,status_label(position.status))}</p>
        </div>
    }
}

fn select_sort(signals: AgingSignals, sort: InventoryAgingSort, on_unauthorized: Callback<()>) {
    if signals.sort.get_untracked() == sort {
        signals.direction.update(|direction| {
            *direction = match *direction {
                InventorySortDirection::Ascending => InventorySortDirection::Descending,
                InventorySortDirection::Descending => InventorySortDirection::Ascending,
            }
        });
    } else {
        signals.sort.set(sort);
        signals.direction.set(InventorySortDirection::Ascending);
    }
    reset_page(signals);
    request_aging(signals, on_unauthorized);
}

fn reset_page(signals: AgingSignals) {
    signals.cursor.set(None);
    signals.history.set(Vec::new());
    signals.selected.set(None);
}

#[cfg(target_arch = "wasm32")]
fn request_aging(signals: AgingSignals, on_unauthorized: Callback<()>) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    signals.error.set(None);
    let filters = signals.filters();
    let sort = signals.sort.get_untracked();
    let direction = signals.direction.get_untracked();
    let cursor = signals.cursor.get_untracked();
    leptos::task::spawn_local(async move {
        let result = api::inventory_aging(filters, sort, direction, cursor.as_ref()).await;
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
fn request_aging(_signals: AgingSignals, _on_unauthorized: Callback<()>) {}

#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "hydration validates the item filter")
)]
fn positive_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|id| *id > 0)
}

fn sort_direction(value: InventorySortDirection) -> SortDirection {
    match value {
        InventorySortDirection::Ascending => SortDirection::Ascending,
        InventorySortDirection::Descending => SortDirection::Descending,
    }
}

fn parse_bucket(value: &str) -> Option<InventoryAgingBucket> {
    match value {
        "expired" => Some(InventoryAgingBucket::Expired),
        "due_within_7_days" => Some(InventoryAgingBucket::DueWithin7Days),
        "due_within_30_days" => Some(InventoryAgingBucket::DueWithin30Days),
        "due_within_90_days" => Some(InventoryAgingBucket::DueWithin90Days),
        "beyond_90_days" => Some(InventoryAgingBucket::Beyond90Days),
        "no_expiration" => Some(InventoryAgingBucket::NoExpiration),
        _ => None,
    }
}

fn bucket_value(value: Option<InventoryAgingBucket>) -> &'static str {
    value.map_or("all", |bucket| match bucket {
        InventoryAgingBucket::Expired => "expired",
        InventoryAgingBucket::DueWithin7Days => "due_within_7_days",
        InventoryAgingBucket::DueWithin30Days => "due_within_30_days",
        InventoryAgingBucket::DueWithin90Days => "due_within_90_days",
        InventoryAgingBucket::Beyond90Days => "beyond_90_days",
        InventoryAgingBucket::NoExpiration => "no_expiration",
    })
}

fn bucket_label(value: InventoryAgingBucket) -> &'static str {
    match value {
        InventoryAgingBucket::Expired => "Expired",
        InventoryAgingBucket::DueWithin7Days => "Due ≤7d",
        InventoryAgingBucket::DueWithin30Days => "Due ≤30d",
        InventoryAgingBucket::DueWithin90Days => "Due ≤90d",
        InventoryAgingBucket::Beyond90Days => "Beyond 90d",
        InventoryAgingBucket::NoExpiration => "No expiration",
    }
}

fn bucket_class(value: InventoryAgingBucket) -> &'static str {
    match value {
        InventoryAgingBucket::Expired => "status held",
        InventoryAgingBucket::DueWithin7Days => "status processing",
        InventoryAgingBucket::DueWithin30Days => "status processing",
        InventoryAgingBucket::DueWithin90Days => "status open",
        InventoryAgingBucket::Beyond90Days => "status shipped",
        InventoryAgingBucket::NoExpiration => "status muted",
    }
}

fn item_label(value: &InventoryAgingResponse) -> String {
    value
        .primary_sku
        .clone()
        .or_else(|| value.item_description.clone())
        .unwrap_or_else(|| format!("Item #{}", value.item_id))
}

fn trace_label(value: &InventoryAgingResponse) -> String {
    let mut values = Vec::new();
    if let Some(lot) = value.lot.as_deref() {
        values.push(format!("Lot {lot}"));
    }
    if let Some(serial) = value.serial.as_deref() {
        values.push(format!("Serial {serial}"));
    }
    if values.is_empty() {
        "Untracked".into()
    } else {
        values.join(" / ")
    }
}

fn expiration_label(value: &InventoryAgingResponse) -> String {
    match (value.expiration.as_deref(), value.days_to_expiration) {
        (Some(expiration), Some(days)) if days < 0 => {
            format!(
                "{} · {}d overdue",
                compact_date(expiration),
                days.unsigned_abs()
            )
        }
        (Some(expiration), Some(days)) => format!("{} · {days}d", compact_date(expiration)),
        _ => "No expiration".into(),
    }
}

fn compact_date(value: &str) -> String {
    value.get(..10).unwrap_or(value).to_owned()
}

fn status_label(value: wareboxes_api_contract::v1::InventoryBalanceStatus) -> &'static str {
    match value {
        wareboxes_api_contract::v1::InventoryBalanceStatus::Available => "Available",
        wareboxes_api_contract::v1::InventoryBalanceStatus::Hold => "Hold",
        wareboxes_api_contract::v1::InventoryBalanceStatus::Damaged => "Damaged",
        wareboxes_api_contract::v1::InventoryBalanceStatus::Quarantine => "Quarantine",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aging_bucket_values_round_trip() {
        for bucket in [
            InventoryAgingBucket::Expired,
            InventoryAgingBucket::DueWithin7Days,
            InventoryAgingBucket::DueWithin30Days,
            InventoryAgingBucket::DueWithin90Days,
            InventoryAgingBucket::Beyond90Days,
            InventoryAgingBucket::NoExpiration,
        ] {
            assert_eq!(parse_bucket(bucket_value(Some(bucket))), Some(bucket));
        }
    }
}
