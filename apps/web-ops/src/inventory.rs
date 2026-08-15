use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    InventoryBalanceResponse, InventoryBalanceSort as ApiInventoryBalanceSort,
    InventoryBalanceStatus, InventorySortDirection as ApiInventorySortDirection, OpaqueCursor,
};

use crate::api;
use crate::components::SearchField;
use crate::inventory_rollups::{InventoryRollupKind, InventoryRollupsWorkbench};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::view_model::format_quantity;

#[derive(Clone, Copy, PartialEq, Eq)]
enum InventoryView {
    Positions,
    Location,
    Facility,
    Item,
}

#[component]
pub fn InventoryWorkspace(
    initial_balances: Vec<InventoryBalanceResponse>,
    initial_cursor: Option<OpaqueCursor>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let section = RwSignal::new(InventoryView::Positions);
    let initial = StoredValue::new((initial_balances, initial_cursor));

    view! {
        <div class="inventory-workspace">
            <nav class="inventory-view-tabs" aria-label="Inventory views">
                <InventoryViewTab
                    label="Positions"
                    selected=Signal::derive(move || section.get() == InventoryView::Positions)
                    select=Callback::new(move |_| section.set(InventoryView::Positions))
                />
                <InventoryViewTab
                    label="By location"
                    selected=Signal::derive(move || section.get() == InventoryView::Location)
                    select=Callback::new(move |_| section.set(InventoryView::Location))
                />
                <InventoryViewTab
                    label="By facility"
                    selected=Signal::derive(move || section.get() == InventoryView::Facility)
                    select=Callback::new(move |_| section.set(InventoryView::Facility))
                />
                <InventoryViewTab
                    label="By item"
                    selected=Signal::derive(move || section.get() == InventoryView::Item)
                    select=Callback::new(move |_| section.set(InventoryView::Item))
                />
            </nav>
            {move || match section.get() {
                InventoryView::Positions => {
                    let (balances, cursor) = initial.get_value();
                    view! {
                        <InventoryTable
                            initial_balances=balances
                            initial_cursor=cursor
                            on_unauthorized
                        />
                    }
                        .into_any()
                }
                InventoryView::Location => {
                    view! {
                        <InventoryRollupsWorkbench
                            kind=InventoryRollupKind::Location
                            on_unauthorized
                        />
                    }
                        .into_any()
                }
                InventoryView::Facility => {
                    view! {
                        <InventoryRollupsWorkbench
                            kind=InventoryRollupKind::Facility
                            on_unauthorized
                        />
                    }
                        .into_any()
                }
                InventoryView::Item => {
                    view! {
                        <InventoryRollupsWorkbench
                            kind=InventoryRollupKind::Item
                            on_unauthorized
                        />
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn InventoryViewTab(
    label: &'static str,
    selected: Signal<bool>,
    select: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class:active=move || selected.get()
            aria-current=move || select_if(selected.get())
            on:click=move |_| select.run(())
        >
            {label}
        </button>
    }
}

const fn select_if(selected: bool) -> Option<&'static str> {
    if selected {
        Some("page")
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InventorySort {
    Position,
    Facility,
    Client,
    Location,
    Item,
    Tracking,
    LicensePlate,
    Status,
    OnHand,
    Reserved,
    Held,
}

#[derive(Clone, Copy)]
struct InventoryTableSignals {
    balances: RwSignal<Vec<InventoryBalanceResponse>>,
    next_cursor: RwSignal<Option<OpaqueCursor>>,
    filter: RwSignal<String>,
    applied_filter: RwSignal<String>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    sort: RwSignal<SortSpec<InventorySort>>,
}

#[component]
pub fn InventoryTable(
    initial_balances: Vec<InventoryBalanceResponse>,
    initial_cursor: Option<OpaqueCursor>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let signals = InventoryTableSignals {
        balances: RwSignal::new(initial_balances),
        next_cursor: RwSignal::new(initial_cursor),
        filter: RwSignal::new(String::new()),
        applied_filter: RwSignal::new(String::new()),
        loading: RwSignal::new(false),
        error: RwSignal::new(None),
        generation: RwSignal::new(0),
        sort: RwSignal::new(SortSpec {
            key: InventorySort::Position,
            direction: SortDirection::Ascending,
        }),
    };
    let load_more = move |_| {
        let Some(cursor) = signals.next_cursor.get_untracked() else {
            return;
        };
        if signals.loading.get_untracked() {
            return;
        }
        signals.loading.set(true);
        signals.error.set(None);
        let generation = signals.generation.get_untracked() + 1;
        signals.generation.set(generation);
        let query = signals.applied_filter.get_untracked();
        let spec = signals.sort.get_untracked();
        leptos::task::spawn_local(async move {
            match api::sorted_balances(
                (!query.is_empty()).then_some(query.as_str()),
                map_inventory_sort(spec.key),
                map_inventory_direction(spec.direction),
                Some(&cursor),
            )
            .await
            {
                Ok(page) if signals.generation.get_untracked() == generation => {
                    signals
                        .balances
                        .update(|current| current.extend(page.items));
                    signals.next_cursor.set(page.next_cursor);
                    signals.loading.set(false);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) if signals.generation.get_untracked() == generation => {
                    signals.error.set(Some(error.message));
                    signals.loading.set(false);
                }
                _ => {}
            }
        });
    };
    let apply_filter = move |_| {
        signals
            .applied_filter
            .set(signals.filter.get_untracked().trim().to_owned());
        request_inventory_page(signals, on_unauthorized);
    };

    view! {
        <section class="data-section page-data">
            <div class="table-toolbar">
                <div class="toolbar-summary">
                    <strong>{move || format_quantity(signals.balances.get().len() as i64)}</strong>
                    <span>"positions loaded"</span>
                </div>
                <SearchField
                    label="Search all inventory positions".to_owned()
                    placeholder="Filter positions"
                    value=signals.filter
                />
                <button type="button" class="button secondary-action" disabled=move || signals.loading.get() on:click=apply_filter>"Apply"</button>
            </div>
            <div class="table-scroll">
                <table class="data-table inventory-position-table">
                    <caption class="sr-only">"Inventory balances in the current access scope"</caption>
                    <thead>
                        <tr>
                            <SortableHeader
                                label="Facility"
                                active=move || signals.sort.get().key == InventorySort::Facility
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::Facility, on_unauthorized)
                                })
                            />
                            <SortableHeader
                                label="Client"
                                active=move || signals.sort.get().key == InventorySort::Client
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::Client, on_unauthorized)
                                })
                            />
                            <SortableHeader
                                label="Location"
                                active=move || signals.sort.get().key == InventorySort::Location
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::Location, on_unauthorized)
                                })
                            />
                            <SortableHeader
                                label="Item"
                                active=move || signals.sort.get().key == InventorySort::Item
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::Item, on_unauthorized)
                                })
                            />
                            <SortableHeader
                                label="Lot / serial"
                                active=move || signals.sort.get().key == InventorySort::Tracking
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::Tracking, on_unauthorized)
                                })
                            />
                            <SortableHeader
                                label="License plate"
                                active=move || signals.sort.get().key == InventorySort::LicensePlate
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::LicensePlate, on_unauthorized)
                                })
                            />
                            <SortableHeader
                                label="Status"
                                active=move || signals.sort.get().key == InventorySort::Status
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::Status, on_unauthorized)
                                })
                            />
                            <SortableHeader
                                label="On hand"
                                active=move || signals.sort.get().key == InventorySort::OnHand
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::OnHand, on_unauthorized)
                                })
                                numeric=true
                            />
                            <SortableHeader
                                label="Reserved"
                                active=move || signals.sort.get().key == InventorySort::Reserved
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::Reserved, on_unauthorized)
                                })
                                numeric=true
                            />
                            <SortableHeader
                                label="Held"
                                active=move || signals.sort.get().key == InventorySort::Held
                                direction=move || signals.sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    select_inventory_sort(signals, InventorySort::Held, on_unauthorized)
                                })
                                numeric=true
                            />
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let rows = signals.balances.get();
                            if rows.is_empty() {
                                view! {
                                    <tr>
                                        <td class="table-empty-row" colspan="10">"No inventory positions match the current filters."</td>
                                    </tr>
                                }
                                    .into_any()
                            } else {
                                rows
                                    .into_iter()
                                    .map(|balance| {
                                        let location = location_label(&balance);
                                        let item = item_label(&balance);
                                        let item_detail = item_detail(&balance);
                                        let tracking = tracking_label(&balance);
                                        let license_plate = license_plate_label(&balance);
                                        view! {
                                            <tr>
                                                <td>{balance.facility_name.unwrap_or_else(|| {
                                                    format!("Facility {}", balance.facility_id)
                                                })}</td>
                                                <td>{balance.inventory_owner_name}</td>
                                                <td><strong>{location}</strong></td>
                                                <td>
                                                    <strong>{item}</strong>
                                                    {item_detail.map(|description| {
                                                        view! { <small class="cell-detail">{description}</small> }
                                                    })}
                                                </td>
                                                <td>{tracking}</td>
                                                <td>{license_plate}</td>
                                                <td>
                                                    <span class=status_class(balance.status)>
                                                        {status_label(balance.status)}
                                                    </span>
                                                </td>
                                                <td class="numeric strong">
                                                    {format_quantity(balance.quantity.on_hand)}
                                                </td>
                                                <td class="numeric">
                                                    {format_quantity(balance.quantity.reserved)}
                                                </td>
                                                <td class="numeric">
                                                    {format_quantity(balance.quantity.held)}
                                                </td>
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
            <div class="table-footer">
                <span>
                    {move || {
                        signals.next_cursor
                            .get()
                            .map_or("All positions loaded", |_| "More positions available")
                    }}
                </span>
                {move || {
                    signals.error.get().map(|message| {
                        view! { <span class="inline-error" role="alert">{message}</span> }
                    })
                }}
                <button
                    class="button secondary-action"
                    type="button"
                    on:click=load_more
                    disabled=move || signals.next_cursor.get().is_none() || signals.loading.get()
                >
                    {move || if signals.loading.get() { "Loading" } else { "Load more" }}
                </button>
            </div>
        </section>
    }
}

fn select_inventory_sort(
    signals: InventoryTableSignals,
    key: InventorySort,
    on_unauthorized: Callback<()>,
) {
    SortSpec::select(signals.sort, key);
    request_inventory_page(signals, on_unauthorized);
}

fn request_inventory_page(signals: InventoryTableSignals, on_unauthorized: Callback<()>) {
    let generation = signals.generation.get_untracked() + 1;
    signals.generation.set(generation);
    signals.loading.set(true);
    signals.error.set(None);
    let query = signals.applied_filter.get_untracked();
    let spec = signals.sort.get_untracked();
    leptos::task::spawn_local(async move {
        match api::sorted_balances(
            (!query.is_empty()).then_some(query.as_str()),
            map_inventory_sort(spec.key),
            map_inventory_direction(spec.direction),
            None,
        )
        .await
        {
            Ok(page) if signals.generation.get_untracked() == generation => {
                signals.balances.set(page.items);
                signals.next_cursor.set(page.next_cursor);
                signals.loading.set(false);
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) if signals.generation.get_untracked() == generation => {
                signals.error.set(Some(error.message));
                signals.loading.set(false);
            }
            _ => {}
        }
    });
}

fn map_inventory_sort(value: InventorySort) -> ApiInventoryBalanceSort {
    match value {
        InventorySort::Position => ApiInventoryBalanceSort::Position,
        InventorySort::Facility => ApiInventoryBalanceSort::Facility,
        InventorySort::Client => ApiInventoryBalanceSort::Client,
        InventorySort::Location => ApiInventoryBalanceSort::Location,
        InventorySort::Item => ApiInventoryBalanceSort::Item,
        InventorySort::Tracking => ApiInventoryBalanceSort::Tracking,
        InventorySort::LicensePlate => ApiInventoryBalanceSort::LicensePlate,
        InventorySort::Status => ApiInventoryBalanceSort::Status,
        InventorySort::OnHand => ApiInventoryBalanceSort::OnHand,
        InventorySort::Reserved => ApiInventoryBalanceSort::Reserved,
        InventorySort::Held => ApiInventoryBalanceSort::Held,
    }
}

fn map_inventory_direction(value: SortDirection) -> ApiInventorySortDirection {
    match value {
        SortDirection::Ascending => ApiInventorySortDirection::Ascending,
        SortDirection::Descending => ApiInventorySortDirection::Descending,
    }
}

fn location_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .location_barcode
        .clone()
        .or_else(|| balance.location_name.clone())
        .unwrap_or_else(|| format!("#{}", balance.location_id))
}

fn license_plate_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .license_plate_barcode
        .clone()
        .or_else(|| balance.license_plate_id.map(|id| format!("#{id}")))
        .unwrap_or_else(|| "-".to_owned())
}

fn status_label(status: InventoryBalanceStatus) -> &'static str {
    match status {
        InventoryBalanceStatus::Available => "Available",
        InventoryBalanceStatus::Hold => "Hold",
        InventoryBalanceStatus::Damaged => "Damaged",
        InventoryBalanceStatus::Quarantine => "Quarantine",
    }
}

fn status_class(status: InventoryBalanceStatus) -> &'static str {
    match status {
        InventoryBalanceStatus::Available => "status shipped",
        InventoryBalanceStatus::Hold | InventoryBalanceStatus::Damaged => "status held",
        InventoryBalanceStatus::Quarantine => "status processing",
    }
}

fn item_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .primary_sku
        .clone()
        .or_else(|| balance.item_description.clone())
        .unwrap_or_else(|| format!("#{}", balance.item_id))
}

fn item_detail(balance: &InventoryBalanceResponse) -> Option<String> {
    balance
        .primary_sku
        .as_ref()
        .and(balance.item_description.clone())
}

fn tracking_label(balance: &InventoryBalanceResponse) -> String {
    match (&balance.lot, &balance.serial) {
        (Some(lot), Some(serial)) => format!("{lot} / {serial}"),
        (Some(lot), None) => lot.clone(),
        (None, Some(serial)) => serial.clone(),
        (None, None) => "-".to_owned(),
    }
}
