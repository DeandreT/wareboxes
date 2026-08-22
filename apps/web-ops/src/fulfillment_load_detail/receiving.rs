use leptos::{html, prelude::*};
use lucide_leptos::RefreshCw;
use wareboxes_api_contract::v1::{
    ExpectedReceiptLine, ExpectedReceivingLoadStatus, ExpectedReceivingSessionResponse,
    ReceiptPolicyResponse, ReceiptPolicySource, StartInboundLoadUnloadingRequest,
};

use crate::api;
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

#[derive(Clone, Copy)]
struct ReceivingState {
    session: RwSignal<Option<ExpectedReceivingSessionResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    on_unauthorized: Callback<()>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ReceivingTotals {
    expected: i64,
    received: i64,
    rejected: i64,
    missing: i64,
    remaining: i64,
}

#[component]
pub(super) fn ReceivingExecutionPanel(
    load_id: i64,
    execution_barcode: String,
    seal_number: Option<String>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let state = ReceivingState {
        session: RwSignal::new(None),
        loading: RwSignal::new(false),
        error: RwSignal::new(None),
        generation: RwSignal::new(0),
        on_unauthorized,
    };
    let execution_barcode = StoredValue::new(execution_barcode);
    let seal_number = StoredValue::new(seal_number);
    let unloading_open = RwSignal::new(false);

    Effect::new(move |_| request_session(load_id, state));

    view! {
        <section class="detail-section receiving-execution-section">
            <div class="detail-section-title receiving-execution-title">
                <div>
                    <h3>"Receiving execution"</h3>
                    <span>{move || state.session.get().as_ref().map_or_else(
                        || "Awaiting executable session".to_owned(),
                        |session| format!("{} expected lines", session.lines.len()),
                    )}</span>
                </div>
                <button
                    type="button"
                    class="icon-button compact"
                    title="Refresh receiving execution"
                    aria-label="Refresh receiving execution"
                    disabled=move || state.loading.get()
                    on:click=move |_| request_session(load_id, state)
                >
                    <RefreshCw size=14/>
                </button>
            </div>

            <Show when=move || state.loading.get() && state.session.get().is_none()>
                <p class="receiving-execution-state" role="status">"Loading receiving execution..."</p>
            </Show>
            <Show when=move || state.error.get().is_some()>
                <p class="inline-command-error receiving-execution-error" role="alert">
                    {move || state.error.get().unwrap_or_default()}
                </p>
            </Show>

            {move || state.session.get().map(|session| {
                let totals = receiving_totals(&session.lines);
                let location_name = session.receiving_location.name.clone().unwrap_or_else(|| {
                    format!("Location #{}", session.receiving_location.location_id)
                });
                let location_barcode = session.receiving_location.barcode.clone();
                let status = session.status;
                let policy_title = receipt_policy_title(&session.receipt_policy);
                let policy_detail = receipt_policy_detail(&session.receipt_policy);
                let unexpected_blocked = !session.receipt_policy.allow_unexpected;
                view! {
                    <div class="receiving-execution-context">
                        <div>
                            <span>"Load scan"</span>
                            <strong class="mono">{execution_barcode.get_value()}</strong>
                        </div>
                        <div>
                            <span>"Directed destination"</span>
                            <strong>{location_name}</strong>
                            <small class="mono">{location_barcode.clone()}</small>
                        </div>
                        <div>
                            <span>"Execution state"</span>
                            <strong class=receiving_status_class(status)>{receiving_status_label(status)}</strong>
                        </div>
                        <div class:receiving-policy-blocked=unexpected_blocked>
                            <span>"Unexpected stock policy"</span>
                            <strong>{policy_title}</strong>
                            <small>{policy_detail}</small>
                        </div>
                    </div>
                    <Show when=move || status == ExpectedReceivingLoadStatus::Arrived && !unloading_open.get()>
                        <div class="receiving-start-action">
                            <div>
                                <strong>"Ready to unload"</strong>
                                <span>"Verify the load, dock, and planned seal before physical unloading begins."</span>
                            </div>
                            <button
                                type="button"
                                class="button primary-action"
                                on:click=move |_| unloading_open.set(true)
                            >
                                "Start unloading"
                            </button>
                        </div>
                    </Show>
                    <Show when=move || status == ExpectedReceivingLoadStatus::Arrived && unloading_open.get()>
                        <UnloadingConfirmation
                            load_id
                            expected_load_scan=execution_barcode.get_value()
                            expected_location_scan=location_barcode.clone()
                            expected_seal=seal_number.get_value()
                            on_close=Callback::new(move |_| unloading_open.set(false))
                            on_started=Callback::new(move |_| {
                                unloading_open.set(false);
                                request_session(load_id, state);
                                on_refreshed.run(load_id);
                            })
                            on_unauthorized
                        />
                    </Show>
                    <div class="receiving-progress-strip">
                        <ReceivingMetric label="Expected" value=totals.expected/>
                        <ReceivingMetric label="Received" value=totals.received/>
                        <ReceivingMetric label="Rejected" value=totals.rejected/>
                        <ReceivingMetric label="Missing" value=totals.missing/>
                        <ReceivingMetric label="Open" value=totals.remaining emphasis=true/>
                    </div>
                    <div class="table-scroll receiving-lines-scroll">
                        <table class="data-table detail-table receiving-lines-table">
                            <thead>
                                <tr>
                                    <th>"Item"</th>
                                    <th class="numeric">"Open"</th>
                                    <th>"Trace"</th>
                                    <th>"Accepted scans"</th>
                                    <th class="numeric">"Expected"</th>
                                    <th class="numeric">"Received"</th>
                                    <th class="numeric">"Rejected"</th>
                                    <th class="numeric">"Missing"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {session.lines.into_iter().map(receiving_line_row).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
            })}
        </section>
    }
}

#[component]
fn UnloadingConfirmation(
    load_id: i64,
    expected_load_scan: String,
    expected_location_scan: String,
    expected_seal: Option<String>,
    on_close: Callback<()>,
    on_started: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let load_scan = RwSignal::new(String::new());
    let location_scan = RwSignal::new(String::new());
    let seal_scan = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(StartInboundLoadUnloadingRequest, String)>);
    let form_ref = NodeRef::<html::Form>::new();
    let load_ref = NodeRef::<html::Input>::new();
    let toasts = use_toast_bus();
    let seal_required = expected_seal.is_some();
    let expected_load_scan = StoredValue::new(expected_load_scan);
    let expected_location_scan = StoredValue::new(expected_location_scan);
    let expected_seal = StoredValue::new(expected_seal);

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
        let (request, key) = if let Some(saved) = retry.get_untracked() {
            saved
        } else {
            let request = StartInboundLoadUnloadingRequest {
                load_scan: load_scan.get_untracked(),
                receiving_location_scan: location_scan.get_untracked(),
                seal_scan: seal_required.then(|| seal_scan.get_untracked()),
                started_at: None,
            };
            let key = api::new_idempotency_key();
            retry.set(Some((request.clone(), key.clone())));
            (request, key)
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::start_inbound_load_unloading(load_id, &request, &key).await {
                Ok(_) => {
                    retry.set(None);
                    pending.set(false);
                    toasts.success(format!("Unloading started for load #{load_id}."));
                    on_started.run(());
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    if !api_error.ambiguous_outcome {
                        retry.set(None);
                    }
                    error.set(Some(if api_error.ambiguous_outcome {
                        "Unloading outcome is unknown. Retry to reconcile the exact saved scans."
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
            on:submit=submit
        >
            <h3>"Verify unloading start"</h3>
            <div class="evidence-summary">
                <span><strong>"Expected load"</strong> {expected_load_scan.get_value()}</span>
                <span><strong>"Expected dock"</strong> {expected_location_scan.get_value()}</span>
                <Show when=move || expected_seal.get_value().is_some()>
                    <span><strong>"Expected seal"</strong> {expected_seal.get_value().unwrap_or_default()}</span>
                </Show>
            </div>
            <div class="fulfillment-form-grid three-column">
                <label>
                    <span>"Load scan"</span>
                    <input
                        node_ref=load_ref
                        required
                        autocomplete="off"
                        prop:value=move || load_scan.get()
                        on:input=move |event| load_scan.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Dock scan"</span>
                    <input
                        required
                        autocomplete="off"
                        prop:value=move || location_scan.get()
                        on:input=move |event| location_scan.set(event_target_value(&event))
                    />
                </label>
                <Show when=move || seal_required>
                    <label>
                        <span>"Seal scan"</span>
                        <input
                            required
                            autocomplete="off"
                            prop:value=move || seal_scan.get()
                            on:input=move |event| seal_scan.set(event_target_value(&event))
                        />
                    </label>
                </Show>
            </div>
            <Show when=move || error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <div class="form-actions">
                <button type="submit" class="button primary-action" disabled=move || pending.get()>
                    {move || if pending.get() { "Starting" } else { "Confirm unloading" }}
                </button>
                <button type="button" class="button secondary-action" on:click=move |_| on_close.run(()) disabled=move || pending.get()>
                    "Go back"
                </button>
            </div>
        </form>
    }
}

#[component]
fn ReceivingMetric(
    label: &'static str,
    value: i64,
    #[prop(default = false)] emphasis: bool,
) -> impl IntoView {
    view! {
        <div class:attention={emphasis && value > 0}>
            <span>{label}</span>
            <strong>{format_quantity(value)}</strong>
        </div>
    }
}

fn receiving_line_row(line: ExpectedReceiptLine) -> impl IntoView {
    let item = line
        .item_description
        .clone()
        .unwrap_or_else(|| format!("Item #{}", line.item_id));
    let trace = trace_label(&line);
    let scans = if line.item_barcodes.is_empty() {
        "No active barcode".to_owned()
    } else {
        line.item_barcodes.join(" / ")
    };
    let uom = line.uom.clone();
    view! {
        <tr class:receiving-line-complete=line.remaining_quantity == 0>
            <td>
                <strong>{item}</strong>
                <small class="cell-detail">{format!("Line #{} · {uom}", line.load_line_id)}</small>
            </td>
            <td class="numeric strong">{format_quantity(line.remaining_quantity)}</td>
            <td>{trace}</td>
            <td class="mono receiving-line-scans">{scans}</td>
            <td class="numeric">{format_quantity(line.expected_quantity)}</td>
            <td class="numeric receiving-quantity-positive">{format_quantity(line.received_quantity)}</td>
            <td class="numeric receiving-quantity-exception">{format_quantity(line.rejected_quantity)}</td>
            <td class="numeric receiving-quantity-exception">{format_quantity(line.missing_quantity)}</td>
        </tr>
    }
}

fn request_session(load_id: i64, state: ReceivingState) {
    let generation = state.generation.get_untracked().wrapping_add(1);
    state.generation.set(generation);
    state.loading.set(true);
    state.error.set(None);
    leptos::task::spawn_local(async move {
        match api::expected_receiving_session(load_id).await {
            Ok(session) if state.generation.get_untracked() == generation => {
                state.session.set(Some(session));
                state.loading.set(false);
            }
            Ok(_) => {}
            Err(_error) if state.generation.get_untracked() != generation => {}
            Err(error) if error.unauthorized => state.on_unauthorized.run(()),
            Err(error) => {
                state.session.set(None);
                state.error.set(Some(error.message));
                state.loading.set(false);
            }
        }
    });
}

fn receiving_totals(lines: &[ExpectedReceiptLine]) -> ReceivingTotals {
    lines
        .iter()
        .fold(ReceivingTotals::default(), |mut totals, line| {
            totals.expected += line.expected_quantity;
            totals.received += line.received_quantity;
            totals.rejected += line.rejected_quantity;
            totals.missing += line.missing_quantity;
            totals.remaining += line.remaining_quantity;
            totals
        })
}

fn trace_label(line: &ExpectedReceiptLine) -> String {
    let mut parts = Vec::new();
    if let Some(lot) = line.lot.as_deref() {
        parts.push(format!("Lot {lot}"));
    }
    if let Some(serial) = line.serial.as_deref() {
        parts.push(format!("Serial {serial}"));
    }
    if let Some(expiration) = line.expiration.as_deref() {
        parts.push(format!(
            "Exp {}",
            expiration.split('T').next().unwrap_or(expiration)
        ));
    }
    if parts.is_empty() {
        "Uncontrolled".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn receipt_policy_title(policy: &ReceiptPolicyResponse) -> String {
    let source = match policy.source {
        ReceiptPolicySource::ProductDefault => "Product default".to_owned(),
        ReceiptPolicySource::Configuration => format!(
            "Configuration #{} r{}",
            policy.configuration_id.unwrap_or_default(),
            policy.configuration_revision.unwrap_or_default()
        ),
    };
    if policy.allow_unexpected {
        format!("{source} · quarantine")
    } else {
        format!("{source} · blocked")
    }
}

fn receipt_policy_detail(policy: &ReceiptPolicyResponse) -> String {
    let mapping = if policy.quarantine_unmapped_items {
        "unmapped items accepted"
    } else {
        "owner mapping required"
    };
    let percentage = f64::from(policy.over_receipt_tolerance_basis_points) / 100.0;
    format!("{mapping} · excess tolerance {percentage:.2}%")
}

const fn receiving_status_label(status: ExpectedReceivingLoadStatus) -> &'static str {
    match status {
        ExpectedReceivingLoadStatus::Arrived => "Arrived",
        ExpectedReceivingLoadStatus::Receiving => "Receiving",
        ExpectedReceivingLoadStatus::Received => "Received",
    }
}

const fn receiving_status_class(status: ExpectedReceivingLoadStatus) -> &'static str {
    match status {
        ExpectedReceivingLoadStatus::Arrived => "status pending",
        ExpectedReceivingLoadStatus::Receiving => "status processing",
        ExpectedReceivingLoadStatus::Received => "status shipped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(
        expected: i64,
        received: i64,
        rejected: i64,
        missing: i64,
        remaining: i64,
    ) -> ExpectedReceiptLine {
        ExpectedReceiptLine {
            load_line_id: 1,
            item_id: 2,
            item_description: None,
            uom: "case".to_owned(),
            item_barcodes: vec!["CASE-2".to_owned()],
            expected_quantity: expected,
            received_quantity: received,
            rejected_quantity: rejected,
            missing_quantity: missing,
            remaining_quantity: remaining,
            lot: None,
            serial: None,
            expiration: None,
        }
    }

    #[test]
    fn totals_preserve_receiving_resolution_dimensions() {
        let totals = receiving_totals(&[line(10, 6, 1, 0, 3), line(5, 2, 0, 2, 1)]);
        assert_eq!(
            totals,
            ReceivingTotals {
                expected: 15,
                received: 8,
                rejected: 1,
                missing: 2,
                remaining: 4,
            }
        );
    }

    #[test]
    fn trace_label_includes_every_controlled_dimension() {
        let mut line = line(1, 0, 0, 0, 1);
        line.lot = Some("LOT-A".to_owned());
        line.serial = Some("SER-9".to_owned());
        line.expiration = Some("2027-05-04T00:00:00+00:00".to_owned());
        assert_eq!(
            trace_label(&line),
            "Lot LOT-A · Serial SER-9 · Exp 2027-05-04"
        );
    }

    #[test]
    fn receipt_policy_evidence_is_operator_readable() {
        let policy = ReceiptPolicyResponse {
            source: ReceiptPolicySource::Configuration,
            configuration_id: Some(41),
            configuration_revision: Some(3),
            configuration_scope: None,
            allow_unexpected: true,
            quarantine_unmapped_items: false,
            over_receipt_tolerance_basis_points: 250,
            policy_hash: "a".repeat(64),
        };
        assert_eq!(
            receipt_policy_title(&policy),
            "Configuration #41 r3 · quarantine"
        );
        assert_eq!(
            receipt_policy_detail(&policy),
            "owner mapping required · excess tolerance 2.50%"
        );
    }
}
