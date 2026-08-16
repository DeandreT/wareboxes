use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{
    OpaqueCursor, PickConfirmationHistoryResponse, PickReversalReason,
    ReversePickConfirmationRequest, ReversePickConfirmationResponse, Revision,
};
use wareboxes_core::models::OrderStatus;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

const MAX_SCAN_LENGTH: usize = 200;
const MAX_NOTE_LENGTH: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReversalAttempt {
    confirmation_id: i64,
    request: ReversePickConfirmationRequest,
    idempotency_key: String,
}

#[derive(Clone, Copy)]
struct HistoryState {
    items: RwSignal<Vec<PickConfirmationHistoryResponse>>,
    next_cursor: RwSignal<Option<OpaqueCursor>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    on_unauthorized: Callback<()>,
}

#[component]
pub(super) fn PickReversalPanel(
    order_id: i64,
    order_revision: i64,
    order_status: OrderStatus,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let items = RwSignal::new(Vec::<PickConfirmationHistoryResponse>::new());
    let next_cursor = RwSignal::new(None::<OpaqueCursor>);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let generation = RwSignal::new(0_u64);
    let selected = RwSignal::new(None::<PickConfirmationHistoryResponse>);
    let state = HistoryState {
        items,
        next_cursor,
        loading,
        error,
        generation,
        on_unauthorized,
    };

    Effect::new(move |_| request_history(order_id, None, false, state));

    let refresh = move |_| {
        selected.set(None);
        request_history(order_id, None, false, state);
    };
    let load_more = move |_| {
        if let Some(cursor) = next_cursor.get_untracked() {
            request_history(order_id, Some(cursor), true, state);
        }
    };
    let reversed = Callback::new(move |_result: ReversePickConfirmationResponse| {
        selected.set(None);
        request_history(order_id, None, false, state);
        on_refreshed.run(order_id);
    });
    let authoritative_refresh = Callback::new(move |_| {
        request_history(order_id, None, false, state);
        on_refreshed.run(order_id);
    });
    let status_allows_reversal = matches!(
        order_status,
        OrderStatus::Processing | OrderStatus::AwaitingPacking
    );

    view! {
        <section class="detail-section pick-reversal-section">
            <div class="detail-section-title pick-history-heading">
                <div>
                    <h3>"Pick execution"</h3>
                    <span>{move || format!("{} confirmations", items.get().len())}</span>
                </div>
                <button
                    type="button"
                    class="button table-action"
                    title="Refresh pick execution"
                    aria-label="Refresh pick execution"
                    disabled=move || loading.get()
                    on:click=refresh
                >
                    <Icon icon=UiIcon::Refresh/>
                </button>
            </div>
            <Show when=move || loading.get() && items.get().is_empty()>
                <p class="pick-history-state" role="status">"Loading pick execution..."</p>
            </Show>
            <Show when=move || error.get().is_some()>
                <p class="inline-command-error pick-history-error" role="alert">
                    {move || error.get().unwrap_or_default()}
                </p>
            </Show>
            <div class="table-scroll pick-history-scroll">
                <table class="data-table detail-table pick-history-table">
                    <thead>
                        <tr>
                            <th>"Item"</th>
                            <th>"Route"</th>
                            <th>"Identity"</th>
                            <th class="numeric">"Qty"</th>
                            <th>"Confirmed"</th>
                            <th>"State"</th>
                            <th><span class="sr-only">"Actions"</span></th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || items
                            .get()
                            .into_iter()
                            .map(|confirmation| {
                                let selectable = status_allows_reversal && confirmation.reversal.is_none();
                                let selected_confirmation = confirmation.clone();
                                let identity = pick_identity(&confirmation);
                                let state = confirmation.reversal.as_ref().map_or("Active", |_| "Reversed");
                                view! {
                                    <tr>
                                        <td>
                                            <strong>{confirmation.item_description}</strong>
                                            <small class="cell-detail">{format!("Item #{} / Confirmation #{}", confirmation.item_id, confirmation.confirmation_id)}</small>
                                        </td>
                                        <td>
                                            <strong>{confirmation.source_location_name}</strong>
                                            <small class="cell-detail">{format!("to {}", confirmation.staged_location_name)}</small>
                                        </td>
                                        <td>{identity}</td>
                                        <td class="numeric strong">{format!("{} {}", format_quantity(confirmation.picked_quantity), confirmation.uom)}</td>
                                        <td>{compact_wire_timestamp(&confirmation.confirmed_at)}</td>
                                        <td>
                                            <span class=if confirmation.reversal.is_some() { "status muted" } else { "status open" }>{state}</span>
                                            {confirmation.reversal.map(|reversal| view! {
                                                <small class="cell-detail">{format!("{} / #{}", reversal_reason_label(reversal.reason), reversal.reversal_id)}</small>
                                            })}
                                        </td>
                                        <td class="pick-history-action-cell">
                                            {selectable.then(|| view! {
                                                <button
                                                    type="button"
                                                    class="button table-action"
                                                    title="Reverse this pick"
                                                    aria-label=format!("Reverse pick confirmation {}", selected_confirmation.confirmation_id)
                                                    on:click=move |_| selected.set(Some(selected_confirmation.clone()))
                                                >
                                                    <Icon icon=UiIcon::Reverse/>
                                                </button>
                                            })}
                                        </td>
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
                <Show when=move || items.get().is_empty() && !loading.get() && error.get().is_none()>
                    <p class="empty-state">"No physical pick confirmations have been recorded."</p>
                </Show>
            </div>
            <Show when=move || next_cursor.get().is_some()>
                <div class="pick-history-pagination">
                    <button
                        type="button"
                        class="button secondary-action"
                        disabled=move || loading.get()
                        on:click=load_more
                    >
                        {move || if loading.get() { "Loading" } else { "Load older" }}
                    </button>
                </div>
            </Show>
        </section>
        {move || selected.get().map(|confirmation| view! {
            <PickReversalDialog
                confirmation
                order_revision
                on_close=Callback::new(move |_| selected.set(None))
                on_reversed=reversed
                on_authoritative_refresh=authoritative_refresh
                on_unauthorized
            />
        })}
    }
}

#[component]
fn PickReversalDialog(
    confirmation: PickConfirmationHistoryResponse,
    order_revision: i64,
    on_close: Callback<()>,
    on_reversed: Callback<ReversePickConfirmationResponse>,
    on_authoritative_refresh: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let staged_location = RwSignal::new(String::new());
    let staged_tote = RwSignal::new(String::new());
    let item_scan = RwSignal::new(String::new());
    let lot_scan = RwSignal::new(String::new());
    let serial_scan = RwSignal::new(String::new());
    let return_location = RwSignal::new(String::new());
    let return_plate = RwSignal::new(String::new());
    let reason = RwSignal::new("mis_pick".to_owned());
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let retry = RwSignal::new(None::<ReversalAttempt>);
    let error = RwSignal::new(None::<String>);
    let invalidated = RwSignal::new(false);
    let toasts = use_toast_bus();
    let confirmation_for_submit = confirmation.clone();
    let locked = move || pending.get() || retry.get().is_some();
    let fields_locked = move || locked() || invalidated.get();
    let staged_location_ref = NodeRef::<html::Input>::new();

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if let Some(input) = staged_location_ref.get() {
            let _ = input.focus();
        }
    });

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
            let expected_order_revision = match Revision::new(order_revision) {
                Ok(revision) => revision,
                Err(_) => {
                    error.set(Some(
                        "The order revision is invalid. Refresh the order.".to_owned(),
                    ));
                    return;
                }
            };
            let request = match reversal_request(
                expected_order_revision,
                &confirmation_for_submit,
                &staged_location.get_untracked(),
                &staged_tote.get_untracked(),
                &item_scan.get_untracked(),
                &lot_scan.get_untracked(),
                &serial_scan.get_untracked(),
                &return_location.get_untracked(),
                &return_plate.get_untracked(),
                &reason.get_untracked(),
                &note.get_untracked(),
            ) {
                Ok(request) => request,
                Err(message) => {
                    error.set(Some(message));
                    return;
                }
            };
            ReversalAttempt {
                confirmation_id: confirmation_for_submit.confirmation_id,
                request,
                idempotency_key: api::new_idempotency_key(),
            }
        };
        retry.set(Some(attempt.clone()));
        pending.set(true);
        error.set(None);
        let confirmation_uom = confirmation_for_submit.uom.clone();
        leptos::task::spawn_local(async move {
            let response = api::reverse_pick_confirmation(
                attempt.confirmation_id,
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
                        "Pick #{} reversed; {} {} returned to directed work.",
                        result.confirmation_id,
                        format_quantity(result.reversed_quantity),
                        confirmation_uom
                    ));
                    on_reversed.run(result);
                    on_close.run(());
                }
                Err(api_error) if api_error.unauthorized => {
                    retry.set(None);
                    on_unauthorized.run(());
                }
                Err(api_error) if api_error.ambiguous_outcome => {
                    error.set(Some(format!(
                        "{} The result is unknown; retry the retained reversal.",
                        api_error.message
                    )));
                    toasts.error(api_error.message);
                }
                Err(api_error) => {
                    retry.set(None);
                    invalidated.set(true);
                    error.set(Some(format!(
                        "{} Authoritative order and pick history are being refreshed.",
                        api_error.message
                    )));
                    toasts.error(api_error.message);
                    on_authoritative_refresh.run(());
                }
            }
        });
    };

    view! {
        <div class="pick-reversal-backdrop">
            <section class="pick-reversal-dialog" role="alertdialog" aria-modal="true" aria-labelledby="pick-reversal-title">
                <header class="pick-reversal-dialog-heading">
                    <span class="pick-reversal-dialog-icon"><Icon icon=UiIcon::Reverse/></span>
                    <div>
                        <h2 id="pick-reversal-title">"Reverse physical pick"</h2>
                        <span>{format!("Confirmation #{} / Task #{} / {} {}", confirmation.confirmation_id, confirmation.task_id, format_quantity(confirmation.picked_quantity), confirmation.uom)}</span>
                    </div>
                    <button type="button" class="pick-reversal-close" title="Close" aria-label="Close reversal" disabled=locked on:click=close><Icon icon=UiIcon::Close/></button>
                </header>
                <form class="pick-reversal-form" on:submit=submit>
                    <dl class="pick-reversal-facts">
                        <div><dt>"Item"</dt><dd>{confirmation.item_description.clone()}</dd></div>
                        <div><dt>"Movement"</dt><dd>{format!("{} to {}", confirmation.source_location_name, confirmation.staged_location_name)}</dd></div>
                        <div><dt>"Identity"</dt><dd>{pick_identity(&confirmation)}</dd></div>
                        <div><dt>"Confirmed"</dt><dd>{compact_wire_timestamp(&confirmation.confirmed_at)}</dd></div>
                    </dl>
                    <div class="pick-reversal-scan-grid">
                        <label><span>"Staged location scan"</span><input autofocus=true node_ref=staged_location_ref disabled=fields_locked prop:value=move || staged_location.get() on:input=move |event| staged_location.set(event_target_value(&event))/></label>
                        <label><span>"Staged tote scan"</span><input disabled=fields_locked prop:value=move || staged_tote.get() on:input=move |event| staged_tote.set(event_target_value(&event))/></label>
                        <label><span>"Item scan"</span><input disabled=fields_locked prop:value=move || item_scan.get() on:input=move |event| item_scan.set(event_target_value(&event))/></label>
                        <Show when=move || confirmation.lot.is_some()><label><span>"Lot scan"</span><input disabled=fields_locked prop:value=move || lot_scan.get() on:input=move |event| lot_scan.set(event_target_value(&event))/></label></Show>
                        <Show when=move || confirmation.serial.is_some()><label><span>"Serial scan"</span><input disabled=fields_locked prop:value=move || serial_scan.get() on:input=move |event| serial_scan.set(event_target_value(&event))/></label></Show>
                        <label><span>"Return location scan"</span><input disabled=fields_locked prop:value=move || return_location.get() on:input=move |event| return_location.set(event_target_value(&event))/></label>
                        <Show when=move || confirmation.source_license_plate_required><label><span>"Return license plate scan"</span><input disabled=fields_locked prop:value=move || return_plate.get() on:input=move |event| return_plate.set(event_target_value(&event))/></label></Show>
                    </div>
                    <div class="pick-reversal-reason-grid">
                        <label><span>"Reason"</span><select disabled=fields_locked prop:value=move || reason.get() on:change=move |event| reason.set(event_target_value(&event))><option value="mis_pick">"Mis-pick"</option><option value="wrong_quantity">"Wrong quantity"</option><option value="wrong_lot_or_serial">"Wrong lot or serial"</option><option value="damaged_during_pick">"Damaged during pick"</option><option value="order_exception">"Order exception"</option><option value="other">"Other"</option></select></label>
                        <label><span>{move || if reason.get() == "other" { "Note (required)" } else { "Note (optional)" }}</span><textarea rows="2" maxlength=MAX_NOTE_LENGTH disabled=fields_locked prop:value=move || note.get() on:input=move |event| note.set(event_target_value(&event))></textarea></label>
                    </div>
                    <p class="pick-reversal-warning">"This posts an equal-and-opposite inventory move and reopens the original RF pick work."</p>
                    <Show when=move || error.get().is_some()><p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p></Show>
                    <Show when=move || retry.get().is_some()><p class="inline-command-note" role="status">"The exact scans, request, and idempotency key are retained for retry."</p></Show>
                    <div class="form-actions pick-reversal-actions">
                        <button type="button" class="button secondary-action" disabled=locked on:click=close>"Keep pick"</button>
                        <button type="submit" class="button danger-action" disabled=move || pending.get() || invalidated.get()><Icon icon=if retry.get().is_some() { UiIcon::Refresh } else { UiIcon::Reverse }/>{move || if pending.get() { "Reversing" } else if retry.get().is_some() { "Retry exact reversal" } else { "Reverse pick" }}</button>
                    </div>
                </form>
            </section>
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
fn reversal_request(
    expected_order_revision: Revision,
    confirmation: &PickConfirmationHistoryResponse,
    staged_location: &str,
    staged_tote: &str,
    item_scan: &str,
    lot_scan: &str,
    serial_scan: &str,
    return_location: &str,
    return_plate: &str,
    reason: &str,
    note: &str,
) -> Result<ReversePickConfirmationRequest, String> {
    let staged_location_barcode = required_scan(staged_location, "staged location")?;
    let staged_license_plate_barcode = required_scan(staged_tote, "staged tote")?;
    let item_barcode = required_scan(item_scan, "item")?;
    let lot_scan = optional_required_scan(lot_scan, "lot", confirmation.lot.is_some())?;
    let serial_scan = optional_required_scan(serial_scan, "serial", confirmation.serial.is_some())?;
    let return_location_barcode = required_scan(return_location, "return location")?;
    let return_license_plate_barcode = optional_required_scan(
        return_plate,
        "return license plate",
        confirmation.source_license_plate_required,
    )?;
    let reason = match reason {
        "mis_pick" => PickReversalReason::MisPick,
        "wrong_quantity" => PickReversalReason::WrongQuantity,
        "wrong_lot_or_serial" => PickReversalReason::WrongLotOrSerial,
        "damaged_during_pick" => PickReversalReason::DamagedDuringPick,
        "order_exception" => PickReversalReason::OrderException,
        "other" => PickReversalReason::Other,
        _ => return Err("Select a valid reversal reason.".to_owned()),
    };
    let note = note.trim();
    if note.chars().count() > MAX_NOTE_LENGTH || note.chars().any(char::is_control) {
        return Err(format!(
            "Note must be control-free and cannot exceed {MAX_NOTE_LENGTH} characters."
        ));
    }
    let note = (!note.is_empty()).then(|| note.to_owned());
    if reason == PickReversalReason::Other && note.is_none() {
        return Err("Enter a note when the reason is Other.".to_owned());
    }
    Ok(ReversePickConfirmationRequest {
        expected_order_revision,
        staged_location_barcode,
        staged_license_plate_barcode,
        item_barcode,
        lot_scan,
        serial_scan,
        return_location_barcode,
        return_license_plate_barcode,
        reason,
        note,
    })
}

fn required_scan(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("Scan the {label}."));
    }
    if value.chars().count() > MAX_SCAN_LENGTH || value.chars().any(char::is_control) {
        return Err(format!("The {label} scan is invalid."));
    }
    Ok(value.to_owned())
}

fn optional_required_scan(
    value: &str,
    label: &str,
    required: bool,
) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return if required {
            Err(format!("Scan the {label}."))
        } else {
            Ok(None)
        };
    }
    if !required {
        return Err(format!("A {label} scan is not expected for this pick."));
    }
    required_scan(value, label).map(Some)
}

fn request_history(order_id: i64, cursor: Option<OpaqueCursor>, append: bool, state: HistoryState) {
    let request_generation = state.generation.get_untracked().wrapping_add(1);
    state.generation.set(request_generation);
    state.loading.set(true);
    state.error.set(None);
    leptos::task::spawn_local(async move {
        match api::pick_confirmation_history(order_id, cursor.as_ref()).await {
            Ok(page) if state.generation.get_untracked() == request_generation => {
                if append {
                    state.items.update(|items| items.extend(page.items));
                } else {
                    state.items.set(page.items);
                }
                state.next_cursor.set(page.next_cursor);
                state.loading.set(false);
            }
            Ok(_) => {}
            Err(_api_error) if state.generation.get_untracked() != request_generation => {}
            Err(api_error) if api_error.unauthorized => state.on_unauthorized.run(()),
            Err(api_error) => {
                state.error.set(Some(api_error.message));
                state.loading.set(false);
            }
        }
    });
}

fn pick_identity(confirmation: &PickConfirmationHistoryResponse) -> String {
    match (&confirmation.lot, &confirmation.serial) {
        (Some(lot), Some(serial)) => format!("Lot {lot} / Serial {serial}"),
        (Some(lot), None) => format!("Lot {lot}"),
        (None, Some(serial)) => format!("Serial {serial}"),
        (None, None) => "Uncontrolled".to_owned(),
    }
}

fn reversal_reason_label(reason: PickReversalReason) -> &'static str {
    match reason {
        PickReversalReason::MisPick => "Mis-pick",
        PickReversalReason::WrongQuantity => "Wrong quantity",
        PickReversalReason::WrongLotOrSerial => "Wrong lot or serial",
        PickReversalReason::DamagedDuringPick => "Damaged during pick",
        PickReversalReason::OrderException => "Order exception",
        PickReversalReason::Other => "Other",
    }
}

fn compact_wire_timestamp(value: &str) -> String {
    value
        .replace('T', " ")
        .split(['+', 'Z'])
        .next()
        .unwrap_or(value)
        .chars()
        .take(16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirmation() -> PickConfirmationHistoryResponse {
        PickConfirmationHistoryResponse {
            confirmation_id: 1,
            task_id: 2,
            content_id: 3,
            order_id: 4,
            item_id: 5,
            item_description: "Controlled item".to_owned(),
            uom: "case".to_owned(),
            lot: Some("LOT-A".to_owned()),
            serial: Some("SER-1".to_owned()),
            picked_quantity: 1,
            source_location_id: 6,
            source_location_name: "Reserve A".to_owned(),
            source_license_plate_required: true,
            staged_location_id: 7,
            staged_location_name: "Stage 1".to_owned(),
            staged_license_plate_id: 8,
            pick_policy: wareboxes_api_contract::v1::PickDecisionPolicyResponse {
                source: wareboxes_api_contract::v1::PickDecisionPolicySource::ProductDefault,
                configuration_id: None,
                configuration_revision: None,
                configuration_scope: None,
                require_source_location_scan: true,
                require_item_scan: true,
                require_destination_container_scan: true,
                policy_hash: wareboxes_api_contract::v1::PRODUCT_DEFAULT_PICK_DECISION_POLICY_HASH
                    .to_owned(),
            },
            source_location_scan_verified: true,
            item_scan_verified: true,
            destination_container_scan_verified: true,
            confirmed_by: 9,
            confirmed_at: "2026-08-08T12:34:56+00:00".to_owned(),
            reversal: None,
        }
    }

    #[test]
    fn controlled_pick_requires_every_identity_and_location_scan() {
        let revision = Revision::new(4).unwrap();
        assert_eq!(
            reversal_request(
                revision,
                &confirmation(),
                "STAGE-1",
                "TOTE-1",
                "ITEM-1",
                "",
                "SER-1",
                "RESERVE-A",
                "LP-1",
                "mis_pick",
                "",
            ),
            Err("Scan the lot.".to_owned())
        );
    }

    #[test]
    fn other_reason_requires_a_bounded_note_and_preserves_scans() {
        let request = reversal_request(
            Revision::new(4).unwrap(),
            &confirmation(),
            " STAGE-1 ",
            "TOTE-1",
            "ITEM-1",
            "LOT-A",
            "SER-1",
            "RESERVE-A",
            "LP-1",
            "other",
            " Supervisor verified correction ",
        )
        .unwrap();
        assert_eq!(request.staged_location_barcode, "STAGE-1");
        assert_eq!(request.lot_scan.as_deref(), Some("LOT-A"));
        assert_eq!(
            request.note.as_deref(),
            Some("Supervisor verified correction")
        );
        assert_eq!(request.reason, PickReversalReason::Other);
    }
}
