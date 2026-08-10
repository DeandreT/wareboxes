use leptos::prelude::*;
use lucide_leptos::{Pencil, RefreshCw, Save, X};
use wareboxes_api_contract::v1::{
    CorrectIntegrationOrderRequest, CreateFulfillmentOrderRequest,
    InboundIntegrationProcessingResponse, IntegrationOrderProcessingStatus,
};

use super::MonitorSignals;
use crate::api;

#[derive(Clone, PartialEq, Eq)]
struct SavedCorrectionCommand {
    receipt_id: i64,
    request: CorrectIntegrationOrderRequest,
    idempotency_key: String,
}

fn correction_request(
    processing: &InboundIntegrationProcessingResponse,
    reason: &str,
    payload: &str,
) -> Result<CorrectIntegrationOrderRequest, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("Correction reason is required.".into());
    }
    if reason.chars().count() > 500 || reason.chars().any(char::is_control) {
        return Err("Correction reason must be 500 characters or fewer.".into());
    }
    let order = serde_json::from_str::<CreateFulfillmentOrderRequest>(payload.trim())
        .map_err(|error| format!("Corrected order JSON is invalid: {error}"))?;
    Ok(CorrectIntegrationOrderRequest {
        expected_revision: processing.revision,
        reason: reason.to_owned(),
        order,
    })
}

#[component]
pub(super) fn CorrectionPanel(
    signals: MonitorSignals,
    receipt_id: i64,
    processing: InboundIntegrationProcessingResponse,
    initial_payload: String,
) -> impl IntoView {
    let processing = StoredValue::new(processing);
    let open = RwSignal::new(false);
    let reason = RwSignal::new(String::new());
    let payload = RwSignal::new(initial_payload);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<SavedCorrectionCommand>);
    let execute = Callback::new(move |saved: SavedCorrectionCommand| {
        if signals.command_pending.get_untracked() {
            return;
        }
        signals.command_pending.set(true);
        error.set(None);
        signals.error.set(None);
        signals.notice.set(None);
        leptos::task::spawn_local(async move {
            match api::correct_inbound_order(
                saved.receipt_id,
                &saved.request,
                &saved.idempotency_key,
            )
            .await
            {
                Ok(result) => {
                    retry.set(None);
                    open.set(false);
                    super::request_inbound(signals, None, Vec::new());
                    super::select_inbound_receipt(signals, saved.receipt_id);
                    signals.notice.set(Some(match result.status {
                        IntegrationOrderProcessingStatus::Processed => result.order_id.map_or_else(
                            || "Corrected envelope processed.".to_owned(),
                            |id| format!("Corrected envelope processed as order #{id}."),
                        ),
                        IntegrationOrderProcessingStatus::Quarantined => format!(
                            "Correction attempt {} remains quarantined.",
                            result.attempt_count
                        ),
                    }));
                }
                Err(api_error) if api_error.unauthorized => signals.on_unauthorized.run(()),
                Err(api_error) if api_error.ambiguous_outcome => {
                    retry.set(Some(saved));
                    error.set(Some(format!(
                        "{} Retry the exact saved correction to reconcile the outcome.",
                        api_error.message
                    )));
                }
                Err(api_error) => {
                    retry.set(None);
                    error.set(Some(api_error.message));
                    super::select_inbound_receipt(signals, saved.receipt_id);
                }
            }
            signals.command_pending.set(false);
        });
    });
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if retry.get_untracked().is_some() {
            return;
        }
        match correction_request(
            &processing.get_value(),
            &reason.get_untracked(),
            &payload.get_untracked(),
        ) {
            Ok(request) => execute.run(SavedCorrectionCommand {
                receipt_id,
                request,
                idempotency_key: api::new_idempotency_key(),
            }),
            Err(message) => error.set(Some(message)),
        }
    };

    view! {
        <Show
            when=move || open.get()
            fallback=move || view! {
                <button type="button" class="button secondary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| open.set(true)>
                    <Pencil size=13/>"Correct envelope"
                </button>
            }
        >
            <form class="integration-correction-form" on:submit=submit>
                <label><span>"Correction reason"</span><input maxlength="500" disabled=move || signals.command_pending.get() || retry.get().is_some() prop:value=move || reason.get() on:input=move |event| { reason.set(event_target_value(&event)); error.set(None); }/></label>
                <label class="wide"><span>"Corrected fulfillment order JSON"</span><textarea spellcheck="false" disabled=move || signals.command_pending.get() || retry.get().is_some() prop:value=move || payload.get() on:input=move |event| { payload.set(event_target_value(&event)); error.set(None); }></textarea></label>
                <Show when=move || error.get().is_some()><p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p></Show>
                <footer>
                    <button type="button" class="icon-button" title="Close correction" aria-label="Close correction" disabled=move || signals.command_pending.get() on:click=move |_| { open.set(false); retry.set(None); error.set(None); }><X size=14/></button>
                    <Show when=move || retry.get().is_some()>
                        <button type="button" class="button secondary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| { if let Some(saved)=retry.get_untracked() { execute.run(saved); } }><RefreshCw size=13/>"Retry exact correction"</button>
                    </Show>
                    <button type="submit" class="button primary-action compact" disabled=move || signals.command_pending.get() || retry.get().is_some()><Save size=13/>{move || if signals.command_pending.get() { "Submitting" } else { "Submit correction" }}</button>
                </footer>
            </form>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use wareboxes_api_contract::v1::{IntegrationOrderProcessingStatus, Revision};

    use super::*;

    fn processing() -> InboundIntegrationProcessingResponse {
        InboundIntegrationProcessingResponse {
            processing_id: 1,
            adapter_key: "wareboxes.fulfillment_order".into(),
            mapping_version: 1,
            status: IntegrationOrderProcessingStatus::Quarantined,
            revision: Revision::new(2).unwrap(),
            attempt_count: 2,
            input_payload_sha256: "0".repeat(64),
            latest_correction_id: None,
            latest_correction_payload: None,
            latest_correction_payload_truncated: false,
            order_id: None,
            order_revision: None,
            error_code: Some("invalid_payload".into()),
            error_message: Some("invalid".into()),
            attempted_by: 1,
            attempted_by_name: "Operator".into(),
            attempted_at: "2026-08-10T00:00:00Z".into(),
            processed_at: None,
            attempts: Vec::new(),
        }
    }

    #[test]
    fn correction_draft_requires_reason_and_strict_order_json() {
        let valid = r#"{
            "inventory_owner_id":7,"order_key":"EXT-1","rush":false,"ship_by":null,
            "destination":{"recipient_name":"Receiving","company":null,"phone":null,
            "email":null,"line1":"10 Main","line2":null,"city":"Reno","region":"NV",
            "postal_code":"89501","country":"US"},
            "lines":[{"line_key":"1","item_id":11,"quantity":2,"requested_uom":"case"}]
        }"#;
        assert!(correction_request(&processing(), "fixed item", valid).is_ok());
        assert!(correction_request(&processing(), "", valid).is_err());
        assert!(correction_request(&processing(), "fixed item", r#"{"force":true}"#).is_err());
    }
}
