use leptos::prelude::*;
use lucide_leptos::Eye;
use wareboxes_api_contract::v1::{
    InventoryIntegrityIssueKind, InventoryIntegrityIssueResponse, InventoryIntegrityPage,
    InventoryIntegritySort, InventoryJournalPage, InventoryJournalSort,
    InventoryJournalTransactionResponse, InventorySortDirection, OpaqueCursor,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api::{self, IntegrityFilters, JournalFilters};
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortableHeader};
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, Copy)]
struct JournalSignals {
    page: RwSignal<InventoryJournalPage>,
    selected: RwSignal<Option<InventoryJournalTransactionResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    search: RwSignal<String>,
    applied_search: RwSignal<String>,
    facility_id: RwSignal<String>,
    owner_id: RwSignal<String>,
    transaction_id: RwSignal<String>,
    sort: RwSignal<InventoryJournalSort>,
    direction: RwSignal<InventorySortDirection>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
}

impl JournalSignals {
    fn new() -> Self {
        Self {
            page: RwSignal::new(InventoryJournalPage::new(Vec::new(), None)),
            selected: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            generation: RwSignal::new(0),
            search: RwSignal::new(String::new()),
            applied_search: RwSignal::new(String::new()),
            facility_id: RwSignal::new(String::new()),
            owner_id: RwSignal::new(String::new()),
            transaction_id: RwSignal::new(String::new()),
            sort: RwSignal::new(InventoryJournalSort::OccurredAt),
            direction: RwSignal::new(InventorySortDirection::Descending),
            cursor: RwSignal::new(None),
            history: RwSignal::new(Vec::new()),
        }
    }

    fn filters(self) -> JournalFilters {
        let search = self.applied_search.get_untracked();
        JournalFilters {
            query: (!search.is_empty()).then_some(search),
            facility_id: positive_id(&self.facility_id.get_untracked()),
            inventory_owner_id: positive_id(&self.owner_id.get_untracked()),
            transaction_id: positive_id(&self.transaction_id.get_untracked()),
            ..Default::default()
        }
    }
}

#[component]
pub(super) fn JournalView(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let signals = JournalSignals::new();
    let layout = SplitPaneState::new("inventory-journal", 790);
    let facilities = StoredValue::new(access.facilities);
    let owners = StoredValue::new(access.inventory_owners);
    Effect::new(move || request_journal(signals, on_unauthorized));

    let apply = move |_| {
        signals
            .applied_search
            .set(signals.search.get_untracked().trim().to_owned());
        reset_journal_page(signals);
        request_journal(signals, on_unauthorized);
    };
    let next = move |_| {
        let Some(cursor) = signals.page.get_untracked().next_cursor else {
            return;
        };
        signals
            .history
            .update(|history| history.push(signals.cursor.get_untracked()));
        signals.cursor.set(Some(cursor));
        request_journal(signals, on_unauthorized);
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
        request_journal(signals, on_unauthorized);
    };

    view! {
        <section class="integrity-read-view">
            <div class="integrity-query-bar">
                <SearchField label="Search the inventory journal and trace dimensions".to_owned() placeholder="SKU, lot, serial, location, operation" value=signals.search/>
                <label><span>"Client"</span><select prop:value=move || signals.owner_id.get() on:change=move |event| signals.owner_id.set(event_target_value(&event))><option value="">"All clients"</option>{owners.get_value().into_iter().map(|owner| view! { <option value=owner.id>{owner.name}</option> }).collect_view()}</select></label>
                <label><span>"Facility"</span><select prop:value=move || signals.facility_id.get() on:change=move |event| signals.facility_id.set(event_target_value(&event))><option value="">"All facilities"</option>{facilities.get_value().into_iter().map(|facility| view! { <option value=facility.id>{facility.name}</option> }).collect_view()}</select></label>
                <label><span>"Transaction"</span><input inputmode="numeric" placeholder="Any" prop:value=move || signals.transaction_id.get() on:input=move |event| signals.transaction_id.set(event_target_value(&event))/></label>
                <button type="button" class="button primary-action" disabled=move || signals.loading.get() on:click=apply>"Apply"</button>
                <PaneControls layout master_label="journal table" detail_label="transaction detail"/>
            </div>
            <Show when=move || signals.error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p>
            </Show>
            <div class="integrity-read-split split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
                <section class="data-section split-master integrity-read-master">
                    <div class="table-toolbar compact"><div class="toolbar-summary"><strong>{move || signals.page.get().items.len()}</strong><span>"transactions on page"</span></div><span>{move || if signals.loading.get() { "Loading journal" } else { "Server-sorted journal" }}</span></div>
                    <div class="table-scroll"><table class="data-table journal-table"><thead><tr>
                        <SortableHeader label="Transaction" active=move || signals.sort.get()==InventoryJournalSort::Transaction direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_journal_sort(signals,InventoryJournalSort::Transaction,on_unauthorized))/>
                        <SortableHeader label="Occurred" active=move || signals.sort.get()==InventoryJournalSort::OccurredAt direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_journal_sort(signals,InventoryJournalSort::OccurredAt,on_unauthorized))/>
                        <SortableHeader label="Type" active=move || signals.sort.get()==InventoryJournalSort::Type direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_journal_sort(signals,InventoryJournalSort::Type,on_unauthorized))/>
                        <SortableHeader label="Client" active=move || signals.sort.get()==InventoryJournalSort::Client direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_journal_sort(signals,InventoryJournalSort::Client,on_unauthorized))/>
                        <th class="numeric">"Entries"</th>
                        <SortableHeader label="Net" active=move || signals.sort.get()==InventoryJournalSort::NetQuantity direction=move || sort_direction(signals.direction.get()) on_sort=Callback::new(move |_| select_journal_sort(signals,InventoryJournalSort::NetQuantity,on_unauthorized)) numeric=true/>
                        <th class="icon-column"><span class="sr-only">"Detail"</span></th>
                    </tr></thead><tbody>{move || journal_rows(signals,layout)}</tbody></table></div>
                    <div class="table-footer"><span>{move || if signals.page.get().next_cursor.is_some() { "More transactions available" } else { "End of journal results" }}</span><div><button type="button" class="button secondary-action" disabled=move || signals.history.get().is_empty()||signals.loading.get() on:click=previous>"Previous"</button><button type="button" class="button secondary-action" disabled=move || signals.page.get().next_cursor.is_none()||signals.loading.get() on:click=next>"Next"</button></div></div>
                </section>
                <SplitPaneHandle layout/>
                <aside class="data-section split-detail journal-detail">{move || signals.selected.get().map_or_else(|| view!{<div class="journal-detail-empty"><h2>"Transaction detail"</h2><p>"Select a journal row to inspect signed trace evidence."</p></div>}.into_any(),|transaction| view!{<JournalDetail transaction/>}.into_any())}</aside>
            </div>
        </section>
    }
}

fn journal_rows(signals: JournalSignals, layout: SplitPaneState) -> AnyView {
    let rows = signals.page.get().items;
    if rows.is_empty() && !signals.loading.get() {
        return view! { <tr><td colspan="7" class="table-empty-row">"No transactions match the current trace filters."</td></tr> }.into_any();
    }
    rows.into_iter().map(|transaction| {
        let row=transaction.clone(); let action=transaction.clone(); let id=transaction.id;
        let selected=signals.selected.get().is_some_and(|value|value.id==id);
        view!{<tr class:selected=selected on:click=move |_|{signals.selected.set(Some(row.clone()));layout.show_detail()}><td><strong>{format!("#{id}")}</strong></td><td>{compact_time(&transaction.occurred_at)}</td><td><strong>{transaction.transaction_type}</strong><small class="cell-detail">{transaction.operation}</small></td><td>{transaction.inventory_owner_name}</td><td class="numeric">{transaction.entry_count}</td><td class="numeric strong">{signed_quantity(transaction.net_quantity)}</td><td class="icon-column"><button type="button" class="icon-button compact" title="View transaction" aria-label=format!("View transaction {id}") aria-pressed=selected on:click=move |event|{event.stop_propagation();signals.selected.set(Some(action.clone()));layout.show_detail()}><Eye size=13/></button></td></tr>}
    }).collect_view().into_any()
}

#[component]
fn JournalDetail(transaction: InventoryJournalTransactionResponse) -> impl IntoView {
    let reference = transaction
        .reference_type
        .as_deref()
        .zip(transaction.reference_id)
        .map_or_else(|| "-".into(), |(kind, id)| format!("{kind} #{id}"));
    view! {<div class="inventory-trace-detail"><header><div><p class="eyebrow">"Immutable journal evidence"</p><h2>{format!("Transaction #{}",transaction.id)}</h2></div><span class="status shipped">{transaction.transaction_type}</span></header><dl class="journal-facts"><div><dt>"Occurred"</dt><dd>{compact_time(&transaction.occurred_at)}</dd></div><div><dt>"Client"</dt><dd>{transaction.inventory_owner_name}</dd></div><div><dt>"Actor"</dt><dd>{transaction.actor_user_id.map_or_else(||"System".into(),|id|format!("User #{id}"))}</dd></div><div><dt>"Reference"</dt><dd>{reference}</dd></div><div><dt>"Operation"</dt><dd>{transaction.operation}</dd></div><div><dt>"Reason"</dt><dd>{transaction.reason.unwrap_or_else(||"-".into())}</dd></div></dl><div class="journal-entry-scroll"><table class="data-table journal-entry-table"><thead><tr><th>"Facility / location"</th><th>"Item / batch"</th><th>"Trace"</th><th>"Status"</th><th class="numeric">"Delta"</th></tr></thead><tbody>{transaction.entries.into_iter().map(|entry|{let location=entry.location_barcode.or(entry.location_name).unwrap_or_else(||format!("#{}",entry.location_id));let item=entry.primary_sku.or(entry.item_description).unwrap_or_else(||format!("#{}",entry.item_id));let trace=trace_label(entry.lot.as_deref(),entry.serial.as_deref(),entry.license_plate_barcode.as_deref());view!{<tr><td><strong>{entry.facility_name}</strong><small class="cell-detail">{location}</small></td><td><strong>{item}</strong><small class="cell-detail">{format!("Batch #{}",entry.item_batch_id)}</small></td><td>{trace}</td><td>{status_label(entry.status)}<small class="cell-detail">{entry.uom}</small></td><td class="numeric strong">{signed_quantity(entry.quantity_delta)}</td></tr>}}).collect_view()}</tbody></table></div></div>}
}

#[derive(Clone, Copy)]
struct IssueSignals {
    page: RwSignal<InventoryIntegrityPage>,
    selected: RwSignal<Option<InventoryIntegrityIssueResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    kind: RwSignal<Option<InventoryIntegrityIssueKind>>,
    sort: RwSignal<InventoryIntegritySort>,
    direction: RwSignal<InventorySortDirection>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
}

impl IssueSignals {
    fn new() -> Self {
        Self {
            page: RwSignal::new(InventoryIntegrityPage::new(Vec::new(), None)),
            selected: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            generation: RwSignal::new(0),
            kind: RwSignal::new(None),
            sort: RwSignal::new(InventoryIntegritySort::Severity),
            direction: RwSignal::new(InventorySortDirection::Descending),
            cursor: RwSignal::new(None),
            history: RwSignal::new(Vec::new()),
        }
    }
}

#[component]
pub(super) fn ReconciliationView(on_unauthorized: Callback<()>) -> impl IntoView {
    let signals = IssueSignals::new();
    let layout = SplitPaneState::new("inventory-reconciliation", 820);
    Effect::new(move || request_issues(signals, on_unauthorized));
    let refresh = move |_| {
        reset_issue_page(signals);
        request_issues(signals, on_unauthorized)
    };
    let next = move |_| {
        let Some(cursor) = signals.page.get_untracked().next_cursor else {
            return;
        };
        signals
            .history
            .update(|values| values.push(signals.cursor.get_untracked()));
        signals.cursor.set(Some(cursor));
        request_issues(signals, on_unauthorized)
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
        request_issues(signals, on_unauthorized)
    };
    view! {<section class="integrity-read-view"><div class="integrity-query-bar reconciliation"><label><span>"Issue type"</span><select prop:value=move||issue_kind_value(signals.kind.get()) on:change=move|event|{signals.kind.set(parse_issue_kind(&event_target_value(&event)));reset_issue_page(signals);request_issues(signals,on_unauthorized)}><option value="all">"All issues"</option><option value="journal_projection">"Journal projection"</option><option value="commitments">"Commitments"</option></select></label><button type="button" class="button secondary-action" disabled=move||signals.loading.get() on:click=refresh>"Refresh"</button><div class="integrity-health" class:attention=move||!signals.page.get().items.is_empty()><strong>{move||signals.page.get().items.len()}</strong><span>"issues on page"</span></div><PaneControls layout master_label="issue table" detail_label="issue detail"/></div><Show when=move||signals.error.get().is_some()><p class="inline-command-error" role="alert">{move||signals.error.get().unwrap_or_default()}</p></Show><div class="integrity-read-split split-workspace" style=move||layout.style() data-pane-mode=move||layout.mode_attribute()><section class="data-section split-master integrity-read-master"><div class="table-scroll"><table class="data-table reconciliation-table"><thead><tr><th>"Issue"</th><SortableHeader label="Client" active=move||signals.sort.get()==InventoryIntegritySort::Client direction=move||sort_direction(signals.direction.get()) on_sort=Callback::new(move |_|select_issue_sort(signals,InventoryIntegritySort::Client,on_unauthorized))/><SortableHeader label="Facility" active=move||signals.sort.get()==InventoryIntegritySort::Facility direction=move||sort_direction(signals.direction.get()) on_sort=Callback::new(move |_|select_issue_sort(signals,InventoryIntegritySort::Facility,on_unauthorized))/><SortableHeader label="Item" active=move||signals.sort.get()==InventoryIntegritySort::Item direction=move||sort_direction(signals.direction.get()) on_sort=Callback::new(move |_|select_issue_sort(signals,InventoryIntegritySort::Item,on_unauthorized))/><th>"Position"</th><SortableHeader label="Severity" active=move||signals.sort.get()==InventoryIntegritySort::Severity direction=move||sort_direction(signals.direction.get()) on_sort=Callback::new(move |_|select_issue_sort(signals,InventoryIntegritySort::Severity,on_unauthorized)) numeric=true/><th class="icon-column"><span class="sr-only">"Detail"</span></th></tr></thead><tbody>{move||issue_rows(signals,layout)}</tbody></table></div><div class="table-footer"><span>{move||if signals.page.get().items.is_empty()&&!signals.loading.get(){"Journal, balances, allocations, and holds agree."}else if signals.page.get().next_cursor.is_some(){"More issues available"}else{"End of reconciliation results"}}</span><div><button type="button" class="button secondary-action" disabled=move||signals.history.get().is_empty()||signals.loading.get() on:click=previous>"Previous"</button><button type="button" class="button secondary-action" disabled=move||signals.page.get().next_cursor.is_none()||signals.loading.get() on:click=next>"Next"</button></div></div></section><SplitPaneHandle layout/><aside class="data-section split-detail journal-detail">{move||signals.selected.get().map_or_else(||view!{<div class="journal-detail-empty"><h2>"Reconciliation evidence"</h2><p>"Select an issue to compare the immutable journal with operational projections."</p></div>}.into_any(),|issue|view!{<IssueDetail issue/>}.into_any())}</aside></div></section>}
}

fn issue_rows(signals: IssueSignals, layout: SplitPaneState) -> AnyView {
    let rows = signals.page.get().items;
    if rows.is_empty() && !signals.loading.get() {
        return view!{<tr><td colspan="7" class="table-empty-row">"No reconciliation issues in the current scope."</td></tr>}.into_any();
    }
    rows
        .into_iter()
        .map(|issue| {
            let row = issue.clone();
            let action = issue.clone();
            let key = issue.issue_key.clone();
            let selected = signals
                .selected
                .get()
                .as_ref()
                .is_some_and(|value| value.issue_key == key);
            let kind = issue.kind;
            let issue_codes = issue.issue_codes.join(", ");
            let owner = issue.inventory_owner_name.clone();
            let facility = issue.facility_name.clone();
            let item = item_label(&issue);
            let trace = trace_label(
                issue.lot.as_deref(),
                issue.serial.as_deref(),
                issue.license_plate_barcode.as_deref(),
            );
            let location = issue
                .location_barcode
                .clone()
                .or_else(|| issue.location_name.clone())
                .unwrap_or_else(|| format!("#{}", issue.location_id));
            let severity = format_quantity(issue.severity_quantity);
            view! {
                <tr class:selected=selected on:click=move |_| { signals.selected.set(Some(row.clone())); layout.show_detail() }>
                    <td><span class=issue_kind_class(kind)>{issue_kind_label(kind)}</span><small class="cell-detail">{issue_codes}</small></td>
                    <td>{owner}</td><td>{facility}</td>
                    <td><strong>{item}</strong><small class="cell-detail">{trace}</small></td>
                    <td>{location}</td><td class="numeric variance-nonzero">{severity}</td>
                    <td class="icon-column"><button type="button" class="icon-button compact" title="View reconciliation evidence" aria-label="View reconciliation evidence" aria-pressed=selected on:click=move |event| { event.stop_propagation(); signals.selected.set(Some(action.clone())); layout.show_detail() }><Eye size=13/></button></td>
                </tr>
            }
        })
        .collect_view()
        .into_any()
}

#[component]
fn IssueDetail(issue: InventoryIntegrityIssueResponse) -> impl IntoView {
    let title = issue_kind_label(issue.kind);
    let location = issue
        .location_barcode
        .clone()
        .or_else(|| issue.location_name.clone())
        .unwrap_or_else(|| format!("#{}", issue.location_id));
    let item = item_label(&issue);
    let trace = trace_label(
        issue.lot.as_deref(),
        issue.serial.as_deref(),
        issue.license_plate_barcode.as_deref(),
    );
    view! {<div class="inventory-trace-detail"><header><div><p class="eyebrow">"Reconciliation evidence"</p><h2>{title}</h2></div><span class=issue_kind_class(issue.kind)>{format!("Severity {}",issue.severity_quantity)}</span></header><dl class="journal-facts"><div><dt>"Client / facility"</dt><dd>{format!("{} / {}",issue.inventory_owner_name,issue.facility_name)}</dd></div><div><dt>"Location"</dt><dd>{location}</dd></div><div><dt>"Item"</dt><dd>{item}</dd></div><div><dt>"Trace"</dt><dd>{trace}</dd></div><div><dt>"Journal / projected"</dt><dd>{quantity_pair(issue.journal_quantity,issue.projected_quantity)}</dd></div><div><dt>"Variance"</dt><dd class="variance-nonzero">{optional_signed(issue.variance_quantity)}</dd></div><div><dt>"Reserved / allocated"</dt><dd>{quantity_pair(issue.reserved_quantity,issue.allocated_quantity)}</dd></div><div><dt>"Held / hold ledger"</dt><dd>{quantity_pair(issue.held_quantity,issue.hold_ledger_quantity)}</dd></div><div><dt>"On hand"</dt><dd>{optional_quantity(issue.on_hand_quantity)}</dd></div><div><dt>"Overcommitted"</dt><dd>{optional_quantity(issue.overcommitted_quantity)}</dd></div></dl><section class="integrity-evidence-codes"><span>"Detected conditions"</span><strong>{issue.issue_codes.join(", ")}</strong></section></div>}
}

fn request_journal(signals: JournalSignals, on_unauthorized: Callback<()>) {
    let generation = signals.generation.get_untracked() + 1;
    signals.generation.set(generation);
    signals.loading.set(true);
    signals.error.set(None);
    let filters = signals.filters();
    let cursor = signals.cursor.get_untracked();
    leptos::task::spawn_local(async move {
        match api::inventory_journal(
            filters,
            signals.sort.get_untracked(),
            signals.direction.get_untracked(),
            cursor.as_ref(),
        )
        .await
        {
            Ok(page) if signals.generation.get_untracked() == generation => {
                let selected = signals
                    .selected
                    .get_untracked()
                    .and_then(|selected| {
                        page.items.iter().find(|row| row.id == selected.id).cloned()
                    })
                    .or_else(|| page.items.first().cloned());
                signals.page.set(page);
                signals.selected.set(selected);
                signals.loading.set(false)
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) if signals.generation.get_untracked() == generation => {
                signals.error.set(Some(error.message));
                signals.loading.set(false)
            }
            _ => {}
        }
    })
}
fn request_issues(signals: IssueSignals, on_unauthorized: Callback<()>) {
    let generation = signals.generation.get_untracked() + 1;
    signals.generation.set(generation);
    signals.loading.set(true);
    signals.error.set(None);
    let cursor = signals.cursor.get_untracked();
    let filters = IntegrityFilters {
        kind: signals.kind.get_untracked(),
        ..Default::default()
    };
    leptos::task::spawn_local(async move {
        match api::inventory_integrity_issues(
            filters,
            signals.sort.get_untracked(),
            signals.direction.get_untracked(),
            cursor.as_ref(),
        )
        .await
        {
            Ok(page) if signals.generation.get_untracked() == generation => {
                let selected = signals
                    .selected
                    .get_untracked()
                    .and_then(|selected| {
                        page.items
                            .iter()
                            .find(|row| row.issue_key == selected.issue_key)
                            .cloned()
                    })
                    .or_else(|| page.items.first().cloned());
                signals.page.set(page);
                signals.selected.set(selected);
                signals.loading.set(false)
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) if signals.generation.get_untracked() == generation => {
                signals.error.set(Some(error.message));
                signals.loading.set(false)
            }
            _ => {}
        }
    })
}
fn reset_journal_page(signals: JournalSignals) {
    signals.cursor.set(None);
    signals.history.set(Vec::new())
}
fn reset_issue_page(signals: IssueSignals) {
    signals.cursor.set(None);
    signals.history.set(Vec::new())
}
fn select_journal_sort(
    signals: JournalSignals,
    sort: InventoryJournalSort,
    on_unauthorized: Callback<()>,
) {
    if signals.sort.get_untracked() == sort {
        signals.direction.update(toggle_direction)
    } else {
        signals.sort.set(sort);
        signals.direction.set(InventorySortDirection::Ascending)
    }
    reset_journal_page(signals);
    request_journal(signals, on_unauthorized)
}
fn select_issue_sort(
    signals: IssueSignals,
    sort: InventoryIntegritySort,
    on_unauthorized: Callback<()>,
) {
    if signals.sort.get_untracked() == sort {
        signals.direction.update(toggle_direction)
    } else {
        signals.sort.set(sort);
        signals.direction.set(InventorySortDirection::Ascending)
    }
    reset_issue_page(signals);
    request_issues(signals, on_unauthorized)
}
fn toggle_direction(value: &mut InventorySortDirection) {
    *value = match *value {
        InventorySortDirection::Ascending => InventorySortDirection::Descending,
        InventorySortDirection::Descending => InventorySortDirection::Ascending,
    }
}
fn sort_direction(value: InventorySortDirection) -> SortDirection {
    match value {
        InventorySortDirection::Ascending => SortDirection::Ascending,
        InventorySortDirection::Descending => SortDirection::Descending,
    }
}
fn positive_id(value: &str) -> Option<i64> {
    value.trim().parse().ok().filter(|value| *value > 0)
}
fn compact_time(value: &str) -> String {
    value
        .replace('T', " ")
        .split(['+', 'Z'])
        .next()
        .unwrap_or(value)
        .chars()
        .take(19)
        .collect()
}
fn signed_quantity(value: i64) -> String {
    format!("{value:+}")
}
fn optional_signed(value: Option<i64>) -> String {
    value.map_or_else(|| "-".into(), signed_quantity)
}
fn optional_quantity(value: Option<i64>) -> String {
    value.map_or_else(|| "-".into(), format_quantity)
}
fn quantity_pair(left: Option<i64>, right: Option<i64>) -> String {
    format!("{} / {}", optional_quantity(left), optional_quantity(right))
}
fn trace_label(lot: Option<&str>, serial: Option<&str>, plate: Option<&str>) -> String {
    let values = [
        lot.map(|v| format!("Lot {v}")),
        serial.map(|v| format!("Serial {v}")),
        plate.map(|v| format!("LP {v}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if values.is_empty() {
        "Untracked".into()
    } else {
        values.join(" / ")
    }
}
fn item_label(issue: &InventoryIntegrityIssueResponse) -> String {
    issue
        .primary_sku
        .clone()
        .or_else(|| issue.item_description.clone())
        .unwrap_or_else(|| format!("Item #{}", issue.item_id))
}
fn issue_kind_label(kind: InventoryIntegrityIssueKind) -> &'static str {
    match kind {
        InventoryIntegrityIssueKind::JournalProjection => "Journal projection",
        InventoryIntegrityIssueKind::Commitments => "Commitments",
    }
}
fn issue_kind_class(kind: InventoryIntegrityIssueKind) -> &'static str {
    match kind {
        InventoryIntegrityIssueKind::JournalProjection => "status held",
        InventoryIntegrityIssueKind::Commitments => "status processing",
    }
}
fn issue_kind_value(kind: Option<InventoryIntegrityIssueKind>) -> &'static str {
    match kind {
        None => "all",
        Some(InventoryIntegrityIssueKind::JournalProjection) => "journal_projection",
        Some(InventoryIntegrityIssueKind::Commitments) => "commitments",
    }
}
fn parse_issue_kind(value: &str) -> Option<InventoryIntegrityIssueKind> {
    match value {
        "journal_projection" => Some(InventoryIntegrityIssueKind::JournalProjection),
        "commitments" => Some(InventoryIntegrityIssueKind::Commitments),
        _ => None,
    }
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
    fn trace_labels_preserve_lot_serial_and_plate_identity() {
        assert_eq!(
            trace_label(Some("L1"), Some("S1"), Some("LP1")),
            "Lot L1 / Serial S1 / LP LP1"
        );
        assert_eq!(trace_label(None, None, None), "Untracked")
    }
    #[test]
    fn sort_selection_toggles_on_repeat_and_resets_on_new_field() {
        let signals = JournalSignals::new();
        signals.sort.set(InventoryJournalSort::OccurredAt);
        signals.direction.set(InventorySortDirection::Descending);
        toggle_direction(&mut signals.direction.write());
        assert_eq!(
            signals.direction.get_untracked(),
            InventorySortDirection::Ascending
        )
    }
}
