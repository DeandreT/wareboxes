use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    BillableEventType, BillingContractStatus, BillingFinancialExportResponse,
    BillingLifecycleRequest, BillingReviewDecision, BillingRunStatus, BillingUnit,
    BillingWorkspaceResponse, CaptureBillableEventRequest, CaptureBillingStorageSnapshotRequest,
    ConfigureBillingRateRequest, CreateBillingContractRequest, ExportBillingRunRequest,
    GenerateBillingRunRequest, ReviewBillingRunRequest,
};
use wareboxes_core::dto::WebSessionContext;
use wareboxes_core::models::{Facility, InventoryOwner};

use super::{InlineCommandError, WorkbenchError, WorkbenchLoading};
use crate::api;
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build dispatches billing commands")
)]
enum PendingCommand {
    Create(CreateBillingContractRequest, String),
    Activate(i64, BillingLifecycleRequest, String),
    Close(i64, BillingLifecycleRequest, String),
    Rate(i64, ConfigureBillingRateRequest, String),
    Event(i64, CaptureBillableEventRequest, String),
    Snapshot(i64, CaptureBillingStorageSnapshotRequest, String),
    Generate(i64, GenerateBillingRunRequest, String),
    Review(i64, ReviewBillingRunRequest, String),
    Export(i64, ExportBillingRunRequest, String),
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build classifies billing command results")
)]
enum CommandResult {
    Updated,
    Exported(BillingFinancialExportResponse),
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "browser build consumes command callbacks and toasts"
    )
)]
struct Signals {
    workspace: RwSignal<BillingWorkspaceResponse>,
    clients: RwSignal<Vec<InventoryOwner>>,
    facilities: RwSignal<Vec<Facility>>,
    owner_filter: RwSignal<Option<i64>>,
    selected_contract: RwSignal<Option<i64>>,
    selected_run: RwSignal<Option<i64>>,
    last_export: RwSignal<Option<BillingFinancialExportResponse>>,
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
    owner_id: RwSignal<Option<i64>>,
    contract_number: RwSignal<String>,
    currency: RwSignal<String>,
    effective_from: RwSignal<String>,
    facility_id: RwSignal<Option<i64>>,
    event_type: RwSignal<BillableEventType>,
    unit: RwSignal<BillingUnit>,
    quantity: RwSignal<i64>,
    rate_minor: RwSignal<u64>,
    minimum_minor: RwSignal<u64>,
    rate_effective_from: RwSignal<String>,
    source_reference: RwSignal<String>,
    description: RwSignal<String>,
    occurred_at: RwSignal<String>,
    snapshot_date: RwSignal<String>,
    period_from: RwSignal<String>,
    period_until: RwSignal<String>,
    review_note: RwSignal<String>,
    export_batch: RwSignal<String>,
}

#[component]
pub fn BillingWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let signals = Signals {
        workspace: RwSignal::new(BillingWorkspaceResponse {
            contracts: Vec::new(),
            rates: Vec::new(),
            events: Vec::new(),
            runs: Vec::new(),
            next_cursor: None,
        }),
        clients: RwSignal::new(Vec::new()),
        facilities: RwSignal::new(Vec::new()),
        owner_filter: RwSignal::new(None),
        selected_contract: RwSignal::new(None),
        selected_run: RwSignal::new(None),
        last_export: RwSignal::new(None),
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
        owner_id: RwSignal::new(None),
        contract_number: RwSignal::new(String::new()),
        currency: RwSignal::new("USD".into()),
        effective_from: RwSignal::new(String::new()),
        facility_id: RwSignal::new(None),
        event_type: RwSignal::new(BillableEventType::Accessorial),
        unit: RwSignal::new(BillingUnit::Event),
        quantity: RwSignal::new(1),
        rate_minor: RwSignal::new(100),
        minimum_minor: RwSignal::new(0),
        rate_effective_from: RwSignal::new(String::new()),
        source_reference: RwSignal::new(String::new()),
        description: RwSignal::new(String::new()),
        occurred_at: RwSignal::new(String::new()),
        snapshot_date: RwSignal::new(String::new()),
        period_from: RwSignal::new(String::new()),
        period_until: RwSignal::new(String::new()),
        review_note: RwSignal::new(String::new()),
        export_batch: RwSignal::new(String::new()),
    };
    let current_user = expect_context::<WebSessionContext>().user.id;
    load_resources(signals);
    Effect::new(move || {
        let _ = signals.owner_filter.get();
        load_workspace(signals);
    });
    let refresh = Callback::new(move |_| load_workspace(signals));
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    });
    let create = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(inventory_owner_id) = draft.owner_id.get_untracked() else {
            signals.command_error.set(Some("Select a client.".into()));
            return;
        };
        let request = CreateBillingContractRequest {
            inventory_owner_id,
            contract_number: draft.contract_number.get_untracked(),
            currency: draft.currency.get_untracked(),
            effective_from: draft.effective_from.get_untracked(),
            effective_until: None,
        };
        dispatch(
            signals,
            PendingCommand::Create(request, api::new_idempotency_key()),
        );
    };

    view! {
        <section class="admin-workbench billing-workbench">
            <div class="admin-toolbar billing-toolbar">
                <label><span>"Client scope"</span><select prop:value=move || signals.owner_filter.get().map_or_else(String::new,|id|id.to_string()) on:change=move |event| signals.owner_filter.set(parse_id(&event_target_value(&event)))><option value="">"All permitted clients"</option>{move || client_options(signals.clients.get())}</select></label>
                <button type="button" class="button secondary-action compact" on:click=move |_| refresh.run(())>"Refresh ledger"</button>
            </div>
            {move || if signals.loading.get() {
                view! { <WorkbenchLoading label="3PL billing ledger"/> }.into_any()
            } else if let Some(message)=signals.load_error.get() {
                view! { <WorkbenchError message retry/> }.into_any()
            } else {
                billing_body(signals,draft,current_user).into_any()
            }}
            <section class="admin-editor billing-contract-create">
                <div class="admin-editor-heading"><div><p class="eyebrow">"Governed commercial terms"</p><h2>"Create billing contract"</h2></div></div>
                <form class="admin-form-grid" on:submit=create>
                    {client_picker(signals.clients,draft.owner_id,"Contract client")}
                    <label><span>"Contract number"</span><input required maxlength="80" prop:value=move || draft.contract_number.get() on:input=move |event| draft.contract_number.set(event_target_value(&event))/></label>
                    <label><span>"Currency"</span><input required minlength="3" maxlength="3" prop:value=move || draft.currency.get() on:input=move |event| draft.currency.set(event_target_value(&event).to_ascii_uppercase())/></label>
                    <label><span>"Effective from (RFC 3339)"</span><input required placeholder="2026-09-01T00:00:00Z" prop:value=move || draft.effective_from.get() on:input=move |event| draft.effective_from.set(event_target_value(&event))/></label>
                    <button class="button primary-action" type="submit" disabled=move || signals.pending.get()>"Create draft contract"</button>
                </form>
                <InlineCommandError message=signals.command_error.read_only()/>
                {move || signals.retry.get().map(|_| view!{<button type="button" class="button secondary-action compact" on:click=move |_| retry.run(())>"Retry exact command"</button>})}
            </section>
        </section>
    }
}

fn billing_body(signals: Signals, draft: Draft, current_user: i64) -> impl IntoView {
    view! {
        <>
            {billing_metrics(signals.workspace.get())}
            <div class="admin-split billing-split">
                <section class="admin-table-wrap">
                    <table class="admin-table"><caption class="sr-only">"Client billing contracts"</caption><thead><tr><th>"Contract"</th><th>"Client"</th><th>"Status"</th><th>"Currency"</th><th>"Revision"</th></tr></thead>
                    <tbody>{move || {
                        let rows=signals.workspace.get().contracts;
                        if rows.is_empty(){view!{<tr><td class="table-empty-row" colspan="5">"No billing contracts in this scope."</td></tr>}.into_any()}else{rows.into_iter().map(|contract|{let id=contract.contract_id;let selected=signals.selected_contract.get()==Some(id);view!{<tr class:selected-row=selected><td><button type="button" class="catalog-row-link" on:click=move |_| signals.selected_contract.set(Some(id))>{contract.contract_number}</button></td><td>{contract.inventory_owner_name}</td><td><span class=status_class_contract(contract.status)>{contract_status(contract.status)}</span></td><td>{contract.currency}</td><td>{contract.revision.get()}</td></tr>}}).collect_view().into_any()}
                    }}</tbody></table>
                </section>
                <section class="admin-editor">{move || selected_contract(signals,draft)}</section>
            </div>
            <div class="admin-split billing-split billing-runs">
                <section class="admin-table-wrap">
                    <table class="admin-table"><caption class="sr-only">"Billing reconciliation runs"</caption><thead><tr><th>"Run"</th><th>"Contract"</th><th>"Period"</th><th>"Coverage"</th><th>"Total"</th><th>"Status"</th></tr></thead>
                    <tbody>{move || {let rows=signals.workspace.get().runs;if rows.is_empty(){view!{<tr><td class="table-empty-row" colspan="6">"No reconciliation runs yet."</td></tr>}.into_any()}else{rows.into_iter().map(|run|{let id=run.run_id;let selected=signals.selected_run.get()==Some(id);view!{<tr class:selected-row=selected><td><button type="button" class="catalog-row-link" on:click=move |_| signals.selected_run.set(Some(id))>{format!("#{} · attempt {}",id,run.attempt)}</button></td><td>{run.contract_number}</td><td>{format!("{} → {}",short_date(&run.period_from),short_date(&run.period_until))}</td><td>{format!("{} / {}",run.charge_count,run.event_count)}</td><td>{money(run.total_minor,&run.currency)}</td><td><span class=status_class_run(run.status)>{run_status(run.status)}</span></td></tr>}}).collect_view().into_any()}}}</tbody></table>
                </section>
                <section class="admin-editor">{move || selected_run(signals,draft,current_user)}</section>
            </div>
            {move || signals.last_export.get().map(export_receipt)}
        </>
    }
}

fn billing_metrics(workspace: BillingWorkspaceResponse) -> impl IntoView {
    let pending = workspace
        .runs
        .iter()
        .filter(|run| run.status == BillingRunStatus::PendingReview)
        .count();
    let unmatched: i64 = workspace
        .runs
        .iter()
        .filter(|run| run.status == BillingRunStatus::PendingReview)
        .map(|run| run.unmatched_event_count)
        .sum();
    let unbilled = workspace.events.len().saturating_sub(
        workspace
            .runs
            .iter()
            .map(|run| run.charge_count.max(0) as usize)
            .sum::<usize>(),
    );
    view! {<div class="billing-metrics"><article><span>"Contracts"</span><strong>{workspace.contracts.len()}</strong></article><article><span>"Pending review"</span><strong>{pending}</strong></article><article><span>"Unmatched events"</span><strong>{unmatched}</strong></article><article><span>"Recent unbilled"</span><strong>{unbilled}</strong></article></div>}
}

fn selected_contract(signals: Signals, draft: Draft) -> AnyView {
    let Some(contract) = signals.selected_contract.get().and_then(|id| {
        signals
            .workspace
            .get()
            .contracts
            .into_iter()
            .find(|item| item.contract_id == id)
    }) else {
        return view!{<div class="admin-editor-placeholder">"Select a contract to configure rates, capture services, snapshot storage, and reconcile a billing period."</div>}.into_any();
    };
    let contract_id = contract.contract_id;
    let revision = contract.revision;
    let activate = contract.clone();
    let close = contract.clone();
    let rates = signals
        .workspace
        .get()
        .rates
        .into_iter()
        .filter(|rate| rate.contract_id == contract_id)
        .collect::<Vec<_>>();
    let rate_revisions = rates.clone();
    let rate_currency = contract.currency.clone();
    let submit_rate = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        dispatch(
            signals,
            PendingCommand::Rate(
                contract_id,
                ConfigureBillingRateRequest {
                    event_type: draft.event_type.get_untracked(),
                    unit: draft.unit.get_untracked(),
                    currency: rate_currency.clone(),
                    rate_minor: draft.rate_minor.get_untracked(),
                    minimum_charge_minor: draft.minimum_minor.get_untracked(),
                    effective_from: draft.rate_effective_from.get_untracked(),
                    effective_until: None,
                    expected_revision: rate_revisions
                        .iter()
                        .filter(|rate| rate.event_type == draft.event_type.get_untracked())
                        .map(|rate| rate.revision)
                        .max(),
                },
                api::new_idempotency_key(),
            ),
        )
    };
    let capture = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(facility_id) = draft.facility_id.get_untracked() else {
            signals.command_error.set(Some("Select a facility.".into()));
            return;
        };
        dispatch(
            signals,
            PendingCommand::Event(
                contract_id,
                CaptureBillableEventRequest {
                    facility_id,
                    event_type: draft.event_type.get_untracked(),
                    unit: draft.unit.get_untracked(),
                    quantity: draft.quantity.get_untracked(),
                    source_reference: draft.source_reference.get_untracked(),
                    description: draft.description.get_untracked(),
                    occurred_at: draft.occurred_at.get_untracked(),
                },
                api::new_idempotency_key(),
            ),
        )
    };
    let snapshot = Callback::new(move |_| {
        let Some(facility_id) = draft.facility_id.get_untracked() else {
            signals.command_error.set(Some("Select a facility.".into()));
            return;
        };
        dispatch(
            signals,
            PendingCommand::Snapshot(
                contract_id,
                CaptureBillingStorageSnapshotRequest {
                    facility_id,
                    snapshot_date: draft.snapshot_date.get_untracked(),
                },
                api::new_idempotency_key(),
            ),
        )
    });
    let generate = Callback::new(move |_| {
        dispatch(
            signals,
            PendingCommand::Generate(
                contract_id,
                GenerateBillingRunRequest {
                    facility_id: draft.facility_id.get_untracked(),
                    period_from: draft.period_from.get_untracked(),
                    period_until: draft.period_until.get_untracked(),
                },
                api::new_idempotency_key(),
            ),
        )
    });
    view!{<div><div class="admin-editor-heading"><div><p class="eyebrow">{contract.inventory_owner_name}</p><h2>{contract.contract_number}</h2></div><span class=status_class_contract(contract.status)>{contract_status(contract.status)}</span></div><dl class="admin-facts"><div><dt>"Effective"</dt><dd>{format!("{} → {}",contract.effective_from,contract.effective_until.unwrap_or_else(||"Open".into()))}</dd></div><div><dt>"Currency"</dt><dd>{contract.currency.clone()}</dd></div></dl>
        <div class="admin-actions">{(contract.status==BillingContractStatus::Draft).then(||view!{<button type="button" class="button primary-action" disabled=move || signals.pending.get() on:click=move |_|dispatch(signals,PendingCommand::Activate(activate.contract_id,BillingLifecycleRequest{expected_revision:revision},api::new_idempotency_key()))>"Activate contract"</button>})}{(contract.status==BillingContractStatus::Active).then(||view!{<button type="button" class="button danger-action" disabled=move || signals.pending.get() on:click=move |_|dispatch(signals,PendingCommand::Close(close.contract_id,BillingLifecycleRequest{expected_revision:revision},api::new_idempotency_key()))>"Close contract"</button>})}</div>
        <h3>"Rate card"</h3><form class="admin-form-grid compact-grid" on:submit=submit_rate><label><span>"Billable event"</span><select prop:value=move || event_wire(draft.event_type.get()) on:change=move |event|set_event_defaults(draft,&event_target_value(&event))>{event_options(false)}</select></label><label><span>"Unit"</span><select prop:value=move || unit_wire(draft.unit.get()) on:change=move |event|if let Some(unit)=parse_unit(&event_target_value(&event)){draft.unit.set(unit)}>{unit_options()}</select></label><label><span>"Rate (minor units)"</span><input type="number" min="1" prop:value=move ||draft.rate_minor.get() on:input=move |event|if let Ok(value)=event_target_value(&event).parse(){draft.rate_minor.set(value)}/></label><label><span>"Minimum"</span><input type="number" min="0" prop:value=move ||draft.minimum_minor.get() on:input=move |event|if let Ok(value)=event_target_value(&event).parse(){draft.minimum_minor.set(value)}/></label><label><span>"Effective from"</span><input required placeholder="RFC 3339" prop:value=move ||draft.rate_effective_from.get() on:input=move |event|draft.rate_effective_from.set(event_target_value(&event))/></label><button class="button secondary-action" type="submit" disabled=move ||signals.pending.get()>"Add rate version"</button></form>
        <div class="billing-rate-list">{rates.into_iter().map(|rate|view!{<span class=if rate.active{"status-chip success"}else{"status-chip neutral"}>{format!("{} · {} {} · rev {}",event_label(rate.event_type),money(rate.rate_minor,&rate.currency),unit_label(rate.unit),rate.revision.get())}</span>}).collect_view()}</div>
        {(contract.status==BillingContractStatus::Active).then(||view!{<div class="billing-command-grid"><form on:submit=capture><h3>"Capture service"</h3>{facility_picker(signals.facilities,draft.facility_id,"Facility")}<label><span>"Service type"</span><select prop:value=move ||event_wire(draft.event_type.get()) on:change=move |event|set_event_defaults(draft,&event_target_value(&event))>{event_options(true)}</select></label><label><span>"Quantity"</span><input type="number" min="1" prop:value=move ||draft.quantity.get() on:input=move |event|if let Ok(value)=event_target_value(&event).parse(){draft.quantity.set(value)}/></label><label><span>"Source reference"</span><input required prop:value=move ||draft.source_reference.get() on:input=move |event|draft.source_reference.set(event_target_value(&event))/></label><label><span>"Description"</span><input required prop:value=move ||draft.description.get() on:input=move |event|draft.description.set(event_target_value(&event))/></label><label><span>"Occurred at"</span><input required placeholder="RFC 3339" prop:value=move ||draft.occurred_at.get() on:input=move |event|draft.occurred_at.set(event_target_value(&event))/></label><button class="button secondary-action" type="submit">"Capture billable service"</button></form><section><h3>"Storage snapshot"</h3><label><span>"Snapshot date"</span><input type="date" prop:value=move ||draft.snapshot_date.get() on:input=move |event|draft.snapshot_date.set(event_target_value(&event))/></label><button type="button" class="button secondary-action" on:click=move |_|snapshot.run(())>"Snapshot occupied storage"</button><h3>"Reconciliation period"</h3><label><span>"From"</span><input placeholder="RFC 3339" prop:value=move ||draft.period_from.get() on:input=move |event|draft.period_from.set(event_target_value(&event))/></label><label><span>"Until"</span><input placeholder="RFC 3339" prop:value=move ||draft.period_until.get() on:input=move |event|draft.period_until.set(event_target_value(&event))/></label><button type="button" class="button primary-action" on:click=move |_|generate.run(())>"Generate reconciliation"</button></section></div>})}
    </div>}.into_any()
}

fn selected_run(signals: Signals, draft: Draft, current_user: i64) -> AnyView {
    let Some(run) = signals.selected_run.get().and_then(|id| {
        signals
            .workspace
            .get()
            .runs
            .into_iter()
            .find(|item| item.run_id == id)
    }) else {
        return view!{<div class="admin-editor-placeholder">"Select a reconciliation run to inspect immutable charges, coverage, review evidence, and export state."</div>}.into_any();
    };
    let approve = run.clone();
    let reject = run.clone();
    let export = run.clone();
    let self_review = run.generated_by == current_user;
    view!{<div><div class="admin-editor-heading"><div><p class="eyebrow">{format!("{} · attempt {}",run.contract_number,run.attempt)}</p><h2>{format!("Reconciliation #{}",run.run_id)}</h2></div><span class=status_class_run(run.status)>{run_status(run.status)}</span></div><div class="billing-run-summary"><strong>{money(run.total_minor,&run.currency)}</strong><span>{format!("{} charges / {} events",run.charge_count,run.event_count)}</span><span class:warning-text=(run.unmatched_event_count>0)>{format!("{} unmatched",run.unmatched_event_count)}</span></div>
        {(run.status==BillingRunStatus::PendingReview).then(||view!{<section class="billing-review"><label><span>"Review note"</span><textarea prop:value=move ||draft.review_note.get() on:input=move |event|draft.review_note.set(event_target_value(&event))></textarea></label><div class="admin-actions"><button type="button" class="button primary-action" disabled={self_review||run.unmatched_event_count>0} on:click=move |_|dispatch(signals,PendingCommand::Review(approve.run_id,ReviewBillingRunRequest{expected_revision:approve.revision,decision:BillingReviewDecision::Approve,note:nonblank(draft.review_note.get_untracked())},api::new_idempotency_key()))>"Approve reconciled charges"</button><button type="button" class="button danger-action" disabled=move ||draft.review_note.get().trim().is_empty() on:click=move |_|dispatch(signals,PendingCommand::Review(reject.run_id,ReviewBillingRunRequest{expected_revision:reject.revision,decision:BillingReviewDecision::Reject,note:nonblank(draft.review_note.get_untracked())},api::new_idempotency_key()))>"Reject for correction"</button></div>{self_review.then(||view!{<p class="admin-help">"A different administrator must review this run."</p>})}</section>})}
        {(run.status==BillingRunStatus::Approved).then(||view!{<section class="billing-review"><label><span>"External batch key"</span><input prop:value=move ||draft.export_batch.get() on:input=move |event|draft.export_batch.set(event_target_value(&event))/></label><button type="button" class="button primary-action" disabled=move ||draft.export_batch.get().trim().is_empty() on:click=move |_|dispatch(signals,PendingCommand::Export(export.run_id,ExportBillingRunRequest{expected_revision:export.revision,external_batch_key:draft.export_batch.get_untracked()},api::new_idempotency_key()))>"Create financial export"</button></section>})}
        <h3>"Immutable charge lines"</h3><div class="admin-table-wrap"><table class="admin-table compact-table"><thead><tr><th>"Event"</th><th>"Reference"</th><th>"Qty"</th><th>"Rate"</th><th>"Amount"</th></tr></thead><tbody>{run.charges.into_iter().map(|charge|view!{<tr><td>{event_label(charge.event_type)}</td><td>{charge.source_reference}</td><td>{charge.quantity}</td><td>{money(charge.rate_minor,&charge.currency)}</td><td>{money(charge.amount_minor,&charge.currency)}</td></tr>}).collect_view()}</tbody></table></div>
    </div>}.into_any()
}

fn export_receipt(export: BillingFinancialExportResponse) -> impl IntoView {
    view! {<section class="admin-editor billing-export-receipt"><div class="admin-editor-heading"><div><p class="eyebrow">"Financial export ready"</p><h2>{export.external_batch_key}</h2></div><span class="status-chip success">"Hashed + immutable"</span></div><dl class="admin-facts"><div><dt>"SHA-256"</dt><dd class="mono-value">{export.content_sha256}</dd></div><div><dt>"Lines"</dt><dd>{export.line_count}</dd></div><div><dt>"Total"</dt><dd>{money(export.total_minor,&export.currency)}</dd></div></dl><label><span>"CSV content"</span><textarea class="billing-csv" readonly prop:value=export.csv_content></textarea></label></section>}
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
            PendingCommand::Create(request, key) => api::create_billing_contract(request, key)
                .await
                .map(|_| CommandResult::Updated),
            PendingCommand::Activate(id, request, key) => {
                api::activate_billing_contract(*id, request, key)
                    .await
                    .map(|_| CommandResult::Updated)
            }
            PendingCommand::Close(id, request, key) => {
                api::close_billing_contract(*id, request, key)
                    .await
                    .map(|_| CommandResult::Updated)
            }
            PendingCommand::Rate(id, request, key) => {
                api::configure_billing_rate(*id, request, key)
                    .await
                    .map(|_| CommandResult::Updated)
            }
            PendingCommand::Event(id, request, key) => {
                api::capture_billable_event(*id, request, key)
                    .await
                    .map(|_| CommandResult::Updated)
            }
            PendingCommand::Snapshot(id, request, key) => {
                api::capture_billing_storage_snapshot(*id, request, key)
                    .await
                    .map(|_| CommandResult::Updated)
            }
            PendingCommand::Generate(id, request, key) => {
                api::generate_billing_run(*id, request, key)
                    .await
                    .map(|_| CommandResult::Updated)
            }
            PendingCommand::Review(id, request, key) => api::review_billing_run(*id, request, key)
                .await
                .map(|_| CommandResult::Updated),
            PendingCommand::Export(id, request, key) => api::export_billing_run(*id, request, key)
                .await
                .map(CommandResult::Exported),
        };
        signals.pending.set(false);
        match result {
            Ok(CommandResult::Updated) => {
                signals.retry.set(None);
                signals.toasts.success("Billing ledger updated.");
                load_workspace(signals)
            }
            Ok(CommandResult::Exported(export)) => {
                signals.retry.set(None);
                signals.last_export.set(Some(export));
                signals.toasts.success("Financial export created.");
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
                signals.facilities.set(facilities)
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.load_error.set(Some(error.message)),
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
        match api::billing_workspace(api::BillingFilters {
            inventory_owner_id: signals.owner_filter.get_untracked(),
            contract_id: None,
        })
        .await
        {
            Ok(workspace) if signals.generation.get_untracked() == generation => {
                if signals.selected_contract.get_untracked().is_none() {
                    signals
                        .selected_contract
                        .set(workspace.contracts.first().map(|item| item.contract_id))
                }
                if signals.selected_run.get_untracked().is_none() {
                    signals
                        .selected_run
                        .set(workspace.runs.first().map(|item| item.run_id))
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

fn client_picker(
    clients: RwSignal<Vec<InventoryOwner>>,
    selected: RwSignal<Option<i64>>,
    label: &'static str,
) -> AnyView {
    view!{<label><span>{label}</span><select required prop:value=move ||selected.get().map_or_else(String::new,|id|id.to_string()) on:change=move |event|selected.set(parse_id(&event_target_value(&event)))><option value="">"Select client"</option>{move ||client_options(clients.get())}</select></label>}.into_any()
}
fn facility_picker(
    facilities: RwSignal<Vec<Facility>>,
    selected: RwSignal<Option<i64>>,
    label: &'static str,
) -> AnyView {
    view!{<label><span>{label}</span><select required prop:value=move ||selected.get().map_or_else(String::new,|id|id.to_string()) on:change=move |event|selected.set(parse_id(&event_target_value(&event)))><option value="">"Select facility"</option>{move ||facilities.get().into_iter().filter(|item|item.deleted.is_none()).map(|item|view!{<option value=item.id.to_string()>{item.name.unwrap_or_else(||format!("Facility #{}",item.id))}</option>}).collect_view()}</select></label>}.into_any()
}
fn client_options(clients: Vec<InventoryOwner>) -> impl IntoView {
    clients
        .into_iter()
        .filter(|item| item.deleted.is_none())
        .map(|item| view! {<option value=item.id.to_string()>{item.name}</option>})
        .collect_view()
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}
fn nonblank(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
fn short_date(value: &str) -> String {
    value.get(..10).unwrap_or(value).to_owned()
}
fn money(value: u64, currency: &str) -> String {
    format!("{currency} {}.{:02}", value / 100, value % 100)
}
fn contract_status(value: BillingContractStatus) -> &'static str {
    match value {
        BillingContractStatus::Draft => "Draft",
        BillingContractStatus::Active => "Active",
        BillingContractStatus::Closed => "Closed",
    }
}
fn status_class_contract(value: BillingContractStatus) -> &'static str {
    match value {
        BillingContractStatus::Active => "status-chip success",
        BillingContractStatus::Draft => "status-chip warning",
        BillingContractStatus::Closed => "status-chip neutral",
    }
}
fn run_status(value: BillingRunStatus) -> &'static str {
    match value {
        BillingRunStatus::PendingReview => "Pending review",
        BillingRunStatus::Approved => "Approved",
        BillingRunStatus::Rejected => "Rejected",
        BillingRunStatus::Exported => "Exported",
    }
}
fn status_class_run(value: BillingRunStatus) -> &'static str {
    match value {
        BillingRunStatus::Exported | BillingRunStatus::Approved => "status-chip success",
        BillingRunStatus::PendingReview => "status-chip warning",
        BillingRunStatus::Rejected => "status-chip danger",
    }
}
fn set_event_defaults(draft: Draft, value: &str) {
    if let Some(event) = parse_event(value) {
        draft.event_type.set(event);
        draft.unit.set(match event {
            BillableEventType::ReceiptLine
            | BillableEventType::PickLine
            | BillableEventType::Accessorial => BillingUnit::Event,
            BillableEventType::PalletDay => BillingUnit::Pallet,
            BillableEventType::PackedCarton => BillingUnit::Carton,
            BillableEventType::DetentionHour => BillingUnit::Hour,
            _ => BillingUnit::Each,
        })
    }
}
fn event_options(manual: bool) -> impl IntoView {
    ALL_EVENTS
        .into_iter()
        .filter(move |(event, _)| {
            !manual
                || matches!(
                    event,
                    BillableEventType::RelabelUnit
                        | BillableEventType::RefurbishmentUnit
                        | BillableEventType::KitUnit
                        | BillableEventType::AssemblyUnit
                        | BillableEventType::Accessorial
                        | BillableEventType::DetentionHour
                        | BillableEventType::ValueAddedServiceUnit
                )
        })
        .map(|(event, label)| view! {<option value=event_wire(event)>{label}</option>})
        .collect_view()
}
fn unit_options() -> impl IntoView {
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
    .map(|unit| view! {<option value=unit_wire(unit)>{unit_label(unit)}</option>})
    .collect_view()
}
const ALL_EVENTS: [(BillableEventType, &str); 15] = [
    (BillableEventType::ReceiptLine, "Receipt line"),
    (BillableEventType::ReceivedUnit, "Received unit"),
    (BillableEventType::PalletDay, "Pallet day"),
    (BillableEventType::PickLine, "Pick line"),
    (BillableEventType::PickedUnit, "Picked unit"),
    (BillableEventType::PackedCarton, "Packed carton"),
    (BillableEventType::ShippedUnit, "Shipped unit"),
    (BillableEventType::ReturnUnit, "Return unit"),
    (BillableEventType::RelabelUnit, "Relabel unit"),
    (BillableEventType::RefurbishmentUnit, "Refurbishment unit"),
    (BillableEventType::KitUnit, "Kit unit"),
    (BillableEventType::AssemblyUnit, "Assembly unit"),
    (BillableEventType::Accessorial, "Accessorial"),
    (BillableEventType::DetentionHour, "Detention hour"),
    (
        BillableEventType::ValueAddedServiceUnit,
        "Value-added service",
    ),
];
fn event_label(value: BillableEventType) -> &'static str {
    ALL_EVENTS
        .into_iter()
        .find(|(event, _)| *event == value)
        .map_or("Unknown", |(_, label)| label)
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
    ALL_EVENTS
        .into_iter()
        .find(|(event, _)| event_wire(*event) == value)
        .map(|(event, _)| event)
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
fn unit_label(value: BillingUnit) -> &'static str {
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
    .find(|unit| unit_wire(*unit) == value)
}
