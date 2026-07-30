use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CreateInventoryRelocationTaskRequest, InventoryBalanceResponse, InventoryRelocationWorkRequest,
    OpaqueCursor,
};
use wareboxes_core::models::{
    InventoryHoldReconciliationIssue, InventoryReconciliationIssue, InventoryTransaction, Location,
};

use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

#[derive(Clone)]
struct IntegrityData {
    transactions: Vec<InventoryTransaction>,
    inventory_issues: Vec<InventoryReconciliationIssue>,
    hold_issues: Vec<InventoryHoldReconciliationIssue>,
    balances: Vec<InventoryBalanceResponse>,
    balance_next_cursor: Option<OpaqueCursor>,
    locations: Vec<Location>,
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "hydration constructs the terminal inventory-control states"
    )
)]
enum IntegrityState {
    Loading,
    Ready(Box<IntegrityData>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IntegrityTab {
    Journal,
    Reconciliation,
    MovePlanning,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JournalSort {
    Transaction,
    Created,
    Type,
    Client,
    Entries,
    Net,
}

#[component]
pub fn InventoryIntegrityWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let state = RwSignal::new(IntegrityState::Loading);
    let tab = RwSignal::new(IntegrityTab::Journal);
    let journal_filter = RwSignal::new(String::new());
    let sort = RwSignal::new(SortSpec {
        key: JournalSort::Created,
        direction: SortDirection::Descending,
    });
    let selected_balance_id = RwSignal::new(String::new());
    let selected_balance = RwSignal::new(None::<InventoryBalanceResponse>);
    let destination_location_id = RwSignal::new(String::new());
    let quantity = RwSignal::new("1".to_owned());
    let instructions = RwSignal::new(String::new());
    let task_pending = RwSignal::new(false);
    let task_error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();

    #[cfg(target_arch = "wasm32")]
    request_integrity(state, on_unauthorized);

    let retry = move |_| request_integrity(state, on_unauthorized);
    let create_task = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if task_pending.get_untracked() {
            return;
        }
        let IntegrityState::Ready(data) = state.get_untracked() else {
            return;
        };
        let Some(source) = selected_balance.get_untracked() else {
            task_error.set(Some("Select a source position.".to_owned()));
            return;
        };
        let Some(destination_id) = positive_id(&destination_location_id.get_untracked()) else {
            task_error.set(Some("Select a destination location.".to_owned()));
            return;
        };
        let Some(destination) = data
            .locations
            .iter()
            .find(|location| location.id == destination_id)
        else {
            task_error.set(Some(
                "The selected destination location is no longer loaded.".to_owned(),
            ));
            return;
        };
        if source.facility_id != destination.facility_id {
            task_error.set(Some(
                "Source and destination must belong to the same facility.".to_owned(),
            ));
            return;
        }
        if source.location_id == destination.id {
            task_error.set(Some(
                "Destination must differ from the source location.".to_owned(),
            ));
            return;
        }
        let movable = movable_quantity(&source);
        let Ok(quantity_value) = quantity.get_untracked().trim().parse::<i64>() else {
            task_error.set(Some("Enter a whole-number quantity.".to_owned()));
            return;
        };
        if source.license_plate_id.is_none() && (quantity_value <= 0 || quantity_value > movable) {
            task_error.set(Some(format!(
                "Quantity must be between 1 and {}.",
                format_quantity(movable)
            )));
            return;
        }
        let instructions_value = optional_text(&instructions.get_untracked());
        let work = source.license_plate_id.map_or_else(
            || InventoryRelocationWorkRequest::LooseBalance {
                source_inventory_balance_id: source.id,
                quantity: quantity_value,
            },
            |license_plate_id| InventoryRelocationWorkRequest::LicensePlate { license_plate_id },
        );
        let key = api::new_idempotency_key();
        task_pending.set(true);
        task_error.set(None);
        leptos::task::spawn_local(async move {
            let result = api::create_inventory_relocation_task(
                &CreateInventoryRelocationTaskRequest {
                    work,
                    destination_location_id: destination_id,
                    priority: Some(50),
                    assigned_user_id: None,
                    scheduled_for: None,
                    due_at: None,
                    instructions: instructions_value,
                },
                &key,
            )
            .await
            .map(|response| response.task_id);

            match result {
                Ok(task_id) => {
                    toasts.success(format!("Move task #{task_id} is ready for RF execution."));
                    selected_balance_id.set(String::new());
                    selected_balance.set(None);
                    destination_location_id.set(String::new());
                    quantity.set("1".to_owned());
                    instructions.set(String::new());
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    task_error.set(Some(error.message));
                }
            }
            task_pending.set(false);
        });
    };

    view! {
        <section class="integrity-workbench">
            {move || match state.get() {
                IntegrityState::Loading => {
                    view! {
                        <div class="data-section integrity-state" aria-live="polite">
                            <span class="loading-line" aria-hidden="true"></span>
                            <strong>"Loading inventory controls"</strong>
                        </div>
                    }
                        .into_any()
                }
                IntegrityState::Failed(message) => {
                    view! {
                        <div class="data-section integrity-state" role="alert">
                            <strong>"Inventory controls are unavailable"</strong>
                            <span>{message}</span>
                            <button class="button secondary-action" type="button" on:click=retry>
                                "Retry"
                            </button>
                        </div>
                    }
                        .into_any()
                }
                IntegrityState::Ready(data) => {
                    let issue_count = data.inventory_issues.len() + data.hold_issues.len();
                    let entry_count = data
                        .transactions
                        .iter()
                        .map(|transaction| transaction.entries.len())
                        .sum::<usize>();
                    let on_hand = data
                        .balances
                        .iter()
                        .map(|balance| balance.quantity.on_hand)
                        .sum::<i64>();
                    let committed = data
                        .balances
                        .iter()
                        .map(|balance| balance.quantity.reserved + balance.quantity.held)
                        .sum::<i64>();
                    view! {
                        <div class="integrity-summary" aria-label="Inventory integrity summary">
                            <div><span>"Journal transactions"</span><strong>{data.transactions.len()}</strong></div>
                            <div><span>"Journal entries"</span><strong>{entry_count}</strong></div>
                            <div><span>"On hand loaded"</span><strong>{format_quantity(on_hand)}</strong></div>
                            <div><span>"Committed loaded"</span><strong>{format_quantity(committed)}</strong></div>
                            <div class:attention=(issue_count > 0)>
                                <span>"Reconciliation issues"</span><strong>{issue_count}</strong>
                            </div>
                        </div>

                        <div class="integrity-tabs" role="tablist" aria-label="Inventory controls">
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || (tab.get() == IntegrityTab::Journal).to_string()
                                class:active=move || tab.get() == IntegrityTab::Journal
                                on:click=move |_| tab.set(IntegrityTab::Journal)
                            >
                                "Journal"
                            </button>
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || {
                                    (tab.get() == IntegrityTab::Reconciliation).to_string()
                                }
                                class:active=move || tab.get() == IntegrityTab::Reconciliation
                                on:click=move |_| tab.set(IntegrityTab::Reconciliation)
                            >
                                "Reconciliation"
                            </button>
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || {
                                    (tab.get() == IntegrityTab::MovePlanning).to_string()
                                }
                                class:active=move || tab.get() == IntegrityTab::MovePlanning
                                on:click=move |_| tab.set(IntegrityTab::MovePlanning)
                            >
                                "Move planning"
                            </button>
                        </div>

                        {move || match tab.get() {
                            IntegrityTab::Journal => {
                                view! {
                                    <JournalView
                                        transactions=data.transactions.clone()
                                        filter=journal_filter
                                        sort
                                    />
                                }
                                    .into_any()
                            }
                            IntegrityTab::Reconciliation => {
                                view! {
                                    <ReconciliationView
                                        inventory_issues=data.inventory_issues.clone()
                                        hold_issues=data.hold_issues.clone()
                                    />
                                }
                                    .into_any()
                            }
                            IntegrityTab::MovePlanning => {
                                view! {
                                    <MovePlanner
                                        balances=data.balances.clone()
                                        initial_cursor=data.balance_next_cursor.clone()
                                        locations=data.locations.clone()
                                        selected_balance_id
                                        selected_balance
                                        destination_location_id
                                        quantity
                                        instructions
                                        pending=task_pending
                                        error=task_error
                                        on_submit=Callback::new(create_task)
                                        on_unauthorized
                                    />
                                }
                                    .into_any()
                            }
                        }}
                    }
                        .into_any()
                }
            }}
        </section>
    }
}

#[cfg(target_arch = "wasm32")]
fn request_integrity(state: RwSignal<IntegrityState>, on_unauthorized: Callback<()>) {
    state.set(IntegrityState::Loading);
    leptos::task::spawn_local(async move {
        let result = async {
            let transactions = api::internal_get("/api/inventory/transactions").await?;
            let inventory_issues = api::internal_get("/api/inventory/reconciliation").await?;
            let hold_issues = api::internal_get("/api/inventory/holds/reconciliation").await?;
            let balance_page = api::balances(None).await?;
            let locations = api::internal_get("/api/locations").await?;
            Ok::<_, api::ApiError>(IntegrityData {
                transactions,
                inventory_issues,
                hold_issues,
                balances: balance_page.items,
                balance_next_cursor: balance_page.next_cursor,
                locations,
            })
        }
        .await;

        match result {
            Ok(data) => state.set(IntegrityState::Ready(Box::new(data))),
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => state.set(IntegrityState::Failed(error.message)),
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn request_integrity(_state: RwSignal<IntegrityState>, _on_unauthorized: Callback<()>) {}

#[component]
fn JournalView(
    transactions: Vec<InventoryTransaction>,
    filter: RwSignal<String>,
    sort: RwSignal<SortSpec<JournalSort>>,
) -> impl IntoView {
    let selected_id = RwSignal::new(transactions.first().map(|transaction| transaction.id));
    let table_transactions = transactions.clone();
    let detail_transactions = transactions;

    view! {
        <section class="data-section">
            <div class="table-toolbar">
                <div class="toolbar-summary">
                    <strong>{table_transactions.len()}</strong><span>"transactions loaded"</span>
                </div>
                <SearchField
                    label="Filter inventory journal".to_owned()
                    placeholder="Filter journal"
                    value=filter
                />
            </div>
            <div class="journal-layout">
                <div class="table-scroll">
                    <table class="data-table journal-table">
                        <caption class="sr-only">"Inventory transaction journal"</caption>
                        <thead>
                            <tr>
                                <SortableHeader
                                    label="Transaction"
                                    active=move || sort.get().key == JournalSort::Transaction
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, JournalSort::Transaction)
                                    })
                                />
                                <SortableHeader
                                    label="Created"
                                    active=move || sort.get().key == JournalSort::Created
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, JournalSort::Created)
                                    })
                                />
                                <SortableHeader
                                    label="Type"
                                    active=move || sort.get().key == JournalSort::Type
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, JournalSort::Type)
                                    })
                                />
                                <SortableHeader
                                    label="Client"
                                    active=move || sort.get().key == JournalSort::Client
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, JournalSort::Client)
                                    })
                                    numeric=true
                                />
                                <SortableHeader
                                    label="Entries"
                                    active=move || sort.get().key == JournalSort::Entries
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, JournalSort::Entries)
                                    })
                                    numeric=true
                                />
                                <SortableHeader
                                    label="Net"
                                    active=move || sort.get().key == JournalSort::Net
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, JournalSort::Net)
                                    })
                                    numeric=true
                                />
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let query = filter.get().trim().to_ascii_lowercase();
                                let mut rows = table_transactions
                                    .iter()
                                    .filter(|transaction| transaction_matches(transaction, &query))
                                    .cloned()
                                    .collect::<Vec<_>>();
                                sort_transactions(&mut rows, sort.get());
                                if rows.is_empty() {
                                    view! {
                                        <tr><td class="table-empty-row" colspan="6">"No transactions match this view."</td></tr>
                                    }
                                        .into_any()
                                } else {
                                    rows
                                        .into_iter()
                                        .map(|transaction| {
                                            let id = transaction.id;
                                            let net = transaction
                                                .entries
                                                .iter()
                                                .map(|entry| entry.quantity_delta)
                                                .sum::<i64>();
                                            let transaction_type =
                                                transaction.transaction_type.to_string();
                                            view! {
                                                <tr class:selected-row=move || selected_id.get() == Some(id)>
                                                    <td>
                                                        <button
                                                            class="table-link"
                                                            type="button"
                                                            on:click=move |_| selected_id.set(Some(id))
                                                        >
                                                            {format!("#{id}")}
                                                        </button>
                                                    </td>
                                                    <td>{compact_timestamp(&transaction.created.to_string())}</td>
                                                    <td>
                                                        <strong>{transaction_type}</strong>
                                                        <small class="cell-detail">{transaction.operation}</small>
                                                    </td>
                                                    <td class="numeric">{transaction.inventory_owner_id.get()}</td>
                                                    <td class="numeric">{transaction.entries.len()}</td>
                                                    <td class="numeric strong">{format_quantity(net)}</td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()
                                        .into_any()
                                }
                            }}
                        </tbody>
                    </table>
                </div>
                <aside class="journal-detail" aria-live="polite">
                    {move || {
                        selected_id
                            .get()
                            .and_then(|id| {
                                detail_transactions
                                    .iter()
                                    .find(|transaction| transaction.id == id)
                                    .cloned()
                            })
                            .map_or_else(
                                || view! {
                                    <div class="journal-detail-empty">"Select a transaction."</div>
                                }
                                .into_any(),
                                |transaction| view! {
                                    <JournalTransactionDetail transaction/>
                                }
                                .into_any(),
                            )
                    }}
                </aside>
            </div>
        </section>
    }
}

#[component]
fn JournalTransactionDetail(transaction: InventoryTransaction) -> impl IntoView {
    let reference = transaction
        .reference_type
        .as_deref()
        .zip(transaction.reference_id)
        .map_or_else(|| "-".to_owned(), |(kind, id)| format!("{kind} #{id}"));
    let reason = transaction.reason.unwrap_or_else(|| "-".to_owned());
    let actor = transaction
        .actor_user_id
        .map_or_else(|| "System".to_owned(), |id| format!("User #{id}"));
    let correlation = transaction.correlation_id.unwrap_or_else(|| "-".to_owned());
    let idempotency = transaction
        .idempotency_key
        .unwrap_or_else(|| "-".to_owned());

    view! {
        <header>
            <div>
                <p class="eyebrow">"Transaction detail"</p>
                <h2>{format!("#{}", transaction.id)}</h2>
            </div>
            <span class="status shipped">{transaction.transaction_type.to_string()}</span>
        </header>
        <dl class="journal-facts">
            <div><dt>"Created"</dt><dd>{compact_timestamp(&transaction.created.to_string())}</dd></div>
            <div><dt>"Client"</dt><dd>{transaction.inventory_owner_id.get()}</dd></div>
            <div><dt>"Actor"</dt><dd>{actor}</dd></div>
            <div><dt>"Reference"</dt><dd>{reference}</dd></div>
            <div><dt>"Operation"</dt><dd>{transaction.operation}</dd></div>
            <div><dt>"Reason"</dt><dd>{reason}</dd></div>
            <div><dt>"Correlation"</dt><dd class="mono">{correlation}</dd></div>
            <div><dt>"Idempotency"</dt><dd class="mono">{idempotency}</dd></div>
        </dl>
        <div class="journal-entry-scroll">
            <table class="data-table journal-entry-table">
                <caption class="sr-only">"Signed inventory entries"</caption>
                <thead><tr>
                    <th scope="col">"Entry"</th>
                    <th scope="col">"Item / batch"</th>
                    <th scope="col">"Facility / location"</th>
                    <th scope="col">"Tracking"</th>
                    <th scope="col">"Status / UOM"</th>
                    <th scope="col" class="numeric">"Delta"</th>
                </tr></thead>
                <tbody>
                    {transaction.entries.into_iter().map(|entry| {
                        let tracking = match (entry.lot, entry.serial) {
                            (Some(lot), Some(serial)) => format!("{lot} / {serial}"),
                            (Some(lot), None) => lot,
                            (None, Some(serial)) => serial,
                            (None, None) => "-".to_owned(),
                        };
                        let license_plate = entry
                            .license_plate_id
                            .map(|id| format!("LPN #{id}"));
                        view! {
                            <tr>
                                <td>{format!("#{}", entry.id)}</td>
                                <td>
                                    <strong>{format!("Item #{}", entry.item_id)}</strong>
                                    <small class="cell-detail">{format!("Batch #{}", entry.item_batch_id)}</small>
                                </td>
                                <td>
                                    <strong>{format!("Facility #{}", entry.facility_id)}</strong>
                                    <small class="cell-detail">{format!("Location #{}", entry.location_id)}</small>
                                </td>
                                <td>
                                    {tracking}
                                    {license_plate.map(|label| {
                                        view! { <small class="cell-detail">{label}</small> }
                                    })}
                                </td>
                                <td>
                                    <strong>{entry.status.to_string()}</strong>
                                    <small class="cell-detail">{entry.uom}</small>
                                </td>
                                <td class="numeric strong">{format_quantity(entry.quantity_delta)}</td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn ReconciliationView(
    inventory_issues: Vec<InventoryReconciliationIssue>,
    hold_issues: Vec<InventoryHoldReconciliationIssue>,
) -> impl IntoView {
    let healthy = inventory_issues.is_empty() && hold_issues.is_empty();
    view! {
        <div class="reconciliation-grid">
            {healthy.then(|| {
                view! {
                    <div class="data-section reconciliation-healthy" role="status">
                        <span class="status shipped">"Reconciled"</span>
                        <strong>"Journal, balances, commitments, and holds agree."</strong>
                    </div>
                }
            })}
            <section class="data-section">
                <div class="section-title">
                    <div><p class="eyebrow">"Journal projection"</p><h2>"Balance variances"</h2></div>
                    <strong>{inventory_issues.len()}</strong>
                </div>
                <div class="table-scroll">
                    <table class="data-table reconciliation-table">
                        <thead><tr>
                            <th scope="col">"Client / facility"</th>
                            <th scope="col">"Position"</th>
                            <th scope="col">"Status"</th>
                            <th scope="col" class="numeric">"Journal"</th>
                            <th scope="col" class="numeric">"Projected"</th>
                            <th scope="col" class="numeric">"Variance"</th>
                        </tr></thead>
                        <tbody>
                            {if inventory_issues.is_empty() {
                                view! {
                                    <tr><td class="table-empty-row" colspan="6">"No balance variances."</td></tr>
                                }
                                    .into_any()
                            } else {
                                inventory_issues
                                    .into_iter()
                                    .map(|issue| view! {
                                        <tr>
                                            <td>{format!("Client {} / Facility {}", issue.inventory_owner_id.get(), issue.facility_id)}</td>
                                            <td>{format!("Location {} / Item {}", issue.location_id, issue.item_id)}</td>
                                            <td>{issue.status.to_string()}</td>
                                            <td class="numeric">{format_quantity(issue.journal_qty)}</td>
                                            <td class="numeric">{format_quantity(issue.projected_qty)}</td>
                                            <td class="numeric strong">{format_quantity(issue.variance)}</td>
                                        </tr>
                                    })
                                    .collect_view()
                                    .into_any()
                            }}
                        </tbody>
                    </table>
                </div>
            </section>

            <section class="data-section">
                <div class="section-title">
                    <div><p class="eyebrow">"Commitments"</p><h2>"Hold and allocation variances"</h2></div>
                    <strong>{hold_issues.len()}</strong>
                </div>
                <div class="table-scroll">
                    <table class="data-table reconciliation-table">
                        <thead><tr>
                            <th scope="col">"Balance"</th>
                            <th scope="col">"Client / facility"</th>
                            <th scope="col">"Issue"</th>
                            <th scope="col" class="numeric">"On hand"</th>
                            <th scope="col" class="numeric">"Held"</th>
                            <th scope="col" class="numeric">"Overcommitted"</th>
                        </tr></thead>
                        <tbody>
                            {if hold_issues.is_empty() {
                                view! {
                                    <tr><td class="table-empty-row" colspan="6">"No commitment variances."</td></tr>
                                }
                                    .into_any()
                            } else {
                                hold_issues
                                    .into_iter()
                                    .map(|issue| view! {
                                        <tr>
                                            <td><strong>{format!("#{}", issue.inventory_balance_id)}</strong></td>
                                            <td>{format!("Client {} / Facility {}", issue.inventory_owner_id.get(), issue.facility_id)}</td>
                                            <td>{issue.issue_codes.join(", ")}</td>
                                            <td class="numeric">{format_quantity(issue.qty_on_hand)}</td>
                                            <td class="numeric">{format_quantity(issue.qty_held)}</td>
                                            <td class="numeric strong">{format_quantity(issue.overcommitted_qty)}</td>
                                        </tr>
                                    })
                                    .collect_view()
                                    .into_any()
                            }}
                        </tbody>
                    </table>
                </div>
            </section>
        </div>
    }
}

#[component]
fn MovePlanner(
    balances: Vec<InventoryBalanceResponse>,
    initial_cursor: Option<OpaqueCursor>,
    locations: Vec<Location>,
    selected_balance_id: RwSignal<String>,
    selected_balance: RwSignal<Option<InventoryBalanceResponse>>,
    destination_location_id: RwSignal<String>,
    quantity: RwSignal<String>,
    instructions: RwSignal<String>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_submit: Callback<leptos::ev::SubmitEvent>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let sources = RwSignal::new(balances);
    let next_cursor = RwSignal::new(initial_cursor);
    let source_query = RwSignal::new(String::new());
    let applied_source_query = RwSignal::new(String::new());
    let source_pending = RwSignal::new(false);
    let source_error = RwSignal::new(None::<String>);
    let selected = Memo::new(move |_| selected_balance.get());

    let search_sources = move |_| {
        if source_pending.get_untracked() {
            return;
        }
        let query = source_query.get_untracked().trim().to_owned();
        source_pending.set(true);
        source_error.set(None);
        leptos::task::spawn_local(async move {
            let result = if query.is_empty() {
                api::balances(None).await
            } else {
                api::search_balances(&query, None).await
            };
            match result {
                Ok(page) => {
                    sources.set(page.items);
                    next_cursor.set(page.next_cursor);
                    applied_source_query.set(query);
                    selected_balance_id.set(String::new());
                    selected_balance.set(None);
                    destination_location_id.set(String::new());
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => source_error.set(Some(api_error.message)),
            }
            source_pending.set(false);
        });
    };
    let load_more_sources = move |_| {
        let Some(cursor) = next_cursor.get_untracked() else {
            return;
        };
        if source_pending.get_untracked() {
            return;
        }
        let query = applied_source_query.get_untracked();
        source_pending.set(true);
        source_error.set(None);
        leptos::task::spawn_local(async move {
            let result = if query.is_empty() {
                api::balances(Some(&cursor)).await
            } else {
                api::search_balances(&query, Some(&cursor)).await
            };
            match result {
                Ok(page) => {
                    sources.update(|current| current.extend(page.items));
                    next_cursor.set(page.next_cursor);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => source_error.set(Some(api_error.message)),
            }
            source_pending.set(false);
        });
    };

    view! {
        <form class="data-section move-planner-form" on:submit=move |event| on_submit.run(event)>
            <header class="move-planner-header">
                <div>
                    <p class="eyebrow">"RF-directed execution"</p>
                    <h2>"Plan inventory move"</h2>
                </div>
                <span class="status-pill">"Confirmation requires RF scans"</span>
            </header>

            <div class="move-source-discovery">
                <label for="move-source-query">"Find source position"</label>
                <div class="move-source-search">
                    <input
                        id="move-source-query"
                        type="search"
                        maxlength="200"
                        placeholder="SKU, item, location, LPN, lot or serial"
                        prop:value=move || source_query.get()
                        on:input=move |event| source_query.set(event_target_value(&event))
                    />
                    <button
                        class="button secondary-action"
                        type="button"
                        disabled=move || source_pending.get()
                        on:click=search_sources
                    >
                        {move || if source_pending.get() { "Searching" } else { "Search" }}
                    </button>
                </div>
                {move || source_error.get().map(|message| {
                    view! { <span class="inline-error" role="alert">{message}</span> }
                })}
            </div>

            <div class="move-fields">
                <div class="move-field move-source-field">
                    <label for="move-source">"Source position"</label>
                    <select
                        id="move-source"
                        required
                        prop:value=move || selected_balance_id.get()
                        on:change=move |event| {
                            let value = event_target_value(&event);
                            let id = positive_id(&value);
                            selected_balance_id.set(value);
                            selected_balance.set(id.and_then(|id| {
                                sources
                                    .get_untracked()
                                    .into_iter()
                                    .find(|balance| balance.id == id)
                            }));
                            destination_location_id.set(String::new());
                            quantity.set("1".to_owned());
                            error.set(None);
                        }
                    >
                        <option value="">"Select a source"</option>
                        {move || {
                            sources
                                .get()
                                .into_iter()
                                .filter(|balance| movable_quantity(balance) > 0)
                                .map(|balance| {
                                    let kind = balance
                                        .license_plate_barcode
                                        .as_deref()
                                        .map_or("Loose", |_| "LPN");
                                    view! {
                                        <option value=balance.id.to_string()>
                                            {format!(
                                                "{} - {} - {} {} ({kind})",
                                                item_label(&balance),
                                                location_label(&balance),
                                                format_quantity(movable_quantity(&balance)),
                                                balance.uom
                                            )}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                    <div class="source-page-status">
                        <span>{move || format!("{} sources loaded", sources.get().len())}</span>
                        <button
                            class="table-link"
                            type="button"
                            disabled=move || next_cursor.get().is_none() || source_pending.get()
                            on:click=load_more_sources
                        >
                            {move || if next_cursor.get().is_some() { "Load more" } else { "All loaded" }}
                        </button>
                    </div>
                </div>

                <div class="move-field">
                <label for="move-destination">"Destination location"</label>
                <select
                    id="move-destination"
                    required
                    prop:value=move || destination_location_id.get()
                    on:change=move |event| {
                        destination_location_id.set(event_target_value(&event));
                        error.set(None);
                    }
                >
                    <option value="">"Select a destination"</option>
                    {move || {
                        let source = selected.get();
                        locations
                            .iter()
                            .filter(|location| {
                                location.deleted.is_none()
                                    && location.active
                                    && location
                                        .barcode
                                        .as_deref()
                                        .is_some_and(|barcode| !barcode.trim().is_empty())
                                    && source.as_ref().is_some_and(|source| {
                                        location.facility_id == source.facility_id
                                            && location.id != source.location_id
                                    })
                            })
                            .map(|location| {
                                view! {
                                    <option value=location.id.to_string()>
                                        {format!(
                                            "{} - {}",
                                            location
                                                .barcode
                                                .as_deref()
                                                .unwrap_or("Unscannable"),
                                            location.r#type
                                        )}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
                </div>

                <div class="move-field">
                <label for="move-quantity">"Quantity"</label>
                <input
                    id="move-quantity"
                    type="number"
                    min="1"
                    step="1"
                    required
                    disabled=move || {
                        selected
                            .get()
                            .is_some_and(|source| source.license_plate_id.is_some())
                    }
                    prop:value=move || quantity.get()
                    on:input=move |event| {
                        quantity.set(event_target_value(&event));
                        error.set(None);
                    }
                />
                </div>

                <div class="move-field move-instructions-field">
                <label for="move-instructions">"RF instructions"</label>
                <textarea
                    id="move-instructions"
                    maxlength="1000"
                    placeholder="Optional handling instructions"
                    prop:value=move || instructions.get()
                    on:input=move |event| instructions.set(event_target_value(&event))
                ></textarea>
                </div>
            </div>

            {move || selected.get().and_then(|source| {
                source.license_plate_barcode.map(|barcode| {
                    view! { <p class="field-note">{format!("License plate {barcode} will move as one container.")}</p> }
                })
            })}
            {move || error.get().map(|message| {
                view! { <div class="inline-command-error" role="alert">{message}</div> }
            })}
            <div class="command-actions">
                <button class="button primary-action" type="submit" disabled=move || pending.get()>
                    {move || if pending.get() { "Creating task" } else { "Create RF task" }}
                </button>
            </div>
        </form>
    }
}

fn movable_quantity(balance: &InventoryBalanceResponse) -> i64 {
    balance
        .quantity
        .on_hand
        .saturating_sub(balance.quantity.reserved)
        .saturating_sub(balance.quantity.held)
}

fn item_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .primary_sku
        .clone()
        .or_else(|| balance.item_description.clone())
        .unwrap_or_else(|| format!("Item #{}", balance.item_id))
}

fn location_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .location_barcode
        .clone()
        .or_else(|| balance.location_name.clone())
        .unwrap_or_else(|| format!("Location #{}", balance.location_id))
}

fn transaction_matches(transaction: &InventoryTransaction, query: &str) -> bool {
    query.is_empty()
        || transaction.id.to_string().contains(query)
        || transaction
            .transaction_type
            .to_string()
            .to_ascii_lowercase()
            .contains(query)
        || transaction
            .reason
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
        || transaction.operation.to_ascii_lowercase().contains(query)
        || transaction
            .reference_type
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
        || transaction.entries.iter().any(|entry| {
            [
                entry.id,
                entry.facility_id,
                entry.location_id,
                entry.item_id,
                entry.item_batch_id,
            ]
            .iter()
            .any(|value| value.to_string().contains(query))
                || [
                    entry.uom.as_str(),
                    entry.lot.as_deref().unwrap_or_default(),
                    entry.serial.as_deref().unwrap_or_default(),
                ]
                .iter()
                .any(|value| value.to_ascii_lowercase().contains(query))
        })
}

fn sort_transactions(transactions: &mut [InventoryTransaction], spec: SortSpec<JournalSort>) {
    transactions.sort_by(|left, right| {
        let left_net = left
            .entries
            .iter()
            .map(|entry| entry.quantity_delta)
            .sum::<i64>();
        let right_net = right
            .entries
            .iter()
            .map(|entry| entry.quantity_delta)
            .sum::<i64>();
        let ordering = match spec.key {
            JournalSort::Transaction => left.id.cmp(&right.id),
            JournalSort::Created => left.created.cmp(&right.created),
            JournalSort::Type => left
                .transaction_type
                .to_string()
                .cmp(&right.transaction_type.to_string()),
            JournalSort::Client => left
                .inventory_owner_id
                .get()
                .cmp(&right.inventory_owner_id.get()),
            JournalSort::Entries => left.entries.len().cmp(&right.entries.len()),
            JournalSort::Net => left_net.cmp(&right_net),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn positive_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|id| *id > 0)
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn compact_timestamp(timestamp: &str) -> String {
    timestamp.get(..16).unwrap_or(timestamp).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::{optional_text, positive_id};

    #[test]
    fn move_planner_normalizes_command_fields() {
        assert_eq!(positive_id("42"), Some(42));
        assert_eq!(positive_id("0"), None);
        assert_eq!(optional_text("  "), None);
        assert_eq!(
            optional_text("  Keep upright "),
            Some("Keep upright".into())
        );
    }
}
