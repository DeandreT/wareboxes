mod display;
mod forms;
mod recovery;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CancelTenantCellMoveRequest, CheckpointTenantCellMoveRequest, CompleteTenantCellMoveRequest,
    CutoverTenantCellMoveRequest, FreezeTenantCellMoveRequest, OpaqueCursor,
    PlanTenantCellMoveRequest, RollbackTenantCellMoveRequest, StartTenantCellMoveCopyRequest,
    TenantCellMoveAction, TenantCellMoveEventPage, TenantCellMoveEventPageRequest,
    TenantCellMovePage, TenantCellMovePageRequest, TenantCellMoveResponse, TenantCellMoveStatus,
    ValidateTenantCellMoveRequest, VerifyTenantCellMoveCutoverRequest,
};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone)]
pub(super) enum Dialog {
    Plan,
    Action(Box<TenantCellMoveResponse>, TenantCellMoveAction),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", content = "arguments", rename_all = "snake_case")]
pub(super) enum PendingCommand {
    Plan(i64, PlanTenantCellMoveRequest, String),
    StartCopy(i64, StartTenantCellMoveCopyRequest, String),
    Checkpoint(i64, CheckpointTenantCellMoveRequest, String),
    Freeze(i64, FreezeTenantCellMoveRequest, String),
    Validate(i64, ValidateTenantCellMoveRequest, String),
    Cutover(i64, CutoverTenantCellMoveRequest, String),
    VerifyCutover(i64, VerifyTenantCellMoveCutoverRequest, String),
    Complete(i64, CompleteTenantCellMoveRequest, String),
    Rollback(i64, RollbackTenantCellMoveRequest, String),
    Cancel(i64, CancelTenantCellMoveRequest, String),
}

impl PendingCommand {
    fn idempotency_key(&self) -> &str {
        match self {
            Self::Plan(_, _, key)
            | Self::StartCopy(_, _, key)
            | Self::Checkpoint(_, _, key)
            | Self::Freeze(_, _, key)
            | Self::Validate(_, _, key)
            | Self::Cutover(_, _, key)
            | Self::VerifyCutover(_, _, key)
            | Self::Complete(_, _, key)
            | Self::Rollback(_, _, key)
            | Self::Cancel(_, _, key) => key,
        }
    }

    fn recovery_label(&self) -> String {
        match self {
            Self::Plan(tenant_id, _, _) => format!("Plan move for tenant #{tenant_id}"),
            Self::StartCopy(id, _, _) => format!("Start copy for move #{id}"),
            Self::Checkpoint(id, _, _) => format!("Checkpoint move #{id}"),
            Self::Freeze(id, _, _) => format!("Freeze writes for move #{id}"),
            Self::Validate(id, _, _) => format!("Validate move #{id}"),
            Self::Cutover(id, _, _) => format!("Cut over move #{id}"),
            Self::VerifyCutover(id, _, _) => format!("Verify cutover for move #{id}"),
            Self::Complete(id, _, _) => format!("Complete move #{id}"),
            Self::Rollback(id, _, _) => format!("Roll back move #{id}"),
            Self::Cancel(id, _, _) => format!("Cancel move #{id}"),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Signals {
    movements: RwSignal<TenantCellMovePage>,
    events: RwSignal<TenantCellMoveEventPage>,
    selected_id: RwSignal<Option<i64>>,
    selected: RwSignal<Option<TenantCellMoveResponse>>,
    tenant_id: RwSignal<String>,
    data_cell_id: RwSignal<String>,
    status: RwSignal<Option<TenantCellMoveStatus>>,
    loading: RwSignal<bool>,
    loaded: RwSignal<bool>,
    detail_loading: RwSignal<bool>,
    events_loading: RwSignal<bool>,
    list_generation: RwSignal<u64>,
    detail_generation: RwSignal<u64>,
    event_generation: RwSignal<u64>,
    error: RwSignal<Option<String>>,
    dialog: RwSignal<Option<Dialog>>,
    command_pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Vec<recovery::StoredPendingCommand>>,
    recovery_loaded: RwSignal<bool>,
    recovery_error: RwSignal<Option<String>>,
    recovery_binding: recovery::RecoveryBinding,
    current_tenant_id: i64,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn TenantCellMovesWorkspace(
    current_user_id: i64,
    current_tenant_id: i64,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let recovery_binding = recovery::RecoveryBinding {
        user_id: current_user_id,
        control_tenant_id: current_tenant_id,
    };
    let signals = Signals {
        movements: RwSignal::new(TenantCellMovePage::new(Vec::new(), None)),
        events: RwSignal::new(TenantCellMoveEventPage::new(Vec::new(), None)),
        selected_id: RwSignal::new(None),
        selected: RwSignal::new(None),
        tenant_id: RwSignal::new(String::new()),
        data_cell_id: RwSignal::new(String::new()),
        status: RwSignal::new(None),
        loading: RwSignal::new(true),
        loaded: RwSignal::new(false),
        detail_loading: RwSignal::new(false),
        events_loading: RwSignal::new(false),
        list_generation: RwSignal::new(0),
        detail_generation: RwSignal::new(0),
        event_generation: RwSignal::new(0),
        error: RwSignal::new(None),
        dialog: RwSignal::new(None),
        command_pending: RwSignal::new(false),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(Vec::new()),
        recovery_loaded: RwSignal::new(false),
        recovery_error: RwSignal::new(None),
        recovery_binding,
        current_tenant_id,
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = forms::Drafts::new();
    Effect::new(move |_| restore_recovery(signals));
    Effect::new(move |_| refresh(signals));
    let plan = move |_| {
        if !can_start_command(
            signals.command_pending.get_untracked(),
            !signals.retry.get_untracked().is_empty(),
            signals.recovery_loaded.get_untracked(),
            signals.recovery_error.get_untracked().is_some(),
        ) {
            return;
        }
        drafts.reset_plan();
        signals.command_error.set(None);
        signals.dialog.set(Some(Dialog::Plan));
    };
    let apply = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        invalidate_detail(signals);
        refresh(signals);
    };
    view! {
        <section class="cell-moves-workspace">
            <header class="page-heading cell-move-heading">
                <div><p class="eyebrow">"Platform control"</p><h1>"Tenant cell moves"</h1><p>"Plan and execute governed tenant placement changes with capacity reservations, write fencing, validation, cutover proof, and an immutable event trail."</p></div>
                <div><button class="button primary-action" type="button" disabled=move || !can_start_command(signals.command_pending.get(), !signals.retry.get().is_empty(), signals.recovery_loaded.get(), signals.recovery_error.get().is_some()) on:click=plan>"Plan move"</button><button class="button secondary-action" type="button" disabled=move || signals.loading.get() on:click=move |_| refresh(signals)><Icon icon=UiIcon::Refresh/><span>"Refresh"</span></button></div>
            </header>
            {move || metrics(signals)}
            <form class="cell-move-toolbar" on:submit=apply>
                <label><span>"Tenant ID"</span><input type="number" min="1" placeholder="All tenants" prop:value=move || signals.tenant_id.get() on:input=move |event| signals.tenant_id.set(event_target_value(&event))/></label>
                <label><span>"Data-cell ID"</span><input type="number" min="1" placeholder="Source or target" prop:value=move || signals.data_cell_id.get() on:input=move |event| signals.data_cell_id.set(event_target_value(&event))/></label>
                <label><span>"Status"</span><select prop:value=move || display::status_wire(signals.status.get()) on:change=move |event| signals.status.set(display::parse_status(&event_target_value(&event)))><option value="">"All statuses"</option><option value="planned">"Planned"</option><option value="copying">"Copying"</option><option value="frozen">"Writes frozen"</option><option value="validated">"Validated"</option><option value="cut_over">"Cut over"</option><option value="completed">"Completed"</option><option value="cancelled">"Cancelled"</option><option value="rolled_back">"Rolled back"</option></select></label>
                <button class="button secondary-action compact" type="submit">"Apply"</button>
            </form>
            <Show when=move || signals.error.get().is_some()><section class="cell-move-error" role="alert"><span>{move || signals.error.get().unwrap_or_default()}</span><button class="text-button" type="button" on:click=move |_| refresh(signals)>"Retry reads"</button></section></Show>
            <Show when=move || signals.recovery_error.get().is_some()><section class="cell-move-error" role="alert"><span>{move || signals.recovery_error.get().unwrap_or_default()}</span></section></Show>
            <div class="cell-move-layout">{move || movement_panel(signals)}{move || detail_panel(signals, drafts)}</div>
            {move || recovery_panels(signals)}
            {move || signals.dialog.get().map(|dialog| forms::dialog(signals, drafts, dialog))}
        </section>
    }
}

fn metrics(signals: Signals) -> AnyView {
    let movements = signals.movements.get().items;
    let in_progress = movements
        .iter()
        .filter(|movement| {
            !matches!(
                movement.status,
                TenantCellMoveStatus::Completed
                    | TenantCellMoveStatus::Cancelled
                    | TenantCellMoveStatus::RolledBack
            )
        })
        .count();
    let frozen = movements
        .iter()
        .filter(|movement| movement.write_frozen)
        .count();
    let awaiting_proof = movements
        .iter()
        .filter(|movement| {
            movement.status == TenantCellMoveStatus::CutOver
                && movement.cutover_verification.is_none()
        })
        .count();
    view! {
        <section class="cell-move-metrics"><article><span>"Moves loaded"</span><strong>{movements.len()}</strong></article><article><span>"In progress"</span><strong>{in_progress}</strong></article><article><span>"Write fenced"</span><strong>{frozen}</strong></article><article><span>"Awaiting cutover proof"</span><strong>{awaiting_proof}</strong></article></section>
    }
    .into_any()
}

fn recovery_panels(signals: Signals) -> AnyView {
    signals
        .retry
        .get()
        .into_iter()
        .map(|stored| {
            let control_tenant_id = stored.control_tenant_id;
            let context_matches =
                retry_context_matches(control_tenant_id, signals.current_tenant_id);
            let label = stored.command.recovery_label();
            let key = stored.command.idempotency_key().to_owned();
            let command = stored.command;
            view! {
                <section class="cell-move-retry">
                    <span><strong>{label}</strong>" · "{if context_matches { "An unresolved command is retained in durable browser recovery.".to_owned() } else { format!("Created under control tenant #{control_tenant_id}. Switch back to that tenant before retrying.") }}<small>{format!("Idempotency key {key}")}</small></span>
                    <button class="button secondary-action compact" type="button" disabled=move || signals.command_pending.get() || !context_matches on:click=move |_| dispatch(signals, command.clone())>"Retry exact command"</button>
                </section>
            }
        })
        .collect_view()
        .into_any()
}

fn movement_panel(signals: Signals) -> AnyView {
    if signals.loading.get() && !signals.loaded.get() {
        return state("Loading tenant cell moves", true);
    }
    let page = signals.movements.get();
    let next = page.next_cursor.clone();
    let count = page.items.len();
    let content = if page.items.is_empty() {
        state("No tenant cell moves match these filters.", false)
    } else {
        view! {
            <div class="table-scroll"><table class="dense-table"><thead><tr><th>"Tenant"</th><th>"Route"</th><th>"Status"</th><th>"Revision"</th><th></th></tr></thead><tbody>{page.items.into_iter().map(|movement| {
                let id = movement.tenant_cell_move_id;
                let selected = signals.selected_id.get() == Some(id);
                view! { <tr class:selected=selected><td><strong>{movement.tenant.name.clone()}</strong><small>{format!("{} · tenant #{}", movement.tenant.slug, movement.tenant.tenant_id)}</small></td><td><strong>{format!("{} → {}", movement.source_cell.key, movement.target_cell.key)}</strong><small>{format!("{} → {}", movement.source_cell.region, movement.target_cell.region)}</small></td><td><span class=display::status_class(movement.status)>{display::status_label(movement.status)}</span></td><td>{movement.revision.get()}</td><td><button class="text-button" type="button" on:click=move |_| load_detail(signals, id)>"Inspect"</button></td></tr> }
            }).collect_view()}</tbody></table></div>
        }.into_any()
    };
    view! {
        <section class="cell-move-panel cell-move-list"><header><div><h2>"Movement ledger"</h2><span>{format!("{count} loaded")}</span></div>{next.map(|cursor| view! { <button class="text-button" type="button" disabled=move || signals.loading.get() on:click=move |_| load_page(signals, Some(cursor.clone()), true)>"Load more"</button> })}</header>{content}</section>
    }
    .into_any()
}

fn detail_panel(signals: Signals, drafts: forms::Drafts) -> AnyView {
    if signals.detail_loading.get() {
        return state("Loading movement evidence", true);
    }
    let Some(movement) = signals.selected.get() else {
        return view! {
            <section class="cell-move-panel cell-move-detail empty-detail"><Icon icon=UiIcon::Orchestration/><strong>"Select a tenant cell move"</strong><span>"Inspect state, explicit transition blockers, proof artifacts, and immutable history."</span></section>
        }.into_any();
    };
    let events = signals.events.get();
    let next = events.next_cursor.clone();
    let event_count = events.items.len();
    let active_tenant_warning = movement.tenant.tenant_id == signals.current_tenant_id;
    let actions = movement.action_eligibility.clone();
    let movement_for_actions = movement.clone();
    let placement_revision = display::current_placement_revision(
        movement.status,
        movement.source_placement_revision,
        movement.cutover_placement_revision,
        movement.rollback_placement_revision,
    );
    let exact_retry_required = !signals.retry.get().is_empty();
    let command_pending = signals.command_pending.get();
    let recovery_loaded = signals.recovery_loaded.get();
    let recovery_unavailable = signals.recovery_error.get().is_some();
    let new_command_allowed = can_start_command(
        command_pending,
        exact_retry_required,
        recovery_loaded,
        recovery_unavailable,
    );
    view! {
        <section class="cell-move-panel cell-move-detail">
            <header><div><p class="eyebrow">{format!("Move #{}", movement.tenant_cell_move_id)}</p><h2>{movement.tenant.name.clone()}</h2><span class=display::status_class(movement.status)>{display::status_label(movement.status)}</span></div><div class="move-route"><strong>{format!("{} → {}", movement.source_cell.key, movement.target_cell.key)}</strong><span>{format!("{} → {}", movement.source_cell.region, movement.target_cell.region)}</span></div></header>
            {active_tenant_warning.then(|| view! { <section class="cell-move-warning danger"><strong>"Your current tenant context is the tenant being moved."</strong><span>"Switch to another authorized tenant before executing transitions. This avoids depending on the placement being changed."</span></section> })}
            <dl class="cell-move-facts"><div><dt>"Move revision"</dt><dd>{movement.revision.get()}</dd></div><div><dt>"Current placement revision"</dt><dd>{placement_revision.get()}</dd></div><div><dt>"Residency"</dt><dd>{movement.residency_requirement.clone()}</dd></div><div><dt>"Write fence"</dt><dd>{if movement.write_frozen { "Active" } else { "Open" }}</dd></div><div><dt>"Requested"</dt><dd>{display::short_timestamp(&movement.requested_at)}</dd></div><div><dt>"Requested by"</dt><dd>{format!("User #{}", movement.requested_by)}</dd></div><div class="wide"><dt>"Reason"</dt><dd>{movement.reason.clone()}</dd></div><div class="wide"><dt>"Copy reference"</dt><dd>{movement.copy_reference.clone().unwrap_or_else(|| "Not recorded".into())}</dd></div></dl>
            <section class="cell-move-controls"><header><div><h3>"Lifecycle controls"</h3><span>"Every transition is revision guarded and idempotent."</span></div></header><div class="action-grid">{actions.into_iter().map(|eligibility| {
                let action = eligibility.action;
                let eligible = eligibility.eligible;
                let actionable = eligible && new_command_allowed;
                let movement = movement_for_actions.clone();
                let blockers = eligibility.blockers;
                view! { <article class:blocked=!actionable><div><strong>{display::action_label(action)}</strong><span>{if !recovery_loaded { "Checking exact retry" } else if recovery_unavailable { "Recovery unavailable" } else if exact_retry_required { "Exact retry required" } else if command_pending { "Command in progress" } else if eligible { "Ready" } else { "Blocked" }}</span></div>{if blockers.is_empty() { view! { <p>{if !recovery_loaded { "Wait while durable exact-retry state is restored." } else if recovery_unavailable { "Restore browser storage access before sending a command." } else if exact_retry_required { "Resolve the ambiguous command before starting another action." } else if command_pending { "Wait for the current command to finish." } else { "All current preconditions are satisfied." }}</p> }.into_any() } else { view! { <ul>{blockers.into_iter().map(|blocker| view! { <li>{display::blocker_label(blocker)}</li> }).collect_view()}</ul> }.into_any() }}<button class=if matches!(action, TenantCellMoveAction::Freeze | TenantCellMoveAction::Cutover | TenantCellMoveAction::Rollback | TenantCellMoveAction::Cancel) { "button danger-action compact" } else { "button secondary-action compact" } type="button" disabled=!actionable on:click=move |_| { if eligible && can_start_command(signals.command_pending.get_untracked(), !signals.retry.get_untracked().is_empty(), signals.recovery_loaded.get_untracked(), signals.recovery_error.get_untracked().is_some()) { drafts.reset_action(&movement); signals.command_error.set(None); signals.dialog.set(Some(Dialog::Action(Box::new(movement.clone()), action))); } }>{display::action_label(action)}</button></article> }
            }).collect_view()}</div></section>
            {proof_panel(&movement)}
            <section class="cell-move-evidence"><header><div><h3>"Event history"</h3><span>{format!("{event_count} events loaded")}</span></div>{next.map(|cursor| { let id = movement.tenant_cell_move_id; view! { <button class="text-button" type="button" disabled=move || signals.events_loading.get() on:click=move |_| load_events(signals, id, Some(cursor.clone()), true)>"Load more"</button> } })}</header>{if signals.events_loading.get() && events.items.is_empty() { state("Loading movement history", true) } else if events.items.is_empty() { state("No movement events are available.", false) } else { view! { <ol>{events.items.into_iter().map(|event| { let evidence = serde_json::to_string_pretty(&event.evidence).unwrap_or_else(|_| "{}".into()); view! { <li><div><strong>{display::event_label(event.action)}</strong><span>{display::short_timestamp(&event.occurred_at)}</span></div><p>{event.reason.unwrap_or_else(|| format!("Result: {}", display::status_label(event.resulting_status)))}</p><small>{format!("Move revision {} · user #{} · request {}", event.move_revision.get(), event.actor_id, event.request_id)}</small><details><summary>"Evidence JSON"</summary><pre>{evidence}</pre></details></li> } }).collect_view()}</ol> }.into_any() }}</section>
        </section>
    }
    .into_any()
}

fn proof_panel(movement: &TenantCellMoveResponse) -> AnyView {
    let checkpoint = movement.latest_checkpoint.as_ref().map(|value| {
        view! { <article><header><strong>"Latest checkpoint"</strong><span>{display::short_timestamp(&value.recorded_at)}</span></header><dl><div><dt>"Source LSN"</dt><dd>{value.checkpoint.source_lsn.clone()}</dd></div><div><dt>"Replay LSN"</dt><dd>{value.checkpoint.target_replay_lsn.clone()}</dd></div><div><dt>"Rows"</dt><dd>{value.checkpoint.copied_row_count}</dd></div><div><dt>"Bytes"</dt><dd>{value.checkpoint.copied_bytes}</dd></div></dl></article> }.into_any()
    });
    let validation = movement.validation.as_ref().map(|value| {
        let matched = value.validation.source_row_count == value.validation.target_row_count
            && value.validation.source_data_checksum == value.validation.target_data_checksum
            && value.validation.source_schema_checksum == value.validation.target_schema_checksum
            && value.validation.source_object_manifest_checksum == value.validation.target_object_manifest_checksum;
        view! { <article><header><strong>"Validation"</strong><span>{display::short_timestamp(&value.validated_at)}</span></header><dl><div><dt>"Tool"</dt><dd>{value.validation.tool_version.clone()}</dd></div><div><dt>"Data match"</dt><dd>{if matched { "Matched" } else { "Mismatch" }}</dd></div><div><dt>"Inventory"</dt><dd>{yes_no(value.validation.inventory_reconciled)}</dd></div><div><dt>"Outbox / idempotency"</dt><dd>{format!("{} / {}", yes_no(value.validation.outbox_verified), yes_no(value.validation.idempotency_verified))}</dd></div></dl></article> }.into_any()
    });
    let verification = movement.cutover_verification.as_ref().map(|value| {
        view! { <article><header><strong>"Cutover verification"</strong><span>{display::short_timestamp(&value.verified_at)}</span></header><dl><div><dt>"Observed cell"</dt><dd>{value.verification.observed_data_cell_id}</dd></div><div><dt>"Routing"</dt><dd>{yes_no(value.verification.routing_verified)}</dd></div><div><dt>"Target read"</dt><dd>{yes_no(value.verification.target_read_verified)}</dd></div><div><dt>"Write fence"</dt><dd>{yes_no(value.verification.write_fence_verified)}</dd></div></dl></article> }.into_any()
    });
    let rollback_verification = movement.rollback_verification.as_ref().map(|value| {
        view! { <article><header><strong>"Rollback safety proof"</strong><span>{display::short_timestamp(&value.verified_at)}</span></header><dl><div><dt>"Observed source cell"</dt><dd>{value.verification.observed_data_cell_id}</dd></div><div><dt>"Expected placement revision"</dt><dd>{value.verification.expected_rollback_placement_revision.get()}</dd></div><div><dt>"Routing / source read"</dt><dd>{format!("{} / {}", yes_no(value.verification.routing_verified), yes_no(value.verification.source_read_verified))}</dd></div><div><dt>"Fence / inventory"</dt><dd>{format!("{} / {}", yes_no(value.verification.write_fence_verified), yes_no(value.verification.inventory_reconciled))}</dd></div><div><dt>"Idempotency / outbox"</dt><dd>{format!("{} / {}", yes_no(value.verification.idempotency_verified), yes_no(value.verification.outbox_verified))}</dd></div><div><dt>"Tool / route"</dt><dd>{format!("{} / {}", value.verification.tool_version, value.verification.routing_reference)}</dd></div></dl></article> }.into_any()
    });
    view! {
        <section class="cell-move-proof"><header><div><h3>"Operational proof"</h3><span>"The latest artifacts attached to this move."</span></div></header><div>{checkpoint.unwrap_or_else(|| missing_proof("Checkpoint not recorded"))}{validation.unwrap_or_else(|| missing_proof("Validation not recorded"))}{verification.unwrap_or_else(|| missing_proof("Cutover verification not recorded"))}{rollback_verification.unwrap_or_else(|| missing_proof("Rollback safety proof not recorded"))}</div></section>
    }
    .into_any()
}

fn missing_proof(label: &'static str) -> AnyView {
    view! { <article class="missing"><strong>{label}</strong><span>"A lifecycle action remains blocked until this evidence is accepted."</span></article> }.into_any()
}

const fn yes_no(value: bool) -> &'static str {
    if value {
        "Yes"
    } else {
        "No"
    }
}

fn invalidate_detail(signals: Signals) {
    signals.detail_generation.update(|value| *value += 1);
    signals.event_generation.update(|value| *value += 1);
    signals.selected_id.set(None);
    signals.selected.set(None);
    signals
        .events
        .set(TenantCellMoveEventPage::new(Vec::new(), None));
}

fn refresh(signals: Signals) {
    let selected_id = signals.selected_id.get_untracked();
    load_page(signals, None, false);
    if let Some(selected_id) = selected_id {
        load_detail(signals, selected_id);
    }
}

fn load_page(signals: Signals, cursor: Option<OpaqueCursor>, append: bool) {
    let tenant_id = match optional_positive_i64(&signals.tenant_id.get_untracked(), "tenant ID") {
        Ok(value) => value,
        Err(message) => return signals.error.set(Some(message)),
    };
    let data_cell_id =
        match optional_positive_i64(&signals.data_cell_id.get_untracked(), "data-cell ID") {
            Ok(value) => value,
            Err(message) => return signals.error.set(Some(message)),
        };
    signals.list_generation.update(|value| *value += 1);
    let generation = signals.list_generation.get_untracked();
    signals.loading.set(true);
    signals.error.set(None);
    let request = TenantCellMovePageRequest {
        tenant_id,
        data_cell_id,
        status: signals.status.get_untracked(),
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::tenant_cell_moves(&request).await {
            Ok(page) if signals.list_generation.get_untracked() == generation => {
                if append {
                    signals.movements.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.movements.set(page);
                }
            }
            Err(error) if signals.list_generation.get_untracked() == generation => {
                handle_read_error(signals, error)
            }
            _ => {}
        }
        if signals.list_generation.get_untracked() == generation {
            signals.loading.set(false);
            signals.loaded.set(true);
        }
    });
}

fn load_detail(signals: Signals, id: i64) {
    signals.detail_generation.update(|value| *value += 1);
    let generation = signals.detail_generation.get_untracked();
    signals.detail_loading.set(true);
    signals.selected_id.set(Some(id));
    signals.selected.set(None);
    signals.event_generation.update(|value| *value += 1);
    signals
        .events
        .set(TenantCellMoveEventPage::new(Vec::new(), None));
    leptos::task::spawn_local(async move {
        match api::tenant_cell_move(id).await {
            Ok(movement) if signals.detail_generation.get_untracked() == generation => {
                signals.selected.set(Some(movement));
                load_events(signals, id, None, false);
            }
            Err(error) if signals.detail_generation.get_untracked() == generation => {
                handle_read_error(signals, error)
            }
            _ => {}
        }
        if signals.detail_generation.get_untracked() == generation {
            signals.detail_loading.set(false);
        }
    });
}

fn load_events(signals: Signals, id: i64, cursor: Option<OpaqueCursor>, append: bool) {
    signals.event_generation.update(|value| *value += 1);
    let generation = signals.event_generation.get_untracked();
    signals.events_loading.set(true);
    let request = TenantCellMoveEventPageRequest {
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::tenant_cell_move_events(id, &request).await {
            Ok(page) if signals.event_generation.get_untracked() == generation => {
                if append {
                    signals.events.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.events.set(page);
                }
            }
            Err(error) if signals.event_generation.get_untracked() == generation => {
                handle_read_error(signals, error)
            }
            _ => {}
        }
        if signals.event_generation.get_untracked() == generation {
            signals.events_loading.set(false);
        }
    });
}

pub(super) fn dispatch(signals: Signals, command: PendingCommand) {
    if signals.command_pending.get_untracked() {
        return;
    }
    if !signals.recovery_loaded.get_untracked() {
        return command_not_sent(
            signals,
            "Durable exact-retry recovery is still loading. Wait before sending the command."
                .into(),
        );
    }
    if let Err(error) = recovery::persist(signals.recovery_binding, &command) {
        return command_not_sent(
            signals,
            format!(
                "The command was not sent because durable exact-retry recovery failed: {error}"
            ),
        );
    }
    signals.recovery_error.set(None);
    signals.command_pending.set(true);
    signals.command_error.set(None);
    leptos::task::spawn_local(async move {
        match execute(&command).await {
            Ok(movement) => {
                signals.toasts.success("Tenant cell move updated.");
                clear_after_definitive_result(signals, &command);
                signals.dialog.set(None);
                signals.selected_id.set(Some(movement.tenant_cell_move_id));
                refresh(signals);
            }
            Err(error) => {
                if error.unauthorized {
                    signals.on_unauthorized.run(());
                }
                signals.command_error.set(Some(error.message.clone()));
                if error.ambiguous_outcome {
                    retain_recovery(signals, command);
                    signals.dialog.set(None);
                    signals.toasts.error(format!(
                        "{} Retry the exact saved command before starting another action.",
                        error.message
                    ));
                } else {
                    clear_after_definitive_result(signals, &command);
                }
            }
        }
        signals.command_pending.set(false);
    });
}

fn restore_recovery(signals: Signals) {
    match recovery::load(signals.recovery_binding.user_id) {
        Ok(stored) if !stored.is_empty() => {
            signals.retry.set(stored);
            signals.toasts.info(
                "Recovered unresolved tenant-cell-move commands. Resolve each exact retry before starting another command.",
            );
        }
        Ok(_) => {}
        Err(error) => signals.recovery_error.set(Some(format!(
            "Durable exact-retry recovery is unavailable: {error} Restore browser storage access and reload before sending a command."
        ))),
    }
    signals.recovery_loaded.set(true);
}

fn command_not_sent(signals: Signals, message: String) {
    if let Ok(stored) = recovery::load(signals.recovery_binding.user_id) {
        signals.retry.set(stored);
    }
    signals.command_error.set(Some(message.clone()));
    signals.recovery_error.set(Some(message.clone()));
    signals.toasts.error(message);
}

fn retain_recovery(signals: Signals, command: PendingCommand) {
    let fallback = recovery::StoredPendingCommand::new(signals.recovery_binding, command);
    match recovery::load(signals.recovery_binding.user_id) {
        Ok(mut stored) => {
            recovery::merge_record(&mut stored, fallback);
            signals.retry.set(stored);
        }
        Err(_) => signals
            .retry
            .update(|stored| recovery::merge_record(stored, fallback)),
    }
}

fn clear_after_definitive_result(signals: Signals, command: &PendingCommand) {
    match recovery::clear(signals.recovery_binding, command) {
        Ok(()) => match recovery::load(signals.recovery_binding.user_id) {
            Ok(stored) => {
                signals.retry.set(stored);
                signals.recovery_error.set(None);
            }
            Err(error) => {
                signals
                    .retry
                    .update(|stored| recovery::remove_record(stored, command));
                signals.recovery_error.set(Some(format!(
                        "The command recovery was cleared, but remaining recoveries could not be loaded: {error}"
                    )));
            }
        },
        Err(error) => {
            retain_recovery(signals, command.clone());
            let message = format!(
                "The command result is definitive, but its durable recovery could not be cleared: {error}"
            );
            signals.recovery_error.set(Some(message.clone()));
            signals.toasts.error(message);
        }
    }
}

const fn can_start_command(
    command_pending: bool,
    exact_retry_required: bool,
    recovery_loaded: bool,
    recovery_unavailable: bool,
) -> bool {
    recovery_loaded && !recovery_unavailable && !command_pending && !exact_retry_required
}

const fn retry_context_matches(stored_control_tenant_id: i64, current_tenant_id: i64) -> bool {
    stored_control_tenant_id == current_tenant_id
}

async fn execute(command: &PendingCommand) -> Result<TenantCellMoveResponse, api::ApiError> {
    match command {
        PendingCommand::Plan(tenant_id, request, key) => {
            api::plan_tenant_cell_move(*tenant_id, request, key).await
        }
        PendingCommand::StartCopy(id, request, key) => {
            api::start_tenant_cell_move_copy(*id, request, key).await
        }
        PendingCommand::Checkpoint(id, request, key) => {
            api::checkpoint_tenant_cell_move(*id, request, key).await
        }
        PendingCommand::Freeze(id, request, key) => {
            api::freeze_tenant_cell_move(*id, request, key).await
        }
        PendingCommand::Validate(id, request, key) => {
            api::validate_tenant_cell_move(*id, request, key).await
        }
        PendingCommand::Cutover(id, request, key) => {
            api::cutover_tenant_cell_move(*id, request, key).await
        }
        PendingCommand::VerifyCutover(id, request, key) => {
            api::verify_tenant_cell_move_cutover(*id, request, key).await
        }
        PendingCommand::Complete(id, request, key) => {
            api::complete_tenant_cell_move(*id, request, key).await
        }
        PendingCommand::Rollback(id, request, key) => {
            api::rollback_tenant_cell_move(*id, request, key).await
        }
        PendingCommand::Cancel(id, request, key) => {
            api::cancel_tenant_cell_move(*id, request, key).await
        }
    }
}

fn handle_read_error(signals: Signals, error: api::ApiError) {
    if error.unauthorized {
        signals.on_unauthorized.run(());
    }
    signals.error.set(Some(error.message));
}

fn optional_positive_i64(value: &str, label: &str) -> Result<Option<i64>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .map(Some)
        .ok_or_else(|| format!("Enter a positive {label}."))
}

fn state(label: &'static str, loading: bool) -> AnyView {
    view! { <section class="cell-move-state" aria-busy=loading><Show when=move || loading><span class="loading-line"></span></Show><strong>{label}</strong></section> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_recovery_must_be_ready_and_resolved_before_a_new_command() {
        assert!(!can_start_command(false, false, false, false));
        assert!(!can_start_command(false, false, true, true));
        assert!(!can_start_command(false, true, true, false));
        assert!(!can_start_command(true, false, true, false));
        assert!(can_start_command(false, false, true, false));
        assert!(retry_context_matches(23, 23));
        assert!(!retry_context_matches(23, 24));
    }
}
