use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CreateVendorReturnLineRequest, CreateVendorReturnRequest, InventoryBalanceResponse,
    VendorReturnLifecycleRequest, VendorReturnPageResponse, VendorReturnReason,
    VendorReturnResponse, VendorReturnStatus,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, PartialEq, Eq)]
struct DraftLine {
    balance: InventoryBalanceResponse,
    quantity: i64,
    reason: VendorReturnReason,
    note: Option<String>,
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build dispatches vendor-return commands")
)]
enum Command {
    Create(CreateVendorReturnRequest, String),
    Release(i64, VendorReturnLifecycleRequest, String),
    Ship(i64, VendorReturnLifecycleRequest, String),
    Cancel(i64, VendorReturnLifecycleRequest, String),
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "browser build consumes vendor-return command callbacks"
    )
)]
struct Signals {
    page: RwSignal<VendorReturnPageResponse>,
    selected: RwSignal<Option<VendorReturnResponse>>,
    selected_id: RwSignal<Option<i64>>,
    create_open: RwSignal<bool>,
    balances: RwSignal<Vec<InventoryBalanceResponse>>,
    loading: RwSignal<bool>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    retry: RwSignal<Option<Command>>,
    owner: RwSignal<Option<i64>>,
    facility: RwSignal<Option<i64>>,
    status: RwSignal<Option<VendorReturnStatus>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn VendorReturnsWorkspace(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let signals = Signals {
        page: RwSignal::new(VendorReturnPageResponse {
            items: Vec::new(),
            next_cursor: None,
        }),
        selected: RwSignal::new(None),
        selected_id: RwSignal::new(None),
        create_open: RwSignal::new(false),
        balances: RwSignal::new(Vec::new()),
        loading: RwSignal::new(false),
        pending: RwSignal::new(false),
        error: RwSignal::new(None),
        retry: RwSignal::new(None),
        owner: RwSignal::new(None),
        facility: RwSignal::new(None),
        status: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let access = StoredValue::new(access);
    let layout = SplitPaneState::new("vendor-returns", 700);
    Effect::new(move |_| {
        load_list(signals);
        load_balances(signals);
    });
    let select = move |id: i64| {
        signals.create_open.set(false);
        signals.selected_id.set(Some(id));
        layout.show_detail();
        load_detail(signals, id);
    };
    view! {
        <div class="vas-workspace vendor-return-workspace split-workspace" style=move ||layout.style() data-pane-mode=move ||layout.mode_attribute()>
            <h1 class="sr-only">"Vendor returns"</h1>
            <section class="data-section vas-list vendor-return-list split-master">
                <form class="vas-toolbar" on:submit=move |event|{event.prevent_default();load_list(signals)}>
                    <div class="toolbar-summary"><strong>{move ||signals.page.get().items.len()}</strong><span>"vendor returns"</span><PaneControls layout master_label="Return table" detail_label="Return detail"/></div>
                    <label><span class="sr-only">"Client"</span><select on:change=move |event|signals.owner.set(parse_id(&event_target_value(&event)))><option value="">"All clients"</option>{access.get_value().inventory_owners.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label>
                    <label><span class="sr-only">"Facility"</span><select on:change=move |event|signals.facility.set(parse_id(&event_target_value(&event)))><option value="">"All facilities"</option>{access.get_value().facilities.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label>
                    <label><span class="sr-only">"Status"</span><select on:change=move |event|signals.status.set(parse_status(&event_target_value(&event)))><option value="">"All statuses"</option><option value="draft">"Draft"</option><option value="released">"Released"</option><option value="shipped">"Shipped"</option><option value="cancelled">"Cancelled"</option></select></label>
                    <button class="button secondary-action compact" type="submit" disabled=move ||signals.loading.get()>"Apply"</button><button class="icon-button" type="button" title="Refresh returns" aria-label="Refresh returns" on:click=move |_|refresh(signals)><Icon icon=UiIcon::Refresh/></button><button class="button primary-action compact" type="button" on:click=move |_|{signals.create_open.set(true);signals.selected.set(None);signals.selected_id.set(None);signals.error.set(None);layout.show_detail()}><Icon icon=UiIcon::Add/><span>"New vendor return"</span></button>
                </form>
                <div class="table-scroll vendor-return-table-scroll"><table class="dense-table vas-table vendor-return-table"><caption class="sr-only">"Vendor returns matching the selected filters"</caption><thead><tr><th>"Return"</th><th>"Vendor"</th><th>"Status"</th><th class="numeric">"Units"</th><th>"Client"</th><th>"Facility"</th><th>"Updated"</th></tr></thead><tbody>{move ||signals.page.get().items.into_iter().map(|value|{let id=value.vendor_return_id;let units=value.lines.iter().map(|line|line.quantity).sum::<i64>();view!{<tr class:active-row=move ||signals.selected_id.get()==Some(id)&&!signals.create_open.get()><td><button class="row-link" type="button" on:click=move |_|select(id)>{value.number}</button><small>{value.vendor_reference.unwrap_or_else(||format!("Return #{id}"))}</small></td><td>{value.vendor_name}</td><td><span class=status_class(value.status)>{status_label(value.status)}</span></td><td class="numeric">{format_quantity(units)}</td><td>{value.inventory_owner_name}</td><td>{value.facility_name}</td><td>{short_timestamp(value.shipped_at.as_deref().or(value.cancelled_at.as_deref()).or(value.released_at.as_deref()).unwrap_or(&value.created_at))}</td></tr>}}).collect_view()}</tbody></table><Show when=move ||!signals.loading.get()&&signals.page.get().items.is_empty()><p class="empty-state">"No vendor returns match these filters."</p></Show></div>
                <footer class="table-pagination"><span>{move ||if signals.loading.get(){"Loading returns…".into()}else{format!("{} records",signals.page.get().items.len())}}</span></footer>
            </section>
            <SplitPaneHandle layout/>
            <section class="data-section vas-detail vendor-return-detail split-detail"><Show when=move ||signals.create_open.get() fallback=move ||view!{<Show when=move ||signals.selected.get().is_some() fallback=move ||view!{<div class="detail-empty"><h2>"Vendor return details"</h2><p>"Select a return to inspect reserved stock, outbound inventory evidence, billing, and history."</p></div>}>{move ||signals.selected.get().map(|value|view!{<ReturnDetail signals value/>})}</Show>}><CreateReturn signals access=access.get_value()/></Show></section>
        </div>
    }
}

#[component]
fn ReturnDetail(signals: Signals, value: VendorReturnResponse) -> impl IntoView {
    let note = RwSignal::new(String::new());
    let id = value.vendor_return_id;
    let revision = value.revision;
    let status = value.status;
    view! {<div class="vas-detail-content"><header class="detail-heading"><div><span class="eyebrow">{format!("Vendor return #{id}")}</span><h2>{value.number.clone()}</h2><p>{format!("{} · {} · {}",value.vendor_name,value.inventory_owner_name,value.facility_name)}</p></div><span class=status_class(status)>{status_label(status)}</span></header><dl class="summary-grid"><div><dt>"Vendor reference"</dt><dd>{value.vendor_reference.unwrap_or_else(||"Not supplied".into())}</dd></div><div><dt>"Revision"</dt><dd>{revision.get()}</dd></div><div><dt>"Inventory transaction"</dt><dd>{value.shipment_inventory_transaction_id.map_or_else(||"Pending".into(),|id|format!("#{id}"))}</dd></div><div><dt>"Billable event"</dt><dd>{value.billable_event_id.map_or_else(||"Not captured".into(),|id|format!("#{id}"))}</dd></div></dl>{value.note.map(|text|view!{<p class="vas-work-note">{text}</p>})}<section class="vas-recipe"><header><h3>"Return stock"</h3></header><div class="table-scroll"><table class="dense-table"><thead><tr><th>"Item"</th><th>"Stock identity"</th><th>"Reason"</th><th class="numeric">"Qty"</th><th>"Hold"</th></tr></thead><tbody>{value.lines.into_iter().map(|line|view!{<tr><td><strong>{line.item_description.unwrap_or_else(||format!("Item #{}",line.item_id))}</strong><small>{line.uom}</small></td><td>{line.location_code}<small>{identity(line.lot.as_deref(),line.serial.as_deref(),line.license_plate_number.as_deref())}</small></td><td>{reason_label(line.reason)}{line.note.map(|text|view!{<small>{text}</small>})}</td><td class="numeric">{format_quantity(line.quantity)}</td><td>{line.hold_id.map_or_else(||"—".into(),|id|format!("#{id}"))}</td></tr>}).collect_view()}</tbody></table></div></section><section class="vas-history"><header><h3>"Lifecycle evidence"</h3><span>{format!("{} events",value.events.len())}</span></header><ol>{value.events.into_iter().map(|event|view!{<li><span class="vas-history-marker"></span><div><strong>{status_label(event.to_status)}</strong><small>{format!("Revision {} · actor #{} · {}",event.resulting_revision.get(),event.actor_id,short_timestamp(&event.occurred_at))}</small>{event.note.map(|text|view!{<p>{text}</p>})}</div></li>}).collect_view()}</ol></section><Show when=move ||matches!(status,VendorReturnStatus::Draft|VendorReturnStatus::Released)><section class="vas-command-bar"><label><span>"Required action note"</span><input prop:value=move ||note.get() on:input=move |event|note.set(event_target_value(&event)) placeholder="Physical staging, carrier receipt, or cancellation evidence"/></label><div>{(status==VendorReturnStatus::Draft).then(||view!{<button class="button primary-action" type="button" disabled=move ||signals.pending.get()||note.get().trim().is_empty() on:click=move |_|dispatch(signals,Command::Release(id,VendorReturnLifecycleRequest{expected_revision:revision,note:note.get_untracked()},api::new_idempotency_key()))>"Release and reserve"</button>})}{(status==VendorReturnStatus::Released).then(||view!{<button class="button primary-action" type="button" disabled=move ||signals.pending.get()||note.get().trim().is_empty() on:click=move |_|dispatch(signals,Command::Ship(id,VendorReturnLifecycleRequest{expected_revision:revision,note:note.get_untracked()},api::new_idempotency_key()))>"Confirm shipment"</button>})}<button class="button danger-action" type="button" disabled=move ||signals.pending.get()||note.get().trim().is_empty() on:click=move |_|dispatch(signals,Command::Cancel(id,VendorReturnLifecycleRequest{expected_revision:revision,note:note.get_untracked()},api::new_idempotency_key()))>"Cancel return"</button></div></section></Show><Feedback signals/></div>}
}

#[component]
fn CreateReturn(signals: Signals, access: AccessScopeWorkspace) -> impl IntoView {
    let owner = RwSignal::new(access.inventory_owners.first().map(|v| v.id));
    let facility = RwSignal::new(access.facilities.first().map(|v| v.id));
    let number = RwSignal::new(String::new());
    let vendor = RwSignal::new(String::new());
    let reference = RwSignal::new(String::new());
    let note = RwSignal::new(String::new());
    let balance_id = RwSignal::new(None::<i64>);
    let quantity = RwSignal::new(String::new());
    let reason = RwSignal::new(VendorReturnReason::Defective);
    let line_note = RwSignal::new(String::new());
    let lines = RwSignal::new(Vec::<DraftLine>::new());
    let form_error = RwSignal::new(None::<String>);
    let options = move || {
        signals
            .balances
            .get()
            .into_iter()
            .filter(|balance| {
                balance.quantity.available > 0
                    && owner
                        .get()
                        .is_none_or(|id| balance.inventory_owner_id == id)
                    && facility.get().is_none_or(|id| balance.facility_id == id)
            })
            .map(|balance| view! {<option value=balance.id>{balance_label(&balance)}</option>})
            .collect_view()
    };
    let add = move |_| {
        let Some(id) = balance_id.get_untracked() else {
            form_error.set(Some("Choose stock to return.".into()));
            return;
        };
        let Ok(qty) = quantity.get_untracked().parse::<i64>() else {
            form_error.set(Some("Enter a whole quantity.".into()));
            return;
        };
        let Some(balance) = signals
            .balances
            .get_untracked()
            .into_iter()
            .find(|value| value.id == id)
        else {
            form_error.set(Some("Refresh inventory and choose the stock again.".into()));
            return;
        };
        if qty <= 0 || qty > balance.quantity.available {
            form_error.set(Some(format!(
                "Quantity must be between 1 and {}.",
                balance.quantity.available
            )));
            return;
        }
        if lines
            .get_untracked()
            .iter()
            .any(|line| line.balance.id == id)
        {
            form_error.set(Some("That balance is already on the return.".into()));
            return;
        }
        let reason_value = reason.get_untracked();
        let note_value = nonblank(line_note.get_untracked());
        if reason_value == VendorReturnReason::Other && note_value.is_none() {
            form_error.set(Some("Other requires a line note.".into()));
            return;
        }
        lines.update(|values| {
            values.push(DraftLine {
                balance,
                quantity: qty,
                reason: reason_value,
                note: note_value,
            })
        });
        quantity.set(String::new());
        line_note.set(String::new());
        form_error.set(None)
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
        let vendor_value = vendor.get_untracked().trim().to_owned();
        let line_values = lines.get_untracked();
        if number_value.is_empty() || vendor_value.is_empty() {
            form_error.set(Some("Return number and vendor are required.".into()));
            return;
        }
        if line_values.is_empty() {
            form_error.set(Some("Add at least one stock line.".into()));
            return;
        }
        dispatch(
            signals,
            Command::Create(
                CreateVendorReturnRequest {
                    inventory_owner_id: owner_id,
                    facility_id,
                    number: number_value,
                    vendor_name: vendor_value,
                    vendor_reference: nonblank(reference.get_untracked()),
                    note: nonblank(note.get_untracked()),
                    lines: line_values
                        .into_iter()
                        .map(|line| CreateVendorReturnLineRequest {
                            inventory_balance_id: line.balance.id,
                            quantity: line.quantity,
                            reason: line.reason,
                            note: line.note,
                        })
                        .collect(),
                },
                api::new_idempotency_key(),
            ),
        )
    };
    view! {
        <form class="vas-create vendor-return-create" on:submit=submit>
            <header class="detail-heading"><div><span class="eyebrow">"Controlled outbound stock"</span><h2>"New vendor return"</h2><p>"Authorize exact inventory identities before reserving or shipping them."</p></div><button class="icon-button" type="button" title="Close" aria-label="Close" on:click=move |_|signals.create_open.set(false)><Icon icon=UiIcon::Close/></button></header>
            <section class="vendor-return-form-section" aria-labelledby="vendor-return-details-title">
                <div class="vendor-return-section-heading"><div><span class="eyebrow">"Return scope"</span><h3 id="vendor-return-details-title">"Vendor authorization"</h3></div><p>"Identify the client, destination, and supplier document."</p></div>
                <div class="vendor-return-field-grid">
                    <label><span>"Client"</span><select prop:value=move ||option_value(owner.get()) on:change=move |event|{owner.set(parse_id(&event_target_value(&event)));lines.set(Vec::new())}><option value="">"Choose client"</option>{access.inventory_owners.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label>
                    <label><span>"Facility"</span><select prop:value=move ||option_value(facility.get()) on:change=move |event|{facility.set(parse_id(&event_target_value(&event)));lines.set(Vec::new())}><option value="">"Choose facility"</option>{access.facilities.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label>
                    <label><span>"Return number"</span><input prop:value=move ||number.get() on:input=move |event|number.set(event_target_value(&event)) placeholder="RTV-2026-0001"/></label>
                    <label><span>"Vendor"</span><input prop:value=move ||vendor.get() on:input=move |event|vendor.set(event_target_value(&event)) placeholder="Supplier legal or trading name"/></label>
                    <label class="vendor-return-wide-field"><span>"Vendor reference"</span><input prop:value=move ||reference.get() on:input=move |event|reference.set(event_target_value(&event)) placeholder="RGA / RMA"/></label>
                    <label class="vendor-return-wide-field"><span>"Return instructions"</span><textarea prop:value=move ||note.get() on:input=move |event|note.set(event_target_value(&event)) placeholder="Handling, packaging, or carrier instructions"></textarea></label>
                </div>
            </section>
            <section class="vendor-return-form-section vendor-return-line-builder" aria-labelledby="vendor-return-stock-title">
                <div class="vendor-return-section-heading"><div><span class="eyebrow">"Line authorization"</span><h3 id="vendor-return-stock-title">"Return stock"</h3></div><span class="vendor-return-line-count">{move ||format!("{} lines",lines.get().len())}</span></div>
                <div class="vendor-return-line-fields">
                    <label class="vendor-return-stock-field"><span>"Available stock"</span><select prop:value=move ||option_value(balance_id.get()) on:change=move |event|balance_id.set(parse_id(&event_target_value(&event)))><option value="">"Choose available stock"</option>{options}</select></label>
                    <label class="vendor-return-reason-field"><span>"Reason"</span><select prop:value=move ||reason_wire(reason.get()) on:change=move |event|reason.set(parse_reason(&event_target_value(&event)))><option value="defective">"Defective"</option><option value="damaged">"Damaged"</option><option value="expired">"Expired"</option><option value="recall">"Recall"</option><option value="overstock">"Overstock"</option><option value="vendor_request">"Vendor request"</option><option value="other">"Other"</option></select></label>
                    <label class="vendor-return-quantity-field"><span>"Quantity"</span><input inputmode="numeric" prop:value=move ||quantity.get() on:input=move |event|quantity.set(event_target_value(&event)) placeholder="Qty"/></label>
                    <button class="button secondary-action compact vendor-return-add-line" type="button" on:click=add><Icon icon=UiIcon::Add/><span>"Add line"</span></button>
                    <label class="vendor-return-evidence-field"><span>"Line evidence"</span><input prop:value=move ||line_note.get() on:input=move |event|line_note.set(event_target_value(&event)) placeholder="Failure, recall, or authorization evidence"/></label>
                </div>
                <Show when=move ||!lines.get().is_empty() fallback=||view!{<p class="vendor-return-lines-empty">"Add available stock to build the vendor return."</p>}><ul class="vas-draft-lines vendor-return-draft-lines">{move ||lines.get().into_iter().enumerate().map(|(index,line)|view!{<li><span><strong>{balance_label(&line.balance)}</strong><small>{reason_label(line.reason)}</small></span><b>{format_quantity(line.quantity)}</b><button class="icon-button" type="button" title="Remove line" aria-label="Remove line" on:click=move |_|lines.update(|values|{values.remove(index);})><Icon icon=UiIcon::Close/></button></li>}).collect_view()}</ul></Show>
            </section>
            <Show when=move ||form_error.get().is_some()>{move ||form_error.get().map(|message|view!{<p class="inline-command-error" role="alert">{message}</p>})}</Show><Feedback signals/>
            <footer class="detail-actions"><button class="button quiet-action" type="button" on:click=move |_|signals.create_open.set(false)>"Discard"</button><button class="button primary-action" type="submit" disabled=move ||signals.pending.get()>"Create draft"</button></footer>
        </form>
    }
}

#[component]
fn Feedback(signals: Signals) -> impl IntoView {
    view! {<Show when=move ||signals.error.get().is_some()>{move ||signals.error.get().map(|message|view!{<div class="vas-command-recovery"><p class="inline-command-error" role="alert">{message}</p><Show when=move ||signals.retry.get().is_some()><button class="button secondary-action compact" type="button" disabled=move ||signals.pending.get() on:click=move |_|{if let Some(command)=signals.retry.get_untracked(){dispatch(signals,command)}}>"Retry same command"</button></Show></div>})}</Show>}
}

fn dispatch(signals: Signals, command: Command) {
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
            Command::Create(request, key) => api::create_vendor_return(request, key).await,
            Command::Release(id, request, key) => {
                api::release_vendor_return(*id, request, key).await
            }
            Command::Ship(id, request, key) => api::ship_vendor_return(*id, request, key).await,
            Command::Cancel(id, request, key) => api::cancel_vendor_return(*id, request, key).await,
        };
        signals.pending.set(false);
        match result {
            Ok(value) => {
                signals.retry.set(None);
                signals.create_open.set(false);
                signals.selected_id.set(Some(value.vendor_return_id));
                signals.selected.set(Some(value));
                signals
                    .toasts
                    .success("Vendor return updated with auditable stock evidence.");
                load_list(signals);
                load_balances(signals)
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None)
                }
                signals.error.set(Some(error.message.clone()));
                signals.toasts.error(error.message)
            }
        }
    })
}
fn refresh(signals: Signals) {
    load_list(signals);
    load_balances(signals);
    if let Some(id) = signals.selected_id.get_untracked() {
        load_detail(signals, id)
    }
}
fn load_list(signals: Signals) {
    signals.loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = signals;
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::vendor_returns(
            api::VendorReturnFilters {
                facility_id: signals.facility.get_untracked(),
                inventory_owner_id: signals.owner.get_untracked(),
                status: signals.status.get_untracked(),
            },
            None,
        )
        .await
        {
            Ok(page) => {
                signals.page.set(page);
                signals.error.set(None)
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
        signals.loading.set(false)
    })
}
fn load_detail(signals: Signals, id: i64) {
    signals.loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, id);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::vendor_return_detail(id).await {
            Ok(value) if signals.selected_id.get_untracked() == Some(id) => {
                signals.selected.set(Some(value))
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
        signals.loading.set(false)
    })
}
fn load_balances(signals: Signals) {
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
            Err(error) => signals.error.set(Some(error.message)),
        }
    })
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|id| *id > 0)
}
fn option_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}
fn nonblank(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn parse_status(value: &str) -> Option<VendorReturnStatus> {
    match value {
        "draft" => Some(VendorReturnStatus::Draft),
        "released" => Some(VendorReturnStatus::Released),
        "shipped" => Some(VendorReturnStatus::Shipped),
        "cancelled" => Some(VendorReturnStatus::Cancelled),
        _ => None,
    }
}
fn parse_reason(value: &str) -> VendorReturnReason {
    match value {
        "damaged" => VendorReturnReason::Damaged,
        "expired" => VendorReturnReason::Expired,
        "recall" => VendorReturnReason::Recall,
        "overstock" => VendorReturnReason::Overstock,
        "vendor_request" => VendorReturnReason::VendorRequest,
        "other" => VendorReturnReason::Other,
        _ => VendorReturnReason::Defective,
    }
}
const fn status_label(value: VendorReturnStatus) -> &'static str {
    match value {
        VendorReturnStatus::Draft => "Draft",
        VendorReturnStatus::Released => "Released",
        VendorReturnStatus::Shipped => "Shipped",
        VendorReturnStatus::Cancelled => "Cancelled",
    }
}
const fn status_class(value: VendorReturnStatus) -> &'static str {
    match value {
        VendorReturnStatus::Draft => "status-pill neutral",
        VendorReturnStatus::Released => "status-pill active",
        VendorReturnStatus::Shipped => "status-pill success",
        VendorReturnStatus::Cancelled => "status-pill danger",
    }
}
const fn reason_wire(value: VendorReturnReason) -> &'static str {
    match value {
        VendorReturnReason::Damaged => "damaged",
        VendorReturnReason::Defective => "defective",
        VendorReturnReason::Expired => "expired",
        VendorReturnReason::Recall => "recall",
        VendorReturnReason::Overstock => "overstock",
        VendorReturnReason::VendorRequest => "vendor_request",
        VendorReturnReason::Other => "other",
    }
}
const fn reason_label(value: VendorReturnReason) -> &'static str {
    match value {
        VendorReturnReason::Damaged => "Damaged",
        VendorReturnReason::Defective => "Defective",
        VendorReturnReason::Expired => "Expired",
        VendorReturnReason::Recall => "Recall",
        VendorReturnReason::Overstock => "Overstock",
        VendorReturnReason::VendorRequest => "Vendor request",
        VendorReturnReason::Other => "Other",
    }
}
fn short_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
fn identity(lot: Option<&str>, serial: Option<&str>, plate: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(v) = lot {
        parts.push(format!("lot {v}"))
    }
    if let Some(v) = serial {
        parts.push(format!("serial {v}"))
    }
    if let Some(v) = plate {
        parts.push(format!("LPN {v}"))
    }
    if parts.is_empty() {
        "untracked".into()
    } else {
        parts.join(" · ")
    }
}
fn balance_label(value: &InventoryBalanceResponse) -> String {
    format!(
        "{} · {} · {} available",
        value
            .item_description
            .clone()
            .unwrap_or_else(|| format!("Item #{}", value.item_id)),
        value
            .location_barcode
            .clone()
            .or_else(|| value.location_name.clone())
            .unwrap_or_else(|| format!("Location #{}", value.location_id)),
        value.quantity.available
    )
}
