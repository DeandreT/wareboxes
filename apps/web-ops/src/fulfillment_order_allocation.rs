use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AllocationPolicyResponse, OrderAllocationDetailResponse, OrderAllocationLineResponse,
    OrderAllocationOutcome, OrderAllocationReadinessBlocker, OrderAllocationReadinessResponse,
    OrderAllocationReadinessStatus, PlanOrderAllocationRequest, ReleaseOrderRequest,
};
use wareboxes_api_contract::web::access::AccessScopeResource;
use wareboxes_core::models::Location;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

mod backorder;
mod policy;
use backorder::BackorderControls;
use policy::{allocation_action_title, AllocationPolicyBadge, CommittedAllocationPolicy};

type AllocationRetry = (PlanOrderAllocationRequest, String);
type ReleaseRetry = (ReleaseOrderRequest, String);

#[derive(Clone, Copy)]
struct ReadinessState {
    facility_id: RwSignal<String>,
    response: RwSignal<Option<OrderAllocationReadinessResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    request_generation: RwSignal<u64>,
    on_unauthorized: Callback<()>,
}

#[component]
pub(super) fn OrderAllocationPanel(
    order_id: i64,
    facilities: Vec<AccessScopeResource>,
    locations: Vec<Location>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let facility_id = RwSignal::new(
        facilities
            .first()
            .map_or_else(String::new, |facility| facility.id.to_string()),
    );
    let access_facilities = StoredValue::new(facilities);
    let release_locations = StoredValue::new(locations);
    let destination_location_id = RwSignal::new(String::new());
    let readiness = RwSignal::new(None::<OrderAllocationReadinessResponse>);
    let loading = RwSignal::new(false);
    let command_pending = RwSignal::new(false);
    let release_pending = RwSignal::new(false);
    let readiness_error = RwSignal::new(None::<String>);
    let command_error = RwSignal::new(None::<String>);
    let release_error = RwSignal::new(None::<String>);
    let committed_policy = RwSignal::new(None::<(i64, AllocationPolicyResponse)>);
    let request_generation = RwSignal::new(0_u64);
    let retry_attempt = RwSignal::new(None::<AllocationRetry>);
    let release_retry = RwSignal::new(None::<ReleaseRetry>);
    let toasts = use_toast_bus();
    let readiness_state = ReadinessState {
        facility_id,
        response: readiness,
        loading,
        error: readiness_error,
        request_generation,
        on_unauthorized,
    };
    let backorder_changed = Callback::new(move |selected_facility: i64| {
        on_refreshed.run(order_id);
        request_readiness(order_id, selected_facility, readiness_state);
    });

    Effect::new(move |_| {
        let selected_facility = facility_id.get();
        retry_attempt.set(None);
        release_retry.set(None);
        command_error.set(None);
        release_error.set(None);
        let Ok(selected_facility) = selected_facility.parse::<i64>() else {
            readiness.set(None);
            loading.set(false);
            destination_location_id.set(String::new());
            return;
        };
        let destinations = release_destinations(release_locations.get_value(), selected_facility);
        let selected_destination = destination_location_id.get_untracked();
        if !destinations
            .iter()
            .any(|destination| destination.location_id.to_string() == selected_destination)
        {
            destination_location_id.set(
                destinations
                    .first()
                    .map_or_else(String::new, |destination| {
                        destination.location_id.to_string()
                    }),
            );
        }
        request_readiness(order_id, selected_facility, readiness_state);
    });

    let refresh = move |_| {
        let Ok(selected_facility) = facility_id.get_untracked().parse::<i64>() else {
            return;
        };
        command_error.set(None);
        request_readiness(order_id, selected_facility, readiness_state);
    };

    let allocate = move |_| {
        if command_pending.get_untracked() || release_pending.get_untracked() {
            return;
        }
        let (request, idempotency_key) = if let Some(attempt) = retry_attempt.get_untracked() {
            attempt
        } else {
            let Some(current) = readiness.get_untracked() else {
                command_error.set(Some("Allocation readiness has not loaded yet.".to_owned()));
                return;
            };
            if current.status != OrderAllocationReadinessStatus::Ready {
                command_error.set(Some(readiness_action_message(&current)));
                return;
            }
            (
                PlanOrderAllocationRequest {
                    facility_id: current.facility_id,
                    expected_revision: current.revision,
                    expected_policy: current.policy.reference(),
                },
                api::new_idempotency_key(),
            )
        };
        retry_attempt.set(Some((request.clone(), idempotency_key.clone())));
        command_pending.set(true);
        command_error.set(None);

        leptos::task::spawn_local(async move {
            match api::plan_order_allocation(order_id, &request, &idempotency_key).await {
                Ok(result) => {
                    committed_policy.set(Some((result.allocation_run_id, result.policy.clone())));
                    retry_attempt.set(None);
                    command_pending.set(false);
                    toasts.success(format!(
                        "Allocation run #{} committed: {} allocated, {} short.",
                        result.allocation_run_id,
                        format_quantity(result.newly_allocated_quantity),
                        format_quantity(result.shortage_quantity)
                    ));
                    on_refreshed.run(order_id);
                    request_readiness(order_id, result.facility_id, readiness_state);
                }
                Err(api_error) if api_error.unauthorized => {
                    retry_attempt.set(None);
                    command_pending.set(false);
                    on_unauthorized.run(());
                }
                Err(api_error) => {
                    command_pending.set(false);
                    let message = if api_error.ambiguous_outcome {
                        format!(
                            "{} The result is unknown; retry to recover the original result.",
                            api_error.message
                        )
                    } else {
                        retry_attempt.set(None);
                        api_error.message.clone()
                    };
                    command_error.set(Some(message));
                    toasts.error(api_error.message);
                    request_readiness(order_id, request.facility_id, readiness_state);
                }
            }
        });
    };

    let release = move |_| {
        if release_pending.get_untracked() || command_pending.get_untracked() {
            return;
        }
        let (request, idempotency_key) = if let Some(attempt) = release_retry.get_untracked() {
            attempt
        } else {
            let Some(current) = readiness.get_untracked() else {
                release_error.set(Some("Allocation readiness has not loaded yet.".to_owned()));
                return;
            };
            if current.status != OrderAllocationReadinessStatus::AlreadyFullyAllocated
                || current.outcome != OrderAllocationOutcome::FullyAllocated
            {
                release_error.set(Some(
                    "Allocate every demand line before releasing this order.".to_owned(),
                ));
                return;
            }
            let Ok(destination_location_id) =
                destination_location_id.get_untracked().parse::<i64>()
            else {
                release_error.set(Some(
                    "Select a scannable staging or packing destination.".to_owned(),
                ));
                return;
            };
            (
                ReleaseOrderRequest {
                    facility_id: current.facility_id,
                    destination_location_id,
                    expected_revision: current.revision,
                },
                api::new_idempotency_key(),
            )
        };
        release_retry.set(Some((request, idempotency_key.clone())));
        release_pending.set(true);
        release_error.set(None);

        leptos::task::spawn_local(async move {
            match api::release_order(order_id, &request, &idempotency_key).await {
                Ok(result) => {
                    release_retry.set(None);
                    release_pending.set(false);
                    toasts.success(format!(
                        "Released {} units to {} pick task(s).",
                        format_quantity(result.released_quantity),
                        result.pick_task_count
                    ));
                    on_refreshed.run(order_id);
                    request_readiness(order_id, result.facility_id, readiness_state);
                }
                Err(api_error) if api_error.unauthorized => {
                    release_retry.set(None);
                    release_pending.set(false);
                    on_unauthorized.run(());
                }
                Err(api_error) => {
                    release_pending.set(false);
                    let message = if api_error.ambiguous_outcome {
                        format!(
                            "{} The result is unknown; retry to recover the original release.",
                            api_error.message
                        )
                    } else {
                        release_retry.set(None);
                        api_error.message.clone()
                    };
                    release_error.set(Some(message));
                    toasts.error(api_error.message);
                    request_readiness(order_id, request.facility_id, readiness_state);
                }
            }
        });
    };

    view! {
        <section class="detail-section allocation-section">
            <div class="allocation-toolbar">
                <div>
                    <h3>"Stock allocation"</h3>
                    {move || readiness.get().map(|state| {
                        view! { <AllocationPolicyBadge policy=state.policy/> }
                    })}
                </div>
                <label class="allocation-facility-selector">
                    <span class="sr-only">"Allocation facility"</span>
                    <select
                        aria-label="Allocation facility"
                        disabled=move || {
                            loading.get()
                                || command_pending.get()
                                || release_pending.get()
                                || readiness.get().is_some_and(|state| state.eligible_facilities.is_empty())
                                || (readiness.get().is_none() && access_facilities.get_value().is_empty())
                        }
                        prop:value=move || facility_id.get()
                        on:change=move |event| facility_id.set(event_target_value(&event))
                    >
                        {move || allocation_facilities(readiness.get(), access_facilities.get_value())
                            .into_iter()
                            .map(|facility| {
                                view! {
                                    <option value=facility.facility_id>{facility.facility_name}</option>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
                <button
                    type="button"
                    class="button icon-action allocation-refresh"
                    title="Refresh allocation readiness"
                    aria-label="Refresh allocation readiness"
                    disabled=move || loading.get() || command_pending.get() || release_pending.get()
                    on:click=refresh
                >
                    <Icon icon=UiIcon::Refresh/>
                </button>
                <button
                    type="button"
                    class="button primary-action allocation-run-action"
                    title=move || allocation_action_title(readiness.get().as_ref().map(|state| &state.policy))
                    disabled=move || {
                        command_pending.get()
                            || release_pending.get()
                            || (retry_attempt.get().is_none()
                                && (loading.get()
                                    || readiness.get().is_none_or(|state| {
                                        state.status != OrderAllocationReadinessStatus::Ready
                                    })))
                    }
                    on:click=allocate
                >
                    <Icon icon=UiIcon::Inventory/>
                    {move || if command_pending.get() {
                        "Allocating"
                    } else if retry_attempt.get().is_some() {
                        "Retry allocation"
                    } else {
                        "Allocate"
                    }}
                </button>
            </div>

            <Show when=move || loading.get()>
                <div class="allocation-loading" role="status">
                    <span class="loading-line" aria-hidden="true"></span>
                    "Checking facility stock..."
                </div>
            </Show>
            <Show when=move || {
                readiness_error.get().is_some()
                    || command_error.get().is_some()
                    || release_error.get().is_some()
            }>
                <p class="inline-command-error allocation-error" role="alert">
                    {move || {
                        release_error
                            .get()
                            .or_else(|| command_error.get())
                            .or_else(|| readiness_error.get())
                            .unwrap_or_default()
                    }}
                </p>
            </Show>

            <BackorderControls
                order_id
                readiness
                allocation_pending=command_pending
                release_pending
                on_changed=backorder_changed
                on_unauthorized
            />

            {move || committed_policy.get().map(|(run_id, policy)| {
                view! { <CommittedAllocationPolicy run_id policy/> }
            })}

            <Show when=move || {
                release_retry.get().is_some()
                    || readiness.get().is_some_and(|state| {
                        state.status == OrderAllocationReadinessStatus::AlreadyFullyAllocated
                            && state.outcome == OrderAllocationOutcome::FullyAllocated
                    })
            }>
                <div class="order-release-toolbar">
                    <div class="order-release-label">
                        <strong>"Release to picking"</strong>
                        <span>"Directed destination"</span>
                    </div>
                    <label class="order-release-destination">
                        <span class="sr-only">"Staging or packing destination"</span>
                        <select
                            aria-label="Staging or packing destination"
                            disabled=move || {
                                release_pending.get()
                                    || command_pending.get()
                                    || release_destinations(
                                        release_locations.get_value(),
                                        facility_id.get().parse::<i64>().unwrap_or_default(),
                                    )
                                    .is_empty()
                            }
                            prop:value=move || destination_location_id.get()
                            on:change=move |event| {
                                destination_location_id.set(event_target_value(&event));
                                release_retry.set(None);
                                release_error.set(None);
                            }
                        >
                            {move || {
                                release_destinations(
                                    release_locations.get_value(),
                                    facility_id.get().parse::<i64>().unwrap_or_default(),
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
                        title="Create allocation-backed RF pick work"
                        disabled=move || {
                            release_pending.get()
                                || command_pending.get()
                                || (release_retry.get().is_none()
                                    && destination_location_id.get().is_empty())
                        }
                        on:click=release
                    >
                        <Icon icon=UiIcon::Release/>
                        {move || if release_pending.get() {
                            "Releasing"
                        } else if release_retry.get().is_some() {
                            "Retry release"
                        } else {
                            "Release"
                        }}
                    </button>
                </div>
                <Show when=move || {
                    release_destinations(
                        release_locations.get_value(),
                        facility_id.get().parse::<i64>().unwrap_or_default(),
                    )
                    .is_empty()
                }>
                    <p class="empty-state compact order-release-empty">
                        "Add an active, barcoded staging or packing location before release."
                    </p>
                </Show>
            </Show>

            {move || readiness.get().map(|state| view! { <AllocationReadiness state/> })}

            <Show when=move || access_facilities.get_value().is_empty()>
                <p class="empty-state compact">"No facility is available in your site scope."</p>
            </Show>
        </section>
    }
}

#[component]
fn AllocationReadiness(state: OrderAllocationReadinessResponse) -> impl IntoView {
    let status = state.status;
    let outcome = state.outcome;
    let revision = state.revision.get();
    let blocking_reasons = state.blocking_reasons.clone();
    let lines = state.lines.clone();
    let has_shortage = state.shortage_quantity > 0;
    let has_backorder = state.backordered_quantity > 0;
    let blocker_view = if blocking_reasons.is_empty() {
        ().into_any()
    } else {
        view! {
            <div class="allocation-blockers" role="status">
                <Icon icon=UiIcon::Alert/>
                {blocking_reasons
                    .into_iter()
                    .map(|reason| view! { <span>{blocker_label(reason)}</span> })
                    .collect_view()}
            </div>
        }
        .into_any()
    };

    view! {
        <div class="allocation-state-line">
            <span class=allocation_status_class(status)>{readiness_status_label(status)}</span>
            <span>{outcome_label(outcome)}</span>
            <span class="allocation-revision">{format!("Order rev. {revision}")}</span>
        </div>
        {blocker_view}
        <dl class="allocation-totals">
            <div><dt>"Original"</dt><dd>{format_quantity(state.original_demand_quantity)}</dd></div>
            <div class:backordered=has_backorder><dt>"Backordered"</dt><dd>{format_quantity(state.backordered_quantity)}</dd></div>
            <div><dt>"Demand"</dt><dd>{format_quantity(state.demand_quantity)}</dd></div>
            <div><dt>"Reserved"</dt><dd>{format_quantity(state.reserved_quantity)}</dd></div>
            <div><dt>"Allocated"</dt><dd>{format_quantity(state.allocated_quantity)}</dd></div>
            <div class:short=has_shortage>
                <dt>"Short"</dt><dd>{format_quantity(state.shortage_quantity)}</dd>
            </div>
        </dl>
        <div class="allocation-lines">
            <table class="data-table allocation-lines-table">
                <caption class="sr-only">"Facility allocation by order line"</caption>
                <thead>
                    <tr>
                        <th>"Line"</th>
                        <th>"Item / UOM"</th>
                        <th class="numeric">"Orig."</th>
                        <th class="numeric">"B/O"</th>
                        <th class="numeric">"Demand"</th>
                        <th class="numeric">"Res."</th>
                        <th class="numeric">"Alloc."</th>
                        <th class="numeric">"Short"</th>
                    </tr>
                </thead>
                {lines.into_iter().map(allocation_line_view).collect_view()}
            </table>
        </div>
    }
}

fn allocation_line_view(line: OrderAllocationLineResponse) -> impl IntoView {
    let has_shortage = line.shortage_quantity > 0;
    let allocations = line.allocations.clone();
    let source_count = allocations.len();
    let item_label = line
        .item_description
        .clone()
        .unwrap_or_else(|| format!("Item #{}", line.item_id));
    let shortage_label = line.shortage_reason.map(shortage_reason_label);
    let source_summary = shortage_label.map_or_else(
        || format!("{source_count} stock assignment(s)"),
        |reason| format!("{source_count} stock assignment(s) - {reason}"),
    );
    let no_source_label = shortage_label.map_or_else(
        || "No concrete stock assigned".to_owned(),
        |reason| format!("No concrete stock assigned - {reason}"),
    );
    let shortage_title = shortage_label.unwrap_or_default();

    view! {
        <tbody class:has-shortage=has_shortage>
            <tr>
                <td>
                    <strong>{line.line_key}</strong>
                    <small class="cell-detail">{format!("ID {}", line.order_line_id)}</small>
                </td>
                <td>
                    <strong title=item_label.clone()>{item_label.clone()}</strong>
                    <small class="cell-detail">{format!("Item #{} / {}", line.item_id, line.uom)}</small>
                </td>
                <td class="numeric">{format_quantity(line.original_demand_quantity)}</td>
                <td class="numeric allocation-backordered">{format_quantity(line.backordered_quantity)}</td>
                <td class="numeric">{format_quantity(line.demand_quantity)}</td>
                <td class="numeric">{format_quantity(line.reserved_quantity)}</td>
                <td class="numeric strong">{format_quantity(line.allocated_quantity)}</td>
                <td class="numeric allocation-shortage" title=shortage_title>
                    {format_quantity(line.shortage_quantity)}
                </td>
            </tr>
            <tr class="allocation-source-row">
                <td colspan="8">
                    {if allocations.is_empty() {
                        view! { <span class="allocation-no-source">{no_source_label}</span> }.into_any()
                    } else {
                        view! {
                            <details class="allocation-source-details">
                                <summary>{source_summary}</summary>
                                <div class="allocation-source-grid">
                                    {allocations.into_iter().map(allocation_source_view).collect_view()}
                                </div>
                            </details>
                        }.into_any()
                    }}
                </td>
            </tr>
        </tbody>
    }
}

fn allocation_source_view(source: OrderAllocationDetailResponse) -> impl IntoView {
    let location = location_label(&source);
    let license_plate = source
        .license_plate_barcode
        .clone()
        .or_else(|| source.license_plate_id.map(|id| format!("LP #{id}")))
        .unwrap_or_else(|| "Loose stock".to_owned());
    let trace = trace_label(&source);
    let expiration = source
        .expiration
        .as_deref()
        .map(expiration_label)
        .unwrap_or_else(|| "No expiry".to_owned());
    let quantity = source.quantity;
    let stock_reference = format!(
        "Balance #{} / Batch #{} / Allocation #{}",
        source.inventory_balance_id, source.item_batch_id, source.allocation_id
    );

    view! {
        <div class="allocation-source" title=stock_reference>
            <div><span>"Location"</span><strong>{location}</strong></div>
            <div><span>"License plate"</span><strong>{license_plate}</strong></div>
            <div><span>"Lot / serial"</span><strong>{trace}</strong></div>
            <div><span>"Expiration"</span><strong>{expiration}</strong></div>
            <div class="numeric"><span>"Qty"</span><strong>{format_quantity(quantity)}</strong></div>
        </div>
    }
}

fn request_readiness(order_id: i64, selected_facility: i64, state: ReadinessState) {
    state
        .request_generation
        .update(|generation| *generation = generation.saturating_add(1));
    let generation = state.request_generation.get_untracked();
    state.loading.set(true);
    state.error.set(None);

    leptos::task::spawn_local(async move {
        let response = api::order_allocation_readiness(order_id, selected_facility).await;
        let is_current = state.request_generation.get_untracked() == generation
            && state.facility_id.get_untracked() == selected_facility.to_string();
        if !is_current {
            return;
        }
        match response {
            Ok(next) => {
                let next_facility = next
                    .eligible_facilities
                    .first()
                    .filter(|_| {
                        !next
                            .eligible_facilities
                            .iter()
                            .any(|facility| facility.facility_id == selected_facility)
                    })
                    .map(|facility| facility.facility_id);
                if let Some(next_facility) = next_facility {
                    state.facility_id.set(next_facility.to_string());
                    return;
                }
                state.loading.set(false);
                state.response.set(Some(next));
            }
            Err(api_error) if api_error.unauthorized => {
                state.loading.set(false);
                state.on_unauthorized.run(());
            }
            Err(api_error) => {
                state.loading.set(false);
                state.error.set(Some(api_error.message));
            }
        }
    });
}

fn allocation_facilities(
    readiness: Option<OrderAllocationReadinessResponse>,
    fallback: Vec<AccessScopeResource>,
) -> Vec<AllocationFacilityOption> {
    readiness.map_or_else(
        || {
            fallback
                .into_iter()
                .map(|facility| AllocationFacilityOption {
                    facility_id: facility.id,
                    facility_name: facility.name,
                })
                .collect()
        },
        |state| {
            state
                .eligible_facilities
                .into_iter()
                .map(|facility| AllocationFacilityOption {
                    facility_id: facility.facility_id,
                    facility_name: facility.facility_name,
                })
                .collect()
        },
    )
}

struct AllocationFacilityOption {
    facility_id: i64,
    facility_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseDestinationOption {
    location_id: i64,
    label: String,
}

fn release_destinations(
    locations: Vec<Location>,
    facility_id: i64,
) -> Vec<ReleaseDestinationOption> {
    let mut destinations = locations
        .into_iter()
        .filter(|location| {
            location.facility_id == facility_id
                && location.deleted.is_none()
                && location.active
                && !location.pickable
                && matches!(
                    location.r#type.to_ascii_lowercase().as_str(),
                    "staging" | "packing"
                )
        })
        .filter_map(|location| {
            let barcode = location.barcode?.trim().to_owned();
            if barcode.is_empty() {
                return None;
            }
            let label = location.name.as_deref().map(str::trim).map_or_else(
                || barcode.clone(),
                |name| {
                    if name.is_empty() || name == barcode {
                        barcode.clone()
                    } else {
                        format!("{name} ({barcode})")
                    }
                },
            );
            Some(ReleaseDestinationOption {
                location_id: location.id,
                label,
            })
        })
        .collect::<Vec<_>>();
    destinations.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then(left.location_id.cmp(&right.location_id))
    });
    destinations
}

fn readiness_action_message(state: &OrderAllocationReadinessResponse) -> String {
    match state.status {
        OrderAllocationReadinessStatus::Ready => "Stock is ready to allocate.".to_owned(),
        OrderAllocationReadinessStatus::AlreadyFullyAllocated => {
            "Every demand line is already fully allocated at this facility.".to_owned()
        }
        OrderAllocationReadinessStatus::Blocked => state.blocking_reasons.first().map_or_else(
            || "This order cannot be allocated in its current state.".to_owned(),
            |reason| blocker_label(*reason).to_owned(),
        ),
    }
}

const fn allocation_status_class(status: OrderAllocationReadinessStatus) -> &'static str {
    match status {
        OrderAllocationReadinessStatus::Ready => "status allocation-ready",
        OrderAllocationReadinessStatus::AlreadyFullyAllocated => "status allocation-complete",
        OrderAllocationReadinessStatus::Blocked => "status held",
    }
}

const fn readiness_status_label(status: OrderAllocationReadinessStatus) -> &'static str {
    match status {
        OrderAllocationReadinessStatus::Ready => "Ready",
        OrderAllocationReadinessStatus::AlreadyFullyAllocated => "Fully allocated",
        OrderAllocationReadinessStatus::Blocked => "Blocked",
    }
}

const fn outcome_label(outcome: OrderAllocationOutcome) -> &'static str {
    match outcome {
        OrderAllocationOutcome::FullyAllocated => "All demand assigned",
        OrderAllocationOutcome::PartiallyAllocated => "Partial assignment",
        OrderAllocationOutcome::NotAllocated => "No demand assigned",
    }
}

const fn blocker_label(reason: OrderAllocationReadinessBlocker) -> &'static str {
    match reason {
        OrderAllocationReadinessBlocker::ActiveHold => "Release active order holds first.",
        OrderAllocationReadinessBlocker::CrossDockInProgress => {
            "Complete or cancel active cross-dock work first."
        }
        OrderAllocationReadinessBlocker::OrderStatusNotAllocatable => {
            "The order status does not allow allocation."
        }
        OrderAllocationReadinessBlocker::FacilityNotEligible => {
            "The selected facility is not assigned to this client."
        }
    }
}

const fn shortage_reason_label(
    reason: wareboxes_api_contract::v1::OrderAllocationShortageReason,
) -> &'static str {
    use wareboxes_api_contract::v1::OrderAllocationShortageReason;

    match reason {
        OrderAllocationShortageReason::NoEligibleInventory => "No eligible stock",
        OrderAllocationShortageReason::InsufficientEligibleInventory => "Insufficient stock",
    }
}

fn location_label(source: &OrderAllocationDetailResponse) -> String {
    match (&source.location_name, &source.location_barcode) {
        (Some(name), Some(barcode)) if name != barcode => format!("{name} ({barcode})"),
        (Some(name), _) => name.clone(),
        (None, Some(barcode)) => barcode.clone(),
        (None, None) => format!("Location #{}", source.location_id),
    }
}

fn trace_label(source: &OrderAllocationDetailResponse) -> String {
    match (&source.lot, &source.serial) {
        (Some(lot), Some(serial)) => format!("{lot} / {serial}"),
        (Some(lot), None) => lot.clone(),
        (None, Some(serial)) => serial.clone(),
        (None, None) => "Untracked".to_owned(),
    }
}

fn expiration_label(value: &str) -> String {
    value
        .split_once('T')
        .map_or_else(|| value.to_owned(), |(date, _)| date.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiration_uses_a_dense_date_label() {
        assert_eq!(expiration_label("2027-08-10T00:00:00+00:00"), "2027-08-10");
        assert_eq!(expiration_label("not-a-timestamp"), "not-a-timestamp");
    }

    #[test]
    fn release_destinations_are_scannable_non_pickable_locations_in_facility() {
        let base = serde_json::from_value::<Location>(serde_json::json!({
            "id": 1,
            "tenant_id": 2,
            "created": "2026-08-08T20:00:00Z",
            "deleted": null,
            "facility_id": 3,
            "facility_name": "Main",
            "parent_location_id": null,
            "barcode": "STAGE-01",
            "name": "Packing lane",
            "type": "staging",
            "active": true,
            "pickable": false,
            "receivable": false
        }))
        .unwrap();
        let mut pickable = base.clone();
        pickable.id = 2;
        pickable.pickable = true;
        let mut other_facility = base.clone();
        other_facility.id = 3;
        other_facility.facility_id = 4;
        let mut damage = base.clone();
        damage.id = 4;
        damage.r#type = "damage".to_owned();

        assert_eq!(
            release_destinations(vec![pickable, other_facility, damage, base], 3),
            vec![ReleaseDestinationOption {
                location_id: 1,
                label: "Packing lane (STAGE-01)".to_owned(),
            }]
        );
    }
}
