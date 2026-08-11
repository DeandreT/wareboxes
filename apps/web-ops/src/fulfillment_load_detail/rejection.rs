use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{InboundLoadRejectionReason, RejectInboundLoadRequest};
use wareboxes_core::models::Load;

use crate::api;
use crate::toast::use_toast_bus;

#[component]
pub(super) fn InboundRejectionConfirmation(
    load: Load,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_close: Callback<()>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let load_scan = RwSignal::new(String::new());
    let location_scan = RwSignal::new(String::new());
    let reason = RwSignal::new(InboundLoadRejectionReason::LoadDamaged);
    let note = RwSignal::new(String::new());
    let retry_attempt = RwSignal::new(None::<(RejectInboundLoadRequest, String)>);
    let form_ref = NodeRef::<html::Form>::new();
    let load_ref = NodeRef::<html::Input>::new();
    let load_id = load.id;
    let expected_load_scan = load.execution_barcode;
    let toasts = use_toast_bus();

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if let Some(input) = load_ref.get() {
            let _ = input.focus();
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
            if reason.get_untracked() == InboundLoadRejectionReason::Other && note_value.is_none() {
                error.set(Some("Explain the other rejection reason.".to_owned()));
                return;
            }
            let request = RejectInboundLoadRequest {
                load_scan: load_scan.get_untracked(),
                receiving_location_scan: location_scan.get_untracked(),
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
            match api::reject_inbound_load(load_id, &request, &key).await {
                Ok(_) => {
                    retry_attempt.set(None);
                    pending.set(false);
                    on_close.run(());
                    toasts.success(format!("Inbound load #{load_id} rejected."));
                    on_refreshed.run(load_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    if !api_error.ambiguous_outcome {
                        retry_attempt.set(None);
                    }
                    toasts.error(api_error.message.clone());
                    error.set(Some(if api_error.ambiguous_outcome {
                        "Rejection outcome is unknown. Retry the exact saved scans and reason to reconcile it."
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
            class="confirmation-panel unloading-confirmation"
            role="alertdialog"
            aria-labelledby="reject-inbound-load-title"
            on:submit=submit
        >
            <h3 id="reject-inbound-load-title">"Reject inbound load"</h3>
            <p>"Confirm the arrived load and receiving location before stopping inbound execution."</p>
            <div class="evidence-summary">
                <span><strong>"Expected load"</strong> {expected_load_scan}</span>
            </div>
            <div class="form-grid two-column">
                <label>
                    <span>"Load scan"</span>
                    <input
                        node_ref=load_ref
                        type="text"
                        autocomplete="off"
                        placeholder="Scan load barcode"
                        prop:value=move || load_scan.get()
                        on:input=move |event| load_scan.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Receiving location scan"</span>
                    <input
                        type="text"
                        autocomplete="off"
                        placeholder="Scan assigned receiving location"
                        prop:value=move || location_scan.get()
                        on:input=move |event| location_scan.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Reason"</span>
                    <select
                        prop:value=move || rejection_reason_wire(reason.get())
                        on:change=move |event| reason.set(rejection_reason_from_wire(&event_target_value(&event)))
                    >
                        <option value="load_damaged">"Load damaged"</option>
                        <option value="seal_discrepancy">"Seal discrepancy"</option>
                        <option value="wrong_facility">"Wrong facility"</option>
                        <option value="documentation_mismatch">"Documentation mismatch"</option>
                        <option value="appointment_violation">"Appointment violation"</option>
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
                    {move || if pending.get() { "Rejecting" } else { "Reject load" }}
                </button>
                <button type="button" class="button secondary-action" on:click=move |_| on_close.run(())>
                    "Go back"
                </button>
            </div>
        </form>
    }
}

const fn rejection_reason_wire(reason: InboundLoadRejectionReason) -> &'static str {
    match reason {
        InboundLoadRejectionReason::LoadDamaged => "load_damaged",
        InboundLoadRejectionReason::SealDiscrepancy => "seal_discrepancy",
        InboundLoadRejectionReason::WrongFacility => "wrong_facility",
        InboundLoadRejectionReason::DocumentationMismatch => "documentation_mismatch",
        InboundLoadRejectionReason::AppointmentViolation => "appointment_violation",
        InboundLoadRejectionReason::Other => "other",
    }
}

fn rejection_reason_from_wire(value: &str) -> InboundLoadRejectionReason {
    match value {
        "seal_discrepancy" => InboundLoadRejectionReason::SealDiscrepancy,
        "wrong_facility" => InboundLoadRejectionReason::WrongFacility,
        "documentation_mismatch" => InboundLoadRejectionReason::DocumentationMismatch,
        "appointment_violation" => InboundLoadRejectionReason::AppointmentViolation,
        "other" => InboundLoadRejectionReason::Other,
        _ => InboundLoadRejectionReason::LoadDamaged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_reason_wire_values_round_trip() {
        for reason in [
            InboundLoadRejectionReason::LoadDamaged,
            InboundLoadRejectionReason::SealDiscrepancy,
            InboundLoadRejectionReason::WrongFacility,
            InboundLoadRejectionReason::DocumentationMismatch,
            InboundLoadRejectionReason::AppointmentViolation,
            InboundLoadRejectionReason::Other,
        ] {
            assert_eq!(
                rejection_reason_from_wire(rejection_reason_wire(reason)),
                reason
            );
        }
    }
}
