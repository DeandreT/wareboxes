mod model;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AutomationCommandResponse, AutomationCommandStatus, AutomationControlMode,
    AutomationDeviceClass, AutomationDeviceResponse, AutomationHealthState,
    AutomationManualResolution, AutomationRecoveryPolicy, AutomationWorkspaceResponse,
    ChangeAutomationControlRequest, EnqueueAutomationCommandRequest,
    RegisterAutomationDeviceRequest, ResolveAutomationCommandRequest,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::toast::use_toast_bus;

use model::{class_label, operations, CommandDraft};

#[derive(Clone)]
enum SavedAttempt {
    Register {
        request: RegisterAutomationDeviceRequest,
        key: String,
    },
    Control {
        device_id: i64,
        request: ChangeAutomationControlRequest,
        key: String,
    },
    Command {
        device_id: i64,
        request: EnqueueAutomationCommandRequest,
        key: String,
    },
    Resolution {
        command_id: i64,
        request: ResolveAutomationCommandRequest,
        key: String,
    },
}

#[component]
pub(crate) fn AutomationWorkspace(
    initial_workspace: AutomationWorkspaceResponse,
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let workspace = RwSignal::new(initial_workspace);
    let facilities = StoredValue::new(access.facilities);
    let facility_id = RwSignal::new(None::<i64>);
    let include_history = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let generation = RwSignal::new(0_u64);
    let error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);
    let retry = RwSignal::new(None::<SavedAttempt>);
    let register_open = RwSignal::new(false);
    let register_facility = RwSignal::new(None::<i64>);
    let register_key = RwSignal::new(String::new());
    let register_name = RwSignal::new(String::new());
    let register_class = RwSignal::new(AutomationDeviceClass::Conveyor);
    let control_device = RwSignal::new(None::<AutomationDeviceResponse>);
    let control_target = RwSignal::new(AutomationControlMode::ManualFallback);
    let control_reason = RwSignal::new(String::new());
    let command_device = RwSignal::new(None::<AutomationDeviceResponse>);
    let correlation_id = RwSignal::new(String::new());
    let recovery_policy = RwSignal::new(AutomationRecoveryPolicy::ManualReview);
    let command_draft = RwSignal::new(CommandDraft::default());
    let resolution_command = RwSignal::new(None::<AutomationCommandResponse>);
    let resolution_outcome = RwSignal::new(AutomationManualResolution::ConfirmedNotExecuted);
    let resolution_reason = RwSignal::new(String::new());
    let toasts = use_toast_bus();

    let reload = Callback::new(move |_| {
        let next = generation.get_untracked().saturating_add(1);
        generation.set(next);
        loading.set(true);
        error.set(None);
        let facility = facility_id.get_untracked();
        let history = include_history.get_untracked();
        leptos::task::spawn_local(async move {
            match api::automation_workspace(facility, history).await {
                Ok(value) if generation.get_untracked() == next => {
                    workspace.set(value);
                    loading.set(false);
                }
                Ok(_) => {}
                Err(value) if value.unauthorized => on_unauthorized.run(()),
                Err(value) if generation.get_untracked() == next => {
                    error.set(Some(value.message));
                    loading.set(false);
                }
                Err(_) => {}
            }
        });
    });

    let dispatch = Callback::new(move |attempt: SavedAttempt| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        retry.set(Some(attempt.clone()));
        error.set(None);
        leptos::task::spawn_local(async move {
            let outcome = match &attempt {
                SavedAttempt::Register { request, key } => {
                    api::register_automation_device(request, key)
                        .await
                        .map(|device| (Some(device), None))
                }
                SavedAttempt::Control {
                    device_id,
                    request,
                    key,
                } => api::change_automation_control(*device_id, request, key)
                    .await
                    .map(|device| (Some(device), None)),
                SavedAttempt::Command {
                    device_id,
                    request,
                    key,
                } => api::enqueue_automation_command(*device_id, request, key)
                    .await
                    .map(|command| (None, Some(command))),
                SavedAttempt::Resolution {
                    command_id,
                    request,
                    key,
                } => api::resolve_automation_command(*command_id, request, key)
                    .await
                    .map(|command| (None, Some(command))),
            };
            match outcome {
                Ok((device, command)) => {
                    if let Some(device) = device {
                        upsert_device(workspace, device);
                    }
                    if let Some(command) = command {
                        upsert_command(workspace, command);
                    }
                    retry.set(None);
                    pending.set(false);
                    register_open.set(false);
                    control_device.set(None);
                    command_device.set(None);
                    resolution_command.set(None);
                    toasts.success("Automation change committed.");
                }
                Err(value) if value.unauthorized => on_unauthorized.run(()),
                Err(value) => {
                    if !value.ambiguous_outcome {
                        retry.set(None);
                        reload.run(());
                    }
                    error.set(Some(value.message.clone()));
                    pending.set(false);
                    toasts.error(value.message);
                }
            }
        });
    });

    let submit_register = move |_| {
        let Some(facility_id) = register_facility.get_untracked() else {
            error.set(Some("Select a facility for the device.".into()));
            return;
        };
        dispatch.run(SavedAttempt::Register {
            request: RegisterAutomationDeviceRequest {
                facility_id,
                device_key: register_key.get_untracked().trim().to_owned(),
                class: register_class.get_untracked(),
                display_name: register_name.get_untracked().trim().to_owned(),
            },
            key: api::new_idempotency_key(),
        });
    };
    let submit_control = move |_| {
        let Some(device) = control_device.get_untracked() else {
            return;
        };
        let target = control_target.get_untracked();
        dispatch.run(SavedAttempt::Control {
            device_id: device.device_id,
            request: ChangeAutomationControlRequest {
                expected_revision: device.revision,
                target_mode: target,
                reason: control_reason.get_untracked().trim().to_owned(),
                safety_confirmation: (target == AutomationControlMode::Automatic)
                    .then(|| "CONFIRM-SAFE-TO-RESUME".to_owned()),
            },
            key: api::new_idempotency_key(),
        });
    };
    let submit_command = move |_| {
        let Some(device) = command_device.get_untracked() else {
            return;
        };
        let command = match command_draft.get_untracked().build(device.class) {
            Ok(command) => command,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        dispatch.run(SavedAttempt::Command {
            device_id: device.device_id,
            request: EnqueueAutomationCommandRequest {
                correlation_id: correlation_id.get_untracked().trim().to_owned(),
                recovery_policy: recovery_policy.get_untracked(),
                command,
            },
            key: api::new_idempotency_key(),
        });
    };
    let submit_resolution = move |_| {
        let Some(command) = resolution_command.get_untracked() else {
            return;
        };
        dispatch.run(SavedAttempt::Resolution {
            command_id: command.command_id,
            request: ResolveAutomationCommandRequest {
                expected_revision: command.revision,
                outcome: resolution_outcome.get_untracked(),
                reason: resolution_reason.get_untracked().trim().to_owned(),
            },
            key: api::new_idempotency_key(),
        });
    };

    let open_register = move |_| {
        register_facility.set(facility_id.get_untracked());
        register_key.set(String::new());
        register_name.set(String::new());
        register_class.set(AutomationDeviceClass::Conveyor);
        error.set(None);
        register_open.set(true);
    };
    let open_control = Callback::new(move |(device, target)| {
        control_reason.set(String::new());
        control_target.set(target);
        control_device.set(Some(device));
        error.set(None);
    });
    let open_command = Callback::new(move |device: AutomationDeviceResponse| {
        let mut draft = CommandDraft::default();
        draft.reset_for(device.class);
        command_draft.set(draft);
        correlation_id.set(String::new());
        recovery_policy.set(AutomationRecoveryPolicy::ManualReview);
        command_device.set(Some(device));
        error.set(None);
    });
    let open_resolution = Callback::new(move |command: AutomationCommandResponse| {
        resolution_outcome.set(AutomationManualResolution::ConfirmedNotExecuted);
        resolution_reason.set(String::new());
        resolution_command.set(Some(command));
        error.set(None);
    });

    view! {
        <main class="automation-workspace">
            <header class="page-heading"><div><span class="eyebrow">"Automation control plane"</span><h1>"Devices & edge commands"</h1><p>"Monitor local equipment, issue typed commands, and reconcile durable edge outcomes."</p></div><div class="page-actions"><button type="button" class="button secondary-action" disabled=move || loading.get() on:click=move |_| reload.run(())>{move || if loading.get(){"Refreshing"}else{"Refresh"}}</button><button type="button" class="button primary-action" on:click=open_register>"Register device"</button></div></header>
            <section class="filter-bar"><label><span>"Facility"</span><select prop:value=move || optional_id(facility_id.get()) on:change=move |event| { facility_id.set(parse_id(&event_target_value(&event))); reload.run(()); }><option value="">"All authorized facilities"</option>{facilities.with_value(|items| items.iter().map(|item| view!{<option value=item.id>{item.name.clone()}</option>}).collect_view())}</select></label><label class="check-control"><input type="checkbox" prop:checked=move || include_history.get() on:change=move |event| { include_history.set(event_target_checked(&event)); reload.run(()); }/><span>"Include terminal history"</span></label></section>
            {move || metrics(&workspace.get())}
            <Show when=move || error.get().is_some()><p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p></Show>
            <Show when=move || retry.get().is_some()><div class="automation-retry" role="status"><span>"The previous command outcome is unknown. Retry the exact request."</span><button type="button" class="button secondary-action" disabled=move || pending.get() on:click=move |_| { if let Some(attempt)=retry.get_untracked(){dispatch.run(attempt);} }>"Retry exact command"</button></div></Show>
            <section class="workspace-panel"><header><div><h2>"Device fleet"</h2><p>"Cloud control state beside the latest authenticated edge health."</p></div></header><div class="table-scroll"><table class="data-table"><caption class="sr-only">"Automation devices currently loaded"</caption><thead><tr><th>"Device"</th><th>"Facility"</th><th>"Control"</th><th>"Health"</th><th>"Last heartbeat"</th><th>"Actions"</th></tr></thead><tbody><Show when=move || workspace.with(|value|value.devices.is_empty())><tr><td class="table-empty-row" colspan="6">"No automation devices are registered in this scope."</td></tr></Show>{move || workspace.get().devices.into_iter().map(|device| { let enable=device.clone(); let fallback=device.clone(); let disable=device.clone(); let issue=device.clone(); view!{<tr><td><strong>{device.display_name.clone()}</strong><small class="cell-detail">{format!("{} · {} · rev {}",device.device_key,class_label(device.class),device.revision.get())}</small></td><td>{facility_name(&facilities,device.facility_id)}</td><td><span class=status_class_control(device.control_mode)>{control_label(device.control_mode)}</span><small class="cell-detail">{device.control_reason.clone()}</small></td><td><span class=status_class_health(device.health)>{health_label(device.health)}</span>{device.health_message.clone().map(|message|view!{<small class="cell-detail">{message}</small>})}</td><td>{device.last_heartbeat_at.clone().unwrap_or_else(||"Never".into())}</td><td><div class="table-actions"><button type="button" class="text-action" disabled=device.control_mode==AutomationControlMode::Automatic on:click=move |_|open_control.run((enable.clone(),AutomationControlMode::Automatic))>"Enable"</button><button type="button" class="text-action" disabled=device.control_mode==AutomationControlMode::ManualFallback on:click=move |_|open_control.run((fallback.clone(),AutomationControlMode::ManualFallback))>"Fallback"</button><button type="button" class="text-action" disabled=device.control_mode==AutomationControlMode::Disabled on:click=move |_|open_control.run((disable.clone(),AutomationControlMode::Disabled))>"Disable"</button><button type="button" class="text-action" disabled=device.control_mode!=AutomationControlMode::Automatic on:click=move |_|open_command.run(issue.clone())>"Command"</button></div></td></tr>}}).collect_view()}</tbody></table></div></section>
            <section class="workspace-panel"><header><div><h2>"Command ledger"</h2><p>"Delivery attempts, durable acceptance, terminal result, and attributed manual reconciliation."</p></div></header><div class="table-scroll"><table class="data-table"><caption class="sr-only">"Automation commands currently loaded"</caption><thead><tr><th>"Command / correlation"</th><th>"Device"</th><th>"Recovery"</th><th>"Status"</th><th>"Delivery"</th><th>"Requested / completed"</th><th>"Actions"</th></tr></thead><tbody><Show when=move || workspace.with(|value|value.commands.is_empty())><tr><td class="table-empty-row" colspan="7">"No automation commands are loaded for this view."</td></tr></Show>{move || workspace.get().commands.into_iter().map(|command| { let resolve=command.clone(); view!{<tr><td><strong>{format!("Command #{}",command.command_id)}</strong><small class="cell-detail">{command.correlation_id}</small></td><td>{command.device_key}<small class="cell-detail">{class_label(command.device_class)}</small></td><td>{recovery_label(command.recovery_policy)}</td><td><span class=status_class_command(command.status)>{command_label(command.status)}</span>{command.error_message.map(|message|view!{<small class="cell-detail danger-text">{message}</small>})}{command.resolution_reason.map(|reason|view!{<small class="cell-detail">{format!("Resolved: {reason}")}</small>})}</td><td>{format!("{} attempt(s)",command.delivery_attempts)}<small class="cell-detail">{command.agent_instance.unwrap_or_else(||"Not delivered".into())}</small></td><td>{command.requested_at}<small class="cell-detail">{command.resolved_at.or(command.completed_at).unwrap_or_else(||"In progress".into())}</small></td><td><button type="button" class="text-action" disabled=command.status!=AutomationCommandStatus::ManualReview on:click=move |_|open_resolution.run(resolve.clone())>"Resolve"</button></td></tr>}}).collect_view()}</tbody></table></div></section>
            <section class="workspace-panel"><header><div><h2>"Edge heartbeats"</h2><p>"Authenticated local state, queue pressure, and manual-review load."</p></div></header><div class="table-scroll"><table class="data-table"><caption class="sr-only">"Latest edge-agent heartbeats currently loaded"</caption><thead><tr><th>"Device"</th><th>"Agent"</th><th>"Health / local control"</th><th>"Queue"</th><th>"Observed / received"</th></tr></thead><tbody><Show when=move || workspace.with(|value|value.heartbeats.is_empty())><tr><td class="table-empty-row" colspan="5">"No edge-agent heartbeats are loaded for this view."</td></tr></Show>{move || workspace.get().heartbeats.into_iter().map(|heartbeat| view!{<tr><td>{format!("Device #{}",heartbeat.device_id)}</td><td><strong>{heartbeat.agent_instance}</strong><small class="cell-detail">{format!("Service account #{}",heartbeat.service_account_id)}</small></td><td>{format!("{} · {}",health_label(heartbeat.health),control_label(heartbeat.control_mode))}{heartbeat.message.map(|message|view!{<small class="cell-detail">{message}</small>})}</td><td>{format!("{} queued · {} review",heartbeat.queued_commands,heartbeat.manual_review_commands)}</td><td>{heartbeat.observed_at}<small class="cell-detail">{heartbeat.received_at}</small></td></tr>}).collect_view()}</tbody></table></div></section>
            <Show when=move || register_open.get()>{register_dialog(RegisterDialogState { facility: register_facility, key: register_key, name: register_name, class: register_class, facilities, pending },submit_register,move |_|register_open.set(false))}</Show>
            <Show when=move || control_device.get().is_some()>{move || control_device.get().map(|device|control_dialog(device,control_target,control_reason,pending,submit_control,move |_|control_device.set(None)))}</Show>
            <Show when=move || command_device.get().is_some()>{move || command_device.get().map(|device|command_dialog(device,correlation_id,recovery_policy,command_draft,pending,submit_command,move |_|command_device.set(None)))}</Show>
            <Show when=move || resolution_command.get().is_some()>{move || resolution_command.get().map(|command|resolution_dialog(command,resolution_outcome,resolution_reason,pending,submit_resolution,move |_|resolution_command.set(None)))}</Show>
        </main>
    }
}

fn metrics(value: &AutomationWorkspaceResponse) -> AnyView {
    let automatic = value
        .devices
        .iter()
        .filter(|item| item.control_mode == AutomationControlMode::Automatic)
        .count();
    let healthy = value
        .devices
        .iter()
        .filter(|item| {
            matches!(
                item.health,
                AutomationHealthState::Healthy | AutomationHealthState::Degraded
            )
        })
        .count();
    let active = value
        .commands
        .iter()
        .filter(|item| {
            !matches!(
                item.status,
                AutomationCommandStatus::Succeeded
                    | AutomationCommandStatus::Failed
                    | AutomationCommandStatus::ResolvedManually
                    | AutomationCommandStatus::Cancelled
            )
        })
        .count();
    let review = value
        .commands
        .iter()
        .filter(|item| item.status == AutomationCommandStatus::ManualReview)
        .count();
    view!{<section class="metric-strip"><article><span>"Devices loaded"</span><strong>{value.devices.len()}</strong></article><article><span>"Automatic loaded"</span><strong>{automatic}</strong></article><article><span>"Healthy / degraded loaded"</span><strong>{healthy}</strong></article><article><span>"Active commands loaded"</span><strong>{active}</strong></article><article><span>"Manual review loaded"</span><strong>{review}</strong></article></section>}.into_any()
}

#[derive(Clone, Copy)]
struct RegisterDialogState {
    facility: RwSignal<Option<i64>>,
    key: RwSignal<String>,
    name: RwSignal<String>,
    class: RwSignal<AutomationDeviceClass>,
    facilities: StoredValue<Vec<wareboxes_api_contract::web::access::AccessScopeResource>>,
    pending: RwSignal<bool>,
}

fn register_dialog(
    state: RegisterDialogState,
    submit: impl Fn(()) + Copy + 'static,
    close: impl Fn(()) + Copy + 'static,
) -> AnyView {
    let RegisterDialogState {
        facility,
        key,
        name,
        class,
        facilities,
        pending,
    } = state;
    view!{<div class="automation-dialog-backdrop"><section class="automation-dialog" role="dialog" aria-modal="true" aria-labelledby="automation-register-title"><header><div><span class="eyebrow">"Device registry"</span><h2 id="automation-register-title">"Register automation device"</h2></div></header><div class="automation-form-grid"><label><span>"Facility"</span><select prop:value=move ||optional_id(facility.get()) on:change=move |event|facility.set(parse_id(&event_target_value(&event)))><option value="">"Select facility"</option>{facilities.with_value(|items|items.iter().map(|item|view!{<option value=item.id>{item.name.clone()}</option>}).collect_view())}</select></label><label><span>"Class"</span><select prop:value=move ||class_wire(class.get()) on:change=move |event|class.set(parse_class(&event_target_value(&event)))>{all_classes().into_iter().map(|value|view!{<option value=class_wire(value)>{class_label(value)}</option>}).collect_view()}</select></label><label><span>"Stable device key"</span><input prop:value=move ||key.get() on:input=move |event|key.set(event_target_value(&event))/></label><label><span>"Display name"</span><input prop:value=move ||name.get() on:input=move |event|name.set(event_target_value(&event))/></label></div><footer><button type="button" class="button secondary-action" disabled=move ||pending.get() on:click=move |_|close(())>"Cancel"</button><button type="button" class="button primary-action" disabled=move ||pending.get() on:click=move |_|submit(())>"Register disabled"</button></footer></section></div>}.into_any()
}

fn control_dialog(
    device: AutomationDeviceResponse,
    target: RwSignal<AutomationControlMode>,
    reason: RwSignal<String>,
    pending: RwSignal<bool>,
    submit: impl Fn(()) + Copy + 'static,
    close: impl Fn(()) + Copy + 'static,
) -> AnyView {
    view!{<div class="automation-dialog-backdrop"><section class="automation-dialog" role="dialog" aria-modal="true" aria-labelledby="automation-control-title"><header><div><span class="eyebrow">"Safety control"</span><h2 id="automation-control-title">{format!("{} · {}",device.display_name,control_label(target.get_untracked()))}</h2></div></header>{(target.get_untracked()==AutomationControlMode::Automatic).then(||view!{<p class="automation-warning">"Enabling confirms the physical guarding checklist is complete, local queues are reconciled, and the latest edge heartbeat is healthy."</p>})}<label><span>"Attributed reason"</span><textarea prop:value=move ||reason.get() on:input=move |event|reason.set(event_target_value(&event))></textarea></label><footer><button type="button" class="button secondary-action" disabled=move ||pending.get() on:click=move |_|close(())>"Cancel"</button><button type="button" class="button primary-action" disabled=move ||pending.get() || reason.get().trim().is_empty() on:click=move |_|submit(())>"Confirm control change"</button></footer></section></div>}.into_any()
}

fn resolution_dialog(
    command: AutomationCommandResponse,
    outcome: RwSignal<AutomationManualResolution>,
    reason: RwSignal<String>,
    pending: RwSignal<bool>,
    submit: impl Fn(()) + Copy + 'static,
    close: impl Fn(()) + Copy + 'static,
) -> AnyView {
    view! {
        <div class="automation-dialog-backdrop">
            <section class="automation-dialog" role="dialog" aria-modal="true" aria-labelledby="automation-resolution-title">
                <header><div><span class="eyebrow">"Physical reconciliation"</span><h2 id="automation-resolution-title">{format!("Resolve command #{}",command.command_id)}</h2></div></header>
                <p class="automation-warning">"Use physical device evidence, scanner history, or a local controller audit before resolving an ambiguous execution."</p>
                <div class="automation-form-grid">
                    <label><span>"Verified outcome"</span><select prop:value=move ||manual_resolution_wire(outcome.get()) on:change=move |event|outcome.set(parse_manual_resolution(&event_target_value(&event)))><option value="confirmed_not_executed">"Confirmed not executed"</option><option value="confirmed_executed">"Confirmed executed"</option></select></label>
                    <label><span>"Attributed evidence / reason"</span><textarea prop:value=move ||reason.get() on:input=move |event|reason.set(event_target_value(&event))></textarea></label>
                </div>
                <footer><button type="button" class="button secondary-action" disabled=move ||pending.get() on:click=move |_|close(())>"Cancel"</button><button type="button" class="button primary-action" disabled=move ||pending.get() || reason.get().trim().is_empty() on:click=move |_|submit(())>"Record resolution"</button></footer>
            </section>
        </div>
    }.into_any()
}

fn command_dialog(
    device: AutomationDeviceResponse,
    correlation: RwSignal<String>,
    recovery: RwSignal<AutomationRecoveryPolicy>,
    draft: RwSignal<CommandDraft>,
    pending: RwSignal<bool>,
    submit: impl Fn(()) + Copy + 'static,
    close: impl Fn(()) + Copy + 'static,
) -> AnyView {
    let class = device.class;
    view!{<div class="automation-dialog-backdrop"><section class="automation-dialog wide" role="dialog" aria-modal="true" aria-labelledby="automation-command-title"><header><div><span class="eyebrow">"Typed edge command"</span><h2 id="automation-command-title">{format!("{} · {}",device.display_name,class_label(class))}</h2></div></header><div class="automation-form-grid"><label><span>"Correlation ID"</span><input prop:value=move ||correlation.get() on:input=move |event|correlation.set(event_target_value(&event))/></label><label><span>"Recovery policy"</span><select prop:value=move ||recovery_wire(recovery.get()) on:change=move |event|recovery.set(parse_recovery(&event_target_value(&event)))><option value="manual_review">"Manual review after ambiguity"</option><option value="probe_then_retry">"Probe, then retry"</option><option value="device_deduplicated_replay">"Device-deduplicated replay"</option></select></label><label><span>"Operation"</span><select prop:value=move ||draft.get().operation on:change=move |event|draft.update(|value|value.operation=event_target_value(&event))>{operations(class).iter().map(|(wire,label)|view!{<option value=*wire>{*label}</option>}).collect_view()}</select></label></div>{move ||command_fields(class,draft)}<footer><button type="button" class="button secondary-action" disabled=move ||pending.get() on:click=move |_|close(())>"Cancel"</button><button type="button" class="button primary-action" disabled=move ||pending.get() || correlation.get().trim().is_empty() on:click=move |_|submit(())>"Queue command"</button></footer></section></div>}.into_any()
}

fn command_fields(class: AutomationDeviceClass, draft: RwSignal<CommandDraft>) -> AnyView {
    let operation = draft.get().operation;
    let text = |label: &'static str, selector: u8| view! {<label><span>{label}</span><input prop:value=move ||field_value(draft.get(),selector) on:input=move |event|set_field(draft,selector,event_target_value(&event))/></label>};
    let number = |label: &'static str| view! {<label><span>{label}</span><input inputmode="numeric" prop:value=move ||draft.get().number on:input=move |event|draft.update(|value|value.number=event_target_value(&event))/></label>};
    let fields:Vec<AnyView>=match (class,operation.as_str()) {
        (AutomationDeviceClass::Plc,"set_output")=>vec![text("PLC point",1).into_any(),view!{<label class="check-control"><input type="checkbox" prop:checked=move ||draft.get().flag on:change=move |event|draft.update(|value|value.flag=event_target_checked(&event))/><span>"Output on"</span></label>}.into_any()],
        (AutomationDeviceClass::Plc,"pulse_output")=>vec![text("PLC point",1).into_any(),number("Duration (ms)").into_any()],
        (AutomationDeviceClass::Plc,"reset_fault")=>vec![text("Fault code",1).into_any()],
        (AutomationDeviceClass::Plc,_)=>Vec::new(),
        (AutomationDeviceClass::Conveyor,"route_carrier")=>vec![text("Carrier ID",1).into_any(),text("Destination",2).into_any()],
        (AutomationDeviceClass::Conveyor,_)=>vec![text("Zone",1).into_any()],
        (AutomationDeviceClass::Robotics,"dispatch_mission")=>vec![text("Mission ID",1).into_any(),choice(draft,"Mission kind",&[("pick","Pick"),("place","Place"),("transport","Transport"),("charge","Charge")]),text("Source",2).into_any(),text("Destination",3).into_any(),text("Payload ID (optional)",4).into_any()],
        (AutomationDeviceClass::Robotics,_)=>vec![text("Mission ID",1).into_any()],
        (AutomationDeviceClass::Sortation,"divert")=>vec![text("Tracking ID",1).into_any(),text("Chute",2).into_any()],
        (AutomationDeviceClass::Sortation,_)=>vec![text("Tracking ID",1).into_any(),text("Lane",2).into_any(),text("Reason code",3).into_any()],
        (AutomationDeviceClass::Printer,"print_document")=>vec![text("Document ID",1).into_any(),choice(draft,"Format",&[("zpl","ZPL"),("pdf","PDF base64"),("png","PNG base64"),("html","HTML")]),text("Content",2).into_any(),number("Copies").into_any()],
        (AutomationDeviceClass::Printer,_)=>vec![text("Spool job ID",1).into_any()],
        (AutomationDeviceClass::Scale,"read_weight")=>vec![choice(draft,"Unit",&[("gram","Gram"),("kilogram","Kilogram"),("pound","Pound")]),number("Timeout (ms)").into_any()],
        (AutomationDeviceClass::Scale,_)=>Vec::new(),
    };
    view! {<div class="automation-form-grid">{fields}</div>}.into_any()
}

fn choice(
    draft: RwSignal<CommandDraft>,
    label: &'static str,
    values: &'static [(&'static str, &'static str)],
) -> AnyView {
    view!{<label><span>{label}</span><select prop:value=move ||draft.get().choice on:change=move |event|draft.update(|value|value.choice=event_target_value(&event))>{values.iter().map(|(wire,label)|view!{<option value=*wire>{*label}</option>}).collect_view()}</select></label>}.into_any()
}
fn field_value(value: CommandDraft, selector: u8) -> String {
    match selector {
        1 => value.first,
        2 => value.second,
        3 => value.third,
        _ => value.fourth,
    }
}
fn set_field(signal: RwSignal<CommandDraft>, selector: u8, value: String) {
    signal.update(|draft| match selector {
        1 => draft.first = value,
        2 => draft.second = value,
        3 => draft.third = value,
        _ => draft.fourth = value,
    });
}
fn upsert_device(
    workspace: RwSignal<AutomationWorkspaceResponse>,
    device: AutomationDeviceResponse,
) {
    workspace.update(|value| {
        if let Some(existing) = value
            .devices
            .iter_mut()
            .find(|item| item.device_id == device.device_id)
        {
            *existing = device;
        } else {
            value.devices.push(device);
        }
    })
}
fn upsert_command(
    workspace: RwSignal<AutomationWorkspaceResponse>,
    command: AutomationCommandResponse,
) {
    workspace.update(|value| {
        if let Some(existing) = value
            .commands
            .iter_mut()
            .find(|item| item.command_id == command.command_id)
        {
            *existing = command;
        } else {
            value.commands.insert(0, command);
        }
    })
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|value| *value > 0)
}
fn optional_id(value: Option<i64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}
fn facility_name(
    facilities: &StoredValue<Vec<wareboxes_api_contract::web::access::AccessScopeResource>>,
    id: i64,
) -> String {
    facilities.with_value(|items| {
        items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.clone())
            .unwrap_or_else(|| format!("Facility #{id}"))
    })
}
fn all_classes() -> [AutomationDeviceClass; 6] {
    [
        AutomationDeviceClass::Plc,
        AutomationDeviceClass::Conveyor,
        AutomationDeviceClass::Robotics,
        AutomationDeviceClass::Sortation,
        AutomationDeviceClass::Printer,
        AutomationDeviceClass::Scale,
    ]
}
fn class_wire(value: AutomationDeviceClass) -> &'static str {
    match value {
        AutomationDeviceClass::Plc => "plc",
        AutomationDeviceClass::Conveyor => "conveyor",
        AutomationDeviceClass::Robotics => "robotics",
        AutomationDeviceClass::Sortation => "sortation",
        AutomationDeviceClass::Printer => "printer",
        AutomationDeviceClass::Scale => "scale",
    }
}
fn parse_class(value: &str) -> AutomationDeviceClass {
    match value {
        "plc" => AutomationDeviceClass::Plc,
        "robotics" => AutomationDeviceClass::Robotics,
        "sortation" => AutomationDeviceClass::Sortation,
        "printer" => AutomationDeviceClass::Printer,
        "scale" => AutomationDeviceClass::Scale,
        _ => AutomationDeviceClass::Conveyor,
    }
}
fn recovery_wire(value: AutomationRecoveryPolicy) -> &'static str {
    match value {
        AutomationRecoveryPolicy::DeviceDeduplicatedReplay => "device_deduplicated_replay",
        AutomationRecoveryPolicy::ProbeThenRetry => "probe_then_retry",
        AutomationRecoveryPolicy::ManualReview => "manual_review",
    }
}
fn parse_recovery(value: &str) -> AutomationRecoveryPolicy {
    match value {
        "device_deduplicated_replay" => AutomationRecoveryPolicy::DeviceDeduplicatedReplay,
        "probe_then_retry" => AutomationRecoveryPolicy::ProbeThenRetry,
        _ => AutomationRecoveryPolicy::ManualReview,
    }
}
fn manual_resolution_wire(value: AutomationManualResolution) -> &'static str {
    match value {
        AutomationManualResolution::ConfirmedExecuted => "confirmed_executed",
        AutomationManualResolution::ConfirmedNotExecuted => "confirmed_not_executed",
    }
}
fn parse_manual_resolution(value: &str) -> AutomationManualResolution {
    match value {
        "confirmed_executed" => AutomationManualResolution::ConfirmedExecuted,
        _ => AutomationManualResolution::ConfirmedNotExecuted,
    }
}
fn control_label(value: AutomationControlMode) -> &'static str {
    match value {
        AutomationControlMode::Disabled => "Disabled",
        AutomationControlMode::Automatic => "Automatic",
        AutomationControlMode::ManualFallback => "Manual fallback",
    }
}
fn health_label(value: AutomationHealthState) -> &'static str {
    match value {
        AutomationHealthState::Unknown => "Unknown",
        AutomationHealthState::Healthy => "Healthy",
        AutomationHealthState::Degraded => "Degraded",
        AutomationHealthState::Offline => "Offline",
        AutomationHealthState::Faulted => "Faulted",
    }
}
fn command_label(value: AutomationCommandStatus) -> &'static str {
    match value {
        AutomationCommandStatus::Queued => "Queued",
        AutomationCommandStatus::Delivered => "Delivered",
        AutomationCommandStatus::Accepted => "Accepted",
        AutomationCommandStatus::Succeeded => "Succeeded",
        AutomationCommandStatus::Failed => "Failed",
        AutomationCommandStatus::ManualReview => "Manual review",
        AutomationCommandStatus::ResolvedManually => "Resolved manually",
        AutomationCommandStatus::Cancelled => "Cancelled",
    }
}
fn recovery_label(value: AutomationRecoveryPolicy) -> &'static str {
    match value {
        AutomationRecoveryPolicy::DeviceDeduplicatedReplay => "Device dedup replay",
        AutomationRecoveryPolicy::ProbeThenRetry => "Probe then retry",
        AutomationRecoveryPolicy::ManualReview => "Manual review",
    }
}
fn status_class_control(value: AutomationControlMode) -> &'static str {
    match value {
        AutomationControlMode::Automatic => "status-pill success",
        AutomationControlMode::Disabled => "status-pill neutral",
        AutomationControlMode::ManualFallback => "status-pill warning",
    }
}
fn status_class_health(value: AutomationHealthState) -> &'static str {
    match value {
        AutomationHealthState::Healthy => "status-pill success",
        AutomationHealthState::Degraded => "status-pill warning",
        AutomationHealthState::Faulted => "status-pill danger",
        AutomationHealthState::Unknown | AutomationHealthState::Offline => "status-pill neutral",
    }
}
fn status_class_command(value: AutomationCommandStatus) -> &'static str {
    match value {
        AutomationCommandStatus::Succeeded | AutomationCommandStatus::ResolvedManually => {
            "status-pill success"
        }
        AutomationCommandStatus::Failed | AutomationCommandStatus::ManualReview => {
            "status-pill danger"
        }
        AutomationCommandStatus::Queued
        | AutomationCommandStatus::Delivered
        | AutomationCommandStatus::Accepted => "status-pill warning",
        AutomationCommandStatus::Cancelled => "status-pill neutral",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::Revision;
    #[test]
    fn automation_status_labels_cover_every_terminal_and_recovery_state() {
        for value in [
            AutomationCommandStatus::Queued,
            AutomationCommandStatus::Delivered,
            AutomationCommandStatus::Accepted,
            AutomationCommandStatus::Succeeded,
            AutomationCommandStatus::Failed,
            AutomationCommandStatus::ManualReview,
            AutomationCommandStatus::ResolvedManually,
            AutomationCommandStatus::Cancelled,
        ] {
            assert!(!command_label(value).is_empty());
            assert!(status_class_command(value).starts_with("status-pill"));
        }
    }
    #[test]
    fn revisions_remain_positive_in_control_requests() {
        assert!(Revision::new(1).is_ok());
    }
}
