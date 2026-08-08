use leptos::prelude::*;
use wareboxes_api_contract::v1::{CancelOrderRequest, OrderCancellationReason, Revision};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

type CancellationRetry = (CancelOrderRequest, String);

#[component]
pub(super) fn OrderCancellationPanel(
    order_id: i64,
    order_key: String,
    revision: i64,
    on_close: Callback<()>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new("client_request".to_owned());
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry_attempt = RwSignal::new(None::<CancellationRetry>);
    let order_key = StoredValue::new(order_key);
    let toasts = use_toast_bus();

    let cancel = move |_| {
        if pending.get_untracked() {
            return;
        }
        let (request, idempotency_key) = if let Some(attempt) = retry_attempt.get_untracked() {
            attempt
        } else {
            let Some(reason_value) = parse_reason(&reason.get_untracked()) else {
                error.set(Some("Choose a valid cancellation reason.".to_owned()));
                return;
            };
            let note_value = optional_note(&note.get_untracked());
            if reason_value == OrderCancellationReason::Other && note_value.is_none() {
                error.set(Some(
                    "Add a note when the cancellation reason is Other.".to_owned(),
                ));
                return;
            }
            let Ok(expected_revision) = Revision::new(revision) else {
                error.set(Some(
                    "The order revision is invalid. Refresh the order.".to_owned(),
                ));
                return;
            };
            (
                CancelOrderRequest {
                    expected_revision,
                    reason: reason_value,
                    note: note_value,
                },
                api::new_idempotency_key(),
            )
        };

        retry_attempt.set(Some((request.clone(), idempotency_key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::cancel_order(order_id, &request, &idempotency_key).await {
                Ok(result) => {
                    retry_attempt.set(None);
                    pending.set(false);
                    let key = order_key.get_value();
                    toasts.success(format!(
                        "Order {key} cancelled. {} units released; revision {}.",
                        format_quantity(result.released_quantity),
                        result.revision.get()
                    ));
                    on_close.run(());
                    on_refreshed.run(order_id);
                }
                Err(api_error) if api_error.unauthorized => {
                    retry_attempt.set(None);
                    pending.set(false);
                    on_unauthorized.run(());
                }
                Err(api_error) => {
                    pending.set(false);
                    let message = if api_error.ambiguous_outcome {
                        format!(
                            "{} The result is unknown; retry to recover the original result.",
                            api_error.message
                        )
                    } else {
                        retry_attempt.set(None);
                        api_error.message.clone()
                    };
                    error.set(Some(message));
                    toasts.error(api_error.message);
                }
            }
        });
    };

    view! {
        <section
            class="confirmation-panel order-cancellation-panel"
            role="alertdialog"
            aria-labelledby="cancel-order-title"
            aria-describedby="cancel-order-warning"
        >
            <div class="order-cancellation-heading">
                <Icon icon=UiIcon::Alert/>
                <div>
                    <h3 id="cancel-order-title">"Cancel fulfillment order"</h3>
                    <span>{format!("{} / Revision {}", order_key.get_value(), revision)}</span>
                </div>
            </div>
            <p id="cancel-order-warning" class="order-cancellation-warning">
                "This permanently closes the order and releases its holds and inventory commitments."
            </p>
            <div class="order-cancellation-fields">
                <label>
                    <span>"Reason"</span>
                    <select
                        autofocus=true
                        disabled=move || pending.get() || retry_attempt.get().is_some()
                        prop:value=move || reason.get()
                        on:change=move |event| {
                            reason.set(event_target_value(&event));
                            error.set(None);
                        }
                    >
                        <option value="client_request">"Client request"</option>
                        <option value="duplicate_order">"Duplicate order"</option>
                        <option value="data_correction">"Data correction"</option>
                        <option value="inventory_unavailable">"Inventory unavailable"</option>
                        <option value="fulfillment_exception">"Fulfillment exception"</option>
                        <option value="other">"Other"</option>
                    </select>
                </label>
                <label>
                    <span>{move || if reason.get() == "other" { "Note (required)" } else { "Note (optional)" }}</span>
                    <textarea
                        maxlength="1000"
                        rows="3"
                        aria-required=move || (reason.get() == "other").to_string()
                        disabled=move || pending.get() || retry_attempt.get().is_some()
                        prop:value=move || note.get()
                        on:input=move |event| {
                            note.set(event_target_value(&event));
                            error.set(None);
                        }
                    ></textarea>
                </label>
            </div>
            <Show when=move || error.get().is_some()>
                <p class="inline-command-error order-cancellation-error" role="alert">
                    {move || error.get().unwrap_or_default()}
                </p>
            </Show>
            <Show when=move || retry_attempt.get().is_some()>
                <p class="order-cancellation-retry-note" role="status">
                    "The original request is retained. Retry to recover its result."
                </p>
            </Show>
            <div class="form-actions">
                <button
                    type="button"
                    class="button danger-action"
                    disabled=move || pending.get()
                    on:click=cancel
                >
                    <Icon icon=UiIcon::Alert/>
                    {move || if pending.get() {
                        "Cancelling"
                    } else if retry_attempt.get().is_some() {
                        "Retry cancellation"
                    } else {
                        "Cancel order"
                    }}
                </button>
                <button
                    type="button"
                    class="button secondary-action"
                    disabled=move || pending.get() || retry_attempt.get().is_some()
                    on:click=move |_| on_close.run(())
                >
                    "Keep order"
                </button>
            </div>
        </section>
    }
}

fn optional_note(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_reason(value: &str) -> Option<OrderCancellationReason> {
    match value {
        "client_request" => Some(OrderCancellationReason::ClientRequest),
        "duplicate_order" => Some(OrderCancellationReason::DuplicateOrder),
        "data_correction" => Some(OrderCancellationReason::DataCorrection),
        "inventory_unavailable" => Some(OrderCancellationReason::InventoryUnavailable),
        "fulfillment_exception" => Some(OrderCancellationReason::FulfillmentException),
        "other" => Some(OrderCancellationReason::Other),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_reasons_are_explicit() {
        assert_eq!(
            parse_reason("fulfillment_exception"),
            Some(OrderCancellationReason::FulfillmentException)
        );
        assert_eq!(parse_reason("customer_request"), None);
    }

    #[test]
    fn notes_are_trimmed_and_blank_notes_are_omitted() {
        assert_eq!(
            optional_note("  Client request  ").as_deref(),
            Some("Client request")
        );
        assert_eq!(optional_note("   "), None);
    }
}
