use leptos::prelude::*;
use lucide_leptos::RefreshCw;
use wareboxes_api_contract::v1::{
    ExpectedReceiptLine, ExpectedReceivingLoadStatus, ExpectedReceivingSessionResponse,
};

use crate::api;
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
                view! {
                    <div class="receiving-execution-context">
                        <div>
                            <span>"Load scan"</span>
                            <strong class="mono">{execution_barcode.get_value()}</strong>
                        </div>
                        <div>
                            <span>"Directed destination"</span>
                            <strong>{location_name}</strong>
                            <small class="mono">{location_barcode}</small>
                        </div>
                        <div>
                            <span>"Execution state"</span>
                            <strong class=receiving_status_class(status)>{receiving_status_label(status)}</strong>
                        </div>
                    </div>
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
}
