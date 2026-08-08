use leptos::prelude::*;
use wareboxes_core::models::Order;

use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::view_model::format_quantity;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OrderSort {
    Order,
    Client,
    Status,
    Units,
    Destination,
}

#[component]
pub fn OrderTable(orders: Vec<Order>, compact: bool) -> impl IntoView {
    let empty = orders.is_empty();
    let sort = RwSignal::new(SortSpec {
        key: OrderSort::Order,
        direction: SortDirection::Descending,
    });
    view! {
        <div class="table-scroll">
            <table class="data-table">
                <caption class="sr-only">"Orders in the current organization and client scope"</caption>
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
                        let mut sorted = orders.clone();
                        sorted.sort_by(|left, right| {
                            let ordering = match spec.key {
                                OrderSort::Order => left
                                    .order_key
                                    .to_ascii_lowercase()
                                    .cmp(&right.order_key.to_ascii_lowercase()),
                                OrderSort::Client => left
                                    .inventory_owner_name
                                    .as_deref()
                                    .unwrap_or_default()
                                    .to_ascii_lowercase()
                                    .cmp(
                                        &right
                                            .inventory_owner_name
                                            .as_deref()
                                            .unwrap_or_default()
                                            .to_ascii_lowercase(),
                                    ),
                                OrderSort::Status => left.status.as_str().cmp(right.status.as_str()),
                                OrderSort::Units => left.ordered_qty.cmp(&right.ordered_qty),
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
                        sorted
                            .into_iter()
                            .map(|order| {
                                let destination = order_destination(&order);
                                view! {
                                    <tr>
                                        <td>
                                            <strong>{order.order_key}</strong>
                                            {order.rush.then(|| view! { <small class="rush">"Rush"</small> })}
                                        </td>
                                        <td>{order.inventory_owner_name.unwrap_or_else(|| "Unassigned".to_owned())}</td>
                                        <td>
                                            <span class=status_class(order.status.as_str())>
                                                {order.status.to_string()}
                                            </span>
                                        </td>
                                        <td class="numeric">{format_quantity(order.ordered_qty)}</td>
                                        <td>{destination}</td>
                                    </tr>
                                }
                            })
                            .collect_view()
                    }}
                </tbody>
            </table>
            {empty.then(|| view! { <p class="empty-state">"No orders are currently in scope."</p> })}
            {compact.then(|| view! { <span class="compact-table-edge" aria-hidden="true"></span> })}
        </div>
    }
}

fn order_destination(order: &Order) -> String {
    let destination = [order.city.as_deref(), order.state.as_deref()]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if destination.is_empty() {
        "Not assigned".to_owned()
    } else {
        destination
    }
}

fn status_class(status: &str) -> &'static str {
    match status {
        "shipped" => "status shipped",
        "cancelled" | "void" => "status muted",
        "held" => "status held",
        "processing" | "awaiting packing" | "packing" | "awaiting shipment" => "status processing",
        _ => "status open",
    }
}
