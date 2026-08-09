use leptos::prelude::*;
use wareboxes_api_contract::v1::ShipmentStatus;

use super::{outbound_qa::outbound_qa_ready, QueueSignals};

#[component]
pub(super) fn ShippingQueue(
    queue: QueueSignals,
    selected_order_id: RwSignal<Option<i64>>,
    on_select: Callback<i64>,
    on_load_more: Callback<()>,
) -> impl IntoView {
    view! {
        <aside class="shipping-queue" aria-label="Shipping queue">
            <header>
                <div>
                    <h2>"Ready orders"</h2>
                    <span>{move || format!("{} visible", queue.entries.get().len())}</span>
                </div>
                <Show when=move || queue.pending.get()>
                    <span class="status pending">"Refreshing"</span>
                </Show>
            </header>
            <Show when=move || queue.error.get().is_some()>
                <p class="shipping-queue-error" role="alert">
                    {move || queue.error.get().unwrap_or_default()}
                </p>
            </Show>
            <div class="shipping-queue-list">
                <For
                    each=move || queue.entries.get()
                    key=|entry| (
                        entry.order_id,
                        entry.order_revision.get(),
                        entry.facility_revision.get(),
                        entry
                            .shipment
                            .as_ref()
                            .map_or(0, |shipment| shipment.revision.get()),
                        entry
                            .outbound_qa_policy
                            .as_ref()
                            .map_or(0, |policy| policy.revision.get()),
                        entry
                            .outbound_qa_session
                            .as_ref()
                            .map_or(0, |session| session.revision.get()),
                    )
                    children=move |entry| {
                        let order_id = entry.order_id;
                        let state = entry.shipment.as_ref().map_or_else(
                            || "Ready".to_owned(),
                            |shipment| match shipment.status {
                                ShipmentStatus::AwaitingManifest => "Needs manifest".to_owned(),
                                ShipmentStatus::Manifested => "Ready to depart".to_owned(),
                                ShipmentStatus::PartiallyDeparted => format!(
                                    "{} / {} departed",
                                    shipment.departed_carton_count,
                                    shipment.carton_count,
                                ),
                                ShipmentStatus::Departed => "Departed".to_owned(),
                                ShipmentStatus::Cancelled => "Cancelled".to_owned(),
                            },
                        );
                        let blocker = if !entry.origin_ready {
                            Some("Origin missing")
                        } else if !entry.destination_ready {
                            Some("Ship-to incomplete")
                        } else if entry.shipment.is_none() && !outbound_qa_ready(&entry) {
                            Some("QA required")
                        } else {
                            None
                        };
                        view! {
                            <button
                                type="button"
                                class="shipping-queue-row"
                                class:selected=move || selected_order_id.get() == Some(order_id)
                                on:click=move |_| on_select.run(order_id)
                            >
                                <span class="shipping-queue-primary">
                                    <strong>{entry.order_key}</strong>
                                    <small>{format!(
                                        "{} · {}",
                                        entry.inventory_owner_name,
                                        entry.facility_name,
                                    )}</small>
                                </span>
                                <span class="shipping-queue-state">
                                    <span class="status">{state}</span>
                                    {entry.rush.then(|| view! {
                                        <span class="status danger">"Rush"</span>
                                    })}
                                    {blocker.map(|label| view! {
                                        <span class="status warning">{label}</span>
                                    })}
                                </span>
                            </button>
                        }
                    }
                />
                <Show when=move || queue.entries.get().is_empty() && !queue.pending.get()>
                    <p class="shipping-empty">"No packed orders are ready in this scope."</p>
                </Show>
            </div>
            <Show when=move || queue.next_cursor.get().is_some()>
                <button
                    type="button"
                    class="button secondary-action shipping-load-more"
                    disabled=move || queue.pending.get()
                    on:click=move |_| on_load_more.run(())
                >
                    "Load more"
                </button>
            </Show>
        </aside>
    }
}
