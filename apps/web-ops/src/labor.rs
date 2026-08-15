use std::collections::BTreeMap;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AttendanceIntervalResponse, AttendanceStatus, CancelLaborActivityRequest,
    ChangeEquipmentStatusRequest, ClockOutRequest, CompleteLaborActivityRequest,
    EmployeeCertificationResponse, EquipmentAssetResponse, EquipmentStatus, LaborActivityResponse,
    LaborActivityStatus, LaborExceptionReason, LaborWorkspaceResponse,
    RevokeEmployeeCertificationRequest,
};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

#[path = "labor_labels.rs"]
mod labels;
use labels::*;
#[path = "labor_forms.rs"]
mod forms;
use forms::LaborCommandCenter;

#[derive(Clone)]
enum ActionTarget {
    ClockOut(AttendanceIntervalResponse),
    Complete(LaborActivityResponse),
    Cancel(LaborActivityResponse),
    Equipment(EquipmentAssetResponse),
    Revoke(EmployeeCertificationResponse),
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build dispatches labor commands")
)]
enum PendingCommand {
    ClockOut(i64, ClockOutRequest, String),
    Complete(i64, CompleteLaborActivityRequest, String),
    Cancel(i64, CancelLaborActivityRequest, String),
    Equipment(i64, ChangeEquipmentStatusRequest, String),
    Revoke(i64, RevokeEmployeeCertificationRequest, String),
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build consumes labor workspace callbacks")
)]
struct Signals {
    workspace: RwSignal<LaborWorkspaceResponse>,
    loaded: RwSignal<bool>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    facility_filter: RwSignal<Option<i64>>,
    owner_filter: RwSignal<Option<i64>>,
    employee_filter: RwSignal<Option<i64>>,
    from_date: RwSignal<String>,
    until_date: RwSignal<String>,
    include_history: RwSignal<bool>,
    action: RwSignal<Option<ActionTarget>>,
    action_note: RwSignal<String>,
    action_quantity: RwSignal<String>,
    action_exception_seconds: RwSignal<String>,
    action_exception_reason: RwSignal<Option<LaborExceptionReason>>,
    action_exception_note: RwSignal<String>,
    action_equipment_status: RwSignal<EquipmentStatus>,
    pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn LaborWorkspace(
    access: AccessScopeWorkspace,
    can_execute: bool,
    can_configure: bool,
    can_manage_equipment: bool,
    can_certify: bool,
    can_supervise: bool,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let access = StoredValue::new(access);
    let signals = Signals {
        workspace: RwSignal::new(empty_workspace()),
        loaded: RwSignal::new(false),
        loading: RwSignal::new(true),
        load_error: RwSignal::new(None),
        generation: RwSignal::new(0),
        facility_filter: RwSignal::new(None),
        owner_filter: RwSignal::new(None),
        employee_filter: RwSignal::new(None),
        from_date: RwSignal::new(String::new()),
        until_date: RwSignal::new(String::new()),
        include_history: RwSignal::new(false),
        action: RwSignal::new(None),
        action_note: RwSignal::new(String::new()),
        action_quantity: RwSignal::new(String::new()),
        action_exception_seconds: RwSignal::new("0".into()),
        action_exception_reason: RwSignal::new(None),
        action_exception_note: RwSignal::new(String::new()),
        action_equipment_status: RwSignal::new(EquipmentStatus::OutOfService),
        pending: RwSignal::new(false),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };

    Effect::new(move |_| load_workspace(signals));

    let apply = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        load_workspace(signals);
    };
    let refresh = move |_| load_workspace(signals);
    let retry = move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    };

    view! {
        <section class="labor-workspace">
            <form class="labor-toolbar" on:submit=apply>
                <div class="labor-heading">
                    <Icon icon=UiIcon::Employees/>
                    <div>
                        <h1>"Labor control"</h1>
                        <span>"Attendance, execution, qualifications, and equipment readiness"</span>
                    </div>
                </div>
                <label>
                    <span>"Facility"</span>
                    <select
                        prop:value=move || option_value(signals.facility_filter.get())
                        on:change=move |event| signals.facility_filter.set(parse_id(&event_target_value(&event)))
                    >
                        <option value="">"All facilities"</option>
                        {access.with_value(|value| scope_options(&value.facilities))}
                    </select>
                </label>
                <label>
                    <span>"Client"</span>
                    <select
                        prop:value=move || option_value(signals.owner_filter.get())
                        on:change=move |event| signals.owner_filter.set(parse_id(&event_target_value(&event)))
                    >
                        <option value="">"All clients"</option>
                        {access.with_value(|value| scope_options(&value.inventory_owners))}
                    </select>
                </label>
                <label>
                    <span>"Employee"</span>
                    <select
                        prop:value=move || option_value(signals.employee_filter.get())
                        on:change=move |event| signals.employee_filter.set(parse_id(&event_target_value(&event)))
                    >
                        <option value="">"All employees"</option>
                        {move || employee_options(&signals.workspace.get())}
                    </select>
                </label>
                <label>
                    <span>"From (UTC)"</span>
                    <input
                        type="date"
                        prop:value=move || signals.from_date.get()
                        on:input=move |event| signals.from_date.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Until, exclusive (UTC)"</span>
                    <input
                        type="date"
                        prop:value=move || signals.until_date.get()
                        on:input=move |event| signals.until_date.set(event_target_value(&event))
                    />
                </label>
                <label class="labor-history-toggle">
                    <input
                        type="checkbox"
                        prop:checked=move || signals.include_history.get()
                        on:change=move |event| signals.include_history.set(event_target_checked(&event))
                    />
                    <span>"Include history"</span>
                </label>
                <button class="button secondary-action compact" type="submit" disabled=move || signals.loading.get()>
                    "Apply"
                </button>
                <button
                    class="icon-button"
                    type="button"
                    title="Refresh labor workspace"
                    aria-label="Refresh labor workspace"
                    disabled=move || signals.loading.get()
                    on:click=refresh
                >
                    <Icon icon=UiIcon::Refresh/>
                </button>
                <small class="labor-window-help">"Blank dates use the last 24 hours. Maximum range: 31 days."</small>
            </form>

            {move || {
                if signals.loading.get() && !signals.loaded.get() {
                    view! {
                        <div class="labor-state">
                            <span class="loading-line"></span>
                            <h2>"Loading the labor floor"</h2>
                            <p>"Reconciling attendance, activity, standards, and equipment."</p>
                        </div>
                    }.into_any()
                } else if let Some(message) = signals.load_error.get() {
                    view! {
                        <div class="labor-state error" role="alert">
                            <h2>"Labor workspace unavailable"</h2>
                            <p>{message}</p>
                            <button class="button secondary-action" type="button" on:click=refresh>"Try again"</button>
                        </div>
                    }.into_any()
                } else {
                    labor_body(
                        signals,
                        access,
                        can_execute,
                        can_configure,
                        can_manage_equipment,
                        can_certify,
                        can_supervise,
                    )
                }
            }}

            {move || signals.action.get().map(|target| action_drawer(signals, target))}

            <Show when=move || signals.command_error.get().is_some() && signals.action.get().is_none()>
                <div class="labor-command-error" role="alert">
                    <span>{move || signals.command_error.get().unwrap_or_default()}</span>
                    <Show when=move || signals.retry.get().is_some()>
                        <button
                            class="button secondary-action compact"
                            type="button"
                            disabled=move || signals.pending.get()
                            on:click=retry
                        >
                            "Retry exact command"
                        </button>
                    </Show>
                    <button class="text-button" type="button" on:click=move |_| signals.command_error.set(None)>
                        "Dismiss"
                    </button>
                </div>
            </Show>
        </section>
    }
}

fn labor_body(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_execute: bool,
    can_configure: bool,
    can_manage_equipment: bool,
    can_certify: bool,
    can_supervise: bool,
) -> AnyView {
    let reload = Callback::new(move |_| load_workspace(signals));
    view! {
        <div class="labor-body">
            <LaborCommandCenter
                workspace=signals.workspace
                access=access.get_value()
                initial_facility_id=signals.facility_filter.get_untracked()
                initial_owner_id=signals.owner_filter.get_untracked()
                can_execute
                can_configure
                can_manage_equipment
                can_certify
                can_supervise
                on_success=reload
                on_unauthorized=signals.on_unauthorized
            />
            {move || labor_metrics(&signals.workspace.get())}
            <div class="labor-live-grid">
                {attendance_panel(signals, access, can_execute)}
                {activity_panel(signals, access, can_execute)}
            </div>
            {move || summary_panel(&signals.workspace.get())}
            <div class="labor-reference-grid">
                {move || qualification_panel(signals, access, can_certify)}
                {move || equipment_panel(signals, access, can_manage_equipment)}
                {move || standards_panel(&signals.workspace.get(), access)}
            </div>
        </div>
    }
    .into_any()
}

fn labor_metrics(workspace: &LaborWorkspaceResponse) -> AnyView {
    let metrics = calculate_metrics(workspace);
    view! {
        <dl class="labor-metrics">
            <div><dt>"On clock"</dt><dd>{metrics.clocked_in}</dd><small>"open intervals"</small></div>
            <div><dt>"Active work"</dt><dd>{metrics.active_activities}</dd><small>"in progress"</small></div>
            <div><dt>"Utilization"</dt><dd>{percent(metrics.utilization_basis_points)}</dd><small>"direct / paid"</small></div>
            <div><dt>"Efficiency"</dt><dd>{percent(metrics.efficiency_basis_points)}</dd><small>"weighted standard"</small></div>
            <div class:attention={metrics.unavailable_equipment > 0}><dt>"Equipment down"</dt><dd>{metrics.unavailable_equipment}</dd><small>"out of service"</small></div>
            <div><dt>"Certified"</dt><dd>{metrics.active_certifications}</dd><small>"active records"</small></div>
        </dl>
    }
    .into_any()
}

fn attendance_panel(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_execute: bool,
) -> AnyView {
    view! {
        <section class="labor-panel labor-live-panel">
            <header><div><h2>"Attendance"</h2><span>{move || format!("{} intervals", signals.workspace.get().attendance.len())}</span></div></header>
            <div class="labor-table-scroll">
                <table>
                    <thead><tr><th>"Employee"</th><th>"Facility"</th><th>"State"</th><th>"Clock in"</th><th>"Paid"</th><th></th></tr></thead>
                    <tbody>
                        {move || {
                            let rows = signals.workspace.get().attendance;
                            if rows.is_empty() {
                                view! { <tr class="empty-row"><td colspan="6">"No attendance in this window."</td></tr> }.into_any()
                            } else {
                                rows.into_iter().map(|row| {
                                    let is_open = row.status == AttendanceStatus::Open;
                                    let target = ActionTarget::ClockOut(row.clone());
                                    let facility = scope_name(&access.get_value().facilities, row.facility_id);
                                    view! {
                                        <tr>
                                            <td><strong>{row.employee_name}</strong><small>{format!("Employee #{}", row.employee_id)}</small></td>
                                            <td>{facility}</td>
                                            <td><span class=attendance_status_class(row.status)>{attendance_status_label(row.status)}</span></td>
                                            <td>{short_timestamp(&row.effective_clocked_in_at)}</td>
                                            <td>{duration(row.effective_paid_seconds)}</td>
                                            <td>{(can_execute && is_open).then(|| view! { <button class="text-button" type="button" on:click=move |_| open_action(signals, target.clone())>"Clock out"</button> })}</td>
                                        </tr>
                                    }
                                }).collect_view().into_any()
                            }
                        }}
                    </tbody>
                </table>
            </div>
        </section>
    }
    .into_any()
}

fn activity_panel(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_execute: bool,
) -> AnyView {
    view! {
        <section class="labor-panel labor-live-panel">
            <header><div><h2>"Labor activity"</h2><span>{move || format!("{} records", signals.workspace.get().activities.len())}</span></div></header>
            <div class="labor-table-scroll">
                <table>
                    <thead><tr><th>"Employee / activity"</th><th>"Client"</th><th>"State"</th><th>"Started"</th><th>"Qty / actual"</th><th></th></tr></thead>
                    <tbody>
                        {move || {
                            let rows = signals.workspace.get().activities;
                            if rows.is_empty() {
                                view! { <tr class="empty-row"><td colspan="6">"No labor activity in this window."</td></tr> }.into_any()
                            } else {
                                rows.into_iter().map(|row| {
                                    let is_active = row.status == LaborActivityStatus::Active;
                                    let complete_target = ActionTarget::Complete(row.clone());
                                    let cancel_target = ActionTarget::Cancel(row.clone());
                                    let owner = row.inventory_owner_id.map_or_else(
                                        || "Internal".to_owned(),
                                        |id| scope_name(&access.get_value().inventory_owners, id),
                                    );
                                    view! {
                                        <tr>
                                            <td><strong>{row.employee_name.clone()}</strong><small>{format!("{} · {}", activity_kind_label(row.activity_kind), reference_label(&row))}</small><small class="labor-explain">{activity_requirements(&row)}</small></td>
                                            <td>{owner}</td>
                                            <td><span class=activity_status_class(row.status)>{activity_status_label(row.status)}</span></td>
                                            <td>{short_timestamp(&row.effective_started_at)}</td>
                                            <td>{quantity_actual(&row)}</td>
                                            <td>{(can_execute && is_active).then(|| view! {
                                                <div class="labor-row-actions">
                                                    <button class="text-button" type="button" on:click=move |_| open_action(signals, complete_target.clone())>"Complete"</button>
                                                    <button class="text-button danger" type="button" on:click=move |_| open_action(signals, cancel_target.clone())>"Cancel"</button>
                                                </div>
                                            })}</td>
                                        </tr>
                                    }
                                }).collect_view().into_any()
                            }
                        }}
                    </tbody>
                </table>
            </div>
        </section>
    }
    .into_any()
}

fn summary_panel(workspace: &LaborWorkspaceResponse) -> AnyView {
    view! {
        <section class="labor-panel labor-summary-panel">
            <header><div><h2>"Performance by employee"</h2><span>{format!("{} employees", workspace.summaries.len())}</span></div></header>
            <div class="labor-table-scroll">
                <table>
                    <thead><tr><th>"Employee"</th><th>"Paid"</th><th>"Direct"</th><th>"Indirect"</th><th>"Exceptions"</th><th>"Expected"</th><th>"Utilization"</th><th>"Efficiency"</th></tr></thead>
                    <tbody>
                        {if workspace.summaries.is_empty() {
                            view! { <tr class="empty-row"><td colspan="8">"No completed labor is attributable to this reporting window."</td></tr> }.into_any()
                        } else {
                            workspace.summaries.iter().map(|row| view! {
                                <tr>
                                    <td><strong>{row.employee_name.clone()}</strong><small>{format!("Employee #{}", row.employee_id)}</small></td>
                                    <td>{duration(Some(row.paid_seconds))}</td>
                                    <td>{duration(Some(row.direct_seconds))}</td>
                                    <td>{duration(Some(row.indirect_seconds))}</td>
                                    <td>{duration(Some(row.exception_seconds))}</td>
                                    <td>{duration(Some(row.expected_seconds))}</td>
                                    <td>{percent(row.utilization_basis_points)}</td>
                                    <td>{percent(row.efficiency_basis_points)}</td>
                                </tr>
                            }).collect_view().into_any()
                        }}
                    </tbody>
                </table>
            </div>
        </section>
    }
    .into_any()
}

fn qualification_panel(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_certify: bool,
) -> AnyView {
    let workspace = signals.workspace.get();
    view! {
        <section class="labor-panel labor-reference-panel">
            <header><div><h2>"Skills & certifications"</h2><span>{format!("{} skills · {} certificates", workspace.skills.len(), workspace.certifications.len())}</span></div></header>
            <div class="labor-subtable">
                <h3>"Skill catalog"</h3>
                {if workspace.skills.is_empty() {
                    view! { <p class="labor-inline-empty">"No skills configured."</p> }.into_any()
                } else {
                    view! { <table><thead><tr><th>"Code"</th><th>"Skill"</th><th>"Requirement"</th></tr></thead><tbody>{workspace.skills.into_iter().map(|row| view! {
                        <tr><td><code>{row.code}</code></td><td>{row.name}</td><td>{if row.certification_required { "Certificate" } else { "Training" }}</td></tr>
                    }).collect_view()}</tbody></table> }.into_any()
                }}
            </div>
            <div class="labor-subtable">
                <h3>"Employee certifications"</h3>
                {if workspace.certifications.is_empty() {
                    view! { <p class="labor-inline-empty">"No certifications in scope."</p> }.into_any()
                } else {
                    view! { <table><thead><tr><th>"Employee"</th><th>"Skill / facility"</th><th>"Expires"</th><th></th></tr></thead><tbody>{workspace.certifications.into_iter().map(|row| {
                        let active = row.revoked_at.is_none();
                        let target = ActionTarget::Revoke(row.clone());
                        let facility = scope_name(&access.get_value().facilities, row.facility_id);
                        view! { <tr><td><strong>{row.employee_name}</strong></td><td><code>{row.skill_code}</code><small>{facility}</small></td><td>{row.expires_at.as_deref().map(short_date).unwrap_or_else(|| "No expiry".into())}</td><td>{(can_certify && active).then(|| view! { <button class="text-button danger" type="button" on:click=move |_| open_action(signals, target.clone())>"Revoke"</button> })}</td></tr> }
                    }).collect_view()}</tbody></table> }.into_any()
                }}
            </div>
        </section>
    }.into_any()
}

fn equipment_panel(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_manage_equipment: bool,
) -> AnyView {
    let workspace = signals.workspace.get();
    view! {
        <section class="labor-panel labor-reference-panel">
            <header><div><h2>"Equipment readiness"</h2><span>{format!("{} assets · {} classes", workspace.equipment_assets.len(), workspace.equipment_classes.len())}</span></div></header>
            <div class="labor-subtable">
                {if workspace.equipment_assets.is_empty() {
                    view! { <p class="labor-inline-empty">"No equipment assets in scope."</p> }.into_any()
                } else {
                    view! { <table><thead><tr><th>"Asset"</th><th>"Facility / class"</th><th>"State"</th><th></th></tr></thead><tbody>{workspace.equipment_assets.into_iter().map(|row| {
                        let mutable = row.status != EquipmentStatus::Retired;
                        let target = ActionTarget::Equipment(row.clone());
                        let facility = scope_name(&access.get_value().facilities, row.facility_id);
                        view! { <tr><td><strong>{row.equipment_number}</strong><small>{row.name}</small></td><td>{facility}<small>{row.equipment_class_code}</small></td><td><span class=equipment_status_class(row.status)>{equipment_status_label(row.status)}</span></td><td>{(can_manage_equipment && mutable).then(|| view! { <button class="text-button" type="button" on:click=move |_| open_action(signals, target.clone())>"Change"</button> })}</td></tr> }
                    }).collect_view()}</tbody></table> }.into_any()
                }}
            </div>
            <div class="labor-class-key">
                {workspace.equipment_classes.into_iter().map(|row| view! { <span><code>{row.code}</code>{row.name}</span> }).collect_view()}
            </div>
        </section>
    }.into_any()
}

fn standards_panel(
    workspace: &LaborWorkspaceResponse,
    access: StoredValue<AccessScopeWorkspace>,
) -> AnyView {
    view! {
        <section class="labor-panel labor-reference-panel labor-standards-panel">
            <header><div><h2>"Labor standards"</h2><span>{format!("{} effective definitions", workspace.standards.len())}</span></div></header>
            <div class="labor-table-scroll">
                <table>
                    <thead><tr><th>"Code / standard"</th><th>"Facility"</th><th>"Client"</th><th>"Activity"</th><th>"Basis"</th><th>"Setup"</th><th>"Rate"</th><th>"Effective"</th></tr></thead>
                    <tbody>
                        {if workspace.standards.is_empty() {
                            view! { <tr class="empty-row"><td colspan="8">"No labor standards in scope."</td></tr> }.into_any()
                        } else {
                            workspace.standards.iter().map(|row| {
                                let facility = scope_name(&access.get_value().facilities, row.facility_id);
                                let owner = row.inventory_owner_id.map_or_else(|| "Tenant default".into(), |id| scope_name(&access.get_value().inventory_owners, id));
                                view! { <tr><td><code>{row.code.clone()}</code><small>{row.name.clone()}</small></td><td>{facility}</td><td>{owner}</td><td>{activity_kind_label(row.activity_kind)}</td><td>{quantity_basis_label(row.quantity_basis)}</td><td>{duration(Some(row.setup_seconds))}</td><td>{format!("{}s / {}", row.seconds_per_unit, quantity_basis_label(row.quantity_basis).to_lowercase())}</td><td>{short_date(&row.effective_from)}</td></tr> }
                            }).collect_view().into_any()
                        }}
                    </tbody>
                </table>
            </div>
        </section>
    }.into_any()
}

fn action_drawer(signals: Signals, target: ActionTarget) -> AnyView {
    let heading = action_heading(&target);
    let context = action_context(&target);
    let primary = action_primary_label(&target);
    let target_for_submit = target.clone();
    view! {
        <div class="labor-drawer-backdrop" role="presentation" on:click=move |_| close_action(signals)>
            <aside class="labor-drawer" role="dialog" aria-modal="true" aria-label=heading on:click=move |event| event.stop_propagation()>
                <header>
                    <div><span>"Labor action"</span><h2>{heading}</h2><p>{context}</p></div>
                    <button class="icon-button" type="button" aria-label="Close action" on:click=move |_| close_action(signals)><Icon icon=UiIcon::Close/></button>
                </header>
                <form on:submit=move |event| {
                    event.prevent_default();
                    match build_command(signals, &target_for_submit) {
                        Ok(command) => dispatch(signals, command),
                        Err(message) => signals.command_error.set(Some(message)),
                    }
                }>
                    {action_fields(signals, &target)}
                    <label class="labor-note-field">
                        <span>{if matches!(target, ActionTarget::ClockOut(_)) { "Note (optional)" } else { "Action note" }}</span>
                        <textarea
                            maxlength="1000"
                            rows="4"
                            prop:value=move || signals.action_note.get()
                            on:input=move |event| signals.action_note.set(event_target_value(&event))
                        ></textarea>
                    </label>
                    <Show when=move || signals.command_error.get().is_some()>
                        <div class="labor-drawer-error" role="alert">{move || signals.command_error.get().unwrap_or_default()}</div>
                    </Show>
                    <footer>
                        <button class="button secondary-action" type="button" disabled=move || signals.pending.get() on:click=move |_| close_action(signals)>"Cancel"</button>
                        <Show when=move || signals.retry.get().is_some()>
                            <button class="button secondary-action" type="button" disabled=move || signals.pending.get() on:click=move |_| {
                                if let Some(command) = signals.retry.get_untracked() { dispatch(signals, command); }
                            }>"Retry exact command"</button>
                        </Show>
                        <button class="button primary-action" type="submit" disabled=move || signals.pending.get()>{move || if signals.pending.get() { "Working…" } else { primary }}</button>
                    </footer>
                </form>
            </aside>
        </div>
    }.into_any()
}

fn action_fields(signals: Signals, target: &ActionTarget) -> AnyView {
    match target {
        ActionTarget::Complete(activity) => {
            let quantity_required = is_direct(activity.activity_kind);
            view! {
                <div class="labor-action-grid">
                    <label><span>{if quantity_required { "Completed quantity" } else { "Quantity (optional)" }}</span><input type="number" min="0" required=quantity_required prop:value=move || signals.action_quantity.get() on:input=move |event| signals.action_quantity.set(event_target_value(&event))/></label>
                    <label><span>"Exception seconds"</span><input type="number" min="0" required prop:value=move || signals.action_exception_seconds.get() on:input=move |event| signals.action_exception_seconds.set(event_target_value(&event))/></label>
                    <label><span>"Exception reason"</span><select prop:value=move || exception_reason_value(signals.action_exception_reason.get()) on:change=move |event| signals.action_exception_reason.set(parse_exception_reason(&event_target_value(&event)))><option value="">"No exception"</option><option value="equipment">"Equipment"</option><option value="congestion">"Congestion"</option><option value="inventory">"Inventory"</option><option value="quality">"Quality"</option><option value="safety">"Safety"</option><option value="system">"System"</option><option value="training">"Training"</option><option value="personal">"Personal"</option><option value="other">"Other"</option></select></label>
                    <label><span>"Exception detail"</span><input maxlength="1000" prop:value=move || signals.action_exception_note.get() on:input=move |event| signals.action_exception_note.set(event_target_value(&event))/></label>
                </div>
            }.into_any()
        }
        ActionTarget::Equipment(asset) => view! {
            <label><span>"New equipment status"</span><select prop:value=move || equipment_status_value(signals.action_equipment_status.get()) on:change=move |event| signals.action_equipment_status.set(parse_equipment_status(&event_target_value(&event)))>
                <option value="available" disabled=asset.status == EquipmentStatus::Available>"Available"</option>
                <option value="out_of_service" disabled=asset.status == EquipmentStatus::OutOfService>"Out of service"</option>
                <option value="retired">"Retired"</option>
            </select></label>
        }.into_any(),
        ActionTarget::ClockOut(_) | ActionTarget::Cancel(_) | ActionTarget::Revoke(_) => ().into_any(),
    }
}

fn open_action(signals: Signals, target: ActionTarget) {
    signals.command_error.set(None);
    signals.retry.set(None);
    signals.action_note.set(String::new());
    signals.action_quantity.set(String::new());
    signals.action_exception_seconds.set("0".into());
    signals.action_exception_reason.set(None);
    signals.action_exception_note.set(String::new());
    let equipment_status = match &target {
        ActionTarget::Equipment(asset) if asset.status == EquipmentStatus::OutOfService => {
            EquipmentStatus::Available
        }
        ActionTarget::Equipment(_) => EquipmentStatus::OutOfService,
        _ => EquipmentStatus::OutOfService,
    };
    signals.action_equipment_status.set(equipment_status);
    signals.action.set(Some(target));
}

fn close_action(signals: Signals) {
    if !signals.pending.get_untracked() {
        signals.action.set(None);
        signals.command_error.set(None);
        signals.retry.set(None);
    }
}

fn build_command(signals: Signals, target: &ActionTarget) -> Result<PendingCommand, String> {
    let key = api::new_idempotency_key();
    let note = nonblank(signals.action_note.get_untracked());
    match target {
        ActionTarget::ClockOut(row) => Ok(PendingCommand::ClockOut(
            row.attendance_interval_id,
            ClockOutRequest {
                expected_revision: row.revision,
                note,
            },
            key,
        )),
        ActionTarget::Complete(row) => {
            let quantity =
                optional_nonnegative(&signals.action_quantity.get_untracked(), "quantity")?;
            if is_direct(row.activity_kind) && quantity.is_none() {
                return Err("Enter the completed quantity for direct work.".into());
            }
            let exception_seconds = required_nonnegative(
                &signals.action_exception_seconds.get_untracked(),
                "exception seconds",
            )?;
            let (exception_reason, exception_note) = completion_exception(
                exception_seconds,
                signals.action_exception_reason.get_untracked(),
                signals.action_exception_note.get_untracked(),
            )?;
            Ok(PendingCommand::Complete(
                row.labor_activity_id,
                CompleteLaborActivityRequest {
                    expected_revision: row.revision,
                    quantity,
                    exception_seconds,
                    exception_reason,
                    exception_note,
                    note,
                },
                key,
            ))
        }
        ActionTarget::Cancel(row) => Ok(PendingCommand::Cancel(
            row.labor_activity_id,
            CancelLaborActivityRequest {
                expected_revision: row.revision,
                note: required_note(note)?,
            },
            key,
        )),
        ActionTarget::Equipment(row) => Ok(PendingCommand::Equipment(
            row.equipment_asset_id,
            ChangeEquipmentStatusRequest {
                expected_revision: row.revision,
                status: signals.action_equipment_status.get_untracked(),
                note: required_note(note)?,
            },
            key,
        )),
        ActionTarget::Revoke(row) => Ok(PendingCommand::Revoke(
            row.certification_id,
            RevokeEmployeeCertificationRequest {
                expected_revision: row.revision,
                note: required_note(note)?,
            },
            key,
        )),
    }
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
            PendingCommand::ClockOut(id, request, key) => {
                api::clock_out_labor(*id, request, key).await.map(|_| ())
            }
            PendingCommand::Complete(id, request, key) => {
                api::complete_labor_activity(*id, request, key)
                    .await
                    .map(|_| ())
            }
            PendingCommand::Cancel(id, request, key) => {
                api::cancel_labor_activity(*id, request, key)
                    .await
                    .map(|_| ())
            }
            PendingCommand::Equipment(id, request, key) => {
                api::change_labor_equipment_status(*id, request, key)
                    .await
                    .map(|_| ())
            }
            PendingCommand::Revoke(id, request, key) => {
                api::revoke_labor_certification(*id, request, key)
                    .await
                    .map(|_| ())
            }
        };
        signals.pending.set(false);
        match result {
            Ok(()) => {
                signals.retry.set(None);
                signals.action.set(None);
                signals.toasts.success("Labor workspace updated.");
                load_workspace(signals);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None);
                }
                signals.toasts.error(error.message.clone());
                signals.command_error.set(Some(error.message));
            }
        }
    });
}

fn load_workspace(signals: Signals) {
    let from = date_bound(&signals.from_date.get_untracked());
    let until = date_bound(&signals.until_date.get_untracked());
    if let (Some(from), Some(until)) = (&from, &until) {
        if from >= until {
            signals.load_error.set(Some(
                "The from date must be before the exclusive until date.".into(),
            ));
            signals.loading.set(false);
            return;
        }
    }
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    signals.load_error.set(None);
    let filters = api::LaborFilters {
        facility_id: signals.facility_filter.get_untracked(),
        inventory_owner_id: signals.owner_filter.get_untracked(),
        employee_id: signals.employee_filter.get_untracked(),
        from,
        until,
        include_history: signals.include_history.get_untracked(),
    };
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, generation, filters);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::labor_workspace(filters).await {
            Ok(workspace) if signals.generation.get_untracked() == generation => {
                signals.workspace.set(workspace);
                signals.loaded.set(true);
                signals.load_error.set(None);
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.load_error.set(Some(error.message)),
        }
        if signals.generation.get_untracked() == generation {
            signals.loading.set(false);
        }
    });
}

#[derive(Debug, PartialEq, Eq)]
struct LaborMetrics {
    clocked_in: usize,
    active_activities: usize,
    utilization_basis_points: Option<i64>,
    efficiency_basis_points: Option<i64>,
    unavailable_equipment: usize,
    active_certifications: usize,
}

fn calculate_metrics(workspace: &LaborWorkspaceResponse) -> LaborMetrics {
    let paid_seconds: i64 = workspace.summaries.iter().map(|row| row.paid_seconds).sum();
    let direct_seconds: i64 = workspace
        .summaries
        .iter()
        .map(|row| row.direct_seconds)
        .sum();
    let (efficiency_sum, efficiency_weight) = workspace
        .summaries
        .iter()
        .filter_map(|row| {
            row.efficiency_basis_points
                .map(|value| (value, row.direct_seconds.max(1)))
        })
        .fold((0_i128, 0_i128), |(sum, weight), (value, row_weight)| {
            (
                sum + i128::from(value) * i128::from(row_weight),
                weight + i128::from(row_weight),
            )
        });
    LaborMetrics {
        clocked_in: workspace
            .attendance
            .iter()
            .filter(|row| row.status == AttendanceStatus::Open)
            .count(),
        active_activities: workspace
            .activities
            .iter()
            .filter(|row| row.status == LaborActivityStatus::Active)
            .count(),
        utilization_basis_points: ratio_basis_points(direct_seconds, paid_seconds),
        efficiency_basis_points: (efficiency_weight > 0)
            .then(|| (efficiency_sum / efficiency_weight) as i64),
        unavailable_equipment: workspace
            .equipment_assets
            .iter()
            .filter(|row| row.status == EquipmentStatus::OutOfService)
            .count(),
        active_certifications: workspace
            .certifications
            .iter()
            .filter(|row| row.revoked_at.is_none())
            .count(),
    }
}

fn ratio_basis_points(numerator: i64, denominator: i64) -> Option<i64> {
    (denominator > 0).then(|| {
        let value = i128::from(numerator) * 10_000 / i128::from(denominator);
        value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
    })
}

fn empty_workspace() -> LaborWorkspaceResponse {
    LaborWorkspaceResponse {
        skills: Vec::new(),
        certifications: Vec::new(),
        equipment_classes: Vec::new(),
        equipment_assets: Vec::new(),
        standards: Vec::new(),
        attendance: Vec::new(),
        activities: Vec::new(),
        attendance_adjustments: Vec::new(),
        activity_adjustments: Vec::new(),
        summaries: Vec::new(),
    }
}

fn scope_options(values: &[AccessScopeResource]) -> AnyView {
    values
        .iter()
        .map(|item| view! { <option value=item.id>{item.name.clone()}</option> })
        .collect_view()
        .into_any()
}

fn employee_options(workspace: &LaborWorkspaceResponse) -> AnyView {
    let mut employees = BTreeMap::new();
    for row in &workspace.attendance {
        employees.insert(row.employee_id, row.employee_name.clone());
    }
    for row in &workspace.activities {
        employees.insert(row.employee_id, row.employee_name.clone());
    }
    for row in &workspace.certifications {
        employees.insert(row.employee_id, row.employee_name.clone());
    }
    for row in &workspace.summaries {
        employees.insert(row.employee_id, row.employee_name.clone());
    }
    employees
        .into_iter()
        .map(|(id, name)| view! { <option value=id>{name}</option> })
        .collect_view()
        .into_any()
}

fn scope_name(values: &[AccessScopeResource], id: i64) -> String {
    values
        .iter()
        .find(|item| item.id == id)
        .map_or_else(|| format!("#{id}"), |item| item.name.clone())
}

fn date_bound(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| format!("{value}T00:00:00Z"))
}

fn option_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}

fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok()
}

fn short_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

fn short_date(value: &str) -> String {
    value.get(..10).unwrap_or(value).to_owned()
}

fn duration(value: Option<i64>) -> String {
    let Some(seconds) = value else {
        return "—".into();
    };
    let seconds = seconds.max(0);
    format!("{}h {:02}m", seconds / 3_600, seconds % 3_600 / 60)
}

fn percent(value: Option<i64>) -> String {
    value.map_or_else(
        || "—".into(),
        |basis_points| format!("{:.1}%", basis_points as f64 / 100.0),
    )
}

fn quantity_actual(row: &LaborActivityResponse) -> String {
    let mut details = Vec::new();
    match (row.effective_quantity, row.reference_quantity) {
        (Some(completed), Some(reference)) => details.push(format!("{completed} / {reference}")),
        (Some(completed), None) => details.push(completed.to_string()),
        (None, Some(reference)) => details.push(format!("— / {reference}")),
        (None, None) => {}
    }
    if let Some(seconds) = row.effective_actual_seconds {
        details.push(duration(Some(seconds)));
    }
    if let Some(seconds) = row
        .effective_exception_seconds
        .filter(|seconds| *seconds > 0)
    {
        let reason = row
            .effective_exception_reason
            .map(exception_reason_label)
            .unwrap_or("Unclassified");
        details.push(format!("{} {reason} exception", duration(Some(seconds))));
    }
    if details.is_empty() {
        "—".into()
    } else {
        details.join(" · ")
    }
}

fn reference_label(row: &LaborActivityResponse) -> String {
    match (&row.reference_type, row.reference_id) {
        (Some(kind), Some(id)) => format!("{kind} #{id}"),
        (Some(kind), None) => kind.clone(),
        _ => format!("Activity #{}", row.labor_activity_id),
    }
}

fn activity_requirements(row: &LaborActivityResponse) -> String {
    let mut requirements = Vec::new();
    if let Some(standard_id) = row.labor_standard_id {
        requirements.push(format!("Standard #{standard_id}"));
    }
    if let Some(skill_id) = row.required_skill_id {
        requirements.push(row.required_skill_certification_id.map_or_else(
            || format!("Skill #{skill_id}"),
            |certification_id| format!("Skill #{skill_id} · cert #{certification_id}"),
        ));
    }
    if let Some(class_id) = row.required_equipment_class_id {
        requirements.push(format!("Equipment class #{class_id}"));
    }
    if let Some(skill_id) = row.equipment_required_skill_id {
        requirements.push(row.equipment_skill_certification_id.map_or_else(
            || format!("Equipment skill #{skill_id}"),
            |certification_id| format!("Equipment skill #{skill_id} · cert #{certification_id}"),
        ));
    }
    if requirements.is_empty() {
        "No qualification gate".into()
    } else {
        requirements.join(" · ")
    }
}

fn nonblank(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn required_note(note: Option<String>) -> Result<String, String> {
    note.ok_or_else(|| "Enter an action note for the audit trail.".into())
}

fn optional_nonnegative(value: &str, label: &str) -> Result<Option<i64>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = value
        .parse::<i64>()
        .map_err(|_| format!("Enter a valid {label}."))?;
    if parsed < 0 {
        return Err(format!("{label} cannot be negative."));
    }
    Ok(Some(parsed))
}

fn required_nonnegative(value: &str, label: &str) -> Result<i64, String> {
    optional_nonnegative(value, label)?.ok_or_else(|| format!("Enter {label}."))
}

fn completion_exception(
    exception_seconds: i64,
    reason: Option<LaborExceptionReason>,
    note: String,
) -> Result<(Option<LaborExceptionReason>, Option<String>), String> {
    if exception_seconds == 0 {
        return Ok((None, None));
    }
    let reason = reason.ok_or_else(|| "Select a reason for exception time.".to_owned())?;
    Ok((Some(reason), nonblank(note)))
}

fn action_heading(target: &ActionTarget) -> &'static str {
    match target {
        ActionTarget::ClockOut(_) => "Clock out employee",
        ActionTarget::Complete(_) => "Complete labor activity",
        ActionTarget::Cancel(_) => "Cancel labor activity",
        ActionTarget::Equipment(_) => "Change equipment status",
        ActionTarget::Revoke(_) => "Revoke certification",
    }
}

fn action_primary_label(target: &ActionTarget) -> &'static str {
    match target {
        ActionTarget::ClockOut(_) => "Clock out",
        ActionTarget::Complete(_) => "Complete activity",
        ActionTarget::Cancel(_) => "Cancel activity",
        ActionTarget::Equipment(_) => "Change status",
        ActionTarget::Revoke(_) => "Revoke certification",
    }
}

fn action_context(target: &ActionTarget) -> String {
    match target {
        ActionTarget::ClockOut(row) => format!(
            "{} · clocked in {}",
            row.employee_name,
            short_timestamp(&row.clocked_in_at)
        ),
        ActionTarget::Complete(row) | ActionTarget::Cancel(row) => format!(
            "{} · {} · activity #{}",
            row.employee_name,
            activity_kind_label(row.activity_kind),
            row.labor_activity_id
        ),
        ActionTarget::Equipment(row) => format!(
            "{} · {} · currently {}",
            row.equipment_number,
            row.name,
            equipment_status_label(row.status)
        ),
        ActionTarget::Revoke(row) => format!(
            "{} · {} · certification #{}",
            row.employee_name, row.skill_code, row.certification_id
        ),
    }
}

#[cfg(test)]
#[path = "labor_tests.rs"]
mod tests;
