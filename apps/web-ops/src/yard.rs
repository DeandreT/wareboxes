use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AssignYardVisitDoorRequest, ConfigureYardLocationRequest, CreateYardAppointmentRequest,
    GateInYardVisitRequest, MoveYardVisitRequest, RegisterYardAssetRequest, YardAppointmentStatus,
    YardAssetKind, YardDirection, YardDockOperationRequest, YardLifecycleRequest, YardLocationKind,
    YardOperation, YardVisitResponse, YardVisitStatus, YardWorkspaceResponse,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build dispatches yard commands")
)]
enum PendingCommand {
    ConfigureLocation(ConfigureYardLocationRequest, String),
    RegisterAsset(RegisterYardAssetRequest, String),
    CreateAppointment(CreateYardAppointmentRequest, String),
    CancelAppointment(i64, YardLifecycleRequest, String),
    NoShowAppointment(i64, YardLifecycleRequest, String),
    GateIn(GateInYardVisitRequest, String),
    Spot(i64, MoveYardVisitRequest, String),
    AssignDoor(i64, AssignYardVisitDoorRequest, String),
    StartOperation(i64, YardDockOperationRequest, String),
    CompleteOperation(i64, YardDockOperationRequest, String),
    Reject(i64, YardLifecycleRequest, String),
    GateOut(i64, YardLifecycleRequest, String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialog {
    Appointment,
    GateIn,
    Location,
    Asset,
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build consumes yard command callbacks")
)]
struct Signals {
    workspace: RwSignal<YardWorkspaceResponse>,
    facility_filter: RwSignal<Option<i64>>,
    owner_filter: RwSignal<Option<i64>>,
    include_completed: RwSignal<bool>,
    selected_visit: RwSignal<Option<i64>>,
    loading: RwSignal<bool>,
    pending: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    generation: RwSignal<u64>,
    dialog: RwSignal<Option<Dialog>>,
    action_location_id: RwSignal<Option<i64>>,
    action_note: RwSignal<String>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[derive(Clone, Copy)]
struct Drafts {
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    direction: RwSignal<YardDirection>,
    appointment_number: RwSignal<String>,
    scheduled_from: RwSignal<String>,
    scheduled_until: RwSignal<String>,
    carrier: RwSignal<String>,
    asset_kind: RwSignal<YardAssetKind>,
    expected_asset_number: RwSignal<String>,
    free_minutes: RwSignal<String>,
    note: RwSignal<String>,
    asset_number: RwSignal<String>,
    location_code: RwSignal<String>,
    location_name: RwSignal<String>,
    location_kind: RwSignal<YardLocationKind>,
    gate_appointment_id: RwSignal<Option<i64>>,
    gate_asset_id: RwSignal<Option<i64>>,
    gate_location_id: RwSignal<Option<i64>>,
    driver_name: RwSignal<String>,
}

#[component]
pub(crate) fn YardWorkspace(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let default_facility = access.facilities.first().map(|item| item.id);
    let default_owner = access.inventory_owners.first().map(|item| item.id);
    let access = StoredValue::new(access);
    let signals = Signals {
        workspace: RwSignal::new(empty_workspace()),
        facility_filter: RwSignal::new(None),
        owner_filter: RwSignal::new(None),
        include_completed: RwSignal::new(false),
        selected_visit: RwSignal::new(None),
        loading: RwSignal::new(true),
        pending: RwSignal::new(false),
        load_error: RwSignal::new(None),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        generation: RwSignal::new(0),
        dialog: RwSignal::new(None),
        action_location_id: RwSignal::new(None),
        action_note: RwSignal::new(String::new()),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = Drafts {
        facility_id: RwSignal::new(default_facility),
        owner_id: RwSignal::new(default_owner),
        direction: RwSignal::new(YardDirection::Inbound),
        appointment_number: RwSignal::new(String::new()),
        scheduled_from: RwSignal::new(String::new()),
        scheduled_until: RwSignal::new(String::new()),
        carrier: RwSignal::new(String::new()),
        asset_kind: RwSignal::new(YardAssetKind::Trailer),
        expected_asset_number: RwSignal::new(String::new()),
        free_minutes: RwSignal::new("120".into()),
        note: RwSignal::new(String::new()),
        asset_number: RwSignal::new(String::new()),
        location_code: RwSignal::new(String::new()),
        location_name: RwSignal::new(String::new()),
        location_kind: RwSignal::new(YardLocationKind::Parking),
        gate_appointment_id: RwSignal::new(None),
        gate_asset_id: RwSignal::new(None),
        gate_location_id: RwSignal::new(None),
        driver_name: RwSignal::new(String::new()),
    };

    Effect::new(move || {
        let _ = (
            signals.facility_filter.get(),
            signals.owner_filter.get(),
            signals.include_completed.get(),
        );
        load_workspace(signals);
    });

    let refresh = Callback::new(move |_| load_workspace(signals));
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    });

    view! {
        <section class="yard-workspace">
            <header class="yard-toolbar">
                <div class="yard-heading">
                    <Icon icon=UiIcon::Shipping/>
                    <div><h1>"Yard control"</h1><span>"Appointments, gate, dock execution, and detention"</span></div>
                </div>
                <label><span>"Facility"</span><select prop:value=move || option_value(signals.facility_filter.get()) on:change=move |event| signals.facility_filter.set(parse_id(&event_target_value(&event)))><option value="">"All facilities"</option>{access.with_value(|value| scope_options(&value.facilities))}</select></label>
                <label><span>"Client"</span><select prop:value=move || option_value(signals.owner_filter.get()) on:change=move |event| signals.owner_filter.set(parse_id(&event_target_value(&event)))><option value="">"All clients"</option>{access.with_value(|value| scope_options(&value.inventory_owners))}</select></label>
                <label class="yard-history-toggle"><input type="checkbox" prop:checked=move || signals.include_completed.get() on:change=move |event| signals.include_completed.set(event_target_checked(&event))/><span>"History"</span></label>
                <button class="icon-button" type="button" title="Refresh yard" aria-label="Refresh yard" disabled=move || signals.loading.get() on:click=move |_|refresh.run(())><Icon icon=UiIcon::Refresh/></button>
                <button class="button secondary-action compact" type="button" on:click=move |_|signals.dialog.set(Some(Dialog::Location))>"Add location"</button>
                <button class="button secondary-action compact" type="button" on:click=move |_|signals.dialog.set(Some(Dialog::Asset))>"Register asset"</button>
                <button class="button secondary-action compact" type="button" on:click=move |_|signals.dialog.set(Some(Dialog::Appointment))>"New appointment"</button>
                <button class="button primary-action compact" type="button" on:click=move |_|signals.dialog.set(Some(Dialog::GateIn))>"Gate in"</button>
            </header>

            {move || if signals.loading.get() && signals.workspace.get().visits.is_empty() {
                view! { <div class="yard-state" role="status" aria-live="polite"><span class="loading-line"></span><h2>"Loading yard positions"</h2></div> }.into_any()
            } else if let Some(message)=signals.load_error.get() {
                view! { <div class="yard-state error" role="alert"><h2>"Yard data unavailable"</h2><p>{message}</p><button class="button secondary-action" type="button" on:click=move |_|refresh.run(())>"Try again"</button></div> }.into_any()
            } else {
                yard_body(signals).into_any()
            }}

            {move || signals.dialog.get().map(|dialog| command_dialog(dialog, signals, drafts, access))}
            <Show when=move || signals.command_error.get().is_some()>
                <div class="yard-command-error" role="alert">
                    <span>{move || signals.command_error.get().unwrap_or_default()}</span>
                    <Show when=move || signals.retry.get().is_some()>
                        <button class="button secondary-action compact" type="button" disabled=move || signals.pending.get() on:click=move |_|retry.run(())>"Retry exact command"</button>
                    </Show>
                    <button class="text-button" type="button" on:click=move |_|signals.command_error.set(None)>"Dismiss"</button>
                </div>
            </Show>
        </section>
    }
}

fn yard_body(signals: Signals) -> AnyView {
    view! {
        <div class="yard-body">
            {move || yard_metrics(signals.workspace.get())}
            <div class="yard-board">
                <section class="yard-queue">
                    <header><div><h2>"Live visits"</h2><span>{move || format!("{} visible", signals.workspace.get().visits.len())}</span></div></header>
                    <div class="yard-table-scroll"><table><caption class="sr-only">"Live yard visits and current positions"</caption><thead><tr><th>"Asset"</th><th>"State"</th><th>"Position"</th><th>"Direction"</th><th>"Client"</th><th>"Gate in"</th></tr></thead><tbody>{move || { let visits=signals.workspace.get().visits; if visits.is_empty(){view!{<tr><td colspan="6" class="empty-row" role="status" aria-live="polite">"No yard visits match the active filters."</td></tr>}.into_any()}else{visits.into_iter().map(|visit|{let id=visit.visit_id;let selected=signals.selected_visit.get()==Some(id);view!{<tr class:selected=selected on:click=move |_|signals.selected_visit.set(Some(id))><td><button type="button" class="row-link">{visit.asset_number}</button><small>{visit.carrier}</small></td><td><span class=status_class(visit.status)>{visit_status(visit.status)}</span></td><td>{visit.current_location_code.unwrap_or_else(||"Unassigned".into())}</td><td>{direction_label(visit.direction)}</td><td>{visit.inventory_owner_name}</td><td>{short_timestamp(&visit.gated_in_at)}</td></tr>}}).collect_view().into_any()} }}</tbody></table></div>
                    <section class="yard-appointments"><header><h3>"Appointment board"</h3><span>{move ||format!("{} appointments",signals.workspace.get().appointments.len())}</span></header><div class="yard-table-scroll"><table><caption class="sr-only">"Scheduled and completed yard appointments"</caption><thead><tr><th>"Appointment"</th><th>"Window"</th><th>"Carrier / asset"</th><th>"State"</th><th><span class="sr-only">"Appointment actions"</span></th></tr></thead><tbody>{move || { let appointments=signals.workspace.get().appointments; if appointments.is_empty(){view!{<tr><td colspan="5" class="empty-row" role="status" aria-live="polite">"No yard appointments match the active filters."</td></tr>}.into_any()}else{appointments.into_iter().map(|appointment|{let id=appointment.appointment_id;let revision=appointment.revision;let is_scheduled=appointment.status==YardAppointmentStatus::Scheduled;view!{<tr><td><strong>{appointment.appointment_number}</strong><small>{format!("{} · {}",appointment.inventory_owner_name,appointment.facility_name)}</small></td><td>{format!("{} – {}",short_timestamp(&appointment.scheduled_from),short_timestamp(&appointment.scheduled_until))}</td><td>{appointment.carrier}<small>{appointment.expected_asset_number.unwrap_or_else(||asset_kind_label(appointment.expected_asset_kind).into())}</small></td><td><span class=appointment_status_class(appointment.status)>{appointment_status(appointment.status)}</span></td><td>{is_scheduled.then(||view!{<div class="yard-row-actions"><button class="text-button danger" type="button" disabled=move ||signals.pending.get() on:click=move |_|dispatch(signals,PendingCommand::CancelAppointment(id,YardLifecycleRequest{expected_revision:revision,note:"Cancelled from yard control".into()},api::new_idempotency_key()))>"Cancel"</button><button class="text-button" type="button" disabled=move ||signals.pending.get() on:click=move |_|dispatch(signals,PendingCommand::NoShowAppointment(id,YardLifecycleRequest{expected_revision:revision,note:"Carrier did not arrive".into()},api::new_idempotency_key()))>"No-show"</button></div>})}</td></tr>}}).collect_view().into_any()} }}</tbody></table></div></section>
                </section>
                <aside class="yard-detail">{move ||selected_visit_view(signals)}</aside>
            </div>
        </div>
    }.into_any()
}

fn yard_metrics(workspace: YardWorkspaceResponse) -> AnyView {
    let scheduled = workspace
        .appointments
        .iter()
        .filter(|item| item.status == YardAppointmentStatus::Scheduled)
        .count();
    let active = workspace
        .visits
        .iter()
        .filter(|item| item.status != YardVisitStatus::GatedOut)
        .count();
    let at_door = workspace
        .visits
        .iter()
        .filter(|item| {
            matches!(
                item.status,
                YardVisitStatus::AtDoor | YardVisitStatus::Loading | YardVisitStatus::Unloading
            )
        })
        .count();
    let ready = workspace
        .visits
        .iter()
        .filter(|item| {
            matches!(
                item.status,
                YardVisitStatus::ReadyToDepart | YardVisitStatus::Rejected
            )
        })
        .count();
    view! { <dl class="yard-metrics"><div><dt>"Scheduled"</dt><dd>{scheduled}</dd><small>"appointments"</small></div><div><dt>"On property"</dt><dd>{active}</dd><small>"active visits"</small></div><div><dt>"Doors occupied"</dt><dd>{at_door}</dd><small>"dock positions"</small></div><div><dt>"Ready at gate"</dt><dd>{ready}</dd><small>"departures"</small></div></dl> }.into_any()
}

fn selected_visit_view(signals: Signals) -> AnyView {
    let Some(id) = signals.selected_visit.get() else {
        return view! { <div class="yard-empty"><Icon icon=UiIcon::Shipping/><h2>"Select an active visit"</h2><p>"Review position history and execute the next controlled yard action."</p></div> }.into_any();
    };
    let Some(visit) = signals
        .workspace
        .get()
        .visits
        .into_iter()
        .find(|item| item.visit_id == id)
    else {
        return view! { <div class="yard-empty"><h2>"Visit is no longer visible"</h2><p>"Refresh or enable history to review completed movements."</p></div> }.into_any();
    };
    let visit_for_actions = visit.clone();
    view! {
        <div class="yard-detail-scroll">
            <header class="yard-detail-header"><div><span class="eyebrow">{format!("Visit #{}",visit.visit_id)}</span><h2>{visit.asset_number.clone()}</h2><p>{format!("{} · {} · {}",visit.carrier,visit.inventory_owner_name,visit.facility_name)}</p></div><span class=status_class(visit.status)>{visit_status(visit.status)}</span></header>
            <dl class="yard-facts"><div><dt>"Direction"</dt><dd>{direction_label(visit.direction)}</dd></div><div><dt>"Driver"</dt><dd>{visit.driver_name}</dd></div><div><dt>"Position"</dt><dd>{visit.current_location_code.unwrap_or_else(||"Unassigned".into())}</dd></div><div><dt>"Appointment"</dt><dd>{visit.appointment_number.unwrap_or_else(||"Walk-in".into())}</dd></div><div><dt>"Gate in"</dt><dd>{short_timestamp(&visit.gated_in_at)}</dd></div><div><dt>"Revision"</dt><dd>{visit.revision.to_string()}</dd></div></dl>
            {visit.detention.map(|item|view!{<section class="yard-detention"><div><span>"Total dwell"</span><strong>{format!("{} min",item.total_minutes)}</strong></div><div><span>"Free time"</span><strong>{format!("{} min",item.free_minutes)}</strong></div><div><span>"Detention"</span><strong>{format!("{} h",item.billable_hours)}</strong></div><div><span>"Billing evidence"</span><strong>{item.billable_event_id.map(|id|format!("Event #{id}")).unwrap_or_else(||"No charge".into())}</strong></div></section>})}
            {visit_actions(&visit_for_actions,signals)}
            <section class="yard-timeline"><header><h3>"Movement history"</h3><span>{format!("{} events",visit.events.len())}</span></header>{visit.events.into_iter().rev().map(|event|view!{<article><span class="yard-event-dot"></span><div><strong>{event_kind_label(event.kind)}</strong><p>{format!("{} → {}",event.from_status.map(visit_status).unwrap_or("Entry"),visit_status(event.to_status))}</p>{event.note.map(|note|view!{<small>{note}</small>})}</div><time>{short_timestamp(&event.occurred_at)}</time></article>}).collect_view()}</section>
        </div>
    }.into_any()
}

fn visit_actions(visit: &YardVisitResponse, signals: Signals) -> AnyView {
    let id = visit.visit_id;
    let revision = visit.revision;
    let operation = match visit.direction {
        YardDirection::Inbound => YardOperation::Unloading,
        YardDirection::Outbound => YardOperation::Loading,
    };
    let locations = signals.workspace.get().locations;
    let move_options=locations.iter().filter(|item|item.active&&item.facility_id==visit.facility_id&&item.kind!=YardLocationKind::DockDoor).map(|item|view!{<option value=item.location_id>{format!("{} · {}",item.code,item.name)}</option>}).collect_view();
    let door_options=locations.into_iter().filter(|item|item.active&&item.facility_id==visit.facility_id&&item.kind==YardLocationKind::DockDoor).map(|item|view!{<option value=item.location_id>{format!("{} · {}",item.code,item.name)}</option>}).collect_view();
    let move_action = matches!(
        visit.status,
        YardVisitStatus::GatedIn | YardVisitStatus::InYard
    );
    let start_action = visit.status == YardVisitStatus::AtDoor;
    let complete_action = matches!(
        visit.status,
        YardVisitStatus::Loading | YardVisitStatus::Unloading
    );
    let depart_action = matches!(
        visit.status,
        YardVisitStatus::ReadyToDepart | YardVisitStatus::Rejected
    );
    view! {
        <section class="yard-actions"><header><h3>"Next action"</h3><span>"Revision-guarded execution"</span></header>
            {(move_action||visit.status==YardVisitStatus::AtDoor).then(||view!{<div class="yard-action-fields"><label><span>"Destination"</span><select prop:value=move ||option_value(signals.action_location_id.get()) on:change=move |event|signals.action_location_id.set(parse_id(&event_target_value(&event)))><option value="">"Choose position"</option>{move_options}</select></label><label><span>"Action note"</span><input prop:value=move ||signals.action_note.get() on:input=move |event|signals.action_note.set(event_target_value(&event))/></label></div>})}
            <div class="yard-action-buttons">
                {move_action.then(||view!{<button class="button secondary-action" type="button" disabled=move ||signals.pending.get()||signals.action_location_id.get().is_none() on:click=move |_|{if let Some(destination_location_id)=signals.action_location_id.get_untracked(){dispatch(signals,PendingCommand::Spot(id,MoveYardVisitRequest{expected_revision:revision,destination_location_id,note:action_note(signals)},api::new_idempotency_key()))}}>"Spot asset"</button>})}
                {move_action.then(||view!{<label class="yard-inline-door"><span>"Dock door"</span><select prop:value=move ||option_value(signals.action_location_id.get()) on:change=move |event|signals.action_location_id.set(parse_id(&event_target_value(&event)))><option value="">"Choose door"</option>{door_options}</select></label><button class="button primary-action" type="button" disabled=move ||signals.pending.get()||signals.action_location_id.get().is_none() on:click=move |_|{if let Some(door_location_id)=signals.action_location_id.get_untracked(){dispatch(signals,PendingCommand::AssignDoor(id,AssignYardVisitDoorRequest{expected_revision:revision,door_location_id,note:action_note(signals)},api::new_idempotency_key()))}}>"Assign door"</button>})}
                {start_action.then(||view!{<button class="button primary-action" type="button" disabled=move ||signals.pending.get() on:click=move |_|dispatch(signals,PendingCommand::StartOperation(id,YardDockOperationRequest{expected_revision:revision,operation,note:action_note(signals)},api::new_idempotency_key()))>{format!("Start {}",operation_label(operation).to_lowercase())}</button>})}
                {complete_action.then(||view!{<button class="button primary-action" type="button" disabled=move ||signals.pending.get() on:click=move |_|dispatch(signals,PendingCommand::CompleteOperation(id,YardDockOperationRequest{expected_revision:revision,operation,note:action_note(signals)},api::new_idempotency_key()))>{format!("Complete {}",operation_label(operation).to_lowercase())}</button>})}
                {move_action.then(||view!{<button class="button danger-action" type="button" disabled=move ||signals.pending.get() on:click=move |_|dispatch(signals,PendingCommand::Reject(id,YardLifecycleRequest{expected_revision:revision,note:required_action_note(signals,"Rejected at gate")},api::new_idempotency_key()))>"Reject visit"</button>})}
                {depart_action.then(||view!{<button class="button primary-action" type="button" disabled=move ||signals.pending.get() on:click=move |_|dispatch(signals,PendingCommand::GateOut(id,YardLifecycleRequest{expected_revision:revision,note:required_action_note(signals,"Gate-out confirmed")},api::new_idempotency_key()))>"Confirm gate-out"</button>})}
            </div>
        </section>
    }.into_any()
}

fn command_dialog(
    dialog: Dialog,
    signals: Signals,
    drafts: Drafts,
    access: StoredValue<AccessScopeWorkspace>,
) -> AnyView {
    let title = match dialog {
        Dialog::Appointment => "Schedule appointment",
        Dialog::GateIn => "Gate in asset",
        Dialog::Location => "Configure yard location",
        Dialog::Asset => "Register yard asset",
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let result = build_dialog_command(dialog, drafts);
        match result {
            Ok(command) => {
                signals.dialog.set(None);
                dispatch(signals, command)
            }
            Err(message) => signals.command_error.set(Some(message)),
        }
    };
    view! {
        <div class="yard-dialog-backdrop" role="presentation">
            <form class="yard-dialog" role="dialog" aria-modal="true" aria-labelledby="yard-dialog-title" aria-busy=move || signals.pending.get().to_string() on:submit=submit>
                <header><div><span class="eyebrow">"Yard control"</span><h2 id="yard-dialog-title">{title}</h2></div><button class="icon-button" type="button" aria-label="Close dialog" on:click=move |_|signals.dialog.set(None)><Icon icon=UiIcon::Close/></button></header>
                <div class="yard-dialog-fields">{match dialog {
                    Dialog::Appointment=>appointment_fields(drafts,access),
                    Dialog::GateIn=>gate_in_fields(drafts,signals,access),
                    Dialog::Location=>location_fields(drafts,access),
                    Dialog::Asset=>asset_fields(drafts),
                }}</div>
                <footer><button class="button quiet-action" type="button" on:click=move |_|signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move ||signals.pending.get()>{move ||if signals.pending.get(){"Submitting..."}else{"Confirm"}}</button></footer>
            </form>
        </div>
    }.into_any()
}

fn appointment_fields(drafts: Drafts, access: StoredValue<AccessScopeWorkspace>) -> AnyView {
    view!{<>
        <label><span>"Client"</span><select required prop:value=move ||option_value(drafts.owner_id.get()) on:change=move |event|drafts.owner_id.set(parse_id(&event_target_value(&event)))><option value="">"Select client"</option>{access.with_value(|value|scope_options(&value.inventory_owners))}</select></label>
        <label><span>"Facility"</span><select required prop:value=move ||option_value(drafts.facility_id.get()) on:change=move |event|drafts.facility_id.set(parse_id(&event_target_value(&event)))><option value="">"Select facility"</option>{access.with_value(|value|scope_options(&value.facilities))}</select></label>
        <label><span>"Direction"</span><select on:change=move |event|drafts.direction.set(parse_direction(&event_target_value(&event)))><option value="inbound">"Inbound"</option><option value="outbound">"Outbound"</option></select></label>
        <label><span>"Appointment number"</span><input required maxlength="120" prop:value=move ||drafts.appointment_number.get() on:input=move |event|drafts.appointment_number.set(event_target_value(&event))/></label>
        <label><span>"Window starts (UTC)"</span><input required type="datetime-local" prop:value=move ||drafts.scheduled_from.get() on:input=move |event|drafts.scheduled_from.set(event_target_value(&event))/></label>
        <label><span>"Window ends (UTC)"</span><input required type="datetime-local" prop:value=move ||drafts.scheduled_until.get() on:input=move |event|drafts.scheduled_until.set(event_target_value(&event))/></label>
        <label><span>"Carrier"</span><input required maxlength="200" prop:value=move ||drafts.carrier.get() on:input=move |event|drafts.carrier.set(event_target_value(&event))/></label>
        <label><span>"Expected equipment"</span><select on:change=move |event|drafts.asset_kind.set(parse_asset_kind(&event_target_value(&event)))><option value="trailer">"Trailer"</option><option value="container">"Container"</option></select></label>
        <label><span>"Expected asset number"</span><input prop:value=move ||drafts.expected_asset_number.get() on:input=move |event|drafts.expected_asset_number.set(event_target_value(&event))/></label>
        <label><span>"Free dwell minutes"</span><input required type="number" min="0" max="10080" prop:value=move ||drafts.free_minutes.get() on:input=move |event|drafts.free_minutes.set(event_target_value(&event))/></label>
        <label class="full"><span>"Note"</span><textarea prop:value=move ||drafts.note.get() on:input=move |event|drafts.note.set(event_target_value(&event))></textarea></label>
    </>}.into_any()
}

fn gate_in_fields(
    drafts: Drafts,
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
) -> AnyView {
    view!{<>
        <label class="full"><span>"Scheduled appointment (optional)"</span><select prop:value=move ||option_value(drafts.gate_appointment_id.get()) on:change=move |event|drafts.gate_appointment_id.set(parse_id(&event_target_value(&event)))><option value="">"Walk-in visit"</option>{move ||signals.workspace.get().appointments.into_iter().filter(|item|item.status==YardAppointmentStatus::Scheduled).map(|item|view!{<option value=item.appointment_id>{format!("{} · {} · {}",item.appointment_number,item.carrier,item.facility_name)}</option>}).collect_view()}</select></label>
        <label><span>"Client"</span><select required prop:value=move ||option_value(drafts.owner_id.get()) on:change=move |event|drafts.owner_id.set(parse_id(&event_target_value(&event)))><option value="">"Select client"</option>{access.with_value(|value|scope_options(&value.inventory_owners))}</select></label>
        <label><span>"Facility"</span><select required prop:value=move ||option_value(drafts.facility_id.get()) on:change=move |event|drafts.facility_id.set(parse_id(&event_target_value(&event)))><option value="">"Select facility"</option>{access.with_value(|value|scope_options(&value.facilities))}</select></label>
        <label><span>"Direction"</span><select on:change=move |event|drafts.direction.set(parse_direction(&event_target_value(&event)))><option value="inbound">"Inbound"</option><option value="outbound">"Outbound"</option></select></label>
        <label><span>"Registered asset"</span><select required prop:value=move ||option_value(drafts.gate_asset_id.get()) on:change=move |event|drafts.gate_asset_id.set(parse_id(&event_target_value(&event)))><option value="">"Select asset"</option>{move ||signals.workspace.get().assets.into_iter().filter(|item|item.active).map(|item|view!{<option value=item.asset_id>{format!("{} · {}",item.asset_number,item.carrier)}</option>}).collect_view()}</select></label>
        <label><span>"Gate"</span><select required prop:value=move ||option_value(drafts.gate_location_id.get()) on:change=move |event|drafts.gate_location_id.set(parse_id(&event_target_value(&event)))><option value="">"Select gate"</option>{move ||signals.workspace.get().locations.into_iter().filter(|item|item.active&&item.kind==YardLocationKind::Gate).map(|item|view!{<option value=item.location_id>{format!("{} · {}",item.code,item.facility_name)}</option>}).collect_view()}</select></label>
        <label><span>"Driver name"</span><input required maxlength="200" prop:value=move ||drafts.driver_name.get() on:input=move |event|drafts.driver_name.set(event_target_value(&event))/></label>
        <label class="full"><span>"Gate note"</span><textarea prop:value=move ||drafts.note.get() on:input=move |event|drafts.note.set(event_target_value(&event))></textarea></label>
    </>}.into_any()
}

fn location_fields(drafts: Drafts, access: StoredValue<AccessScopeWorkspace>) -> AnyView {
    view!{<><label><span>"Facility"</span><select required prop:value=move ||option_value(drafts.facility_id.get()) on:change=move |event|drafts.facility_id.set(parse_id(&event_target_value(&event)))><option value="">"Select facility"</option>{access.with_value(|value|scope_options(&value.facilities))}</select></label><label><span>"Location type"</span><select on:change=move |event|drafts.location_kind.set(parse_location_kind(&event_target_value(&event)))><option value="gate">"Gate"</option><option selected value="parking">"Parking"</option><option value="dock_door">"Dock door"</option><option value="inspection">"Inspection"</option><option value="staging">"Staging"</option></select></label><label><span>"Code"</span><input required maxlength="80" prop:value=move ||drafts.location_code.get() on:input=move |event|drafts.location_code.set(event_target_value(&event))/></label><label><span>"Name"</span><input required maxlength="200" prop:value=move ||drafts.location_name.get() on:input=move |event|drafts.location_name.set(event_target_value(&event))/></label></>}.into_any()
}

fn asset_fields(drafts: Drafts) -> AnyView {
    view!{<><label><span>"Equipment type"</span><select on:change=move |event|drafts.asset_kind.set(parse_asset_kind(&event_target_value(&event)))><option value="trailer">"Trailer"</option><option value="container">"Container"</option></select></label><label><span>"Asset number"</span><input required maxlength="120" prop:value=move ||drafts.asset_number.get() on:input=move |event|drafts.asset_number.set(event_target_value(&event))/></label><label class="full"><span>"Carrier"</span><input required maxlength="200" prop:value=move ||drafts.carrier.get() on:input=move |event|drafts.carrier.set(event_target_value(&event))/></label></>}.into_any()
}

fn build_dialog_command(dialog: Dialog, drafts: Drafts) -> Result<PendingCommand, String> {
    let facility_id = drafts
        .facility_id
        .get_untracked()
        .ok_or_else(|| "Select a facility.".to_owned())?;
    let command = match dialog {
        Dialog::Location => PendingCommand::ConfigureLocation(
            ConfigureYardLocationRequest {
                facility_id,
                code: required(
                    drafts.location_code.get_untracked(),
                    "Enter a location code.",
                )?,
                name: required(
                    drafts.location_name.get_untracked(),
                    "Enter a location name.",
                )?,
                kind: drafts.location_kind.get_untracked(),
            },
            api::new_idempotency_key(),
        ),
        Dialog::Asset => PendingCommand::RegisterAsset(
            RegisterYardAssetRequest {
                kind: drafts.asset_kind.get_untracked(),
                asset_number: required(
                    drafts.asset_number.get_untracked(),
                    "Enter an asset number.",
                )?,
                carrier: required(drafts.carrier.get_untracked(), "Enter a carrier.")?,
            },
            api::new_idempotency_key(),
        ),
        Dialog::Appointment => PendingCommand::CreateAppointment(
            CreateYardAppointmentRequest {
                inventory_owner_id: drafts
                    .owner_id
                    .get_untracked()
                    .ok_or_else(|| "Select a client.".to_owned())?,
                facility_id,
                direction: drafts.direction.get_untracked(),
                appointment_number: required(
                    drafts.appointment_number.get_untracked(),
                    "Enter an appointment number.",
                )?,
                scheduled_from: api_timestamp(required(
                    drafts.scheduled_from.get_untracked(),
                    "Enter the start of the appointment window.",
                )?),
                scheduled_until: api_timestamp(required(
                    drafts.scheduled_until.get_untracked(),
                    "Enter the end of the appointment window.",
                )?),
                carrier: required(drafts.carrier.get_untracked(), "Enter a carrier.")?,
                expected_asset_kind: drafts.asset_kind.get_untracked(),
                expected_asset_number: nonblank(drafts.expected_asset_number.get_untracked()),
                inbound_load_id: None,
                outbound_load_id: None,
                free_minutes: drafts
                    .free_minutes
                    .get_untracked()
                    .parse::<u32>()
                    .map_err(|_| "Free dwell minutes must be a whole number.".to_owned())?,
                note: nonblank(drafts.note.get_untracked()),
            },
            api::new_idempotency_key(),
        ),
        Dialog::GateIn => PendingCommand::GateIn(
            GateInYardVisitRequest {
                appointment_id: drafts.gate_appointment_id.get_untracked(),
                inventory_owner_id: drafts
                    .owner_id
                    .get_untracked()
                    .ok_or_else(|| "Select a client.".to_owned())?,
                facility_id,
                direction: drafts.direction.get_untracked(),
                asset_id: drafts
                    .gate_asset_id
                    .get_untracked()
                    .ok_or_else(|| "Select a registered asset.".to_owned())?,
                driver_name: required(
                    drafts.driver_name.get_untracked(),
                    "Enter the driver name.",
                )?,
                gate_location_id: drafts
                    .gate_location_id
                    .get_untracked()
                    .ok_or_else(|| "Select a gate.".to_owned())?,
                note: nonblank(drafts.note.get_untracked()),
            },
            api::new_idempotency_key(),
        ),
    };
    Ok(command)
}

fn dispatch(signals: Signals, command: PendingCommand) {
    if signals.pending.get_untracked() {
        return;
    }
    signals.pending.set(true);
    signals.command_error.set(None);
    signals.retry.set(Some(command.clone()));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, command);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = match &command {
            PendingCommand::ConfigureLocation(request, key) => {
                api::configure_yard_location(request, key).await.map(|_| ())
            }
            PendingCommand::RegisterAsset(request, key) => {
                api::register_yard_asset(request, key).await.map(|_| ())
            }
            PendingCommand::CreateAppointment(request, key) => {
                api::create_yard_appointment(request, key).await.map(|_| ())
            }
            PendingCommand::CancelAppointment(id, request, key) => {
                api::cancel_yard_appointment(*id, request, key)
                    .await
                    .map(|_| ())
            }
            PendingCommand::NoShowAppointment(id, request, key) => {
                api::no_show_yard_appointment(*id, request, key)
                    .await
                    .map(|_| ())
            }
            PendingCommand::GateIn(request, key) => {
                api::gate_in_yard_visit(request, key).await.map(|_| ())
            }
            PendingCommand::Spot(id, request, key) => {
                api::spot_yard_visit(*id, request, key).await.map(|_| ())
            }
            PendingCommand::AssignDoor(id, request, key) => {
                api::assign_yard_visit_door(*id, request, key)
                    .await
                    .map(|_| ())
            }
            PendingCommand::StartOperation(id, request, key) => {
                api::start_yard_operation(*id, request, key)
                    .await
                    .map(|_| ())
            }
            PendingCommand::CompleteOperation(id, request, key) => {
                api::complete_yard_operation(*id, request, key)
                    .await
                    .map(|_| ())
            }
            PendingCommand::Reject(id, request, key) => {
                api::reject_yard_visit(*id, request, key).await.map(|_| ())
            }
            PendingCommand::GateOut(id, request, key) => {
                api::gate_out_yard_visit(*id, request, key)
                    .await
                    .map(|_| ())
            }
        };
        signals.pending.set(false);
        match result {
            Ok(()) => {
                signals.retry.set(None);
                signals.action_location_id.set(None);
                signals.action_note.set(String::new());
                signals.toasts.success("Yard control updated.");
                load_workspace(signals)
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None)
                }
                signals.toasts.error(error.message.clone());
                signals.command_error.set(Some(error.message))
            }
        }
    });
}

fn load_workspace(signals: Signals) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::yard_workspace(api::YardFilters {
            facility_id: signals.facility_filter.get_untracked(),
            inventory_owner_id: signals.owner_filter.get_untracked(),
            include_completed: signals.include_completed.get_untracked(),
        })
        .await
        {
            Ok(workspace) if signals.generation.get_untracked() == generation => {
                let selected = signals.selected_visit.get_untracked();
                if selected.is_none()
                    || !workspace
                        .visits
                        .iter()
                        .any(|item| Some(item.visit_id) == selected)
                {
                    signals
                        .selected_visit
                        .set(workspace.visits.first().map(|item| item.visit_id))
                }
                signals.workspace.set(workspace);
                signals.load_error.set(None)
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.load_error.set(Some(error.message)),
        }
        if signals.generation.get_untracked() == generation {
            signals.loading.set(false)
        }
    });
}

fn empty_workspace() -> YardWorkspaceResponse {
    YardWorkspaceResponse {
        locations: Vec::new(),
        assets: Vec::new(),
        appointments: Vec::new(),
        visits: Vec::new(),
        next_cursor: None,
    }
}
fn scope_options(values: &[wareboxes_api_contract::web::access::AccessScopeResource]) -> AnyView {
    values
        .iter()
        .map(|item| view! {<option value=item.id>{item.name.clone()}</option>})
        .collect_view()
        .into_any()
}
fn option_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}
fn parse_direction(value: &str) -> YardDirection {
    if value == "outbound" {
        YardDirection::Outbound
    } else {
        YardDirection::Inbound
    }
}
fn parse_asset_kind(value: &str) -> YardAssetKind {
    if value == "container" {
        YardAssetKind::Container
    } else {
        YardAssetKind::Trailer
    }
}
fn parse_location_kind(value: &str) -> YardLocationKind {
    match value {
        "gate" => YardLocationKind::Gate,
        "dock_door" => YardLocationKind::DockDoor,
        "inspection" => YardLocationKind::Inspection,
        "staging" => YardLocationKind::Staging,
        _ => YardLocationKind::Parking,
    }
}
fn required(value: String, message: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err(message.into())
    } else {
        Ok(value)
    }
}
fn nonblank(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
fn api_timestamp(value: String) -> String {
    if value.len() == 16 && !value.ends_with('Z') {
        format!("{value}:00Z")
    } else {
        value
    }
}
fn action_note(signals: Signals) -> String {
    signals.action_note.get_untracked().trim().to_owned()
}
fn required_action_note(signals: Signals, fallback: &str) -> String {
    let value = action_note(signals);
    if value.is_empty() {
        fallback.into()
    } else {
        value
    }
}
fn short_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
fn direction_label(value: YardDirection) -> &'static str {
    match value {
        YardDirection::Inbound => "Inbound",
        YardDirection::Outbound => "Outbound",
    }
}
fn asset_kind_label(value: YardAssetKind) -> &'static str {
    match value {
        YardAssetKind::Trailer => "Trailer",
        YardAssetKind::Container => "Container",
    }
}
fn operation_label(value: YardOperation) -> &'static str {
    match value {
        YardOperation::Loading => "Loading",
        YardOperation::Unloading => "Unloading",
    }
}
fn visit_status(value: YardVisitStatus) -> &'static str {
    match value {
        YardVisitStatus::GatedIn => "Gated in",
        YardVisitStatus::InYard => "In yard",
        YardVisitStatus::AtDoor => "At door",
        YardVisitStatus::Loading => "Loading",
        YardVisitStatus::Unloading => "Unloading",
        YardVisitStatus::ReadyToDepart => "Ready to depart",
        YardVisitStatus::Rejected => "Rejected",
        YardVisitStatus::GatedOut => "Gated out",
    }
}
fn status_class(value: YardVisitStatus) -> &'static str {
    match value {
        YardVisitStatus::ReadyToDepart => "status-badge success",
        YardVisitStatus::Rejected => "status-badge danger",
        YardVisitStatus::GatedOut => "status-badge neutral",
        YardVisitStatus::Loading | YardVisitStatus::Unloading => "status-badge warning",
        _ => "status-badge info",
    }
}
fn appointment_status(value: YardAppointmentStatus) -> &'static str {
    match value {
        YardAppointmentStatus::Scheduled => "Scheduled",
        YardAppointmentStatus::CheckedIn => "Checked in",
        YardAppointmentStatus::Completed => "Completed",
        YardAppointmentStatus::Cancelled => "Cancelled",
        YardAppointmentStatus::NoShow => "No-show",
    }
}
fn appointment_status_class(value: YardAppointmentStatus) -> &'static str {
    match value {
        YardAppointmentStatus::Scheduled => "status-badge info",
        YardAppointmentStatus::CheckedIn => "status-badge warning",
        YardAppointmentStatus::Completed => "status-badge success",
        YardAppointmentStatus::Cancelled | YardAppointmentStatus::NoShow => "status-badge neutral",
    }
}
fn event_kind_label(value: wareboxes_api_contract::v1::YardVisitEventKind) -> &'static str {
    use wareboxes_api_contract::v1::YardVisitEventKind::*;
    match value {
        GatedIn => "Gate-in recorded",
        Spotted => "Asset spotted",
        DoorAssigned => "Dock door assigned",
        OperationStarted => "Dock operation started",
        OperationCompleted => "Dock operation completed",
        Rejected => "Visit rejected",
        GatedOut => "Gate-out recorded",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_datetime_is_sent_as_utc_rfc3339() {
        assert_eq!(
            api_timestamp("2026-08-12T10:30".into()),
            "2026-08-12T10:30:00Z"
        );
        assert_eq!(
            api_timestamp("2026-08-12T10:30:00Z".into()),
            "2026-08-12T10:30:00Z"
        );
    }

    #[test]
    fn completed_and_rejected_states_have_clear_labels() {
        assert_eq!(
            visit_status(YardVisitStatus::ReadyToDepart),
            "Ready to depart"
        );
        assert_eq!(appointment_status(YardAppointmentStatus::NoShow), "No-show");
    }
}
