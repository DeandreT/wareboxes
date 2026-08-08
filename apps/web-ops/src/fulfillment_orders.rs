use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use std::time::Duration;
use wareboxes_api_contract::v1::{
    CreateFulfillmentOrderLineRequest, CreateFulfillmentOrderRequest, FulfillmentOrderDestination,
    OrderEntryItemResponse, OrderHoldReason as OrderHoldRequestReason, PlaceOrderHoldRequest,
    ReleaseOrderHoldRequest,
};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};
use wareboxes_core::dto::{OrderPage, OrderUpdate};
use wareboxes_core::models::{Order, OrderStatus};

use crate::api;
use crate::components::{Icon, SearchField, UiIcon};
use crate::fulfillment_order_allocation::OrderAllocationPanel;
use crate::fulfillment_order_cancellation::OrderCancellationPanel;
use crate::fulfillment_shared::{
    cmp_option_str, optional_text, order_destination, order_status_class, parse_optional_timestamp,
    query_encode, short_timestamp, timestamp_input,
};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

const ORDER_PAGE_SIZE: i64 = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OrderSort {
    Order,
    Client,
    Status,
    Units,
    ShipBy,
    Destination,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OrderDetailTab {
    Header,
    Lines,
    Fulfillment,
    Holds,
    Activity,
}

#[derive(Clone, PartialEq, Eq)]
struct DraftOrderLine {
    line_key: String,
    item_id: i64,
    description: String,
    requested_uom: String,
    quantity: i64,
}

#[component]
pub fn OrdersWorkbench(
    initial_page: OrderPage,
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let page = RwSignal::new(initial_page);
    let search = RwSignal::new(String::new());
    let status = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let selected = RwSignal::new(None::<Order>);
    let selected_pending = RwSignal::new(false);
    let selected_error = RwSignal::new(None::<String>);
    let create_open = RwSignal::new(false);
    let sort = RwSignal::new(SortSpec {
        key: OrderSort::Order,
        direction: SortDirection::Descending,
    });
    let toasts = use_toast_bus();
    let detail_facilities = StoredValue::new(access.facilities);
    let create_clients = StoredValue::new(access.inventory_owners);

    let run_search = move |offset: i64| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        error.set(None);
        let search_value = search.get_untracked();
        let status_value = status.get_untracked();
        leptos::task::spawn_local(async move {
            match fetch_orders(offset, &search_value, &status_value).await {
                Ok(next) => {
                    page.set(next);
                    pending.set(false);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    error.set(Some(api_error.message));
                    pending.set(false);
                }
            }
        });
    };

    let submit_search = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        run_search(0);
    };

    let open_order = move |order_id: i64| {
        create_open.set(false);
        request_order_detail(
            order_id,
            selected,
            selected_pending,
            selected_error,
            on_unauthorized,
        );
    };

    let refresh_after_command = Callback::new(move |order_id: i64| {
        request_order_detail(
            order_id,
            selected,
            selected_pending,
            selected_error,
            on_unauthorized,
        );
        let search_value = search.get_untracked();
        let status_value = status.get_untracked();
        let offset = page.get_untracked().page.offset;
        leptos::task::spawn_local(async move {
            match fetch_orders(offset, &search_value, &status_value).await {
                Ok(next) => page.set(next),
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => toasts.error(api_error.message),
            }
        });
    });

    let created = Callback::new(move |order_id: i64| {
        create_open.set(false);
        request_order_detail(
            order_id,
            selected,
            selected_pending,
            selected_error,
            on_unauthorized,
        );
        run_search(0);
    });

    view! {
        <div class="fulfillment-workbench" class:create-mode=move || create_open.get()>
            <section class="data-section fulfillment-list">
                <form class="table-toolbar fulfillment-toolbar" on:submit=submit_search>
                    <div class="toolbar-summary">
                        <strong>{move || format_quantity(page.get().page.total)}</strong>
                        <span>"orders"</span>
                    </div>
                    <div class="fulfillment-filters">
                        <SearchField
                            label="Search orders".to_owned()
                            placeholder="Order, client, destination"
                            value=search
                        />
                        <label>
                            <span class="sr-only">"Order status"</span>
                            <select
                                prop:value=move || status.get()
                                on:change=move |event| status.set(event_target_value(&event))
                            >
                                <option value="">"All statuses"</option>
                                {OrderStatus::ALL
                                    .into_iter()
                                    .map(|value| {
                                        view! {
                                            <option value=value.as_str()>{title_case(value.as_str())}</option>
                                        }
                                    })
                                    .collect_view()}
                            </select>
                        </label>
                        <button class="button secondary-action" type="submit" disabled=move || pending.get()>
                            {move || if pending.get() { "Loading" } else { "Apply" }}
                        </button>
                        <button
                            class="button primary-action"
                            type="button"
                            on:click=move |_| {
                                selected.set(None);
                                create_open.set(true);
                            }
                        >
                            "New order"
                        </button>
                    </div>
                </form>
                <div class="table-scroll">
                    <table class="data-table fulfillment-table orders-workbench-table">
                        <caption class="sr-only">"Orders matching the active filters"</caption>
                        <thead>
                            <tr>
                                <SortableHeader
                                    label="Order"
                                    active=move || sort.get().key == OrderSort::Order
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, OrderSort::Order))
                                />
                                <SortableHeader
                                    label="Client"
                                    active=move || sort.get().key == OrderSort::Client
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, OrderSort::Client))
                                />
                                <SortableHeader
                                    label="Status"
                                    active=move || sort.get().key == OrderSort::Status
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, OrderSort::Status))
                                />
                                <SortableHeader
                                    label="Units"
                                    active=move || sort.get().key == OrderSort::Units
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, OrderSort::Units))
                                    numeric=true
                                />
                                <SortableHeader
                                    label="Ship by"
                                    active=move || sort.get().key == OrderSort::ShipBy
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, OrderSort::ShipBy))
                                />
                                <SortableHeader
                                    label="Destination"
                                    active=move || sort.get().key == OrderSort::Destination
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, OrderSort::Destination)
                                    })
                                />
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let spec = sort.get();
                                let selected_id = selected.get().map(|order| order.id);
                                let mut orders = page.get().page.items;
                                orders.sort_by(|left, right| {
                                    let ordering = match spec.key {
                                        OrderSort::Order => left
                                            .order_key
                                            .to_ascii_lowercase()
                                            .cmp(&right.order_key.to_ascii_lowercase()),
                                        OrderSort::Client => cmp_option_str(
                                            left.inventory_owner_name.as_deref(),
                                            right.inventory_owner_name.as_deref(),
                                        ),
                                        OrderSort::Status => {
                                            left.status.as_str().cmp(right.status.as_str())
                                        }
                                        OrderSort::Units => left.ordered_qty.cmp(&right.ordered_qty),
                                        OrderSort::ShipBy => left.ship_by.cmp(&right.ship_by),
                                        OrderSort::Destination => order_destination(left)
                                            .to_ascii_lowercase()
                                            .cmp(&order_destination(right).to_ascii_lowercase()),
                                    }
                                    .then_with(|| left.id.cmp(&right.id));
                                    if spec.direction == SortDirection::Ascending {
                                        ordering
                                    } else {
                                        ordering.reverse()
                                    }
                                });
                                orders
                                    .into_iter()
                                    .map(|order| {
                                        let id = order.id;
                                        let is_selected = selected_id == Some(id);
                                        let destination = order_destination(&order);
                                        view! {
                                            <tr
                                                class:active-row=is_selected
                                                on:click=move |_| open_order(id)
                                            >
                                                <td>
                                                    <button
                                                        type="button"
                                                        class="row-link"
                                                        on:click=move |event| {
                                                            event.stop_propagation();
                                                            open_order(id);
                                                        }
                                                    >
                                                        {order.order_key}
                                                    </button>
                                                    {order.rush.then(|| view! { <small class="rush">"Rush"</small> })}
                                                </td>
                                                <td>{order.inventory_owner_name.unwrap_or_else(|| "Unassigned".to_owned())}</td>
                                                <td>
                                                    <span class=order_status_class(order.status)>
                                                        {title_case(order.status.as_str())}
                                                    </span>
                                                </td>
                                                <td class="numeric">{format_quantity(order.ordered_qty)}</td>
                                                <td>{order.ship_by.map_or_else(|| "-".to_owned(), short_timestamp)}</td>
                                                <td>{if destination.is_empty() { "-".to_owned() } else { destination }}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </tbody>
                    </table>
                    <Show when=move || page.get().page.items.is_empty()>
                        <p class="empty-state">"No orders match the active filters."</p>
                    </Show>
                </div>
                <div class="table-footer">
                    <span>
                        {move || {
                            let current = page.get().page;
                            if current.total == 0 {
                                "No results".to_owned()
                            } else {
                                let start = current.offset + 1;
                                let end = (current.offset + current.items.len() as i64).min(current.total);
                                format!("{start}-{end} of {}", current.total)
                            }
                        }}
                    </span>
                    <button
                        type="button"
                        class="button secondary-action"
                        disabled=move || page.get().page.offset == 0 || pending.get()
                        on:click=move |_| {
                            let offset = (page.get_untracked().page.offset - ORDER_PAGE_SIZE).max(0);
                            run_search(offset);
                        }
                    >
                        "Previous"
                    </button>
                    <button
                        type="button"
                        class="button secondary-action"
                        disabled=move || {
                            let current = page.get();
                            current.page.offset + current.page.items.len() as i64 >= current.page.total
                                || pending.get()
                        }
                        on:click=move |_| {
                            run_search(page.get_untracked().page.offset + ORDER_PAGE_SIZE);
                        }
                    >
                        "Next"
                    </button>
                </div>
                <Show when=move || error.get().is_some()>
                    <p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
                </Show>
            </section>

            <aside class="command-panel fulfillment-detail">
                <Show
                    when=move || create_open.get()
                    fallback=move || {
                        view! {
                            <Show
                                when=move || selected.get().is_some()
                                fallback=move || {
                                    view! {
                                        <div class="command-placeholder">
                                            <h2>"Order details"</h2>
                                            <p>"Select an order to inspect its header, demand, reservations, tracking, and activity."</p>
                                        </div>
                                    }
                                }
                            >
                                {move || {
                                    selected
                                        .get()
                                        .map(|order| {
                                            view! {
                                                <OrderDetailPanel
                                                    order
                                                    facilities=detail_facilities.get_value()
                                                    pending=selected_pending
                                                    load_error=selected_error
                                                    on_refreshed=refresh_after_command
                                                    on_unauthorized
                                                />
                                            }
                                        })
                                }}
                            </Show>
                        }
                    }
                >
                    <CreateOrderPanel
                        clients=create_clients.get_value()
                        on_created=created
                        on_close=Callback::new(move |_| create_open.set(false))
                        on_unauthorized
                    />
                </Show>
            </aside>
        </div>
    }
}

#[component]
fn CreateOrderPanel(
    clients: Vec<AccessScopeResource>,
    on_created: Callback<i64>,
    on_close: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let key = RwSignal::new(String::new());
    let client_id = RwSignal::new(
        clients
            .first()
            .map_or_else(String::new, |client| client.id.to_string()),
    );
    let rush = RwSignal::new(false);
    let ship_by = RwSignal::new(String::new());
    let line1 = RwSignal::new(String::new());
    let line2 = RwSignal::new(String::new());
    let city = RwSignal::new(String::new());
    let state = RwSignal::new(String::new());
    let postal_code = RwSignal::new(String::new());
    let country = RwSignal::new("US".to_owned());
    let item_search = RwSignal::new(String::new());
    let catalog_query = RwSignal::new(String::new());
    let entry_items = RwSignal::new(Vec::<OrderEntryItemResponse>::new());
    let items_pending = RwSignal::new(false);
    let selected_item_id = RwSignal::new(String::new());
    let line_quantity = RwSignal::new("1".to_owned());
    let lines = RwSignal::new(Vec::<DraftOrderLine>::new());
    let next_line_number = RwSignal::new(1_i64);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry_attempt = RwSignal::new(None::<(CreateFulfillmentOrderRequest, String)>);
    let toasts = use_toast_bus();

    Effect::new(move |_| {
        let search = item_search.get();
        #[cfg(target_arch = "wasm32")]
        set_timeout(
            move || {
                if item_search.get_untracked() == search && catalog_query.get_untracked() != search
                {
                    catalog_query.set(search);
                }
            },
            Duration::from_millis(250),
        );
        #[cfg(not(target_arch = "wasm32"))]
        catalog_query.set(search);
    });

    Effect::new(move |_| {
        let selected_client = client_id.get();
        let selected_search = catalog_query.get();
        entry_items.set(Vec::new());
        selected_item_id.set(String::new());
        let Ok(inventory_owner_id) = selected_client.parse::<i64>() else {
            return;
        };
        items_pending.set(true);
        leptos::task::spawn_local(async move {
            match api::order_entry_items(inventory_owner_id, &selected_search).await {
                Ok(items) => {
                    if client_id.get_untracked() == selected_client
                        && catalog_query.get_untracked() == selected_search
                    {
                        entry_items.set(items);
                        items_pending.set(false);
                    }
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    if client_id.get_untracked() == selected_client
                        && catalog_query.get_untracked() == selected_search
                    {
                        error.set(Some(api_error.message));
                        items_pending.set(false);
                    }
                }
            }
        });
    });

    let add_line = move |_| {
        let Ok(item_id) = selected_item_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose an item for the demand line.".to_owned()));
            return;
        };
        let Ok(quantity) = line_quantity.get_untracked().parse::<i64>() else {
            error.set(Some(
                "Line quantity must be a positive whole number.".to_owned(),
            ));
            return;
        };
        if quantity <= 0 {
            error.set(Some(
                "Line quantity must be a positive whole number.".to_owned(),
            ));
            return;
        }
        let Some(item) = entry_items
            .get_untracked()
            .into_iter()
            .find(|item| item.item_id == item_id)
        else {
            error.set(Some("The selected item is no longer available.".to_owned()));
            return;
        };
        let mut current_lines = lines.get_untracked();
        if let Some(existing) = current_lines
            .iter_mut()
            .find(|line| line.item_id == item.item_id && line.requested_uom == item.requested_uom)
        {
            let Some(merged) = existing.quantity.checked_add(quantity) else {
                error.set(Some("Line quantity is too large.".to_owned()));
                return;
            };
            existing.quantity = merged;
        } else {
            let line_number = next_line_number.get_untracked();
            current_lines.push(DraftOrderLine {
                line_key: line_number.to_string(),
                item_id: item.item_id,
                description: item
                    .description
                    .unwrap_or_else(|| format!("Item #{}", item.item_id)),
                requested_uom: item.requested_uom,
                quantity,
            });
            next_line_number.set(line_number.saturating_add(1));
        }
        lines.set(current_lines);
        selected_item_id.set(String::new());
        line_quantity.set("1".to_owned());
        error.set(None);
    };

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let order_key = key.get_untracked().trim().to_owned();
        if order_key.is_empty() {
            error.set(Some("Enter an order number.".to_owned()));
            return;
        }
        let Ok(inventory_owner_id) = client_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a client.".to_owned()));
            return;
        };
        let line1_value = line1.get_untracked().trim().to_owned();
        let city_value = city.get_untracked().trim().to_owned();
        let state_value = state.get_untracked().trim().to_owned();
        let postal_value = postal_code.get_untracked().trim().to_owned();
        let country_value = country.get_untracked().trim().to_owned();
        if [
            line1_value.as_str(),
            city_value.as_str(),
            state_value.as_str(),
            postal_value.as_str(),
            country_value.as_str(),
        ]
        .iter()
        .any(|value| value.is_empty())
        {
            error.set(Some(
                "Address, city, state, postal code, and country are required.".to_owned(),
            ));
            return;
        }
        let ship_by_value = match parse_optional_timestamp(&ship_by.get_untracked()) {
            Ok(value) => value,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        let draft_lines = lines.get_untracked();
        if draft_lines.is_empty() {
            error.set(Some("Add at least one demand line.".to_owned()));
            return;
        }
        if draft_lines.iter().any(|line| line.quantity <= 0) {
            error.set(Some(
                "Every demand line must have a positive whole-number quantity.".to_owned(),
            ));
            return;
        }
        let request = CreateFulfillmentOrderRequest {
            inventory_owner_id,
            order_key: order_key.clone(),
            rush: rush.get_untracked(),
            ship_by: ship_by_value.map(|value| value.to_rfc3339()),
            destination: FulfillmentOrderDestination {
                line1: line1_value,
                line2: optional_text(&line2.get_untracked()),
                city: city_value,
                region: state_value,
                postal_code: postal_value,
                country: country_value,
            },
            lines: draft_lines
                .iter()
                .map(|line| CreateFulfillmentOrderLineRequest {
                    line_key: line.line_key.clone(),
                    item_id: line.item_id,
                    quantity: line.quantity,
                    requested_uom: line.requested_uom.clone(),
                })
                .collect(),
        };
        pending.set(true);
        error.set(None);
        let idempotency_key = retry_attempt
            .get_untracked()
            .filter(|(prior_request, _)| prior_request == &request)
            .map_or_else(api::new_idempotency_key, |(_, key)| key);
        retry_attempt.set(Some((request.clone(), idempotency_key.clone())));
        leptos::task::spawn_local(async move {
            match api::create_fulfillment_order(&request, &idempotency_key).await {
                Ok(result) => {
                    pending.set(false);
                    retry_attempt.set(None);
                    toasts.success(format!(
                        "Order {order_key} created with {} line(s).",
                        result.lines.len()
                    ));
                    on_created.run(result.order_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    error.set(Some(api_error.message));
                    pending.set(false);
                }
            }
        });
    };

    view! {
        <form class="fulfillment-form" on:submit=submit>
            <div class="detail-heading">
                <div>
                    <span class="eyebrow">"Order entry"</span>
                    <h2>"New order"</h2>
                </div>
                <button type="button" class="text-button" on:click=move |_| on_close.run(())>
                    "Close"
                </button>
            </div>
            <div class="form-grid two-column">
                <label>
                    <span>"Order number"</span>
                    <input
                        required
                        autocomplete="off"
                        prop:value=move || key.get()
                        on:input=move |event| key.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Client"</span>
                    <select
                        required
                        disabled=move || pending.get() || !lines.get().is_empty()
                        prop:value=move || client_id.get()
                        on:change=move |event| client_id.set(event_target_value(&event))
                    >
                        {clients
                            .into_iter()
                            .map(|client| view! { <option value=client.id>{client.name}</option> })
                            .collect_view()}
                    </select>
                </label>
                <label>
                    <span>"Ship by (UTC)"</span>
                    <input
                        type="datetime-local"
                        prop:value=move || ship_by.get()
                        on:input=move |event| ship_by.set(event_target_value(&event))
                    />
                </label>
                <label class="checkbox-label">
                    <input
                        type="checkbox"
                        prop:checked=move || rush.get()
                        on:change=move |event| rush.set(event_target_checked(&event))
                    />
                    <span>"Rush order"</span>
                </label>
            </div>
            <fieldset>
                <legend>"Demand lines"</legend>
                <div class="order-line-entry">
                    <label>
                        <span>"Find item"</span>
                        <input
                            type="search"
                            autocomplete="off"
                            placeholder="SKU, barcode, or description"
                            disabled=move || pending.get()
                            prop:value=move || item_search.get()
                            on:input=move |event| {
                                item_search.set(event_target_value(&event));
                                selected_item_id.set(String::new());
                            }
                        />
                    </label>
                    <label>
                        <span>"Item"</span>
                        <select
                            disabled=move || {
                                pending.get()
                                    || items_pending.get()
                                    || item_search.get() != catalog_query.get()
                            }
                            prop:value=move || selected_item_id.get()
                            on:change=move |event| selected_item_id.set(event_target_value(&event))
                        >
                            <option value="">{move || {
                                if item_search.get() != catalog_query.get() {
                                    "Searching items"
                                } else if items_pending.get() {
                                    "Loading items"
                                } else {
                                    "Select item"
                                }
                            }}</option>
                            {move || entry_items
                                .get()
                                .into_iter()
                                .map(|item| {
                                    let label = item
                                        .description
                                        .unwrap_or_else(|| format!("Item #{}", item.item_id));
                                    view! {
                                        <option value=item.item_id>{format!("{label} - {}", item.requested_uom)}</option>
                                    }
                                })
                                .collect_view()}
                        </select>
                        <Show when=move || {
                            item_search.get() == catalog_query.get()
                                && !items_pending.get()
                                && entry_items.get().len() == 50
                        }>
                            <small class="cell-detail">"50 matches shown"</small>
                        </Show>
                    </label>
                    <label>
                        <span>"Quantity"</span>
                        <input
                            type="number"
                            min="1"
                            step="1"
                            disabled=move || pending.get()
                            prop:value=move || line_quantity.get()
                            on:input=move |event| line_quantity.set(event_target_value(&event))
                        />
                    </label>
                    <button
                        type="button"
                        class="button secondary-action order-line-add"
                        disabled=move || {
                            pending.get()
                                || items_pending.get()
                                || item_search.get() != catalog_query.get()
                        }
                        on:click=add_line
                    >
                        <Icon icon=UiIcon::Add/>
                        "Add line"
                    </button>
                </div>
                <div class="table-scroll order-lines-draft-scroll">
                    <table class="data-table order-lines-draft-table">
                        <thead>
                            <tr><th>"Line"</th><th>"Item"</th><th>"UOM"</th><th>"Qty"</th><th></th></tr>
                        </thead>
                        <tbody>
                            {move || lines
                                .get()
                                .into_iter()
                                .enumerate()
                                .map(|(index, line)| {
                                    let item_id = line.item_id;
                                    view! {
                                        <tr>
                                            <td><strong>{line.line_key}</strong></td>
                                            <td>
                                                <strong>{line.description}</strong>
                                                <small class="cell-detail">{format!("Item #{item_id}")}</small>
                                            </td>
                                            <td>{line.requested_uom}</td>
                                            <td>
                                                <input
                                                    class="line-quantity-input"
                                                    type="number"
                                                    min="1"
                                                    step="1"
                                                    required
                                                    disabled=move || pending.get()
                                                    prop:value=line.quantity
                                                    on:input=move |event| {
                                                        let quantity = event_target_value(&event)
                                                            .parse::<i64>()
                                                            .unwrap_or(0);
                                                        lines.update(|values| {
                                                            if let Some(value) = values.get_mut(index) {
                                                                value.quantity = quantity;
                                                            }
                                                        });
                                                    }
                                                />
                                            </td>
                                            <td>
                                                <button
                                                    type="button"
                                                    class="button order-line-remove"
                                                    title="Remove demand line"
                                                    aria-label="Remove demand line"
                                                    disabled=move || pending.get()
                                                    on:click=move |_| {
                                                        lines.update(|values| {
                                                            if index < values.len() {
                                                                values.remove(index);
                                                            }
                                                        });
                                                    }
                                                >
                                                    <Icon icon=UiIcon::Remove/>
                                                </button>
                                            </td>
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                    {move || lines.get().is_empty().then(|| {
                        view! { <p class="empty-state compact">"No demand lines added."</p> }
                    })}
                </div>
                <div class="order-lines-draft-summary">
                    <span>{move || format!("{} lines", lines.get().len())}</span>
                    <strong>{move || format!(
                        "{} units",
                        lines
                            .get()
                            .iter()
                            .fold(0_i64, |total, line| {
                                total.saturating_add(line.quantity.max(0))
                            })
                    )}</strong>
                    <button
                        type="button"
                        class="text-button"
                        disabled=move || pending.get() || lines.get().is_empty()
                        on:click=move |_| {
                            lines.set(Vec::new());
                            next_line_number.set(1);
                        }
                    >
                        "Clear lines"
                    </button>
                </div>
            </fieldset>
            <fieldset>
                <legend>"Ship to"</legend>
                <div class="form-grid two-column">
                    <label class="wide">
                        <span>"Address line 1"</span>
                        <input
                            required
                            autocomplete="street-address"
                            prop:value=move || line1.get()
                            on:input=move |event| line1.set(event_target_value(&event))
                        />
                    </label>
                    <label class="wide">
                        <span>"Address line 2"</span>
                        <input
                            prop:value=move || line2.get()
                            on:input=move |event| line2.set(event_target_value(&event))
                        />
                    </label>
                    <label>
                        <span>"City"</span>
                        <input
                            required
                            autocomplete="address-level2"
                            prop:value=move || city.get()
                            on:input=move |event| city.set(event_target_value(&event))
                        />
                    </label>
                    <label>
                        <span>"State / region"</span>
                        <input
                            required
                            autocomplete="address-level1"
                            prop:value=move || state.get()
                            on:input=move |event| state.set(event_target_value(&event))
                        />
                    </label>
                    <label>
                        <span>"Postal code"</span>
                        <input
                            required
                            autocomplete="postal-code"
                            prop:value=move || postal_code.get()
                            on:input=move |event| postal_code.set(event_target_value(&event))
                        />
                    </label>
                    <label>
                        <span>"Country"</span>
                        <input
                            required
                            autocomplete="country"
                            prop:value=move || country.get()
                            on:input=move |event| country.set(event_target_value(&event))
                        />
                    </label>
                </div>
            </fieldset>
            <Show when=move || error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <div class="form-actions">
                <button class="button primary-action" type="submit" disabled=move || pending.get()>
                    {move || if pending.get() { "Creating" } else { "Create order" }}
                </button>
                <button class="button secondary-action" type="button" on:click=move |_| on_close.run(())>
                    "Cancel"
                </button>
            </div>
        </form>
    }
}

#[component]
fn OrderDetailPanel(
    order: Order,
    facilities: Vec<AccessScopeResource>,
    pending: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let tab = RwSignal::new(OrderDetailTab::Header);
    let order_id = order.id;
    let cancellation_order_key = StoredValue::new(order.order_key.clone());
    let order_key = RwSignal::new(order.order_key.clone());
    let rush = RwSignal::new(order.rush);
    let ship_by = RwSignal::new(timestamp_input(order.ship_by));
    let line1 = RwSignal::new(order.line1.clone().unwrap_or_default());
    let line2 = RwSignal::new(order.line2.clone().unwrap_or_default());
    let city = RwSignal::new(order.city.clone().unwrap_or_default());
    let state = RwSignal::new(order.state.clone().unwrap_or_default());
    let postal_code = RwSignal::new(order.postal_code.clone().unwrap_or_default());
    let country = RwSignal::new(order.country.clone().unwrap_or_default());
    let command_pending = RwSignal::new(false);
    let command_error = RwSignal::new(None::<String>);
    let cancel_open = RwSignal::new(false);
    let hold_open = RwSignal::new(false);
    let hold_reason = RwSignal::new("customer_request".to_owned());
    let hold_note = RwSignal::new(String::new());
    let release_candidate = RwSignal::new(None::<i64>);
    let release_note = RwSignal::new(String::new());
    let reservation_item_names = StoredValue::new(
        order
            .order_items
            .iter()
            .filter_map(|line| {
                line.item_description
                    .clone()
                    .map(|description| (line.item_id, description))
            })
            .collect::<Vec<_>>(),
    );
    let facility_names = StoredValue::new(
        facilities
            .iter()
            .map(|facility| (facility.id, facility.name.clone()))
            .collect::<Vec<_>>(),
    );
    let facilities = StoredValue::new(facilities);
    let toasts = use_toast_bus();

    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if command_pending.get_untracked() {
            return;
        }
        let key_value = order_key.get_untracked().trim().to_owned();
        if key_value.is_empty() {
            command_error.set(Some("Order number cannot be blank.".to_owned()));
            return;
        }
        let ship_by_value = match parse_optional_timestamp(&ship_by.get_untracked()) {
            Ok(value) => value,
            Err(message) => {
                command_error.set(Some(message));
                return;
            }
        };
        let request = OrderUpdate {
            order_id,
            order_key: Some(key_value.clone()),
            rush: Some(rush.get_untracked()),
            ship_by: ship_by_value,
            line1: Some(line1.get_untracked().trim().to_owned()),
            line2: Some(line2.get_untracked().trim().to_owned()),
            city: Some(city.get_untracked().trim().to_owned()),
            state: Some(state.get_untracked().trim().to_owned()),
            postal_code: Some(postal_code.get_untracked().trim().to_owned()),
            country: Some(country.get_untracked().trim().to_owned()),
        };
        command_pending.set(true);
        command_error.set(None);
        let idempotency_key = api::new_idempotency_key();
        leptos::task::spawn_local(async move {
            match api::internal_post_idempotent::<_, bool>(
                "/api/orders/update",
                &request,
                &idempotency_key,
            )
            .await
            {
                Ok(true) => {
                    command_pending.set(false);
                    toasts.success(format!("Order {key_value} updated."));
                    on_refreshed.run(order_id);
                }
                Ok(false) => {
                    command_error.set(Some(
                        "The order could not be updated in its current state.".to_owned(),
                    ));
                    command_pending.set(false);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    command_error.set(Some(api_error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    let place_hold = move |_| {
        if command_pending.get_untracked() {
            return;
        }
        let Some(reason) = parse_order_hold_reason(&hold_reason.get_untracked()) else {
            command_error.set(Some("Choose a valid hold reason.".to_owned()));
            return;
        };
        let note = optional_text(&hold_note.get_untracked());
        if reason == OrderHoldRequestReason::Other && note.is_none() {
            command_error.set(Some("Add a note for an Other hold.".to_owned()));
            return;
        }
        let request = PlaceOrderHoldRequest { reason, note };
        command_pending.set(true);
        command_error.set(None);
        let idempotency_key = api::new_idempotency_key();
        leptos::task::spawn_local(async move {
            match api::place_order_hold(order_id, &request, &idempotency_key).await {
                Ok(result) => {
                    hold_open.set(false);
                    hold_note.set(String::new());
                    command_pending.set(false);
                    toasts.success(format!(
                        "Order hold #{} placed. {} active hold(s).",
                        result.hold_id, result.active_hold_count
                    ));
                    on_refreshed.run(order_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    command_error.set(Some(api_error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    let release_hold = move |_| {
        if command_pending.get_untracked() {
            return;
        }
        let Some(hold_id) = release_candidate.get_untracked() else {
            return;
        };
        let request = ReleaseOrderHoldRequest {
            note: optional_text(&release_note.get_untracked()),
        };
        command_pending.set(true);
        command_error.set(None);
        let idempotency_key = api::new_idempotency_key();
        leptos::task::spawn_local(async move {
            match api::release_order_hold(order_id, hold_id, &request, &idempotency_key).await {
                Ok(result) => {
                    release_candidate.set(None);
                    release_note.set(String::new());
                    command_pending.set(false);
                    toasts.success(if result.active_hold_count == 0 {
                        "The last order hold was released; the order is open.".to_owned()
                    } else {
                        format!(
                            "Hold #{hold_id} released. {} active hold(s) remain.",
                            result.active_hold_count
                        )
                    });
                    on_refreshed.run(order_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    command_error.set(Some(api_error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    view! {
        <div class="fulfillment-detail-content">
            <div class="detail-heading">
                <div>
                    <span class="eyebrow">{format!("Order #{}", order.id)}</span>
                    <h2>{order.order_key.clone()}</h2>
                </div>
                <span class=order_status_class(order.status)>{title_case(order.status.as_str())}</span>
            </div>
            <dl class="detail-facts four-column">
                <div>
                    <dt>"Client"</dt>
                    <dd>{order.inventory_owner_name.clone().unwrap_or_else(|| "Unassigned".to_owned())}</dd>
                </div>
                <div>
                    <dt>"Ordered"</dt>
                    <dd>{format_quantity(order.ordered_qty)}</dd>
                </div>
                <div>
                    <dt>"Reserved"</dt>
                    <dd>{format_quantity(order.reserved_qty)}</dd>
                </div>
                <div>
                    <dt>"Created"</dt>
                    <dd>{short_timestamp(order.created)}</dd>
                </div>
            </dl>
            <div class="detail-tabs" role="tablist" aria-label="Order detail sections">
                {[
                    (OrderDetailTab::Header, "Header"),
                    (OrderDetailTab::Lines, "Lines"),
                    (OrderDetailTab::Fulfillment, "Fulfillment"),
                    (OrderDetailTab::Holds, "Holds"),
                    (OrderDetailTab::Activity, "Activity"),
                ]
                    .into_iter()
                    .map(|(value, label)| {
                        view! {
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || (tab.get() == value).to_string()
                                class:active=move || tab.get() == value
                                on:click=move |_| tab.set(value)
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>

            <Show when=move || pending.get()>
                <div class="detail-loading" role="status">"Refreshing order..."</div>
            </Show>
            <Show when=move || load_error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || load_error.get().unwrap_or_default()}</p>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Header>
                <form class="fulfillment-form detail-form" on:submit=save>
                    <div class="form-grid two-column">
                        <label>
                            <span>"Order number"</span>
                            <input
                                required
                                prop:value=move || order_key.get()
                                on:input=move |event| order_key.set(event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span>"Ship by (UTC)"</span>
                            <input
                                type="datetime-local"
                                prop:value=move || ship_by.get()
                                on:input=move |event| ship_by.set(event_target_value(&event))
                            />
                        </label>
                        <label class="checkbox-label">
                            <input
                                type="checkbox"
                                prop:checked=move || rush.get()
                                on:change=move |event| rush.set(event_target_checked(&event))
                            />
                            <span>"Rush order"</span>
                        </label>
                    </div>
                    <fieldset>
                        <legend>"Ship to"</legend>
                        <div class="form-grid two-column">
                            <label class="wide">
                                <span>"Address line 1"</span>
                                <input
                                    prop:value=move || line1.get()
                                    on:input=move |event| line1.set(event_target_value(&event))
                                />
                            </label>
                            <label class="wide">
                                <span>"Address line 2"</span>
                                <input
                                    prop:value=move || line2.get()
                                    on:input=move |event| line2.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"City"</span>
                                <input
                                    prop:value=move || city.get()
                                    on:input=move |event| city.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"State / region"</span>
                                <input
                                    prop:value=move || state.get()
                                    on:input=move |event| state.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"Postal code"</span>
                                <input
                                    prop:value=move || postal_code.get()
                                    on:input=move |event| postal_code.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"Country"</span>
                                <input
                                    prop:value=move || country.get()
                                    on:input=move |event| country.set(event_target_value(&event))
                                />
                            </label>
                        </div>
                    </fieldset>
                    <Show when=move || command_error.get().is_some()>
                        <p class="inline-command-error" role="alert">
                            {move || command_error.get().unwrap_or_default()}
                        </p>
                    </Show>
                    <div class="form-actions">
                        <button class="button primary-action" type="submit" disabled=move || command_pending.get()>
                            {move || if command_pending.get() { "Saving" } else { "Save header" }}
                        </button>
                        {can_place_order_hold(order.status).then(|| {
                            view! {
                                <button
                                    class="button secondary-action"
                                    type="button"
                                    on:click=move |_| {
                                        cancel_open.set(false);
                                        hold_open.set(true);
                                    }
                                >
                                    <Icon icon=UiIcon::Holds/>
                                    "Place hold"
                                </button>
                            }
                        })}
                        {can_cancel_order(order.status).then(|| {
                            view! {
                                <button
                                    class="button danger-action"
                                    type="button"
                                    on:click=move |_| {
                                        hold_open.set(false);
                                        cancel_open.set(true);
                                    }
                                >
                                    <Icon icon=UiIcon::Alert/>
                                    "Cancel order"
                                </button>
                            }
                        })}
                    </div>
                    <Show when=move || hold_open.get()>
                        <section class="confirmation-panel order-hold-panel" role="dialog" aria-labelledby="place-order-hold-title">
                            <h3 id="place-order-hold-title">"Place order hold"</h3>
                            <p>"Block release and execution until every active hold is cleared."</p>
                            <label>
                                <span>"Reason"</span>
                                <select
                                    prop:value=move || hold_reason.get()
                                    on:change=move |event| hold_reason.set(event_target_value(&event))
                                >
                                    <option value="address_review">"Address review"</option>
                                    <option value="compliance_review">"Compliance review"</option>
                                    <option value="customer_request">"Client request"</option>
                                    <option value="inventory_shortage">"Inventory shortage"</option>
                                    <option value="payment_review">"Payment review"</option>
                                    <option value="other">"Other"</option>
                                </select>
                            </label>
                            <label>
                                <span>"Note"</span>
                                <textarea
                                    maxlength="1000"
                                    rows="3"
                                    prop:value=move || hold_note.get()
                                    on:input=move |event| hold_note.set(event_target_value(&event))
                                ></textarea>
                            </label>
                            <div class="form-actions">
                                <button
                                    type="button"
                                    class="button primary-action"
                                    disabled=move || command_pending.get()
                                    on:click=place_hold
                                >
                                    <Icon icon=UiIcon::Holds/>
                                    {move || if command_pending.get() { "Placing" } else { "Place hold" }}
                                </button>
                                <button
                                    type="button"
                                    class="button secondary-action"
                                    on:click=move |_| hold_open.set(false)
                                >
                                    "Close"
                                </button>
                            </div>
                        </section>
                    </Show>
                    <Show when=move || cancel_open.get()>
                        <OrderCancellationPanel
                            order_id
                            order_key=cancellation_order_key.get_value()
                            revision=order.revision
                            on_close=Callback::new(move |_| cancel_open.set(false))
                            on_refreshed
                            on_unauthorized
                        />
                    </Show>
                </form>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Lines>
                <div class="detail-section">
                    <div class="detail-section-title">
                        <h3>"Demand lines"</h3>
                        <span>{format!("{} lines", order.order_items.len())}</span>
                    </div>
                    <div class="table-scroll">
                        <table class="data-table detail-table order-demand-lines-table">
                            <thead>
                                <tr>
                                    <th>"Line"</th><th>"Item"</th><th>"Description"</th>
                                    <th>"UOM"</th><th class="numeric">"Quantity"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {order
                                    .order_items
                                    .clone()
                                    .into_iter()
                                    .map(|line| {
                                        view! {
                                            <tr>
                                                <td>
                                                    <strong>{line.line_key}</strong>
                                                    <small class="cell-detail">
                                                        {format!("Line {}", line.line_number)}
                                                    </small>
                                                </td>
                                                <td>{format!("#{}", line.item_id)}</td>
                                                <td>{line.item_description.unwrap_or_else(|| "-".to_owned())}</td>
                                                <td>{line.uom}</td>
                                                <td class="numeric strong">{format_quantity(line.qty)}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                        {order.order_items.is_empty().then(|| {
                            view! { <p class="empty-state">"No demand lines are attached to this order."</p> }
                        })}
                    </div>
                </div>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Fulfillment>
                <div class="detail-section-stack">
                    <OrderAllocationPanel
                        order_id
                        facilities=facilities.get_value()
                        on_refreshed
                        on_unauthorized
                    />
                    <section class="detail-section">
                        <div class="detail-section-title">
                            <h3>"Reservations"</h3>
                            <span>{format!("{} records", order.reservations.len())}</span>
                        </div>
                        <div class="table-scroll">
                            <table class="data-table detail-table">
                                <thead>
                                    <tr>
                                        <th>"Item"</th><th>"Facility"</th><th>"UOM"</th><th>"State"</th>
                                        <th class="numeric">"Reserved"</th><th class="numeric">"Allocated"</th>
                                        <th>"Created"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {order
                                        .reservations
                                        .clone()
                                        .into_iter()
                                        .map(|reservation| {
                                            let item_label = reservation_item_names.with_value(|names| {
                                                names
                                                    .iter()
                                                    .find(|(id, _)| *id == reservation.item_id)
                                                    .map(|(_, name)| name.clone())
                                                    .unwrap_or_else(|| format!("Item #{}", reservation.item_id))
                                            });
                                            let facility_label = facility_names.with_value(|names| {
                                                names
                                                    .iter()
                                                    .find(|(id, _)| *id == reservation.facility_id)
                                                    .map(|(_, name)| name.clone())
                                                    .unwrap_or_else(|| {
                                                        format!("Facility #{}", reservation.facility_id)
                                                    })
                                            });
                                            view! {
                                                <tr>
                                                    <td>
                                                        <strong>{item_label}</strong>
                                                        <small class="cell-detail">{format!("#{}", reservation.item_id)}</small>
                                                    </td>
                                                    <td>{facility_label}</td>
                                                    <td>{reservation.uom}</td>
                                                    <td>{title_case(reservation.status.as_str())}</td>
                                                    <td class="numeric">{format_quantity(reservation.qty)}</td>
                                                    <td class="numeric">{format_quantity(reservation.allocated_qty)}</td>
                                                    <td>{short_timestamp(reservation.created)}</td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            </table>
                            {order.reservations.is_empty().then(|| {
                                view! { <p class="empty-state">"No stock is currently reserved."</p> }
                            })}
                        </div>
                    </section>
                    <section class="detail-section">
                        <div class="detail-section-title">
                            <h3>"Tracking"</h3>
                            <span>{format!("{} numbers", order.tracking_numbers.len())}</span>
                        </div>
                        <div class="tracking-list">
                            {order
                                .tracking_numbers
                                .clone()
                                .into_iter()
                                .map(|tracking| {
                                    view! {
                                        <div class="tracking-row">
                                            <strong>{tracking.tracking_number}</strong>
                                            <span>{tracking.carrier.unwrap_or_else(|| "Carrier not set".to_owned())}</span>
                                            <span>{tracking.service.unwrap_or_else(|| "Service not set".to_owned())}</span>
                                        </div>
                                    }
                                })
                                .collect_view()}
                            {order.tracking_numbers.is_empty().then(|| {
                                view! { <p class="empty-state">"No tracking numbers have been recorded."</p> }
                            })}
                        </div>
                    </section>
                </div>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Holds>
                <div class="detail-section-stack">
                    <section class="detail-section">
                        <div class="detail-section-title">
                            <h3>"Order holds"</h3>
                            <span>{format!(
                                "{} active / {} total",
                                order.holds.iter().filter(|hold| hold.is_active()).count(),
                                order.holds.len()
                            )}</span>
                        </div>
                        <div class="table-scroll">
                            <table class="data-table detail-table order-holds-table">
                                <thead>
                                    <tr>
                                        <th>"Hold"</th><th>"Placed"</th><th>"State"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {order
                                        .holds
                                        .clone()
                                        .into_iter()
                                        .map(|hold| {
                                            let active = hold.is_active();
                                            let hold_detail = hold
                                                .note
                                                .as_deref()
                                                .map_or_else(
                                                    || format!("#{}", hold.id),
                                                    |note| format!("#{} - {note}", hold.id),
                                                );
                                            let hold_detail_title = hold_detail.clone();
                                            let released_detail = hold.released_at.map(|released_at| {
                                                format!(
                                                    "{} - User #{}",
                                                    short_timestamp(released_at),
                                                    hold.released_by_user_id.unwrap_or_default()
                                                )
                                            });
                                            view! {
                                                <tr>
                                                    <td>
                                                        <strong>{order_hold_reason_label(hold.reason.as_str())}</strong>
                                                        <small class="cell-detail" title=hold_detail_title>{hold_detail}</small>
                                                    </td>
                                                    <td>
                                                        <strong>{short_timestamp(hold.created)}</strong>
                                                        <small class="cell-detail">{format!("User #{}", hold.created_by_user_id)}</small>
                                                    </td>
                                                    <td>
                                                        <div class="order-hold-state-line">
                                                            <span class=if active { "status held" } else { "status muted" }>
                                                                {if active { "Active" } else { "Released" }}
                                                            </span>
                                                            {active.then(|| {
                                                                let hold_id = hold.id;
                                                                view! {
                                                                    <button
                                                                        type="button"
                                                                        class="button table-action order-hold-release-action"
                                                                        title="Release order hold"
                                                                        aria-label="Release order hold"
                                                                        on:click=move |_| {
                                                                            release_note.set(String::new());
                                                                            release_candidate.set(Some(hold_id));
                                                                        }
                                                                    >
                                                                        <Icon icon=UiIcon::Unlock/>
                                                                    </button>
                                                                }
                                                            })}
                                                        </div>
                                                        {released_detail.map(|detail| {
                                                            let title = detail.clone();
                                                            view! { <small class="cell-detail" title=title>{detail}</small> }
                                                        })}
                                                        {hold.release_note.map(|note| {
                                                            let title = note.clone();
                                                            view! { <small class="cell-detail" title=title>{note}</small> }
                                                        })}
                                                    </td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            </table>
                            {order.holds.is_empty().then(|| {
                                view! { <p class="empty-state">"No order holds have been recorded."</p> }
                            })}
                        </div>
                    </section>
                    <Show when=move || release_candidate.get().is_some()>
                        <section class="confirmation-panel release-hold-panel" role="alertdialog" aria-labelledby="release-order-hold-title">
                            <h3 id="release-order-hold-title">{move || format!(
                                "Release hold #{}?",
                                release_candidate.get().unwrap_or_default()
                            )}</h3>
                            <p>"The order stays blocked when another active hold remains."</p>
                            <label>
                                <span>"Release note"</span>
                                <textarea
                                    maxlength="1000"
                                    rows="3"
                                    prop:value=move || release_note.get()
                                    on:input=move |event| release_note.set(event_target_value(&event))
                                ></textarea>
                            </label>
                            <div class="form-actions">
                                <button
                                    type="button"
                                    class="button primary-action"
                                    disabled=move || command_pending.get()
                                    on:click=release_hold
                                >
                                    <Icon icon=UiIcon::Unlock/>
                                    {move || if command_pending.get() { "Releasing" } else { "Release hold" }}
                                </button>
                                <button
                                    type="button"
                                    class="button secondary-action"
                                    on:click=move |_| release_candidate.set(None)
                                >
                                    "Keep hold"
                                </button>
                            </div>
                        </section>
                    </Show>
                </div>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Activity>
                <div class="detail-section">
                    <div class="detail-section-title">
                        <h3>"Order activity"</h3>
                        <span>{format!("{} events", order.activity.len())}</span>
                    </div>
                    <ol class="activity-list">
                        {order
                            .activity
                            .clone()
                            .into_iter()
                            .rev()
                            .map(|activity| {
                                view! {
                                    <li>
                                        <span>{short_timestamp(activity.created)}</span>
                                        <strong>{title_case(&activity.action)}</strong>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ol>
                    {order.activity.is_empty().then(|| {
                        view! { <p class="empty-state">"No activity has been recorded."</p> }
                    })}
                </div>
            </Show>
        </div>
    }
}

async fn fetch_orders(offset: i64, search: &str, status: &str) -> Result<OrderPage, api::ApiError> {
    let mut path = format!("/api/orders?limit={ORDER_PAGE_SIZE}&offset={offset}");
    let search = search.trim();
    if !search.is_empty() {
        path.push_str("&search=");
        path.push_str(&query_encode(search));
    }
    let status = status.trim();
    if !status.is_empty() {
        path.push_str("&status=");
        path.push_str(&query_encode(status));
    }
    api::internal_get(&path).await
}

fn request_order_detail(
    order_id: i64,
    selected: RwSignal<Option<Order>>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    pending.set(true);
    error.set(None);
    leptos::task::spawn_local(async move {
        match api::internal_get::<Option<Order>>(&format!("/api/orders/{order_id}")).await {
            Ok(Some(order)) => {
                selected.set(Some(order));
                pending.set(false);
            }
            Ok(None) => {
                selected.set(None);
                error.set(Some(
                    "Order not found or outside your client scope.".to_owned(),
                ));
                pending.set(false);
            }
            Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
            Err(api_error) => {
                error.set(Some(api_error.message));
                pending.set(false);
            }
        }
    });
}

fn can_cancel_order(status: OrderStatus) -> bool {
    matches!(status, OrderStatus::Open | OrderStatus::Held)
}

fn can_place_order_hold(status: OrderStatus) -> bool {
    matches!(status, OrderStatus::Open | OrderStatus::Held)
}

fn parse_order_hold_reason(value: &str) -> Option<OrderHoldRequestReason> {
    match value {
        "address_review" => Some(OrderHoldRequestReason::AddressReview),
        "compliance_review" => Some(OrderHoldRequestReason::ComplianceReview),
        "customer_request" => Some(OrderHoldRequestReason::CustomerRequest),
        "inventory_shortage" => Some(OrderHoldRequestReason::InventoryShortage),
        "payment_review" => Some(OrderHoldRequestReason::PaymentReview),
        "other" => Some(OrderHoldRequestReason::Other),
        _ => None,
    }
}

fn order_hold_reason_label(value: &str) -> &'static str {
    match value {
        "address_review" => "Address review",
        "compliance_review" => "Compliance review",
        "customer_request" => "Client request",
        "inventory_shortage" => "Inventory shortage",
        "payment_review" => "Payment review",
        "other" => "Other",
        _ => "Unknown",
    }
}

fn title_case(value: &str) -> String {
    value
        .split(['_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_not_offered_for_terminal_orders() {
        assert!(can_cancel_order(OrderStatus::Open));
        assert!(can_cancel_order(OrderStatus::Held));
        assert!(!can_cancel_order(OrderStatus::Processing));
        assert!(!can_cancel_order(OrderStatus::AwaitingShipment));
        assert!(!can_cancel_order(OrderStatus::Shipped));
        assert!(!can_cancel_order(OrderStatus::Cancelled));
        assert!(!can_cancel_order(OrderStatus::Void));
    }

    #[test]
    fn labels_replace_wire_separators() {
        assert_eq!(title_case("awaiting shipment"), "Awaiting Shipment");
        assert_eq!(title_case("quality_hold"), "Quality Hold");
    }
}
