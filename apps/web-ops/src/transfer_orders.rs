mod execution;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CancelTransferOrderRequest, CancelTransferOrderResponse, CreateTransferOrderLineRequest,
    CreateTransferOrderRequest, OpaqueCursor, ReleaseTransferOrderRequest,
    TransferOrderCancellationReason, TransferOrderDetailResponse, TransferOrderPage,
    TransferOrderStatus,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::components::{Icon, SearchField, UiIcon};
use crate::fulfillment_shared::{optional_text, parse_optional_timestamp};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

use execution::TransferExecutionDialog;

#[derive(Clone, Copy)]
struct PageSignals {
    page: RwSignal<Option<TransferOrderPage>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    current_cursor: RwSignal<Option<OpaqueCursor>>,
    cursor_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    source: RwSignal<String>,
    destination: RwSignal<String>,
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
}

#[component]
pub(crate) fn TransferOrdersWorkspace(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let page = RwSignal::new(None::<TransferOrderPage>);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let generation = RwSignal::new(0_u64);
    let current_cursor = RwSignal::new(None::<OpaqueCursor>);
    let cursor_history = RwSignal::new(Vec::<Option<OpaqueCursor>>::new());
    let source = RwSignal::new(String::new());
    let destination = RwSignal::new(String::new());
    let owner = RwSignal::new(String::new());
    let status = RwSignal::new(String::new());
    let search = RwSignal::new(String::new());
    let selected_id = RwSignal::new(None::<i64>);
    let selected = RwSignal::new(None::<TransferOrderDetailResponse>);
    let detail_loading = RwSignal::new(false);
    let detail_error = RwSignal::new(None::<String>);
    let detail_generation = RwSignal::new(0_u64);
    let create_open = RwSignal::new(false);
    let cancel_open = RwSignal::new(false);
    let execution_open = RwSignal::new(false);
    let layout = SplitPaneState::new("transfer-orders", 690);
    let signals = PageSignals {
        page,
        loading,
        error,
        generation,
        current_cursor,
        cursor_history,
        source,
        destination,
        owner,
        status,
        search,
        on_unauthorized,
    };
    Effect::new(move |_| request_page(signals, None, Vec::new()));

    let load_detail = move |id: i64| {
        create_open.set(false);
        cancel_open.set(false);
        execution_open.set(false);
        layout.show_detail();
        selected_id.set(Some(id));
        request_detail(
            id,
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
        if let Some(id) = selected_id.get_untracked() {
            load_detail(id);
        }
    };
    let changed = Callback::new(move |id: i64| {
        request_page(signals, None, Vec::new());
        load_detail(id);
    });
    let cancelled = Callback::new(move |result: CancelTransferOrderResponse| {
        cancel_open.set(false);
        request_page(signals, None, Vec::new());
        load_detail(result.transfer_order_id);
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
    let filter_facilities = access.facilities.clone();
    let destination_facilities = access.facilities.clone();

    view! {
        <div class="purchase-order-workspace transfer-order-workspace split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <section class="data-section purchase-order-list split-master">
                <form class="purchase-order-toolbar" on:submit=move |event| { event.prevent_default(); request_page(signals,None,Vec::new()); }>
                    <div class="toolbar-summary"><strong>{move || page.get().map_or(0,|value|value.items.len())}</strong><span>"transfers"</span><PaneControls layout master_label="Transfer table" detail_label="Transfer detail"/></div>
                    <SearchField label="Search transfer orders".to_owned() placeholder="Transfer number" value=search/>
                    <label><span class="sr-only">"Client"</span><select prop:value=move || owner.get() on:change=move |event| owner.set(event_target_value(&event))><option value="">"All clients"</option>{access.inventory_owners.clone().into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label>
                    <label><span class="sr-only">"Source"</span><select prop:value=move || source.get() on:change=move |event| source.set(event_target_value(&event))><option value="">"All origins"</option>{filter_facilities.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label>
                    <label><span class="sr-only">"Destination"</span><select prop:value=move || destination.get() on:change=move |event| destination.set(event_target_value(&event))><option value="">"All destinations"</option>{destination_facilities.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label>
                    <label><span class="sr-only">"Status"</span><select prop:value=move || status.get() on:change=move |event| status.set(event_target_value(&event))><option value="">"All statuses"</option><option value="draft">"Draft"</option><option value="released">"Released"</option><option value="in_transit">"In transit"</option><option value="received">"Received"</option><option value="cancelled">"Cancelled"</option></select></label>
                    <button class="button secondary-action compact" type="submit" disabled=move || loading.get()>"Apply"</button>
                    <button class="icon-button" type="button" title="Refresh transfers" aria-label="Refresh transfers" disabled=move || loading.get() on:click=refresh><Icon icon=UiIcon::Refresh/></button>
                    <button class="button primary-action compact" type="button" on:click=move |_| { create_open.set(true); layout.show_detail(); }><Icon icon=UiIcon::Add/><span>"New transfer"</span></button>
                </form>
                <div class="table-scroll"><table class="dense-table transfer-order-table"><thead><tr><th>"Transfer"</th><th>"Status"</th><th>"Origin"</th><th>"Destination"</th><th>"Client"</th><th class="numeric">"Qty"</th><th class="numeric">"Lines"</th><th>"Depart"</th><th>"Arrive"</th></tr></thead>
                    <tbody>{move || page.get().map(|current|current.items.into_iter().map(|entry|{let id=entry.transfer_order_id;let active=selected_id.get()==Some(id)&&!create_open.get();view!{<tr class:active-row=active><td><button type="button" class="row-link" on:click=move |_|load_detail(id)>{entry.number}</button></td><td><span class=status_class(entry.status)>{status_label(entry.status)}</span></td><td>{entry.source_facility_name}</td><td>{entry.destination_facility_name}</td><td>{entry.inventory_owner_name}</td><td class="numeric"><strong>{format_quantity(entry.total_requested_quantity)}</strong></td><td class="numeric">{entry.line_count}</td><td>{entry.expected_departure_at.as_deref().map(short_timestamp).unwrap_or_else(||"Not scheduled".into())}</td><td>{entry.expected_arrival_at.as_deref().map(short_timestamp).unwrap_or_else(||"Not scheduled".into())}</td></tr>}}).collect_view())}</tbody>
                </table><Show when=move || !loading.get()&&page.get().is_some_and(|value|value.items.is_empty())><p class="empty-state">"No transfer orders match these filters."</p></Show></div>
                <Show when=move || error.get().is_some()>{move ||error.get().map(|message|view!{<p class="inline-command-error">{message}</p>})}</Show>
                <footer class="table-pagination"><span>{move ||page.get().map_or_else(||"No records".into(),|value|format!("{} records on this page",value.items.len()))}</span><div><button class="button quiet-action compact" type="button" disabled=move ||loading.get()||cursor_history.get().is_empty() on:click=previous>"Previous"</button><button class="button quiet-action compact" type="button" disabled=move ||loading.get()||page.get().and_then(|value|value.next_cursor).is_none() on:click=next>"Next"</button></div></footer>
            </section>
            <SplitPaneHandle layout/>
            <section class="data-section purchase-order-detail split-detail">
                <Show when=move || create_open.get() fallback=move || view!{
                    <Show when=move || selected.get().is_some() fallback=move ||view!{<div class="detail-empty"><h2>"Transfer details"</h2><p>"Select a transfer to inspect route, demand, and lifecycle evidence."</p></div>}>
                        {move ||selected.get().map(|detail|view!{<TransferDetail detail on_changed=changed on_cancel=Callback::new(move |_|cancel_open.set(true)) on_execute=Callback::new(move |_|execution_open.set(true)) on_unauthorized/>})}
                    </Show>
                }><CreateTransferPanel access=access.clone() on_close=Callback::new(move |_|create_open.set(false)) on_created=changed on_unauthorized/></Show>
                <Show when=move ||detail_loading.get()><div class="panel-loading">"Loading transfer..."</div></Show>
                <Show when=move ||detail_error.get().is_some()>{move ||detail_error.get().map(|message|view!{<p class="inline-command-error">{message}</p>})}</Show>
            </section>
        </div>
        <Show when=move ||cancel_open.get()&&selected.get().is_some()>{move ||selected.get().map(|detail|view!{<CancelTransferDialog detail on_close=Callback::new(move |_|cancel_open.set(false)) on_cancelled=cancelled on_unauthorized/>})}</Show>
        <Show when=move ||execution_open.get()&&selected.get().is_some()>{move ||selected.get().map(|detail|view!{<TransferExecutionDialog detail on_close=Callback::new(move |_|execution_open.set(false)) on_changed=changed on_unauthorized/>})}</Show>
    }
}

#[component]
fn TransferDetail(
    detail: TransferOrderDetailResponse,
    on_changed: Callback<i64>,
    on_cancel: Callback<()>,
    on_execute: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(ReleaseTransferOrderRequest, String)>);
    let toasts = use_toast_bus();
    let summary = detail.summary.clone();
    let id = summary.transfer_order_id;
    let can_release = summary.status == TransferOrderStatus::Draft;
    let can_cancel = matches!(
        summary.status,
        TransferOrderStatus::Draft | TransferOrderStatus::Released
    );
    let release = move |_| {
        if pending.get_untracked() {
            return;
        }
        let request = ReleaseTransferOrderRequest {
            expected_revision: summary.revision,
        };
        let key = retry
            .get_untracked()
            .filter(|(saved, _)| saved == &request)
            .map_or_else(api::new_idempotency_key, |(_, key)| key);
        retry.set(Some((request.clone(), key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::release_transfer_order(id, &request, &key).await {
                Ok(_) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success("Transfer released for execution.");
                    on_changed.run(id);
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
    let cancellation=summary.cancellation_reason.map(|reason|{let note=summary.cancellation_note.clone();view!{<section class="purchase-order-cancellation-evidence"><div><span>"Cancellation"</span><strong>{reason_label(reason)}</strong></div>{note.map(|value|view!{<p>{value}</p>})}<small>{summary.cancelled_at.as_deref().map(short_timestamp).unwrap_or_else(||"Time unavailable".into())}</small></section>}});
    view! {<article class="purchase-order-detail-content"><header class="detail-heading"><div><span class="eyebrow">{format!("Transfer #{} · revision {}",id,summary.revision.get())}</span><h2>{summary.number}</h2><p>{format!("{} → {}",summary.source_facility_name,summary.destination_facility_name)}</p></div><span class=status_class(summary.status)>{status_label(summary.status)}</span></header>
        <dl class="summary-grid"><div><dt>"Client"</dt><dd>{summary.inventory_owner_name}</dd></div><div><dt>"Requested"</dt><dd>{format_quantity(summary.total_requested_quantity)}</dd></div><div><dt>"Lines"</dt><dd>{summary.line_count}</dd></div><div><dt>"Expected departure"</dt><dd>{summary.expected_departure_at.as_deref().map(short_timestamp).unwrap_or_else(||"Not scheduled".into())}</dd></div><div><dt>"Expected arrival"</dt><dd>{summary.expected_arrival_at.as_deref().map(short_timestamp).unwrap_or_else(||"Not scheduled".into())}</dd></div><div><dt>"Created"</dt><dd>{short_timestamp(&summary.created_at)}</dd></div></dl>
        {cancellation}<section><div class="detail-section-heading"><h3>"Transfer demand"</h3><span>{format!("{} lines",detail.lines.len())}</span></div><table class="dense-table"><thead><tr><th>"Item"</th><th>"UOM"</th><th class="numeric">"Requested"</th><th class="numeric">"Dispatched"</th><th class="numeric">"Received"</th></tr></thead><tbody>{detail.lines.into_iter().map(|line|view!{<tr><td><strong>{line.item_description}</strong><small>{format!("Item #{}",line.item_id)}</small></td><td>{line.uom}</td><td class="numeric"><strong>{format_quantity(line.requested_quantity)}</strong></td><td class="numeric">{format_quantity(line.dispatched_quantity)}</td><td class="numeric">{format_quantity(line.received_quantity)}</td></tr>}).collect_view()}</tbody></table></section>
        <Show when=move ||error.get().is_some()>{move ||error.get().map(|message|view!{<p class="inline-command-error">{message}</p>})}</Show><footer class="detail-actions"><Show when=move ||can_cancel><button class="button danger-action compact" type="button" disabled=move ||pending.get() on:click=move |_|on_cancel.run(())>"Cancel transfer"</button></Show><Show when=move ||matches!(summary.status,TransferOrderStatus::Released|TransferOrderStatus::InTransit)><button class="button primary-action compact" type="button" disabled=move ||pending.get() on:click=move |_|on_execute.run(())>{if summary.status==TransferOrderStatus::Released{"Dispatch transfer"}else{"Receive transfer"}}</button></Show><Show when=move ||can_release><button class="button primary-action compact" type="button" disabled=move ||pending.get() on:click=release>{move ||if pending.get(){"Releasing..."}else{"Release transfer"}}</button></Show></footer>
    </article>}
}

#[component]
fn CreateTransferPanel(
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
    let source = RwSignal::new(
        access
            .facilities
            .first()
            .map_or_else(String::new, |v| v.id.to_string()),
    );
    let destination = RwSignal::new(
        access
            .facilities
            .get(1)
            .or_else(|| access.facilities.first())
            .map_or_else(String::new, |v| v.id.to_string()),
    );
    let number = RwSignal::new(String::new());
    let departure = RwSignal::new(String::new());
    let arrival = RwSignal::new(String::new());
    let items = RwSignal::new(Vec::<
        wareboxes_api_contract::v1::InboundLoadEntryItemResponse,
    >::new());
    let items_loading = RwSignal::new(false);
    let item_id = RwSignal::new(String::new());
    let quantity = RwSignal::new(String::new());
    let lines = RwSignal::new(Vec::<DraftLine>::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CreateTransferOrderRequest, String)>);
    let toasts = use_toast_bus();
    let clients = access.inventory_owners;
    let sources = access.facilities.clone();
    let destinations = access.facilities;
    Effect::new(move |_| {
        request_owner_items(owner.get(), items, items_loading, item_id, on_unauthorized)
    });
    let add_line = move |_| {
        let Ok(selected) = item_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose an item.".into()));
            return;
        };
        let Ok(qty) = quantity.get_untracked().parse::<i64>() else {
            error.set(Some("Enter a whole quantity.".into()));
            return;
        };
        if qty <= 0 {
            error.set(Some("Quantity must be greater than zero.".into()));
            return;
        }
        if lines
            .get_untracked()
            .iter()
            .any(|line| line.item_id == selected)
        {
            error.set(Some("That item is already on the transfer.".into()));
            return;
        }
        let Some(item) = items
            .get_untracked()
            .into_iter()
            .find(|value| value.item_id == selected)
        else {
            error.set(Some("Refresh the client item list.".into()));
            return;
        };
        lines.update(|values| {
            values.push(DraftLine {
                item_id: selected,
                description: item
                    .description
                    .unwrap_or_else(|| format!("Item #{selected}")),
                uom: item.uom,
                quantity: qty,
            })
        });
        quantity.set(String::new());
        error.set(None);
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let parsed = || -> Result<CreateTransferOrderRequest, String> {
            let owner_id = owner
                .get_untracked()
                .parse()
                .map_err(|_| "Choose a client.".to_owned())?;
            let source_id = source
                .get_untracked()
                .parse()
                .map_err(|_| "Choose an origin.".to_owned())?;
            let destination_id = destination
                .get_untracked()
                .parse()
                .map_err(|_| "Choose a destination.".to_owned())?;
            if source_id == destination_id {
                return Err("Origin and destination must differ.".into());
            }
            let number_value = number.get_untracked().trim().to_owned();
            if number_value.is_empty() {
                return Err("Transfer number is required.".into());
            }
            let line_values = lines.get_untracked();
            if line_values.is_empty() {
                return Err("Add at least one item.".into());
            }
            let expected_departure_at = parse_optional_timestamp(&departure.get_untracked())
                .map_err(|value| format!("Expected departure: {value}"))?
                .map(|value| value.to_rfc3339());
            let expected_arrival_at = parse_optional_timestamp(&arrival.get_untracked())
                .map_err(|value| format!("Expected arrival: {value}"))?
                .map(|value| value.to_rfc3339());
            Ok(CreateTransferOrderRequest {
                inventory_owner_id: owner_id,
                source_facility_id: source_id,
                destination_facility_id: destination_id,
                number: number_value,
                expected_departure_at,
                expected_arrival_at,
                lines: line_values
                    .into_iter()
                    .map(|line| CreateTransferOrderLineRequest {
                        item_id: line.item_id,
                        requested_quantity: line.quantity,
                    })
                    .collect(),
            })
        };
        let request = match parsed() {
            Ok(value) => value,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        let key = retry
            .get_untracked()
            .filter(|(saved, _)| saved == &request)
            .map_or_else(api::new_idempotency_key, |(_, key)| key);
        retry.set(Some((request.clone(), key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::create_transfer_order(&request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success(format!("Transfer {} created.", result.number));
                    on_created.run(result.transfer_order_id);
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
    view! {<form class="purchase-order-form" on:submit=submit><header class="detail-heading"><div><span class="eyebrow">"Interfacility demand"</span><h2>"New transfer order"</h2></div><button class="text-button" type="button" on:click=move |_|on_close.run(())>"Close"</button></header>
    <div class="form-grid two-column"><label><span>"Client"</span><select required disabled=move ||!lines.get().is_empty() prop:value=move ||owner.get() on:change=move|event|{owner.set(event_target_value(&event));lines.set(Vec::new());}>{clients.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Transfer number"</span><input required maxlength="120" prop:value=move ||number.get() on:input=move|event|number.set(event_target_value(&event))/></label><label><span>"Origin"</span><select required prop:value=move ||source.get() on:change=move|event|source.set(event_target_value(&event))>{sources.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Destination"</span><select required prop:value=move ||destination.get() on:change=move|event|destination.set(event_target_value(&event))>{destinations.into_iter().map(|item|view!{<option value=item.id>{item.name}</option>}).collect_view()}</select></label><label><span>"Expected departure"</span><input type="datetime-local" prop:value=move ||departure.get() on:input=move|event|departure.set(event_target_value(&event))/></label><label><span>"Expected arrival"</span><input type="datetime-local" prop:value=move ||arrival.get() on:input=move|event|arrival.set(event_target_value(&event))/></label></div>
    <section class="po-line-builder"><div class="detail-section-heading"><h3>"Requested items"</h3><span>{move ||format!("{} lines",lines.get().len())}</span></div><div class="po-line-inputs"><select aria-label="Item" prop:value=move ||item_id.get() on:change=move|event|item_id.set(event_target_value(&event))><option value="">{move ||if items_loading.get(){"Loading items"}else{"Choose item"}}</option>{move ||items.get().into_iter().map(|item|view!{<option value=item.item_id>{item.description.unwrap_or_else(||format!("Item #{}",item.item_id))}</option>}).collect_view()}</select><input aria-label="Quantity" type="number" min="1" placeholder="Qty" prop:value=move ||quantity.get() on:input=move|event|quantity.set(event_target_value(&event))/><button class="button secondary-action compact" type="button" on:click=add_line>"Add line"</button></div><div class="table-scroll"><table class="dense-table"><tbody>{move ||lines.get().into_iter().enumerate().map(|(index,line)|view!{<tr><td><strong>{line.description}</strong><small>{line.uom}</small></td><td class="numeric">{format_quantity(line.quantity)}</td><td><button class="icon-button danger" type="button" title="Remove line" aria-label="Remove line" on:click=move |_|lines.update(|values|{values.remove(index);})><Icon icon=UiIcon::Remove/></button></td></tr>}).collect_view()}</tbody></table></div></section>
    <Show when=move ||error.get().is_some()>{move ||error.get().map(|message|view!{<p class="inline-command-error">{message}</p>})}</Show><footer class="detail-actions"><button class="button quiet-action compact" type="button" on:click=move |_|on_close.run(())>"Cancel"</button><button class="button primary-action compact" type="submit" disabled=move ||pending.get()>{move ||if pending.get(){"Creating..."}else{"Create transfer"}}</button></footer></form>}
}

#[component]
fn CancelTransferDialog(
    detail: TransferOrderDetailResponse,
    on_close: Callback<()>,
    on_cancelled: Callback<CancelTransferOrderResponse>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new("demand_cancelled".to_owned());
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CancelTransferOrderRequest, String)>);
    let toasts = use_toast_bus();
    let id = detail.summary.transfer_order_id;
    let revision = detail.summary.revision;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let selected = parse_reason(&reason.get_untracked());
        let note_value = optional_text(&note.get_untracked());
        if selected == TransferOrderCancellationReason::Other && note_value.is_none() {
            error.set(Some(
                "Explain the cancellation when reason is Other.".into(),
            ));
            return;
        }
        let request = CancelTransferOrderRequest {
            expected_revision: revision,
            reason: selected,
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
            match api::cancel_transfer_order(id, &request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success("Transfer cancelled.");
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
    view! {<div class="purchase-order-dialog-backdrop"><form class="purchase-order-dialog purchase-order-cancel-dialog" role="dialog" aria-modal="true" aria-labelledby="cancel-transfer-title" on:submit=submit><header><div><span class="eyebrow">{detail.summary.number}</span><h2 id="cancel-transfer-title">"Cancel transfer order"</h2></div><button class="text-button" type="button" on:click=move |_|on_close.run(())>"Close"</button></header><p>"This terminal action removes the transfer from future execution."</p><div class="form-grid two-column"><label><span>"Reason"</span><select prop:value=move ||reason.get() on:change=move|event|reason.set(event_target_value(&event))><option value="demand_cancelled">"Demand cancelled"</option><option value="duplicate_order">"Duplicate transfer"</option><option value="route_cancelled">"Route cancelled"</option><option value="other">"Other"</option></select></label><label><span>"Note"</span><input maxlength="500" placeholder="Optional unless Other" prop:value=move ||note.get() on:input=move|event|note.set(event_target_value(&event))/></label></div><Show when=move ||error.get().is_some()>{move ||error.get().map(|message|view!{<p class="inline-command-error" role="alert">{message}</p>})}</Show><footer><button class="button quiet-action compact" type="button" on:click=move |_|on_close.run(())>"Keep transfer"</button><button class="button danger-action compact" type="submit" disabled=move ||pending.get()>{move ||if pending.get(){"Cancelling..."}else{"Cancel transfer"}}</button></footer></form></div>}
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
    let filters = api::TransferOrderFilters {
        source_facility_id: parse_id(&signals.source.get_untracked()),
        destination_facility_id: parse_id(&signals.destination.get_untracked()),
        inventory_owner_id: parse_id(&signals.owner.get_untracked()),
        status: match signals.status.get_untracked().as_str() {
            "draft" => Some(TransferOrderStatus::Draft),
            "released" => Some(TransferOrderStatus::Released),
            "in_transit" => Some(TransferOrderStatus::InTransit),
            "received" => Some(TransferOrderStatus::Received),
            "cancelled" => Some(TransferOrderStatus::Cancelled),
            _ => None,
        },
        search: optional_text(&signals.search.get_untracked()),
    };
    leptos::task::spawn_local(async move {
        match api::transfer_orders(filters, cursor.as_ref()).await {
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

#[allow(clippy::too_many_arguments)]
fn request_detail(
    id: i64,
    selected_id: RwSignal<Option<i64>>,
    selected: RwSignal<Option<TransferOrderDetailResponse>>,
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
        match api::transfer_order_detail(id).await {
            Ok(value)
                if generation.get_untracked() == request_generation
                    && selected_id.get_untracked() == Some(id) =>
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
    loading: RwSignal<bool>,
    selected: RwSignal<String>,
    on_unauthorized: Callback<()>,
) {
    let Ok(id) = owner.parse() else {
        items.set(Vec::new());
        return;
    };
    loading.set(true);
    leptos::task::spawn_local(async move {
        match api::inbound_load_entry_items(id).await {
            Ok(value) => {
                selected.set(String::new());
                items.set(value);
                loading.set(false);
            }
            Err(value) if value.unauthorized => on_unauthorized.run(()),
            Err(_) => {
                items.set(Vec::new());
                loading.set(false);
            }
        }
    });
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok()
}
fn status_label(value: TransferOrderStatus) -> &'static str {
    match value {
        TransferOrderStatus::Draft => "Draft",
        TransferOrderStatus::Released => "Released",
        TransferOrderStatus::InTransit => "In transit",
        TransferOrderStatus::Received => "Received",
        TransferOrderStatus::Cancelled => "Cancelled",
    }
}
fn status_class(value: TransferOrderStatus) -> &'static str {
    match value {
        TransferOrderStatus::Draft => "status-chip info",
        TransferOrderStatus::Released => "status-chip success",
        TransferOrderStatus::InTransit => "status-chip warning",
        TransferOrderStatus::Received => "status-chip success",
        TransferOrderStatus::Cancelled => "status-chip neutral",
    }
}
fn parse_reason(value: &str) -> TransferOrderCancellationReason {
    match value {
        "duplicate_order" => TransferOrderCancellationReason::DuplicateOrder,
        "route_cancelled" => TransferOrderCancellationReason::RouteCancelled,
        "other" => TransferOrderCancellationReason::Other,
        _ => TransferOrderCancellationReason::DemandCancelled,
    }
}
fn reason_label(value: TransferOrderCancellationReason) -> &'static str {
    match value {
        TransferOrderCancellationReason::DemandCancelled => "Demand cancelled",
        TransferOrderCancellationReason::DuplicateOrder => "Duplicate transfer",
        TransferOrderCancellationReason::RouteCancelled => "Route cancelled",
        TransferOrderCancellationReason::Other => "Other",
    }
}
fn short_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_cover_terminal_status() {
        assert_eq!(status_label(TransferOrderStatus::Cancelled), "Cancelled");
        assert_eq!(
            reason_label(TransferOrderCancellationReason::RouteCancelled),
            "Route cancelled"
        );
    }
}
