use leptos::prelude::*;
use wareboxes_api_contract::v1::{InventoryBalanceResponse, OpaqueCursor};
use wareboxes_core::models::Location;

use crate::{api, view_model::format_quantity};

#[component]
pub(super) fn MovePlanner(
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
                                                location.barcode.as_deref().unwrap_or("Unscannable"),
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

pub(super) fn movable_quantity(balance: &InventoryBalanceResponse) -> i64 {
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

fn positive_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|id| *id > 0)
}
