use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{CancelInboundLoadRequest, InboundLoadCancellationReason};
use wareboxes_core::models::Load;

use crate::api;
use crate::toast::use_toast_bus;

#[component]
pub(super) fn InboundCancellationConfirmation(
    load: Load,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_close: Callback<()>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new(InboundLoadCancellationReason::SupplierCancelled);
    let note = RwSignal::new(String::new());
    let retry_attempt = RwSignal::new(None::<(CancelInboundLoadRequest, String)>);
    let form_ref = NodeRef::<html::Form>::new();
    let reason_ref = NodeRef::<html::Select>::new();
    let load_id = load.id;
    let reference = load
        .reference_number
        .clone()
        .unwrap_or_else(|| format!("Load #{load_id}"));
    let toasts = use_toast_bus();

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if let Some(select) = reason_ref.get() {
            let _ = select.focus();
        }
        if let Some(form) = form_ref.get() {
            form.scroll_into_view_with_bool(false);
        }
    });

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let (request, key) = if let Some(saved) = retry_attempt.get_untracked() {
            saved
        } else {
            let note_value = note.get_untracked().trim().to_owned();
            let note_value = (!note_value.is_empty()).then_some(note_value);
            if reason.get_untracked() == InboundLoadCancellationReason::Other
                && note_value.is_none()
            {
                error.set(Some("Explain the other cancellation reason.".to_owned()));
                return;
            }
            let request = CancelInboundLoadRequest {
                reason: reason.get_untracked(),
                note: note_value,
            };
            let key = api::new_idempotency_key();
            retry_attempt.set(Some((request.clone(), key.clone())));
            (request, key)
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::cancel_inbound_load(load_id, &request, &key).await {
                Ok(_) => {
                    retry_attempt.set(None);
                    pending.set(false);
                    on_close.run(());
                    toasts.success(format!("Inbound load #{load_id} cancelled."));
                    on_refreshed.run(load_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    if !api_error.ambiguous_outcome {
                        retry_attempt.set(None);
                    }
                    toasts.error(api_error.message.clone());
                    error.set(Some(if api_error.ambiguous_outcome {
                        "Cancellation outcome is unknown. Retry the exact saved cancellation to reconcile it."
                            .to_owned()
                    } else {
                        api_error.message
                    }));
                    pending.set(false);
                }
            }
        });
    };

    view! {
        <form
            node_ref=form_ref
            class="confirmation-panel arrival-confirmation"
            role="alertdialog"
            aria-labelledby="cancel-inbound-load-title"
            on:submit=submit
        >
            <h3 id="cancel-inbound-load-title">"Cancel inbound load"</h3>
            <p>"This stops the planned receipt before physical execution begins."</p>
            <div class="evidence-summary">
                <span><strong>"Load"</strong> {reference}</span>
            </div>
            <div class="form-grid two-column">
                <label>
                    <span>"Reason"</span>
                    <select
                        node_ref=reason_ref
                        prop:value=move || cancellation_reason_wire(reason.get())
                        on:change=move |event| reason.set(cancellation_reason_from_wire(&event_target_value(&event)))
                    >
                        <option value="supplier_cancelled">"Supplier cancelled"</option>
                        <option value="carrier_cancelled">"Carrier cancelled"</option>
                        <option value="duplicate_plan">"Duplicate plan"</option>
                        <option value="warehouse_capacity">"Warehouse capacity"</option>
                        <option value="other">"Other"</option>
                    </select>
                </label>
                <label>
                    <span>"Note"</span>
                    <input
                        type="text"
                        maxlength="500"
                        placeholder="Optional unless reason is Other"
                        prop:value=move || note.get()
                        on:input=move |event| note.set(event_target_value(&event))
                    />
                </label>
            </div>
            <div class="form-actions">
                <button type="submit" class="button danger-action" disabled=move || pending.get()>
                    {move || if pending.get() { "Cancelling" } else { "Cancel load" }}
                </button>
                <button type="button" class="button secondary-action" on:click=move |_| on_close.run(())>
                    "Go back"
                </button>
            </div>
        </form>
    }
}

const fn cancellation_reason_wire(reason: InboundLoadCancellationReason) -> &'static str {
    match reason {
        InboundLoadCancellationReason::CarrierCancelled => "carrier_cancelled",
        InboundLoadCancellationReason::SupplierCancelled => "supplier_cancelled",
        InboundLoadCancellationReason::DuplicatePlan => "duplicate_plan",
        InboundLoadCancellationReason::WarehouseCapacity => "warehouse_capacity",
        InboundLoadCancellationReason::Other => "other",
    }
}

fn cancellation_reason_from_wire(value: &str) -> InboundLoadCancellationReason {
    match value {
        "carrier_cancelled" => InboundLoadCancellationReason::CarrierCancelled,
        "duplicate_plan" => InboundLoadCancellationReason::DuplicatePlan,
        "warehouse_capacity" => InboundLoadCancellationReason::WarehouseCapacity,
        "other" => InboundLoadCancellationReason::Other,
        _ => InboundLoadCancellationReason::SupplierCancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_wire_values_round_trip() {
        for reason in [
            InboundLoadCancellationReason::CarrierCancelled,
            InboundLoadCancellationReason::SupplierCancelled,
            InboundLoadCancellationReason::DuplicatePlan,
            InboundLoadCancellationReason::WarehouseCapacity,
            InboundLoadCancellationReason::Other,
        ] {
            assert_eq!(
                cancellation_reason_from_wire(cancellation_reason_wire(reason)),
                reason
            );
        }
    }
}
