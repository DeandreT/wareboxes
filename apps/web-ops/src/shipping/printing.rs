use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AutomationCommandStatus, CancelShipmentDocumentPrintRequest, OpaqueCursor,
    PrintShipmentDocumentRequest, ShipmentDocumentPrintJobResponse, ShipmentDocumentResponse,
    ShipmentPrinterDeviceResponse,
};

use crate::api;

const HISTORY_PAGE_SIZE: u16 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingPrint {
    request: PrintShipmentDocumentRequest,
    idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingCancellation {
    job: ShipmentDocumentPrintJobResponse,
    idempotency_key: String,
}

#[component]
pub(super) fn ShipmentDocumentPrintControls(
    document: ShipmentDocumentResponse,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let document_id = document.document_id;
    let content_sha256 = StoredValue::new(document.content_sha256);
    let printers = RwSignal::new(Vec::<ShipmentPrinterDeviceResponse>::new());
    let selected_printer = RwSignal::new(String::new());
    let copies = RwSignal::new("1".to_owned());
    let jobs = RwSignal::new(Vec::<ShipmentDocumentPrintJobResponse>::new());
    let next_cursor = RwSignal::new(None::<OpaqueCursor>);
    let loading = RwSignal::new(true);
    let printing = RwSignal::new(false);
    let active_command_id = RwSignal::new(None::<i64>);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<PendingPrint>);
    let cancellation_retry = RwSignal::new(None::<PendingCancellation>);
    let generation = RwSignal::new(0_u64);

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        let request_generation = generation.get_untracked().saturating_add(1);
        generation.set(request_generation);
        loading.set(true);
        leptos::task::spawn_local(async move {
            let printer_result = api::shipment_document_printers(document_id).await;
            let history_result =
                api::shipment_document_print_jobs(document_id, None, HISTORY_PAGE_SIZE).await;
            if generation.get_untracked() != request_generation {
                return;
            }
            match printer_result {
                Ok(page) => {
                    if selected_printer.get_untracked().is_empty() {
                        selected_printer.set(
                            page.items
                                .first()
                                .map_or_else(String::new, |printer| printer.device_id.to_string()),
                        );
                    }
                    printers.set(page.items);
                }
                Err(api_error) => {
                    if api_error.unauthorized {
                        on_unauthorized.run(());
                    }
                    error.set(Some(api_error.message));
                }
            }
            match history_result {
                Ok(page) => {
                    jobs.set(page.items);
                    next_cursor.set(page.next_cursor);
                }
                Err(api_error) => {
                    if api_error.unauthorized {
                        on_unauthorized.run(());
                    }
                    error.set(Some(api_error.message));
                }
            }
            loading.set(false);
        });
    });

    let dispatch_print = Callback::new(move |pending: PendingPrint| {
        if printing.get_untracked() {
            return;
        }
        printing.set(true);
        error.set(None);
        let retained = pending.clone();
        let request_generation = generation.get_untracked();
        leptos::task::spawn_local(async move {
            match api::print_shipment_document(
                document_id,
                &pending.request,
                &pending.idempotency_key,
            )
            .await
            {
                Ok(response) if generation.get_untracked() == request_generation => {
                    let job = response.print_job;
                    upsert_job(jobs, job.clone());
                    retry.set(None);
                    if terminal(job.status) {
                        printing.set(false);
                        active_command_id.set(None);
                    } else {
                        active_command_id.set(Some(job.command_id));
                        schedule_poll(
                            document_id,
                            job.command_id,
                            request_generation,
                            generation,
                            jobs,
                            printing,
                            active_command_id,
                            error,
                            on_unauthorized,
                        );
                    }
                }
                Err(api_error) if generation.get_untracked() == request_generation => {
                    printing.set(false);
                    active_command_id.set(None);
                    retry.set(api_error.ambiguous_outcome.then_some(retained));
                    error.set(Some(api_error.message));
                    if api_error.unauthorized {
                        on_unauthorized.run(());
                    }
                }
                _ => {}
            }
        });
    });

    let request_print = move |_| {
        let Some(device_id) = selected_printer
            .get_untracked()
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
        else {
            error.set(Some("Select an available facility printer.".to_owned()));
            return;
        };
        let Some(copies) = copies
            .get_untracked()
            .parse::<u16>()
            .ok()
            .filter(|value| (1..=100).contains(value))
        else {
            error.set(Some("Copies must be between 1 and 100.".to_owned()));
            return;
        };
        dispatch_print.run(PendingPrint {
            request: PrintShipmentDocumentRequest {
                device_id,
                copies,
                expected_content_sha256: content_sha256.get_value(),
            },
            idempotency_key: api::new_idempotency_key(),
        });
    };
    let retry_exact = move |_| {
        if let Some(pending) = retry.get_untracked() {
            dispatch_print.run(pending);
        }
    };
    let dispatch_cancellation = Callback::new(move |pending: PendingCancellation| {
        if pending.job.status != AutomationCommandStatus::Queued {
            return;
        }
        let retained = pending.clone();
        let request_generation = generation.get_untracked();
        leptos::task::spawn_local(async move {
            match api::cancel_shipment_document_print(
                document_id,
                pending.job.command_id,
                &CancelShipmentDocumentPrintRequest {
                    expected_revision: pending.job.revision,
                },
                &pending.idempotency_key,
            )
            .await
            {
                Ok(response) if generation.get_untracked() == request_generation => {
                    upsert_job(jobs, response.print_job);
                    cancellation_retry.set(None);
                    if active_command_id.get_untracked() == Some(pending.job.command_id) {
                        active_command_id.set(None);
                        printing.set(false);
                    }
                }
                Err(api_error) if generation.get_untracked() == request_generation => {
                    cancellation_retry.set(api_error.ambiguous_outcome.then_some(retained));
                    error.set(Some(api_error.message));
                    if api_error.unauthorized {
                        on_unauthorized.run(());
                    }
                }
                _ => {}
            }
        });
    });
    let cancel_queued = Callback::new(move |job: ShipmentDocumentPrintJobResponse| {
        dispatch_cancellation.run(PendingCancellation {
            job,
            idempotency_key: api::new_idempotency_key(),
        });
    });
    let retry_cancellation = move |_| {
        if let Some(pending) = cancellation_retry.get_untracked() {
            dispatch_cancellation.run(pending);
        }
    };
    let load_more = move |_| {
        let Some(cursor) = next_cursor.get_untracked() else {
            return;
        };
        loading.set(true);
        let request_generation = generation.get_untracked();
        leptos::task::spawn_local(async move {
            match api::shipment_document_print_jobs(document_id, Some(&cursor), HISTORY_PAGE_SIZE)
                .await
            {
                Ok(page) if generation.get_untracked() == request_generation => {
                    jobs.update(|current| {
                        for job in page.items {
                            if current.iter().all(|item| item.command_id != job.command_id) {
                                current.push(job);
                            }
                        }
                    });
                    next_cursor.set(page.next_cursor);
                }
                Err(api_error) if generation.get_untracked() == request_generation => {
                    error.set(Some(api_error.message));
                    if api_error.unauthorized {
                        on_unauthorized.run(());
                    }
                }
                _ => {}
            }
            loading.set(false);
        });
    };

    view! {
        <div class="shipment-print-controls">
            <div class="shipment-print-form">
                <select
                    aria-label="Facility printer"
                    prop:value=move || selected_printer.get()
                    disabled=move || loading.get() || printing.get()
                    on:change=move |event| selected_printer.set(event_target_value(&event))
                >
                    {move || printers.get().into_iter().map(|printer| view! {
                        <option value=printer.device_id.to_string()>
                            {format!("{} · {}", printer.display_name, printer.device_key)}
                        </option>
                    }).collect_view()}
                </select>
                <label><span>"Copies"</span><input inputmode="numeric" prop:value=move || copies.get() disabled=move || printing.get() on:input=move |event| copies.set(event_target_value(&event))/></label>
                <button type="button" class="button secondary-action" disabled=move || loading.get() || printing.get() || printers.get().is_empty() on:click=request_print>
                    {move || if printing.get() { "Printing..." } else { "Print at station" }}
                </button>
                <Show when=move || printers.get().is_empty() && !loading.get()>
                    <small>"No healthy automatic printer is connected; download for manual fallback."</small>
                </Show>
            </div>
            <Show when=move || error.get().is_some()>
                <div class="shipping-documents-error" role="alert">
                    <span>{move || error.get().unwrap_or_default()}</span>
                    <Show when=move || retry.get().is_some()>
                        <button type="button" class="button secondary-action" disabled=move || printing.get() on:click=retry_exact>"Retry exact print"</button>
                    </Show>
                    <Show when=move || cancellation_retry.get().is_some()>
                        <button type="button" class="button secondary-action" on:click=retry_cancellation>"Retry exact cancellation"</button>
                    </Show>
                </div>
            </Show>
            <Show when=move || !jobs.get().is_empty()>
                <div class="shipment-print-history" aria-label="Print history">
                    <For
                        each=move || jobs.get()
                        key=|job| job.command_id
                        children=move |job| {
                            let cancellable = job.status == AutomationCommandStatus::Queued;
                            let cancel_job = StoredValue::new(job.clone());
                            view! {
                                <div class="shipment-print-job">
                                    <strong>{print_status(job.status)}</strong>
                                    <span>{format!("{} · {} copies · command {}",job.device_key,job.copies,job.command_id)}</span>
                                    <small>{job.spool_job_id.map_or_else(|| job.requested_at.clone(), |spool| format!("spool {spool}"))}</small>
                                    <Show when=move || cancellable>
                                        <button type="button" class="button secondary-action" on:click=move |_| cancel_queued.run(cancel_job.get_value())>"Cancel queued"</button>
                                    </Show>
                                </div>
                            }
                        }
                    />
                    <Show when=move || next_cursor.get().is_some()>
                        <button type="button" class="button secondary-action" disabled=move || loading.get() on:click=load_more>"Load older prints"</button>
                    </Show>
                </div>
            </Show>
        </div>
    }
}

fn upsert_job(
    jobs: RwSignal<Vec<ShipmentDocumentPrintJobResponse>>,
    job: ShipmentDocumentPrintJobResponse,
) {
    jobs.update(|current| {
        current.retain(|item| item.command_id != job.command_id);
        current.insert(0, job);
    });
}

#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)]
fn schedule_poll(
    document_id: i64,
    command_id: i64,
    request_generation: u64,
    generation: RwSignal<u64>,
    jobs: RwSignal<Vec<ShipmentDocumentPrintJobResponse>>,
    printing: RwSignal<bool>,
    active_command_id: RwSignal<Option<i64>>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    use std::time::Duration;

    set_timeout(
        move || {
            leptos::task::spawn_local(async move {
                if generation.get_untracked() != request_generation {
                    return;
                }
                match api::shipment_document_print_job(document_id, command_id).await {
                    Ok(job) => {
                        let done = terminal(job.status);
                        upsert_job(jobs, job);
                        if done {
                            printing.set(false);
                            active_command_id.set(None);
                        } else {
                            schedule_poll(
                                document_id,
                                command_id,
                                request_generation,
                                generation,
                                jobs,
                                printing,
                                active_command_id,
                                error,
                                on_unauthorized,
                            );
                        }
                    }
                    Err(api_error) => {
                        printing.set(false);
                        active_command_id.set(None);
                        error.set(Some(api_error.message));
                        if api_error.unauthorized {
                            on_unauthorized.run(());
                        }
                    }
                }
            });
        },
        Duration::from_secs(1),
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn schedule_poll(
    _document_id: i64,
    _command_id: i64,
    _request_generation: u64,
    _generation: RwSignal<u64>,
    _jobs: RwSignal<Vec<ShipmentDocumentPrintJobResponse>>,
    printing: RwSignal<bool>,
    active_command_id: RwSignal<Option<i64>>,
    error: RwSignal<Option<String>>,
    _on_unauthorized: Callback<()>,
) {
    printing.set(false);
    active_command_id.set(None);
    error.set(Some(
        "Print status polling starts after the browser connects.".to_owned(),
    ));
}

const fn terminal(status: AutomationCommandStatus) -> bool {
    matches!(
        status,
        AutomationCommandStatus::Succeeded
            | AutomationCommandStatus::Failed
            | AutomationCommandStatus::ManualReview
            | AutomationCommandStatus::ResolvedManually
            | AutomationCommandStatus::Cancelled
    )
}

const fn print_status(status: AutomationCommandStatus) -> &'static str {
    match status {
        AutomationCommandStatus::Queued => "Queued",
        AutomationCommandStatus::Delivered => "Delivered",
        AutomationCommandStatus::Accepted => "Printing",
        AutomationCommandStatus::Succeeded => "Printed",
        AutomationCommandStatus::Failed => "Failed",
        AutomationCommandStatus::ManualReview => "Review required",
        AutomationCommandStatus::ResolvedManually => "Manually resolved",
        AutomationCommandStatus::Cancelled => "Cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_statuses_keep_in_flight_and_terminal_states_distinct() {
        assert!(!terminal(AutomationCommandStatus::Accepted));
        assert!(terminal(AutomationCommandStatus::Succeeded));
        assert_eq!(
            print_status(AutomationCommandStatus::ManualReview),
            "Review required"
        );
    }
}
