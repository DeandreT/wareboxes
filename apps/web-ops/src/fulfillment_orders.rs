use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use std::time::Duration;
use wareboxes_api_contract::v1::{
    AmendFulfillmentOrderRequest, CreateFulfillmentOrderLineRequest, CreateFulfillmentOrderRequest,
    FulfillmentOrderDestination, OrderEntryItemResponse, OrderHoldReason as OrderHoldRequestReason,
    PlaceOrderHoldRequest, ReleaseOrderHoldRequest, Revision,
};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};
use wareboxes_core::dto::OrderPage;
use wareboxes_core::models::{Location, Order, OrderStatus};

use crate::api;
use crate::components::{Icon, SearchField, UiIcon};
use crate::fulfillment_order_allocation::OrderAllocationPanel;
use crate::fulfillment_order_cancellation::OrderCancellationPanel;
use crate::fulfillment_pick_shortages::PickShortageWorkbench;
use crate::fulfillment_shared::{
    optional_text, order_destination, order_status_class, parse_optional_timestamp, query_encode,
    short_timestamp, timestamp_input,
};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

mod detail;

use detail::{title_case, OrderDetailPanel};

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

impl OrderSort {
    const fn wire_value(self) -> &'static str {
        match self {
            Self::Order => "order",
            Self::Client => "client",
            Self::Status => "status",
            Self::Units => "units",
            Self::ShipBy => "ship_by",
            Self::Destination => "destination",
        }
    }
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
    locations: Vec<Location>,
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
    let shortages_open = RwSignal::new(false);
    let sort = RwSignal::new(SortSpec {
        key: OrderSort::Order,
        direction: SortDirection::Descending,
    });
    let layout = SplitPaneState::new("orders", 720);
    let toasts = use_toast_bus();
    let shortage_facilities = StoredValue::new(access.facilities.clone());
    let shortage_clients = StoredValue::new(access.inventory_owners.clone());
    let detail_facilities = StoredValue::new(access.facilities);
    let detail_locations = StoredValue::new(locations);
    let create_clients = StoredValue::new(access.inventory_owners);

    let run_search = move |offset: i64| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        error.set(None);
        let search_value = search.get_untracked();
        let status_value = status.get_untracked();
        let sort_value = sort.get_untracked();
        leptos::task::spawn_local(async move {
            match fetch_orders(offset, &search_value, &status_value, sort_value).await {
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

    let change_sort = Callback::new(move |key: OrderSort| {
        if pending.get_untracked() {
            return;
        }
        SortSpec::select(sort, key);
        run_search(0);
    });

    let submit_search = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        run_search(0);
    };

    let open_order = move |order_id: i64| {
        layout.show_detail();
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
        let sort_value = sort.get_untracked();
        let offset = page.get_untracked().page.offset;
        leptos::task::spawn_local(async move {
            match fetch_orders(offset, &search_value, &status_value, sort_value).await {
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
        <Show
            when=move || shortages_open.get()
            fallback=move || view! {
        <div
            class="fulfillment-workbench split-workspace"
            class:create-mode=move || create_open.get()
            style=move || layout.style()
            data-pane-mode=move || layout.mode_attribute()
        >
            <section class="data-section fulfillment-list split-master">
                <form class="table-toolbar fulfillment-toolbar" on:submit=submit_search>
                    <div class="toolbar-summary">
                        <strong>{move || format_quantity(page.get().page.total)}</strong>
                        <span>"orders"</span>
                        <PaneControls layout master_label="order list" detail_label="order detail"/>
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
                            class="button secondary-action"
                            type="button"
                            on:click=move |_| {
                                create_open.set(false);
                                shortages_open.set(true);
                            }
                        >
                            <Icon icon=UiIcon::Alert/>
                            "Pick shortages"
                        </button>
                        <button
                            class="button primary-action"
                            type="button"
                            on:click=move |_| {
                                layout.show_detail();
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
                                    on_sort=Callback::new(move |_| change_sort.run(OrderSort::Order))
                                />
                                <SortableHeader
                                    label="Client"
                                    active=move || sort.get().key == OrderSort::Client
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| change_sort.run(OrderSort::Client))
                                />
                                <SortableHeader
                                    label="Status"
                                    active=move || sort.get().key == OrderSort::Status
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| change_sort.run(OrderSort::Status))
                                />
                                <SortableHeader
                                    label="Units"
                                    active=move || sort.get().key == OrderSort::Units
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| change_sort.run(OrderSort::Units))
                                    numeric=true
                                />
                                <SortableHeader
                                    label="Ship by"
                                    active=move || sort.get().key == OrderSort::ShipBy
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| change_sort.run(OrderSort::ShipBy))
                                />
                                <SortableHeader
                                    label="Destination"
                                    active=move || sort.get().key == OrderSort::Destination
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        change_sort.run(OrderSort::Destination)
                                    })
                                />
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let selected_id = selected.get().map(|order| order.id);
                                page.get().page.items
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

            <SplitPaneHandle layout/>

            <aside class="command-panel fulfillment-detail split-detail">
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
                                                    locations=detail_locations.get_value()
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
        >
            <PickShortageWorkbench
                facilities=shortage_facilities.get_value()
                inventory_owners=shortage_clients.get_value()
                on_close=Callback::new(move |_| shortages_open.set(false))
                on_unauthorized
            />
        </Show>
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
    let recipient_name = RwSignal::new(String::new());
    let company = RwSignal::new(String::new());
    let phone = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
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
        let recipient_name_value = recipient_name.get_untracked().trim().to_owned();
        let line1_value = line1.get_untracked().trim().to_owned();
        let city_value = city.get_untracked().trim().to_owned();
        let state_value = state.get_untracked().trim().to_owned();
        let postal_value = postal_code.get_untracked().trim().to_owned();
        let country_value = country.get_untracked().trim().to_owned();
        if [
            recipient_name_value.as_str(),
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
                "Recipient, address, city, state, postal code, and country are required."
                    .to_owned(),
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
                recipient_name: recipient_name_value,
                company: optional_text(&company.get_untracked()),
                phone: optional_text(&phone.get_untracked()),
                email: optional_text(&email.get_untracked()),
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
                    <label>
                        <span>"Recipient name"</span>
                        <input
                            required
                            autocomplete="name"
                            prop:value=move || recipient_name.get()
                            on:input=move |event| recipient_name.set(event_target_value(&event))
                        />
                    </label>
                    <label>
                        <span>"Company"</span>
                        <input
                            autocomplete="organization"
                            prop:value=move || company.get()
                            on:input=move |event| company.set(event_target_value(&event))
                        />
                    </label>
                    <label>
                        <span>"Phone"</span>
                        <input
                            type="tel"
                            autocomplete="tel"
                            prop:value=move || phone.get()
                            on:input=move |event| phone.set(event_target_value(&event))
                        />
                    </label>
                    <label>
                        <span>"Email"</span>
                        <input
                            type="email"
                            autocomplete="email"
                            prop:value=move || email.get()
                            on:input=move |event| email.set(event_target_value(&event))
                        />
                    </label>
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

async fn fetch_orders(
    offset: i64,
    search: &str,
    status: &str,
    sort: SortSpec<OrderSort>,
) -> Result<OrderPage, api::ApiError> {
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
    path.push_str("&sort=");
    path.push_str(sort.key.wire_value());
    path.push_str("&direction=");
    path.push_str(match sort.direction {
        SortDirection::Ascending => "asc",
        SortDirection::Descending => "desc",
    });
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
