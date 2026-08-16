use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AllocationPolicyResponse, OrderAllocationReadinessResponse, OrderAllocationReadinessStatus,
    StreamOrderRequest,
};
use wareboxes_core::models::Location;

use super::{release_destinations, request_readiness, ReadinessState};
use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

type StreamRetry = (StreamOrderRequest, String);

#[derive(Clone, Copy)]
pub(super) struct StreamControlsState {
    pub(super) order_id: i64,
    pub(super) readiness: RwSignal<Option<OrderAllocationReadinessResponse>>,
    pub(super) facility_id: RwSignal<String>,
    pub(super) destination_location_id: RwSignal<String>,
    pub(super) release_locations: StoredValue<Vec<Location>>,
    pub(super) allocation_pending: RwSignal<bool>,
    pub(super) release_pending: RwSignal<bool>,
    pub(super) stream_pending: RwSignal<bool>,
    pub(super) committed_policy: RwSignal<Option<(i64, AllocationPolicyResponse)>>,
    pub(super) readiness_state: ReadinessState,
    pub(super) on_refreshed: Callback<i64>,
    pub(super) on_unauthorized: Callback<()>,
}

#[component]
pub(super) fn OrderStreamControls(state: StreamControlsState) -> impl IntoView {
    let retry = RwSignal::new(None::<StreamRetry>);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();

    Effect::new(move |_| {
        let _ = state.facility_id.get();
        retry.set(None);
        error.set(None);
    });

    let stream_order = move |_| {
        if state.allocation_pending.get_untracked()
            || state.release_pending.get_untracked()
            || state.stream_pending.get_untracked()
        {
            return;
        }
        let (request, idempotency_key) = if let Some(attempt) = retry.get_untracked() {
            attempt
        } else {
            let Some(current) = state.readiness.get_untracked() else {
                error.set(Some("Allocation readiness has not loaded yet.".to_owned()));
                return;
            };
            if current.status != OrderAllocationReadinessStatus::Ready {
                error.set(Some(super::readiness_action_message(&current)));
                return;
            }
            let Ok(destination_location_id) =
                state.destination_location_id.get_untracked().parse::<i64>()
            else {
                error.set(Some(
                    "Select a scannable staging or packing destination.".to_owned(),
                ));
                return;
            };
            (
                StreamOrderRequest {
                    facility_id: current.facility_id,
                    destination_location_id,
                    expected_revision: current.revision,
                    expected_allocation_policy: current.policy.reference(),
                },
                api::new_idempotency_key(),
            )
        };
        retry.set(Some((request.clone(), idempotency_key.clone())));
        state.stream_pending.set(true);
        error.set(None);

        leptos::task::spawn_local(async move {
            match api::stream_order(state.order_id, &request, &idempotency_key).await {
                Ok(result) => {
                    state.committed_policy.set(Some((
                        result.allocation.allocation_run_id,
                        result.allocation.policy.clone(),
                    )));
                    retry.set(None);
                    state.stream_pending.set(false);
                    toasts.success(format!(
                        "Allocated and released {} units to {} pick task(s).",
                        format_quantity(result.release.released_quantity),
                        result.release.pick_task_count
                    ));
                    state.on_refreshed.run(state.order_id);
                    request_readiness(
                        state.order_id,
                        result.release.facility_id,
                        state.readiness_state,
                    );
                }
                Err(api_error) if api_error.unauthorized => {
                    retry.set(None);
                    state.stream_pending.set(false);
                    state.on_unauthorized.run(());
                }
                Err(api_error) => {
                    state.stream_pending.set(false);
                    let message = if api_error.ambiguous_outcome {
                        format!(
                            "{} The result is unknown; retry to recover the original stream.",
                            api_error.message
                        )
                    } else {
                        retry.set(None);
                        api_error.message.clone()
                    };
                    error.set(Some(message));
                    toasts.error(api_error.message);
                    request_readiness(state.order_id, request.facility_id, state.readiness_state);
                }
            }
        });
    };

    view! {
        <Show when=move || error.get().is_some()>
            <p class="inline-command-error allocation-error" role="alert">
                {move || error.get().unwrap_or_default()}
            </p>
        </Show>
        <Show when=move || {
            retry.get().is_some()
                || state.readiness.get().is_some_and(|readiness| {
                    readiness.status == OrderAllocationReadinessStatus::Ready
                })
        }>
            <div class="order-release-toolbar order-stream-toolbar">
                <div class="order-release-label">
                    <strong>"Allocate and release"</strong>
                    <span>"One atomic command — shortage leaves the order unchanged"</span>
                </div>
                <label class="order-release-destination">
                    <span class="sr-only">"Stream destination"</span>
                    <select
                        aria-label="Stream destination"
                        disabled=move || {
                            state.stream_pending.get()
                                || state.release_pending.get()
                                || state.allocation_pending.get()
                                || release_destinations(
                                    state.release_locations.get_value(),
                                    state.facility_id.get().parse::<i64>().unwrap_or_default(),
                                )
                                .is_empty()
                        }
                        prop:value=move || state.destination_location_id.get()
                        on:change=move |event| {
                            state.destination_location_id.set(event_target_value(&event));
                            retry.set(None);
                            error.set(None);
                        }
                    >
                        {move || {
                            release_destinations(
                                state.release_locations.get_value(),
                                state.facility_id.get().parse::<i64>().unwrap_or_default(),
                            )
                            .into_iter()
                            .map(|destination| {
                                view! {
                                    <option value=destination.location_id>
                                        {destination.label}
                                    </option>
                                }
                            })
                            .collect_view()
                        }}
                    </select>
                </label>
                <button
                    type="button"
                    class="button primary-action order-release-action"
                    title="Allocate all demand and create RF pick work atomically"
                    disabled=move || {
                        state.stream_pending.get()
                            || state.release_pending.get()
                            || state.allocation_pending.get()
                            || (retry.get().is_none()
                                && state.destination_location_id.get().is_empty())
                    }
                    on:click=stream_order
                >
                    <Icon icon=UiIcon::Release/>
                    {move || if state.stream_pending.get() {
                        "Streaming"
                    } else if retry.get().is_some() {
                        "Retry stream"
                    } else {
                        "Allocate + release"
                    }}
                </button>
            </div>
            <Show when=move || {
                release_destinations(
                    state.release_locations.get_value(),
                    state.facility_id.get().parse::<i64>().unwrap_or_default(),
                )
                .is_empty()
            }>
                <p class="empty-state compact order-release-empty">
                    "Add an active, barcoded staging or packing location before streaming."
                </p>
            </Show>
        </Show>
    }
}
