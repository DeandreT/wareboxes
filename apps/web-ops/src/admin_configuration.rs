use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use wareboxes_api_contract::v1::SimulateConfigurationRequest;
use wareboxes_api_contract::v1::{
    BillableEventType, BillingUnit, ConfigurationLifecycleRequest, ConfigurationPage,
    ConfigurationResponse, ConfigurationScope, ConfigurationSimulationResponse,
    ConfigurationStatus, CreateConfigurationRequest, DecisionRule, DecisionRuleKind,
    InventoryRotation, Revision, RollbackConfigurationRequest,
};
use wareboxes_core::dto::WebSessionContext;
use wareboxes_core::models::{Facility, InventoryOwner};

use super::{InlineCommandError, WorkbenchError, WorkbenchLoading};
use crate::api;
#[cfg(target_arch = "wasm32")]
use crate::api::ConfigurationFilters;
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeLevel {
    Tenant,
    InventoryOwner,
    Facility,
    OwnerFacility,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LifecycleAction {
    Submit,
    Approve,
    Activate,
    Retire,
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "the browser build dispatches and retries these command payloads"
    )
)]
enum PendingCommand {
    Create {
        request: CreateConfigurationRequest,
        key: String,
    },
    Lifecycle {
        configuration_id: i64,
        action: LifecycleAction,
        request: ConfigurationLifecycleRequest,
        key: String,
    },
    Rollback {
        configuration_id: i64,
        request: RollbackConfigurationRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "the browser build publishes configuration command toasts"
    )
)]
struct Signals {
    page: RwSignal<ConfigurationPage>,
    clients: RwSignal<Vec<InventoryOwner>>,
    facilities: RwSignal<Vec<Facility>>,
    selected: RwSignal<Option<ConfigurationResponse>>,
    kind_filter: RwSignal<Option<DecisionRuleKind>>,
    status_filter: RwSignal<Option<ConfigurationStatus>>,
    loading: RwSignal<bool>,
    pending: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    generation: RwSignal<u64>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[derive(Clone, Copy)]
struct Draft {
    open: RwSignal<bool>,
    kind: RwSignal<DecisionRuleKind>,
    scope: RwSignal<ScopeLevel>,
    owner_id: RwSignal<Option<i64>>,
    facility_id: RwSignal<Option<i64>>,
    effective_from: RwSignal<String>,
    effective_until: RwSignal<String>,
    expected_revision: RwSignal<String>,
    first_flag: RwSignal<bool>,
    second_flag: RwSignal<bool>,
    third_flag: RwSignal<bool>,
    first_number: RwSignal<i64>,
    second_number: RwSignal<i64>,
    third_number: RwSignal<i64>,
    rotation: RwSignal<InventoryRotation>,
    event_type: RwSignal<BillableEventType>,
    billing_unit: RwSignal<BillingUnit>,
    currency: RwSignal<String>,
}

#[derive(Clone, Copy)]
struct Simulation {
    kind: RwSignal<DecisionRuleKind>,
    owner_id: RwSignal<Option<i64>>,
    facility_id: RwSignal<Option<i64>>,
    effective_at: RwSignal<String>,
    result: RwSignal<Option<ConfigurationSimulationResponse>>,
    error: RwSignal<Option<String>>,
    pending: RwSignal<bool>,
}

#[component]
pub fn ConfigurationsWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let signals = Signals {
        page: RwSignal::new(ConfigurationPage {
            items: Vec::new(),
            next_cursor: None,
        }),
        clients: RwSignal::new(Vec::new()),
        facilities: RwSignal::new(Vec::new()),
        selected: RwSignal::new(None),
        kind_filter: RwSignal::new(None),
        status_filter: RwSignal::new(None),
        loading: RwSignal::new(true),
        pending: RwSignal::new(false),
        load_error: RwSignal::new(None),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        generation: RwSignal::new(0),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let draft = Draft {
        open: RwSignal::new(false),
        kind: RwSignal::new(DecisionRuleKind::Receipt),
        scope: RwSignal::new(ScopeLevel::Tenant),
        owner_id: RwSignal::new(None),
        facility_id: RwSignal::new(None),
        effective_from: RwSignal::new(String::new()),
        effective_until: RwSignal::new(String::new()),
        expected_revision: RwSignal::new(String::new()),
        first_flag: RwSignal::new(false),
        second_flag: RwSignal::new(true),
        third_flag: RwSignal::new(false),
        first_number: RwSignal::new(0),
        second_number: RwSignal::new(80),
        third_number: RwSignal::new(100),
        rotation: RwSignal::new(InventoryRotation::Fefo),
        event_type: RwSignal::new(BillableEventType::ReceivedUnit),
        billing_unit: RwSignal::new(BillingUnit::Each),
        currency: RwSignal::new("USD".into()),
    };
    let simulation = Simulation {
        kind: RwSignal::new(DecisionRuleKind::Receipt),
        owner_id: RwSignal::new(None),
        facility_id: RwSignal::new(None),
        effective_at: RwSignal::new(String::new()),
        result: RwSignal::new(None),
        error: RwSignal::new(None),
        pending: RwSignal::new(false),
    };
    let rollback_effective_from = RwSignal::new(String::new());
    let session = expect_context::<WebSessionContext>();
    let current_user_id = session.user.id;

    load_resources(signals);
    Effect::new(move || {
        let _ = (signals.kind_filter.get(), signals.status_filter.get());
        load_page(signals);
    });

    let refresh = Callback::new(move |_| load_page(signals));
    let open_create = Callback::new(move |_| {
        reset_draft(draft);
        signals.command_error.set(None);
        signals.retry.set(None);
        draft.open.set(true);
    });
    let submit_create = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        match build_create_request(draft) {
            Ok(request) => dispatch(
                signals,
                draft,
                PendingCommand::Create {
                    request,
                    key: api::new_idempotency_key(),
                },
            ),
            Err(message) => signals.command_error.set(Some(message)),
        }
    };
    let run_lifecycle = Callback::new(move |configuration: ConfigurationResponse| {
        let action = match configuration.status {
            ConfigurationStatus::Draft => LifecycleAction::Submit,
            ConfigurationStatus::PendingApproval => LifecycleAction::Approve,
            ConfigurationStatus::Approved => LifecycleAction::Activate,
            ConfigurationStatus::Active => LifecycleAction::Retire,
            ConfigurationStatus::Retired => return,
        };
        dispatch(
            signals,
            draft,
            PendingCommand::Lifecycle {
                configuration_id: configuration.configuration_id,
                action,
                request: ConfigurationLifecycleRequest {
                    expected_revision: configuration.revision,
                },
                key: api::new_idempotency_key(),
            },
        );
    });
    let retire_approved = Callback::new(move |configuration: ConfigurationResponse| {
        dispatch(
            signals,
            draft,
            PendingCommand::Lifecycle {
                configuration_id: configuration.configuration_id,
                action: LifecycleAction::Retire,
                request: ConfigurationLifecycleRequest {
                    expected_revision: configuration.revision,
                },
                key: api::new_idempotency_key(),
            },
        );
    });
    let rollback = Callback::new(move |configuration: ConfigurationResponse| {
        let effective_from = rollback_effective_from.get_untracked().trim().to_owned();
        if effective_from.is_empty() {
            signals.command_error.set(Some(
                "Enter the effective timestamp for the rollback draft.".into(),
            ));
            return;
        }
        dispatch(
            signals,
            draft,
            PendingCommand::Rollback {
                configuration_id: configuration.configuration_id,
                request: RollbackConfigurationRequest {
                    expected_source_revision: configuration.revision,
                    effective_from,
                    effective_until: None,
                },
                key: api::new_idempotency_key(),
            },
        );
    });
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, draft, command);
        }
    });
    let simulate = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        run_simulation(simulation, signals.on_unauthorized);
    };

    view! {
        <section class="admin-workbench configuration-workbench">
            <div class="admin-toolbar">
                <label>
                    <span class="sr-only">"Rule category"</span>
                    <select
                        prop:value=move || signals.kind_filter.get().map_or("all", kind_wire)
                        on:change=move |event| signals.kind_filter.set(parse_kind_filter(&event_target_value(&event)))
                    >
                        <option value="all">"All rule categories"</option>
                        {kind_options()}
                    </select>
                </label>
                <label>
                    <span class="sr-only">"Lifecycle status"</span>
                    <select
                        prop:value=move || signals.status_filter.get().map_or("all", status_wire)
                        on:change=move |event| signals.status_filter.set(parse_status_filter(&event_target_value(&event)))
                    >
                        <option value="all">"All lifecycle states"</option>
                        {status_options()}
                    </select>
                </label>
                <div class="admin-toolbar-actions">
                    <button type="button" class="button secondary-action compact" disabled=move || signals.loading.get() on:click=move |_| refresh.run(())>"Refresh"</button>
                    <button type="button" class="button primary-action compact" on:click=move |_| open_create.run(())>"New rule version"</button>
                </div>
            </div>
            <InlineCommandError message=signals.command_error.read_only()/>
            <Show when=move || signals.retry.get().is_some()>
                <div class="admin-form-actions">
                    <span>"The last result was ambiguous. Retry sends the exact same command key."</span>
                    <button type="button" class="button secondary-action compact" disabled=move || signals.pending.get() on:click=move |_| retry.run(())>"Retry exact command"</button>
                </div>
            </Show>

            {move || {
                if signals.loading.get() && signals.page.get().items.is_empty() {
                    view! { <WorkbenchLoading label="configuration history"/> }.into_any()
                } else if let Some(message) = signals.load_error.get() {
                    view! { <WorkbenchError message retry=refresh/> }.into_any()
                } else {
                    configuration_browser(
                        signals,
                        current_user_id,
                        rollback_effective_from,
                        run_lifecycle,
                        retire_approved,
                        rollback,
                    )
                }
            }}

            <section class="admin-editor configuration-simulator">
                <div class="admin-editor-heading">
                    <div><p class="eyebrow">"Effective rule preview"</p><h2>"Simulate configuration"</h2></div>
                </div>
                <form class="admin-form-grid" on:submit=simulate>
                    <label><span>"Category"</span><select prop:value=move || kind_wire(simulation.kind.get()) on:change=move |event| { if let Some(kind)=parse_kind_filter(&event_target_value(&event)){ simulation.kind.set(kind); } }>{kind_options()}</select></label>
                    {owner_picker(signals.clients, simulation.owner_id, "Simulation client")}
                    {facility_picker(signals.facilities, simulation.facility_id, "Simulation facility")}
                    <label><span>"Effective at (RFC 3339)"</span><input required type="text" placeholder="2026-08-12T12:00:00Z" prop:value=move || simulation.effective_at.get() on:input=move |event| simulation.effective_at.set(event_target_value(&event))/></label>
                    <div class="admin-form-actions"><button type="submit" class="button primary-action compact" disabled=move || simulation.pending.get()>"Resolve rule"</button></div>
                </form>
                <Show when=move || simulation.error.get().is_some()><p class="inline-command-error" role="alert">{move || simulation.error.get().unwrap_or_default()}</p></Show>
                {move || simulation.result.get().map(simulation_result)}
            </section>
        </section>
        <Show when=move || draft.open.get()>
            {move || configuration_dialog(signals, draft, submit_create)}
        </Show>
    }
}

fn configuration_browser(
    signals: Signals,
    current_user_id: i64,
    rollback_effective_from: RwSignal<String>,
    run_lifecycle: Callback<ConfigurationResponse>,
    retire_approved: Callback<ConfigurationResponse>,
    rollback: Callback<ConfigurationResponse>,
) -> AnyView {
    view! {
        <div class="admin-split configuration-split">
            <section class="admin-list">
                <div class="table-scroll">
                    <table class="data-table admin-table">
                        <caption class="sr-only">"Versioned warehouse configuration rules"</caption>
                        <thead><tr><th>"ID"</th><th>"Category"</th><th>"Scope"</th><th>"Status"</th><th>"Effective"</th><th>"Rev"</th></tr></thead>
                        <tbody>{move || {
                            let rows=signals.page.get().items;
                            if rows.is_empty() {
                                view!{<tr><td class="table-empty-row" colspan="6">"No configuration versions match this view."</td></tr>}.into_any()
                            } else {
                                let clients=signals.clients.get();
                                let facilities=signals.facilities.get();
                                rows.into_iter().map(|configuration| {
                                    let selected=signals.selected.get().as_ref().is_some_and(|value|value.configuration_id==configuration.configuration_id);
                                    let choose=configuration.clone();
                                    view!{<tr class:selected-row=selected><td><button type="button" class="catalog-row-link" on:click=move |_| signals.selected.set(Some(choose.clone()))>{format!("#{}",configuration.configuration_id)}</button></td><td>{kind_label(configuration.rule.kind())}</td><td>{scope_label(configuration.scope,&clients,&facilities)}</td><td><span class=status_class(configuration.status)>{status_label(configuration.status)}</span></td><td>{effective_label(&configuration)}</td><td>{configuration.revision.get()}</td></tr>}
                                }).collect_view().into_any()
                            }
                        }}</tbody>
                    </table>
                </div>
            </section>
            <section class="admin-editor" aria-label="Configuration details">
                {move || signals.selected.get().map(|configuration| configuration_detail(configuration,current_user_id,signals,rollback_effective_from,run_lifecycle,retire_approved,rollback)).unwrap_or_else(|| view!{<div class="admin-editor-placeholder">"Select a rule version to inspect its immutable facts, audit trail, and available lifecycle actions."</div>}.into_any())}
            </section>
        </div>
    }.into_any()
}

fn configuration_detail(
    configuration: ConfigurationResponse,
    current_user_id: i64,
    signals: Signals,
    rollback_effective_from: RwSignal<String>,
    run_lifecycle: Callback<ConfigurationResponse>,
    retire_approved: Callback<ConfigurationResponse>,
    rollback: Callback<ConfigurationResponse>,
) -> AnyView {
    let primary = configuration.clone();
    let retire = configuration.clone();
    let rollback_source = configuration.clone();
    let can_primary = configuration.status != ConfigurationStatus::Retired
        && !(configuration.status == ConfigurationStatus::PendingApproval
            && configuration.created_by == current_user_id);
    let primary_label = match configuration.status {
        ConfigurationStatus::Draft => "Submit for approval",
        ConfigurationStatus::PendingApproval => "Approve version",
        ConfigurationStatus::Approved => "Activate version",
        ConfigurationStatus::Active => "Retire version",
        ConfigurationStatus::Retired => "No lifecycle action",
    };
    let can_rollback = matches!(
        configuration.status,
        ConfigurationStatus::Approved | ConfigurationStatus::Active | ConfigurationStatus::Retired
    );
    view! {
        <div class="admin-editor-form configuration-detail">
            <div class="admin-editor-heading"><div><p class="eyebrow">{kind_label(configuration.rule.kind())}</p><h2>{format!("Configuration #{}",configuration.configuration_id)}</h2></div><span class=status_class(configuration.status)>{status_label(configuration.status)}</span></div>
            <dl class="catalog-summary-grid"><div><dt>"Scope"</dt><dd>{scope_level_label(configuration.scope)}</dd></div><div><dt>"Revision"</dt><dd>{configuration.revision.get()}</dd></div><div><dt>"Effective from"</dt><dd>{configuration.effective_from.clone()}</dd></div><div><dt>"Effective until"</dt><dd>{configuration.effective_until.clone().unwrap_or_else(||"Open ended".into())}</dd></div></dl>
            <section><h3>"Typed rule"</h3><p>{rule_summary(&configuration.rule)}</p></section>
            <section><h3>"Approval and promotion audit"</h3><dl class="catalog-summary-grid"><div><dt>"Created"</dt><dd>{audit_label(configuration.created_by,&configuration.created_at)}</dd></div><div><dt>"Submitted"</dt><dd>{optional_audit(configuration.submitted_by,configuration.submitted_at.as_deref())}</dd></div><div><dt>"Approved"</dt><dd>{optional_audit(configuration.approved_by,configuration.approved_at.as_deref())}</dd></div><div><dt>"Activated"</dt><dd>{optional_audit(configuration.activated_by,configuration.activated_at.as_deref())}</dd></div><div><dt>"Retired"</dt><dd>{optional_audit(configuration.retired_by,configuration.retired_at.as_deref())}</dd></div><div><dt>"Rollback source"</dt><dd>{configuration.rollback_of_configuration_id.map_or_else(||"—".into(),|id|format!("#{id}"))}</dd></div></dl></section>
            {(!can_primary && configuration.status==ConfigurationStatus::PendingApproval).then(|| view!{<p class="admin-help">"A different administrator must approve this version."</p>})}
            <div class="admin-form-actions">
                <button type="button" class="button primary-action" disabled=move || signals.pending.get() || !can_primary on:click=move |_| run_lifecycle.run(primary.clone())>{primary_label}</button>
                {(configuration.status==ConfigurationStatus::Approved).then(|| view!{<button type="button" class="button danger-action" disabled=move || signals.pending.get() on:click=move |_| retire_approved.run(retire.clone())>"Retire without activation"</button>})}
            </div>
            {can_rollback.then(|| view!{<section class="configuration-rollback"><h3>"Create rollback draft"</h3><p>"Copies this exact approved definition into a new immutable draft. The new draft still requires two-person approval."</p><label><span>"New effective timestamp (RFC 3339)"</span><input type="text" placeholder="2026-09-01T00:00:00Z" prop:value=move || rollback_effective_from.get() on:input=move |event| rollback_effective_from.set(event_target_value(&event))/></label><button type="button" class="button secondary-action" disabled=move || signals.pending.get() on:click=move |_| rollback.run(rollback_source.clone())>"Create rollback draft"</button></section>})}
        </div>
    }.into_any()
}

fn configuration_dialog(
    signals: Signals,
    draft: Draft,
    submit: impl Fn(leptos::ev::SubmitEvent) + Copy + 'static,
) -> AnyView {
    view! {
        <div class="modal-backdrop" role="presentation">
            <section class="modal-panel configuration-dialog" role="dialog" aria-modal="true" aria-labelledby="configuration-dialog-title">
                <header><div><p class="eyebrow">"Versioned decision table"</p><h2 id="configuration-dialog-title">"New configuration rule"</h2></div><button type="button" class="icon-button" aria-label="Close configuration dialog" disabled=move || signals.pending.get() on:click=move |_| draft.open.set(false)>"×"</button></header>
                <form on:submit=submit>
                    <fieldset disabled=move || signals.pending.get()>
                        <div class="admin-form-grid">
                            <label><span>"Rule category"</span><select prop:value=move || kind_wire(draft.kind.get()) on:change=move |event| { if let Some(kind)=parse_kind_filter(&event_target_value(&event)){ draft.kind.set(kind); reset_rule_defaults(draft,kind); } }>{kind_options()}</select></label>
                            <label><span>"Scope"</span><select prop:value=move || scope_wire(draft.scope.get()) on:change=move |event| draft.scope.set(parse_scope(&event_target_value(&event)))><option value="tenant">"Tenant default"</option><option value="inventory_owner">"Client"</option><option value="facility">"Facility"</option><option value="owner_facility">"Client + facility"</option></select></label>
                            {move || scope_fields(signals,draft)}
                            <label><span>"Effective from (RFC 3339)"</span><input required type="text" placeholder="2026-09-01T00:00:00Z" prop:value=move || draft.effective_from.get() on:input=move |event| draft.effective_from.set(event_target_value(&event))/></label>
                            <label><span>"Effective until (optional)"</span><input type="text" placeholder="Open ended" prop:value=move || draft.effective_until.get() on:input=move |event| draft.effective_until.set(event_target_value(&event))/></label>
                            <label><span>"Expected latest revision (for a new version)"</span><input type="number" min="1" prop:value=move || draft.expected_revision.get() on:input=move |event| draft.expected_revision.set(event_target_value(&event))/></label>
                        </div>
                        <section class="configuration-rule-editor"><h3>{move || format!("{} policy",kind_label(draft.kind.get()))}</h3>{move || rule_editor(draft)}</section>
                    </fieldset>
                    <InlineCommandError message=signals.command_error.read_only()/>
                    <footer class="admin-form-actions"><button type="button" class="button secondary-action" disabled=move || signals.pending.get() on:click=move |_| draft.open.set(false)>"Cancel"</button><button type="submit" class="button primary-action" disabled=move || signals.pending.get()>"Create draft"</button></footer>
                </form>
            </section>
        </div>
    }.into_any()
}

fn scope_fields(signals: Signals, draft: Draft) -> AnyView {
    match draft.scope.get() {
        ScopeLevel::Tenant => view!{<p class="admin-help">"Applies when no more-specific active rule matches."</p>}.into_any(),
        ScopeLevel::InventoryOwner => owner_picker(signals.clients,draft.owner_id,"Client"),
        ScopeLevel::Facility => facility_picker(signals.facilities,draft.facility_id,"Facility"),
        ScopeLevel::OwnerFacility => view!{<>{owner_picker(signals.clients,draft.owner_id,"Client")}{facility_picker(signals.facilities,draft.facility_id,"Facility")}</>}.into_any(),
    }
}

fn rule_editor(draft: Draft) -> AnyView {
    match draft.kind.get() {
        DecisionRuleKind::Receipt => view!{<div class="admin-form-grid">{flag(draft.first_flag,"Allow unexpected receipts")}{flag(draft.second_flag,"Quarantine unmapped items")}{number(draft.first_number,"Over-receipt tolerance (basis points)",0,10000)}</div>}.into_any(),
        DecisionRuleKind::Putaway => view!{<div class="admin-form-grid">{flag(draft.first_flag,"Require zone compatibility")}{flag(draft.second_flag,"Enforce location capacity")}{flag(draft.third_flag,"Allow mixed lots")}</div>}.into_any(),
        DecisionRuleKind::Allocation => view!{<div class="admin-form-grid"><label><span>"Rotation"</span><select prop:value=move || rotation_wire(draft.rotation.get()) on:change=move |event| draft.rotation.set(if event_target_value(&event)=="fifo"{InventoryRotation::Fifo}else{InventoryRotation::Fefo})><option value="fifo">"FIFO"</option><option value="fefo">"FEFO"</option></select></label>{flag(draft.first_flag,"Allow partial allocation")}{flag(draft.second_flag,"Require complete line")}</div>}.into_any(),
        DecisionRuleKind::Replenishment => view!{<div class="admin-form-grid">{number(draft.first_number,"Minimum percent",0,99)}{number(draft.second_number,"Target percent",1,100)}{flag(draft.first_flag,"Include inbound projection")}</div>}.into_any(),
        DecisionRuleKind::Wave => view!{<div class="admin-form-grid">{number(draft.first_number,"Maximum orders",1,10000)}{flag(draft.first_flag,"Require complete allocation")}</div>}.into_any(),
        DecisionRuleKind::Pick => view!{<div class="admin-form-grid">{flag(draft.first_flag,"Require source location scan")}{flag(draft.second_flag,"Require item scan")}{flag(draft.third_flag,"Require destination container scan")}</div>}.into_any(),
        DecisionRuleKind::Pack => view!{<div class="admin-form-grid">{flag(draft.first_flag,"Require station scan")}{flag(draft.second_flag,"Require weight")}{flag(draft.third_flag,"Allow mixed orders")}</div>}.into_any(),
        DecisionRuleKind::Count => view!{<div class="admin-form-grid">{number(draft.first_number,"Absolute tolerance",0,i64::MAX)}{number(draft.second_number,"Percentage tolerance (basis points)",0,10000)}{number(draft.third_number,"Approval threshold",0,i64::MAX)}</div>}.into_any(),
        DecisionRuleKind::Document => view!{<div class="admin-form-grid">{flag(draft.first_flag,"Generate packing slip")}{flag(draft.second_flag,"Generate carton label")}{flag(draft.third_flag,"Require tracking barcode")}</div>}.into_any(),
        DecisionRuleKind::Billing => view!{<div class="admin-form-grid"><label><span>"Billable event"</span><select prop:value=move || event_wire(draft.event_type.get()) on:change=move |event| { if let Some(value)=parse_event(&event_target_value(&event)){draft.event_type.set(value);} }>{billing_event_options()}</select></label><label><span>"Billing unit"</span><select prop:value=move || unit_wire(draft.billing_unit.get()) on:change=move |event| { if let Some(value)=parse_unit(&event_target_value(&event)){draft.billing_unit.set(value);} }>{billing_unit_options()}</select></label><label><span>"Currency"</span><input required minlength="3" maxlength="3" prop:value=move || draft.currency.get() on:input=move |event| draft.currency.set(event_target_value(&event).to_ascii_uppercase())/></label>{number(draft.first_number,"Rate (minor units)",1,1_000_000_000_000)}{number(draft.second_number,"Minimum charge (minor units)",0,1_000_000_000_000)}</div>}.into_any(),
    }
}

fn flag(signal: RwSignal<bool>, label: &'static str) -> AnyView {
    view!{<label class="admin-toggle"><input type="checkbox" prop:checked=move || signal.get() on:change=move |event| signal.set(event_target_checked(&event))/><span>{label}</span></label>}.into_any()
}

fn number(signal: RwSignal<i64>, label: &'static str, min: i64, max: i64) -> AnyView {
    view!{<label><span>{label}</span><input type="number" min=min max=max prop:value=move || signal.get() on:input=move |event| { if let Ok(value)=event_target_value(&event).parse(){signal.set(value);} }/></label>}.into_any()
}

fn owner_picker(
    clients: RwSignal<Vec<InventoryOwner>>,
    selected: RwSignal<Option<i64>>,
    label: &'static str,
) -> AnyView {
    view!{<label><span>{label}</span><select required prop:value=move || selected.get().map_or_else(String::new,|id|id.to_string()) on:change=move |event| selected.set(parse_optional_id(&event_target_value(&event)))><option value="">"Select client"</option>{move || clients.get().into_iter().filter(|client|client.deleted.is_none()).map(|client|view!{<option value=client.id.to_string()>{client.name}</option>}).collect_view()}</select></label>}.into_any()
}

fn facility_picker(
    facilities: RwSignal<Vec<Facility>>,
    selected: RwSignal<Option<i64>>,
    label: &'static str,
) -> AnyView {
    view!{<label><span>{label}</span><select required prop:value=move || selected.get().map_or_else(String::new,|id|id.to_string()) on:change=move |event| selected.set(parse_optional_id(&event_target_value(&event)))><option value="">"Select facility"</option>{move || facilities.get().into_iter().filter(|facility|facility.deleted.is_none()).map(|facility|view!{<option value=facility.id.to_string()>{facility.name.unwrap_or_else(||format!("Facility #{}",facility.id))}</option>}).collect_view()}</select></label>}.into_any()
}

fn build_create_request(draft: Draft) -> Result<CreateConfigurationRequest, String> {
    let scope = match draft.scope.get_untracked() {
        ScopeLevel::Tenant => ConfigurationScope::Tenant,
        ScopeLevel::InventoryOwner => ConfigurationScope::InventoryOwner {
            inventory_owner_id: draft.owner_id.get_untracked().ok_or("Select a client.")?,
        },
        ScopeLevel::Facility => ConfigurationScope::Facility {
            facility_id: draft
                .facility_id
                .get_untracked()
                .ok_or("Select a facility.")?,
        },
        ScopeLevel::OwnerFacility => ConfigurationScope::OwnerFacility {
            inventory_owner_id: draft.owner_id.get_untracked().ok_or("Select a client.")?,
            facility_id: draft
                .facility_id
                .get_untracked()
                .ok_or("Select a facility.")?,
        },
    };
    let effective_from = draft.effective_from.get_untracked().trim().to_owned();
    if effective_from.is_empty() {
        return Err("Enter an effective timestamp.".into());
    }
    let effective_until = nonblank(draft.effective_until.get_untracked());
    let expected_revision = nonblank(draft.expected_revision.get_untracked())
        .map(|value| {
            value
                .parse::<i64>()
                .map_err(|_| "Expected revision must be a positive integer.".to_owned())
                .and_then(|value| Revision::new(value).map_err(|error| error.to_string()))
        })
        .transpose()?;
    let rule = build_rule(draft)?;
    rule.validate().map_err(str::to_owned)?;
    Ok(CreateConfigurationRequest {
        scope,
        effective_from,
        effective_until,
        rule,
        expected_revision,
    })
}

fn build_rule(draft: Draft) -> Result<DecisionRule, String> {
    let first = draft.first_number.get_untracked();
    let second = draft.second_number.get_untracked();
    let third = draft.third_number.get_untracked();
    let first_u16 = || {
        u16::try_from(first)
            .map_err(|_| "The first numeric value is outside its supported range.".to_owned())
    };
    Ok(match draft.kind.get_untracked() {
        DecisionRuleKind::Receipt => DecisionRule::Receipt {
            allow_unexpected: draft.first_flag.get_untracked(),
            quarantine_unmapped_items: draft.second_flag.get_untracked(),
            over_receipt_tolerance_basis_points: first_u16()?,
        },
        DecisionRuleKind::Putaway => DecisionRule::Putaway {
            require_zone_compatibility: draft.first_flag.get_untracked(),
            enforce_location_capacity: draft.second_flag.get_untracked(),
            allow_mixed_lots: draft.third_flag.get_untracked(),
        },
        DecisionRuleKind::Allocation => DecisionRule::Allocation {
            rotation: draft.rotation.get_untracked(),
            allow_partial: draft.first_flag.get_untracked(),
            require_complete_line: draft.second_flag.get_untracked(),
        },
        DecisionRuleKind::Replenishment => DecisionRule::Replenishment {
            minimum_percent: u8::try_from(first)
                .map_err(|_| "Minimum percent is invalid.".to_owned())?,
            target_percent: u8::try_from(second)
                .map_err(|_| "Target percent is invalid.".to_owned())?,
            include_inbound_projection: draft.first_flag.get_untracked(),
        },
        DecisionRuleKind::Wave => DecisionRule::Wave {
            max_orders: u32::try_from(first)
                .map_err(|_| "Maximum orders is invalid.".to_owned())?,
            require_complete_allocation: draft.first_flag.get_untracked(),
        },
        DecisionRuleKind::Pick => DecisionRule::Pick {
            require_source_location_scan: draft.first_flag.get_untracked(),
            require_item_scan: draft.second_flag.get_untracked(),
            require_destination_container_scan: draft.third_flag.get_untracked(),
        },
        DecisionRuleKind::Pack => DecisionRule::Pack {
            require_station_scan: draft.first_flag.get_untracked(),
            require_weight: draft.second_flag.get_untracked(),
            allow_mixed_orders: draft.third_flag.get_untracked(),
        },
        DecisionRuleKind::Count => DecisionRule::Count {
            absolute_tolerance: first,
            percentage_tolerance_basis_points: u16::try_from(second)
                .map_err(|_| "Count tolerance is invalid.".to_owned())?,
            approval_threshold: third,
        },
        DecisionRuleKind::Document => DecisionRule::Document {
            generate_packing_slip: draft.first_flag.get_untracked(),
            generate_carton_label: draft.second_flag.get_untracked(),
            require_tracking_barcode: draft.third_flag.get_untracked(),
        },
        DecisionRuleKind::Billing => DecisionRule::Billing {
            event_type: draft.event_type.get_untracked(),
            unit: draft.billing_unit.get_untracked(),
            currency: draft.currency.get_untracked().trim().to_ascii_uppercase(),
            rate_minor: u64::try_from(first).map_err(|_| "Billing rate is invalid.".to_owned())?,
            minimum_charge_minor: u64::try_from(second)
                .map_err(|_| "Minimum charge is invalid.".to_owned())?,
        },
    })
}

fn dispatch(signals: Signals, draft: Draft, command: PendingCommand) {
    if signals.pending.get_untracked() {
        return;
    }
    signals.pending.set(true);
    signals.command_error.set(None);
    signals.retry.set(Some(command.clone()));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, draft, command);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = match &command {
            PendingCommand::Create { request, key } => {
                api::create_configuration(request, key).await
            }
            PendingCommand::Lifecycle {
                configuration_id,
                action,
                request,
                key,
            } => match action {
                LifecycleAction::Submit => {
                    api::submit_configuration(*configuration_id, request, key).await
                }
                LifecycleAction::Approve => {
                    api::approve_configuration(*configuration_id, request, key).await
                }
                LifecycleAction::Activate => {
                    api::activate_configuration(*configuration_id, request, key).await
                }
                LifecycleAction::Retire => {
                    api::retire_configuration(*configuration_id, request, key).await
                }
            },
            PendingCommand::Rollback {
                configuration_id,
                request,
                key,
            } => api::rollback_configuration(*configuration_id, request, key).await,
        };
        signals.pending.set(false);
        match result {
            Ok(configuration) => {
                let message = match command {
                    PendingCommand::Create { .. } => "Configuration draft created.",
                    PendingCommand::Lifecycle {
                        action: LifecycleAction::Submit,
                        ..
                    } => "Configuration submitted for approval.",
                    PendingCommand::Lifecycle {
                        action: LifecycleAction::Approve,
                        ..
                    } => "Configuration approved.",
                    PendingCommand::Lifecycle {
                        action: LifecycleAction::Activate,
                        ..
                    } => "Configuration activated.",
                    PendingCommand::Lifecycle {
                        action: LifecycleAction::Retire,
                        ..
                    } => "Configuration retired.",
                    PendingCommand::Rollback { .. } => "Rollback draft created.",
                };
                signals.retry.set(None);
                signals.selected.set(Some(configuration));
                draft.open.set(false);
                signals.toasts.success(message);
                load_page(signals);
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

fn run_simulation(simulation: Simulation, on_unauthorized: Callback<()>) {
    let (Some(inventory_owner_id), Some(facility_id)) = (
        simulation.owner_id.get_untracked(),
        simulation.facility_id.get_untracked(),
    ) else {
        simulation
            .error
            .set(Some("Select both a client and facility.".into()));
        return;
    };
    let effective_at = simulation.effective_at.get_untracked().trim().to_owned();
    if effective_at.is_empty() {
        simulation
            .error
            .set(Some("Enter the effective timestamp to evaluate.".into()));
        return;
    }
    simulation.pending.set(true);
    simulation.error.set(None);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (
        inventory_owner_id,
        facility_id,
        effective_at,
        on_unauthorized,
    );
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::simulate_configuration(&SimulateConfigurationRequest {
            kind: simulation.kind.get_untracked(),
            inventory_owner_id,
            facility_id,
            effective_at,
        })
        .await
        {
            Ok(result) => simulation.result.set(Some(result)),
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => simulation.error.set(Some(error.message)),
        }
        simulation.pending.set(false);
    });
}

fn load_resources(signals: Signals) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = signals;
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = async {
            let clients = api::internal_get::<Vec<InventoryOwner>>(
                "/api/inventory-owners?show_deleted=false",
            )
            .await?;
            let facilities =
                api::internal_get::<Vec<Facility>>("/api/facilities?show_deleted=false").await?;
            Ok::<_, api::ApiError>((clients, facilities))
        }
        .await;
        match result {
            Ok((clients, facilities)) => {
                signals.clients.set(clients);
                signals.facilities.set(facilities);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.load_error.set(Some(error.message)),
        }
    });
}

fn load_page(signals: Signals) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    signals.load_error.set(None);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::configurations(
            ConfigurationFilters {
                kind: signals.kind_filter.get_untracked(),
                status: signals.status_filter.get_untracked(),
                inventory_owner_id: None,
                facility_id: None,
            },
            None,
        )
        .await
        {
            Ok(page) if signals.generation.get_untracked() == generation => {
                if let Some(selected) = signals.selected.get_untracked() {
                    if let Some(updated) = page
                        .items
                        .iter()
                        .find(|item| item.configuration_id == selected.configuration_id)
                    {
                        signals.selected.set(Some(updated.clone()));
                    }
                }
                signals.page.set(page);
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

fn simulation_result(result: ConfigurationSimulationResponse) -> AnyView {
    let evaluated = result.evaluated_candidate_count;
    result.matched_configuration.map_or_else(||view!{<div class="admin-state"><strong>"No active rule matched"</strong><span>{format!("{evaluated} candidates evaluated. The workflow must fail closed or use an explicit product default.")}</span></div>}.into_any(),|configuration|view!{<div class="admin-state"><strong>{format!("Matched #{} · {}",configuration.configuration_id,scope_level_label(configuration.scope))}</strong><span>{rule_summary(&configuration.rule)}</span><small>{format!("{evaluated} active candidates evaluated at {}.",result.effective_at)}</small></div>}.into_any())
}

fn reset_draft(draft: Draft) {
    draft.open.set(false);
    draft.kind.set(DecisionRuleKind::Receipt);
    draft.scope.set(ScopeLevel::Tenant);
    draft.owner_id.set(None);
    draft.facility_id.set(None);
    draft.effective_from.set(String::new());
    draft.effective_until.set(String::new());
    draft.expected_revision.set(String::new());
    reset_rule_defaults(draft, DecisionRuleKind::Receipt);
}
fn reset_rule_defaults(draft: Draft, kind: DecisionRuleKind) {
    draft.first_flag.set(false);
    draft.second_flag.set(false);
    draft.third_flag.set(false);
    draft.first_number.set(0);
    draft.second_number.set(0);
    draft.third_number.set(0);
    match kind {
        DecisionRuleKind::Receipt => draft.second_flag.set(true),
        DecisionRuleKind::Putaway => {
            draft.first_flag.set(true);
            draft.second_flag.set(true);
        }
        DecisionRuleKind::Allocation => {
            draft.first_flag.set(true);
            draft.rotation.set(InventoryRotation::Fefo);
        }
        DecisionRuleKind::Replenishment => {
            draft.first_number.set(30);
            draft.second_number.set(80);
            draft.first_flag.set(true);
        }
        DecisionRuleKind::Wave => {
            draft.first_number.set(100);
            draft.first_flag.set(true);
        }
        DecisionRuleKind::Pick => {
            draft.first_flag.set(true);
            draft.second_flag.set(true);
            draft.third_flag.set(true);
        }
        DecisionRuleKind::Pack => {
            draft.first_flag.set(true);
            draft.second_flag.set(true);
        }
        DecisionRuleKind::Count => {
            draft.first_number.set(0);
            draft.second_number.set(0);
            draft.third_number.set(1);
        }
        DecisionRuleKind::Document => {
            draft.first_flag.set(true);
            draft.second_flag.set(true);
            draft.third_flag.set(true);
        }
        DecisionRuleKind::Billing => {
            draft.event_type.set(BillableEventType::ReceivedUnit);
            draft.billing_unit.set(BillingUnit::Each);
            draft.currency.set("USD".into());
            draft.first_number.set(1);
        }
    }
}

fn nonblank(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}
fn parse_optional_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|id| *id > 0)
}
fn scope_wire(value: ScopeLevel) -> &'static str {
    match value {
        ScopeLevel::Tenant => "tenant",
        ScopeLevel::InventoryOwner => "inventory_owner",
        ScopeLevel::Facility => "facility",
        ScopeLevel::OwnerFacility => "owner_facility",
    }
}
fn parse_scope(value: &str) -> ScopeLevel {
    match value {
        "inventory_owner" => ScopeLevel::InventoryOwner,
        "facility" => ScopeLevel::Facility,
        "owner_facility" => ScopeLevel::OwnerFacility,
        _ => ScopeLevel::Tenant,
    }
}
fn scope_level_label(value: ConfigurationScope) -> &'static str {
    match value {
        ConfigurationScope::Tenant => "Tenant default",
        ConfigurationScope::InventoryOwner { .. } => "Client",
        ConfigurationScope::Facility { .. } => "Facility",
        ConfigurationScope::OwnerFacility { .. } => "Client + facility",
    }
}
fn scope_label(
    value: ConfigurationScope,
    clients: &[InventoryOwner],
    facilities: &[Facility],
) -> String {
    match value {
        ConfigurationScope::Tenant => "Tenant default".into(),
        ConfigurationScope::InventoryOwner { inventory_owner_id } => {
            client_name(clients, inventory_owner_id)
        }
        ConfigurationScope::Facility { facility_id } => facility_name(facilities, facility_id),
        ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => format!(
            "{} · {}",
            client_name(clients, inventory_owner_id),
            facility_name(facilities, facility_id)
        ),
    }
}
fn client_name(clients: &[InventoryOwner], id: i64) -> String {
    clients
        .iter()
        .find(|item| item.id == id)
        .map_or_else(|| format!("Client #{id}"), |item| item.name.clone())
}
fn facility_name(facilities: &[Facility], id: i64) -> String {
    facilities
        .iter()
        .find(|item| item.id == id)
        .and_then(|item| item.name.clone())
        .unwrap_or_else(|| format!("Facility #{id}"))
}
fn effective_label(value: &ConfigurationResponse) -> String {
    value.effective_until.as_ref().map_or_else(
        || format!("{} → open", value.effective_from),
        |until| format!("{} → {until}", value.effective_from),
    )
}
fn audit_label(user: i64, at: &str) -> String {
    format!("User #{user} · {at}")
}
fn optional_audit(user: Option<i64>, at: Option<&str>) -> String {
    match (user, at) {
        (Some(user), Some(at)) => audit_label(user, at),
        _ => "—".into(),
    }
}
fn status_class(value: ConfigurationStatus) -> &'static str {
    match value {
        ConfigurationStatus::Active => "status-chip success",
        ConfigurationStatus::PendingApproval | ConfigurationStatus::Approved => {
            "status-chip warning"
        }
        ConfigurationStatus::Draft | ConfigurationStatus::Retired => "status-chip neutral",
    }
}
fn status_label(value: ConfigurationStatus) -> &'static str {
    match value {
        ConfigurationStatus::Draft => "Draft",
        ConfigurationStatus::PendingApproval => "Pending approval",
        ConfigurationStatus::Approved => "Approved",
        ConfigurationStatus::Active => "Active",
        ConfigurationStatus::Retired => "Retired",
    }
}
fn status_wire(value: ConfigurationStatus) -> &'static str {
    match value {
        ConfigurationStatus::Draft => "draft",
        ConfigurationStatus::PendingApproval => "pending_approval",
        ConfigurationStatus::Approved => "approved",
        ConfigurationStatus::Active => "active",
        ConfigurationStatus::Retired => "retired",
    }
}
fn parse_status_filter(value: &str) -> Option<ConfigurationStatus> {
    match value {
        "draft" => Some(ConfigurationStatus::Draft),
        "pending_approval" => Some(ConfigurationStatus::PendingApproval),
        "approved" => Some(ConfigurationStatus::Approved),
        "active" => Some(ConfigurationStatus::Active),
        "retired" => Some(ConfigurationStatus::Retired),
        _ => None,
    }
}
fn kind_wire(value: DecisionRuleKind) -> &'static str {
    match value {
        DecisionRuleKind::Receipt => "receipt",
        DecisionRuleKind::Putaway => "putaway",
        DecisionRuleKind::Allocation => "allocation",
        DecisionRuleKind::Replenishment => "replenishment",
        DecisionRuleKind::Wave => "wave",
        DecisionRuleKind::Pick => "pick",
        DecisionRuleKind::Pack => "pack",
        DecisionRuleKind::Count => "count",
        DecisionRuleKind::Document => "document",
        DecisionRuleKind::Billing => "billing",
    }
}
fn kind_label(value: DecisionRuleKind) -> &'static str {
    match value {
        DecisionRuleKind::Receipt => "Receipt",
        DecisionRuleKind::Putaway => "Putaway",
        DecisionRuleKind::Allocation => "Allocation",
        DecisionRuleKind::Replenishment => "Replenishment",
        DecisionRuleKind::Wave => "Wave",
        DecisionRuleKind::Pick => "Pick",
        DecisionRuleKind::Pack => "Pack",
        DecisionRuleKind::Count => "Count",
        DecisionRuleKind::Document => "Document",
        DecisionRuleKind::Billing => "Billing",
    }
}
fn parse_kind_filter(value: &str) -> Option<DecisionRuleKind> {
    match value {
        "receipt" => Some(DecisionRuleKind::Receipt),
        "putaway" => Some(DecisionRuleKind::Putaway),
        "allocation" => Some(DecisionRuleKind::Allocation),
        "replenishment" => Some(DecisionRuleKind::Replenishment),
        "wave" => Some(DecisionRuleKind::Wave),
        "pick" => Some(DecisionRuleKind::Pick),
        "pack" => Some(DecisionRuleKind::Pack),
        "count" => Some(DecisionRuleKind::Count),
        "document" => Some(DecisionRuleKind::Document),
        "billing" => Some(DecisionRuleKind::Billing),
        _ => None,
    }
}
fn rotation_wire(value: InventoryRotation) -> &'static str {
    match value {
        InventoryRotation::Fifo => "fifo",
        InventoryRotation::Fefo => "fefo",
    }
}
fn event_wire(value: BillableEventType) -> &'static str {
    match value {
        BillableEventType::ReceiptLine => "receipt_line",
        BillableEventType::ReceivedUnit => "received_unit",
        BillableEventType::PalletDay => "pallet_day",
        BillableEventType::PickLine => "pick_line",
        BillableEventType::PickedUnit => "picked_unit",
        BillableEventType::PackedCarton => "packed_carton",
        BillableEventType::ShippedUnit => "shipped_unit",
        BillableEventType::ReturnUnit => "return_unit",
        BillableEventType::RelabelUnit => "relabel_unit",
        BillableEventType::RefurbishmentUnit => "refurbishment_unit",
        BillableEventType::KitUnit => "kit_unit",
        BillableEventType::AssemblyUnit => "assembly_unit",
        BillableEventType::Accessorial => "accessorial",
        BillableEventType::DetentionHour => "detention_hour",
        BillableEventType::ValueAddedServiceUnit => "value_added_service_unit",
    }
}
fn parse_event(value: &str) -> Option<BillableEventType> {
    match value {
        "receipt_line" => Some(BillableEventType::ReceiptLine),
        "received_unit" => Some(BillableEventType::ReceivedUnit),
        "pallet_day" => Some(BillableEventType::PalletDay),
        "pick_line" => Some(BillableEventType::PickLine),
        "picked_unit" => Some(BillableEventType::PickedUnit),
        "packed_carton" => Some(BillableEventType::PackedCarton),
        "shipped_unit" => Some(BillableEventType::ShippedUnit),
        "return_unit" => Some(BillableEventType::ReturnUnit),
        "relabel_unit" => Some(BillableEventType::RelabelUnit),
        "refurbishment_unit" => Some(BillableEventType::RefurbishmentUnit),
        "kit_unit" => Some(BillableEventType::KitUnit),
        "assembly_unit" => Some(BillableEventType::AssemblyUnit),
        "accessorial" => Some(BillableEventType::Accessorial),
        "detention_hour" => Some(BillableEventType::DetentionHour),
        "value_added_service_unit" => Some(BillableEventType::ValueAddedServiceUnit),
        _ => None,
    }
}
fn unit_wire(value: BillingUnit) -> &'static str {
    match value {
        BillingUnit::Event => "event",
        BillingUnit::Each => "each",
        BillingUnit::Case => "case",
        BillingUnit::Pallet => "pallet",
        BillingUnit::Carton => "carton",
        BillingUnit::Hour => "hour",
        BillingUnit::Day => "day",
    }
}
fn parse_unit(value: &str) -> Option<BillingUnit> {
    match value {
        "event" => Some(BillingUnit::Event),
        "each" => Some(BillingUnit::Each),
        "case" => Some(BillingUnit::Case),
        "pallet" => Some(BillingUnit::Pallet),
        "carton" => Some(BillingUnit::Carton),
        "hour" => Some(BillingUnit::Hour),
        "day" => Some(BillingUnit::Day),
        _ => None,
    }
}
fn kind_options() -> AnyView {
    [
        DecisionRuleKind::Receipt,
        DecisionRuleKind::Putaway,
        DecisionRuleKind::Allocation,
        DecisionRuleKind::Replenishment,
        DecisionRuleKind::Wave,
        DecisionRuleKind::Pick,
        DecisionRuleKind::Pack,
        DecisionRuleKind::Count,
        DecisionRuleKind::Document,
        DecisionRuleKind::Billing,
    ]
    .into_iter()
    .map(|kind| view! {<option value=kind_wire(kind)>{kind_label(kind)}</option>})
    .collect_view()
    .into_any()
}
fn status_options() -> AnyView {
    [
        ConfigurationStatus::Draft,
        ConfigurationStatus::PendingApproval,
        ConfigurationStatus::Approved,
        ConfigurationStatus::Active,
        ConfigurationStatus::Retired,
    ]
    .into_iter()
    .map(|status| view! {<option value=status_wire(status)>{status_label(status)}</option>})
    .collect_view()
    .into_any()
}
fn billing_event_options() -> AnyView {
    [BillableEventType::ReceiptLine,BillableEventType::ReceivedUnit,BillableEventType::PalletDay,BillableEventType::PickLine,BillableEventType::PickedUnit,BillableEventType::PackedCarton,BillableEventType::ShippedUnit,BillableEventType::ReturnUnit,BillableEventType::RelabelUnit,BillableEventType::RefurbishmentUnit,BillableEventType::KitUnit,BillableEventType::AssemblyUnit,BillableEventType::Accessorial,BillableEventType::DetentionHour,BillableEventType::ValueAddedServiceUnit].into_iter().map(|value|view!{<option value=event_wire(value)>{event_wire(value).replace('_'," ")}</option>}).collect_view().into_any()
}
fn billing_unit_options() -> AnyView {
    [
        BillingUnit::Event,
        BillingUnit::Each,
        BillingUnit::Case,
        BillingUnit::Pallet,
        BillingUnit::Carton,
        BillingUnit::Hour,
        BillingUnit::Day,
    ]
    .into_iter()
    .map(|value| view! {<option value=unit_wire(value)>{unit_wire(value)}</option>})
    .collect_view()
    .into_any()
}
fn rule_summary(rule: &DecisionRule) -> String {
    match rule{DecisionRule::Receipt{allow_unexpected,quarantine_unmapped_items,over_receipt_tolerance_basis_points}=>format!("Unexpected: {allow_unexpected}; quarantine unmapped: {quarantine_unmapped_items}; tolerance: {over_receipt_tolerance_basis_points} bp"),DecisionRule::Putaway{require_zone_compatibility,enforce_location_capacity,allow_mixed_lots}=>format!("Zone compatibility: {require_zone_compatibility}; capacity: {enforce_location_capacity}; mixed lots: {allow_mixed_lots}"),DecisionRule::Allocation{rotation,allow_partial,require_complete_line}=>format!("{rotation:?}; partial: {allow_partial}; complete line: {require_complete_line}"),DecisionRule::Replenishment{minimum_percent,target_percent,include_inbound_projection}=>format!("Minimum {minimum_percent}%; target {target_percent}%; inbound projection: {include_inbound_projection}"),DecisionRule::Wave{max_orders,require_complete_allocation}=>format!("Maximum {max_orders} orders; complete allocation: {require_complete_allocation}"),DecisionRule::Pick{require_source_location_scan,require_item_scan,require_destination_container_scan}=>format!("Source scan: {require_source_location_scan}; item scan: {require_item_scan}; destination scan: {require_destination_container_scan}"),DecisionRule::Pack{require_station_scan,require_weight,allow_mixed_orders}=>format!("Station scan: {require_station_scan}; weight: {require_weight}; mixed orders: {allow_mixed_orders}"),DecisionRule::Count{absolute_tolerance,percentage_tolerance_basis_points,approval_threshold}=>format!("Absolute tolerance {absolute_tolerance}; percentage {percentage_tolerance_basis_points} bp; approval threshold {approval_threshold}"),DecisionRule::Document{generate_packing_slip,generate_carton_label,require_tracking_barcode}=>format!("Packing slip: {generate_packing_slip}; carton label: {generate_carton_label}; tracking scan: {require_tracking_barcode}"),DecisionRule::Billing{event_type,unit,currency,rate_minor,minimum_charge_minor}=>format!("{event_type:?} per {unit:?}: {currency} {rate_minor} minor units; minimum {minimum_charge_minor}")}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_and_scope_wire_value_round_trips() {
        for kind in [
            DecisionRuleKind::Receipt,
            DecisionRuleKind::Putaway,
            DecisionRuleKind::Allocation,
            DecisionRuleKind::Replenishment,
            DecisionRuleKind::Wave,
            DecisionRuleKind::Pick,
            DecisionRuleKind::Pack,
            DecisionRuleKind::Count,
            DecisionRuleKind::Document,
            DecisionRuleKind::Billing,
        ] {
            assert_eq!(parse_kind_filter(kind_wire(kind)), Some(kind));
        }
        for scope in [
            ScopeLevel::Tenant,
            ScopeLevel::InventoryOwner,
            ScopeLevel::Facility,
            ScopeLevel::OwnerFacility,
        ] {
            assert_eq!(parse_scope(scope_wire(scope)), scope);
        }
    }
}
