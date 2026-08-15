use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AttendanceStatus, CertifyEmployeeRequest, ClockInRequest, ConfigureEquipmentClassRequest,
    ConfigureLaborSkillRequest, ConfigureLaborStandardRequest, CorrectAttendanceRequest,
    CorrectLaborActivityRequest, CreateEquipmentAssetRequest, LaborActivityKind,
    LaborActivityStatus, LaborCorrectionReason, LaborExceptionReason, LaborQuantityBasis,
    LaborReferenceCandidatePageRequest, LaborReferenceCandidateResponse,
    LaborRosterCandidateResponse, LaborRosterPageRequest, LaborWorkspaceResponse, OpaqueCursor,
    StartLaborActivityRequest,
};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};

use crate::api;
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone, Copy, PartialEq, Eq)]
enum FormKind {
    Skill,
    EquipmentClass,
    EquipmentAsset,
    Standard,
    Certification,
    ClockIn,
    StartActivity,
    AttendanceCorrection,
    ActivityCorrection,
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build dispatches labor form commands")
)]
enum SavedCommand {
    Skill(ConfigureLaborSkillRequest, String),
    EquipmentClass(ConfigureEquipmentClassRequest, String),
    EquipmentAsset(CreateEquipmentAssetRequest, String),
    Standard(ConfigureLaborStandardRequest, String),
    Certification(CertifyEmployeeRequest, String),
    ClockIn(ClockInRequest, String),
    StartActivity(StartLaborActivityRequest, String),
    AttendanceCorrection(i64, CorrectAttendanceRequest, String),
    ActivityCorrection(i64, CorrectLaborActivityRequest, String),
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build consumes labor command callbacks")
)]
struct FormSignals {
    form: RwSignal<FormKind>,
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    roster: RwSignal<Vec<LaborRosterCandidateResponse>>,
    roster_cursor: RwSignal<Option<OpaqueCursor>>,
    roster_loading: RwSignal<bool>,
    roster_error: RwSignal<Option<String>>,
    reference_candidates: RwSignal<Vec<LaborReferenceCandidateResponse>>,
    reference_cursor: RwSignal<Option<OpaqueCursor>>,
    reference_loading: RwSignal<bool>,
    employee_id: RwSignal<Option<i64>>,
    skill_id: RwSignal<Option<i64>>,
    equipment_class_id: RwSignal<Option<i64>>,
    labor_standard_id: RwSignal<Option<i64>>,
    equipment_asset_id: RwSignal<Option<i64>>,
    reference_id: RwSignal<Option<i64>>,
    attendance_target_id: RwSignal<Option<i64>>,
    activity_target_id: RwSignal<Option<i64>>,
    code: RwSignal<String>,
    name: RwSignal<String>,
    equipment_number: RwSignal<String>,
    certification_number: RwSignal<String>,
    note: RwSignal<String>,
    certification_required: RwSignal<bool>,
    activity_kind: RwSignal<LaborActivityKind>,
    quantity_basis: RwSignal<LaborQuantityBasis>,
    setup_seconds: RwSignal<String>,
    seconds_per_unit: RwSignal<String>,
    issued_at: RwSignal<String>,
    expires_at: RwSignal<String>,
    effective_from: RwSignal<String>,
    effective_until: RwSignal<String>,
    corrected_clocked_in_at: RwSignal<String>,
    corrected_clocked_out_at: RwSignal<String>,
    corrected_started_at: RwSignal<String>,
    corrected_completed_at: RwSignal<String>,
    corrected_quantity: RwSignal<String>,
    exception_seconds: RwSignal<String>,
    exception_reason: RwSignal<Option<LaborExceptionReason>>,
    exception_note: RwSignal<String>,
    correction_reason: RwSignal<LaborCorrectionReason>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    retry: RwSignal<Option<SavedCommand>>,
    roster_generation: RwSignal<u64>,
    reference_generation: RwSignal<u64>,
    on_success: Callback<()>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(super) fn LaborCommandCenter(
    workspace: RwSignal<LaborWorkspaceResponse>,
    access: AccessScopeWorkspace,
    initial_facility_id: Option<i64>,
    initial_owner_id: Option<i64>,
    can_execute: bool,
    can_configure: bool,
    can_manage_equipment: bool,
    can_certify: bool,
    can_supervise: bool,
    on_success: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let access = StoredValue::new(access);
    let signals = FormSignals {
        form: RwSignal::new(first_form(
            can_execute,
            can_configure,
            can_manage_equipment,
            can_certify,
            can_supervise,
        )),
        facility_id: RwSignal::new(initial_facility_id),
        owner_id: RwSignal::new(initial_owner_id),
        roster: RwSignal::new(Vec::new()),
        roster_cursor: RwSignal::new(None),
        roster_loading: RwSignal::new(false),
        roster_error: RwSignal::new(None),
        reference_candidates: RwSignal::new(Vec::new()),
        reference_cursor: RwSignal::new(None),
        reference_loading: RwSignal::new(false),
        employee_id: RwSignal::new(None),
        skill_id: RwSignal::new(None),
        equipment_class_id: RwSignal::new(None),
        labor_standard_id: RwSignal::new(None),
        equipment_asset_id: RwSignal::new(None),
        reference_id: RwSignal::new(None),
        attendance_target_id: RwSignal::new(None),
        activity_target_id: RwSignal::new(None),
        code: RwSignal::new(String::new()),
        name: RwSignal::new(String::new()),
        equipment_number: RwSignal::new(String::new()),
        certification_number: RwSignal::new(String::new()),
        note: RwSignal::new(String::new()),
        certification_required: RwSignal::new(false),
        activity_kind: RwSignal::new(LaborActivityKind::Receiving),
        quantity_basis: RwSignal::new(LaborQuantityBasis::Unit),
        setup_seconds: RwSignal::new("0".into()),
        seconds_per_unit: RwSignal::new(String::new()),
        issued_at: RwSignal::new(String::new()),
        expires_at: RwSignal::new(String::new()),
        effective_from: RwSignal::new(String::new()),
        effective_until: RwSignal::new(String::new()),
        corrected_clocked_in_at: RwSignal::new(String::new()),
        corrected_clocked_out_at: RwSignal::new(String::new()),
        corrected_started_at: RwSignal::new(String::new()),
        corrected_completed_at: RwSignal::new(String::new()),
        corrected_quantity: RwSignal::new(String::new()),
        exception_seconds: RwSignal::new("0".into()),
        exception_reason: RwSignal::new(None),
        exception_note: RwSignal::new(String::new()),
        correction_reason: RwSignal::new(LaborCorrectionReason::MissedPunch),
        pending: RwSignal::new(false),
        error: RwSignal::new(None),
        retry: RwSignal::new(None),
        roster_generation: RwSignal::new(0),
        reference_generation: RwSignal::new(0),
        on_success,
        on_unauthorized,
        toasts: use_toast_bus(),
    };

    Effect::new(move |_| {
        let facility_id = signals.facility_id.get();
        let owner_id = signals.owner_id.get();
        signals.employee_id.set(None);
        reset_references(signals);
        load_roster(signals, facility_id, owner_id, false);
    });

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
            return;
        }
        match build_command(signals, &workspace.get_untracked()) {
            Ok(command) => dispatch(signals, command),
            Err(message) => signals.error.set(Some(message)),
        }
    };
    let search_references = move |_| load_references(signals, false);
    let load_more_references = move |_| load_references(signals, true);
    let load_more_roster = move |_| {
        load_roster(
            signals,
            signals.facility_id.get_untracked(),
            signals.owner_id.get_untracked(),
            true,
        )
    };

    view! {
        <section class="labor-panel labor-command-center" aria-label="Labor operator commands">
            <header>
                <div><h2>"Operator workbench"</h2><span>"Scoped, eligibility-checked labor commands"</span></div>
                <span class="labor-command-permissions">{permission_summary(can_execute, can_configure, can_manage_equipment, can_certify, can_supervise)}</span>
            </header>
            <form on:submit=submit>
                <div class="labor-command-scope">
                    <label><span>"Operation"</span><select prop:value=move || form_value(signals.form.get()) on:change=move |event| {
                        signals.form.set(parse_form(&event_target_value(&event)));
                        signals.error.set(None);
                        signals.retry.set(None);
                    }>
                        {can_configure.then(|| view! { <option value="skill">"Configure skill"</option><option value="equipment_class">"Configure equipment class"</option><option value="standard">"Configure labor standard"</option> })}
                        {can_manage_equipment.then(|| view! { <option value="equipment_asset">"Create equipment asset"</option> })}
                        {can_certify.then(|| view! { <option value="certification">"Certify employee"</option> })}
                        {can_execute.then(|| view! { <option value="clock_in">"Clock in"</option><option value="start_activity">"Start activity"</option> })}
                        {can_supervise.then(|| view! { <option value="attendance_correction">"Correct attendance"</option><option value="activity_correction">"Correct activity"</option> })}
                    </select></label>
                    <label><span>"Facility scope"</span><select required prop:value=move || option_id(signals.facility_id.get()) on:change=move |event| {
                        signals.facility_id.set(parse_id(&event_target_value(&event)));
                        signals.retry.set(None);
                    }><option value="">"Select facility"</option>{scope_options(&access.get_value().facilities)}</select></label>
                    <label><span>"Client scope"</span><select prop:value=move || option_id(signals.owner_id.get()) on:change=move |event| {
                        signals.owner_id.set(parse_id(&event_target_value(&event)));
                        signals.retry.set(None);
                    }><option value="">"Tenant / facility shared"</option>{scope_options(&access.get_value().inventory_owners)}</select></label>
                    <div class="labor-roster-state">
                        {move || if signals.roster_loading.get() { "Loading eligible roster…".to_owned() }
                            else if let Some(error) = signals.roster_error.get() { error }
                            else { format!("{} scoped employee(s)", signals.roster.get().len()) }}
                        <Show when=move || signals.roster_cursor.get().is_some()>
                            <button type="button" class="text-button" disabled=move || signals.roster_loading.get() on:click=load_more_roster>"Load more"</button>
                        </Show>
                    </div>
                </div>

                {move || operation_fields(signals, workspace.get(), can_execute, can_supervise)}

                <Show when=move || signals.form.get() == FormKind::StartActivity && is_direct(signals.activity_kind.get())>
                    <div class="labor-reference-search">
                        <button type="button" class="button secondary-action compact" disabled=move || signals.reference_loading.get() on:click=search_references>
                            {move || if signals.reference_loading.get() { "Checking work…" } else { "Find executable work" }}
                        </button>
                        <span>"Candidates are filtered by tenant, facility, client, employee assignment, work state, lease, and quantity evidence."</span>
                        <Show when=move || signals.reference_cursor.get().is_some()>
                            <button type="button" class="text-button" on:click=load_more_references>"Load more"</button>
                        </Show>
                    </div>
                </Show>

                {move || eligibility_panel(signals, &workspace.get())}
                <Show when=move || signals.error.get().is_some()>
                    <div class="labor-form-error" role="alert">{move || signals.error.get().unwrap_or_default()}</div>
                </Show>
                <footer>
                    <Show when=move || signals.retry.get().is_some()>
                        <span class="labor-retry-evidence">"Outcome unknown: the saved request and idempotency key will be retried exactly."</span>
                    </Show>
                    <button class="button primary-action" type="submit" disabled=move || signals.pending.get()>
                        {move || if signals.pending.get() { "Submitting…" } else if signals.retry.get().is_some() { "Retry exact command" } else { submit_label(signals.form.get()) }}
                    </button>
                </footer>
            </form>
        </section>
    }
}

fn operation_fields(
    signals: FormSignals,
    workspace: LaborWorkspaceResponse,
    _can_execute: bool,
    _can_supervise: bool,
) -> AnyView {
    match signals.form.get() {
        FormKind::Skill => view! {
            <div class="labor-form-grid">
                {text_field("Skill code", signals.code, "PICK")}
                {text_field("Skill name", signals.name, "Order picking")}
                <label class="labor-check"><input type="checkbox" prop:checked=move || signals.certification_required.get() on:change=move |event| signals.certification_required.set(event_target_checked(&event))/><span>"Certification required"</span></label>
            </div>
        }.into_any(),
        FormKind::EquipmentClass => view! {
            <div class="labor-form-grid">
                {text_field("Class code", signals.code, "FORKLIFT")}
                {text_field("Class name", signals.name, "Counterbalance forklift")}
                <label><span>"Required skill"</span><select prop:value=move || option_id(signals.skill_id.get()) on:change=move |event| signals.skill_id.set(parse_id(&event_target_value(&event)))><option value="">"No skill gate"</option>{skill_options(&workspace)}</select></label>
            </div>
        }.into_any(),
        FormKind::EquipmentAsset => view! {
            <div class="labor-form-grid">
                <label><span>"Equipment class"</span><select required prop:value=move || option_id(signals.equipment_class_id.get()) on:change=move |event| signals.equipment_class_id.set(parse_id(&event_target_value(&event)))><option value="">"Select class"</option>{class_options(&workspace)}</select></label>
                {text_field("Equipment number", signals.equipment_number, "FL-017")}
                {text_field("Display name", signals.name, "Forklift 17")}
            </div>
        }.into_any(),
        FormKind::Standard => view! {
            <div class="labor-form-grid labor-form-grid-wide">
                {text_field("Standard code", signals.code, "PICK-EACH")}
                {text_field("Standard name", signals.name, "Each picking")}
                {activity_kind_select(signals)}
                {quantity_basis_select(signals)}
                {number_field("Setup seconds", signals.setup_seconds, "0")}
                {number_field("Seconds per unit", signals.seconds_per_unit, "8")}
                <label><span>"Required skill"</span><select prop:value=move || option_id(signals.skill_id.get()) on:change=move |event| signals.skill_id.set(parse_id(&event_target_value(&event)))><option value="">"No skill gate"</option>{skill_options(&workspace)}</select></label>
                <label><span>"Required equipment class"</span><select prop:value=move || option_id(signals.equipment_class_id.get()) on:change=move |event| signals.equipment_class_id.set(parse_id(&event_target_value(&event)))><option value="">"No equipment gate"</option>{class_options(&workspace)}</select></label>
                {datetime_field("Effective from (UTC)", signals.effective_from)}
                {datetime_field("Effective until (optional)", signals.effective_until)}
            </div>
        }.into_any(),
        FormKind::Certification => view! {
            <div class="labor-form-grid labor-form-grid-wide">
                {employee_select(signals, false, false)}
                <label><span>"Skill"</span><select required prop:value=move || option_id(signals.skill_id.get()) on:change=move |event| signals.skill_id.set(parse_id(&event_target_value(&event)))><option value="">"Select skill"</option>{skill_options(&workspace)}</select></label>
                {text_field("Certification number", signals.certification_number, "CERT-2026-001")}
                {datetime_field("Issued at (UTC)", signals.issued_at)}
                {datetime_field("Expires at (optional)", signals.expires_at)}
                {note_field(signals.note)}
            </div>
        }.into_any(),
        FormKind::ClockIn => view! {
            <div class="labor-form-grid">
                {employee_select(signals, true, false)}
                {note_field(signals.note)}
            </div>
        }.into_any(),
        FormKind::StartActivity => {
            let direct = is_direct(signals.activity_kind.get());
            view! {
                <div class="labor-form-grid labor-form-grid-wide">
                    {employee_select(signals, false, true)}
                    {activity_kind_select(signals)}
                    {direct.then(|| quantity_basis_select(signals))}
                    {direct.then(|| view! { <label><span>"Executable reference"</span><select required prop:value=move || option_id(signals.reference_id.get()) on:change=move |event| signals.reference_id.set(parse_id(&event_target_value(&event)))><option value="">"Search and select work"</option>{reference_options(signals.reference_candidates.get())}</select></label> })}
                    {direct.then(|| view! { <label><span>"Labor standard (optional)"</span><select prop:value=move || option_id(signals.labor_standard_id.get()) on:change=move |event| signals.labor_standard_id.set(parse_id(&event_target_value(&event)))><option value="">"No standard"</option>{standard_options(&workspace, signals)}</select></label> })}
                    <label><span>"Equipment (optional)"</span><select prop:value=move || option_id(signals.equipment_asset_id.get()) on:change=move |event| signals.equipment_asset_id.set(parse_id(&event_target_value(&event)))><option value="">"No equipment"</option>{asset_options(&workspace, signals.facility_id.get())}</select></label>
                    {note_field(signals.note)}
                </div>
            }.into_any()
        }
        FormKind::AttendanceCorrection => view! {
            <div class="labor-form-grid labor-form-grid-wide">
                <label><span>"Closed attendance interval"</span><select required prop:value=move || option_id(signals.attendance_target_id.get()) on:change=move |event| signals.attendance_target_id.set(parse_id(&event_target_value(&event)))><option value="">"Select interval"</option>{attendance_options(&workspace, signals.facility_id.get())}</select></label>
                {datetime_field("Corrected clock in (UTC)", signals.corrected_clocked_in_at)}
                {datetime_field("Corrected clock out (UTC)", signals.corrected_clocked_out_at)}
                {correction_reason_select(signals)}
                {note_field(signals.note)}
            </div>
        }.into_any(),
        FormKind::ActivityCorrection => view! {
            <div class="labor-form-grid labor-form-grid-wide">
                <label><span>"Completed labor activity"</span><select required prop:value=move || option_id(signals.activity_target_id.get()) on:change=move |event| signals.activity_target_id.set(parse_id(&event_target_value(&event)))><option value="">"Select activity"</option>{activity_options(&workspace, signals.facility_id.get(), signals.owner_id.get())}</select></label>
                {datetime_field("Corrected start (optional)", signals.corrected_started_at)}
                {datetime_field("Corrected completion (optional)", signals.corrected_completed_at)}
                {number_field("Corrected quantity", signals.corrected_quantity, "")}
                {number_field("Exception seconds", signals.exception_seconds, "0")}
                {exception_reason_select(signals)}
                {text_field("Exception evidence", signals.exception_note, "Delay or exception detail")}
                {correction_reason_select(signals)}
                {note_field(signals.note)}
            </div>
        }.into_any(),
    }
}

fn build_command(
    signals: FormSignals,
    workspace: &LaborWorkspaceResponse,
) -> Result<SavedCommand, String> {
    let key = api::new_idempotency_key();
    let facility_id = signals
        .facility_id
        .get_untracked()
        .ok_or_else(|| "Select a facility scope.".to_owned())?;
    let note = optional_text(signals.note.get_untracked());
    match signals.form.get_untracked() {
        FormKind::Skill => Ok(SavedCommand::Skill(
            ConfigureLaborSkillRequest {
                code: required_text(signals.code.get_untracked(), "skill code")?,
                name: required_text(signals.name.get_untracked(), "skill name")?,
                certification_required: signals.certification_required.get_untracked(),
            },
            key,
        )),
        FormKind::EquipmentClass => Ok(SavedCommand::EquipmentClass(
            ConfigureEquipmentClassRequest {
                code: required_text(signals.code.get_untracked(), "class code")?,
                name: required_text(signals.name.get_untracked(), "class name")?,
                required_skill_id: signals.skill_id.get_untracked(),
            },
            key,
        )),
        FormKind::EquipmentAsset => Ok(SavedCommand::EquipmentAsset(
            CreateEquipmentAssetRequest {
                facility_id,
                equipment_class_id: required_id(signals.equipment_class_id, "equipment class")?,
                equipment_number: required_text(
                    signals.equipment_number.get_untracked(),
                    "equipment number",
                )?,
                name: required_text(signals.name.get_untracked(), "equipment name")?,
            },
            key,
        )),
        FormKind::Standard => {
            let kind = signals.activity_kind.get_untracked();
            let basis = signals.quantity_basis.get_untracked();
            validate_kind_basis(kind, Some(basis), true)?;
            Ok(SavedCommand::Standard(
                ConfigureLaborStandardRequest {
                    facility_id,
                    inventory_owner_id: signals.owner_id.get_untracked(),
                    code: required_text(signals.code.get_untracked(), "standard code")?,
                    name: required_text(signals.name.get_untracked(), "standard name")?,
                    activity_kind: kind,
                    quantity_basis: basis,
                    setup_seconds: nonnegative(
                        signals.setup_seconds.get_untracked(),
                        "setup seconds",
                    )?,
                    seconds_per_unit: positive(
                        signals.seconds_per_unit.get_untracked(),
                        "seconds per unit",
                    )?,
                    required_skill_id: signals.skill_id.get_untracked(),
                    required_equipment_class_id: signals.equipment_class_id.get_untracked(),
                    effective_from: required_timestamp(
                        signals.effective_from.get_untracked(),
                        "effective from",
                    )?,
                    effective_until: optional_timestamp(signals.effective_until.get_untracked()),
                },
                key,
            ))
        }
        FormKind::Certification => Ok(SavedCommand::Certification(
            CertifyEmployeeRequest {
                employee_id: required_id(signals.employee_id, "employee")?,
                skill_id: required_id(signals.skill_id, "skill")?,
                facility_id,
                certification_number: optional_text(signals.certification_number.get_untracked()),
                issued_at: required_timestamp(signals.issued_at.get_untracked(), "issued at")?,
                expires_at: optional_timestamp(signals.expires_at.get_untracked()),
                note,
            },
            key,
        )),
        FormKind::ClockIn => {
            let employee_id = required_id(signals.employee_id, "eligible employee")?;
            require_roster_state(signals, employee_id, true, false)?;
            Ok(SavedCommand::ClockIn(
                ClockInRequest {
                    employee_id,
                    facility_id,
                    note,
                },
                key,
            ))
        }
        FormKind::StartActivity => {
            let employee_id = required_id(signals.employee_id, "eligible employee")?;
            let roster = require_roster_state(signals, employee_id, false, true)?;
            let attendance_interval_id = roster
                .attendance_interval_id
                .ok_or_else(|| "The employee has no open attendance interval.".to_owned())?;
            let kind = signals.activity_kind.get_untracked();
            let direct = is_direct(kind);
            let basis = direct.then(|| signals.quantity_basis.get_untracked());
            validate_kind_basis(kind, basis, direct)?;
            let selected_reference = if direct {
                let reference_id = required_id(signals.reference_id, "executable work reference")?;
                Some(
                    signals
                        .reference_candidates
                        .get_untracked()
                        .into_iter()
                        .find(|item| item.reference_id == reference_id)
                        .ok_or_else(|| {
                            "Select an executable reference returned by the eligibility check."
                                .to_owned()
                        })?,
                )
            } else {
                None
            };
            let owner_id = if direct {
                let owner = signals.owner_id.get_untracked();
                if owner.is_none() && kind != LaborActivityKind::CycleCount {
                    return Err("Select a client for this direct activity.".into());
                }
                owner
            } else {
                None
            };
            let standard_id = if direct {
                signals.labor_standard_id.get_untracked()
            } else {
                None
            };
            if let Some(standard_id) = standard_id {
                let valid = workspace.standards.iter().any(|standard| {
                    standard.labor_standard_id == standard_id
                        && standard.facility_id == facility_id
                        && standard.inventory_owner_id == owner_id
                        && standard.activity_kind == kind
                        && Some(standard.quantity_basis) == basis
                });
                if !valid {
                    return Err("The selected standard does not match the activity scope and quantity basis.".into());
                }
            }
            Ok(SavedCommand::StartActivity(
                StartLaborActivityRequest {
                    attendance_interval_id,
                    inventory_owner_id: owner_id,
                    activity_kind: kind,
                    quantity_basis: basis,
                    labor_standard_id: standard_id,
                    equipment_asset_id: signals.equipment_asset_id.get_untracked(),
                    reference_type: selected_reference
                        .as_ref()
                        .map(|item| item.reference_type.as_str().to_owned()),
                    reference_id: selected_reference.as_ref().map(|item| item.reference_id),
                    note,
                },
                key,
            ))
        }
        FormKind::AttendanceCorrection => {
            let id = required_id(signals.attendance_target_id, "closed attendance interval")?;
            let row = workspace
                .attendance
                .iter()
                .find(|row| {
                    row.attendance_interval_id == id && row.status == AttendanceStatus::Closed
                })
                .ok_or_else(|| "Select a visible closed attendance interval.".to_owned())?;
            Ok(SavedCommand::AttendanceCorrection(
                id,
                CorrectAttendanceRequest {
                    expected_revision: row.effective_revision,
                    corrected_clocked_in_at: required_timestamp(
                        signals.corrected_clocked_in_at.get_untracked(),
                        "corrected clock in",
                    )?,
                    corrected_clocked_out_at: required_timestamp(
                        signals.corrected_clocked_out_at.get_untracked(),
                        "corrected clock out",
                    )?,
                    reason: signals.correction_reason.get_untracked(),
                    note: required_text(signals.note.get_untracked(), "correction note")?,
                },
                key,
            ))
        }
        FormKind::ActivityCorrection => {
            let id = required_id(signals.activity_target_id, "completed labor activity")?;
            let row = workspace
                .activities
                .iter()
                .find(|row| {
                    row.labor_activity_id == id && row.status == LaborActivityStatus::Completed
                })
                .ok_or_else(|| "Select a visible completed labor activity.".to_owned())?;
            let quantity =
                optional_positive(signals.corrected_quantity.get_untracked(), "quantity")?;
            if is_direct(row.activity_kind) && quantity.is_none() {
                return Err("Enter the corrected quantity for direct labor.".into());
            }
            let exception_seconds = nonnegative(
                signals.exception_seconds.get_untracked(),
                "exception seconds",
            )?;
            let (exception_reason, exception_note) = exception_fields(
                exception_seconds,
                signals.exception_reason.get_untracked(),
                signals.exception_note.get_untracked(),
            )?;
            Ok(SavedCommand::ActivityCorrection(
                id,
                CorrectLaborActivityRequest {
                    expected_revision: row.effective_revision,
                    corrected_started_at: optional_timestamp(
                        signals.corrected_started_at.get_untracked(),
                    ),
                    corrected_completed_at: optional_timestamp(
                        signals.corrected_completed_at.get_untracked(),
                    ),
                    quantity,
                    exception_seconds,
                    exception_reason,
                    exception_note,
                    reason: signals.correction_reason.get_untracked(),
                    note: required_text(signals.note.get_untracked(), "correction note")?,
                },
                key,
            ))
        }
    }
}

fn dispatch(signals: FormSignals, command: SavedCommand) {
    if signals.pending.get_untracked() {
        return;
    }
    signals.pending.set(true);
    signals.error.set(None);
    signals.retry.set(Some(command.clone()));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, command);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = match &command {
            SavedCommand::Skill(request, key) => {
                api::configure_labor_skill(request, key).await.map(|_| ())
            }
            SavedCommand::EquipmentClass(request, key) => {
                api::configure_labor_equipment_class(request, key)
                    .await
                    .map(|_| ())
            }
            SavedCommand::EquipmentAsset(request, key) => {
                api::create_labor_equipment_asset(request, key)
                    .await
                    .map(|_| ())
            }
            SavedCommand::Standard(request, key) => api::configure_labor_standard(request, key)
                .await
                .map(|_| ()),
            SavedCommand::Certification(request, key) => {
                api::certify_labor_employee(request, key).await.map(|_| ())
            }
            SavedCommand::ClockIn(request, key) => {
                api::clock_in_labor(request, key).await.map(|_| ())
            }
            SavedCommand::StartActivity(request, key) => {
                api::start_labor_activity(request, key).await.map(|_| ())
            }
            SavedCommand::AttendanceCorrection(id, request, key) => {
                api::correct_labor_attendance(*id, request, key)
                    .await
                    .map(|_| ())
            }
            SavedCommand::ActivityCorrection(id, request, key) => {
                api::correct_labor_activity(*id, request, key)
                    .await
                    .map(|_| ())
            }
        };
        signals.pending.set(false);
        match result {
            Ok(()) => {
                signals.retry.set(None);
                signals.error.set(None);
                signals.toasts.success("Labor command accepted.");
                signals.on_success.run(());
                load_roster(
                    signals,
                    signals.facility_id.get_untracked(),
                    signals.owner_id.get_untracked(),
                    false,
                );
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None);
                }
                signals.toasts.error(error.message.clone());
                signals.error.set(Some(error.message));
            }
        }
    });
}

fn load_roster(
    signals: FormSignals,
    facility_id: Option<i64>,
    owner_id: Option<i64>,
    append: bool,
) {
    let Some(facility_id) = facility_id else {
        signals.roster.set(Vec::new());
        signals.roster_cursor.set(None);
        signals.roster_error.set(None);
        return;
    };
    let generation = signals.roster_generation.get_untracked().wrapping_add(1);
    signals.roster_generation.set(generation);
    signals.roster_loading.set(true);
    signals.roster_error.set(None);
    let request = LaborRosterPageRequest {
        facility_id,
        inventory_owner_id: owner_id,
        limit: Default::default(),
        cursor: append
            .then(|| signals.roster_cursor.get_untracked())
            .flatten(),
    };
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, request, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::labor_roster(&request).await {
            Ok(page) if signals.roster_generation.get_untracked() == generation => {
                if append {
                    signals.roster.update(|items| items.extend(page.items));
                } else {
                    signals.roster.set(page.items);
                }
                signals.roster_cursor.set(page.next_cursor);
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                signals.roster.set(Vec::new());
                signals.roster_cursor.set(None);
                signals.roster_error.set(Some(error.message));
            }
        }
        if signals.roster_generation.get_untracked() == generation {
            signals.roster_loading.set(false);
        }
    });
}

fn load_references(signals: FormSignals, append: bool) {
    let Some(facility_id) = signals.facility_id.get_untracked() else {
        signals
            .error
            .set(Some("Select a facility before searching for work.".into()));
        return;
    };
    let Some(employee_id) = signals.employee_id.get_untracked() else {
        signals.error.set(Some(
            "Select an eligible employee before searching for work.".into(),
        ));
        return;
    };
    let kind = signals.activity_kind.get_untracked();
    let basis = signals.quantity_basis.get_untracked();
    if let Err(message) = validate_kind_basis(kind, Some(basis), true) {
        signals.error.set(Some(message));
        return;
    }
    if signals.owner_id.get_untracked().is_none() && kind != LaborActivityKind::CycleCount {
        signals
            .error
            .set(Some("Select a client for this direct activity.".into()));
        return;
    }
    let generation = signals.reference_generation.get_untracked().wrapping_add(1);
    signals.reference_generation.set(generation);
    signals.reference_loading.set(true);
    signals.error.set(None);
    let request = LaborReferenceCandidatePageRequest {
        facility_id,
        inventory_owner_id: signals.owner_id.get_untracked(),
        employee_id,
        activity_kind: kind,
        quantity_basis: basis,
        limit: Default::default(),
        cursor: append
            .then(|| signals.reference_cursor.get_untracked())
            .flatten(),
    };
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, request, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::labor_reference_candidates(&request).await {
            Ok(page) if signals.reference_generation.get_untracked() == generation => {
                if append {
                    signals
                        .reference_candidates
                        .update(|items| items.extend(page.items));
                } else {
                    signals.reference_candidates.set(page.items);
                }
                signals.reference_cursor.set(page.next_cursor);
                if !append && signals.reference_candidates.get_untracked().is_empty() {
                    signals.error.set(Some("No executable references match the employee, scope, kind, basis, and current work state.".into()));
                }
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
        if signals.reference_generation.get_untracked() == generation {
            signals.reference_loading.set(false);
        }
    });
}

fn reset_references(signals: FormSignals) {
    signals.reference_candidates.set(Vec::new());
    signals.reference_cursor.set(None);
    signals.reference_id.set(None);
}

fn eligibility_panel(signals: FormSignals, workspace: &LaborWorkspaceResponse) -> AnyView {
    let employee = signals.employee_id.get().and_then(|id| {
        signals
            .roster
            .get()
            .into_iter()
            .find(|candidate| candidate.employee_id == id)
    });
    let reference = signals.reference_id.get().and_then(|id| {
        signals
            .reference_candidates
            .get()
            .into_iter()
            .find(|candidate| candidate.reference_id == id)
    });
    let standard = signals.labor_standard_id.get().and_then(|id| {
        workspace
            .standards
            .iter()
            .find(|candidate| candidate.labor_standard_id == id)
    });
    if employee.is_none() && reference.is_none() && standard.is_none() {
        return ().into_any();
    }
    view! {
        <aside class="labor-eligibility" aria-label="Eligibility and evidence">
            <strong>"Eligibility evidence"</strong>
            {employee.map(|item| view! { <div><b>{item.display_name}</b>{item.eligibility_evidence.into_iter().map(|line| view! { <span>{line}</span> }).collect_view()}</div> })}
            {reference.map(|item| view! { <div><b>{format!("{} · {} {}", item.display_label, item.canonical_quantity, basis_label(item.quantity_basis))}</b>{item.eligibility_evidence.into_iter().map(|line| view! { <span>{line}</span> }).collect_view()}</div> })}
            {standard.map(|item| view! { <div><b>{format!("Standard {} · {}", item.code, item.name)}</b><span>{format!("{} setup + {} seconds per {}", item.setup_seconds, item.seconds_per_unit, basis_label(item.quantity_basis))}</span><span>{format!("Skill {} · equipment class {}", optional_id(item.required_skill_id), optional_id(item.required_equipment_class_id))}</span></div> })}
        </aside>
    }.into_any()
}

fn first_form(
    can_execute: bool,
    can_configure: bool,
    can_manage_equipment: bool,
    can_certify: bool,
    can_supervise: bool,
) -> FormKind {
    if can_execute {
        FormKind::ClockIn
    } else if can_configure {
        FormKind::Skill
    } else if can_manage_equipment {
        FormKind::EquipmentAsset
    } else if can_certify {
        FormKind::Certification
    } else if can_supervise {
        FormKind::AttendanceCorrection
    } else {
        FormKind::Skill
    }
}

fn validate_kind_basis(
    kind: LaborActivityKind,
    basis: Option<LaborQuantityBasis>,
    direct_required: bool,
) -> Result<(), String> {
    if direct_required && !is_direct(kind) {
        return Err("Select a direct activity kind.".into());
    }
    if !is_direct(kind) {
        return basis.is_none().then_some(()).ok_or_else(|| {
            "Indirect activity cannot carry a quantity basis or direct reference.".into()
        });
    }
    let basis = basis.ok_or_else(|| "Direct activity requires a quantity basis.".to_owned())?;
    let valid = basis == LaborQuantityBasis::Task
        || matches!(
            kind,
            LaborActivityKind::Receiving
                | LaborActivityKind::Replenishment
                | LaborActivityKind::Picking
                | LaborActivityKind::Packing
                | LaborActivityKind::CrossDock
                | LaborActivityKind::CustomerReturn
                | LaborActivityKind::VendorReturn
                | LaborActivityKind::ValueAddedWork
        ) && matches!(basis, LaborQuantityBasis::Unit | LaborQuantityBasis::Line)
        || matches!(
            kind,
            LaborActivityKind::Putaway | LaborActivityKind::InventoryRelocation
        ) && matches!(
            basis,
            LaborQuantityBasis::Unit | LaborQuantityBasis::Line | LaborQuantityBasis::Container
        )
        || kind == LaborActivityKind::Shipping
            && matches!(
                basis,
                LaborQuantityBasis::Unit
                    | LaborQuantityBasis::Line
                    | LaborQuantityBasis::Container
                    | LaborQuantityBasis::WeightGram
            )
        || kind == LaborActivityKind::CycleCount && basis == LaborQuantityBasis::Line
        || kind == LaborActivityKind::Yard && basis == LaborQuantityBasis::Container;
    valid
        .then_some(())
        .ok_or_else(|| "The quantity basis is not supported by this activity kind.".into())
}

fn require_roster_state(
    signals: FormSignals,
    employee_id: i64,
    clock_in: bool,
    start: bool,
) -> Result<LaborRosterCandidateResponse, String> {
    let candidate = signals
        .roster
        .get_untracked()
        .into_iter()
        .find(|item| item.employee_id == employee_id)
        .ok_or_else(|| "Select an employee returned by the scoped roster.".to_owned())?;
    if clock_in && !candidate.can_clock_in {
        return Err("The employee already has open attendance.".into());
    }
    if start && !candidate.can_start_activity {
        return Err("The employee needs open attendance and no active labor activity.".into());
    }
    Ok(candidate)
}

fn form_value(value: FormKind) -> &'static str {
    match value {
        FormKind::Skill => "skill",
        FormKind::EquipmentClass => "equipment_class",
        FormKind::EquipmentAsset => "equipment_asset",
        FormKind::Standard => "standard",
        FormKind::Certification => "certification",
        FormKind::ClockIn => "clock_in",
        FormKind::StartActivity => "start_activity",
        FormKind::AttendanceCorrection => "attendance_correction",
        FormKind::ActivityCorrection => "activity_correction",
    }
}

fn parse_form(value: &str) -> FormKind {
    match value {
        "equipment_class" => FormKind::EquipmentClass,
        "equipment_asset" => FormKind::EquipmentAsset,
        "standard" => FormKind::Standard,
        "certification" => FormKind::Certification,
        "clock_in" => FormKind::ClockIn,
        "start_activity" => FormKind::StartActivity,
        "attendance_correction" => FormKind::AttendanceCorrection,
        "activity_correction" => FormKind::ActivityCorrection,
        _ => FormKind::Skill,
    }
}

fn submit_label(value: FormKind) -> &'static str {
    match value {
        FormKind::Skill => "Save skill",
        FormKind::EquipmentClass => "Save class",
        FormKind::EquipmentAsset => "Create asset",
        FormKind::Standard => "Publish standard",
        FormKind::Certification => "Certify employee",
        FormKind::ClockIn => "Clock in employee",
        FormKind::StartActivity => "Start activity",
        FormKind::AttendanceCorrection => "Append attendance correction",
        FormKind::ActivityCorrection => "Append activity correction",
    }
}

fn permission_summary(
    execute: bool,
    configure: bool,
    equipment: bool,
    certify: bool,
    supervise: bool,
) -> String {
    let mut values = Vec::new();
    if execute {
        values.push("execute");
    }
    if configure {
        values.push("configure");
    }
    if equipment {
        values.push("equipment");
    }
    if certify {
        values.push("certify");
    }
    if supervise {
        values.push("supervise");
    }
    format!("Granted: {}", values.join(" · "))
}

fn text_field(label: &'static str, signal: RwSignal<String>, placeholder: &'static str) -> AnyView {
    view! { <label><span>{label}</span><input required maxlength="500" placeholder=placeholder prop:value=move || signal.get() on:input=move |event| signal.set(event_target_value(&event))/></label> }.into_any()
}

fn note_field(signal: RwSignal<String>) -> AnyView {
    view! { <label class="labor-note"><span>"Audit note"</span><input maxlength="500" placeholder="Operational context" prop:value=move || signal.get() on:input=move |event| signal.set(event_target_value(&event))/></label> }.into_any()
}

fn number_field(
    label: &'static str,
    signal: RwSignal<String>,
    placeholder: &'static str,
) -> AnyView {
    view! { <label><span>{label}</span><input type="number" min="0" placeholder=placeholder prop:value=move || signal.get() on:input=move |event| signal.set(event_target_value(&event))/></label> }.into_any()
}

fn datetime_field(label: &'static str, signal: RwSignal<String>) -> AnyView {
    view! { <label><span>{label}</span><input type="datetime-local" prop:value=move || signal.get() on:input=move |event| signal.set(event_target_value(&event))/></label> }.into_any()
}

fn employee_select(signals: FormSignals, clock_in_only: bool, start_only: bool) -> AnyView {
    view! { <label><span>"Employee"</span><select required prop:value=move || option_id(signals.employee_id.get()) on:change=move |event| { signals.employee_id.set(parse_id(&event_target_value(&event))); reset_references(signals); }><option value="">"Select eligible employee"</option>{move || signals.roster.get().into_iter().filter(|item| (!clock_in_only || item.can_clock_in) && (!start_only || item.can_start_activity)).map(|item| view! { <option value=item.employee_id>{format!("{} · {}", item.display_name, item.title)}</option> }).collect_view()}</select></label> }.into_any()
}

fn activity_kind_select(signals: FormSignals) -> AnyView {
    view! { <label><span>"Activity kind"</span><select prop:value=move || kind_value(signals.activity_kind.get()) on:change=move |event| { signals.activity_kind.set(parse_kind(&event_target_value(&event))); signals.labor_standard_id.set(None); reset_references(signals); }>{activity_kind_options()}</select></label> }.into_any()
}

fn quantity_basis_select(signals: FormSignals) -> AnyView {
    view! { <label><span>"Quantity basis"</span><select prop:value=move || basis_value(signals.quantity_basis.get()) on:change=move |event| { signals.quantity_basis.set(parse_basis(&event_target_value(&event))); signals.labor_standard_id.set(None); reset_references(signals); }><option value="unit">"Unit"</option><option value="line">"Line"</option><option value="container">"Container"</option><option value="task">"Task"</option><option value="weight_gram">"Weight (gram)"</option></select></label> }.into_any()
}

fn correction_reason_select(signals: FormSignals) -> AnyView {
    view! { <label><span>"Correction reason"</span><select prop:value=move || correction_value(signals.correction_reason.get()) on:change=move |event| signals.correction_reason.set(parse_correction(&event_target_value(&event)))><option value="missed_punch">"Missed punch"</option><option value="timekeeping_error">"Timekeeping error"</option><option value="quantity_error">"Quantity error"</option><option value="exception_error">"Exception error"</option><option value="system_error">"System error"</option><option value="other">"Other"</option></select></label> }.into_any()
}

fn exception_reason_select(signals: FormSignals) -> AnyView {
    view! { <label><span>"Exception reason"</span><select prop:value=move || exception_value(signals.exception_reason.get()) on:change=move |event| signals.exception_reason.set(parse_exception(&event_target_value(&event)))><option value="">"No exception"</option><option value="equipment">"Equipment"</option><option value="congestion">"Congestion"</option><option value="inventory">"Inventory"</option><option value="quality">"Quality"</option><option value="safety">"Safety"</option><option value="system">"System"</option><option value="training">"Training"</option><option value="personal">"Personal"</option><option value="other">"Other"</option></select></label> }.into_any()
}

fn activity_kind_options() -> AnyView {
    [
        LaborActivityKind::Receiving,
        LaborActivityKind::Putaway,
        LaborActivityKind::Replenishment,
        LaborActivityKind::Picking,
        LaborActivityKind::Packing,
        LaborActivityKind::Shipping,
        LaborActivityKind::CycleCount,
        LaborActivityKind::InventoryRelocation,
        LaborActivityKind::CrossDock,
        LaborActivityKind::Yard,
        LaborActivityKind::CustomerReturn,
        LaborActivityKind::VendorReturn,
        LaborActivityKind::ValueAddedWork,
        LaborActivityKind::Break,
        LaborActivityKind::Meeting,
        LaborActivityKind::Training,
        LaborActivityKind::Maintenance,
        LaborActivityKind::Delay,
        LaborActivityKind::OtherIndirect,
    ]
    .into_iter()
    .map(|kind| view! { <option value=kind_value(kind)>{kind_label(kind)}</option> })
    .collect_view()
    .into_any()
}

fn scope_options(values: &[AccessScopeResource]) -> AnyView {
    values
        .iter()
        .map(|item| view! { <option value=item.id>{item.name.clone()}</option> })
        .collect_view()
        .into_any()
}

fn skill_options(workspace: &LaborWorkspaceResponse) -> AnyView {
    workspace.skills.iter().filter(|item| item.active).map(|item| view! { <option value=item.skill_id>{format!("{} · {}", item.code, item.name)}</option> }).collect_view().into_any()
}

fn class_options(workspace: &LaborWorkspaceResponse) -> AnyView {
    workspace.equipment_classes.iter().filter(|item| item.active).map(|item| view! { <option value=item.equipment_class_id>{format!("{} · {}", item.code, item.name)}</option> }).collect_view().into_any()
}

fn asset_options(workspace: &LaborWorkspaceResponse, facility_id: Option<i64>) -> AnyView {
    workspace.equipment_assets.iter().filter(|item| Some(item.facility_id) == facility_id && item.status == wareboxes_api_contract::v1::EquipmentStatus::Available).map(|item| view! { <option value=item.equipment_asset_id>{format!("{} · {} · {}", item.equipment_number, item.name, item.equipment_class_code)}</option> }).collect_view().into_any()
}

fn standard_options(workspace: &LaborWorkspaceResponse, signals: FormSignals) -> AnyView {
    let facility = signals.facility_id.get();
    let owner = signals.owner_id.get();
    let kind = signals.activity_kind.get();
    let basis = signals.quantity_basis.get();
    workspace.standards.iter().filter(|item| Some(item.facility_id) == facility && item.inventory_owner_id == owner && item.activity_kind == kind && item.quantity_basis == basis && item.retired_at.is_none()).map(|item| view! { <option value=item.labor_standard_id>{format!("{} · {}", item.code, item.name)}</option> }).collect_view().into_any()
}

fn reference_options(items: Vec<LaborReferenceCandidateResponse>) -> AnyView {
    items.into_iter().map(|item| view! { <option value=item.reference_id>{format!("{} · {} {}", item.display_label, item.canonical_quantity, basis_label(item.quantity_basis))}</option> }).collect_view().into_any()
}

fn attendance_options(workspace: &LaborWorkspaceResponse, facility: Option<i64>) -> AnyView {
    workspace.attendance.iter().filter(|item| item.status == AttendanceStatus::Closed && Some(item.facility_id) == facility).map(|item| view! { <option value=item.attendance_interval_id>{format!("{} · {} → {} · rev {}", item.employee_name, short_time(&item.effective_clocked_in_at), item.effective_clocked_out_at.as_deref().map(short_time).unwrap_or_else(|| "—".into()), item.effective_revision)}</option> }).collect_view().into_any()
}

fn activity_options(
    workspace: &LaborWorkspaceResponse,
    facility: Option<i64>,
    owner: Option<i64>,
) -> AnyView {
    workspace.activities.iter().filter(|item| item.status == LaborActivityStatus::Completed && Some(item.facility_id) == facility && (item.inventory_owner_id.is_none() || item.inventory_owner_id == owner)).map(|item| view! { <option value=item.labor_activity_id>{format!("{} · {} · rev {}", item.employee_name, kind_label(item.activity_kind), item.effective_revision)}</option> }).collect_view().into_any()
}

fn required_id(signal: RwSignal<Option<i64>>, label: &str) -> Result<i64, String> {
    signal
        .get_untracked()
        .ok_or_else(|| format!("Select {label}."))
}

fn required_text(value: String, label: &str) -> Result<String, String> {
    optional_text(value).ok_or_else(|| format!("Enter {label}."))
}

fn optional_text(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn nonnegative(value: String, label: &str) -> Result<i64, String> {
    let value = required_text(value, label)?;
    let parsed = value
        .parse::<i64>()
        .map_err(|_| format!("Enter a valid {label}."))?;
    (parsed >= 0)
        .then_some(parsed)
        .ok_or_else(|| format!("{label} cannot be negative."))
}

fn positive(value: String, label: &str) -> Result<i64, String> {
    let parsed = nonnegative(value, label)?;
    (parsed > 0)
        .then_some(parsed)
        .ok_or_else(|| format!("{label} must be positive."))
}

fn optional_positive(value: String, label: &str) -> Result<Option<i64>, String> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        positive(value, label).map(Some)
    }
}

fn required_timestamp(value: String, label: &str) -> Result<String, String> {
    optional_timestamp(value).ok_or_else(|| format!("Enter {label}."))
}

fn optional_timestamp(value: String) -> Option<String> {
    optional_text(value).map(|value| {
        if value.ends_with('Z') || value.contains('+') {
            value
        } else {
            format!("{value}:00Z")
        }
    })
}

fn exception_fields(
    seconds: i64,
    reason: Option<LaborExceptionReason>,
    note: String,
) -> Result<(Option<LaborExceptionReason>, Option<String>), String> {
    if seconds == 0 {
        return Ok((None, None));
    }
    Ok((
        Some(reason.ok_or_else(|| "Select an exception reason.".to_owned())?),
        Some(required_text(note, "exception evidence")?),
    ))
}

fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}
fn option_id(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}
fn optional_id(value: Option<i64>) -> String {
    value.map_or_else(|| "none".into(), |id| format!("#{id}"))
}
fn short_time(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
fn is_direct(kind: LaborActivityKind) -> bool {
    !matches!(
        kind,
        LaborActivityKind::Break
            | LaborActivityKind::Meeting
            | LaborActivityKind::Training
            | LaborActivityKind::Maintenance
            | LaborActivityKind::Delay
            | LaborActivityKind::OtherIndirect
    )
}

fn kind_value(value: LaborActivityKind) -> &'static str {
    match value {
        LaborActivityKind::Receiving => "receiving",
        LaborActivityKind::Putaway => "putaway",
        LaborActivityKind::Replenishment => "replenishment",
        LaborActivityKind::Picking => "picking",
        LaborActivityKind::Packing => "packing",
        LaborActivityKind::Shipping => "shipping",
        LaborActivityKind::CycleCount => "cycle_count",
        LaborActivityKind::InventoryRelocation => "inventory_relocation",
        LaborActivityKind::CrossDock => "cross_dock",
        LaborActivityKind::Yard => "yard",
        LaborActivityKind::CustomerReturn => "customer_return",
        LaborActivityKind::VendorReturn => "vendor_return",
        LaborActivityKind::ValueAddedWork => "value_added_work",
        LaborActivityKind::Break => "break",
        LaborActivityKind::Meeting => "meeting",
        LaborActivityKind::Training => "training",
        LaborActivityKind::Maintenance => "maintenance",
        LaborActivityKind::Delay => "delay",
        LaborActivityKind::OtherIndirect => "other_indirect",
    }
}
fn parse_kind(value: &str) -> LaborActivityKind {
    match value {
        "putaway" => LaborActivityKind::Putaway,
        "replenishment" => LaborActivityKind::Replenishment,
        "picking" => LaborActivityKind::Picking,
        "packing" => LaborActivityKind::Packing,
        "shipping" => LaborActivityKind::Shipping,
        "cycle_count" => LaborActivityKind::CycleCount,
        "inventory_relocation" => LaborActivityKind::InventoryRelocation,
        "cross_dock" => LaborActivityKind::CrossDock,
        "yard" => LaborActivityKind::Yard,
        "customer_return" => LaborActivityKind::CustomerReturn,
        "vendor_return" => LaborActivityKind::VendorReturn,
        "value_added_work" => LaborActivityKind::ValueAddedWork,
        "break" => LaborActivityKind::Break,
        "meeting" => LaborActivityKind::Meeting,
        "training" => LaborActivityKind::Training,
        "maintenance" => LaborActivityKind::Maintenance,
        "delay" => LaborActivityKind::Delay,
        "other_indirect" => LaborActivityKind::OtherIndirect,
        _ => LaborActivityKind::Receiving,
    }
}
fn kind_label(value: LaborActivityKind) -> &'static str {
    match value {
        LaborActivityKind::Receiving => "Receiving",
        LaborActivityKind::Putaway => "Putaway",
        LaborActivityKind::Replenishment => "Replenishment",
        LaborActivityKind::Picking => "Picking",
        LaborActivityKind::Packing => "Packing",
        LaborActivityKind::Shipping => "Shipping",
        LaborActivityKind::CycleCount => "Cycle count",
        LaborActivityKind::InventoryRelocation => "Inventory relocation",
        LaborActivityKind::CrossDock => "Cross-dock",
        LaborActivityKind::Yard => "Yard",
        LaborActivityKind::CustomerReturn => "Customer return",
        LaborActivityKind::VendorReturn => "Vendor return",
        LaborActivityKind::ValueAddedWork => "Value-added work",
        LaborActivityKind::Break => "Break",
        LaborActivityKind::Meeting => "Meeting",
        LaborActivityKind::Training => "Training",
        LaborActivityKind::Maintenance => "Maintenance",
        LaborActivityKind::Delay => "Delay",
        LaborActivityKind::OtherIndirect => "Other indirect",
    }
}
fn basis_value(value: LaborQuantityBasis) -> &'static str {
    match value {
        LaborQuantityBasis::Unit => "unit",
        LaborQuantityBasis::Line => "line",
        LaborQuantityBasis::Container => "container",
        LaborQuantityBasis::Task => "task",
        LaborQuantityBasis::WeightGram => "weight_gram",
    }
}
fn parse_basis(value: &str) -> LaborQuantityBasis {
    match value {
        "line" => LaborQuantityBasis::Line,
        "container" => LaborQuantityBasis::Container,
        "task" => LaborQuantityBasis::Task,
        "weight_gram" => LaborQuantityBasis::WeightGram,
        _ => LaborQuantityBasis::Unit,
    }
}
fn basis_label(value: LaborQuantityBasis) -> &'static str {
    match value {
        LaborQuantityBasis::Unit => "units",
        LaborQuantityBasis::Line => "lines",
        LaborQuantityBasis::Container => "containers",
        LaborQuantityBasis::Task => "tasks",
        LaborQuantityBasis::WeightGram => "grams",
    }
}
fn correction_value(value: LaborCorrectionReason) -> &'static str {
    match value {
        LaborCorrectionReason::MissedPunch => "missed_punch",
        LaborCorrectionReason::TimekeepingError => "timekeeping_error",
        LaborCorrectionReason::QuantityError => "quantity_error",
        LaborCorrectionReason::ExceptionError => "exception_error",
        LaborCorrectionReason::SystemError => "system_error",
        LaborCorrectionReason::Other => "other",
    }
}
fn parse_correction(value: &str) -> LaborCorrectionReason {
    match value {
        "timekeeping_error" => LaborCorrectionReason::TimekeepingError,
        "quantity_error" => LaborCorrectionReason::QuantityError,
        "exception_error" => LaborCorrectionReason::ExceptionError,
        "system_error" => LaborCorrectionReason::SystemError,
        "other" => LaborCorrectionReason::Other,
        _ => LaborCorrectionReason::MissedPunch,
    }
}
fn exception_value(value: Option<LaborExceptionReason>) -> &'static str {
    match value {
        Some(LaborExceptionReason::Equipment) => "equipment",
        Some(LaborExceptionReason::Congestion) => "congestion",
        Some(LaborExceptionReason::Inventory) => "inventory",
        Some(LaborExceptionReason::Quality) => "quality",
        Some(LaborExceptionReason::Safety) => "safety",
        Some(LaborExceptionReason::System) => "system",
        Some(LaborExceptionReason::Training) => "training",
        Some(LaborExceptionReason::Personal) => "personal",
        Some(LaborExceptionReason::Other) => "other",
        None => "",
    }
}
fn parse_exception(value: &str) -> Option<LaborExceptionReason> {
    match value {
        "equipment" => Some(LaborExceptionReason::Equipment),
        "congestion" => Some(LaborExceptionReason::Congestion),
        "inventory" => Some(LaborExceptionReason::Inventory),
        "quality" => Some(LaborExceptionReason::Quality),
        "safety" => Some(LaborExceptionReason::Safety),
        "system" => Some(LaborExceptionReason::System),
        "training" => Some(LaborExceptionReason::Training),
        "personal" => Some(LaborExceptionReason::Personal),
        "other" => Some(LaborExceptionReason::Other),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_kind_basis_matrix_accepts_direct_and_rejects_invalid_pairs() {
        assert!(validate_kind_basis(
            LaborActivityKind::Picking,
            Some(LaborQuantityBasis::Unit),
            true
        )
        .is_ok());
        assert!(validate_kind_basis(
            LaborActivityKind::Shipping,
            Some(LaborQuantityBasis::WeightGram),
            true
        )
        .is_ok());
        assert!(validate_kind_basis(
            LaborActivityKind::Yard,
            Some(LaborQuantityBasis::Unit),
            true
        )
        .is_err());
        assert!(validate_kind_basis(LaborActivityKind::Meeting, None, false).is_ok());
        assert!(validate_kind_basis(
            LaborActivityKind::Meeting,
            Some(LaborQuantityBasis::Task),
            false
        )
        .is_err());
    }

    #[test]
    fn timestamp_and_exception_builders_preserve_typed_evidence() {
        assert_eq!(
            optional_timestamp("2026-08-15T10:30".into()).as_deref(),
            Some("2026-08-15T10:30:00Z")
        );
        assert_eq!(
            exception_fields(0, Some(LaborExceptionReason::System), "ignored".into()),
            Ok((None, None))
        );
        assert!(exception_fields(12, None, "network outage".into()).is_err());
        assert!(exception_fields(
            12,
            Some(LaborExceptionReason::System),
            "network outage".into()
        )
        .is_ok());
    }
}
