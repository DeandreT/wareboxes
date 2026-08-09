use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{
    AbandonPackSessionRequest, PackSessionAbandonmentReason, Revision,
};

use crate::components::{Icon, UiIcon};

#[derive(Clone, Debug, PartialEq, Eq)]
struct AbandonmentDraft {
    reason: String,
    note: String,
}

#[component]
pub(super) fn PackingAbandonmentDialog(
    order_key: String,
    expected_revision: Revision,
    pending: Signal<bool>,
    retrying: Signal<bool>,
    command_error: Signal<Option<String>>,
    on_cancel: Callback<()>,
    on_submit: Callback<AbandonPackSessionRequest>,
    on_retry: Callback<()>,
) -> impl IntoView {
    let draft = RwSignal::new(AbandonmentDraft {
        reason: "order_cancellation".to_owned(),
        note: String::new(),
    });
    let validation_error = RwSignal::new(None::<String>);
    let reason_input = NodeRef::<html::Select>::new();
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if let Some(input) = reason_input.get() {
            let _ = input.focus();
        }
    });
    let locked = move || pending.get() || retrying.get();
    let close = move |_| {
        if !locked() {
            on_cancel.run(());
        }
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        if retrying.get_untracked() {
            on_retry.run(());
            return;
        }
        match build_request(&draft.get_untracked(), expected_revision) {
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
                class="packing-removal-dialog packing-abandonment-dialog"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="packing-abandonment-title"
            >
                <header class="packing-dialog-heading">
                    <span class="packing-dialog-icon"><Icon icon=UiIcon::Remove/></span>
                    <div>
                        <h2 id="packing-abandonment-title">"Abandon empty packing session"</h2>
                        <span>{format!("{order_key} returns to Awaiting packing for pick recovery.")}</span>
                    </div>
                    <button
                        type="button"
                        class="packing-dialog-close"
                        title="Close"
                        aria-label="Close session abandonment"
                        disabled=locked
                        on:click=close
                    >
                        <Icon icon=UiIcon::Close/>
                    </button>
                </header>
                <form class="packing-removal-form packing-abandonment-form" on:submit=submit>
                    <p class="packing-abandonment-warning">
                        "All content has been returned and every carton is voided. This preserves the session history and enables pick reversal or a fresh execution attempt."
                    </p>
                    <div class="packing-removal-fields">
                        <label>
                            <span>"Reason"</span>
                            <select
                                node_ref=reason_input
                                autofocus
                                disabled=locked
                                prop:value=move || draft.get().reason
                                on:change=move |event| draft.update(|value| value.reason = event_target_value(&event))
                            >
                                <option value="order_cancellation">"Order cancellation"</option>
                                <option value="repack">"Restart packing"</option>
                                <option value="station_issue">"Station issue"</option>
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
                            "The result is unknown. Retry sends the exact saved reason, note, revision, and idempotency key."
                        </p>
                    </Show>
                    <div class="form-actions packing-dialog-actions">
                        <button type="button" class="button secondary-action" disabled=locked on:click=close>
                            "Keep session"
                        </button>
                        <button type="submit" class="button danger-action" disabled=move || pending.get()>
                            <Icon icon=UiIcon::Remove/>
                            {move || if pending.get() { "Abandoning" } else if retrying.get() { "Retry exact command" } else { "Abandon session" }}
                        </button>
                    </div>
                </form>
            </section>
        </div>
    }
}

fn build_request(
    draft: &AbandonmentDraft,
    expected_revision: Revision,
) -> Result<AbandonPackSessionRequest, &'static str> {
    let reason = match draft.reason.as_str() {
        "order_cancellation" => PackSessionAbandonmentReason::OrderCancellation,
        "repack" => PackSessionAbandonmentReason::Repack,
        "station_issue" => PackSessionAbandonmentReason::StationIssue,
        "other" => PackSessionAbandonmentReason::Other,
        _ => return Err("Select a valid abandonment reason."),
    };
    let note = draft.note.trim();
    if reason == PackSessionAbandonmentReason::Other && note.is_empty() {
        return Err("A note is required when the reason is Other.");
    }
    if note.chars().count() > 500 {
        return Err("The abandonment note must be 500 characters or fewer.");
    }
    Ok(AbandonPackSessionRequest {
        reason,
        note: (!note.is_empty()).then(|| note.to_owned()),
        expected_revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_requires_a_note_and_revision_is_preserved() {
        let revision = Revision::new(8).unwrap();
        let mut draft = AbandonmentDraft {
            reason: "other".to_owned(),
            note: String::new(),
        };
        assert_eq!(
            build_request(&draft, revision),
            Err("A note is required when the reason is Other.")
        );
        draft.note = "station damaged carton".to_owned();
        let request = build_request(&draft, revision).unwrap();
        assert_eq!(request.expected_revision, revision);
        assert_eq!(request.note.as_deref(), Some("station damaged carton"));
    }
}
