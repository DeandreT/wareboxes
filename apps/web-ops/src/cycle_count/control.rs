//! Supervisor policy configuration and variance-review views.

use leptos::prelude::*;
use lucide_leptos::{Eye, Plus, RefreshCw, RotateCcw, Save, X};
use wareboxes_api_contract::v1::{
    ConfigurationScope, ConfigureCycleCountPolicyRequest, CountDecisionPolicySource,
    CycleCountPolicyPage, CycleCountPolicyResponse, CycleCountVarianceDecision,
    CycleCountVariancePage, CycleCountVarianceReason, CycleCountVarianceResponse,
    CycleCountVarianceStatus, DecideCycleCountVarianceRequest, OpaqueCursor,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, Copy)]
struct PolicySignals {
    page: RwSignal<CycleCountPolicyPage>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
    generation: RwSignal<u64>,
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    dialog: RwSignal<Option<PolicyDialogMode>>,
    on_unauthorized: Callback<()>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PolicyDialogMode {
    Create,
    Reconfigure(CycleCountPolicyResponse),
}

#[component]
pub(super) fn CycleCountPolicyControl(
    initial_page: CycleCountPolicyPage,
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let access = StoredValue::new(access);
    let signals = PolicySignals {
        page: RwSignal::new(initial_page),
        cursor: RwSignal::new(None),
        history: RwSignal::new(Vec::new()),
        generation: RwSignal::new(0),
        facility_id: RwSignal::new(None),
        owner_id: RwSignal::new(None),
        loading: RwSignal::new(false),
        error: RwSignal::new(None),
        dialog: RwSignal::new(None),
        on_unauthorized,
    };
    let reset = move |_| {
        signals.facility_id.set(None);
        signals.owner_id.set(None);
        reset_policies(signals);
    };
    let previous = move |_| previous_policies(signals);
    let next = move |_| next_policies(signals);

    view! {
        <section class="cycle-count-control-view">
            <div class="cycle-count-control-bar">
                <label><span>"Facility"</span><select prop:value=move || option_value(signals.facility_id.get()) on:change=move |event| { signals.facility_id.set(parse_optional_id(&event_target_value(&event))); reset_policies(signals); }><option value="">"All facilities"</option>{move || access.with_value(|value| value.facilities.iter().map(|item| view! { <option value=item.id>{item.name.clone()}</option> }).collect_view())}</select></label>
                <label><span>"Client"</span><select prop:value=move || option_value(signals.owner_id.get()) on:change=move |event| { signals.owner_id.set(parse_optional_id(&event_target_value(&event))); reset_policies(signals); }><option value="">"All clients"</option>{move || access.with_value(|value| value.inventory_owners.iter().map(|item| view! { <option value=item.id>{item.name.clone()}</option> }).collect_view())}</select></label>
                <span class="cycle-count-control-summary">{move || format!("{} active policies", signals.page.get().items.len())}</span>
                <button type="button" class="icon-button" title="Reset filters" aria-label="Reset policy filters" on:click=reset><RotateCcw size=14/></button>
                <button type="button" class="icon-button" title="Refresh" aria-label="Refresh policies" disabled=move || signals.loading.get() on:click=move |_| request_policies(signals, signals.cursor.get_untracked())><RefreshCw size=14/></button>
                <button type="button" class="button primary-action" on:click=move |_| signals.dialog.set(Some(PolicyDialogMode::Create))><Plus size=14/>"Configure policy"</button>
            </div>
            <div class="cycle-count-control-table-region">
                <table class="data-table cycle-count-control-table"><caption class="sr-only">"Cycle count tolerance policies"</caption><thead><tr><th>"Client"</th><th>"Facility"</th><th class="numeric">"Absolute"</th><th class="numeric">"Percent"</th><th class="numeric">"Auto recounts"</th><th>"Revision"</th><th>"Configured"</th><th class="icon-column"><span class="sr-only">"Edit"</span></th></tr></thead>
                    <tbody>{move || {
                        let rows=signals.page.get().items;
                        if rows.is_empty() && !signals.loading.get() { view! { <tr><td colspan="8" class="table-empty-row">"No count policy is configured for this scope."</td></tr> }.into_any() }
                        else { rows.into_iter().map(|policy| { let edit=policy.clone(); view! { <tr><td><strong>{policy.inventory_owner_name}</strong><small class="cell-detail">{format!("Client #{}",policy.inventory_owner_id)}</small></td><td>{policy.facility_name}</td><td class="numeric strong">{format_quantity(policy.absolute_tolerance_quantity)}</td><td class="numeric">{format!("{:.2}%",f64::from(policy.percentage_tolerance_basis_points)/100.0)}</td><td class="numeric">{policy.automatic_recount_limit}</td><td>{format!("r{}",policy.revision)}</td><td>{compact_time(&policy.configured_at)}<small class="cell-detail">{format!("User #{}",policy.configured_by)}</small></td><td class="icon-column"><button type="button" class="icon-button compact" title="Reconfigure" aria-label=format!("Reconfigure policy {}",policy.policy_id) on:click=move |_| signals.dialog.set(Some(PolicyDialogMode::Reconfigure(edit.clone())))><Eye size=13/></button></td></tr> } }).collect_view().into_any() }
                    }}</tbody>
                </table>
            </div>
            <ControlFooter label="policies" count=Signal::derive(move || signals.page.get().items.len()) loading=signals.loading can_previous=Signal::derive(move || !signals.history.get().is_empty()) can_next=Signal::derive(move || signals.page.get().has_more()) on_previous=Callback::new(previous) on_next=Callback::new(next)/>
            <Show when=move || signals.error.get().is_some()><p class="inline-command-error cycle-count-control-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show>
            {move || signals.dialog.get().map(|mode| view! { <PolicyDialog mode access=access.get_value() on_close=Callback::new(move |_| signals.dialog.set(None)) on_saved=Callback::new(move |_| { signals.dialog.set(None); reset_policies(signals); }) on_unauthorized/> })}
        </section>
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PolicyAttempt {
    request: ConfigureCycleCountPolicyRequest,
    key: String,
}

#[component]
fn PolicyDialog(
    mode: PolicyDialogMode,
    access: AccessScopeWorkspace,
    on_close: Callback<()>,
    on_saved: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let existing = match &mode {
        PolicyDialogMode::Create => None,
        PolicyDialogMode::Reconfigure(policy) => Some(policy.clone()),
    };
    let reconfiguring = existing.is_some();
    let owner = RwSignal::new(existing.as_ref().map_or_else(
        || {
            access
                .inventory_owners
                .first()
                .map_or_else(String::new, |item| item.id.to_string())
        },
        |policy| policy.inventory_owner_id.to_string(),
    ));
    let facility = RwSignal::new(existing.as_ref().map_or_else(
        || {
            access
                .facilities
                .first()
                .map_or_else(String::new, |item| item.id.to_string())
        },
        |policy| policy.facility_id.to_string(),
    ));
    let absolute = RwSignal::new(existing.as_ref().map_or_else(
        || "0".to_owned(),
        |policy| policy.absolute_tolerance_quantity.to_string(),
    ));
    let percentage = RwSignal::new(existing.as_ref().map_or_else(
        || "0".to_owned(),
        |policy| policy.percentage_tolerance_basis_points.to_string(),
    ));
    let recounts = RwSignal::new(existing.as_ref().map_or_else(
        || "1".to_owned(),
        |policy| policy.automatic_recount_limit.to_string(),
    ));
    let pending = RwSignal::new(false);
    let retry = RwSignal::new(None::<PolicyAttempt>);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();
    let mode_for_submit = mode.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let attempt = if let Some(attempt) = retry.get_untracked() {
            attempt
        } else {
            match build_policy_attempt(
                &mode_for_submit,
                owner.get_untracked(),
                facility.get_untracked(),
                absolute.get_untracked(),
                percentage.get_untracked(),
                recounts.get_untracked(),
            ) {
                Ok(attempt) => attempt,
                Err(message) => {
                    error.set(Some(message));
                    return;
                }
            }
        };
        pending.set(true);
        error.set(None);
        retry.set(None);
        leptos::task::spawn_local(async move {
            match api::configure_cycle_count_policy(&attempt.request, &attempt.key).await {
                Ok(response) => {
                    pending.set(false);
                    toasts.success(format!("Cycle count policy r{} saved.", response.revision));
                    on_saved.run(());
                }
                Err(api_error) if api_error.unauthorized => {
                    pending.set(false);
                    on_unauthorized.run(());
                }
                Err(api_error) => {
                    pending.set(false);
                    if api_error.ambiguous_outcome {
                        retry.set(Some(attempt));
                    }
                    error.set(Some(api_error.message));
                }
            }
        });
    };
    let locked = move || pending.get() || retry.get().is_some();
    view! { <div class="cycle-count-dialog-backdrop"><section class="cycle-count-dialog" role="dialog" aria-modal="true" aria-labelledby="cycle-count-policy-title"><header><div><span class="eyebrow">"Inventory control"</span><h2 id="cycle-count-policy-title">{if reconfiguring{"Reconfigure count policy"}else{"Configure count policy"}}</h2></div><button type="button" class="icon-button" title="Close" aria-label="Close policy dialog" disabled=locked on:click=move |_| on_close.run(())><X size=15/></button></header><form on:submit=submit>
        <div class="cycle-count-dialog-grid"><label><span>"Client"</span><select prop:value=move || owner.get() disabled=move || locked() || reconfiguring on:change=move |event| owner.set(event_target_value(&event))>{access.inventory_owners.clone().into_iter().map(|item| view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Facility"</span><select prop:value=move || facility.get() disabled=move || locked() || reconfiguring on:change=move |event| facility.set(event_target_value(&event))>{access.facilities.clone().into_iter().map(|item| view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Absolute tolerance"</span><input type="number" min="0" prop:value=move || absolute.get() disabled=locked on:input=move |event| absolute.set(event_target_value(&event))/></label><label><span>"Percentage (basis points)"</span><input type="number" min="0" max="10000" prop:value=move || percentage.get() disabled=locked on:input=move |event| percentage.set(event_target_value(&event))/></label><label><span>"Automatic recount limit"</span><input type="number" min="0" max="10" prop:value=move || recounts.get() disabled=locked on:input=move |event| recounts.set(event_target_value(&event))/></label></div>
        <Show when=move || error.get().is_some()><p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p></Show><Show when=move || retry.get().is_some()><p class="cycle-count-retry-note">"Retry sends the exact saved request and idempotency key."</p></Show><footer><button type="button" class="button secondary-action" disabled=locked on:click=move |_| on_close.run(())>"Cancel"</button><button type="submit" class="button primary-action" disabled=move || pending.get()><Save size=14/>{move || if pending.get(){"Saving..."}else if retry.get().is_some(){"Retry save"}else{"Save policy"}}</button></footer>
    </form></section></div> }
}

fn build_policy_attempt(
    mode: &PolicyDialogMode,
    owner: String,
    facility: String,
    absolute: String,
    percentage: String,
    recounts: String,
) -> Result<PolicyAttempt, String> {
    let inventory_owner_id = parse_required_id(&owner, "Client")?;
    let facility_id = parse_required_id(&facility, "Facility")?;
    let absolute_tolerance_quantity = absolute
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .ok_or_else(|| "Absolute tolerance must be zero or greater.".to_owned())?;
    let percentage_tolerance_basis_points = percentage
        .parse::<u32>()
        .ok()
        .filter(|value| *value <= 10_000)
        .ok_or_else(|| "Percentage tolerance must be 0 through 10000 basis points.".to_owned())?;
    let automatic_recount_limit = recounts
        .parse::<u16>()
        .ok()
        .filter(|value| *value <= 10)
        .ok_or_else(|| "Automatic recount limit must be 0 through 10.".to_owned())?;
    let expected_revision = match mode {
        PolicyDialogMode::Create => None,
        PolicyDialogMode::Reconfigure(policy) => Some(policy.revision),
    };
    Ok(PolicyAttempt {
        request: ConfigureCycleCountPolicyRequest {
            inventory_owner_id,
            facility_id,
            absolute_tolerance_quantity,
            percentage_tolerance_basis_points,
            automatic_recount_limit,
            expected_revision,
        },
        key: api::new_idempotency_key(),
    })
}

#[derive(Clone, Copy)]
struct VarianceSignals {
    page: RwSignal<CycleCountVariancePage>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
    generation: RwSignal<u64>,
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    status: RwSignal<Option<CycleCountVarianceStatus>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    selected: RwSignal<Option<CycleCountVarianceResponse>>,
    dialog: RwSignal<Option<CycleCountVarianceResponse>>,
    on_unauthorized: Callback<()>,
}

#[component]
pub(super) fn CycleCountVarianceControl(
    initial_page: CycleCountVariancePage,
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let access = StoredValue::new(access);
    let layout = SplitPaneState::new("cycle-count-variance", 760);
    let signals = VarianceSignals {
        page: RwSignal::new(initial_page),
        cursor: RwSignal::new(None),
        history: RwSignal::new(Vec::new()),
        generation: RwSignal::new(0),
        facility_id: RwSignal::new(None),
        owner_id: RwSignal::new(None),
        status: RwSignal::new(None),
        loading: RwSignal::new(false),
        error: RwSignal::new(None),
        selected: RwSignal::new(None),
        dialog: RwSignal::new(None),
        on_unauthorized,
    };
    view! { <section class="cycle-count-control-view"><div class="cycle-count-control-bar"><label><span>"Facility"</span><select prop:value=move || option_value(signals.facility_id.get()) on:change=move |event|{signals.facility_id.set(parse_optional_id(&event_target_value(&event)));reset_variances(signals)}><option value="">"All facilities"</option>{move || access.with_value(|value|value.facilities.iter().map(|item|view!{<option value=item.id>{item.name.clone()}</option>}).collect_view())}</select></label><label><span>"Client"</span><select prop:value=move || option_value(signals.owner_id.get()) on:change=move |event|{signals.owner_id.set(parse_optional_id(&event_target_value(&event)));reset_variances(signals)}><option value="">"All clients"</option>{move || access.with_value(|value|value.inventory_owners.iter().map(|item|view!{<option value=item.id>{item.name.clone()}</option>}).collect_view())}</select></label><label><span>"Status"</span><select prop:value=move || variance_status_value(signals.status.get()) on:change=move |event|{signals.status.set(parse_variance_status(&event_target_value(&event)));reset_variances(signals)}><option value="">"All cases"</option><option value="awaiting_approval">"Awaiting approval"</option><option value="awaiting_recount">"Awaiting recount"</option><option value="posted">"Posted"</option></select></label><span class="cycle-count-control-summary">{move || format!("{} cases",signals.page.get().items.len())}</span><PaneControls layout master_label="variance queue" detail_label="variance detail"/><button type="button" class="icon-button" title="Reset filters" aria-label="Reset variance filters" on:click=move |_|{signals.facility_id.set(None);signals.owner_id.set(None);signals.status.set(None);reset_variances(signals)}><RotateCcw size=14/></button><button type="button" class="icon-button" title="Refresh" aria-label="Refresh variance cases" disabled=move ||signals.loading.get() on:click=move |_|request_variances(signals,signals.cursor.get_untracked())><RefreshCw size=14/></button></div>
    <div class="cycle-count-control-body split-workspace" style=move ||layout.style() data-pane-mode=move ||layout.mode_attribute()><section class="split-master cycle-count-control-master"><VarianceTable signals layout/></section><SplitPaneHandle layout/><aside class="split-detail cycle-count-control-detail">{move || signals.selected.get().map_or_else(||view!{<div class="cycle-count-empty"><h2>"Variance detail"</h2><p>"Select a variance case to inspect count evidence and make a supervisor decision."</p></div>}.into_any(),|variance|view!{<VarianceDetail variance signals/>}.into_any())}</aside></div>
    <Show when=move ||signals.error.get().is_some()><p class="inline-command-error cycle-count-control-error" role="alert">{move ||signals.error.get().unwrap_or_default()}</p></Show>{move ||signals.dialog.get().map(|variance|view!{<VarianceDecisionDialog variance on_close=Callback::new(move |_|signals.dialog.set(None)) on_saved=Callback::new(move |_|{signals.dialog.set(None);signals.selected.set(None);reset_variances(signals)}) on_unauthorized/>})}</section> }
}

#[component]
fn VarianceTable(signals: VarianceSignals, layout: SplitPaneState) -> impl IntoView {
    view! {<><div class="cycle-count-control-table-region"><table class="data-table cycle-count-control-table variance-table"><caption class="sr-only">"Cycle count variance cases"</caption><thead><tr><th>"Status"</th><th>"Client / facility"</th><th>"Location"</th><th>"Item / trace"</th><th class="numeric">"System"</th><th class="numeric">"Counted"</th><th class="numeric">"Variance"</th><th>"Attempt"</th><th class="icon-column"><span class="sr-only">"View"</span></th></tr></thead><tbody>{move ||{let rows=signals.page.get().items;if rows.is_empty()&&!signals.loading.get(){view!{<tr><td colspan="9" class="table-empty-row">"No variance cases match these filters."</td></tr>}.into_any()}else{rows.into_iter().map(|variance|{
        let selected=signals.selected.get().as_ref().is_some_and(|value|value.variance_id==variance.variance_id);
        let row=variance.clone();
        let action=variance.clone();
        let variance_id=variance.variance_id;
        let status=variance.status;
        let owner_name=variance.inventory_owner_name.clone();
        let facility_name=variance.facility_name.clone();
        let location_name=variance.stock.location_name.clone().unwrap_or_else(||variance.stock.location_barcode.clone());
        let location_barcode=variance.stock.location_barcode.clone();
        let item_name=variance.stock.primary_sku.clone().or(variance.stock.item_description.clone()).unwrap_or_else(||format!("Item #{}",variance.stock.item_id));
        let trace=trace_label(&variance);
        let system_quantity=format_quantity(variance.system_quantity);
        let counted_quantity=format_quantity(variance.counted_quantity);
        let variance_quantity=format!("{:+}",variance.variance_quantity);
        let allowed_quantity=format!("Allowed ±{}",variance.allowed_variance_quantity);
        let attempt=format!("{} / {}",variance.latest_attempt_sequence,variance.automatic_recounts_used);
        let modified_at=compact_time(&variance.modified_at);
        view!{<tr class:selected=selected on:click=move |_|{signals.selected.set(Some(row.clone()));layout.show_detail()}><td><span class=variance_status_class(status)>{variance_status_label(status)}</span></td><td><strong>{owner_name}</strong><small class="cell-detail">{facility_name}</small></td><td><strong>{location_name}</strong><small class="cell-detail">{location_barcode}</small></td><td><strong>{item_name}</strong><small class="cell-detail">{trace}</small></td><td class="numeric">{system_quantity}</td><td class="numeric strong">{counted_quantity}</td><td class="numeric variance-nonzero">{variance_quantity}<small class="cell-detail">{allowed_quantity}</small></td><td>{attempt}<small class="cell-detail">{modified_at}</small></td><td class="icon-column"><button type="button" class="icon-button compact" title="View variance" aria-label=format!("View variance {variance_id}") aria-pressed=selected on:click=move |event|{event.stop_propagation();signals.selected.set(Some(action.clone()));layout.show_detail()}><Eye size=13/></button></td></tr>}
    }).collect_view().into_any()}}}</tbody></table></div><ControlFooter label="cases" count=Signal::derive(move ||signals.page.get().items.len()) loading=signals.loading can_previous=Signal::derive(move ||!signals.history.get().is_empty()) can_next=Signal::derive(move ||signals.page.get().has_more()) on_previous=Callback::new(move |_|previous_variances(signals)) on_next=Callback::new(move |_|next_variances(signals))/></>}
}

#[component]
fn VarianceDetail(variance: CycleCountVarianceResponse, signals: VarianceSignals) -> impl IntoView {
    let action = StoredValue::new(variance.clone());
    let decision = count_decision_summary(&variance);
    view! {<div class="cycle-count-panel"><header><span class="eyebrow">"Count evidence"</span><h2>{format!("Variance #{}",variance.variance_id)}</h2><span class=variance_status_class(variance.status)>{variance_status_label(variance.status)}</span></header><dl class="cycle-count-facts"><div><dt>"Client / facility"</dt><dd>{format!("{} / {}",variance.inventory_owner_name,variance.facility_name)}</dd></div><div><dt>"Location"</dt><dd>{variance.stock.location_barcode.clone()}</dd></div><div><dt>"Inventory"</dt><dd>{variance.stock.primary_sku.clone().or(variance.stock.item_description.clone()).unwrap_or_else(||format!("Item #{}",variance.stock.item_id))}</dd></div><div><dt>"Trace"</dt><dd>{trace_label(&variance)}</dd></div><div><dt>"System / counted"</dt><dd>{format!("{} / {} {}",format_quantity(variance.system_quantity),format_quantity(variance.counted_quantity),variance.stock.uom)}</dd></div><div><dt>"Variance / allowed"</dt><dd class="variance-nonzero">{format!("{:+} / ±{}",variance.variance_quantity,variance.allowed_variance_quantity)}</dd></div><div><dt>"Operational control"</dt><dd>{format!("Policy #{} r{} / {} recounts",variance.policy_id,variance.policy_revision,variance.automatic_recount_limit)}</dd></div><div><dt>"Count decision"</dt><dd>{decision}</dd></div><div><dt>"Attempt"</dt><dd>{format!("#{} / {} automatic",variance.latest_attempt_sequence,variance.automatic_recounts_used)}</dd></div><div><dt>"Revision"</dt><dd>{format!("r{}",variance.revision)}</dd></div><div><dt>"Adjustment"</dt><dd>{variance.inventory_transaction_id.map_or_else(||"Not posted".to_owned(),|id|format!("Transaction #{id}"))}</dd></div></dl><Show when=move ||action.with_value(|value|value.status==CycleCountVarianceStatus::AwaitingApproval)><footer><button type="button" class="button primary-action" on:click=move |_|signals.dialog.set(Some(action.with_value(Clone::clone)))>"Review decision"</button></footer></Show></div>}
}

fn count_decision_summary(variance: &CycleCountVarianceResponse) -> String {
    let policy = &variance.decision_policy;
    let source = match policy.source {
        CountDecisionPolicySource::ProductDefault => "operational default".to_owned(),
        CountDecisionPolicySource::Configuration => format!(
            "configuration #{} r{} ({})",
            policy.configuration_id.unwrap_or_default(),
            policy
                .configuration_revision
                .map_or(0, wareboxes_api_contract::v1::Revision::get),
            policy
                .configuration_scope
                .map(configuration_scope_label)
                .unwrap_or("invalid scope")
        ),
    };
    let threshold = policy.approval_threshold_quantity.map_or_else(
        || "after recount limit".to_owned(),
        |quantity| format!("at ±{quantity}"),
    );
    format!(
        "{source}; tolerance {} / {:.2}%; approval {threshold}",
        policy.absolute_tolerance_quantity,
        f64::from(policy.percentage_tolerance_basis_points) / 100.0,
    )
}

const fn configuration_scope_label(scope: ConfigurationScope) -> &'static str {
    match scope {
        ConfigurationScope::Tenant => "tenant",
        ConfigurationScope::InventoryOwner { .. } => "client",
        ConfigurationScope::Facility { .. } => "facility",
        ConfigurationScope::OwnerFacility { .. } => "client + facility",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DecisionAttempt {
    request: DecideCycleCountVarianceRequest,
    key: String,
}
#[component]
fn VarianceDecisionDialog(
    variance: CycleCountVarianceResponse,
    on_close: Callback<()>,
    on_saved: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let decision = RwSignal::new(CycleCountVarianceDecision::ApproveAdjustment);
    let reason = RwSignal::new(CycleCountVarianceReason::VerifiedPhysicalCount);
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let retry = RwSignal::new(None::<DecisionAttempt>);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();
    let variance_for_submit = variance.clone();
    let variance_id = variance.variance_id;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let attempt = retry.get_untracked().or_else(|| {
            build_decision_attempt(
                &variance_for_submit,
                decision.get_untracked(),
                reason.get_untracked(),
                note.get_untracked(),
            )
            .map_err(|message| error.set(Some(message)))
            .ok()
        });
        let Some(attempt) = attempt else { return };
        pending.set(true);
        error.set(None);
        retry.set(None);
        leptos::task::spawn_local(async move {
            match api::decide_cycle_count_variance(variance_id, &attempt.request, &attempt.key)
                .await
            {
                Ok(result) => {
                    pending.set(false);
                    toasts.success(match result.decision_id {
                        _ if result.inventory_transaction_id.is_some() => {
                            "Cycle count adjustment approved and posted.".to_owned()
                        }
                        _ => format!(
                            "Blind recount task #{} created.",
                            result.next_task_id.unwrap_or_default()
                        ),
                    });
                    on_saved.run(())
                }
                Err(api_error) if api_error.unauthorized => {
                    pending.set(false);
                    on_unauthorized.run(())
                }
                Err(api_error) => {
                    pending.set(false);
                    if api_error.ambiguous_outcome {
                        retry.set(Some(attempt))
                    }
                    error.set(Some(api_error.message))
                }
            }
        })
    };
    let locked = move || pending.get() || retry.get().is_some();
    view! {<div class="cycle-count-dialog-backdrop"><section class="cycle-count-dialog" role="alertdialog" aria-modal="true" aria-labelledby="variance-decision-title"><header><div><span class="eyebrow">"Supervisor decision"</span><h2 id="variance-decision-title">{format!("Resolve variance #{}",variance.variance_id)}</h2></div><button type="button" class="icon-button" title="Close" aria-label="Close decision dialog" disabled=locked on:click=move |_|on_close.run(())><X size=15/></button></header><form on:submit=submit><p class="cycle-count-decision-summary">{format!("System {} / counted {} / variance {:+} {}",variance.system_quantity,variance.counted_quantity,variance.variance_quantity,variance.stock.uom)}</p><div class="cycle-count-dialog-grid"><label><span>"Decision"</span><select prop:value=move||decision_value(decision.get()) disabled=locked on:change=move|event|decision.set(parse_decision(&event_target_value(&event)))><option value="approve_adjustment">"Approve adjustment"</option><option value="request_recount">"Request blind recount"</option></select></label><label><span>"Reason"</span><select prop:value=move||reason_value(reason.get()) disabled=locked on:change=move|event|reason.set(parse_reason(&event_target_value(&event)))><option value="verified_physical_count">"Verified physical count"</option><option value="suspected_miscount">"Suspected miscount"</option><option value="packaging_or_uom_issue">"Packaging or UOM issue"</option><option value="receiving_or_shipping_timing">"Receiving or shipping timing"</option><option value="other">"Other"</option></select></label><label class="wide"><span>"Decision note"</span><textarea prop:value=move||note.get() disabled=locked on:input=move|event|note.set(event_target_value(&event))></textarea></label></div><Show when=move||error.get().is_some()><p class="inline-command-error" role="alert">{move||error.get().unwrap_or_default()}</p></Show><Show when=move||retry.get().is_some()><p class="cycle-count-retry-note">"Retry sends the exact saved decision and idempotency key."</p></Show><footer><button type="button" class="button secondary-action" disabled=locked on:click=move |_|on_close.run(())>"Cancel"</button><button type="submit" class="button primary-action" disabled=move||pending.get()><Save size=14/>{move||if pending.get(){"Submitting..."}else if retry.get().is_some(){"Retry decision"}else{"Submit decision"}}</button></footer></form></section></div>}
}

fn build_decision_attempt(
    variance: &CycleCountVarianceResponse,
    decision: CycleCountVarianceDecision,
    reason: CycleCountVarianceReason,
    note: String,
) -> Result<DecisionAttempt, String> {
    let note = optional_text(&note);
    if reason == CycleCountVarianceReason::Other && note.is_none() {
        return Err("Other requires a decision note.".to_owned());
    }
    Ok(DecisionAttempt {
        request: DecideCycleCountVarianceRequest {
            expected_revision: variance.revision,
            decision,
            reason,
            note,
        },
        key: api::new_idempotency_key(),
    })
}

#[component]
fn ControlFooter(
    label: &'static str,
    count: Signal<usize>,
    loading: RwSignal<bool>,
    can_previous: Signal<bool>,
    can_next: Signal<bool>,
    on_previous: Callback<()>,
    on_next: Callback<()>,
) -> impl IntoView {
    view! {<footer class="table-footer"><span>{move||if loading.get(){"Refreshing...".to_owned()}else{format!("{} {label} on this page",count.get())}}</span><button type="button" class="button secondary-action" disabled=move||loading.get()||!can_previous.get() on:click=move |_|on_previous.run(())>"Previous"</button><button type="button" class="button secondary-action" disabled=move||loading.get()||!can_next.get() on:click=move |_|on_next.run(())>"Next"</button></footer>}
}

fn request_policies(signals: PolicySignals, cursor: Option<OpaqueCursor>) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    leptos::task::spawn_local(async move {
        let result = api::cycle_count_policies(
            signals.facility_id.get_untracked(),
            signals.owner_id.get_untracked(),
            cursor.as_ref(),
        )
        .await;
        if signals.generation.get_untracked() != generation {
            return;
        }
        signals.loading.set(false);
        match result {
            Ok(page) => {
                signals.cursor.set(cursor);
                signals.page.set(page);
                signals.error.set(None)
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
    })
}
fn reset_policies(signals: PolicySignals) {
    signals.history.set(Vec::new());
    signals.cursor.set(None);
    request_policies(signals, None)
}
fn next_policies(signals: PolicySignals) {
    if let Some(next) = signals.page.get_untracked().next_cursor {
        signals
            .history
            .update(|history| history.push(signals.cursor.get_untracked()));
        request_policies(signals, Some(next))
    }
}
fn previous_policies(signals: PolicySignals) {
    let previous = signals
        .history
        .try_update(|history| history.pop())
        .flatten()
        .flatten();
    request_policies(signals, previous)
}
fn request_variances(signals: VarianceSignals, cursor: Option<OpaqueCursor>) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    leptos::task::spawn_local(async move {
        let result = api::cycle_count_variances(
            signals.facility_id.get_untracked(),
            signals.owner_id.get_untracked(),
            signals.status.get_untracked(),
            cursor.as_ref(),
        )
        .await;
        if signals.generation.get_untracked() != generation {
            return;
        }
        signals.loading.set(false);
        match result {
            Ok(page) => {
                signals.cursor.set(cursor);
                signals.page.set(page);
                signals.error.set(None)
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
    })
}
fn reset_variances(signals: VarianceSignals) {
    signals.history.set(Vec::new());
    signals.cursor.set(None);
    request_variances(signals, None)
}
fn next_variances(signals: VarianceSignals) {
    if let Some(next) = signals.page.get_untracked().next_cursor {
        signals
            .history
            .update(|history| history.push(signals.cursor.get_untracked()));
        request_variances(signals, Some(next))
    }
}
fn previous_variances(signals: VarianceSignals) {
    let previous = signals
        .history
        .try_update(|history| history.pop())
        .flatten()
        .flatten();
    request_variances(signals, previous)
}

fn parse_required_id(value: &str, label: &str) -> Result<i64, String> {
    value
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| format!("{label} is required."))
}
fn parse_optional_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|id| *id > 0)
}
fn option_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}
fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn compact_time(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
fn trace_label(value: &CycleCountVarianceResponse) -> String {
    [
        value
            .stock
            .license_plate_barcode
            .as_ref()
            .map(|v| format!("LPN {v}")),
        value.stock.lot.as_ref().map(|v| format!("Lot {v}")),
        value.stock.serial.as_ref().map(|v| format!("Serial {v}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" / ")
    .pipe(|value| {
        if value.is_empty() {
            "No LPN / lot / serial".to_owned()
        } else {
            value
        }
    })
}
fn variance_status_value(value: Option<CycleCountVarianceStatus>) -> &'static str {
    match value {
        None => "",
        Some(CycleCountVarianceStatus::AwaitingApproval) => "awaiting_approval",
        Some(CycleCountVarianceStatus::AwaitingRecount) => "awaiting_recount",
        Some(CycleCountVarianceStatus::Posted) => "posted",
    }
}
fn parse_variance_status(value: &str) -> Option<CycleCountVarianceStatus> {
    match value {
        "awaiting_approval" => Some(CycleCountVarianceStatus::AwaitingApproval),
        "awaiting_recount" => Some(CycleCountVarianceStatus::AwaitingRecount),
        "posted" => Some(CycleCountVarianceStatus::Posted),
        _ => None,
    }
}
fn variance_status_label(value: CycleCountVarianceStatus) -> &'static str {
    match value {
        CycleCountVarianceStatus::AwaitingApproval => "Awaiting approval",
        CycleCountVarianceStatus::AwaitingRecount => "Awaiting recount",
        CycleCountVarianceStatus::Posted => "Posted",
    }
}
fn variance_status_class(value: CycleCountVarianceStatus) -> &'static str {
    match value {
        CycleCountVarianceStatus::AwaitingApproval => "status held",
        CycleCountVarianceStatus::AwaitingRecount => "status processing",
        CycleCountVarianceStatus::Posted => "status shipped",
    }
}
fn decision_value(value: CycleCountVarianceDecision) -> &'static str {
    match value {
        CycleCountVarianceDecision::ApproveAdjustment => "approve_adjustment",
        CycleCountVarianceDecision::RequestRecount => "request_recount",
    }
}
fn parse_decision(value: &str) -> CycleCountVarianceDecision {
    if value == "request_recount" {
        CycleCountVarianceDecision::RequestRecount
    } else {
        CycleCountVarianceDecision::ApproveAdjustment
    }
}
fn reason_value(value: CycleCountVarianceReason) -> &'static str {
    match value {
        CycleCountVarianceReason::VerifiedPhysicalCount => "verified_physical_count",
        CycleCountVarianceReason::PackagingOrUomIssue => "packaging_or_uom_issue",
        CycleCountVarianceReason::ReceivingOrShippingTiming => "receiving_or_shipping_timing",
        CycleCountVarianceReason::SuspectedMiscount => "suspected_miscount",
        CycleCountVarianceReason::Other => "other",
    }
}
fn parse_reason(value: &str) -> CycleCountVarianceReason {
    match value {
        "packaging_or_uom_issue" => CycleCountVarianceReason::PackagingOrUomIssue,
        "receiving_or_shipping_timing" => CycleCountVarianceReason::ReceivingOrShippingTiming,
        "suspected_miscount" => CycleCountVarianceReason::SuspectedMiscount,
        "other" => CycleCountVarianceReason::Other,
        _ => CycleCountVarianceReason::VerifiedPhysicalCount,
    }
}
trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decision_validation_requires_other_note() {
        let variance = sample_variance();
        assert!(build_decision_attempt(
            &variance,
            CycleCountVarianceDecision::ApproveAdjustment,
            CycleCountVarianceReason::Other,
            String::new()
        )
        .is_err())
    }
    #[test]
    fn variance_filters_round_trip() {
        assert_eq!(
            parse_variance_status(variance_status_value(Some(
                CycleCountVarianceStatus::AwaitingApproval
            ))),
            Some(CycleCountVarianceStatus::AwaitingApproval)
        )
    }
    fn sample_variance() -> CycleCountVarianceResponse {
        serde_json::from_value(serde_json::json!({"variance_id":1,"revision":2,"status":"awaiting_approval","inventory_owner_id":1,"inventory_owner_name":"Owner","facility_id":1,"facility_name":"DC","stock":{"inventory_balance_id":1,"location_id":1,"location_barcode":"A-1","location_name":null,"item_id":1,"item_description":null,"primary_sku":null,"license_plate_barcode":null,"uom":"each","lot":null,"serial":null,"inventory_status":"available"},"policy_id":1,"policy_revision":1,"absolute_tolerance_quantity":0,"percentage_tolerance_basis_points":0,"automatic_recount_limit":0,"decision_policy":{"source":"product_default","absolute_tolerance_quantity":0,"percentage_tolerance_basis_points":0,"policy_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"latest_task_id":1,"latest_attempt_sequence":1,"automatic_recounts_used":0,"system_quantity":10,"counted_quantity":5,"variance_quantity":-5,"allowed_variance_quantity":0,"inventory_transaction_id":null,"created_at":"2026-08-09T00:00:00Z","modified_at":"2026-08-09T00:00:00Z"})).unwrap()
    }
}
