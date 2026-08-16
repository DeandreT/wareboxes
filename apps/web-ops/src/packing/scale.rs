use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AutomationCommandResponse, AutomationCommandResult, AutomationCommandStatus,
    PackingScaleDeviceResponse, RequestPackingScaleWeight,
};

use crate::api;

use super::measurements::CartonMeasurementSignals;
use super::PackingSignals;

#[component]
pub(super) fn PackingScaleCapture(
    session_id: i64,
    carton_id: i64,
    measurements: CartonMeasurementSignals,
    signals: PackingSignals,
) -> impl IntoView {
    let devices = RwSignal::new(Vec::<PackingScaleDeviceResponse>::new());
    let selected_device_id = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let status = RwSignal::new("Connecting to station scales...".to_owned());
    let generation = RwSignal::new(0_u64);

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        let request_generation = generation.get_untracked().saturating_add(1);
        generation.set(request_generation);
        loading.set(true);
        leptos::task::spawn_local(async move {
            match api::packing_scale_devices(session_id).await {
                Ok(page) if generation.get_untracked() == request_generation => {
                    if selected_device_id.get_untracked().is_empty() {
                        selected_device_id.set(
                            page.items
                                .first()
                                .map_or_else(String::new, |device| device.device_id.to_string()),
                        );
                    }
                    status.set(if page.items.is_empty() {
                        "No healthy automatic scale is connected to this facility.".to_owned()
                    } else {
                        "Choose a scale and capture a stable weight.".to_owned()
                    });
                    devices.set(page.items);
                    loading.set(false);
                }
                Err(error) if generation.get_untracked() == request_generation => {
                    loading.set(false);
                    status.set(error.message);
                    if error.unauthorized {
                        signals.on_unauthorized.run(());
                    }
                }
                _ => {}
            }
        });
    });

    let request_weight = move |_| {
        if measurements.scale_busy.get_untracked() {
            return;
        }
        let Some(device_id) = selected_device_id
            .get_untracked()
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
        else {
            status.set("Select an available scale.".to_owned());
            return;
        };
        let request_generation = generation.get_untracked().saturating_add(1);
        generation.set(request_generation);
        measurements.scale_busy.set(true);
        measurements.weight_automation_command_id.set(None);
        status.set("Waiting for a stable scale reading...".to_owned());
        leptos::task::spawn_local(async move {
            let idempotency_key = api::new_idempotency_key();
            match api::request_packing_scale_weight(
                session_id,
                &RequestPackingScaleWeight {
                    device_id,
                    carton_id,
                    timeout_ms: 30_000,
                },
                &idempotency_key,
            )
            .await
            {
                Ok(command) if generation.get_untracked() == request_generation => {
                    handle_reading(
                        session_id,
                        command,
                        request_generation,
                        generation,
                        measurements,
                        status,
                        signals,
                    );
                }
                Err(error) if generation.get_untracked() == request_generation => {
                    measurements.scale_busy.set(false);
                    status.set(error.message);
                    if error.unauthorized {
                        signals.on_unauthorized.run(());
                    }
                }
                _ => {}
            }
        });
    };
    let use_manual_fallback = move |_| {
        generation.update(|value| *value = value.saturating_add(1));
        measurements.scale_busy.set(false);
        measurements.weight_automation_command_id.set(None);
        status.set(
            "Manual fallback selected. Enter the measured grams and preserve the physical check."
                .to_owned(),
        );
    };

    view! {
        <div class="packing-scale-capture">
            <div class="packing-scale-copy">
                <strong>"Connected scale"</strong>
                <small>{move || status.get()}</small>
            </div>
            <select
                aria-label="Packing scale"
                prop:value=move || selected_device_id.get()
                disabled=move || loading.get() || measurements.scale_busy.get()
                on:change=move |event| selected_device_id.set(event_target_value(&event))
            >
                {move || devices.get().into_iter().map(|device| view! {
                    <option value=device.device_id.to_string()>
                        {format!("{} · {}", device.display_name, device.device_key)}
                    </option>
                }).collect_view()}
            </select>
            <button
                class="button secondary-action"
                type="button"
                disabled=move || loading.get()
                    || devices.get().is_empty()
                    || measurements.scale_busy.get()
                    || signals.pending.get()
                on:click=request_weight
            >
                {move || if measurements.scale_busy.get() { "Reading..." } else { "Read weight" }}
            </button>
            <Show when=move || measurements.scale_busy.get()>
                <button
                    class="button secondary-action"
                    type="button"
                    on:click=use_manual_fallback
                >
                    "Use manual fallback"
                </button>
            </Show>
        </div>
    }
}

fn handle_reading(
    session_id: i64,
    command: AutomationCommandResponse,
    request_generation: u64,
    generation: RwSignal<u64>,
    measurements: CartonMeasurementSignals,
    status: RwSignal<String>,
    signals: PackingSignals,
) {
    if generation.get_untracked() != request_generation {
        return;
    }
    match command.status {
        AutomationCommandStatus::Succeeded => match stable_grams(&command) {
            Some(weight_grams) => {
                measurements.weight.set(weight_grams.to_string());
                measurements
                    .weight_automation_command_id
                    .set(Some(command.command_id));
                measurements.scale_busy.set(false);
                status.set(format!(
                    "Captured {weight_grams} g from {}. Manual edits discard this evidence.",
                    command.device_key
                ));
            }
            None => {
                measurements.scale_busy.set(false);
                status.set(
                    "Scale returned an unstable, nonpositive, or sub-gram reading.".to_owned(),
                );
            }
        },
        AutomationCommandStatus::Failed
        | AutomationCommandStatus::ManualReview
        | AutomationCommandStatus::ResolvedManually
        | AutomationCommandStatus::Cancelled => {
            measurements.scale_busy.set(false);
            status.set(command.error_message.unwrap_or_else(|| {
                "Scale reading did not complete; enter a manual fallback weight.".to_owned()
            }));
        }
        AutomationCommandStatus::Queued
        | AutomationCommandStatus::Delivered
        | AutomationCommandStatus::Accepted => schedule_poll(
            session_id,
            command.command_id,
            request_generation,
            generation,
            measurements,
            status,
            signals,
        ),
    }
}

#[cfg(target_arch = "wasm32")]
fn schedule_poll(
    session_id: i64,
    command_id: i64,
    request_generation: u64,
    generation: RwSignal<u64>,
    measurements: CartonMeasurementSignals,
    status: RwSignal<String>,
    signals: PackingSignals,
) {
    use std::time::Duration;

    set_timeout(
        move || {
            leptos::task::spawn_local(async move {
                if generation.get_untracked() != request_generation {
                    return;
                }
                match api::packing_scale_reading(session_id, command_id).await {
                    Ok(command) => handle_reading(
                        session_id,
                        command,
                        request_generation,
                        generation,
                        measurements,
                        status,
                        signals,
                    ),
                    Err(error) => {
                        measurements.scale_busy.set(false);
                        status.set(error.message);
                        if error.unauthorized {
                            signals.on_unauthorized.run(());
                        }
                    }
                }
            });
        },
        Duration::from_secs(1),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn schedule_poll(
    _session_id: i64,
    _command_id: i64,
    _request_generation: u64,
    _generation: RwSignal<u64>,
    measurements: CartonMeasurementSignals,
    status: RwSignal<String>,
    _signals: PackingSignals,
) {
    measurements.scale_busy.set(false);
    status.set("Scale polling starts after the browser connects.".to_owned());
}

fn stable_grams(command: &AutomationCommandResponse) -> Option<i64> {
    let AutomationCommandResult::Scale(result) = command.result.as_ref()? else {
        return None;
    };
    (result.stable && result.mass_milligrams > 0 && result.mass_milligrams % 1000 == 0)
        .then_some(result.mass_milligrams / 1000)
}

#[cfg(test)]
mod tests {
    use super::stable_grams;
    use wareboxes_api_contract::v1::{
        AutomationCommandResponse, AutomationCommandResult, AutomationCommandStatus,
        AutomationDeviceClass, AutomationDeviceCommand, AutomationRecoveryPolicy,
        AutomationScaleCommand, AutomationScaleResult, AutomationWeightUnit, Revision,
    };

    fn response(mass_milligrams: i64, stable: bool) -> AutomationCommandResponse {
        AutomationCommandResponse {
            command_id: 1,
            facility_id: 2,
            device_id: 3,
            device_key: "scale-1".to_owned(),
            device_class: AutomationDeviceClass::Scale,
            correlation_id: "read-1".to_owned(),
            recovery_policy: AutomationRecoveryPolicy::DeviceDeduplicatedReplay,
            command: AutomationDeviceCommand::Scale(AutomationScaleCommand::ReadStableWeight {
                requested_unit: AutomationWeightUnit::Gram,
                timeout_ms: 30_000,
            }),
            packing_scale_context: Some(
                wareboxes_api_contract::v1::PackingScaleCommandContextResponse {
                    inventory_owner_id: 6,
                    session_id: 7,
                    carton_id: 8,
                    carton_reopen_count: 0,
                },
            ),
            shipping_document_print_context: None,
            status: AutomationCommandStatus::Succeeded,
            revision: Revision::new(4).unwrap(),
            delivery_attempts: 1,
            assigned_service_account_id: Some(4),
            agent_instance: Some("edge-1".to_owned()),
            delivered_at: Some("2026-08-16T12:00:00Z".to_owned()),
            accepted_at: Some("2026-08-16T12:00:01Z".to_owned()),
            completed_at: Some("2026-08-16T12:00:02Z".to_owned()),
            result: Some(AutomationCommandResult::Scale(AutomationScaleResult {
                mass_milligrams,
                stable,
            })),
            error_code: None,
            error_message: None,
            resolved_by: None,
            resolution_outcome: None,
            resolution_reason: None,
            resolved_at: None,
            requested_by: 5,
            requested_at: "2026-08-16T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn only_exact_positive_stable_grams_are_accepted() {
        assert_eq!(stable_grams(&response(1_250_000, true)), Some(1_250));
        assert_eq!(stable_grams(&response(1_250_001, true)), None);
        assert_eq!(stable_grams(&response(1_250_000, false)), None);
        assert_eq!(stable_grams(&response(0, true)), None);
    }
}
