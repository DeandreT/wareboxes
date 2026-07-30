use leptos::prelude::*;
use wareboxes_api_contract::v1::{InventoryBalanceResponse, InventoryBalanceStatus, OpaqueCursor};

use crate::api;
use crate::components::SearchField;
use crate::view_model::format_quantity;

#[component]
pub fn InventoryTable(
    initial_balances: Vec<InventoryBalanceResponse>,
    initial_cursor: Option<OpaqueCursor>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let balances = RwSignal::new(initial_balances);
    let next_cursor = RwSignal::new(initial_cursor);
    let filter = RwSignal::new(String::new());
    let loading_more = RwSignal::new(false);
    let page_error = RwSignal::new(None::<String>);
    let load_more = move |_| {
        let Some(cursor) = next_cursor.get_untracked() else {
            return;
        };
        if loading_more.get_untracked() {
            return;
        }
        loading_more.set(true);
        page_error.set(None);
        leptos::task::spawn_local(async move {
            match api::balances(Some(&cursor)).await {
                Ok(page) => {
                    balances.update(|current| current.extend(page.items));
                    next_cursor.set(page.next_cursor);
                    loading_more.set(false);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    page_error.set(Some(error.message));
                    loading_more.set(false);
                }
            }
        });
    };

    view! {
        <section class="data-section page-data">
            <div class="table-toolbar">
                <div class="toolbar-summary">
                    <strong>{move || format_quantity(balances.get().len() as i64)}</strong>
                    <span>"positions loaded"</span>
                </div>
                <SearchField
                    label="Filter loaded inventory positions".to_owned()
                    placeholder="Filter positions"
                    value=filter
                />
            </div>
            <div class="table-scroll">
                <table class="data-table">
                    <caption class="sr-only">"Inventory balances in the current access scope"</caption>
                    <thead>
                        <tr>
                            <th scope="col">"Facility"</th>
                            <th scope="col">"Owner"</th>
                            <th scope="col">"Location"</th>
                            <th scope="col">"Item"</th>
                            <th scope="col">"Lot / serial"</th>
                            <th scope="col">"License plate"</th>
                            <th scope="col">"Status"</th>
                            <th scope="col" class="numeric">"On hand"</th>
                            <th scope="col" class="numeric">"Reserved"</th>
                            <th scope="col" class="numeric">"Held"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let query = filter.get().trim().to_ascii_lowercase();
                            let all_balances = balances.get();
                            let is_empty = all_balances.is_empty();
                            let matching = all_balances
                                .into_iter()
                                .filter(|balance| balance_matches(balance, &query))
                                .collect::<Vec<_>>();
                            if matching.is_empty() {
                                let message = if is_empty {
                                    "No inventory balances are currently in scope."
                                } else {
                                    "No loaded positions match this filter."
                                };
                                view! {
                                    <tr>
                                        <td class="table-empty-row" colspan="10">{message}</td>
                                    </tr>
                                }
                                    .into_any()
                            } else {
                                matching
                                    .into_iter()
                                    .map(|balance| {
                                        let location = location_label(&balance);
                                        let item = item_label(&balance);
                                        let item_detail = item_detail(&balance);
                                        let tracking = tracking_label(&balance);
                                        let license_plate = balance
                                            .license_plate_barcode
                                            .clone()
                                            .or_else(|| {
                                                balance.license_plate_id.map(|id| format!("#{id}"))
                                            })
                                            .unwrap_or_else(|| "-".to_owned());
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
                        next_cursor
                            .get()
                            .map_or("All positions loaded", |_| "More positions available")
                    }}
                </span>
                {move || {
                    page_error.get().map(|message| {
                        view! { <span class="inline-error" role="alert">{message}</span> }
                    })
                }}
                <button
                    class="button secondary-action"
                    type="button"
                    on:click=load_more
                    disabled=move || next_cursor.get().is_none() || loading_more.get()
                >
                    {move || if loading_more.get() { "Loading" } else { "Load more" }}
                </button>
            </div>
        </section>
    }
}

fn location_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .location_barcode
        .clone()
        .or_else(|| balance.location_name.clone())
        .unwrap_or_else(|| format!("#{}", balance.location_id))
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

fn balance_matches(balance: &InventoryBalanceResponse, query: &str) -> bool {
    query.is_empty()
        || [
            balance.inventory_owner_name.as_str(),
            balance.facility_name.as_deref().unwrap_or_default(),
            balance.location_name.as_deref().unwrap_or_default(),
            balance.location_barcode.as_deref().unwrap_or_default(),
            balance.license_plate_barcode.as_deref().unwrap_or_default(),
            balance.item_description.as_deref().unwrap_or_default(),
            balance.primary_sku.as_deref().unwrap_or_default(),
            balance.lot.as_deref().unwrap_or_default(),
            balance.serial.as_deref().unwrap_or_default(),
            balance.uom.as_str(),
            status_label(balance.status),
        ]
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(query))
        || [
            balance.id,
            balance.location_id,
            balance.item_id,
            balance.item_batch_id,
        ]
        .iter()
        .any(|value| value.to_string().contains(query))
}
