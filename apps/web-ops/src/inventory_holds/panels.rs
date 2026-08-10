use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    DisposeInboundInspectionRequest, InboundInspectionOutcome, InventoryBalanceStatus,
    InventoryHoldResponse, InventoryHoldStatus,
};

use crate::components::{Icon, UiIcon};
use crate::view_model::format_quantity;

use super::{hold_item_label, hold_location_label, reason_label};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct InspectionAttempt {
    pub request: DisposeInboundInspectionRequest,
    pub idempotency_key: String,
}

pub(super) fn inspection_request(
    outcome: InboundInspectionOutcome,
    note: &str,
) -> Result<DisposeInboundInspectionRequest, &'static str> {
    let note = note.trim();
    if note.is_empty() {
        return Err("Enter the inspection findings.");
    }
    if note.chars().count() > 500 {
        return Err("Inspection findings cannot exceed 500 characters.");
    }
    Ok(DisposeInboundInspectionRequest {
        outcome,
        note: note.to_owned(),
    })
}

pub(super) fn is_receipt_inspection_hold(hold: &InventoryHoldResponse) -> bool {
    hold.status == InventoryHoldStatus::Active
        && hold.inventory_status == InventoryBalanceStatus::Quarantine
        && matches!(
            hold.reference_type.as_deref(),
            Some("expected_receipt_line" | "unexpected_receipt")
        )
        && hold.reference_id.is_some()
}

pub(super) const fn retain_inspection_attempt(ambiguous_outcome: bool) -> bool {
    ambiguous_outcome
}

#[component]
pub(super) fn InspectionPanel(
    hold: InventoryHoldResponse,
    outcome: RwSignal<InboundInspectionOutcome>,
    note: RwSignal<String>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    retry_retained: Signal<bool>,
    on_change: Callback<()>,
    on_confirm: Callback<leptos::ev::SubmitEvent>,
    on_cancel: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    let action_label = move || match outcome.get() {
        InboundInspectionOutcome::Approved => "Approve stock",
        InboundInspectionOutcome::Damaged => "Mark damaged",
    };

    view! {
        <form
            class="hold-form inspection-panel"
            on:submit=move |event| on_confirm.run(event)
        >
            <div class="command-panel-heading">
                <span class="command-icon"><Icon icon=UiIcon::Disposition/></span>
                <div>
                    <p class="eyebrow">"Inbound inspection"</p>
                    <h2 id="hold-command-title">{format!("Hold #{}", hold.id)}</h2>
                </div>
            </div>
            <dl class="position-facts">
                <div><dt>"Item"</dt><dd>{hold_item_label(&hold)}</dd></div>
                <div><dt>"Client"</dt><dd>{hold.inventory_owner_name.clone()}</dd></div>
                <div><dt>"Position"</dt><dd>{hold_location_label(&hold)}</dd></div>
                <div>
                    <dt>"Quarantined"</dt>
                    <dd>{format!("{} {}", format_quantity(hold.quantity), hold.uom)}</dd>
                </div>
            </dl>

            <label for="inspection-outcome">"Outcome"</label>
            <select
                id="inspection-outcome"
                prop:value=move || outcome_code(outcome.get())
                on:change=move |event| {
                    if let Some(value) = parse_outcome(&event_target_value(&event)) {
                        outcome.set(value);
                        on_change.run(());
                    }
                }
            >
                <option value="approved">"Approved"</option>
                <option value="damaged">"Damaged"</option>
            </select>

            <label for="inspection-note">"Inspection findings"</label>
            <textarea
                id="inspection-note"
                rows="4"
                maxlength="500"
                required
                prop:value=move || note.get()
                on:input=move |event| {
                    note.set(event_target_value(&event));
                    on_change.run(());
                }
            ></textarea>

            {move || retry_retained.get().then(|| view! {
                <p class="inline-command-note" role="status">
                    "The result is unknown. Retry uses the exact saved disposition."
                </p>
            })}
            {move || error.get().map(|message| {
                view! { <div class="inline-command-error" role="alert">{message}</div> }
            })}
            <div class="command-actions">
                <button
                    class="button quiet-action"
                    type="button"
                    on:click=move |event| on_cancel.run(event)
                    disabled=move || pending.get()
                >
                    "Cancel"
                </button>
                <button
                    class=move || if outcome.get() == InboundInspectionOutcome::Damaged {
                        "button danger-action"
                    } else {
                        "button primary-action"
                    }
                    type="submit"
                    disabled=move || pending.get()
                >
                    <Icon icon=UiIcon::Disposition/>
                    <span>{move || if pending.get() { "Saving" } else { action_label() }}</span>
                </button>
            </div>
        </form>
    }
}

#[component]
pub(super) fn ReleasePanel(
    hold: InventoryHoldResponse,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_confirm: Callback<leptos::ev::MouseEvent>,
    on_cancel: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    view! {
        <div class="release-panel">
            <div class="command-panel-heading danger">
                <span class="command-icon"><Icon icon=UiIcon::Alert/></span>
                <div>
                    <p class="eyebrow">"Release quantity hold"</p>
                    <h2 id="hold-command-title">{format!("Hold #{}", hold.id)}</h2>
                </div>
            </div>
            <dl class="position-facts">
                <div><dt>"Item"</dt><dd>{hold_item_label(&hold)}</dd></div>
                <div><dt>"Client"</dt><dd>{hold.inventory_owner_name.clone()}</dd></div>
                <div><dt>"Position"</dt><dd>{hold_location_label(&hold)}</dd></div>
                <div>
                    <dt>"Quantity"</dt>
                    <dd>{format!("{} {}", format_quantity(hold.quantity), hold.uom)}</dd>
                </div>
                <div class="wide"><dt>"Reason"</dt><dd>{reason_label(hold.reason)}</dd></div>
            </dl>
            {move || error.get().map(|message| {
                view! { <div class="inline-command-error" role="alert">{message}</div> }
            })}
            <div class="command-actions">
                <button
                    class="button quiet-action"
                    type="button"
                    on:click=move |event| on_cancel.run(event)
                    disabled=move || pending.get()
                >
                    "Cancel"
                </button>
                <button
                    class="button danger-action"
                    type="button"
                    on:click=move |event| on_confirm.run(event)
                    disabled=move || pending.get()
                >
                    {move || if pending.get() { "Releasing" } else { "Release hold" }}
                </button>
            </div>
        </div>
    }
}

fn outcome_code(outcome: InboundInspectionOutcome) -> &'static str {
    match outcome {
        InboundInspectionOutcome::Approved => "approved",
        InboundInspectionOutcome::Damaged => "damaged",
    }
}

fn parse_outcome(value: &str) -> Option<InboundInspectionOutcome> {
    match value {
        "approved" => Some(InboundInspectionOutcome::Approved),
        "damaged" => Some(InboundInspectionOutcome::Damaged),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::InventoryHoldReason;

    #[test]
    fn inspection_findings_are_trimmed_and_required() {
        assert_eq!(
            inspection_request(InboundInspectionOutcome::Approved, "  Passed seal check  ")
                .unwrap()
                .note,
            "Passed seal check"
        );
        assert!(inspection_request(InboundInspectionOutcome::Damaged, "   ").is_err());
        assert!(inspection_request(InboundInspectionOutcome::Damaged, &"x".repeat(501)).is_err());
    }

    #[test]
    fn only_active_receipt_quarantine_uses_the_inspection_action() {
        let mut hold = InventoryHoldResponse {
            id: 1,
            created_at: "2026-08-09T12:00:00+00:00".into(),
            created_by_user_id: 2,
            released_at: None,
            released_by_user_id: None,
            inventory_balance_id: 3,
            inventory_owner_id: 4,
            inventory_owner_name: "Northstar Retail".into(),
            facility_id: 5,
            facility_name: Some("Riverside DC".into()),
            location_id: 6,
            location_barcode: Some("QA-01".into()),
            location_name: Some("QA lane".into()),
            license_plate_id: Some(7),
            license_plate_barcode: Some("LP-QA-01".into()),
            item_batch_id: 8,
            lot: Some("LOT-01".into()),
            serial: None,
            expiration: None,
            item_id: 9,
            item_description: Some("Widget case".into()),
            uom: "case".into(),
            inventory_status: InventoryBalanceStatus::Quarantine,
            quantity: 2,
            reason: InventoryHoldReason::QualityInspection,
            note: Some("Inspect seal".into()),
            reference_type: Some("expected_receipt_line".into()),
            reference_id: Some(10),
            status: InventoryHoldStatus::Active,
        };
        assert!(is_receipt_inspection_hold(&hold));
        hold.reference_type = Some("cycle_count".into());
        assert!(!is_receipt_inspection_hold(&hold));
        assert!(retain_inspection_attempt(true));
        assert!(!retain_inspection_attempt(false));
    }
}
