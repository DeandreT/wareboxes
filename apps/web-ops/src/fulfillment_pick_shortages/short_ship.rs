use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AcceptPickShortageAsShortShipRequest, PickShortShipReason, PickShortageResponse,
    PickShortageStatus,
};

use super::{refresh_after_command, DetailSignals, PendingShortageCommand};
use crate::api;
use crate::components::{Icon, UiIcon};
use crate::view_model::format_quantity;

pub(super) type ShortShipRetry = (i64, AcceptPickShortageAsShortShipRequest, String);

#[component]
pub(super) fn ShortShipConfirmation(
    shortage: PickShortageResponse,
    signals: DetailSignals,
) -> impl IntoView {
    let shortage_id = shortage.shortage_id;
    let open_quantity = shortage.remaining_to_allocate_quantity;
    let hold_id = shortage.hold.hold_id;
    let confirmation_ref = NodeRef::<leptos::html::Form>::new();
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        let _ = signals.short_ship_error.get();
        if let Some(form) = confirmation_ref.get() {
            form.scroll_into_view_with_bool(false);
        }
    });
    let retry_for_shortage = move || {
        signals
            .short_ship_retry
            .get()
            .is_some_and(|(retry_id, _, _)| retry_id == shortage_id)
    };
    let pending = move || {
        signals.command_pending.get() == Some(PendingShortageCommand::ShortShip(shortage_id))
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        dispatch_short_ship(shortage_id, signals);
    };
    let close = move |_| {
        signals.short_ship_error.set(None);
        signals.short_ship_open.set(None);
    };

    view! {
        <form
            node_ref=confirmation_ref
            class="confirmation-panel short-ship-confirmation"
            role="alertdialog"
            aria-labelledby="short-ship-title"
            aria-describedby="short-ship-consequence short-ship-hold"
            on:submit=submit
        >
            <div class="short-ship-heading">
                <Icon icon=UiIcon::Shipping/>
                <div>
                    <h3 id="short-ship-title">"Accept short shipment"</h3>
                    <span>{format!("{} / Exception #{}", shortage.order_key, shortage_id)}</span>
                </div>
            </div>
            <dl class="short-ship-impact">
                <div><dt>"Accept open"</dt><dd>{format_quantity(open_quantity)}</dd></div>
                <div><dt>"Line reduction"</dt><dd>{format!("-{}", format_quantity(open_quantity))}</dd></div>
                <div><dt>"Order reduction"</dt><dd>{format!("-{}", format_quantity(open_quantity))}</dd></div>
            </dl>
            <p id="short-ship-consequence">
                "All currently open units will be accepted. Original demand remains recorded; packing and shipping use the reduced effective demand."
            </p>
            <p id="short-ship-hold" class="short-ship-hold-warning">
                <Icon icon=UiIcon::Holds/>
                {format!("Discrepancy hold #{hold_id} remains active on the suspect source stock.")}
            </p>
            <div class="short-ship-fields">
                <label>
                    <span>"Reason"</span>
                    <select
                        disabled=move || pending() || retry_for_shortage()
                        prop:value=move || signals.short_ship_reason.get()
                        on:change=move |event| {
                            signals.short_ship_reason.set(event_target_value(&event));
                            signals.short_ship_error.set(None);
                        }
                    >
                        <option value="client_authorized">"Client authorized"</option>
                        <option value="inventory_unavailable">"Inventory unavailable"</option>
                        <option value="ship_by_commitment">"Ship-by commitment"</option>
                        <option value="other">"Other"</option>
                    </select>
                </label>
                <label>
                    <span>{move || if signals.short_ship_reason.get() == "other" { "Note (required)" } else { "Note (optional)" }}</span>
                    <textarea
                        maxlength="500"
                        rows="2"
                        aria-required=move || (signals.short_ship_reason.get() == "other").to_string()
                        disabled=move || pending() || retry_for_shortage()
                        prop:value=move || signals.short_ship_note.get()
                        on:input=move |event| {
                            signals.short_ship_note.set(event_target_value(&event));
                            signals.short_ship_error.set(None);
                        }
                    ></textarea>
                </label>
            </div>
            <Show when=retry_for_shortage>
                <p class="shortage-retry-note" role="status">
                    "The original request is retained. Retry sends the exact request and idempotency key."
                </p>
            </Show>
            <Show when=move || signals.short_ship_error.get().is_some()>
                <p class="inline-command-error" role="alert">
                    {move || signals.short_ship_error.get().unwrap_or_default()}
                </p>
            </Show>
            <div class="form-actions">
                <button type="submit" class="button danger-action" disabled=pending>
                    <Icon icon=UiIcon::Shipping/>
                    {move || if pending() { "Accepting" } else if retry_for_shortage() { "Retry short shipment" } else { "Accept short shipment" }}
                </button>
                <button
                    type="button"
                    class="button secondary-action"
                    disabled=move || pending() || retry_for_shortage()
                    on:click=close
                >
                    "Keep recovering"
                </button>
            </div>
        </form>
    }
}

fn dispatch_short_ship(shortage_id: i64, signals: DetailSignals) {
    if signals.command_pending.get_untracked().is_some() {
        return;
    }
    if let Some((retry_id, _, _)) = signals.substitution_retry.get_untracked() {
        signals.short_ship_error.set(Some(format!(
            "Resolve the unknown substitution result for pick exception #{retry_id} first."
        )));
        return;
    }
    if let Some((retry_id, _, _)) = signals.reallocation_retry.get_untracked() {
        signals.short_ship_error.set(Some(format!(
            "Resolve the unknown recovery result for pick exception #{retry_id} first."
        )));
        return;
    }
    let (request, idempotency_key) = match signals.short_ship_retry.get_untracked() {
        Some((retry_id, request, key)) if retry_id == shortage_id => (request, key),
        Some((retry_id, _, _)) => {
            signals.short_ship_error.set(Some(format!(
                "Resolve the unknown short-shipment result for pick exception #{retry_id} first."
            )));
            return;
        }
        None => {
            let Some(shortage) = signals.selected.get_untracked() else {
                return;
            };
            if shortage.shortage_id != shortage_id
                || shortage.status != PickShortageStatus::AwaitingInventory
                || shortage.remaining_to_allocate_quantity <= 0
            {
                signals.short_ship_error.set(Some(
                    "This exception is no longer eligible for short shipment. Refresh it."
                        .to_owned(),
                ));
                return;
            }
            let reason_value = signals.short_ship_reason.get_untracked();
            let Some(reason) = parse_short_ship_reason(&reason_value) else {
                signals
                    .short_ship_error
                    .set(Some("Select a short-shipment reason.".to_owned()));
                return;
            };
            let note =
                match validate_short_ship_note(reason, &signals.short_ship_note.get_untracked()) {
                    Ok(note) => note,
                    Err(message) => {
                        signals.short_ship_error.set(Some(message));
                        return;
                    }
                };
            (
                AcceptPickShortageAsShortShipRequest {
                    expected_shortage_revision: shortage.shortage_revision,
                    expected_order_revision: shortage.order_revision,
                    reason,
                    note,
                },
                api::new_idempotency_key(),
            )
        }
    };
    signals.short_ship_retry.set(Some((
        shortage_id,
        request.clone(),
        idempotency_key.clone(),
    )));
    signals
        .command_pending
        .set(Some(PendingShortageCommand::ShortShip(shortage_id)));
    signals.short_ship_error.set(None);

    leptos::task::spawn_local(async move {
        let response =
            api::accept_pick_shortage_as_short_ship(shortage_id, &request, &idempotency_key).await;
        if signals.command_pending.get_untracked()
            == Some(PendingShortageCommand::ShortShip(shortage_id))
        {
            signals.command_pending.set(None);
        }
        match response {
            Ok(result) => {
                if signals
                    .short_ship_retry
                    .get_untracked()
                    .is_some_and(|(retry_id, _, _)| retry_id == shortage_id)
                {
                    signals.short_ship_retry.set(None);
                }
                signals.short_ship_open.set(None);
                signals.short_ship_error.set(None);
                signals.toasts.success(format!(
                    "Accepted {} units short for order #{}. Effective demand is {} of {}; discrepancy hold #{} remains active.",
                    format_quantity(result.accepted_short_quantity),
                    result.order_id,
                    format_quantity(result.order_demand.effective),
                    format_quantity(result.order_demand.ordered),
                    result.inventory_hold_id,
                ));
                refresh_after_command(shortage_id, signals);
            }
            Err(api_error) if api_error.unauthorized => {
                signals.short_ship_retry.set(None);
                signals.on_unauthorized.run(());
            }
            Err(api_error) if api_error.ambiguous_outcome => {
                signals.short_ship_error.set(Some(format!(
                    "{} The result is unknown; retry the saved short-shipment command.",
                    api_error.message
                )));
                signals.toasts.error(api_error.message);
            }
            Err(api_error) => {
                signals.short_ship_retry.set(None);
                signals.short_ship_error.set(Some(format!(
                    "{} Authoritative shortage revisions were refreshed.",
                    api_error.message
                )));
                signals.toasts.error(api_error.message);
                refresh_after_command(shortage_id, signals);
            }
        }
    });
}

fn parse_short_ship_reason(value: &str) -> Option<PickShortShipReason> {
    match value {
        "client_authorized" => Some(PickShortShipReason::ClientAuthorized),
        "inventory_unavailable" => Some(PickShortShipReason::InventoryUnavailable),
        "ship_by_commitment" => Some(PickShortShipReason::ShipByCommitment),
        "other" => Some(PickShortShipReason::Other),
        _ => None,
    }
}

fn validate_short_ship_note(
    reason: PickShortShipReason,
    value: &str,
) -> Result<Option<String>, String> {
    let note = value.trim();
    if note.chars().count() > 500 {
        return Err("Short-shipment note cannot exceed 500 characters.".to_owned());
    }
    if reason == PickShortShipReason::Other && note.is_empty() {
        return Err("Add a note when the short-shipment reason is Other.".to_owned());
    }
    Ok((!note.is_empty()).then(|| note.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_uses_typed_reasons_and_validated_notes() {
        assert_eq!(
            parse_short_ship_reason("inventory_unavailable"),
            Some(PickShortShipReason::InventoryUnavailable)
        );
        assert_eq!(parse_short_ship_reason("override"), None);
        assert_eq!(
            validate_short_ship_note(PickShortShipReason::ClientAuthorized, " approved "),
            Ok(Some("approved".to_owned()))
        );
        assert!(validate_short_ship_note(PickShortShipReason::Other, " ").is_err());
        assert!(validate_short_ship_note(
            PickShortShipReason::InventoryUnavailable,
            &"x".repeat(501),
        )
        .is_err());
    }
}
