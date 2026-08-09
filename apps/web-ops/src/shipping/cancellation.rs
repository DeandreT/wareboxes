use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{CancelShipmentRequest, Revision, ShipmentCancellationReason};

use crate::components::{Icon, UiIcon};

#[component]
pub(super) fn ShipmentCancellationAction(
    shipment_id: i64,
    shipment_revision: Revision,
    order_revision: Revision,
    blocked: Signal<bool>,
    on_cancel: Callback<(i64, CancelShipmentRequest)>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let reason = RwSignal::new(ShipmentCancellationReason::PackingCorrection);
    let note = RwSignal::new(String::new());
    let error = RwSignal::<Option<String>>::new(None);
    let reason_input = NodeRef::<html::Select>::new();

    Effect::new(move |_| {
        if open.get() {
            if let Some(input) = reason_input.get() {
                let _ = input.focus();
            }
        }
    });

    let close = Callback::new(move |_| {
        if !blocked.get_untracked() {
            open.set(false);
            error.set(None);
        }
    });
    let submit = Callback::new(move |_| {
        if blocked.get_untracked() {
            return;
        }
        let trimmed_note = note.get_untracked().trim().to_owned();
        if reason.get_untracked() == ShipmentCancellationReason::Other && trimmed_note.is_empty() {
            error.set(Some("An audit note is required for Other.".to_owned()));
            return;
        }
        if trimmed_note.chars().any(char::is_control) {
            error.set(Some("The audit note must be a single line.".to_owned()));
            return;
        }
        on_cancel.run((
            shipment_id,
            CancelShipmentRequest {
                expected_shipment_revision: shipment_revision,
                expected_order_revision: order_revision,
                reason: reason.get_untracked(),
                note: (!trimmed_note.is_empty()).then_some(trimmed_note),
            },
        ));
        open.set(false);
        error.set(None);
    });

    view! {
        <button
            type="button"
            class="button secondary-action shipping-shipment-cancel"
            disabled=move || blocked.get()
            on:click=move |_| {
                reason.set(ShipmentCancellationReason::PackingCorrection);
                note.set(String::new());
                error.set(None);
                open.set(true);
            }
        >
            <Icon icon=UiIcon::Reverse/>
            "Cancel shipment"
        </button>
        <Show when=move || open.get()>
            <div class="shipping-qa-dialog-backdrop">
                <section
                    class="shipping-qa-dialog shipping-shipment-cancel-dialog"
                    role="alertdialog"
                    aria-modal="true"
                    aria-labelledby="shipping-shipment-cancel-title"
                >
                    <header>
                        <div>
                            <span class="eyebrow">"Supervisor recovery"</span>
                            <h2 id="shipping-shipment-cancel-title">"Cancel shipment attempt"</h2>
                        </div>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Close shipment cancellation"
                            disabled=move || blocked.get()
                            on:click=move |_| close.run(())
                        ><Icon icon=UiIcon::Close/></button>
                    </header>
                    <p>
                        "This is allowed only before any carton departs. Shipment snapshots, manifest, tracking assignments, and documents remain in immutable history; packing recovery and a new shipment attempt become available."
                    </p>
                    <label>
                        <span>"Reason"</span>
                        <select
                            node_ref=reason_input
                            prop:value=move || reason_wire(reason.get())
                            on:change=move |event| {
                                reason.set(reason_from_wire(&event_target_value(&event)));
                                error.set(None);
                            }
                            disabled=move || blocked.get()
                        >
                            <option value="packing_correction">"Packing correction"</option>
                            <option value="shipping_data_correction">"Shipping data correction"</option>
                            <option value="duplicate_shipment">"Duplicate shipment"</option>
                            <option value="operator_error">"Operator error"</option>
                            <option value="other">"Other"</option>
                        </select>
                    </label>
                    <label>
                        <span>"Audit note"</span>
                        <input
                            maxlength="500"
                            placeholder="Required for Other"
                            prop:value=move || note.get()
                            on:input=move |event| {
                                note.set(event_target_value(&event));
                                error.set(None);
                            }
                            disabled=move || blocked.get()
                        />
                    </label>
                    <Show when=move || error.get().is_some()>
                        <p class="error" role="alert">{move || error.get().unwrap_or_default()}</p>
                    </Show>
                    <footer>
                        <button
                            type="button"
                            class="button secondary-action"
                            disabled=move || blocked.get()
                            on:click=move |_| close.run(())
                        >"Keep shipment active"</button>
                        <button
                            type="button"
                            class="button danger-action"
                            disabled=move || blocked.get()
                            on:click=move |_| submit.run(())
                        ><Icon icon=UiIcon::Reverse/>"Cancel attempt"</button>
                    </footer>
                </section>
            </div>
        </Show>
    }
}

fn reason_wire(reason: ShipmentCancellationReason) -> &'static str {
    match reason {
        ShipmentCancellationReason::PackingCorrection => "packing_correction",
        ShipmentCancellationReason::ShippingDataCorrection => "shipping_data_correction",
        ShipmentCancellationReason::DuplicateShipment => "duplicate_shipment",
        ShipmentCancellationReason::OperatorError => "operator_error",
        ShipmentCancellationReason::Other => "other",
    }
}

fn reason_from_wire(value: &str) -> ShipmentCancellationReason {
    match value {
        "shipping_data_correction" => ShipmentCancellationReason::ShippingDataCorrection,
        "duplicate_shipment" => ShipmentCancellationReason::DuplicateShipment,
        "operator_error" => ShipmentCancellationReason::OperatorError,
        "other" => ShipmentCancellationReason::Other,
        _ => ShipmentCancellationReason::PackingCorrection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_reason_wire_values_round_trip() {
        for reason in [
            ShipmentCancellationReason::PackingCorrection,
            ShipmentCancellationReason::ShippingDataCorrection,
            ShipmentCancellationReason::DuplicateShipment,
            ShipmentCancellationReason::OperatorError,
            ShipmentCancellationReason::Other,
        ] {
            assert_eq!(reason_from_wire(reason_wire(reason)), reason);
        }
    }
}
