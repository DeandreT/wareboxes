use std::collections::BTreeMap;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ActivateWorkOrchestrationDispatchRequest, CancelWorkOrchestrationDispatchRequest,
    ConfigureWorkOrchestrationPolicyRequest, GenerateWorkOrchestrationPlanRequest,
    OrchestrationWorkKind, RecordResourceCapacitySignalRequest, RecordZoneCongestionSignalRequest,
    WorkOrchestrationDispatchCancellationReason, WorkOrchestrationDispatchResponse,
    WorkOrchestrationMode, WorkOrchestrationPlanResponse, WorkOrchestrationPolicyResponse,
    WorkResourceKind,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::Location;

use super::{dispatch, Dialog, PendingCommand, Signals};
use crate::api;
use crate::components::{Icon, UiIcon};

#[derive(Clone, Copy)]
pub(super) struct Drafts {
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    mode: RwSignal<WorkOrchestrationMode>,
    priority_weight: RwSignal<String>,
    due_weight: RwSignal<String>,
    proximity_weight: RwSignal<String>,
    interleaving_weight: RwSignal<String>,
    congestion_weight: RwSignal<String>,
    bottleneck_weight: RwSignal<String>,
    due_horizon: RwSignal<String>,
    max_candidates: RwSignal<String>,
    storage_zone_id: RwSignal<Option<i64>>,
    congestion_basis_points: RwSignal<String>,
    queue_depth: RwSignal<String>,
    congestion_ttl: RwSignal<String>,
    resource_kind: RwSignal<WorkResourceKind>,
    available_units: RwSignal<String>,
    demand_units: RwSignal<String>,
    resource_ttl: RwSignal<String>,
    current_location_id: RwSignal<Option<i64>>,
    previous_work_kind: RwSignal<Option<OrchestrationWorkKind>>,
    generated_for_user_id: RwSignal<Option<i64>>,
    cancellation_reason: RwSignal<WorkOrchestrationDispatchCancellationReason>,
    cancellation_note: RwSignal<String>,
}

impl Drafts {
    pub(super) fn new() -> Self {
        Self {
            facility_id: RwSignal::new(None),
            owner_id: RwSignal::new(None),
            mode: RwSignal::new(WorkOrchestrationMode::Enabled),
            priority_weight: RwSignal::new("100".into()),
            due_weight: RwSignal::new("80".into()),
            proximity_weight: RwSignal::new("60".into()),
            interleaving_weight: RwSignal::new("30".into()),
            congestion_weight: RwSignal::new("50".into()),
            bottleneck_weight: RwSignal::new("50".into()),
            due_horizon: RwSignal::new("120".into()),
            max_candidates: RwSignal::new("100".into()),
            storage_zone_id: RwSignal::new(None),
            congestion_basis_points: RwSignal::new("0".into()),
            queue_depth: RwSignal::new("0".into()),
            congestion_ttl: RwSignal::new("300".into()),
            resource_kind: RwSignal::new(WorkResourceKind::GeneralLabor),
            available_units: RwSignal::new("0".into()),
            demand_units: RwSignal::new("0".into()),
            resource_ttl: RwSignal::new("300".into()),
            current_location_id: RwSignal::new(None),
            previous_work_kind: RwSignal::new(None),
            generated_for_user_id: RwSignal::new(None),
            cancellation_reason: RwSignal::new(
                WorkOrchestrationDispatchCancellationReason::OperatorCancelled,
            ),
            cancellation_note: RwSignal::new(String::new()),
        }
    }

    pub(super) fn reset_policy(
        self,
        current: Option<&WorkOrchestrationPolicyResponse>,
        disable: bool,
        signals: Signals,
    ) {
        self.facility_id.set(
            current
                .map(|value| value.facility_id)
                .or_else(|| signals.facility_id.get_untracked()),
        );
        self.owner_id.set(policy_owner_draft(
            current,
            signals.owner_id.get_untracked(),
        ));
        self.mode.set(if disable {
            WorkOrchestrationMode::Disabled
        } else {
            current.map_or(WorkOrchestrationMode::Enabled, |value| value.mode)
        });
        self.priority_weight.set(
            current
                .map_or(100, |value| value.priority_weight)
                .to_string(),
        );
        self.due_weight.set(
            current
                .map_or(80, |value| value.due_urgency_weight)
                .to_string(),
        );
        self.proximity_weight.set(
            current
                .map_or(60, |value| value.proximity_weight)
                .to_string(),
        );
        self.interleaving_weight.set(
            current
                .map_or(30, |value| value.interleaving_weight)
                .to_string(),
        );
        self.congestion_weight.set(
            current
                .map_or(50, |value| value.congestion_penalty_weight)
                .to_string(),
        );
        self.bottleneck_weight.set(
            current
                .map_or(50, |value| value.bottleneck_penalty_weight)
                .to_string(),
        );
        self.due_horizon.set(
            current
                .map_or(120, |value| value.due_horizon_minutes)
                .to_string(),
        );
        self.max_candidates.set(
            current
                .map_or(100, |value| value.max_candidates)
                .to_string(),
        );
        clear_command_state(signals);
    }

    pub(super) fn reset_congestion(self) {
        self.storage_zone_id.set(None);
        self.congestion_basis_points.set("0".into());
        self.queue_depth.set("0".into());
        self.congestion_ttl.set("300".into());
    }

    pub(super) fn reset_resource(self) {
        self.resource_kind.set(WorkResourceKind::GeneralLabor);
        self.available_units.set("0".into());
        self.demand_units.set("0".into());
        self.resource_ttl.set("300".into());
    }

    pub(super) fn reset_plan(
        self,
        policy: &WorkOrchestrationPolicyResponse,
        signals: Signals,
        locations: StoredValue<Vec<Location>>,
    ) {
        self.facility_id.set(Some(policy.facility_id));
        self.owner_id.set(
            policy
                .inventory_owner_id
                .or_else(|| signals.owner_id.get_untracked()),
        );
        let first_location = locations.with_value(|values| {
            values
                .iter()
                .find(|location| location.facility_id == policy.facility_id && location.active)
                .map(|location| location.id)
        });
        self.current_location_id.set(first_location);
        self.previous_work_kind.set(None);
        self.generated_for_user_id.set(None);
        clear_command_state(signals);
    }

    pub(super) fn reset_cancellation(self, signals: Signals) {
        self.cancellation_reason
            .set(WorkOrchestrationDispatchCancellationReason::OperatorCancelled);
        self.cancellation_note.set(String::new());
        clear_command_state(signals);
    }
}

pub(super) fn command_dialog(
    signals: Signals,
    drafts: Drafts,
    access: StoredValue<AccessScopeWorkspace>,
    locations: StoredValue<Vec<Location>>,
    dialog: Dialog,
) -> AnyView {
    let close = move |_| {
        if !signals.command_pending.get_untracked() {
            signals.dialog.set(None);
            clear_command_state(signals);
        }
    };
    let body = match dialog.clone() {
        Dialog::Configure { current, disable } => {
            configure_form(signals, drafts, access, current, disable)
        }
        Dialog::Congestion => congestion_form(signals, drafts, locations),
        Dialog::Resource => resource_form(signals, drafts),
        Dialog::Generate(policy) => generate_form(signals, drafts, locations, policy),
        Dialog::Activate(plan) => activate_form(signals, *plan),
        Dialog::Cancel(dispatch) => cancel_form(signals, drafts, dispatch),
    };
    let title = match dialog {
        Dialog::Configure {
            current: Some(_),
            disable: true,
        } => "Use manual FIFO fallback",
        Dialog::Configure {
            current: Some(_), ..
        } => "Supersede orchestration policy",
        Dialog::Configure { current: None, .. } => "Configure orchestration policy",
        Dialog::Congestion => "Record zone congestion",
        Dialog::Resource => "Record resource capacity",
        Dialog::Generate(_) => "Generate advisory plan",
        Dialog::Activate(_) => "Activate worker dispatch",
        Dialog::Cancel(_) => "Cancel worker dispatch",
    };
    view! {
        <div class="orchestration-dialog-backdrop" role="presentation">
            <section class="orchestration-dialog" role="dialog" aria-modal="true" aria-label=title>
                <header>
                    <div><p class="eyebrow">"Work orchestration"</p><h2>{title}</h2></div>
                    <button
                        class="icon-button"
                        type="button"
                        aria-label="Close dialog"
                        disabled=move || signals.command_pending.get()
                        on:click=close
                    ><Icon icon=UiIcon::Close/></button>
                </header>
                {body}
            </section>
        </div>
    }
    .into_any()
}

fn configure_form(
    signals: Signals,
    drafts: Drafts,
    access: StoredValue<AccessScopeWorkspace>,
    current: Option<WorkOrchestrationPolicyResponse>,
    disable: bool,
) -> AnyView {
    let expected_revision = current.as_ref().map(|value| value.revision);
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let request = (|| {
            let facility_id = drafts
                .facility_id
                .get_untracked()
                .ok_or_else(|| "Select a facility.".to_owned())?;
            let inventory_owner_id = drafts.owner_id.get_untracked();
            if !access.with_value(|value| {
                super::display::owner_is_allowed(value, Some(facility_id), inventory_owner_id)
            }) {
                return Err("Select a client actively assigned to this facility.".to_owned());
            }
            let max_candidates = parsed_positive::<u16>(drafts.max_candidates, "candidate limit")?;
            let due_horizon_minutes = parsed_positive::<u32>(drafts.due_horizon, "due horizon")?;
            Ok::<_, String>(ConfigureWorkOrchestrationPolicyRequest {
                facility_id,
                inventory_owner_id,
                mode: drafts.mode.get_untracked(),
                priority_weight: parsed(drafts.priority_weight, "priority weight")?,
                due_urgency_weight: parsed(drafts.due_weight, "due urgency weight")?,
                proximity_weight: parsed(drafts.proximity_weight, "proximity weight")?,
                interleaving_weight: parsed(drafts.interleaving_weight, "interleaving weight")?,
                congestion_penalty_weight: parsed(
                    drafts.congestion_weight,
                    "congestion penalty weight",
                )?,
                bottleneck_penalty_weight: parsed(
                    drafts.bottleneck_weight,
                    "bottleneck penalty weight",
                )?,
                due_horizon_minutes,
                max_candidates,
                expected_revision,
            })
        })();
        match request {
            Ok(request) => dispatch(
                signals,
                PendingCommand::Configure(request, api::new_idempotency_key()),
            ),
            Err(message) => signals.command_error.set(Some(message)),
        }
    };
    view! {
        <form on:submit=submit>
            <Show when=move || drafts.mode.get() == WorkOrchestrationMode::Disabled>
                <div class="orchestration-fallback-warning">
                    <strong>"Safe fallback remains explicit"</strong>
                    <span>"New plans will preserve policy evidence and present eligible work in manual FIFO order. No task is auto-claimed or mutated."</span>
                </div>
            </Show>
            <div class="orchestration-form-grid">
                <label><span>"Facility"</span><select required disabled=current.is_some() prop:value=move || super::display::option_id(drafts.facility_id.get()) on:change=move |event| { let facility_id=super::display::parse_id(&event_target_value(&event)); drafts.facility_id.set(facility_id); let owner_id=drafts.owner_id.get_untracked(); if !access.with_value(|value|super::display::owner_is_allowed(value,facility_id,owner_id)){drafts.owner_id.set(None);} }><option value="">"Select facility"</option>{access.with_value(|value| super::display::scope_options(&value.facilities))}</select></label>
                <label><span>"Client override"</span><select disabled=current.is_some() prop:value=move || super::display::option_id(drafts.owner_id.get()) on:change=move |event| drafts.owner_id.set(super::display::parse_id(&event_target_value(&event)))><option value="">"Facility default"</option>{move || access.with_value(|value| super::display::owner_scope_options(value,drafts.facility_id.get()))}</select></label>
                <label><span>"Mode"</span><select disabled=disable prop:value=move || policy_mode_wire(drafts.mode.get()) on:change=move |event| drafts.mode.set(parse_policy_mode(&event_target_value(&event)))><option value="enabled">"Optimized advisory"</option><option value="disabled">"Manual FIFO fallback"</option></select></label>
                {number_input("Maximum candidates", drafts.max_candidates)}
                {number_input("Priority weight", drafts.priority_weight)}
                {number_input("Due urgency weight", drafts.due_weight)}
                {number_input("Proximity weight", drafts.proximity_weight)}
                {number_input("Interleaving weight", drafts.interleaving_weight)}
                {number_input("Congestion penalty weight", drafts.congestion_weight)}
                {number_input("Bottleneck penalty weight", drafts.bottleneck_weight)}
                {number_input("Due horizon (minutes)", drafts.due_horizon)}
            </div>
            {command_feedback(signals)}
            <footer>
                <button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button>
                <button class=move || if drafts.mode.get() == WorkOrchestrationMode::Disabled { "button danger-action" } else { "button primary-action" } type="submit" disabled=move || signals.command_pending.get()>{move || if signals.command_pending.get() { "Saving..." } else if drafts.mode.get() == WorkOrchestrationMode::Disabled { "Confirm manual FIFO" } else { "Save policy" }}</button>
            </footer>
        </form>
    }.into_any()
}

fn congestion_form(
    signals: Signals,
    drafts: Drafts,
    locations: StoredValue<Vec<Location>>,
) -> AnyView {
    let facility_id = signals.facility_id.get_untracked();
    let zones = locations.with_value(|values| zone_options(values, facility_id));
    if drafts.storage_zone_id.get_untracked().is_none() {
        drafts
            .storage_zone_id
            .set(zones.first().map(|value| value.0));
    }
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let request = (|| {
            let facility_id = signals
                .facility_id
                .get_untracked()
                .ok_or_else(|| "Select one facility before recording a signal.".to_owned())?;
            let storage_zone_id = drafts
                .storage_zone_id
                .get_untracked()
                .ok_or_else(|| "Select a configured storage zone.".to_owned())?;
            let congestion_basis_points =
                parsed::<u16>(drafts.congestion_basis_points, "congestion basis points")?;
            if congestion_basis_points > 10_000 {
                return Err("Congestion basis points must be between 0 and 10,000.".to_owned());
            }
            Ok::<_, String>(RecordZoneCongestionSignalRequest {
                facility_id,
                storage_zone_id,
                congestion_basis_points,
                queue_depth: parsed(drafts.queue_depth, "queue depth")?,
                ttl_seconds: parsed_positive(drafts.congestion_ttl, "signal TTL")?,
            })
        })();
        match request {
            Ok(request) => dispatch(
                signals,
                PendingCommand::Congestion(request, api::new_idempotency_key()),
            ),
            Err(message) => signals.command_error.set(Some(message)),
        }
    };
    view! {
        <form on:submit=submit>
            <p class="orchestration-form-intro">"Signals are immutable, expire automatically, and are frozen into every generated plan that uses them."</p>
            <div class="orchestration-form-grid">
                <label class="wide"><span>"Storage zone"</span><select required prop:value=move || super::display::option_id(drafts.storage_zone_id.get()) on:change=move |event| drafts.storage_zone_id.set(super::display::parse_id(&event_target_value(&event)))><option value="">"Select zone"</option>{zones.into_iter().map(|(id, label)| view!{<option value=id>{label}</option>}).collect_view()}</select></label>
                {number_input("Congestion (basis points)", drafts.congestion_basis_points)}
                {number_input("Queue depth", drafts.queue_depth)}
                {number_input("TTL (seconds)", drafts.congestion_ttl)}
            </div>
            {command_feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>{move || if signals.command_pending.get(){"Recording..."}else{"Record congestion"}}</button></footer>
        </form>
    }.into_any()
}

fn resource_form(signals: Signals, drafts: Drafts) -> AnyView {
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let request = (|| {
            let facility_id = signals
                .facility_id
                .get_untracked()
                .ok_or_else(|| "Select one facility before recording a signal.".to_owned())?;
            Ok::<_, String>(RecordResourceCapacitySignalRequest {
                facility_id,
                resource_kind: drafts.resource_kind.get_untracked(),
                available_units: parsed(drafts.available_units, "available units")?,
                demand_units: parsed(drafts.demand_units, "demand units")?,
                ttl_seconds: parsed_positive(drafts.resource_ttl, "signal TTL")?,
            })
        })();
        match request {
            Ok(request) => dispatch(
                signals,
                PendingCommand::Resource(request, api::new_idempotency_key()),
            ),
            Err(message) => signals.command_error.set(Some(message)),
        }
    };
    view! {
        <form on:submit=submit>
            <p class="orchestration-form-intro">"Capacity pressure reduces the ranking of work that depends on the constrained resource; it never blocks manual execution."</p>
            <div class="orchestration-form-grid">
                <label class="wide"><span>"Resource"</span><select prop:value=move || resource_wire(drafts.resource_kind.get()) on:change=move |event| drafts.resource_kind.set(parse_resource(&event_target_value(&event)))>{all_resources().into_iter().map(|kind| view!{<option value=resource_wire(kind)>{super::display::resource_label(kind)}</option>}).collect_view()}</select></label>
                {number_input("Available units", drafts.available_units)}
                {number_input("Demand units", drafts.demand_units)}
                {number_input("TTL (seconds)", drafts.resource_ttl)}
            </div>
            {command_feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>{move || if signals.command_pending.get(){"Recording..."}else{"Record capacity"}}</button></footer>
        </form>
    }.into_any()
}

fn generate_form(
    signals: Signals,
    drafts: Drafts,
    locations: StoredValue<Vec<Location>>,
    policy: WorkOrchestrationPolicyResponse,
) -> AnyView {
    let facility_id = policy.facility_id;
    let policy_revision = policy.revision;
    let requested_owner_id = drafts.owner_id.get_untracked();
    let location_options = locations.with_value(|values| {
        values
            .iter()
            .filter(|location| location.facility_id == facility_id && location.active)
            .cloned()
            .collect::<Vec<_>>()
    });
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let request = (|| {
            let current_location_id = drafts
                .current_location_id
                .get_untracked()
                .ok_or_else(|| "Select the worker's current location.".to_owned())?;
            Ok::<_, String>(GenerateWorkOrchestrationPlanRequest {
                facility_id,
                inventory_owner_id: drafts.owner_id.get_untracked(),
                current_location_id,
                previous_work_kind: drafts.previous_work_kind.get_untracked(),
                generated_for_user_id: drafts.generated_for_user_id.get_untracked(),
                expected_policy_id: policy.policy_id,
                expected_policy_revision: policy_revision,
            })
        })();
        match request {
            Ok(request) => dispatch(
                signals,
                PendingCommand::Generate(request, api::new_idempotency_key()),
            ),
            Err(message) => signals.command_error.set(Some(message)),
        }
    };
    let worker_page = signals.workers.get();
    let next_worker_cursor = worker_page.next_cursor.clone();
    view! {
        <form on:submit=submit>
            <div class="orchestration-plan-safety"><strong>"Advisory only"</strong><span>"Generating a plan freezes evidence but does not claim tasks, change task status, or post inventory."</span></div>
            <div class="orchestration-form-grid">
                <label class="wide"><span>"Current location"</span><select required prop:value=move || super::display::option_id(drafts.current_location_id.get()) on:change=move |event| drafts.current_location_id.set(super::display::parse_id(&event_target_value(&event)))><option value="">"Select current location"</option>{location_options.into_iter().map(|location| { let label=location_label(&location); view!{<option value=location.id>{label}</option>} }).collect_view()}</select></label>
                <label><span>"Previous work kind"</span><select prop:value=move || work_kind_wire(drafts.previous_work_kind.get()) on:change=move |event| drafts.previous_work_kind.set(parse_work_kind(&event_target_value(&event)))><option value="">"None / new sequence"</option>{all_work_kinds().into_iter().map(|kind| view!{<option value=work_kind_value(kind)>{super::display::work_kind_label(kind)}</option>}).collect_view()}</select></label>
                <label><span>"Eligible worker (optional)"</span><select prop:value=move || super::display::option_id(drafts.generated_for_user_id.get()) on:change=move |event| drafts.generated_for_user_id.set(super::display::parse_id(&event_target_value(&event)))><option value="">"Unassigned advisory"</option>{worker_page.items.into_iter().map(|worker|view!{<option value=worker.user_id>{format!("{} · {}",worker.display_name,worker.title)}</option>}).collect_view()}</select></label>
            </div>
            <Show when=move || signals.workers_loading.get() && !signals.workers_loaded.get()><div class="orchestration-worker-state"><span class="loading-line"></span><span>"Loading eligible scoped workers"</span></div></Show>
            <Show when=move || signals.workers_loaded.get() && signals.workers.get().items.is_empty()><div class="orchestration-worker-state"><span>"No eligible worker is assigned to this facility and owner scope. The plan can remain unassigned."</span></div></Show>
            {next_worker_cursor.map(|cursor|view!{<button class="button secondary-action compact" type="button" disabled=move || signals.workers_loading.get() on:click=move |_| super::load_workers(signals,facility_id,requested_owner_id,Some(cursor.clone()),true)>"Load more eligible workers"</button>})}
            <div class="orchestration-policy-reference"><span>{format!("Policy #{}", policy.policy_id)}</span><strong>{format!("Revision {} · {}", policy.revision.get(), super::display::policy_mode_label(policy.mode))}</strong></div>
            {command_feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>{move || if signals.command_pending.get(){"Generating..."}else{"Generate plan"}}</button></footer>
        </form>
    }.into_any()
}

fn activate_form(signals: Signals, plan: WorkOrchestrationPlanResponse) -> AnyView {
    let plan_id = plan.plan_id;
    let worker_user_id = plan.generated_for_user_id;
    let item_count = plan.item_count;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        dispatch(
            signals,
            PendingCommand::Activate {
                plan_id,
                request: ActivateWorkOrchestrationDispatchRequest::default(),
                key: api::new_idempotency_key(),
            },
        );
    };
    view! {
        <form on:submit=submit>
            <div class="orchestration-plan-safety executable">
                <strong>"This reserves executable work"</strong>
                <span>{format!("The first of {item_count} tasks will be assigned to worker #{}. Later tasks remain reserved in this exact sequence and advance only through their owning workflows.",worker_user_id.unwrap_or_default())}</span>
            </div>
            {command_feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Keep advisory"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>{move || if signals.command_pending.get(){"Activating..."}else{"Activate dispatch"}}</button></footer>
        </form>
    }.into_any()
}

fn cancel_form(
    signals: Signals,
    drafts: Drafts,
    active: WorkOrchestrationDispatchResponse,
) -> AnyView {
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let reason = drafts.cancellation_reason.get_untracked();
        let note = drafts.cancellation_note.get_untracked();
        if reason == WorkOrchestrationDispatchCancellationReason::Other && note.trim().is_empty() {
            signals.command_error.set(Some(
                "Enter a note for the other cancellation reason.".into(),
            ));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Cancel {
                dispatch_id: active.dispatch_id,
                request: CancelWorkOrchestrationDispatchRequest {
                    expected_revision: active.revision,
                    reason,
                    note: (!note.trim().is_empty()).then(|| note.trim().to_owned()),
                },
                key: api::new_idempotency_key(),
            },
        );
    };
    view! {
        <form on:submit=submit>
            <p class="orchestration-form-intro">"Cancellation releases assigned-but-unstarted work and all remaining reservations. In-progress work must be released through its typed workflow first."</p>
            <div class="orchestration-form-grid">
                <label><span>"Reason"</span><select prop:value=move || cancellation_reason_wire(drafts.cancellation_reason.get()) on:change=move |event| drafts.cancellation_reason.set(parse_cancellation_reason(&event_target_value(&event)))><option value="operator_cancelled">"Operator cancelled"</option><option value="worker_unavailable">"Worker unavailable"</option><option value="scope_changed">"Scope changed"</option><option value="plan_invalidated">"Plan invalidated"</option><option value="other">"Other"</option></select></label>
                <label class="wide"><span>"Note"</span><textarea maxlength="500" prop:value=move || drafts.cancellation_note.get() on:input=move |event| drafts.cancellation_note.set(event_target_value(&event))></textarea></label>
            </div>
            {command_feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Keep active"</button><button class="button danger-action" type="submit" disabled=move || signals.command_pending.get()>{move || if signals.command_pending.get(){"Cancelling..."}else{"Cancel dispatch"}}</button></footer>
        </form>
    }.into_any()
}

fn cancellation_reason_wire(value: WorkOrchestrationDispatchCancellationReason) -> &'static str {
    match value {
        WorkOrchestrationDispatchCancellationReason::OperatorCancelled => "operator_cancelled",
        WorkOrchestrationDispatchCancellationReason::WorkerUnavailable => "worker_unavailable",
        WorkOrchestrationDispatchCancellationReason::ScopeChanged => "scope_changed",
        WorkOrchestrationDispatchCancellationReason::PlanInvalidated => "plan_invalidated",
        WorkOrchestrationDispatchCancellationReason::Other => "other",
    }
}

fn parse_cancellation_reason(value: &str) -> WorkOrchestrationDispatchCancellationReason {
    match value {
        "worker_unavailable" => WorkOrchestrationDispatchCancellationReason::WorkerUnavailable,
        "scope_changed" => WorkOrchestrationDispatchCancellationReason::ScopeChanged,
        "plan_invalidated" => WorkOrchestrationDispatchCancellationReason::PlanInvalidated,
        "other" => WorkOrchestrationDispatchCancellationReason::Other,
        _ => WorkOrchestrationDispatchCancellationReason::OperatorCancelled,
    }
}

fn command_feedback(signals: Signals) -> AnyView {
    let retry = move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    };
    view! {
        <Show when=move || signals.command_error.get().is_some()>
            <div class="orchestration-form-error" role="alert">
                <span>{move || signals.command_error.get().unwrap_or_default()}</span>
                <Show when=move || signals.retry.get().is_some()>
                    <button class="button secondary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=retry>"Retry exact command"</button>
                </Show>
            </div>
        </Show>
    }.into_any()
}

fn clear_command_state(signals: Signals) {
    signals.command_error.set(None);
    signals.retry.set(None);
}

fn policy_owner_draft(
    current: Option<&WorkOrchestrationPolicyResponse>,
    filtered_owner_id: Option<i64>,
) -> Option<i64> {
    current.map_or(filtered_owner_id, |policy| policy.inventory_owner_id)
}

fn zone_options(locations: &[Location], facility_id: Option<i64>) -> Vec<(i64, String)> {
    let mut zones = BTreeMap::new();
    for location in locations
        .iter()
        .filter(|location| facility_id.is_none_or(|id| location.facility_id == id))
    {
        if let Some(id) = location.storage_zone_id {
            let code = location
                .storage_zone_code
                .clone()
                .unwrap_or_else(|| format!("Zone #{id}"));
            let name = location.storage_zone_name.clone().unwrap_or_default();
            let label = if name.is_empty() {
                code
            } else {
                format!("{code} · {name}")
            };
            zones.entry(id).or_insert(label);
        }
    }
    zones.into_iter().collect()
}

fn location_label(location: &Location) -> String {
    let identity = location
        .barcode
        .clone()
        .or_else(|| location.name.clone())
        .unwrap_or_else(|| format!("Location #{}", location.id));
    location
        .storage_zone_code
        .as_ref()
        .map_or(identity.clone(), |zone| format!("{identity} · {zone}"))
}

fn number_input(label: &'static str, signal: RwSignal<String>) -> AnyView {
    view! { <label><span>{label}</span><input type="number" min="0" required prop:value=move || signal.get() on:input=move |event| signal.set(event_target_value(&event))/></label> }.into_any()
}

fn parsed<T: std::str::FromStr>(signal: RwSignal<String>, label: &str) -> Result<T, String> {
    signal
        .get_untracked()
        .trim()
        .parse::<T>()
        .map_err(|_| format!("Enter a valid {label}."))
}

fn parsed_positive<T>(signal: RwSignal<String>, label: &str) -> Result<T, String>
where
    T: std::str::FromStr + Default + PartialOrd,
{
    let value = parsed::<T>(signal, label)?;
    if value <= T::default() {
        Err(format!("{label} must be greater than zero."))
    } else {
        Ok(value)
    }
}

const fn policy_mode_wire(value: WorkOrchestrationMode) -> &'static str {
    match value {
        WorkOrchestrationMode::Enabled => "enabled",
        WorkOrchestrationMode::Disabled => "disabled",
    }
}

fn parse_policy_mode(value: &str) -> WorkOrchestrationMode {
    if value == "disabled" {
        WorkOrchestrationMode::Disabled
    } else {
        WorkOrchestrationMode::Enabled
    }
}

const fn resource_wire(value: WorkResourceKind) -> &'static str {
    match value {
        WorkResourceKind::GeneralLabor => "general_labor",
        WorkResourceKind::InventoryControl => "inventory_control",
        WorkResourceKind::MaterialHandling => "material_handling",
        WorkResourceKind::DockDoor => "dock_door",
        WorkResourceKind::PackStation => "pack_station",
        WorkResourceKind::Automation => "automation",
    }
}

fn parse_resource(value: &str) -> WorkResourceKind {
    match value {
        "inventory_control" => WorkResourceKind::InventoryControl,
        "material_handling" => WorkResourceKind::MaterialHandling,
        "dock_door" => WorkResourceKind::DockDoor,
        "pack_station" => WorkResourceKind::PackStation,
        "automation" => WorkResourceKind::Automation,
        _ => WorkResourceKind::GeneralLabor,
    }
}

const fn all_resources() -> [WorkResourceKind; 6] {
    [
        WorkResourceKind::GeneralLabor,
        WorkResourceKind::InventoryControl,
        WorkResourceKind::MaterialHandling,
        WorkResourceKind::DockDoor,
        WorkResourceKind::PackStation,
        WorkResourceKind::Automation,
    ]
}

fn work_kind_wire(value: Option<OrchestrationWorkKind>) -> &'static str {
    value.map_or("", work_kind_value)
}

const fn work_kind_value(value: OrchestrationWorkKind) -> &'static str {
    match value {
        OrchestrationWorkKind::CycleCountItemLocation => "cycle_count_item_location",
        OrchestrationWorkKind::CycleCountLocation => "cycle_count_location",
        OrchestrationWorkKind::Putaway => "putaway",
        OrchestrationWorkKind::LicensePlatePutaway => "license_plate_putaway",
        OrchestrationWorkKind::InventoryRelocation => "inventory_relocation",
        OrchestrationWorkKind::Replenishment => "replenishment",
        OrchestrationWorkKind::CrossDock => "cross_dock",
    }
}

fn parse_work_kind(value: &str) -> Option<OrchestrationWorkKind> {
    match value {
        "cycle_count_item_location" => Some(OrchestrationWorkKind::CycleCountItemLocation),
        "cycle_count_location" => Some(OrchestrationWorkKind::CycleCountLocation),
        "putaway" => Some(OrchestrationWorkKind::Putaway),
        "license_plate_putaway" => Some(OrchestrationWorkKind::LicensePlatePutaway),
        "inventory_relocation" => Some(OrchestrationWorkKind::InventoryRelocation),
        "replenishment" => Some(OrchestrationWorkKind::Replenishment),
        "cross_dock" => Some(OrchestrationWorkKind::CrossDock),
        _ => None,
    }
}

const fn all_work_kinds() -> [OrchestrationWorkKind; 7] {
    [
        OrchestrationWorkKind::CycleCountItemLocation,
        OrchestrationWorkKind::CycleCountLocation,
        OrchestrationWorkKind::Putaway,
        OrchestrationWorkKind::LicensePlatePutaway,
        OrchestrationWorkKind::InventoryRelocation,
        OrchestrationWorkKind::Replenishment,
        OrchestrationWorkKind::CrossDock,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_choices_are_facility_scoped_and_deduplicated() {
        let location = |id, facility_id, zone_id, zone_code: &str| {
            serde_json::from_value::<Location>(serde_json::json!({
                "id": id,
                "tenant_id": 1,
                "created": "2026-01-01T00:00:00Z",
                "deleted": null,
                "facility_id": facility_id,
                "facility_name": null,
                "parent_location_id": null,
                "barcode": format!("L{id}"),
                "name": null,
                "type": "bin",
                "active": true,
                "pickable": true,
                "receivable": false,
                "storage_zone_id": zone_id,
                "storage_zone_code": zone_code,
                "storage_zone_name": "Fast pick",
                "storage_zone_purpose": "pick",
                "storage_zone_travel_sequence": 1
            }))
            .unwrap()
        };
        let choices = zone_options(
            &[
                location(1, 8, 20, "PICK-A"),
                location(2, 8, 20, "PICK-A"),
                location(3, 9, 21, "PICK-B"),
            ],
            Some(8),
        );
        assert_eq!(choices, vec![(20, "PICK-A · Fast pick".into())]);
    }

    #[test]
    fn wire_values_cover_explicit_fallback_and_all_work_kinds() {
        assert_eq!(
            policy_mode_wire(WorkOrchestrationMode::Disabled),
            "disabled"
        );
        for kind in all_work_kinds() {
            assert_eq!(parse_work_kind(work_kind_value(kind)), Some(kind));
        }
        for reason in [
            WorkOrchestrationDispatchCancellationReason::OperatorCancelled,
            WorkOrchestrationDispatchCancellationReason::WorkerUnavailable,
            WorkOrchestrationDispatchCancellationReason::ScopeChanged,
            WorkOrchestrationDispatchCancellationReason::PlanInvalidated,
            WorkOrchestrationDispatchCancellationReason::Other,
        ] {
            assert_eq!(
                parse_cancellation_reason(cancellation_reason_wire(reason)),
                reason
            );
        }
    }

    #[test]
    fn superseding_facility_default_preserves_exact_ownerless_scope() {
        let policy: WorkOrchestrationPolicyResponse = serde_json::from_value(serde_json::json!({
            "policy_id": 7,
            "facility_id": 8,
            "inventory_owner_id": null,
            "mode": "enabled",
            "priority_weight": 100,
            "due_urgency_weight": 80,
            "proximity_weight": 60,
            "interleaving_weight": 30,
            "congestion_penalty_weight": 50,
            "bottleneck_penalty_weight": 50,
            "due_horizon_minutes": 120,
            "max_candidates": 100,
            "revision": 2,
            "configured_by": 4,
            "configured_at": "2026-01-01T00:00:00Z",
            "effective_from": "2026-01-01T00:00:00Z",
            "supersedes_policy_id": null,
            "effective_to": null
        }))
        .unwrap();
        assert_eq!(policy_owner_draft(Some(&policy), Some(99)), None);
        assert_eq!(policy_owner_draft(None, Some(99)), Some(99));
    }
}
