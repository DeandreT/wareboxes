use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{CartonReopenReason, ReopenCartonRequest, Revision};

use crate::components::{Icon, UiIcon};

use super::commands::PendingPackingCommand;
use super::{dispatch_command, PackingSignals};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingCartonReopening {
    pub session_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub carton_id: i64,
    pub carton_barcode: String,
    pub content_count: i64,
    pub expected_revision: Revision,
}

pub(super) fn reopening_callbacks(
    signals: PackingSignals,
) -> (
    Callback<PendingCartonReopening>,
    Callback<()>,
    Callback<ReopenCartonRequest>,
) {
    let start = Callback::new(move |selection: PendingCartonReopening| {
        if signals.blocked() {
            return;
        }
        signals.scan.set(String::new());
        signals.error.set(false);
        signals.reopening.set(Some(selection));
    });
    let cancel = Callback::new(move |_| {
        if signals.pending.get_untracked() || signals.retry.get_untracked().is_some() {
            return;
        }
        signals.reopening.set(None);
        signals.error.set(false);
        signals
            .message
            .set("Continue packing the order.".to_owned());
        signals.refocus();
    });
    let submit = Callback::new(move |request: ReopenCartonRequest| {
        let Some(selection) = signals.reopening.get_untracked() else {
            return;
        };
        dispatch_command(
            PendingPackingCommand::ReopenCarton {
                session_id: selection.session_id,
                carton_id: selection.carton_id,
                request,
                idempotency_key: crate::api::new_idempotency_key(),
            },
            signals,
        );
    });
    (start, cancel, submit)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReopeningDraft {
    carton_scan: String,
    reason: String,
    note: String,
}

#[component]
pub(super) fn PackingReopeningDialog(
    selection: PendingCartonReopening,
    pending: Signal<bool>,
    retrying: Signal<bool>,
    command_error: Signal<Option<String>>,
    on_cancel: Callback<()>,
    on_submit: Callback<ReopenCartonRequest>,
    on_retry: Callback<()>,
) -> impl IntoView {
    let draft = RwSignal::new(ReopeningDraft {
        carton_scan: String::new(),
        reason: "packing_correction".to_owned(),
        note: String::new(),
    });
    let validation_error = RwSignal::new(None::<String>);
    let carton_input = NodeRef::<html::Input>::new();
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if let Some(input) = carton_input.get() {
            let _ = input.focus();
        }
    });
    let locked = move || pending.get() || retrying.get();
    let close = move |_| {
        if !locked() {
            on_cancel.run(());
        }
    };
    let expected_barcode = selection.carton_barcode.clone();
    let expected_revision = selection.expected_revision;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        if retrying.get_untracked() {
            on_retry.run(());
            return;
        }
        match build_request(&draft.get_untracked(), &expected_barcode, expected_revision) {
            Ok(request) => {
                validation_error.set(None);
                on_submit.run(request);
            }
            Err(message) => validation_error.set(Some(message.to_owned())),
        }
    };

    view! {
        <div class="packing-dialog-backdrop">
            <section
                class="packing-removal-dialog packing-reopening-dialog"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="packing-reopening-title"
            >
                <header class="packing-dialog-heading">
                    <span class="packing-dialog-icon"><Icon icon=UiIcon::Reverse/></span>
                    <div>
                        <h2 id="packing-reopening-title">"Reopen closed carton"</h2>
                        <span>{format!(
                            "{} - {} contents remain packed",
                            selection.order_key, selection.content_count
                        )}</span>
                    </div>
                    <button
                        type="button"
                        class="packing-dialog-close"
                        title="Close"
                        aria-label="Close carton reopening"
                        disabled=locked
                        on:click=close
                    >
                        <Icon icon=UiIcon::Close/>
                    </button>
                </header>
                <form class="packing-removal-form packing-reopening-form" on:submit=submit>
                    <p class="packing-abandonment-warning packing-reopening-warning">
                        "Reopening preserves the prior closure evidence and returns this carton to active packing. It is unavailable after outbound QA or shipment execution begins."
                    </p>
                    <div class="packing-removal-fields">
                        <label class="wide">
                            <span>"Scan carton"</span>
                            <input
                                node_ref=carton_input
                                autofocus
                                autocomplete="off"
                                placeholder=selection.carton_barcode.clone()
                                disabled=locked
                                prop:value=move || draft.get().carton_scan
                                on:input=move |event| draft.update(|value| value.carton_scan = event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span>"Reason"</span>
                            <select
                                disabled=locked
                                prop:value=move || draft.get().reason
                                on:change=move |event| draft.update(|value| value.reason = event_target_value(&event))
                            >
                                <option value="packing_correction">"Packing correction"</option>
                                <option value="quality_issue">"Quality issue"</option>
                                <option value="order_cancellation">"Order cancellation"</option>
                                <option value="other">"Other"</option>
                            </select>
                        </label>
                        <label>
                            <span>"Note"</span>
                            <input
                                maxlength="500"
                                placeholder="Required for Other"
                                disabled=locked
                                prop:value=move || draft.get().note
                                on:input=move |event| draft.update(|value| value.note = event_target_value(&event))
                            />
                        </label>
                    </div>
                    <Show when=move || validation_error.get().is_some() || command_error.get().is_some()>
                        <p class="inline-command-error packing-removal-error" role="alert">
                            {move || validation_error.get().or_else(|| command_error.get()).unwrap_or_default()}
                        </p>
                    </Show>
                    <Show when=move || retrying.get()>
                        <p class="packing-removal-retry" role="status">
                            "The result is unknown. Retry sends the exact saved scan, reason, note, revision, and idempotency key."
                        </p>
                    </Show>
                    <div class="form-actions packing-dialog-actions">
                        <button type="button" class="button secondary-action" disabled=locked on:click=close>
                            "Keep closed"
                        </button>
                        <button type="submit" class="button primary-action" disabled=move || pending.get()>
                            <Icon icon=UiIcon::Reverse/>
                            {move || if pending.get() { "Reopening" } else if retrying.get() { "Retry exact command" } else { "Reopen carton" }}
                        </button>
                    </div>
                </form>
            </section>
        </div>
    }
}

fn build_request(
    draft: &ReopeningDraft,
    expected_barcode: &str,
    expected_revision: Revision,
) -> Result<ReopenCartonRequest, &'static str> {
    let carton_scan = draft.carton_scan.trim();
    if carton_scan != expected_barcode {
        return Err("Scan the exact closed carton barcode.");
    }
    let reason = match draft.reason.as_str() {
        "packing_correction" => CartonReopenReason::PackingCorrection,
        "quality_issue" => CartonReopenReason::QualityIssue,
        "order_cancellation" => CartonReopenReason::OrderCancellation,
        "other" => CartonReopenReason::Other,
        _ => return Err("Select a valid reopening reason."),
    };
    let note = draft.note.trim();
    if reason == CartonReopenReason::Other && note.is_empty() {
        return Err("A note is required when the reason is Other.");
    }
    if note.chars().count() > 500 {
        return Err("The reopening note must be 500 characters or fewer.");
    }
    Ok(ReopenCartonRequest {
        carton_barcode: carton_scan.to_owned(),
        reason,
        note: (!note.is_empty()).then(|| note.to_owned()),
        expected_revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_carton_scan_and_other_note_are_required() {
        let revision = Revision::new(8).unwrap();
        let mut draft = ReopeningDraft {
            carton_scan: "WRONG".to_owned(),
            reason: "packing_correction".to_owned(),
            note: String::new(),
        };
        assert_eq!(
            build_request(&draft, "CARTON-1", revision),
            Err("Scan the exact closed carton barcode.")
        );
        draft.carton_scan = "CARTON-1".to_owned();
        draft.reason = "other".to_owned();
        assert_eq!(
            build_request(&draft, "CARTON-1", revision),
            Err("A note is required when the reason is Other.")
        );
        draft.note = "Mispack correction".to_owned();
        let request = build_request(&draft, "CARTON-1", revision).unwrap();
        assert_eq!(request.expected_revision, revision);
        assert_eq!(request.note.as_deref(), Some("Mispack correction"));
    }
}
