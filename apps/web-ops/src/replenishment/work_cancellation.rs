use leptos::prelude::*;
use lucide_leptos::{ArchiveX, RefreshCw, X};
use wareboxes_api_contract::v1::{
    CancelReplenishmentWorkRequest, ReplenishmentQueueEntryResponse,
    ReplenishmentWorkCancellationReason, ReplenishmentWorkCancellationResponse,
    ReplenishmentWorkStatus,
};

use crate::api;
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

const MAX_NOTE_LENGTH: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CancellationAttempt {
    work_id: i64,
    request: CancelReplenishmentWorkRequest,
    idempotency_key: String,
}

#[component]
pub(super) fn WorkCancellationDialog(
    work: ReplenishmentQueueEntryResponse,
    on_close: Callback<()>,
    on_cancelled: Callback<ReplenishmentWorkCancellationResponse>,
    on_authoritative_refresh: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new("demand_removed".to_owned());
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let retry = RwSignal::new(None::<CancellationAttempt>);
    let error = RwSignal::new(None::<String>);
    let invalidated = RwSignal::new(false);
    let toasts = use_toast_bus();
    let locked = move || pending.get() || retry.get().is_some();
    let fields_locked = move || locked() || invalidated.get();
    let work_for_submit = work.clone();

    let close = move |_| {
        if !locked() {
            on_close.run(());
        }
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() || invalidated.get_untracked() {
            return;
        }
        let attempt = if let Some(attempt) = retry.get_untracked() {
            attempt
        } else {
            if work_for_submit.status != ReplenishmentWorkStatus::Pending {
                error.set(Some(
                    "Only pending replenishment work can be cancelled. Refresh the queue."
                        .to_owned(),
                ));
                invalidated.set(true);
                on_authoritative_refresh.run(());
                return;
            }
            let request = match cancellation_request(&reason.get_untracked(), &note.get_untracked())
            {
                Ok(request) => request,
                Err(message) => {
                    error.set(Some(message));
                    return;
                }
            };
            CancellationAttempt {
                work_id: work_for_submit.work_id,
                request,
                idempotency_key: api::new_idempotency_key(),
            }
        };
        retry.set(Some(attempt.clone()));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let response = api::cancel_replenishment_work(
                attempt.work_id,
                &attempt.request,
                &attempt.idempotency_key,
            )
            .await;
            if retry.get_untracked().as_ref() != Some(&attempt) {
                return;
            }
            pending.set(false);
            match response {
                Ok(result) => {
                    retry.set(None);
                    toasts.success(format!(
                        "Work #{} cancelled; {} {} removed from active inbound.",
                        result.work_id,
                        format_quantity(result.quantity),
                        result.uom
                    ));
                    on_cancelled.run(result);
                    on_close.run(());
                }
                Err(api_error) if api_error.unauthorized => {
                    retry.set(None);
                    on_unauthorized.run(());
                }
                Err(api_error) if api_error.ambiguous_outcome => {
                    error.set(Some(format!(
                        "{} The result is unknown; retry the retained cancellation.",
                        api_error.message
                    )));
                    toasts.error(api_error.message);
                }
                Err(api_error) => {
                    retry.set(None);
                    invalidated.set(true);
                    error.set(Some(format!(
                        "{} No retry was retained; authoritative work is being refreshed.",
                        api_error.message
                    )));
                    toasts.error(api_error.message);
                    on_authoritative_refresh.run(());
                }
            }
        });
    };

    let item = work
        .item_description
        .clone()
        .unwrap_or_else(|| format!("Item #{}", work.item_id));
    let assigned = work
        .claimed_by
        .map_or_else(|| "Unassigned".to_owned(), |user| format!("User #{user}"));

    view! {
        <div class="replenishment-dialog-backdrop">
            <section
                class="replenishment-dialog danger replenishment-cancel-dialog"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="replenishment-cancel-title"
            >
                <header class="replenishment-dialog-heading">
                    <span class="replenishment-dialog-icon" aria-hidden="true"><ArchiveX size=17/></span>
                    <div>
                        <h2 id="replenishment-cancel-title">"Cancel pending work"</h2>
                        <span>{format!("Work #{} / Plan #{} / Policy #{}", work.work_id, work.plan_id, work.policy_id)}</span>
                    </div>
                    <button type="button" class="replenishment-dialog-close" title="Close" aria-label="Close cancellation" disabled=locked on:click=close><X size=16/></button>
                </header>
                <form class="replenishment-dialog-form" on:submit=submit>
                    <dl class="replenishment-cancel-facts">
                        <div><dt>"Client / facility"</dt><dd>{format!("{} / {}", work.inventory_owner_name, work.facility_name)}</dd></div>
                        <div><dt>"Item / quantity"</dt><dd>{format!("{} / {} {}", item, format_quantity(work.quantity), work.uom)}</dd></div>
                        <div><dt>"Source"</dt><dd>{work.source_location.barcode}</dd></div>
                        <div><dt>"Pick face"</dt><dd>{work.destination_pick_face.barcode}</dd></div>
                        <div><dt>"Assigned user"</dt><dd>{assigned}</dd></div>
                        <div><dt>"Status"</dt><dd>"Pending"</dd></div>
                    </dl>
                    <label class="replenishment-cancel-field">
                        <span>"Reason"</span>
                        <select disabled=fields_locked prop:value=move || reason.get() on:change=move |event| reason.set(event_target_value(&event))>
                            <option value="demand_removed">"Demand removed"</option>
                            <option value="policy_reconfigured">"Policy reconfigured"</option>
                            <option value="source_unavailable">"Source unavailable"</option>
                            <option value="destination_unavailable">"Destination unavailable"</option>
                            <option value="planning_error">"Planning error"</option>
                            <option value="other">"Other"</option>
                        </select>
                    </label>
                    <label class="replenishment-cancel-field">
                        <span>"Note" <small>{move || format!("{} / {MAX_NOTE_LENGTH}", note.get().chars().count())}</small></span>
                        <textarea
                            rows="3"
                            maxlength=MAX_NOTE_LENGTH
                            disabled=fields_locked
                            placeholder="Required for Other"
                            prop:value=move || note.get()
                            on:input=move |event| note.set(event_target_value(&event))
                        ></textarea>
                    </label>
                    <p class="replenishment-dialog-intro">
                        "Cancellation removes this quantity from active inbound. It does not move inventory."
                    </p>
                    <Show when=move || error.get().is_some()>
                        <p class="inline-command-error replenishment-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
                    </Show>
                    <Show when=move || retry.get().is_some()>
                        <p class="replenishment-retry-note" role="status">"The exact cancellation and idempotency key are retained for retry."</p>
                    </Show>
                    <Show when=move || invalidated.get()>
                        <p class="replenishment-refresh-note" role="status">"Authoritative work was refreshed. Close this dialog and act from the current row state."</p>
                    </Show>
                    <div class="form-actions replenishment-dialog-actions">
                        <button type="button" class="button secondary-action" disabled=locked on:click=close>"Keep work"</button>
                        <button type="submit" class="button danger-action" disabled=move || pending.get() || invalidated.get()>
                            {move || if retry.get().is_some() { view! { <RefreshCw size=15/> }.into_any() } else { view! { <ArchiveX size=15/> }.into_any() }}
                            {move || if pending.get() { "Cancelling" } else if retry.get().is_some() { "Retry exact cancellation" } else { "Cancel work" }}
                        </button>
                    </div>
                </form>
            </section>
        </div>
    }
}

fn cancellation_request(
    reason: &str,
    note: &str,
) -> Result<CancelReplenishmentWorkRequest, String> {
    let reason = match reason {
        "demand_removed" => ReplenishmentWorkCancellationReason::DemandRemoved,
        "policy_reconfigured" => ReplenishmentWorkCancellationReason::PolicyReconfigured,
        "source_unavailable" => ReplenishmentWorkCancellationReason::SourceUnavailable,
        "destination_unavailable" => ReplenishmentWorkCancellationReason::DestinationUnavailable,
        "planning_error" => ReplenishmentWorkCancellationReason::PlanningError,
        "other" => ReplenishmentWorkCancellationReason::Other,
        _ => return Err("Select a valid cancellation reason.".to_owned()),
    };
    let note = note.trim();
    if note.chars().count() > MAX_NOTE_LENGTH || note.chars().any(char::is_control) {
        return Err(format!(
            "Note must be control-free and cannot exceed {MAX_NOTE_LENGTH} characters."
        ));
    }
    let note = (!note.is_empty()).then(|| note.to_owned());
    if reason == ReplenishmentWorkCancellationReason::Other && note.is_none() {
        return Err("Enter a note when the reason is Other.".to_owned());
    }
    Ok(CancelReplenishmentWorkRequest { reason, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_normalizes_note_and_requires_evidence_for_other() {
        let request = cancellation_request("source_unavailable", "  blocked aisle  ").unwrap();
        assert_eq!(request.note.as_deref(), Some("blocked aisle"));
        assert_eq!(
            request.reason,
            ReplenishmentWorkCancellationReason::SourceUnavailable
        );
        assert!(cancellation_request("other", "  ").is_err());
    }

    #[test]
    fn cancellation_note_is_bounded_before_transport() {
        assert!(cancellation_request("planning_error", &"x".repeat(MAX_NOTE_LENGTH + 1)).is_err());
    }
}
