use chrono::{DateTime, Utc};
use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    OrchestrationPlanMode, OrchestrationWorkKind, ResourceCapacitySignalResponse,
    WorkOrchestrationMode, WorkOrchestrationPlanItemResponse, WorkOrchestrationPolicyResponse,
    WorkResourceKind, ZoneCongestionSignalResponse,
};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};
use wareboxes_core::models::Location;

use super::forms::Drafts;
use super::{
    load_plan_detail, load_plans, load_policies, load_signals, load_workers, Dialog, Signals,
};

pub(super) fn metrics(signals: Signals) -> AnyView {
    let policies = signals.policies.get();
    let plans = signals.plans.get();
    let operational = signals.operational_signals.get();
    let active_optimized = policies
        .items
        .iter()
        .filter(|policy| {
            policy.effective_to.is_none() && policy.mode == WorkOrchestrationMode::Enabled
        })
        .count();
    let manual_plans = plans
        .items
        .iter()
        .filter(|plan| plan.plan_mode == OrchestrationPlanMode::ManualFifo)
        .count();
    let bottlenecks = operational
        .resource_signals
        .iter()
        .filter(|signal| !signal_is_historical(&signal.expires_at) && is_bottleneck(signal))
        .count();
    let work_in_view = plans.items.iter().map(|plan| plan.item_count).sum::<i64>();
    view! {
        <section class="orchestration-metrics" aria-label="Orchestration totals">
            <div><span>"Active optimized policies"</span><strong>{active_optimized}</strong></div>
            <div><span>"Manual FIFO plans"</span><strong>{manual_plans}</strong></div>
            <div class:bottleneck={bottlenecks > 0}><span>"Resource bottlenecks"</span><strong>{bottlenecks}</strong></div>
            <div><span>"Tasks in plan history"</span><strong>{work_in_view}</strong></div>
        </section>
    }.into_any()
}

pub(super) fn policy_panel(
    signals: Signals,
    drafts: Drafts,
    access: StoredValue<AccessScopeWorkspace>,
    locations: StoredValue<Vec<Location>>,
    can_supervise: bool,
) -> AnyView {
    if signals.policies_loading.get() && !signals.policies_loaded.get() {
        return loading_state("Loading orchestration policies");
    }
    let page = signals.policies.get();
    let next = page.next_cursor.clone();
    let page_complete = next.is_none();
    let selected_facility_id = signals.facility_id.get();
    let requested_owner_id = signals.owner_id.get();
    let resolved_overrides = requested_owner_id.map_or_else(Vec::new, |owner_id| {
        page.items
            .iter()
            .filter(|policy| {
                policy.effective_to.is_none() && policy.inventory_owner_id == Some(owner_id)
            })
            .map(|policy| (policy.facility_id, policy.policy_id))
            .collect::<Vec<_>>()
    });
    let resolution = PolicyResolution {
        selected_facility_id,
        selected_owner_id: requested_owner_id,
        page_complete,
        resolved_overrides: &resolved_overrides,
    };
    view! {
        <section class="orchestration-panel">
            <header><div><p class="eyebrow">"Decision configuration"</p><h2>"Versioned policies"</h2></div><span>{format!("{} in view", page.items.len())}</span></header>
            {if page.items.is_empty() {
                empty_inner("No orchestration policies", "Configure a facility default or client override. Until then, no advisory plan can be generated.")
            } else {
                view! {
                    <div class="table-scroll">
                        <table class="dense-table orchestration-policy-table">
                            <thead><tr><th>"Scope"</th><th>"Mode"</th><th>"Positive weights"</th><th>"Penalty weights"</th><th>"Limits"</th><th>"Version evidence"</th><th></th></tr></thead>
                            <tbody>{page.items.into_iter().map(|policy| {
                                let resolved_for_filter = generation_policy_is_resolved(
                                    &resolution,
                                    PolicyResolutionCandidate {
                                        facility_id: policy.facility_id,
                                        owner_id: policy.inventory_owner_id,
                                        policy_id: policy.policy_id,
                                        active: policy.effective_to.is_none(),
                                    },
                                );
                                policy_row(signals, drafts, access, locations, can_supervise, resolved_for_filter, policy)
                            }).collect_view()}</tbody>
                        </table>
                    </div>
                }.into_any()
            }}
            {next.map(|cursor| view! {
                <button class="button secondary-action compact orchestration-load-more" type="button" disabled=move || signals.policies_loading.get() on:click=move |_| load_policies(signals, Some(cursor.clone()), true)>"Load more policies"</button>
            })}
        </section>
    }.into_any()
}

fn policy_row(
    signals: Signals,
    drafts: Drafts,
    access: StoredValue<AccessScopeWorkspace>,
    locations: StoredValue<Vec<Location>>,
    can_supervise: bool,
    resolved_for_filter: bool,
    policy: WorkOrchestrationPolicyResponse,
) -> AnyView {
    let active = policy.effective_to.is_none();
    let enabled = policy.mode == WorkOrchestrationMode::Enabled;
    let edit_policy = policy.clone();
    let disable_policy = policy.clone();
    let plan_policy = policy.clone();
    let scope = access.with_value(|value| policy_scope(value, &policy));
    let configure = move |_| {
        drafts.reset_policy(Some(&edit_policy), false, signals);
        signals.dialog.set(Some(Dialog::Configure {
            current: Some(edit_policy.clone()),
            disable: false,
        }));
    };
    let disable = move |_| {
        drafts.reset_policy(Some(&disable_policy), true, signals);
        signals.dialog.set(Some(Dialog::Configure {
            current: Some(disable_policy.clone()),
            disable: true,
        }));
    };
    let generate = move |_| {
        drafts.reset_plan(&plan_policy, signals, locations);
        let requested_owner_id = plan_policy
            .inventory_owner_id
            .or_else(|| signals.owner_id.get_untracked());
        load_workers(
            signals,
            plan_policy.facility_id,
            requested_owner_id,
            None,
            false,
        );
        signals
            .dialog
            .set(Some(Dialog::Generate(plan_policy.clone())));
    };
    view! {
        <tr class:historical=!active>
            <td><strong>{scope}</strong><small>{if policy.inventory_owner_id.is_some(){"Client override"}else{"Facility default"}}</small></td>
            <td><span class=policy_mode_class(policy.mode)>{policy_mode_label(policy.mode)}</span><small>{if active {"Current"} else {"Historical"}}</small></td>
            <td class="numeric"><strong>{format!("P{} · D{} · X{} · I{}", policy.priority_weight, policy.due_urgency_weight, policy.proximity_weight, policy.interleaving_weight)}</strong><small>"Priority · due · proximity · interleave"</small></td>
            <td class="numeric"><strong>{format!("C{} · B{}", policy.congestion_penalty_weight, policy.bottleneck_penalty_weight)}</strong><small>"Congestion · bottleneck"</small></td>
            <td><strong>{format!("{} candidates", policy.max_candidates)}</strong><small>{format!("{} min due horizon", policy.due_horizon_minutes)}</small></td>
            <td><strong>{format!("#{} · rev {}", policy.policy_id, policy.revision.get())}</strong><small>{format!("Actor #{} · {}", policy.configured_by, short_timestamp(&policy.configured_at))}</small>{policy.supersedes_policy_id.map(|id| view!{<small>{format!("Supersedes #{id}")}</small>})}</td>
            <td>{(can_supervise && active).then(|| view! {
                <div class="orchestration-row-actions">
                    <button class="button secondary-action compact" type="button" on:click=configure>"Supersede"</button>
                    {(enabled).then(|| view!{<button class="button secondary-action compact fallback" type="button" on:click=disable>"Use manual FIFO"</button>})}
                    {(resolved_for_filter).then(|| view!{<button class="button primary-action compact" type="button" on:click=generate>"Generate plan"</button>})}
                </div>
            })}</td>
        </tr>
    }.into_any()
}

pub(super) fn zone_signal_panel(
    signals: Signals,
    can_supervise: bool,
    open_form: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> AnyView {
    let facility_selected = signals.facility_id.get().is_some();
    let workspace = signals.operational_signals.get();
    let next = workspace.next_zone_cursor.clone();
    let rows = workspace.zone_signals;
    view! {
        <section class="orchestration-panel signal-panel">
            <header><div><p class="eyebrow">"Live input"</p><h2>"Zone congestion"</h2></div>{(can_supervise && facility_selected).then(|| view!{<button class="button secondary-action compact" type="button" on:click=open_form>"Record signal"</button>})}</header>
            {if signals.signals_loading.get() && !signals.signals_loaded.get() {
                inline_loading("Loading congestion")
            } else if !facility_selected {
                empty_inner("Select one facility", "Congestion is read and recorded within a single facility scope.")
            } else if rows.is_empty() {
                empty_inner("No active congestion", "Plans will use zero congestion unless an unexpired zone signal exists.")
            } else {
                view! { <div class="orchestration-signal-list">{rows.into_iter().map(zone_signal).collect_view()}</div> }.into_any()
            }}
            {next.map(|cursor| view!{<button class="button secondary-action compact orchestration-load-more" type="button" disabled=move || signals.signals_loading.get() on:click=move |_| load_signals(signals,Some(cursor.clone()),None,true,false)>"Load more congestion history"</button>})}
        </section>
    }.into_any()
}

fn zone_signal(signal: ZoneCongestionSignalResponse) -> AnyView {
    let pressure = basis_points(signal.congestion_basis_points);
    let historical = signal_is_historical(&signal.expires_at);
    view! {
        <article class="orchestration-signal" class:historical=historical>
            <div><strong>{signal.storage_zone_code}</strong><small>{format!("Signal #{} · queue {}", signal.signal_id, signal.queue_depth)}<span class=if historical{"status-badge neutral"}else{"status-badge success"}>{if historical{"Historical"}else{"Active"}}</span></small></div>
            <div class="signal-pressure"><strong>{pressure}</strong><small>"congested"</small></div>
            <div><span>"Observed / expires"</span><small>{format!("{} / {}",short_timestamp(&signal.observed_at),short_timestamp(&signal.expires_at))}</small></div>
        </article>
    }.into_any()
}

pub(super) fn resource_signal_panel(
    signals: Signals,
    can_supervise: bool,
    open_form: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> AnyView {
    let facility_selected = signals.facility_id.get().is_some();
    let workspace = signals.operational_signals.get();
    let next = workspace.next_resource_cursor.clone();
    let rows = workspace.resource_signals;
    view! {
        <section class="orchestration-panel signal-panel">
            <header><div><p class="eyebrow">"Live input"</p><h2>"Resource capacity"</h2></div>{(can_supervise && facility_selected).then(|| view!{<button class="button secondary-action compact" type="button" on:click=open_form>"Record signal"</button>})}</header>
            {if signals.signals_loading.get() && !signals.signals_loaded.get() {
                inline_loading("Loading capacity")
            } else if !facility_selected {
                empty_inner("Select one facility", "Capacity is read and recorded within a single facility scope.")
            } else if rows.is_empty() {
                empty_inner("No active capacity signals", "Plans assume unconstrained resources until an unexpired signal exists.")
            } else {
                view! { <div class="orchestration-signal-list">{rows.into_iter().map(resource_signal).collect_view()}</div> }.into_any()
            }}
            {next.map(|cursor| view!{<button class="button secondary-action compact orchestration-load-more" type="button" disabled=move || signals.signals_loading.get() on:click=move |_| load_signals(signals,None,Some(cursor.clone()),false,true)>"Load more capacity history"</button>})}
        </section>
    }.into_any()
}

fn resource_signal(signal: ResourceCapacitySignalResponse) -> AnyView {
    let bottleneck = is_bottleneck(&signal);
    let historical = signal_is_historical(&signal.expires_at);
    view! {
        <article class="orchestration-signal" class:bottleneck={bottleneck && !historical} class:historical=historical>
            <div><strong>{resource_label(signal.resource_kind)}</strong><small>{format!("Signal #{}", signal.signal_id)}<span class=if historical{"status-badge neutral"}else{"status-badge success"}>{if historical{"Historical"}else{"Active"}}</span></small></div>
            <div class="signal-pressure"><strong>{basis_points(signal.utilization_basis_points)}</strong><small>"utilized"</small></div>
            <div><span>{if historical {"Expired"} else if bottleneck {"Bottleneck"} else {"Capacity"}}</span><small>{format!("{} available / {} demand · observed {} · expires {}", signal.available_units, signal.demand_units,short_timestamp(&signal.observed_at),short_timestamp(&signal.expires_at))}</small></div>
        </article>
    }.into_any()
}

pub(super) fn plan_panel(signals: Signals, access: StoredValue<AccessScopeWorkspace>) -> AnyView {
    if signals.plans_loading.get() && !signals.plans_loaded.get() {
        return loading_state("Loading plan history");
    }
    let page = signals.plans.get();
    let next = page.next_cursor.clone();
    view! {
        <section class="orchestration-panel plan-history">
            <header><div><p class="eyebrow">"Immutable output"</p><h2>"Plan history"</h2></div><span>{format!("{} in view", page.items.len())}</span></header>
            {if page.items.is_empty() {
                empty_inner("No advisory plans", "Generate a plan from an active policy to freeze current task, congestion, and capacity evidence.")
            } else {
                view! {
                    <div class="table-scroll"><table class="dense-table"><thead><tr><th>"Plan"</th><th>"Scope / position"</th><th>"Mode"</th><th>"Candidate evidence"</th><th>"Policy"</th><th>"Generated"</th><th></th></tr></thead><tbody>{page.items.into_iter().map(|plan| { let plan_id=plan.plan_id; let scope=access.with_value(|value| plan_scope(value, plan.facility_id, plan.requested_inventory_owner_id)); view!{<tr class:selected=signals.selected_plan.get().as_ref().is_some_and(|selected|selected.plan_id==plan.plan_id)><td><strong>{format!("#{}",plan.plan_id)}</strong>{plan.generated_for_user_id.map(|id|view!{<small>{format!("Worker #{id}")}</small>})}</td><td><strong>{scope}</strong><small>{plan.current_location_label}</small></td><td><span class=plan_mode_class(plan.plan_mode)>{plan_mode_label(plan.plan_mode)}</span>{(plan.plan_mode==OrchestrationPlanMode::ManualFifo).then(||view!{<small>"Safe fallback"</small>})}</td><td><strong>{format!("{} ranked / {} eligible",plan.item_count,plan.candidate_count)}</strong><small>{short_timestamp(&plan.input_snapshot_at)}</small></td><td><strong>{format!("#{} · rev {}",plan.policy_id,plan.policy_revision.get())}</strong><small>{if plan.policy_inventory_owner_id.is_some(){"Client override"}else{"Facility default"}}</small></td><td><strong>{short_timestamp(&plan.generated_at)}</strong><small>{format!("Actor #{}",plan.generated_by)}</small></td><td><button class="button secondary-action compact" type="button" disabled=move || signals.detail_loading.get() on:click=move |_| load_plan_detail(signals,plan_id)>"Inspect evidence"</button></td></tr>} }).collect_view()}</tbody></table></div>
                }.into_any()
            }}
            {next.map(|cursor| view!{<button class="button secondary-action compact orchestration-load-more" type="button" disabled=move || signals.plans_loading.get() on:click=move |_| load_plans(signals,Some(cursor.clone()),true)>"Load more plans"</button>})}
        </section>
    }.into_any()
}

pub(super) fn plan_detail(signals: Signals, access: StoredValue<AccessScopeWorkspace>) -> AnyView {
    let Some(plan) = signals.selected_plan.get() else {
        return view! { <section class="orchestration-detail-empty"><strong>"Select a plan to inspect its frozen decision evidence."</strong><span>"Plans are advisory snapshots; viewing one never claims or changes work."</span></section> }.into_any();
    };
    let scope = access
        .with_value(|value| plan_scope(value, plan.facility_id, plan.requested_inventory_owner_id));
    let configuration = serde_json::to_string_pretty(&plan.configuration_snapshot)
        .unwrap_or_else(|_| "Configuration snapshot unavailable".into());
    let fallback = plan.plan_mode == OrchestrationPlanMode::ManualFifo;
    view! {
        <section class="orchestration-plan-detail">
            <header>
                <div><p class="eyebrow">"Frozen plan evidence"</p><h2>{format!("Plan #{}",plan.plan_id)}</h2><span>{format!("{} · current position {}",scope,plan.current_location_label)}</span></div>
                <div class="orchestration-detail-summary"><span class=plan_mode_class(plan.plan_mode)>{plan_mode_label(plan.plan_mode)}</span><strong>{format!("{} tasks",plan.item_count)}</strong><small>{format!("{} eligible candidates",plan.candidate_count)}</small></div>
            </header>
            {if fallback {
                view!{<div class="orchestration-fallback-banner"><strong>"Manual FIFO fallback was intentional"</strong><span>{format!("Policy #{} revision {} had optimization disabled. Eligible tasks remain unclaimed and are ordered by their canonical FIFO sequence.",plan.policy_id,plan.policy_revision.get())}</span></div>}.into_any()
            } else {
                view!{<div class="orchestration-advisory-banner"><strong>"Optimized, not auto-assigned"</strong><span>"Scores explain the proposed sequence. Canonical tasks remain independently claimable through their owning workflows."</span></div>}.into_any()
            }}
            <div class="orchestration-plan-metadata">
                <div><span>"Policy"</span><strong>{format!("#{} · rev {}",plan.policy_id,plan.policy_revision.get())}</strong></div>
                <div><span>"Input snapshot"</span><strong>{short_timestamp(&plan.input_snapshot_at)}</strong></div>
                <div><span>"Previous work"</span><strong>{plan.previous_work_kind.map_or("None",work_kind_label)}</strong></div>
                <div><span>"Generated"</span><strong>{format!("{} · actor #{}",short_timestamp(&plan.generated_at),plan.generated_by)}</strong></div>
            </div>
            <details class="orchestration-configuration"><summary>"Frozen configuration snapshot"</summary><pre>{configuration}</pre></details>
            <div class="orchestration-ranked-list">
                {plan.items.into_iter().map(plan_item).collect_view()}
            </div>
        </section>
    }.into_any()
}

fn plan_item(item: WorkOrchestrationPlanItemResponse) -> AnyView {
    let bottleneck = item.evidence.resource_demand_units > item.evidence.resource_available_units
        && item.evidence.resource_demand_units > 0;
    let destination = item
        .destination_location_label
        .clone()
        .unwrap_or_else(|| "Same location".into());
    view! {
        <article class="orchestration-plan-item" class:bottleneck=bottleneck>
            <header><span class="sequence">{format!("{:02}",item.sequence)}</span><div><strong>{item.title}</strong><small>{format!("{} · task #{} · {} · created {}",work_kind_label(item.work_kind),item.work_task_id,item.task_status,short_timestamp(&item.task_created_at))}</small></div><div class="total-score"><span>"Total score"</span><strong>{item.score.total}</strong></div></header>
            <div class="orchestration-route"><div><span>"Source"</span><strong>{item.source_location_label}</strong></div><span>"→"</span><div><span>"Destination"</span><strong>{destination}</strong></div><div><span>"Travel"</span><strong>{format!("{} sequence units",item.evidence.travel_distance)}</strong></div></div>
            {item.instructions.map(|instructions|view!{<p class="orchestration-instructions">{instructions}</p>})}
            <div class="orchestration-score-grid">
                {score_cell("Priority",item.score.priority_component,false)}
                {score_cell("Due urgency",item.score.due_urgency_component,false)}
                {score_cell("Proximity",item.score.proximity_component,false)}
                {score_cell("Interleaving",item.score.interleaving_component,false)}
                {score_cell("Congestion",item.score.congestion_penalty,true)}
                {score_cell("Bottleneck",item.score.bottleneck_penalty,true)}
            </div>
            <details class="orchestration-evidence"><summary>"Task, signal, and score inputs"</summary><dl><div><dt>"FIFO audit"</dt><dd>{format!("Task priority {} · created {}",item.evidence.task_priority,short_timestamp(&item.task_created_at))}</dd></div><div><dt>"Due evidence"</dt><dd>{format!("Due {} · {} urgency · {}s overdue",item.evidence.due_at.as_deref().map(short_timestamp).unwrap_or_else(||"not set".into()),basis_points(item.evidence.due_urgency_basis_points),item.evidence.overdue_seconds)}</dd></div><div><dt>"Proximity"</dt><dd>{format!("{} · {} → {} → {}",basis_points(item.evidence.proximity_basis_points),item.evidence.current_travel_sequence,item.evidence.source_travel_sequence,item.evidence.destination_travel_sequence.map_or("-".into(),|v|v.to_string()))}</dd></div><div><dt>"Congestion"</dt><dd>{format!("{} · queue {} · {}",basis_points(item.evidence.congestion_basis_points),item.evidence.congestion_queue_depth,item.evidence.source_zone_code.clone().unwrap_or_else(||"No zone".into()))}</dd></div><div class:bottleneck=bottleneck><dt>"Resource capacity"</dt><dd>{format!("{} · {} available / {} demand · {}",resource_label(item.evidence.resource_kind),item.evidence.resource_available_units,item.evidence.resource_demand_units,basis_points(item.evidence.resource_utilization_basis_points))}</dd></div><div><dt>"Interleaving"</dt><dd>{if item.evidence.interleaving_compatible{"Compatible with previous work"}else{"No compatibility bonus"}}</dd></div><div><dt>"Frozen signal IDs"</dt><dd>{format!("Zone {} · resource {}",item.zone_signal_id.map_or("none".into(),|id|format!("#{id}")),item.resource_signal_id.map_or("none".into(),|id|format!("#{id}")))}</dd></div></dl></details>
        </article>
    }.into_any()
}

fn score_cell(label: &'static str, value: i64, penalty: bool) -> AnyView {
    view!{<div class:penalty=penalty><span>{label}</span><strong>{if penalty{format!("−{}",value.abs())}else{format!("+{value}")}}</strong></div>}.into_any()
}

fn is_bottleneck(signal: &ResourceCapacitySignalResponse) -> bool {
    signal.demand_units > signal.available_units && signal.demand_units > 0
}

fn policy_scope(access: &AccessScopeWorkspace, policy: &WorkOrchestrationPolicyResponse) -> String {
    let facility = scope_name(&access.facilities, policy.facility_id);
    policy
        .inventory_owner_id
        .map_or(facility.clone(), |owner_id| {
            format!(
                "{} · {facility}",
                scope_name(&access.inventory_owners, owner_id)
            )
        })
}

fn plan_scope(access: &AccessScopeWorkspace, facility_id: i64, owner_id: Option<i64>) -> String {
    let facility = scope_name(&access.facilities, facility_id);
    owner_id.map_or_else(
        || format!("Facility-shared / ownerless work · {facility}"),
        |id| format!("{} · {facility}", scope_name(&access.inventory_owners, id)),
    )
}

#[derive(Clone, Copy)]
struct PolicyResolution<'a> {
    selected_facility_id: Option<i64>,
    selected_owner_id: Option<i64>,
    page_complete: bool,
    resolved_overrides: &'a [(i64, i64)],
}

#[derive(Clone, Copy)]
struct PolicyResolutionCandidate {
    facility_id: i64,
    owner_id: Option<i64>,
    policy_id: i64,
    active: bool,
}

fn generation_policy_is_resolved(
    resolution: &PolicyResolution<'_>,
    candidate: PolicyResolutionCandidate,
) -> bool {
    if !candidate.active
        || !resolution.page_complete
        || resolution.selected_facility_id != Some(candidate.facility_id)
    {
        return false;
    }
    match resolution.selected_owner_id {
        None => candidate.owner_id.is_none(),
        Some(owner_id) => resolution
            .resolved_overrides
            .iter()
            .find(|(facility_id, _)| *facility_id == candidate.facility_id)
            .map_or(candidate.owner_id.is_none(), |(_, override_id)| {
                candidate.owner_id == Some(owner_id) && candidate.policy_id == *override_id
            }),
    }
}

pub(super) fn scope_options(values: &[AccessScopeResource]) -> AnyView {
    values
        .iter()
        .map(|row| view! {<option value=row.id>{row.name.clone()}</option>})
        .collect_view()
        .into_any()
}

pub(super) fn owner_scope_options(
    access: &AccessScopeWorkspace,
    facility_id: Option<i64>,
) -> AnyView {
    access
        .inventory_owners
        .iter()
        .filter(|owner| owner_is_allowed(access, facility_id, Some(owner.id)))
        .map(|owner| view! {<option value=owner.id>{owner.name.clone()}</option>})
        .collect_view()
        .into_any()
}

pub(super) fn owner_is_allowed(
    access: &AccessScopeWorkspace,
    facility_id: Option<i64>,
    owner_id: Option<i64>,
) -> bool {
    let Some(owner_id) = owner_id else {
        return true;
    };
    access.owner_facilities.iter().any(|link| {
        link.inventory_owner_id == owner_id
            && facility_id.is_none_or(|facility_id| link.facility_id == facility_id)
    })
}

fn scope_name(values: &[AccessScopeResource], id: i64) -> String {
    values
        .iter()
        .find(|row| row.id == id)
        .map_or_else(|| format!("#{id}"), |row| row.name.clone())
}

pub(super) fn option_id(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}

pub(super) fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}

pub(super) const fn policy_mode_label(value: WorkOrchestrationMode) -> &'static str {
    match value {
        WorkOrchestrationMode::Enabled => "Optimized advisory",
        WorkOrchestrationMode::Disabled => "Manual FIFO fallback",
    }
}

fn policy_mode_class(value: WorkOrchestrationMode) -> &'static str {
    match value {
        WorkOrchestrationMode::Enabled => "status-badge success",
        WorkOrchestrationMode::Disabled => "status-badge warning",
    }
}

pub(super) fn plan_mode_wire(value: Option<OrchestrationPlanMode>) -> &'static str {
    match value {
        Some(OrchestrationPlanMode::Optimized) => "optimized",
        Some(OrchestrationPlanMode::ManualFifo) => "manual_fifo",
        None => "",
    }
}

pub(super) fn parse_plan_mode(value: &str) -> Option<OrchestrationPlanMode> {
    match value {
        "optimized" => Some(OrchestrationPlanMode::Optimized),
        "manual_fifo" => Some(OrchestrationPlanMode::ManualFifo),
        _ => None,
    }
}

fn plan_mode_label(value: OrchestrationPlanMode) -> &'static str {
    match value {
        OrchestrationPlanMode::Optimized => "Optimized",
        OrchestrationPlanMode::ManualFifo => "Manual FIFO",
    }
}

fn plan_mode_class(value: OrchestrationPlanMode) -> &'static str {
    match value {
        OrchestrationPlanMode::Optimized => "status-badge success",
        OrchestrationPlanMode::ManualFifo => "status-badge warning",
    }
}

pub(super) const fn resource_label(value: WorkResourceKind) -> &'static str {
    match value {
        WorkResourceKind::GeneralLabor => "General labor",
        WorkResourceKind::InventoryControl => "Inventory control",
        WorkResourceKind::MaterialHandling => "Material handling",
        WorkResourceKind::DockDoor => "Dock door",
        WorkResourceKind::PackStation => "Pack station",
        WorkResourceKind::Automation => "Automation",
    }
}

pub(super) const fn work_kind_label(value: OrchestrationWorkKind) -> &'static str {
    match value {
        OrchestrationWorkKind::CycleCountItemLocation => "Item/location count",
        OrchestrationWorkKind::CycleCountLocation => "Location count",
        OrchestrationWorkKind::Putaway => "Putaway",
        OrchestrationWorkKind::LicensePlatePutaway => "License plate putaway",
        OrchestrationWorkKind::InventoryRelocation => "Inventory relocation",
        OrchestrationWorkKind::Replenishment => "Replenishment",
        OrchestrationWorkKind::CrossDock => "Cross-dock",
    }
}

fn basis_points(value: u16) -> String {
    format!("{}.{:02}%", value / 100, value % 100)
}

fn short_timestamp(value: &str) -> String {
    value
        .replace('T', " ")
        .trim_end_matches('Z')
        .chars()
        .take(19)
        .collect()
}

fn signal_is_historical(expires_at: &str) -> bool {
    signal_is_historical_at(expires_at, Utc::now())
}

fn signal_is_historical_at(expires_at: &str, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(expires_at)
        .map(|expires| expires <= now)
        .unwrap_or(false)
}

fn loading_state(label: &'static str) -> AnyView {
    view!{<section class="orchestration-state"><span class="loading-line"></span><h2>{label}</h2></section>}.into_any()
}

fn inline_loading(label: &'static str) -> AnyView {
    view!{<div class="orchestration-inline-state"><span class="loading-line"></span><span>{label}</span></div>}.into_any()
}

fn empty_inner(title: &'static str, message: &'static str) -> AnyView {
    view!{<div class="orchestration-inline-state"><strong>{title}</strong><span>{message}</span></div>}.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_pressure_labels_are_exact() {
        assert_eq!(basis_points(0), "0.00%");
        assert_eq!(basis_points(9_375), "93.75%");
        assert_eq!(basis_points(10_000), "100.00%");
        assert_eq!(
            parse_plan_mode("manual_fifo"),
            Some(OrchestrationPlanMode::ManualFifo)
        );
    }

    #[test]
    fn generation_requires_a_complete_exact_single_facility_resolution() {
        let overrides = [(8, 21)];
        let exact = PolicyResolution {
            selected_facility_id: Some(8),
            selected_owner_id: Some(12),
            page_complete: true,
            resolved_overrides: &overrides,
        };
        let override_candidate = PolicyResolutionCandidate {
            facility_id: 8,
            owner_id: Some(12),
            policy_id: 21,
            active: true,
        };
        let default_candidate = PolicyResolutionCandidate {
            facility_id: 8,
            owner_id: None,
            policy_id: 20,
            active: true,
        };
        assert!(generation_policy_is_resolved(&exact, override_candidate));
        assert!(!generation_policy_is_resolved(&exact, default_candidate));
        let incomplete = PolicyResolution {
            page_complete: false,
            ..exact
        };
        assert!(!generation_policy_is_resolved(
            &incomplete,
            override_candidate
        ));
        let ownerless = PolicyResolution {
            selected_facility_id: Some(8),
            selected_owner_id: None,
            page_complete: true,
            resolved_overrides: &[],
        };
        assert!(generation_policy_is_resolved(&ownerless, default_candidate));
        let no_facility = PolicyResolution {
            selected_facility_id: None,
            ..ownerless
        };
        assert!(!generation_policy_is_resolved(
            &no_facility,
            default_candidate
        ));
    }

    #[test]
    fn expired_signal_history_is_not_live_pressure() {
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(signal_is_historical_at("2026-01-01T00:05:00Z", now));
        assert!(!signal_is_historical_at("2026-01-01T00:15:00Z", now));
        assert!(!signal_is_historical_at("invalid", now));
    }

    #[test]
    fn owner_choices_require_an_active_authorized_facility_link() {
        let access = AccessScopeWorkspace {
            facilities: vec![AccessScopeResource {
                id: 8,
                name: "Reno".into(),
            }],
            inventory_owners: vec![
                AccessScopeResource {
                    id: 12,
                    name: "Northwind".into(),
                },
                AccessScopeResource {
                    id: 13,
                    name: "Contoso".into(),
                },
            ],
            owner_facilities: vec![wareboxes_api_contract::web::access::AccessOwnerFacility {
                inventory_owner_id: 12,
                facility_id: 8,
            }],
        };
        assert!(owner_is_allowed(&access, Some(8), Some(12)));
        assert!(!owner_is_allowed(&access, Some(8), Some(13)));
        assert!(owner_is_allowed(&access, Some(8), None));
    }
}
