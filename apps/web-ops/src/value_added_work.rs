use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CreateValueAddedWorkInputRequest, CreateValueAddedWorkOutputRequest,
    CreateValueAddedWorkRequest, InventoryBalanceResponse, OpaqueCursor, ValueAddedInventoryStatus,
    ValueAddedWorkKind, ValueAddedWorkLifecycleRequest, ValueAddedWorkPageResponse,
    ValueAddedWorkResponse, ValueAddedWorkStatus,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, PartialEq, Eq)]
struct DraftInput {
    balance: InventoryBalanceResponse,
    quantity: i64,
}

#[derive(Clone, PartialEq, Eq)]
struct DraftOutput {
    balance: InventoryBalanceResponse,
    status: ValueAddedInventoryStatus,
    quantity: i64,
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build dispatches value-added commands")
)]
enum PendingCommand {
    Create(CreateValueAddedWorkRequest, String),
    Release(i64, ValueAddedWorkLifecycleRequest, String),
    Complete(i64, ValueAddedWorkLifecycleRequest, String),
    Cancel(i64, ValueAddedWorkLifecycleRequest, String),
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "browser build consumes value-added command callbacks"
    )
)]
struct Signals {
    page: RwSignal<ValueAddedWorkPageResponse>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    cursor_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    owner_filter: RwSignal<Option<i64>>,
    facility_filter: RwSignal<Option<i64>>,
    status_filter: RwSignal<Option<ValueAddedWorkStatus>>,
    selected_id: RwSignal<Option<i64>>,
    selected: RwSignal<Option<ValueAddedWorkResponse>>,
    detail_loading: RwSignal<bool>,
    create_open: RwSignal<bool>,
    balances: RwSignal<Vec<InventoryBalanceResponse>>,
    balances_loading: RwSignal<bool>,
    pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn ValueAddedWorkWorkspace(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let signals = Signals {
        page: RwSignal::new(ValueAddedWorkPageResponse {
            items: Vec::new(),
            next_cursor: None,
        }),
        loading: RwSignal::new(false),
        load_error: RwSignal::new(None),
        generation: RwSignal::new(0),
        cursor: RwSignal::new(None),
        cursor_history: RwSignal::new(Vec::new()),
        owner_filter: RwSignal::new(None),
        facility_filter: RwSignal::new(None),
        status_filter: RwSignal::new(None),
        selected_id: RwSignal::new(None),
        selected: RwSignal::new(None),
        detail_loading: RwSignal::new(false),
        create_open: RwSignal::new(false),
        balances: RwSignal::new(Vec::new()),
        balances_loading: RwSignal::new(false),
        pending: RwSignal::new(false),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let layout = SplitPaneState::new("value-added-work", 700);
    let access = StoredValue::new(access);
    Effect::new(move |_| {
        load_page(signals, None, Vec::new());
        load_balances(signals);
    });

    let open_detail = move |work_id: i64| {
        signals.create_open.set(false);
        signals.selected_id.set(Some(work_id));
        layout.show_detail();
        load_detail(signals, work_id);
    };
    let refresh = move |_| refresh_workspace(signals);
    let previous = move |_| {
        if signals.loading.get_untracked() {
            return;
        }
        let mut history = signals.cursor_history.get_untracked();
        if let Some(cursor) = history.pop() {
            load_page(signals, cursor, history);
        }
    };
    let next = move |_| {
        if signals.loading.get_untracked() {
            return;
        }
        if let Some(cursor) = signals.page.get_untracked().next_cursor {
            let mut history = signals.cursor_history.get_untracked();
            history.push(signals.cursor.get_untracked());
            load_page(signals, Some(cursor), history);
        }
    };

    view! {
        <div class="vas-workspace split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <h1 class="sr-only">"Value-added work"</h1>
            <section class="data-section vas-list split-master">
                <form class="vas-toolbar" on:submit=move |event| { event.prevent_default(); load_page(signals, None, Vec::new()); }>
                    <div class="toolbar-summary"><strong>{move || signals.page.get().items.len()}</strong><span>"work orders"</span><PaneControls layout master_label="Work table" detail_label="Work detail"/></div>
                    <label><span class="sr-only">"Client"</span><select on:change=move |event| signals.owner_filter.set(parse_id(&event_target_value(&event)))><option value="">"All clients"</option>{access.get_value().inventory_owners.into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span class="sr-only">"Facility"</span><select on:change=move |event| signals.facility_filter.set(parse_id(&event_target_value(&event)))><option value="">"All facilities"</option>{access.get_value().facilities.into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span class="sr-only">"Status"</span><select on:change=move |event| signals.status_filter.set(parse_status(&event_target_value(&event)))><option value="">"All statuses"</option><option value="draft">"Draft"</option><option value="released">"Released"</option><option value="completed">"Completed"</option><option value="cancelled">"Cancelled"</option></select></label>
                    <button class="button secondary-action compact" type="submit" disabled=move || signals.loading.get()>"Apply"</button>
                    <button class="icon-button" type="button" title="Refresh value-added work" aria-label="Refresh value-added work" on:click=refresh disabled=move || signals.loading.get()><Icon icon=UiIcon::Refresh/></button>
                    <button class="button primary-action compact" type="button" on:click=move |_| { signals.create_open.set(true); signals.selected_id.set(None); signals.selected.set(None); signals.command_error.set(None); layout.show_detail(); }><Icon icon=UiIcon::Add/><span>"New work"</span></button>
                </form>
                <div class="table-scroll"><table class="dense-table vas-table"><thead><tr><th>"Work"</th><th>"Type"</th><th>"Status"</th><th>"Recipe"</th><th>"Client"</th><th>"Facility"</th><th>"Updated"</th></tr></thead><tbody>{move || signals.page.get().items.into_iter().map(|work| { let id=work.work_id; view! { <tr class:active-row=move || signals.selected_id.get()==Some(id) && !signals.create_open.get()><td><button class="row-link" type="button" on:click=move |_| open_detail(id)>{work.number}</button><small>{format!("#{} · rev {}",id,work.revision.get())}</small></td><td>{kind_label(work.kind)}</td><td><span class=status_class(work.status)>{status_label(work.status)}</span></td><td>{format!("{} in / {} out",work.inputs.len(),work.outputs.len())}</td><td>{work.inventory_owner_name}</td><td>{work.facility_name}</td><td>{short_timestamp(work.completed_at.as_deref().or(work.cancelled_at.as_deref()).or(work.released_at.as_deref()).unwrap_or(&work.created_at))}</td></tr> } }).collect_view()}</tbody></table><Show when=move || !signals.loading.get() && signals.page.get().items.is_empty()><p class="empty-state">"No value-added work matches these filters."</p></Show></div>
                <Show when=move || signals.load_error.get().is_some()>{move || signals.load_error.get().map(|message| view! { <p class="inline-command-error" role="alert">{message}</p> })}</Show>
                <footer class="table-pagination"><span>{move || if signals.loading.get() { "Loading work…".into() } else { format!("{} records on this page",signals.page.get().items.len()) }}</span><div><button class="button quiet-action compact" type="button" disabled=move || signals.loading.get() || signals.cursor_history.get().is_empty() on:click=previous>"Previous"</button><button class="button quiet-action compact" type="button" disabled=move || signals.loading.get() || signals.page.get().next_cursor.is_none() on:click=next>"Next"</button></div></footer>
            </section>
            <SplitPaneHandle layout/>
            <section class="data-section vas-detail split-detail">
                <Show when=move || signals.create_open.get() fallback=move || view! { <Show when=move || signals.selected.get().is_some() fallback=move || view! { <div class="detail-empty"><h2>"Work details"</h2><p>"Select a work order to inspect inventory reservations, journal evidence, billing, and immutable history."</p></div> }>{move || signals.selected.get().map(|work| view! { <WorkDetail signals work/> })}</Show> }>
                    <CreateWorkPanel signals access=access.get_value()/>
                </Show>
                <Show when=move || signals.detail_loading.get()><div class="panel-loading" role="status">"Loading work…"</div></Show>
            </section>
        </div>
    }
}

#[component]
fn WorkDetail(signals: Signals, work: ValueAddedWorkResponse) -> impl IntoView {
    let note = RwSignal::new(String::new());
    let id = work.work_id;
    let revision = work.revision;
    let status = work.status;
    view! {
        <div class="vas-detail-content">
            <header class="detail-heading"><div><span class="eyebrow">{format!("{} #{}",kind_label(work.kind),id)}</span><h2>{work.number.clone()}</h2><p>{format!("{} · {}",work.inventory_owner_name,work.facility_name)}</p></div><span class=status_class(status)>{status_label(status)}</span></header>
            <dl class="summary-grid"><div><dt>"Revision"</dt><dd>{revision.get()}</dd></div><div><dt>"Created"</dt><dd>{short_timestamp(&work.created_at)}</dd></div><div><dt>"Inventory transaction"</dt><dd>{work.completion_inventory_transaction_id.map_or_else(|| "Pending".into(),|id|format!("#{id}"))}</dd></div><div><dt>"Billable event"</dt><dd>{work.billable_event_id.map_or_else(|| "Not captured".into(),|id|format!("#{id}"))}</dd></div></dl>
            {work.note.map(|value| view! { <p class="vas-work-note">{value}</p> })}
            <RecipeTable title="Inputs" inputs=Some(work.inputs) outputs=None/>
            <RecipeTable title="Outputs" inputs=None outputs=Some(work.outputs)/>
            <section class="vas-history"><header><h3>"Lifecycle evidence"</h3><span>{format!("{} events",work.events.len())}</span></header><ol>{work.events.into_iter().map(|event| view! { <li><span class="vas-history-marker"></span><div><strong>{status_label(event.to_status)}</strong><small>{format!("Revision {} · actor #{} · {}",event.resulting_revision.get(),event.actor_id,short_timestamp(&event.occurred_at))}</small>{event.note.map(|value| view! { <p>{value}</p> })}</div></li> }).collect_view()}</ol></section>
            <Show when=move || matches!(status,ValueAddedWorkStatus::Draft|ValueAddedWorkStatus::Released)>
                <section class="vas-command-bar"><label><span>"Required action note"</span><input prop:value=move || note.get() on:input=move |event| note.set(event_target_value(&event)) placeholder="Record the physical verification or cancellation reason"/></label><div>
                    {(status==ValueAddedWorkStatus::Draft).then(|| view! { <button class="button primary-action" type="button" disabled=move || signals.pending.get() || note.get().trim().is_empty() on:click=move |_| dispatch(signals,PendingCommand::Release(id,ValueAddedWorkLifecycleRequest{expected_revision:revision,note:note.get_untracked()},api::new_idempotency_key()))>"Release and reserve"</button> })}
                    {(status==ValueAddedWorkStatus::Released).then(|| view! { <button class="button primary-action" type="button" disabled=move || signals.pending.get() || note.get().trim().is_empty() on:click=move |_| dispatch(signals,PendingCommand::Complete(id,ValueAddedWorkLifecycleRequest{expected_revision:revision,note:note.get_untracked()},api::new_idempotency_key()))>"Complete work"</button> })}
                    <button class="button danger-action" type="button" disabled=move || signals.pending.get() || note.get().trim().is_empty() on:click=move |_| dispatch(signals,PendingCommand::Cancel(id,ValueAddedWorkLifecycleRequest{expected_revision:revision,note:note.get_untracked()},api::new_idempotency_key()))>"Cancel work"</button>
                </div></section>
            </Show>
            <CommandFeedback signals/>
        </div>
    }
}

#[component]
fn RecipeTable(
    title: &'static str,
    inputs: Option<Vec<wareboxes_api_contract::v1::ValueAddedWorkInputResponse>>,
    outputs: Option<Vec<wareboxes_api_contract::v1::ValueAddedWorkOutputResponse>>,
) -> impl IntoView {
    let input_rows = inputs.unwrap_or_default().into_iter().map(|line| view! { <tr><td><strong>{line.item_description.unwrap_or_else(||format!("Item #{}",line.item_id))}</strong><small>{line.uom}</small></td><td>{line.location_code}<small>{identity_label(line.lot.as_deref(),line.serial.as_deref(),line.license_plate_number.as_deref())}</small></td><td>{inventory_status_label(line.inventory_status)}</td><td class="numeric">{format_quantity(line.quantity)}</td><td>{line.hold_id.map_or_else(||"—".into(),|id|format!("#{id}"))}</td></tr> }).collect_view();
    let output_rows = outputs.unwrap_or_default().into_iter().map(|line| view! { <tr><td><strong>{line.item_description.unwrap_or_else(||format!("Item #{}",line.item_id))}</strong><small>{line.uom}</small></td><td>{line.location_code}<small>{identity_label(line.lot.as_deref(),line.serial.as_deref(),line.license_plate_number.as_deref())}</small></td><td>{inventory_status_label(line.inventory_status)}</td><td class="numeric">{format_quantity(line.quantity)}</td><td>"—"</td></tr> }).collect_view();
    view! { <section class="vas-recipe"><header><h3>{title}</h3></header><div class="table-scroll"><table class="dense-table"><thead><tr><th>"Item"</th><th>"Stock identity"</th><th>"Disposition"</th><th class="numeric">"Quantity"</th><th>"Hold"</th></tr></thead><tbody>{input_rows}{output_rows}</tbody></table></div></section> }
}

#[component]
fn CreateWorkPanel(signals: Signals, access: AccessScopeWorkspace) -> impl IntoView {
    let owner = RwSignal::new(access.inventory_owners.first().map(|v| v.id));
    let facility = RwSignal::new(access.facilities.first().map(|v| v.id));
    let number = RwSignal::new(String::new());
    let kind = RwSignal::new(ValueAddedWorkKind::ValueAddedService);
    let note = RwSignal::new(String::new());
    let input_balance = RwSignal::new(None::<i64>);
    let input_qty = RwSignal::new(String::new());
    let output_balance = RwSignal::new(None::<i64>);
    let output_qty = RwSignal::new(String::new());
    let output_status = RwSignal::new(ValueAddedInventoryStatus::Available);
    let inputs = RwSignal::new(Vec::<DraftInput>::new());
    let outputs = RwSignal::new(Vec::<DraftOutput>::new());
    let form_error = RwSignal::new(None::<String>);
    let visible_balances = move || {
        signals
            .balances
            .get()
            .into_iter()
            .filter(|balance| {
                owner
                    .get()
                    .is_none_or(|id| balance.inventory_owner_id == id)
                    && facility.get().is_none_or(|id| balance.facility_id == id)
            })
            .collect::<Vec<_>>()
    };
    let input_options = move || {
        visible_balances()
            .into_iter()
            .filter(|balance| balance.quantity.available > 0)
            .map(|balance| {
                let label = balance_label(&balance);
                view! { <option value=balance.id>{label}</option> }
            })
            .collect_view()
    };
    let output_options = move || {
        visible_balances()
            .into_iter()
            .map(|balance| {
                let label = balance_label(&balance);
                view! { <option value=balance.id>{label}</option> }
            })
            .collect_view()
    };
    let add_input = move |_| match draft_line(
        input_balance.get_untracked(),
        &input_qty.get_untracked(),
        signals.balances.get_untracked(),
    ) {
        Ok(line)
            if inputs
                .get_untracked()
                .iter()
                .any(|current| current.balance.id == line.balance.id) =>
        {
            form_error.set(Some("That input balance is already in the recipe.".into()))
        }
        Ok(line) if line.quantity > line.balance.quantity.available => {
            form_error.set(Some(format!(
                "Only {} is available on that balance.",
                line.balance.quantity.available
            )))
        }
        Ok(line) => {
            inputs.update(|values| values.push(line));
            input_qty.set(String::new());
            form_error.set(None);
        }
        Err(message) => form_error.set(Some(message)),
    };
    let add_output = move |_| match draft_line(
        output_balance.get_untracked(),
        &output_qty.get_untracked(),
        signals.balances.get_untracked(),
    ) {
        Ok(line)
            if outputs.get_untracked().iter().any(|current| {
                current.balance.id == line.balance.id
                    && current.status == output_status.get_untracked()
            }) =>
        {
            form_error.set(Some(
                "That output stock identity and disposition is already in the recipe.".into(),
            ))
        }
        Ok(line) => {
            outputs.update(|values| {
                values.push(DraftOutput {
                    balance: line.balance,
                    quantity: line.quantity,
                    status: output_status.get_untracked(),
                })
            });
            output_qty.set(String::new());
            form_error.set(None);
        }
        Err(message) => form_error.set(Some(message)),
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(owner_id) = owner.get_untracked() else {
            form_error.set(Some("Choose a client.".into()));
            return;
        };
        let Some(facility_id) = facility.get_untracked() else {
            form_error.set(Some("Choose a facility.".into()));
            return;
        };
        let number_value = number.get_untracked().trim().to_owned();
        if number_value.is_empty() {
            form_error.set(Some("Enter a work number.".into()));
            return;
        }
        let input_values = inputs.get_untracked();
        let output_values = outputs.get_untracked();
        if let Err(message) = validate_recipe(kind.get_untracked(), &input_values, &output_values) {
            form_error.set(Some(message));
            return;
        }
        let request = CreateValueAddedWorkRequest {
            inventory_owner_id: owner_id,
            facility_id,
            number: number_value,
            kind: kind.get_untracked(),
            note: nonblank(note.get_untracked()),
            inputs: input_values
                .into_iter()
                .map(|line| CreateValueAddedWorkInputRequest {
                    inventory_balance_id: line.balance.id,
                    quantity: line.quantity,
                })
                .collect(),
            outputs: output_values
                .into_iter()
                .map(|line| CreateValueAddedWorkOutputRequest {
                    location_id: line.balance.location_id,
                    license_plate_id: line.balance.license_plate_id,
                    item_batch_id: line.balance.item_batch_id,
                    inventory_status: line.status,
                    quantity: line.quantity,
                })
                .collect(),
        };
        dispatch(
            signals,
            PendingCommand::Create(request, api::new_idempotency_key()),
        );
    };
    view! {
        <form class="vas-create" on:submit=submit>
            <header class="detail-heading"><div><span class="eyebrow">"Controlled inventory conversion"</span><h2>"New value-added work"</h2><p>"Define the exact source stock and resulting stock identity before release."</p></div><button class="icon-button" type="button" title="Close" aria-label="Close" on:click=move |_| signals.create_open.set(false)><Icon icon=UiIcon::Close/></button></header>
            <div class="vas-form-grid"><label><span>"Client"</span><select prop:value=move || option_value(owner.get()) on:change=move |event| { owner.set(parse_id(&event_target_value(&event))); inputs.set(Vec::new()); outputs.set(Vec::new()); }><option value="">"Choose client"</option>{access.inventory_owners.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Facility"</span><select prop:value=move || option_value(facility.get()) on:change=move |event| { facility.set(parse_id(&event_target_value(&event))); inputs.set(Vec::new()); outputs.set(Vec::new()); }><option value="">"Choose facility"</option>{access.facilities.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Work number"</span><input prop:value=move ||number.get() on:input=move |event|number.set(event_target_value(&event)) placeholder="VAS-2026-0001"/></label><label><span>"Workflow"</span><select prop:value=move ||kind_wire(kind.get()) on:change=move |event|kind.set(parse_kind(&event_target_value(&event)))><option value="relabel">"Relabel"</option><option value="refurbishment">"Refurbishment"</option><option value="kit">"Kit"</option><option value="dekit">"De-kit"</option><option value="assembly">"Assembly"</option><option value="value_added_service">"Value-added service"</option></select></label></div>
            <label class="vas-note"><span>"Instructions"</span><textarea prop:value=move ||note.get() on:input=move |event|note.set(event_target_value(&event)) placeholder="Operator instructions and quality requirements"></textarea></label>
            <section class="vas-builder"><header><h3>"Inputs"</h3><span>{move ||format!("{} lines",inputs.get().len())}</span></header><div class="vas-line-editor"><select aria-label="Input stock" prop:value=move ||option_value(input_balance.get()) on:change=move |event|input_balance.set(parse_id(&event_target_value(&event)))><option value="">"Choose available stock"</option>{input_options}</select><input aria-label="Input quantity" inputmode="numeric" prop:value=move ||input_qty.get() on:input=move |event|input_qty.set(event_target_value(&event)) placeholder="Qty"/><button class="button secondary-action compact" type="button" on:click=add_input>"Add input"</button></div><ul class="vas-draft-lines">{move ||inputs.get().into_iter().enumerate().map(|(index,line)|view!{<li><span><strong>{balance_label(&line.balance)}</strong><small>{format!("{} available",line.balance.quantity.available)}</small></span><b>{format_quantity(line.quantity)}</b><button type="button" class="icon-button" title="Remove input" aria-label="Remove input" on:click=move |_|inputs.update(|values|{values.remove(index);})><Icon icon=UiIcon::Close/></button></li>}).collect_view()}</ul></section>
            <section class="vas-builder"><header><h3>"Outputs"</h3><span>{move ||format!("{} lines",outputs.get().len())}</span></header><p class="admin-help">"Output choices reuse a visible item batch, location, and optional LPN identity. Create required master data before planning the work."</p><div class="vas-line-editor vas-output-editor"><select aria-label="Output stock identity" prop:value=move ||option_value(output_balance.get()) on:change=move |event|output_balance.set(parse_id(&event_target_value(&event)))><option value="">"Choose output identity"</option>{output_options}</select><select aria-label="Output inventory status" prop:value=move ||inventory_status_wire(output_status.get()) on:change=move |event|output_status.set(parse_inventory_status(&event_target_value(&event)))><option value="available">"Available"</option><option value="hold">"Hold"</option><option value="quarantine">"Quarantine"</option><option value="damaged">"Damaged"</option></select><input aria-label="Output quantity" inputmode="numeric" prop:value=move ||output_qty.get() on:input=move |event|output_qty.set(event_target_value(&event)) placeholder="Qty"/><button class="button secondary-action compact" type="button" on:click=add_output>"Add output"</button></div><ul class="vas-draft-lines">{move ||outputs.get().into_iter().enumerate().map(|(index,line)|view!{<li><span><strong>{balance_label(&line.balance)}</strong><small>{inventory_status_label(line.status)}</small></span><b>{format_quantity(line.quantity)}</b><button type="button" class="icon-button" title="Remove output" aria-label="Remove output" on:click=move |_|outputs.update(|values|{values.remove(index);})><Icon icon=UiIcon::Close/></button></li>}).collect_view()}</ul></section>
            <Show when=move || signals.balances_loading.get()><p class="panel-loading" role="status">"Loading visible inventory identities…"</p></Show>
            <Show when=move ||form_error.get().is_some()>{move ||form_error.get().map(|message|view!{<p class="inline-command-error" role="alert">{message}</p>})}</Show>
            <CommandFeedback signals/>
            <footer class="detail-actions"><button class="button quiet-action" type="button" on:click=move |_|signals.create_open.set(false)>"Discard"</button><button class="button primary-action" type="submit" disabled=move ||signals.pending.get() || signals.balances_loading.get()>"Create draft"</button></footer>
        </form>
    }
}

#[component]
fn CommandFeedback(signals: Signals) -> impl IntoView {
    view! { <Show when=move ||signals.command_error.get().is_some()>{move ||signals.command_error.get().map(|message|view!{<div class="vas-command-recovery"><p class="inline-command-error" role="alert">{message}</p><Show when=move ||signals.retry.get().is_some()><button class="button secondary-action compact" type="button" disabled=move ||signals.pending.get() on:click=move |_|{if let Some(command)=signals.retry.get_untracked(){dispatch(signals,command)}}>"Retry same command"</button></Show></div>})}</Show> }
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
            PendingCommand::Create(request, key) => {
                api::create_value_added_work(request, key).await
            }
            PendingCommand::Release(id, request, key) => {
                api::release_value_added_work(*id, request, key).await
            }
            PendingCommand::Complete(id, request, key) => {
                api::complete_value_added_work(*id, request, key).await
            }
            PendingCommand::Cancel(id, request, key) => {
                api::cancel_value_added_work(*id, request, key).await
            }
        };
        signals.pending.set(false);
        match result {
            Ok(work) => {
                signals.retry.set(None);
                signals.command_error.set(None);
                signals.create_open.set(false);
                signals.selected_id.set(Some(work.work_id));
                signals.selected.set(Some(work));
                signals
                    .toasts
                    .success("Value-added work updated with auditable inventory evidence.");
                load_page(signals, None, Vec::new());
                load_balances(signals);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None);
                }
                signals.command_error.set(Some(error.message.clone()));
                signals.toasts.error(error.message);
            }
        }
    });
}

fn refresh_workspace(signals: Signals) {
    load_page(
        signals,
        signals.cursor.get_untracked(),
        signals.cursor_history.get_untracked(),
    );
    load_balances(signals);
    if let Some(id) = signals.selected_id.get_untracked() {
        load_detail(signals, id)
    }
}

fn load_page(signals: Signals, cursor: Option<OpaqueCursor>, history: Vec<Option<OpaqueCursor>>) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, cursor, history, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = api::value_added_work(
            api::ValueAddedWorkFilters {
                facility_id: signals.facility_filter.get_untracked(),
                inventory_owner_id: signals.owner_filter.get_untracked(),
                status: signals.status_filter.get_untracked(),
            },
            cursor.as_ref(),
        )
        .await;
        if signals.generation.get_untracked() != generation {
            return;
        }
        signals.loading.set(false);
        match result {
            Ok(page) => {
                signals.page.set(page);
                signals.cursor.set(cursor);
                signals.cursor_history.set(history);
                signals.load_error.set(None)
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.load_error.set(Some(error.message)),
        }
    });
}

fn load_detail(signals: Signals, work_id: i64) {
    signals.detail_loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, work_id);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::value_added_work_detail(work_id).await {
            Ok(work) if signals.selected_id.get_untracked() == Some(work_id) => {
                signals.selected.set(Some(work));
                signals.command_error.set(None)
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.command_error.set(Some(error.message)),
        }
        signals.detail_loading.set(false);
    });
}

fn load_balances(signals: Signals) {
    signals.balances_loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = signals;
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::sorted_movable_balances(
            None,
            wareboxes_api_contract::v1::InventoryBalanceSort::Position,
            wareboxes_api_contract::v1::InventorySortDirection::Ascending,
            None,
        )
        .await
        {
            Ok(page) => signals.balances.set(page.items),
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.command_error.set(Some(format!(
                "Inventory identities could not be loaded: {}",
                error.message
            ))),
        }
        signals.balances_loading.set(false);
    });
}

fn draft_line(
    balance_id: Option<i64>,
    quantity: &str,
    balances: Vec<InventoryBalanceResponse>,
) -> Result<DraftInput, String> {
    let id = balance_id.ok_or_else(|| "Choose an inventory balance.".to_owned())?;
    let quantity = quantity
        .trim()
        .parse::<i64>()
        .map_err(|_| "Enter a whole quantity greater than zero.".to_owned())?;
    if quantity <= 0 {
        return Err("Quantity must be greater than zero.".into());
    }
    let balance = balances
        .into_iter()
        .find(|value| value.id == id)
        .ok_or_else(|| "Refresh visible inventory and choose the balance again.".to_owned())?;
    Ok(DraftInput { balance, quantity })
}

fn validate_recipe(
    kind: ValueAddedWorkKind,
    inputs: &[DraftInput],
    outputs: &[DraftOutput],
) -> Result<(), String> {
    let shape = match kind {
        ValueAddedWorkKind::Relabel | ValueAddedWorkKind::Refurbishment => {
            inputs.len() == 1 && outputs.len() == 1
        }
        ValueAddedWorkKind::Kit | ValueAddedWorkKind::Assembly => {
            inputs.len() >= 2 && outputs.len() == 1
        }
        ValueAddedWorkKind::Dekit => inputs.len() == 1 && outputs.len() >= 2,
        ValueAddedWorkKind::ValueAddedService => !inputs.is_empty() && !outputs.is_empty(),
    };
    if !shape {
        return Err(match kind {
            ValueAddedWorkKind::Relabel | ValueAddedWorkKind::Refurbishment => {
                "Relabeling and refurbishment require exactly one input and one output."
            }
            ValueAddedWorkKind::Kit | ValueAddedWorkKind::Assembly => {
                "Kitting and assembly require at least two inputs and exactly one output."
            }
            ValueAddedWorkKind::Dekit => {
                "De-kitting requires exactly one input and at least two outputs."
            }
            ValueAddedWorkKind::ValueAddedService => {
                "Value-added service requires at least one input and one output."
            }
        }
        .into());
    }
    if matches!(
        kind,
        ValueAddedWorkKind::Relabel | ValueAddedWorkKind::Refurbishment
    ) && inputs.iter().map(|line| line.quantity).sum::<i64>()
        != outputs.iter().map(|line| line.quantity).sum::<i64>()
    {
        return Err("Relabeling and refurbishment must conserve quantity.".into());
    }
    Ok(())
}

fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}
fn option_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}
fn nonblank(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn parse_status(value: &str) -> Option<ValueAddedWorkStatus> {
    match value {
        "draft" => Some(ValueAddedWorkStatus::Draft),
        "released" => Some(ValueAddedWorkStatus::Released),
        "completed" => Some(ValueAddedWorkStatus::Completed),
        "cancelled" => Some(ValueAddedWorkStatus::Cancelled),
        _ => None,
    }
}
fn parse_kind(value: &str) -> ValueAddedWorkKind {
    match value {
        "relabel" => ValueAddedWorkKind::Relabel,
        "refurbishment" => ValueAddedWorkKind::Refurbishment,
        "kit" => ValueAddedWorkKind::Kit,
        "dekit" => ValueAddedWorkKind::Dekit,
        "assembly" => ValueAddedWorkKind::Assembly,
        _ => ValueAddedWorkKind::ValueAddedService,
    }
}
fn parse_inventory_status(value: &str) -> ValueAddedInventoryStatus {
    match value {
        "hold" => ValueAddedInventoryStatus::Hold,
        "damaged" => ValueAddedInventoryStatus::Damaged,
        "quarantine" => ValueAddedInventoryStatus::Quarantine,
        _ => ValueAddedInventoryStatus::Available,
    }
}
const fn kind_wire(value: ValueAddedWorkKind) -> &'static str {
    match value {
        ValueAddedWorkKind::Relabel => "relabel",
        ValueAddedWorkKind::Refurbishment => "refurbishment",
        ValueAddedWorkKind::Kit => "kit",
        ValueAddedWorkKind::Dekit => "dekit",
        ValueAddedWorkKind::Assembly => "assembly",
        ValueAddedWorkKind::ValueAddedService => "value_added_service",
    }
}
const fn kind_label(value: ValueAddedWorkKind) -> &'static str {
    match value {
        ValueAddedWorkKind::Relabel => "Relabel",
        ValueAddedWorkKind::Refurbishment => "Refurbishment",
        ValueAddedWorkKind::Kit => "Kit",
        ValueAddedWorkKind::Dekit => "De-kit",
        ValueAddedWorkKind::Assembly => "Assembly",
        ValueAddedWorkKind::ValueAddedService => "Value-added service",
    }
}
const fn status_label(value: ValueAddedWorkStatus) -> &'static str {
    match value {
        ValueAddedWorkStatus::Draft => "Draft",
        ValueAddedWorkStatus::Released => "Released",
        ValueAddedWorkStatus::Completed => "Completed",
        ValueAddedWorkStatus::Cancelled => "Cancelled",
    }
}
const fn status_class(value: ValueAddedWorkStatus) -> &'static str {
    match value {
        ValueAddedWorkStatus::Draft => "status-pill neutral",
        ValueAddedWorkStatus::Released => "status-pill active",
        ValueAddedWorkStatus::Completed => "status-pill success",
        ValueAddedWorkStatus::Cancelled => "status-pill danger",
    }
}
const fn inventory_status_wire(value: ValueAddedInventoryStatus) -> &'static str {
    match value {
        ValueAddedInventoryStatus::Available => "available",
        ValueAddedInventoryStatus::Hold => "hold",
        ValueAddedInventoryStatus::Damaged => "damaged",
        ValueAddedInventoryStatus::Quarantine => "quarantine",
    }
}
const fn inventory_status_label(value: ValueAddedInventoryStatus) -> &'static str {
    match value {
        ValueAddedInventoryStatus::Available => "Available",
        ValueAddedInventoryStatus::Hold => "Hold",
        ValueAddedInventoryStatus::Damaged => "Damaged",
        ValueAddedInventoryStatus::Quarantine => "Quarantine",
    }
}
fn short_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
fn balance_label(balance: &InventoryBalanceResponse) -> String {
    format!(
        "{} · {} · {}",
        balance
            .item_description
            .clone()
            .unwrap_or_else(|| format!("Item #{}", balance.item_id)),
        balance
            .location_barcode
            .clone()
            .or_else(|| balance.location_name.clone())
            .unwrap_or_else(|| format!("Location #{}", balance.location_id)),
        identity_label(
            balance.lot.as_deref(),
            balance.serial.as_deref(),
            balance.license_plate_barcode.as_deref()
        )
    )
}
fn identity_label(lot: Option<&str>, serial: Option<&str>, plate: Option<&str>) -> String {
    let mut values = Vec::new();
    if let Some(value) = lot {
        values.push(format!("lot {value}"))
    }
    if let Some(value) = serial {
        values.push(format!("serial {value}"))
    }
    if let Some(value) = plate {
        values.push(format!("LPN {value}"))
    }
    if values.is_empty() {
        "untracked".into()
    } else {
        values.join(" · ")
    }
}
