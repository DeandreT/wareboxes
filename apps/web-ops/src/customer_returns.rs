use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CancelCustomerReturnRequest, CancelCustomerReturnResponse, CreateCustomerReturnLineRequest,
    CreateCustomerReturnRequest, CustomerReturnCancellationReason, CustomerReturnDetailResponse,
    CustomerReturnExecutionStatus, CustomerReturnPage, CustomerReturnReason, CustomerReturnStatus,
    OpaqueCursor, PlanCustomerReturnLoadRequest, PlanCustomerReturnLoadResponse,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::Location;

use crate::api;
use crate::components::{Icon, SearchField, UiIcon};
use crate::fulfillment_shared::{optional_text, parse_optional_timestamp};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, Copy)]
struct PageSignals {
    page: RwSignal<Option<CustomerReturnPage>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    current_cursor: RwSignal<Option<OpaqueCursor>>,
    cursor_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    facility: RwSignal<String>,
    owner: RwSignal<String>,
    status: RwSignal<String>,
    search: RwSignal<String>,
    on_unauthorized: Callback<()>,
}

#[derive(Clone, PartialEq, Eq)]
struct DraftLine {
    item_id: i64,
    description: String,
    uom: String,
    quantity: i64,
    reason: CustomerReturnReason,
    note: Option<String>,
    lot: Option<String>,
    serial: Option<String>,
}

#[component]
pub(crate) fn CustomerReturnsWorkspace(
    access: AccessScopeWorkspace,
    locations: Vec<Location>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let page = RwSignal::new(None::<CustomerReturnPage>);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let generation = RwSignal::new(0_u64);
    let current_cursor = RwSignal::new(None::<OpaqueCursor>);
    let cursor_history = RwSignal::new(Vec::<Option<OpaqueCursor>>::new());
    let facility = RwSignal::new(String::new());
    let owner = RwSignal::new(String::new());
    let status = RwSignal::new(String::new());
    let search = RwSignal::new(String::new());
    let selected_id = RwSignal::new(None::<i64>);
    let selected = RwSignal::new(None::<CustomerReturnDetailResponse>);
    let detail_loading = RwSignal::new(false);
    let detail_error = RwSignal::new(None::<String>);
    let detail_generation = RwSignal::new(0_u64);
    let create_open = RwSignal::new(false);
    let plan_open = RwSignal::new(false);
    let cancel_open = RwSignal::new(false);
    let layout = SplitPaneState::new("customer-returns", 720);
    let scoped_locations = StoredValue::new(locations);
    let signals = PageSignals {
        page,
        loading,
        error,
        generation,
        current_cursor,
        cursor_history,
        facility,
        owner,
        status,
        search,
        on_unauthorized,
    };
    Effect::new(move |_| request_page(signals, None, Vec::new()));

    let open_detail = move |return_id: i64| {
        create_open.set(false);
        plan_open.set(false);
        cancel_open.set(false);
        layout.show_detail();
        selected_id.set(Some(return_id));
        request_detail(
            return_id,
            selected_id,
            selected,
            detail_loading,
            detail_error,
            detail_generation,
            on_unauthorized,
        );
    };
    let refresh = move |_| {
        request_page(
            signals,
            current_cursor.get_untracked(),
            cursor_history.get_untracked(),
        );
        if let Some(return_id) = selected_id.get_untracked() {
            open_detail(return_id);
        }
    };
    let created = Callback::new(move |return_id: i64| {
        create_open.set(false);
        request_page(signals, None, Vec::new());
        open_detail(return_id);
    });
    let planned = Callback::new(move |result: PlanCustomerReturnLoadResponse| {
        plan_open.set(false);
        request_page(signals, None, Vec::new());
        open_detail(result.customer_return_id);
    });
    let cancelled = Callback::new(move |result: CancelCustomerReturnResponse| {
        cancel_open.set(false);
        request_page(signals, None, Vec::new());
        open_detail(result.customer_return_id);
    });
    let previous = move |_| {
        if loading.get_untracked() {
            return;
        }
        let mut history = cursor_history.get_untracked();
        if let Some(cursor) = history.pop() {
            request_page(signals, cursor, history);
        }
    };
    let next = move |_| {
        if loading.get_untracked() {
            return;
        }
        if let Some(cursor) = page.get_untracked().and_then(|value| value.next_cursor) {
            let mut history = cursor_history.get_untracked();
            history.push(current_cursor.get_untracked());
            request_page(signals, Some(cursor), history);
        }
    };

    view! {
        <div class="inbound-asn-workspace customer-return-workspace split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <h1 class="sr-only">"Customer returns"</h1>
            <section class="data-section inbound-asn-list split-master">
                <form class="inbound-asn-toolbar" on:submit=move |event| { event.prevent_default(); request_page(signals, None, Vec::new()); }>
                    <div class="toolbar-summary"><strong>{move || page.get().map_or(0, |value| value.items.len())}</strong><span>"returns"</span><PaneControls layout master_label="Return table" detail_label="Return detail"/></div>
                    <SearchField label="Search returns".to_owned() placeholder="Return or customer reference" value=search/>
                    <label><span class="sr-only">"Client"</span><select prop:value=move || owner.get() on:change=move |event| owner.set(event_target_value(&event))><option value="">"All clients"</option>{access.inventory_owners.clone().into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span class="sr-only">"Facility"</span><select prop:value=move || facility.get() on:change=move |event| facility.set(event_target_value(&event))><option value="">"All facilities"</option>{access.facilities.clone().into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span class="sr-only">"Status"</span><select prop:value=move || status.get() on:change=move |event| status.set(event_target_value(&event))><option value="">"All statuses"</option><option value="open">"Open"</option><option value="planned">"Planned"</option><option value="cancelled">"Cancelled"</option></select></label>
                    <button class="button secondary-action compact" type="submit" disabled=move || loading.get()>"Apply"</button>
                    <button class="icon-button" type="button" title="Refresh returns" aria-label="Refresh returns" disabled=move || loading.get() on:click=refresh><Icon icon=UiIcon::Refresh/></button>
                    <button class="button primary-action compact" type="button" on:click=move |_| { create_open.set(true); layout.show_detail(); }><Icon icon=UiIcon::Add/><span>"New return"</span></button>
                </form>
                <div class="table-scroll"><table class="dense-table inbound-asn-table"><thead><tr><th>"Return"</th><th>"Customer ref"</th><th>"Status"</th><th>"Execution"</th><th class="numeric">"Authorized"</th><th class="numeric">"Quarantined"</th><th class="numeric">"Open"</th><th>"Due"</th><th>"Client"</th><th>"Facility"</th></tr></thead><tbody>{move || page.get().map(|current| current.items.into_iter().map(|entry| { let return_id=entry.customer_return_id; view! { <tr class:active-row=move || selected_id.get()==Some(return_id) && !create_open.get()><td><button type="button" class="row-link" on:click=move |_| open_detail(return_id)>{entry.number}</button></td><td>{entry.customer_reference}</td><td><span class=status_class(entry.status)>{status_label(entry.status)}</span></td><td>{entry.execution_status.map(execution_label).unwrap_or("Not planned")}</td><td class="numeric">{format_quantity(entry.total_authorized_quantity)}</td><td class="numeric">{format_quantity(entry.total_rejected_quantity)}</td><td class="numeric"><strong>{format_quantity(entry.total_remaining_quantity)}</strong></td><td>{entry.expected_at.as_deref().map(short_timestamp).unwrap_or_else(|| "Not supplied".into())}</td><td>{entry.inventory_owner_name}</td><td>{entry.facility_name}</td></tr> } }).collect_view())}</tbody></table><Show when=move || !loading.get() && page.get().is_some_and(|value| value.items.is_empty())><p class="empty-state">"No customer returns match these filters."</p></Show></div>
                <Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
                <footer class="table-pagination"><span>{move || page.get().map_or_else(|| "No records".into(), |value| format!("{} records on this page", value.items.len()))}</span><div><button class="button quiet-action compact" type="button" disabled=move || loading.get() || cursor_history.get().is_empty() on:click=previous>"Previous"</button><button class="button quiet-action compact" type="button" disabled=move || loading.get() || page.get().and_then(|value| value.next_cursor).is_none() on:click=next>"Next"</button></div></footer>
            </section>
            <SplitPaneHandle layout/>
            <section class="data-section inbound-asn-detail split-detail">
                <Show when=move || create_open.get() fallback=move || view! { <Show when=move || selected.get().is_some() fallback=move || view! { <div class="detail-empty"><h2>"Return details"</h2><p>"Select a return to inspect authorization, quarantine progress, and its inbound load."</p></div> }>{move || selected.get().map(|detail| view! { <ReturnDetail detail=detail.clone() on_plan=Callback::new(move |_| plan_open.set(true)) on_cancel=Callback::new(move |_| cancel_open.set(true))/> })}</Show> }>
                    <CreateReturnPanel access=access.clone() on_close=Callback::new(move |_| create_open.set(false)) on_created=created on_unauthorized/>
                </Show>
                <Show when=move || detail_loading.get()><div class="panel-loading">"Loading return..."</div></Show>
                <Show when=move || detail_error.get().is_some()>{move || detail_error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
            </section>
        </div>
        <Show when=move || plan_open.get() && selected.get().is_some()>{move || selected.get().map(|detail| view! { <PlanReturnDialog detail locations=scoped_locations.get_value() on_close=Callback::new(move |_| plan_open.set(false)) on_planned=planned on_unauthorized/> })}</Show>
        <Show when=move || cancel_open.get() && selected.get().is_some()>{move || selected.get().map(|detail| view! { <CancelReturnDialog detail on_close=Callback::new(move |_| cancel_open.set(false)) on_cancelled=cancelled on_unauthorized/> })}</Show>
    }
}

#[component]
fn ReturnDetail(
    detail: CustomerReturnDetailResponse,
    on_plan: Callback<()>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let open = detail.summary.status == CustomerReturnStatus::Open;
    let cancellation = detail.summary.cancellation_reason.map(|reason| {
        let note=detail.summary.cancellation_note.clone();
        view! { <section class="asn-cancellation-evidence"><div><span>"Cancellation"</span><strong>{cancellation_label(reason)}</strong></div>{note.map(|value| view! { <p>{value}</p> })}</section> }
    });
    view! {
        <div class="inbound-asn-detail-content customer-return-detail-content">
            <header class="detail-heading"><div><span class="eyebrow">{format!("Return #{}",detail.summary.customer_return_id)}</span><h2>{detail.summary.number.clone()}</h2><p>{format!("Customer reference {}",detail.summary.customer_reference)}</p></div><span class=status_class(detail.summary.status)>{status_label(detail.summary.status)}</span></header>
            <dl class="summary-grid"><div><dt>"Client"</dt><dd>{detail.summary.inventory_owner_name}</dd></div><div><dt>"Facility"</dt><dd>{detail.summary.facility_name}</dd></div><div><dt>"Load state"</dt><dd>{detail.summary.execution_status.map(execution_label).unwrap_or("Not planned")}</dd></div><div><dt>"Due"</dt><dd>{detail.summary.expected_at.as_deref().map(short_timestamp).unwrap_or_else(|| "Not supplied".into())}</dd></div><div><dt>"Authorized"</dt><dd>{format_quantity(detail.summary.total_authorized_quantity)}</dd></div><div><dt>"Quarantined"</dt><dd>{format_quantity(detail.summary.total_rejected_quantity)}</dd></div><div><dt>"Missing"</dt><dd>{format_quantity(detail.summary.total_missing_quantity)}</dd></div><div><dt>"Open"</dt><dd><strong>{format_quantity(detail.summary.total_remaining_quantity)}</strong></dd></div></dl>
            <section class="customer-return-lines" aria-labelledby="customer-return-lines-title">
                <div class="detail-section-heading"><h3 id="customer-return-lines-title">"Authorized items"</h3><span>{format!("{} lines",detail.lines.len())}</span></div>
                <div class="table-scroll customer-return-line-scroll">
                    <table class="data-table customer-return-line-table">
                        <caption class="sr-only">"Authorized items, return reasons, controlled identities, and receipt progress"</caption>
                        <thead><tr><th>"Item"</th><th>"Return reason"</th><th>"Identity"</th><th>"Progress"</th></tr></thead>
                        <tbody>{detail.lines.into_iter().map(|line| view! { <tr><td><strong>{line.item_description}</strong><small>{format!("Item #{} · {}",line.item_id,line.uom)}</small></td><td><strong>{reason_label(line.reason)}</strong>{line.note.map(|note| view! { <small>{note}</small> })}</td><td>{identity_label(line.lot.as_deref(),line.serial.as_deref())}</td><td><dl class="line-metrics"><div><dt>"Authorized"</dt><dd>{format_quantity(line.authorized_quantity)}</dd></div><div><dt>"Quarantined"</dt><dd>{format_quantity(line.rejected_quantity)}</dd></div><div><dt>"Open"</dt><dd><strong>{format_quantity(line.remaining_quantity)}</strong></dd></div><div><dt>"Holds"</dt><dd>{line.inspection_hold_ids.len()}</dd></div></dl></td></tr> }).collect_view()}</tbody>
                    </table>
                </div>
            </section>
            {cancellation}
            <footer class="detail-actions"><a class="button quiet-action" href="/loads">"Open inbound load"</a>{open.then(|| view! { <button class="button danger-action" type="button" on:click=move |_| on_cancel.run(())>"Cancel return"</button> })}{open.then(|| view! { <button class="button primary-action" type="button" on:click=move |_| on_plan.run(())><Icon icon=UiIcon::Loads/><span>"Plan return load"</span></button> })}</footer>
        </div>
    }
}

#[component]
fn CreateReturnPanel(
    access: AccessScopeWorkspace,
    on_close: Callback<()>,
    on_created: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let owner = RwSignal::new(
        access
            .inventory_owners
            .first()
            .map_or_else(String::new, |v| v.id.to_string()),
    );
    let facility = RwSignal::new(
        access
            .facilities
            .first()
            .map_or_else(String::new, |v| v.id.to_string()),
    );
    let number = RwSignal::new(String::new());
    let reference = RwSignal::new(String::new());
    let expected = RwSignal::new(String::new());
    let items = RwSignal::new(Vec::<
        wareboxes_api_contract::v1::InboundLoadEntryItemResponse,
    >::new());
    let item_id = RwSignal::new(String::new());
    let quantity = RwSignal::new(String::new());
    let reason = RwSignal::new(CustomerReturnReason::CustomerRequest);
    let note = RwSignal::new(String::new());
    let lot = RwSignal::new(String::new());
    let serial = RwSignal::new(String::new());
    let lines = RwSignal::new(Vec::<DraftLine>::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CreateCustomerReturnRequest, String)>);
    let toasts = use_toast_bus();
    Effect::new(move |_| request_owner_items(owner.get(), items, item_id, on_unauthorized));
    let add_line = move |_| {
        let Ok(item) = item_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose an item.".into()));
            return;
        };
        let Ok(qty) = quantity.get_untracked().parse::<i64>() else {
            error.set(Some("Enter a whole authorized quantity.".into()));
            return;
        };
        if qty <= 0 {
            error.set(Some(
                "Authorized quantity must be greater than zero.".into(),
            ));
            return;
        }
        let reason_value = reason.get_untracked();
        let note_value = optional_text(&note.get_untracked());
        if reason_value == CustomerReturnReason::Other && note_value.is_none() {
            error.set(Some("Enter a note for the Other reason.".into()));
            return;
        }
        let Some(source) = items
            .get_untracked()
            .into_iter()
            .find(|value| value.item_id == item)
        else {
            error.set(Some("Refresh the client item list.".into()));
            return;
        };
        let lot_value = optional_text(&lot.get_untracked());
        let serial_value = optional_text(&serial.get_untracked());
        if lines.get_untracked().iter().any(|line| {
            line.item_id == item && line.lot == lot_value && line.serial == serial_value
        }) {
            error.set(Some("That item identity is already present.".into()));
            return;
        }
        lines.update(|values| {
            values.push(DraftLine {
                item_id: item,
                description: source
                    .description
                    .unwrap_or_else(|| format!("Item #{item}")),
                uom: source.uom,
                quantity: qty,
                reason: reason_value,
                note: note_value,
                lot: lot_value,
                serial: serial_value,
            })
        });
        quantity.set(String::new());
        note.set(String::new());
        lot.set(String::new());
        serial.set(String::new());
        error.set(None);
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Ok(owner_id) = owner.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a client.".into()));
            return;
        };
        let Ok(facility_id) = facility.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a facility.".into()));
            return;
        };
        let expected_at = match parse_optional_timestamp(&expected.get_untracked()) {
            Ok(v) => v.map(|t| t.to_rfc3339()),
            Err(message) => {
                error.set(Some(format!("Expected arrival: {message}")));
                return;
            }
        };
        let source_lines = lines.get_untracked();
        if number.get_untracked().trim().is_empty() || reference.get_untracked().trim().is_empty() {
            error.set(Some(
                "Return number and customer reference are required.".into(),
            ));
            return;
        }
        if source_lines.is_empty() {
            error.set(Some("Add at least one authorized item.".into()));
            return;
        }
        let request = CreateCustomerReturnRequest {
            inventory_owner_id: owner_id,
            facility_id,
            number: number.get_untracked().trim().into(),
            customer_reference: reference.get_untracked().trim().into(),
            expected_at,
            lines: source_lines
                .into_iter()
                .map(|line| CreateCustomerReturnLineRequest {
                    item_id: line.item_id,
                    authorized_quantity: line.quantity,
                    reason: line.reason,
                    note: line.note,
                    lot: line.lot,
                    serial: line.serial,
                })
                .collect(),
        };
        let key = retry
            .get_untracked()
            .filter(|(saved, _)| saved == &request)
            .map_or_else(api::new_idempotency_key, |(_, key)| key);
        retry.set(Some((request.clone(), key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::create_customer_return(&request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success(format!("Return {} authorized.", result.number));
                    on_created.run(result.customer_return_id);
                }
                Err(value) if value.unauthorized => on_unauthorized.run(()),
                Err(value) => {
                    pending.set(false);
                    if !value.ambiguous_outcome {
                        retry.set(None);
                    }
                    error.set(Some(value.message.clone()));
                    toasts.error(value.message);
                }
            }
        });
    };
    view! { <form class="inbound-asn-form" on:submit=submit><header class="detail-heading"><div><span class="eyebrow">"Returns intake"</span><h2>"New customer return"</h2></div><button class="text-button" type="button" on:click=move |_|on_close.run(())>"Close"</button></header><div class="form-grid two-column"><label><span>"Client"</span><select required disabled=move || !lines.get().is_empty() prop:value=move ||owner.get() on:change=move |event|{owner.set(event_target_value(&event));lines.set(Vec::new());}>{access.inventory_owners.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Facility"</span><select required prop:value=move ||facility.get() on:change=move |event|facility.set(event_target_value(&event))>{access.facilities.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Return number"</span><input required maxlength="120" prop:value=move ||number.get() on:input=move |event|number.set(event_target_value(&event))/></label><label><span>"Customer reference"</span><input required maxlength="200" placeholder="Order or authorization" prop:value=move ||reference.get() on:input=move |event|reference.set(event_target_value(&event))/></label><label class="full-width"><span>"Expected arrival"</span><input type="datetime-local" prop:value=move ||expected.get() on:input=move |event|expected.set(event_target_value(&event))/></label></div><section class="asn-line-builder"><div class="detail-section-heading"><h3>"Authorized items"</h3><span>{move ||format!("{} lines",lines.get().len())}</span></div><div class="asn-line-inputs return-line-inputs"><select aria-label="Item" prop:value=move ||item_id.get() on:change=move |event|item_id.set(event_target_value(&event))><option value="">"Choose item"</option>{move ||items.get().into_iter().map(|item|view!{<option value=item.item_id>{item.description.unwrap_or_else(||format!("Item #{}",item.item_id))}</option>}).collect_view()}</select><input aria-label="Quantity" type="number" min="1" placeholder="Qty" prop:value=move ||quantity.get() on:input=move |event|quantity.set(event_target_value(&event))/><select aria-label="Reason" on:change=move |event|reason.set(parse_reason(&event_target_value(&event)))><option value="customer_request">"Customer request"</option><option value="damaged">"Damaged"</option><option value="refused_delivery">"Refused delivery"</option><option value="recall">"Recall"</option><option value="warranty">"Warranty"</option><option value="other">"Other"</option></select><input aria-label="Note" placeholder="Reason note" prop:value=move ||note.get() on:input=move |event|note.set(event_target_value(&event))/><input aria-label="Lot" placeholder="Lot (optional)" prop:value=move ||lot.get() on:input=move |event|lot.set(event_target_value(&event))/><input aria-label="Serial" placeholder="Serial (optional)" prop:value=move ||serial.get() on:input=move |event|serial.set(event_target_value(&event))/><button class="button secondary-action compact" type="button" on:click=add_line>"Add line"</button></div><div class="table-scroll"><table class="dense-table"><tbody>{move ||lines.get().into_iter().enumerate().map(|(index,line)|view!{<tr><td><strong>{line.description}</strong><small>{line.uom}</small></td><td>{reason_label(line.reason)}</td><td>{identity_label(line.lot.as_deref(),line.serial.as_deref())}</td><td class="numeric">{format_quantity(line.quantity)}</td><td><button class="icon-button danger" type="button" title="Remove line" aria-label="Remove line" on:click=move |_|lines.update(|values|{values.remove(index);})><Icon icon=UiIcon::Remove/></button></td></tr>}).collect_view()}</tbody></table></div></section><Show when=move ||error.get().is_some()>{move ||error.get().map(|message|view!{<p class="inline-command-error">{message}</p>})}</Show><footer class="detail-actions"><button class="button quiet-action" type="button" on:click=move |_|on_close.run(())>"Cancel"</button><button class="button primary-action" type="submit" disabled=move ||pending.get()>{move ||if pending.get(){"Authorizing..."}else{"Authorize return"}}</button></footer></form> }
}

#[component]
fn PlanReturnDialog(
    detail: CustomerReturnDetailResponse,
    locations: Vec<Location>,
    on_close: Callback<()>,
    on_planned: Callback<PlanCustomerReturnLoadResponse>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let dock = RwSignal::new(String::new());
    let carrier = RwSignal::new(String::new());
    let trailer = RwSignal::new(String::new());
    let seal = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(PlanCustomerReturnLoadRequest, String)>);
    let toasts = use_toast_bus();
    let return_id = detail.summary.customer_return_id;
    let revision = detail.summary.revision;
    let docks = locations
        .into_iter()
        .filter(|location| {
            location.facility_id == detail.summary.facility_id
                && location.active
                && location.deleted.is_none()
                && location.receivable
                && location
                    .barcode
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .collect::<Vec<_>>();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Ok(receiving_location_id) = dock.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a receivable dock.".into()));
            return;
        };
        let request = PlanCustomerReturnLoadRequest {
            expected_revision: revision,
            receiving_location_id,
            carrier: optional_text(&carrier.get_untracked()),
            trailer_number: optional_text(&trailer.get_untracked()),
            seal_number: optional_text(&seal.get_untracked()),
        };
        let key = retry
            .get_untracked()
            .filter(|(saved, _)| saved == &request)
            .map_or_else(api::new_idempotency_key, |(_, key)| key);
        retry.set(Some((request.clone(), key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::plan_customer_return_load(return_id, &request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success(format!("Return load {} planned.", result.execution_barcode));
                    on_planned.run(result);
                }
                Err(value) if value.unauthorized => on_unauthorized.run(()),
                Err(value) => {
                    pending.set(false);
                    if !value.ambiguous_outcome {
                        retry.set(None);
                    }
                    error.set(Some(value.message.clone()));
                    toasts.error(value.message);
                }
            }
        });
    };
    view! {<div class="inbound-asn-dialog-backdrop" role="presentation"><form class="inbound-asn-dialog" role="dialog" aria-modal="true" aria-labelledby="return-plan-title" on:submit=submit><header><div><span class="eyebrow">"Quarantine-bound receiving"</span><h2 id="return-plan-title">{format!("Plan load from {}",detail.summary.number)}</h2></div><button type="button" class="icon-button" title="Close" aria-label="Close" on:click=move |_|on_close.run(())><Icon icon=UiIcon::Close/></button></header><p>"All physical stock received against this load is forced into quarantine for inspection."</p><div class="form-grid two-column"><label class="full-width"><span>"Receiving dock"</span><select required prop:value=move ||dock.get() on:change=move |event|dock.set(event_target_value(&event))><option value="">"Choose dock"</option>{docks.into_iter().map(|location|{let label=location.name.or(location.barcode).unwrap_or_else(||format!("Dock #{}",location.id));view!{<option value=location.id>{label}</option>}}).collect_view()}</select></label><label><span>"Carrier"</span><input prop:value=move ||carrier.get() on:input=move |event|carrier.set(event_target_value(&event))/></label><label><span>"Trailer"</span><input prop:value=move ||trailer.get() on:input=move |event|trailer.set(event_target_value(&event))/></label><label><span>"Seal"</span><input prop:value=move ||seal.get() on:input=move |event|seal.set(event_target_value(&event))/></label></div><Show when=move ||error.get().is_some()>{move ||error.get().map(|message|view!{<p class="inline-command-error">{message}</p>})}</Show><footer><button class="button quiet-action" type="button" on:click=move |_|on_close.run(())>"Cancel"</button><button class="button primary-action" type="submit" disabled=move ||pending.get()>{move ||if pending.get(){"Planning..."}else{"Plan return load"}}</button></footer></form></div>}
}

#[component]
fn CancelReturnDialog(
    detail: CustomerReturnDetailResponse,
    on_close: Callback<()>,
    on_cancelled: Callback<CancelCustomerReturnResponse>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new(CustomerReturnCancellationReason::CustomerCancelled);
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CancelCustomerReturnRequest, String)>);
    let toasts = use_toast_bus();
    let return_id = detail.summary.customer_return_id;
    let revision = detail.summary.revision;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let note_value = optional_text(&note.get_untracked());
        if reason.get_untracked() == CustomerReturnCancellationReason::Other && note_value.is_none()
        {
            error.set(Some("Enter a note for the Other reason.".into()));
            return;
        }
        let request = CancelCustomerReturnRequest {
            expected_revision: revision,
            reason: reason.get_untracked(),
            note: note_value,
        };
        let key = retry
            .get_untracked()
            .filter(|(saved, _)| saved == &request)
            .map_or_else(api::new_idempotency_key, |(_, key)| key);
        retry.set(Some((request.clone(), key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::cancel_customer_return(return_id, &request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success("Customer return cancelled.");
                    on_cancelled.run(result);
                }
                Err(value) if value.unauthorized => on_unauthorized.run(()),
                Err(value) => {
                    pending.set(false);
                    if !value.ambiguous_outcome {
                        retry.set(None);
                    }
                    error.set(Some(value.message.clone()));
                    toasts.error(value.message);
                }
            }
        });
    };
    view! {<div class="inbound-asn-dialog-backdrop" role="presentation"><form class="inbound-asn-dialog asn-cancel-dialog" role="dialog" aria-modal="true" aria-labelledby="return-cancel-title" on:submit=submit><header><div><span class="eyebrow">"Terminal authorization action"</span><h2 id="return-cancel-title">{format!("Cancel {}",detail.summary.number)}</h2></div><button type="button" class="icon-button" title="Close" aria-label="Close" on:click=move |_|on_close.run(())><Icon icon=UiIcon::Close/></button></header><div class="form-grid"><label><span>"Reason"</span><select on:change=move |event|reason.set(parse_cancellation(&event_target_value(&event)))><option value="customer_cancelled">"Customer cancelled"</option><option value="duplicate_authorization">"Duplicate authorization"</option><option value="return_window_expired">"Return window expired"</option><option value="other">"Other"</option></select></label><label><span>"Note"</span><textarea maxlength="500" placeholder="Optional unless reason is Other" prop:value=move ||note.get() on:input=move |event|note.set(event_target_value(&event))></textarea></label></div><Show when=move ||error.get().is_some()>{move ||error.get().map(|message|view!{<p class="inline-command-error">{message}</p>})}</Show><footer><button class="button quiet-action" type="button" on:click=move |_|on_close.run(())>"Go back"</button><button class="button danger-action" type="submit" disabled=move ||pending.get()>{move ||if pending.get(){"Cancelling..."}else{"Cancel return"}}</button></footer></form></div>}
}

fn request_page(
    signals: PageSignals,
    cursor: Option<OpaqueCursor>,
    history: Vec<Option<OpaqueCursor>>,
) {
    let request_generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(request_generation);
    signals.loading.set(true);
    signals.error.set(None);
    let filters = api::CustomerReturnFilters {
        facility_id: signals.facility.get_untracked().parse().ok(),
        inventory_owner_id: signals.owner.get_untracked().parse().ok(),
        status: match signals.status.get_untracked().as_str() {
            "open" => Some(CustomerReturnStatus::Open),
            "planned" => Some(CustomerReturnStatus::Planned),
            "cancelled" => Some(CustomerReturnStatus::Cancelled),
            _ => None,
        },
        search: optional_text(&signals.search.get_untracked()),
    };
    leptos::task::spawn_local(async move {
        match api::customer_returns(filters, cursor.as_ref()).await {
            Ok(value) if signals.generation.get_untracked() == request_generation => {
                signals.page.set(Some(value));
                signals.current_cursor.set(cursor);
                signals.cursor_history.set(history);
                signals.loading.set(false);
            }
            Ok(_) => {}
            Err(value) if value.unauthorized => signals.on_unauthorized.run(()),
            Err(value) if signals.generation.get_untracked() == request_generation => {
                signals.error.set(Some(value.message));
                signals.loading.set(false);
            }
            Err(_) => {}
        }
    });
}
fn request_detail(
    return_id: i64,
    selected_id: RwSignal<Option<i64>>,
    selected: RwSignal<Option<CustomerReturnDetailResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    on_unauthorized: Callback<()>,
) {
    let request_generation = generation.get_untracked().wrapping_add(1);
    generation.set(request_generation);
    loading.set(true);
    error.set(None);
    leptos::task::spawn_local(async move {
        match api::customer_return_detail(return_id).await {
            Ok(value)
                if generation.get_untracked() == request_generation
                    && selected_id.get_untracked() == Some(return_id) =>
            {
                selected.set(Some(value));
                loading.set(false);
            }
            Ok(_) => {}
            Err(value) if value.unauthorized => on_unauthorized.run(()),
            Err(value) if generation.get_untracked() == request_generation => {
                error.set(Some(value.message));
                loading.set(false);
            }
            Err(_) => {}
        }
    });
}
fn request_owner_items(
    owner: String,
    items: RwSignal<Vec<wareboxes_api_contract::v1::InboundLoadEntryItemResponse>>,
    selected: RwSignal<String>,
    on_unauthorized: Callback<()>,
) {
    let Ok(owner_id) = owner.parse::<i64>() else {
        items.set(Vec::new());
        return;
    };
    leptos::task::spawn_local(async move {
        match api::inbound_load_entry_items(owner_id).await {
            Ok(value) => {
                selected.set(String::new());
                items.set(value);
            }
            Err(value) if value.unauthorized => on_unauthorized.run(()),
            Err(_) => items.set(Vec::new()),
        }
    });
}
fn status_label(value: CustomerReturnStatus) -> &'static str {
    match value {
        CustomerReturnStatus::Open => "Open",
        CustomerReturnStatus::Planned => "Planned",
        CustomerReturnStatus::Cancelled => "Cancelled",
    }
}
fn status_class(value: CustomerReturnStatus) -> &'static str {
    match value {
        CustomerReturnStatus::Open => "status-chip info",
        CustomerReturnStatus::Planned => "status-chip success",
        CustomerReturnStatus::Cancelled => "status-chip neutral",
    }
}
fn execution_label(value: CustomerReturnExecutionStatus) -> &'static str {
    match value {
        CustomerReturnExecutionStatus::Planned => "Planned",
        CustomerReturnExecutionStatus::Scheduled => "Scheduled",
        CustomerReturnExecutionStatus::Arrived => "Arrived",
        CustomerReturnExecutionStatus::Receiving => "Receiving",
        CustomerReturnExecutionStatus::Received => "Received",
        CustomerReturnExecutionStatus::Rejected => "Rejected",
        CustomerReturnExecutionStatus::Closed => "Closed",
        CustomerReturnExecutionStatus::Cancelled => "Cancelled",
    }
}
fn reason_label(value: CustomerReturnReason) -> &'static str {
    match value {
        CustomerReturnReason::CustomerRequest => "Customer request",
        CustomerReturnReason::Damaged => "Damaged",
        CustomerReturnReason::RefusedDelivery => "Refused delivery",
        CustomerReturnReason::Recall => "Recall",
        CustomerReturnReason::Warranty => "Warranty",
        CustomerReturnReason::Other => "Other",
    }
}
fn cancellation_label(value: CustomerReturnCancellationReason) -> &'static str {
    match value {
        CustomerReturnCancellationReason::CustomerCancelled => "Customer cancelled",
        CustomerReturnCancellationReason::DuplicateAuthorization => "Duplicate authorization",
        CustomerReturnCancellationReason::ReturnWindowExpired => "Return window expired",
        CustomerReturnCancellationReason::Other => "Other",
    }
}
fn parse_reason(value: &str) -> CustomerReturnReason {
    match value {
        "damaged" => CustomerReturnReason::Damaged,
        "refused_delivery" => CustomerReturnReason::RefusedDelivery,
        "recall" => CustomerReturnReason::Recall,
        "warranty" => CustomerReturnReason::Warranty,
        "other" => CustomerReturnReason::Other,
        _ => CustomerReturnReason::CustomerRequest,
    }
}
fn parse_cancellation(value: &str) -> CustomerReturnCancellationReason {
    match value {
        "duplicate_authorization" => CustomerReturnCancellationReason::DuplicateAuthorization,
        "return_window_expired" => CustomerReturnCancellationReason::ReturnWindowExpired,
        "other" => CustomerReturnCancellationReason::Other,
        _ => CustomerReturnCancellationReason::CustomerCancelled,
    }
}
fn short_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
fn identity_label(lot: Option<&str>, serial: Option<&str>) -> String {
    let mut values = Vec::new();
    if let Some(value) = lot {
        values.push(format!("Lot {value}"));
    }
    if let Some(value) = serial {
        values.push(format!("Serial {value}"));
    }
    if values.is_empty() {
        "No controlled identity".into()
    } else {
        values.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_distinguish_quarantine_and_terminal_state() {
        assert_eq!(reason_label(CustomerReturnReason::Damaged), "Damaged");
        assert_eq!(status_label(CustomerReturnStatus::Cancelled), "Cancelled");
    }
}
