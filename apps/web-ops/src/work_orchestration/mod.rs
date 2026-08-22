mod display;
mod forms;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ActivateWorkOrchestrationDispatchRequest, CancelWorkOrchestrationDispatchRequest,
    ConfigureWorkOrchestrationPolicyRequest, GenerateWorkOrchestrationPlanRequest,
    OrchestrationPlanMode, OrchestrationSignalWorkspaceRequest,
    OrchestrationSignalWorkspaceResponse, RecordResourceCapacitySignalRequest,
    RecordZoneCongestionSignalRequest, WorkOrchestrationDispatchResponse,
    WorkOrchestrationPlanPage, WorkOrchestrationPlanPageRequest, WorkOrchestrationPlanResponse,
    WorkOrchestrationPolicyPage, WorkOrchestrationPolicyPageRequest,
    WorkOrchestrationPolicyResponse, WorkOrchestrationWorkerPage,
    WorkOrchestrationWorkerPageRequest,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::Location;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone)]
enum Dialog {
    Configure {
        current: Option<WorkOrchestrationPolicyResponse>,
        disable: bool,
    },
    Congestion,
    Resource,
    Generate(WorkOrchestrationPolicyResponse),
    Activate(Box<WorkOrchestrationPlanResponse>),
    Cancel(WorkOrchestrationDispatchResponse),
}

#[derive(Clone)]
enum PendingCommand {
    Configure(ConfigureWorkOrchestrationPolicyRequest, String),
    Congestion(RecordZoneCongestionSignalRequest, String),
    Resource(RecordResourceCapacitySignalRequest, String),
    Generate(GenerateWorkOrchestrationPlanRequest, String),
    Activate {
        plan_id: i64,
        request: ActivateWorkOrchestrationDispatchRequest,
        key: String,
    },
    Cancel {
        dispatch_id: i64,
        request: CancelWorkOrchestrationDispatchRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
struct Signals {
    policies: RwSignal<WorkOrchestrationPolicyPage>,
    plans: RwSignal<WorkOrchestrationPlanPage>,
    operational_signals: RwSignal<OrchestrationSignalWorkspaceResponse>,
    selected_plan: RwSignal<Option<WorkOrchestrationPlanResponse>>,
    policies_loaded: RwSignal<bool>,
    plans_loaded: RwSignal<bool>,
    signals_loaded: RwSignal<bool>,
    policies_loading: RwSignal<bool>,
    plans_loading: RwSignal<bool>,
    signals_loading: RwSignal<bool>,
    detail_loading: RwSignal<bool>,
    policy_generation: RwSignal<u64>,
    plan_generation: RwSignal<u64>,
    signal_generation: RwSignal<u64>,
    detail_generation: RwSignal<u64>,
    worker_generation: RwSignal<u64>,
    workers: RwSignal<WorkOrchestrationWorkerPage>,
    workers_loading: RwSignal<bool>,
    workers_loaded: RwSignal<bool>,
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    include_policy_history: RwSignal<bool>,
    include_signal_history: RwSignal<bool>,
    plan_mode: RwSignal<Option<OrchestrationPlanMode>>,
    error: RwSignal<Option<String>>,
    dialog: RwSignal<Option<Dialog>>,
    command_pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn WorkOrchestrationWorkspace(
    access: AccessScopeWorkspace,
    locations: Vec<Location>,
    can_supervise: bool,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let initial_facility = access.facilities.first().map(|value| value.id);
    let access = StoredValue::new(access);
    let locations = StoredValue::new(locations);
    let signals = Signals {
        policies: RwSignal::new(WorkOrchestrationPolicyPage::new(Vec::new(), None)),
        plans: RwSignal::new(WorkOrchestrationPlanPage::new(Vec::new(), None)),
        operational_signals: RwSignal::new(OrchestrationSignalWorkspaceResponse {
            zone_signals: Vec::new(),
            resource_signals: Vec::new(),
            next_zone_cursor: None,
            next_resource_cursor: None,
        }),
        selected_plan: RwSignal::new(None),
        policies_loaded: RwSignal::new(false),
        plans_loaded: RwSignal::new(false),
        signals_loaded: RwSignal::new(false),
        policies_loading: RwSignal::new(true),
        plans_loading: RwSignal::new(true),
        signals_loading: RwSignal::new(true),
        detail_loading: RwSignal::new(false),
        policy_generation: RwSignal::new(0),
        plan_generation: RwSignal::new(0),
        signal_generation: RwSignal::new(0),
        detail_generation: RwSignal::new(0),
        worker_generation: RwSignal::new(0),
        workers: RwSignal::new(WorkOrchestrationWorkerPage::new(Vec::new(), None)),
        workers_loading: RwSignal::new(false),
        workers_loaded: RwSignal::new(false),
        facility_id: RwSignal::new(initial_facility),
        owner_id: RwSignal::new(None),
        include_policy_history: RwSignal::new(false),
        include_signal_history: RwSignal::new(false),
        plan_mode: RwSignal::new(None),
        error: RwSignal::new(None),
        dialog: RwSignal::new(None),
        command_pending: RwSignal::new(false),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = forms::Drafts::new();

    Effect::new(move |_| refresh_all(signals));

    let apply_filters = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        signals.selected_plan.set(None);
        refresh_all(signals);
    };
    let refresh = move |_| refresh_all(signals);
    let configure = move |_| {
        drafts.reset_policy(None, false, signals);
        signals.dialog.set(Some(Dialog::Configure {
            current: None,
            disable: false,
        }));
    };
    let congestion = move |_| {
        drafts.reset_congestion();
        signals.command_error.set(None);
        signals.dialog.set(Some(Dialog::Congestion));
    };
    let resource = move |_| {
        drafts.reset_resource();
        signals.command_error.set(None);
        signals.dialog.set(Some(Dialog::Resource));
    };
    let retry = move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    };

    view! {
        <section class="orchestration-workspace">
            <header class="page-heading orchestration-heading">
                <div>
                    <p class="eyebrow">"Execution control"</p>
                    <h1>"Work orchestration"</h1>
                    <p>"Explainable task sequencing with frozen policy, travel, congestion, and capacity evidence."</p>
                </div>
                <div class="orchestration-heading-actions">
                    {can_supervise.then(|| view! {
                        <button class="button primary-action" type="button" on:click=configure>
                            "Configure policy"
                        </button>
                    })}
                    <button
                        class="button secondary-action"
                        type="button"
                        disabled=move || all_loading(signals)
                        on:click=refresh
                    >
                        <Icon icon=UiIcon::Refresh/>
                        <span>"Refresh"</span>
                    </button>
                </div>
            </header>

            <form class="orchestration-toolbar" aria-label="Orchestration filters" on:submit=apply_filters>
                <label>
                    <span>"Facility"</span>
                    <select
                        prop:value=move || display::option_id(signals.facility_id.get())
                        on:change=move |event| {
                            let facility_id = display::parse_id(&event_target_value(&event));
                            signals.facility_id.set(facility_id);
                            let owner_id = signals.owner_id.get_untracked();
                            if !access.with_value(|value| display::owner_is_allowed(value, facility_id, owner_id)) {
                                signals.owner_id.set(None);
                            }
                        }
                    >
                        <option value="">"All authorized facilities"</option>
                        {access.with_value(|value| display::scope_options(&value.facilities))}
                    </select>
                </label>
                <label>
                    <span>"Client"</span>
                    <select
                        prop:value=move || display::option_id(signals.owner_id.get())
                        on:change=move |event| signals.owner_id.set(display::parse_id(&event_target_value(&event)))
                    >
                        <option value="">"All scopes · default plans use shared work"</option>
                        {move || access.with_value(|value| display::owner_scope_options(value, signals.facility_id.get()))}
                    </select>
                </label>
                <label>
                    <span>"Plan mode"</span>
                    <select
                        prop:value=move || display::plan_mode_wire(signals.plan_mode.get())
                        on:change=move |event| signals.plan_mode.set(display::parse_plan_mode(&event_target_value(&event)))
                    >
                        <option value="">"All plans"</option>
                        <option value="optimized">"Optimized"</option>
                        <option value="manual_fifo">"Manual FIFO"</option>
                    </select>
                </label>
                <label class="orchestration-history-toggle">
                    <input
                        type="checkbox"
                        prop:checked=move || signals.include_policy_history.get()
                        on:change=move |event| signals.include_policy_history.set(event_target_checked(&event))
                    />
                    <span>"Policy history"</span>
                </label>
                <label class="orchestration-history-toggle">
                    <input
                        type="checkbox"
                        prop:checked=move || signals.include_signal_history.get()
                        on:change=move |event| signals.include_signal_history.set(event_target_checked(&event))
                    />
                    <span>"Signal history"</span>
                </label>
                <button class="button secondary-action compact" type="submit">"Apply"</button>
            </form>

            {move || display::metrics(signals)}

            <Show when=move || signals.error.get().is_some()>
                <div class="orchestration-error" role="alert">
                    <span>{move || signals.error.get().unwrap_or_default()}</span>
                    <button class="text-button" type="button" on:click=refresh>"Retry reads"</button>
                </div>
            </Show>

            {move || display::policy_panel(signals, drafts, access, locations, can_supervise)}

            <section class="orchestration-signal-grid">
                {move || display::zone_signal_panel(signals, can_supervise, congestion)}
                {move || display::resource_signal_panel(signals, can_supervise, resource)}
            </section>

            <div class="orchestration-plan-workspace">
                {move || display::plan_panel(signals, access)}
                {move || display::plan_detail(signals, access, drafts, can_supervise)}
            </div>

            {move || signals.dialog.get().map(|dialog| {
                forms::command_dialog(signals, drafts, access, locations, dialog)
            })}

            <Show when=move || signals.command_error.get().is_some() && signals.dialog.get().is_none()>
                <div class="orchestration-error command" role="alert">
                    <span>{move || signals.command_error.get().unwrap_or_default()}</span>
                    <Show when=move || signals.retry.get().is_some()>
                        <button
                            class="button secondary-action compact"
                            type="button"
                            disabled=move || signals.command_pending.get()
                            on:click=retry
                        >"Retry exact command"</button>
                    </Show>
                </div>
            </Show>
        </section>
    }
}

fn all_loading(signals: Signals) -> bool {
    signals.policies_loading.get() || signals.plans_loading.get() || signals.signals_loading.get()
}

fn refresh_all(signals: Signals) {
    invalidate_plan_detail(signals);
    signals.error.set(None);
    load_policies(signals, None, false);
    load_plans(signals, None, false);
    load_signals(signals, None, None, false, false);
}

fn load_policies(
    signals: Signals,
    cursor: Option<wareboxes_api_contract::v1::OpaqueCursor>,
    append: bool,
) {
    let generation = signals.policy_generation.get_untracked() + 1;
    signals.policy_generation.set(generation);
    signals.policies_loading.set(true);
    leptos::task::spawn_local(async move {
        let request = WorkOrchestrationPolicyPageRequest {
            facility_id: signals.facility_id.get_untracked(),
            inventory_owner_id: signals.owner_id.get_untracked(),
            include_facility_defaults: true,
            include_history: signals.include_policy_history.get_untracked(),
            cursor,
            ..WorkOrchestrationPolicyPageRequest::default()
        };
        match api::work_orchestration_policies(&request).await {
            Ok(mut page) if signals.policy_generation.get_untracked() == generation => {
                if append {
                    let mut current = signals.policies.get_untracked();
                    current.items.append(&mut page.items);
                    current.next_cursor = page.next_cursor;
                    signals.policies.set(current);
                } else {
                    signals.policies.set(page);
                }
                signals.policies_loaded.set(true);
                signals.policies_loading.set(false);
            }
            Err(error) if signals.policy_generation.get_untracked() == generation => {
                read_failed(signals, error);
                signals.policies_loading.set(false);
            }
            _ => {}
        }
    });
}

fn load_plans(
    signals: Signals,
    cursor: Option<wareboxes_api_contract::v1::OpaqueCursor>,
    append: bool,
) {
    let generation = signals.plan_generation.get_untracked() + 1;
    signals.plan_generation.set(generation);
    signals.plans_loading.set(true);
    leptos::task::spawn_local(async move {
        let request = WorkOrchestrationPlanPageRequest {
            facility_id: signals.facility_id.get_untracked(),
            inventory_owner_id: signals.owner_id.get_untracked(),
            plan_mode: signals.plan_mode.get_untracked(),
            cursor,
            ..WorkOrchestrationPlanPageRequest::default()
        };
        match api::work_orchestration_plans(&request).await {
            Ok(mut page) if signals.plan_generation.get_untracked() == generation => {
                if append {
                    let mut current = signals.plans.get_untracked();
                    current.items.append(&mut page.items);
                    current.next_cursor = page.next_cursor;
                    signals.plans.set(current);
                } else {
                    signals.plans.set(page);
                }
                signals.plans_loaded.set(true);
                signals.plans_loading.set(false);
            }
            Err(error) if signals.plan_generation.get_untracked() == generation => {
                read_failed(signals, error);
                signals.plans_loading.set(false);
            }
            _ => {}
        }
    });
}

fn load_signals(
    signals: Signals,
    zone_cursor: Option<wareboxes_api_contract::v1::OpaqueCursor>,
    resource_cursor: Option<wareboxes_api_contract::v1::OpaqueCursor>,
    append_zones: bool,
    append_resources: bool,
) {
    let decision = signal_read_decision(
        signals.signal_generation.get_untracked(),
        signals.facility_id.get_untracked(),
    );
    let generation = decision.generation;
    signals.signal_generation.set(generation);
    let Some(facility_id) = decision.facility_id else {
        signals
            .operational_signals
            .set(OrchestrationSignalWorkspaceResponse {
                zone_signals: Vec::new(),
                resource_signals: Vec::new(),
                next_zone_cursor: None,
                next_resource_cursor: None,
            });
        signals.signals_loaded.set(true);
        signals.signals_loading.set(false);
        return;
    };
    signals.signals_loading.set(true);
    leptos::task::spawn_local(async move {
        let request = OrchestrationSignalWorkspaceRequest {
            facility_id,
            include_history: signals.include_signal_history.get_untracked(),
            zone_cursor,
            resource_cursor,
            limit: wareboxes_api_contract::v1::PageLimit::default(),
        };
        match api::work_orchestration_signals(&request).await {
            Ok(workspace) if signals.signal_generation.get_untracked() == generation => {
                if append_zones || append_resources {
                    let current = signals.operational_signals.get_untracked();
                    signals.operational_signals.set(merge_signal_workspace(
                        current,
                        workspace,
                        append_zones,
                        append_resources,
                    ));
                } else {
                    signals.operational_signals.set(workspace);
                }
                signals.signals_loaded.set(true);
                signals.signals_loading.set(false);
            }
            Err(error) if signals.signal_generation.get_untracked() == generation => {
                read_failed(signals, error);
                signals.signals_loading.set(false);
            }
            _ => {}
        }
    });
}

fn read_failed(signals: Signals, error: api::ApiError) {
    if error.unauthorized {
        signals.on_unauthorized.run(());
    } else {
        signals.error.set(Some(error.message));
    }
}

fn dispatch(signals: Signals, command: PendingCommand) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.command_error.set(None);
    signals.retry.set(None);
    leptos::task::spawn_local(async move {
        match command.clone() {
            PendingCommand::Configure(request, key) => {
                match api::configure_work_orchestration_policy(&request, &key).await {
                    Ok(policy) => {
                        signals.dialog.set(None);
                        signals.command_pending.set(false);
                        signals.toasts.success(format!(
                            "Policy #{} revision {} is active.",
                            policy.policy_id,
                            policy.revision.get()
                        ));
                        load_policies(signals, None, false);
                    }
                    Err(error) => command_failed(signals, command, error),
                }
            }
            PendingCommand::Congestion(request, key) => {
                match api::record_zone_congestion_signal(&request, &key).await {
                    Ok(signal) => {
                        signals.dialog.set(None);
                        signals.command_pending.set(false);
                        signals.toasts.success(format!(
                            "Congestion signal #{} recorded for {}.",
                            signal.signal_id, signal.storage_zone_code
                        ));
                        load_signals(signals, None, None, false, false);
                    }
                    Err(error) => command_failed(signals, command, error),
                }
            }
            PendingCommand::Resource(request, key) => {
                match api::record_resource_capacity_signal(&request, &key).await {
                    Ok(signal) => {
                        signals.dialog.set(None);
                        signals.command_pending.set(false);
                        signals
                            .toasts
                            .success(format!("Capacity signal #{} recorded.", signal.signal_id));
                        load_signals(signals, None, None, false, false);
                    }
                    Err(error) => command_failed(signals, command, error),
                }
            }
            PendingCommand::Generate(request, key) => {
                match api::generate_work_orchestration_plan(&request, &key).await {
                    Ok(plan) => {
                        signals.dialog.set(None);
                        signals.command_pending.set(false);
                        let outcome = if plan.plan_mode == OrchestrationPlanMode::ManualFifo {
                            "Manual FIFO fallback"
                        } else {
                            "Optimized advisory"
                        };
                        signals.toasts.success(format!(
                            "{outcome} plan #{} generated with {} tasks.",
                            plan.plan_id, plan.item_count
                        ));
                        invalidate_plan_detail(signals);
                        signals.selected_plan.set(Some(plan));
                        load_plans(signals, None, false);
                    }
                    Err(error) => command_failed(signals, command, error),
                }
            }
            PendingCommand::Activate {
                plan_id,
                request,
                key,
            } => match api::activate_work_orchestration_dispatch(plan_id, &request, &key).await {
                Ok(active) => {
                    signals.dialog.set(None);
                    signals.command_pending.set(false);
                    signals.toasts.success(format!(
                        "Dispatch #{} activated for worker #{}.",
                        active.dispatch_id, active.worker_user_id
                    ));
                    load_plan_detail(signals, active.plan_id);
                    load_plans(signals, None, false);
                }
                Err(error) => command_failed(signals, command, error),
            },
            PendingCommand::Cancel {
                dispatch_id,
                request,
                key,
            } => match api::cancel_work_orchestration_dispatch(dispatch_id, &request, &key).await {
                Ok(cancelled) => {
                    signals.dialog.set(None);
                    signals.command_pending.set(false);
                    signals.toasts.success(format!(
                        "Dispatch #{} cancelled; unstarted work was released.",
                        cancelled.dispatch_id
                    ));
                    load_plan_detail(signals, cancelled.plan_id);
                    load_plans(signals, None, false);
                }
                Err(error) => command_failed(signals, command, error),
            },
        }
    });
}

fn command_failed(signals: Signals, command: PendingCommand, error: api::ApiError) {
    signals.command_pending.set(false);
    if error.unauthorized {
        signals.on_unauthorized.run(());
        return;
    }
    let message = if error.ambiguous_outcome {
        format!(
            "{} Retry with the preserved command key to recover its result.",
            error.message
        )
    } else {
        error.message
    };
    signals.command_error.set(Some(message));
    signals.retry.set(Some(command));
}

fn load_plan_detail(signals: Signals, plan_id: i64) {
    let generation = signals.detail_generation.get_untracked() + 1;
    signals.detail_generation.set(generation);
    let filter = current_plan_filter(signals);
    signals.detail_loading.set(true);
    signals.error.set(None);
    leptos::task::spawn_local(async move {
        match api::work_orchestration_plan(plan_id).await {
            Ok(plan)
                if response_is_current(
                    signals.detail_generation.get_untracked(),
                    generation,
                    current_plan_filter(signals),
                    filter,
                ) =>
            {
                signals.selected_plan.set(Some(plan));
            }
            Err(error)
                if response_is_current(
                    signals.detail_generation.get_untracked(),
                    generation,
                    current_plan_filter(signals),
                    filter,
                ) =>
            {
                read_failed(signals, error);
            }
            _ => {}
        }
        if signals.detail_generation.get_untracked() == generation {
            signals.detail_loading.set(false);
        }
    });
}

fn load_workers(
    signals: Signals,
    facility_id: i64,
    inventory_owner_id: Option<i64>,
    cursor: Option<wareboxes_api_contract::v1::OpaqueCursor>,
    append: bool,
) {
    let generation = signals.worker_generation.get_untracked().wrapping_add(1);
    signals.worker_generation.set(generation);
    signals.workers_loading.set(true);
    if !append {
        signals
            .workers
            .set(WorkOrchestrationWorkerPage::new(Vec::new(), None));
        signals.workers_loaded.set(false);
    }
    leptos::task::spawn_local(async move {
        let request = WorkOrchestrationWorkerPageRequest {
            facility_id,
            inventory_owner_id,
            cursor,
            limit: wareboxes_api_contract::v1::PageLimit::default(),
        };
        match api::work_orchestration_workers(&request).await {
            Ok(page) if signals.worker_generation.get_untracked() == generation => {
                if append {
                    let current = signals.workers.get_untracked();
                    signals.workers.set(merge_worker_page(current, page));
                } else {
                    signals.workers.set(page);
                }
                signals.workers_loaded.set(true);
                signals.workers_loading.set(false);
            }
            Err(error) if signals.worker_generation.get_untracked() == generation => {
                if error.unauthorized {
                    signals.on_unauthorized.run(());
                } else {
                    signals.command_error.set(Some(error.message));
                }
                signals.workers_loading.set(false);
            }
            _ => {}
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SignalReadDecision {
    generation: u64,
    facility_id: Option<i64>,
}

fn signal_read_decision(current_generation: u64, facility_id: Option<i64>) -> SignalReadDecision {
    SignalReadDecision {
        generation: current_generation.wrapping_add(1),
        facility_id,
    }
}

fn merge_signal_workspace(
    mut current: OrchestrationSignalWorkspaceResponse,
    mut incoming: OrchestrationSignalWorkspaceResponse,
    append_zones: bool,
    append_resources: bool,
) -> OrchestrationSignalWorkspaceResponse {
    if append_zones {
        current.zone_signals.append(&mut incoming.zone_signals);
        current.next_zone_cursor = incoming.next_zone_cursor;
    }
    if append_resources {
        current
            .resource_signals
            .append(&mut incoming.resource_signals);
        current.next_resource_cursor = incoming.next_resource_cursor;
    }
    current
}

fn merge_worker_page(
    mut current: WorkOrchestrationWorkerPage,
    mut incoming: WorkOrchestrationWorkerPage,
) -> WorkOrchestrationWorkerPage {
    current.items.append(&mut incoming.items);
    current.next_cursor = incoming.next_cursor;
    current
}

type PlanFilterIdentity = (Option<i64>, Option<i64>, Option<OrchestrationPlanMode>);

fn current_plan_filter(signals: Signals) -> PlanFilterIdentity {
    (
        signals.facility_id.get_untracked(),
        signals.owner_id.get_untracked(),
        signals.plan_mode.get_untracked(),
    )
}

fn response_is_current<T: PartialEq>(
    current_generation: u64,
    response_generation: u64,
    current_filter: T,
    response_filter: T,
) -> bool {
    current_generation == response_generation && current_filter == response_filter
}

fn invalidate_plan_detail(signals: Signals) {
    signals
        .detail_generation
        .set(signals.detail_generation.get_untracked().wrapping_add(1));
    signals.detail_loading.set(false);
    signals.selected_plan.set(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::OpaqueCursor;

    #[test]
    fn clearing_facility_still_invalidates_an_in_flight_signal_read() {
        assert_eq!(
            signal_read_decision(41, None),
            SignalReadDecision {
                generation: 42,
                facility_id: None,
            }
        );
    }

    #[test]
    fn stale_or_cross_filter_detail_responses_are_rejected() {
        let filter = (Some(8), Some(12), Some(OrchestrationPlanMode::Optimized));
        assert!(response_is_current(4, 4, filter, filter));
        assert!(!response_is_current(5, 4, filter, filter));
        assert!(!response_is_current(
            4,
            4,
            (Some(9), Some(12), Some(OrchestrationPlanMode::Optimized)),
            filter,
        ));
    }

    #[test]
    fn paging_one_signal_stream_preserves_the_other_stream_and_cursor() {
        let cursor = |value| OpaqueCursor::new(value).unwrap();
        let current: OrchestrationSignalWorkspaceResponse =
            serde_json::from_value(serde_json::json!({
                "zone_signals": [],
                "resource_signals": [{
                    "signal_id": 2,
                    "facility_id": 8,
                    "resource_kind": "general_labor",
                    "available_units": 2,
                    "demand_units": 3,
                    "utilization_basis_points": 15000,
                    "ttl_seconds": 300,
                    "recorded_by": 4,
                    "observed_at": "2026-01-01T00:00:00Z",
                    "expires_at": "2026-01-01T00:05:00Z"
                }],
                "next_zone_cursor": "woz1.old",
                "next_resource_cursor": "wor1.keep"
            }))
            .unwrap();
        let incoming = OrchestrationSignalWorkspaceResponse {
            zone_signals: Vec::new(),
            resource_signals: Vec::new(),
            next_zone_cursor: Some(cursor("woz1.next")),
            next_resource_cursor: Some(cursor("wor1.ignore")),
        };
        let merged = merge_signal_workspace(current, incoming, true, false);
        assert_eq!(merged.resource_signals.len(), 1);
        assert_eq!(merged.next_zone_cursor, Some(cursor("woz1.next")));
        assert_eq!(merged.next_resource_cursor, Some(cursor("wor1.keep")));
    }

    #[test]
    fn worker_paging_appends_options_and_advances_cursor() {
        let option = |employee_id, user_id, name: &str| {
            wareboxes_api_contract::v1::WorkOrchestrationWorkerOptionResponse {
                employee_id,
                user_id,
                display_name: name.into(),
                title: "Operator".into(),
            }
        };
        let current = WorkOrchestrationWorkerPage::new(
            vec![option(1, 11, "Ada")],
            Some(OpaqueCursor::new("wow1.first").unwrap()),
        );
        let incoming = WorkOrchestrationWorkerPage::new(
            vec![option(2, 12, "Grace")],
            Some(OpaqueCursor::new("wow1.second").unwrap()),
        );
        let merged = merge_worker_page(current, incoming);
        assert_eq!(merged.items.len(), 2);
        assert_eq!(
            merged.next_cursor,
            Some(OpaqueCursor::new("wow1.second").unwrap())
        );
    }
}
