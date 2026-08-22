use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{
    InboundLoadAppointmentRescheduleReason, RescheduleInboundLoadAppointmentRequest,
    ScheduleInboundLoadRequest,
};
use wareboxes_core::models::{Load, LoadStatus};

use super::{parse_optional_timestamp, timestamp_input};
use crate::api;
use crate::toast::use_toast_bus;

#[derive(Clone)]
enum AppointmentAttempt {
    Schedule(ScheduleInboundLoadRequest),
    Reschedule(RescheduleInboundLoadAppointmentRequest),
}

#[component]
pub(super) fn InboundAppointmentConfirmation(
    load: Load,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_close: Callback<()>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let rescheduling = load.status == LoadStatus::Scheduled;
    let current_scheduled_for = load.appointment_time;
    let scheduled_for = RwSignal::new(timestamp_input(load.appointment_time));
    let reason = RwSignal::new(InboundLoadAppointmentRescheduleReason::CarrierDelay);
    let note = RwSignal::new(String::new());
    let retry_attempt = RwSignal::new(None::<(AppointmentAttempt, String)>);
    let form_ref = NodeRef::<html::Form>::new();
    let scheduled_for_ref = NodeRef::<html::Input>::new();
    let load_id = load.id;
    let reference = load
        .reference_number
        .clone()
        .unwrap_or_else(|| format!("Load #{load_id}"));
    let toasts = use_toast_bus();

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if let Some(input) = scheduled_for_ref.get() {
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
        let (request, key) = if let Some(saved) = retry_attempt.get_untracked() {
            saved
        } else {
            let scheduled_for = match parse_optional_timestamp(&scheduled_for.get_untracked()) {
                Ok(Some(value)) => value,
                Ok(None) => {
                    error.set(Some("Choose an appointment date and time.".to_owned()));
                    return;
                }
                Err(message) => {
                    error.set(Some(format!("Appointment time: {message}")));
                    return;
                }
            };
            let request = if rescheduling {
                let Some(expected_scheduled_for) = current_scheduled_for else {
                    error.set(Some("Refresh this load before rescheduling.".to_owned()));
                    return;
                };
                let note_value = note.get_untracked().trim().to_owned();
                let note_value = (!note_value.is_empty()).then_some(note_value);
                if reason.get_untracked() == InboundLoadAppointmentRescheduleReason::Other
                    && note_value.is_none()
                {
                    error.set(Some("Explain the other reschedule reason.".to_owned()));
                    return;
                }
                AppointmentAttempt::Reschedule(RescheduleInboundLoadAppointmentRequest {
                    expected_scheduled_for: expected_scheduled_for.to_rfc3339(),
                    scheduled_for: scheduled_for.to_rfc3339(),
                    reason: reason.get_untracked(),
                    note: note_value,
                })
            } else {
                AppointmentAttempt::Schedule(ScheduleInboundLoadRequest {
                    scheduled_for: scheduled_for.to_rfc3339(),
                })
            };
            let key = api::new_idempotency_key();
            retry_attempt.set(Some((request.clone(), key.clone())));
            (request, key)
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let outcome = match &request {
                AppointmentAttempt::Schedule(request) => {
                    api::schedule_inbound_load(load_id, request, &key)
                        .await
                        .map(|_| ())
                }
                AppointmentAttempt::Reschedule(request) => {
                    api::reschedule_inbound_load_appointment(load_id, request, &key)
                        .await
                        .map(|_| ())
                }
            };
            match outcome {
                Ok(_) => {
                    retry_attempt.set(None);
                    pending.set(false);
                    on_close.run(());
                    toasts.success(if rescheduling {
                        format!("Appointment rescheduled for inbound load #{load_id}.")
                    } else {
                        format!("Appointment scheduled for inbound load #{load_id}.")
                    });
                    on_refreshed.run(load_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    if !api_error.ambiguous_outcome {
                        retry_attempt.set(None);
                    }
                    toasts.error(api_error.message.clone());
                    error.set(Some(if api_error.ambiguous_outcome {
                        "Appointment outcome is unknown. Retry the exact saved appointment change to reconcile it."
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
            class="confirmation-panel arrival-confirmation"
            role="alertdialog"
            aria-labelledby="schedule-inbound-load-title"
            on:submit=submit
        >
            <h3 id="schedule-inbound-load-title">
                {if rescheduling { "Reschedule inbound appointment" } else { "Schedule inbound appointment" }}
            </h3>
            <p>
                {if rescheduling {
                    "Record a reasoned change to the current warehouse appointment."
                } else {
                    "Set the first warehouse appointment for this planned load."
                }}
            </p>
            <div class="evidence-summary">
                <span><strong>"Load"</strong> {reference}</span>
                {current_scheduled_for.map(|value| view! {
                    <span><strong>"Current"</strong> {value.to_rfc3339()}</span>
                })}
            </div>
            <div class="fulfillment-form-grid two-column">
                <label>
                    <span>"Appointment (UTC)"</span>
                    <input
                        node_ref=scheduled_for_ref
                        type="datetime-local"
                        required
                        prop:value=move || scheduled_for.get()
                        on:input=move |event| scheduled_for.set(event_target_value(&event))
                    />
                </label>
                {rescheduling.then(|| view! {
                    <label>
                        <span>"Reason"</span>
                        <select
                            prop:value=move || reschedule_reason_wire(reason.get())
                            on:change=move |event| reason.set(reschedule_reason_from_wire(&event_target_value(&event)))
                        >
                            <option value="carrier_delay">"Carrier delay"</option>
                            <option value="supplier_change">"Supplier change"</option>
                            <option value="dock_capacity">"Dock capacity"</option>
                            <option value="weather">"Weather"</option>
                            <option value="correction">"Correction"</option>
                            <option value="other">"Other"</option>
                        </select>
                    </label>
                })}
                {rescheduling.then(|| view! {
                    <label class="wide">
                        <span>"Note"</span>
                        <input
                            type="text"
                            maxlength="500"
                            placeholder="Optional unless reason is Other"
                            prop:value=move || note.get()
                            on:input=move |event| note.set(event_target_value(&event))
                        />
                    </label>
                })}
            </div>
            <div class="form-actions">
                <button type="submit" class="button primary-action" disabled=move || pending.get()>
                    {move || if pending.get() {
                        if rescheduling { "Rescheduling" } else { "Scheduling" }
                    } else if rescheduling {
                        "Reschedule load"
                    } else {
                        "Schedule load"
                    }}
                </button>
                <button type="button" class="button secondary-action" on:click=move |_| on_close.run(())>
                    "Go back"
                </button>
            </div>
        </form>
    }
}

const fn reschedule_reason_wire(reason: InboundLoadAppointmentRescheduleReason) -> &'static str {
    match reason {
        InboundLoadAppointmentRescheduleReason::CarrierDelay => "carrier_delay",
        InboundLoadAppointmentRescheduleReason::SupplierChange => "supplier_change",
        InboundLoadAppointmentRescheduleReason::DockCapacity => "dock_capacity",
        InboundLoadAppointmentRescheduleReason::Weather => "weather",
        InboundLoadAppointmentRescheduleReason::Correction => "correction",
        InboundLoadAppointmentRescheduleReason::Other => "other",
    }
}

fn reschedule_reason_from_wire(value: &str) -> InboundLoadAppointmentRescheduleReason {
    match value {
        "supplier_change" => InboundLoadAppointmentRescheduleReason::SupplierChange,
        "dock_capacity" => InboundLoadAppointmentRescheduleReason::DockCapacity,
        "weather" => InboundLoadAppointmentRescheduleReason::Weather,
        "correction" => InboundLoadAppointmentRescheduleReason::Correction,
        "other" => InboundLoadAppointmentRescheduleReason::Other,
        _ => InboundLoadAppointmentRescheduleReason::CarrierDelay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appointment_reschedule_reasons_round_trip() {
        for reason in [
            InboundLoadAppointmentRescheduleReason::CarrierDelay,
            InboundLoadAppointmentRescheduleReason::SupplierChange,
            InboundLoadAppointmentRescheduleReason::DockCapacity,
            InboundLoadAppointmentRescheduleReason::Weather,
            InboundLoadAppointmentRescheduleReason::Correction,
            InboundLoadAppointmentRescheduleReason::Other,
        ] {
            assert_eq!(
                reschedule_reason_from_wire(reschedule_reason_wire(reason)),
                reason
            );
        }
    }
}
