use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{PackContentRemovalReason, RemovePackedContentRequest, Revision};

use crate::components::{Icon, UiIcon};
use crate::view_model::format_quantity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingContentRemoval {
    pub session_id: i64,
    pub order_id: i64,
    pub carton_id: i64,
    pub carton_barcode: String,
    pub content_id: i64,
    pub item_barcodes: Vec<String>,
    pub item_description: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub destination_tote_barcode: String,
    pub quantity: i64,
    pub uom: String,
    pub expected_revision: Revision,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RemovalDraft {
    carton_scan: String,
    item_scan: String,
    lot_scan: String,
    serial_scan: String,
    destination_tote_scan: String,
    reason: String,
    note: String,
}

#[component]
pub(super) fn PackingRemovalDialog(
    selection: PendingContentRemoval,
    pending: Signal<bool>,
    retrying: Signal<bool>,
    command_error: Signal<Option<String>>,
    on_cancel: Callback<()>,
    on_submit: Callback<RemovePackedContentRequest>,
    on_retry: Callback<()>,
) -> impl IntoView {
    let draft = RwSignal::new(RemovalDraft {
        reason: "wrong_carton".to_owned(),
        ..RemovalDraft::default()
    });
    let validation_error = RwSignal::new(None::<String>);
    let selection_for_submit = StoredValue::new(selection.clone());
    let carton_input = NodeRef::<html::Input>::new();
    let item_input = NodeRef::<html::Input>::new();
    let lot_input = NodeRef::<html::Input>::new();
    let serial_input = NodeRef::<html::Input>::new();
    let tote_input = NodeRef::<html::Input>::new();
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
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        if retrying.get_untracked() {
            on_retry.run(());
            return;
        }
        let selection = selection_for_submit.get_value();
        match build_request(&selection, &draft.get_untracked()) {
            Ok(request) => {
                validation_error.set(None);
                on_submit.run(request);
            }
            Err(message) => validation_error.set(Some(message.to_owned())),
        }
    };
    let item_label = selection.item_description.clone().unwrap_or_else(|| {
        format!(
            "Item #{}",
            selection.item_barcodes.first().cloned().unwrap_or_default()
        )
    });
    let trace = match (&selection.lot, &selection.serial) {
        (Some(lot), Some(serial)) => format!("Lot {lot} / Serial {serial}"),
        (Some(lot), None) => format!("Lot {lot}"),
        (None, Some(serial)) => format!("Serial {serial}"),
        (None, None) => "No lot or serial control".to_owned(),
    };
    let quantity = format!("{} {}", format_quantity(selection.quantity), selection.uom);
    let expected_tote = selection.destination_tote_barcode.clone();
    let has_lot = selection.lot.is_some();
    let has_serial = selection.serial.is_some();

    view! {
        <div class="packing-dialog-backdrop">
            <section
                class="packing-removal-dialog"
                role="dialog"
                aria-modal="true"
                aria-labelledby="packing-removal-title"
            >
                <header class="packing-dialog-heading">
                    <span class="packing-dialog-icon"><Icon icon=UiIcon::Reverse/></span>
                    <div>
                        <h2 id="packing-removal-title">"Return content to picked tote"</h2>
                        <span>"The carton must remain open. Every physical identity is re-scanned."</span>
                    </div>
                    <button
                        type="button"
                        class="packing-dialog-close"
                        title="Close"
                        aria-label="Close content removal"
                        disabled=locked
                        on:click=close
                    >
                        <Icon icon=UiIcon::Close/>
                    </button>
                </header>
                <form class="packing-removal-form" on:submit=submit>
                    <dl class="packing-removal-facts">
                        <div><dt>"Item"</dt><dd>{item_label}</dd></div>
                        <div><dt>"Quantity"</dt><dd>{quantity}</dd></div>
                        <div><dt>"Trace"</dt><dd>{trace}</dd></div>
                        <div><dt>"Return tote"</dt><dd>{expected_tote}</dd></div>
                    </dl>
                    <div class="packing-removal-fields">
                        <label>
                            <span>"Carton scan"</span>
                            <input
                                node_ref=carton_input
                                autofocus
                                autocomplete="off"
                                placeholder="SCAN CARTON"
                                disabled=locked
                                prop:value=move || draft.get().carton_scan
                                on:input=move |event| draft.update(|value| value.carton_scan = event_target_value(&event))
                                on:keydown=move |event| {
                                    if event.key() == "Enter" {
                                        event.prevent_default();
                                        if let Some(input) = item_input.get() {
                                            let _ = input.focus();
                                        }
                                    }
                                }
                            />
                        </label>
                        <label>
                            <span>"Item scan"</span>
                            <input
                                node_ref=item_input
                                autocomplete="off"
                                placeholder="SCAN ITEM"
                                disabled=locked
                                prop:value=move || draft.get().item_scan
                                on:input=move |event| draft.update(|value| value.item_scan = event_target_value(&event))
                                on:keydown=move |event| {
                                    if event.key() == "Enter" {
                                        event.prevent_default();
                                        let next = if has_lot {
                                            lot_input.get()
                                        } else if has_serial {
                                            serial_input.get()
                                        } else {
                                            tote_input.get()
                                        };
                                        if let Some(input) = next {
                                            let _ = input.focus();
                                        }
                                    }
                                }
                            />
                        </label>
                        <Show when=move || has_lot>
                            <label>
                                <span>"Lot scan"</span>
                                <input
                                    node_ref=lot_input
                                    autocomplete="off"
                                    placeholder="SCAN LOT"
                                    disabled=locked
                                    prop:value=move || draft.get().lot_scan
                                    on:input=move |event| draft.update(|value| value.lot_scan = event_target_value(&event))
                                    on:keydown=move |event| {
                                        if event.key() == "Enter" {
                                            event.prevent_default();
                                            let next = if has_serial { serial_input.get() } else { tote_input.get() };
                                            if let Some(input) = next {
                                                let _ = input.focus();
                                            }
                                        }
                                    }
                                />
                            </label>
                        </Show>
                        <Show when=move || has_serial>
                            <label>
                                <span>"Serial scan"</span>
                                <input
                                    node_ref=serial_input
                                    autocomplete="off"
                                    placeholder="SCAN SERIAL"
                                    disabled=locked
                                    prop:value=move || draft.get().serial_scan
                                    on:input=move |event| draft.update(|value| value.serial_scan = event_target_value(&event))
                                    on:keydown=move |event| {
                                        if event.key() == "Enter" {
                                            event.prevent_default();
                                            if let Some(input) = tote_input.get() {
                                                let _ = input.focus();
                                            }
                                        }
                                    }
                                />
                            </label>
                        </Show>
                        <label class="wide">
                            <span>"Destination tote scan"</span>
                            <input
                                node_ref=tote_input
                                autocomplete="off"
                                placeholder="SCAN ORIGINAL PICKED TOTE"
                                disabled=locked
                                prop:value=move || draft.get().destination_tote_scan
                                on:input=move |event| draft.update(|value| value.destination_tote_scan = event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span>"Reason"</span>
                            <select
                                disabled=locked
                                prop:value=move || draft.get().reason
                                on:change=move |event| draft.update(|value| value.reason = event_target_value(&event))
                            >
                                <option value="wrong_carton">"Wrong carton"</option>
                                <option value="wrong_item">"Wrong item"</option>
                                <option value="quality_issue">"Quality issue"</option>
                                <option value="damaged_carton">"Damaged carton"</option>
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
                            "The result is unknown. Retry sends the exact saved scans, reason, note, revision, and idempotency key."
                        </p>
                    </Show>
                    <div class="form-actions packing-dialog-actions">
                        <button type="button" class="button secondary-action" disabled=locked on:click=close>
                            "Cancel"
                        </button>
                        <button type="submit" class="button danger-action" disabled=move || pending.get()>
                            <Icon icon=UiIcon::Reverse/>
                            {move || if pending.get() { "Returning" } else if retrying.get() { "Retry exact command" } else { "Return to tote" }}
                        </button>
                    </div>
                </form>
            </section>
        </div>
    }
}

fn build_request(
    selection: &PendingContentRemoval,
    draft: &RemovalDraft,
) -> Result<RemovePackedContentRequest, &'static str> {
    let carton_scan = draft.carton_scan.trim();
    if carton_scan != selection.carton_barcode {
        return Err("Scan the active carton barcode exactly.");
    }
    let item_scan = draft.item_scan.trim();
    if !selection
        .item_barcodes
        .iter()
        .any(|barcode| barcode == item_scan)
    {
        return Err("Scan an item barcode assigned to this content.");
    }
    let lot_scan = required_identity_scan("lot", selection.lot.as_deref(), &draft.lot_scan)?;
    let serial_scan =
        required_identity_scan("serial", selection.serial.as_deref(), &draft.serial_scan)?;
    let destination_tote = draft.destination_tote_scan.trim();
    if destination_tote != selection.destination_tote_barcode {
        return Err("Scan the original picked tote barcode exactly.");
    }
    let reason = match draft.reason.as_str() {
        "wrong_carton" => PackContentRemovalReason::WrongCarton,
        "wrong_item" => PackContentRemovalReason::WrongItem,
        "quality_issue" => PackContentRemovalReason::QualityIssue,
        "damaged_carton" => PackContentRemovalReason::DamagedCarton,
        "other" => PackContentRemovalReason::Other,
        _ => return Err("Select a valid removal reason."),
    };
    let note = draft.note.trim();
    if reason == PackContentRemovalReason::Other && note.is_empty() {
        return Err("A note is required when the reason is Other.");
    }
    if note.len() > 500 {
        return Err("The removal note must be 500 characters or fewer.");
    }
    Ok(RemovePackedContentRequest {
        carton_barcode: carton_scan.to_owned(),
        item_barcode: item_scan.to_owned(),
        lot_scan,
        serial_scan,
        destination_license_plate_barcode: destination_tote.to_owned(),
        reason,
        note: (!note.is_empty()).then(|| note.to_owned()),
        expected_revision: selection.expected_revision,
    })
}

fn required_identity_scan(
    label: &'static str,
    expected: Option<&str>,
    scanned: &str,
) -> Result<Option<String>, &'static str> {
    let scanned = scanned.trim();
    match expected {
        Some(expected) if scanned == expected => Ok(Some(scanned.to_owned())),
        Some(_) if label == "lot" => Err("Scan the content lot exactly."),
        Some(_) => Err("Scan the content serial exactly."),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> PendingContentRemoval {
        PendingContentRemoval {
            session_id: 1,
            order_id: 2,
            carton_id: 3,
            carton_barcode: "CARTON-1".to_owned(),
            content_id: 4,
            item_barcodes: vec!["SKU-1".to_owned(), "UPC-1".to_owned()],
            item_description: Some("Controlled item".to_owned()),
            lot: Some("LOT-1".to_owned()),
            serial: Some("SERIAL-1".to_owned()),
            destination_tote_barcode: "TOTE-1".to_owned(),
            quantity: 1,
            uom: "each".to_owned(),
            expected_revision: Revision::new(7).expect("revision"),
        }
    }

    #[test]
    fn request_requires_all_physical_identity_and_other_note() {
        let selected = selection();
        let mut draft = RemovalDraft {
            carton_scan: "CARTON-1".to_owned(),
            item_scan: "UPC-1".to_owned(),
            lot_scan: "LOT-1".to_owned(),
            serial_scan: "SERIAL-1".to_owned(),
            destination_tote_scan: "TOTE-1".to_owned(),
            reason: "other".to_owned(),
            note: String::new(),
        };
        assert_eq!(
            build_request(&selected, &draft),
            Err("A note is required when the reason is Other.")
        );
        draft.note = "Mispacked during inspection".to_owned();
        let request = build_request(&selected, &draft).expect("valid request");
        assert_eq!(request.item_barcode, "UPC-1");
        assert_eq!(request.lot_scan.as_deref(), Some("LOT-1"));
        assert_eq!(request.serial_scan.as_deref(), Some("SERIAL-1"));
        assert_eq!(request.destination_license_plate_barcode, "TOTE-1");
        assert_eq!(request.reason, PackContentRemovalReason::Other);
    }

    #[test]
    fn request_rejects_wrong_carton_item_trace_or_tote() {
        let selected = selection();
        let base = RemovalDraft {
            carton_scan: "CARTON-1".to_owned(),
            item_scan: "SKU-1".to_owned(),
            lot_scan: "LOT-1".to_owned(),
            serial_scan: "SERIAL-1".to_owned(),
            destination_tote_scan: "TOTE-1".to_owned(),
            reason: "wrong_carton".to_owned(),
            note: String::new(),
        };
        for (mut draft, expected) in [
            (
                {
                    let mut value = base.clone();
                    value.carton_scan = "WRONG".to_owned();
                    value
                },
                "Scan the active carton barcode exactly.",
            ),
            (
                {
                    let mut value = base.clone();
                    value.item_scan = "WRONG".to_owned();
                    value
                },
                "Scan an item barcode assigned to this content.",
            ),
            (
                {
                    let mut value = base.clone();
                    value.lot_scan = "WRONG".to_owned();
                    value
                },
                "Scan the content lot exactly.",
            ),
            (
                {
                    let mut value = base.clone();
                    value.serial_scan = "WRONG".to_owned();
                    value
                },
                "Scan the content serial exactly.",
            ),
            (
                {
                    let mut value = base.clone();
                    value.destination_tote_scan = "WRONG".to_owned();
                    value
                },
                "Scan the original picked tote barcode exactly.",
            ),
        ] {
            assert_eq!(build_request(&selected, &draft), Err(expected));
            draft.note.clear();
        }
    }
}
