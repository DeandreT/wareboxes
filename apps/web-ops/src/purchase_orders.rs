use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{
    CancelPurchaseOrderRequest, CancelPurchaseOrderResponse, CreatePurchaseOrderAsnLineRequest,
    CreatePurchaseOrderAsnRequest, CreatePurchaseOrderLineRequest, CreatePurchaseOrderRequest,
    OpaqueCursor, PurchaseOrderCancellationReason, PurchaseOrderDetailResponse,
    PurchaseOrderLineResponse, PurchaseOrderPage, PurchaseOrderStatus,
    PurchaseOrderSummaryResponse, ReleasePurchaseOrderRequest, Revision,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::components::{Icon, SearchField, UiIcon};
use crate::fulfillment_shared::{optional_text, parse_optional_timestamp};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

#[derive(Clone, Copy)]
struct PageSignals {
    page: RwSignal<Option<PurchaseOrderPage>>,
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
    ordered_quantity: i64,
}

#[derive(Clone, PartialEq, Eq)]
struct DraftAsnLine {
    purchase_order_line_id: i64,
    description: String,
    uom: String,
    available_to_notify_quantity: i64,
    expected_quantity: String,
    lot: String,
    serial: String,
    expiration: String,
}

#[component]
pub(crate) fn PurchaseOrdersWorkspace(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let page = RwSignal::new(None::<PurchaseOrderPage>);
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
    let selected = RwSignal::new(None::<PurchaseOrderDetailResponse>);
    let detail_loading = RwSignal::new(false);
    let detail_error = RwSignal::new(None::<String>);
    let detail_generation = RwSignal::new(0_u64);
    let create_open = RwSignal::new(false);
    let cancel_open = RwSignal::new(false);
    let layout = SplitPaneState::new("purchase-orders", 700);
    let page_signals = PageSignals {
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

    Effect::new(move |_| request_page(page_signals, None, Vec::new()));

    let load_detail = move |purchase_order_id: i64| {
        create_open.set(false);
        cancel_open.set(false);
        layout.show_detail();
        selected_id.set(Some(purchase_order_id));
        request_detail(
            purchase_order_id,
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
            page_signals,
            current_cursor.get_untracked(),
            cursor_history.get_untracked(),
        );
        if let Some(purchase_order_id) = selected_id.get_untracked() {
            load_detail(purchase_order_id);
        }
    };
    let created = Callback::new(move |purchase_order_id: i64| {
        create_open.set(false);
        request_page(page_signals, None, Vec::new());
        load_detail(purchase_order_id);
    });
    let released = Callback::new(move |purchase_order_id: i64| {
        request_page(page_signals, None, Vec::new());
        load_detail(purchase_order_id);
    });
    let cancelled = Callback::new(move |result: CancelPurchaseOrderResponse| {
        cancel_open.set(false);
        request_page(page_signals, None, Vec::new());
        load_detail(result.purchase_order_id);
    });
    let previous = move |_| {
        if loading.get_untracked() {
            return;
        }
        let mut history = cursor_history.get_untracked();
        if let Some(cursor) = history.pop() {
            request_page(page_signals, cursor, history);
        }
    };
    let next = move |_| {
        if loading.get_untracked() {
            return;
        }
        if let Some(cursor) = page.get_untracked().and_then(|value| value.next_cursor) {
            let mut history = cursor_history.get_untracked();
            history.push(current_cursor.get_untracked());
            request_page(page_signals, Some(cursor), history);
        }
    };

    view! {
        <div class="purchase-order-workspace split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <section class="data-section purchase-order-list split-master">
                <form class="purchase-order-toolbar" on:submit=move |event| { event.prevent_default(); request_page(page_signals, None, Vec::new()); }>
                    <div class="toolbar-summary"><strong>{move || page.get().map_or(0, |value| value.items.len())}</strong><span>"purchase orders"</span><PaneControls layout master_label="Purchase order table" detail_label="Purchase order detail"/></div>
                    <SearchField label="Search purchase orders".to_owned() placeholder="PO or supplier" value=search/>
                    <label><span class="sr-only">"Client"</span><select prop:value=move || owner.get() on:change=move |event| owner.set(event_target_value(&event))><option value="">"All clients"</option>{access.inventory_owners.clone().into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span class="sr-only">"Facility"</span><select prop:value=move || facility.get() on:change=move |event| facility.set(event_target_value(&event))><option value="">"All facilities"</option>{access.facilities.clone().into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span class="sr-only">"Status"</span><select prop:value=move || status.get() on:change=move |event| status.set(event_target_value(&event))><option value="">"All statuses"</option><option value="draft">"Draft"</option><option value="released">"Released"</option><option value="cancelled">"Cancelled"</option></select></label>
                    <button class="button secondary-action compact" type="submit" disabled=move || loading.get()>"Apply"</button>
                    <button class="icon-button" type="button" title="Refresh purchase orders" aria-label="Refresh purchase orders" disabled=move || loading.get() on:click=refresh><Icon icon=UiIcon::Refresh/></button>
                    <button class="button primary-action compact" type="button" on:click=move |_| { create_open.set(true); layout.show_detail(); }><Icon icon=UiIcon::Add/><span>"New PO"</span></button>
                </form>
                <div class="table-scroll">
                    <table class="dense-table purchase-order-table">
                        <thead><tr><th>"PO"</th><th>"Status"</th><th class="numeric">"Ordered"</th><th class="numeric">"Inbound"</th><th class="numeric">"Available"</th><th class="numeric">"Received"</th><th class="numeric">"Open"</th><th>"Due"</th><th>"Supplier"</th><th>"Client"</th><th>"Facility"</th><th class="numeric">"Lines"</th></tr></thead>
                        <tbody>{move || page.get().map(|current| current.items.into_iter().map(|entry| { let id=entry.purchase_order_id; let active=selected_id.get()==Some(id) && !create_open.get(); view! { <tr class:active-row=active><td><button type="button" class="row-link" on:click=move |_| load_detail(id)>{entry.number}</button></td><td><span class=status_class(entry.status)>{status_label(entry.status)}</span></td><td class="numeric">{format_quantity(entry.total_ordered_quantity)}</td><td class="numeric">{format_quantity(entry.total_active_inbound_quantity)}</td><td class="numeric">{format_quantity(entry.total_available_to_notify_quantity)}</td><td class="numeric">{format_quantity(entry.total_received_quantity)}</td><td class="numeric"><strong>{format_quantity(entry.total_open_receipt_quantity)}</strong></td><td>{entry.expected_by.as_deref().map(short_timestamp).unwrap_or_else(|| "Not supplied".into())}</td><td>{entry.supplier}</td><td>{entry.inventory_owner_name}</td><td>{entry.facility_name}</td><td class="numeric">{entry.line_count}</td></tr> } }).collect_view())}</tbody>
                    </table>
                    <Show when=move || !loading.get() && page.get().is_some_and(|value| value.items.is_empty())><p class="empty-state">"No purchase orders match these filters."</p></Show>
                </div>
                <Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
                <footer class="table-pagination"><span>{move || page.get().map_or_else(|| "No records".into(), |value| format!("{} records on this page", value.items.len()))}</span><div><button class="button quiet-action compact" type="button" disabled=move || loading.get() || cursor_history.get().is_empty() on:click=previous>"Previous"</button><button class="button quiet-action compact" type="button" disabled=move || loading.get() || page.get().and_then(|value| value.next_cursor).is_none() on:click=next>"Next"</button></div></footer>
            </section>
            <SplitPaneHandle layout/>
            <section class="data-section purchase-order-detail split-detail">
                <Show when=move || create_open.get() fallback=move || view! {
                    <Show when=move || selected.get().is_some() fallback=move || view! { <div class="detail-empty"><h2>"Purchase order details"</h2><p>"Select a purchase order to inspect supplier demand and release evidence."</p></div> }>
                        {move || selected.get().map(|detail| view! { <PurchaseOrderDetail detail on_released=released on_cancel=Callback::new(move |_| cancel_open.set(true)) on_unauthorized/> })}
                    </Show>
                }>
                    <CreatePurchaseOrderPanel access=access.clone() on_close=Callback::new(move |_| create_open.set(false)) on_created=created on_unauthorized/>
                </Show>
                <Show when=move || detail_loading.get()><div class="panel-loading">"Loading purchase order..."</div></Show>
                <Show when=move || detail_error.get().is_some()>{move || detail_error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
            </section>
        </div>
        <Show when=move || cancel_open.get() && selected.get().is_some()>
            {move || selected.get().map(|detail| view! { <CancelPurchaseOrderDialog detail on_close=Callback::new(move |_| cancel_open.set(false)) on_cancelled=cancelled on_unauthorized/> })}
        </Show>
    }
}

#[component]
fn PurchaseOrderDetail(
    detail: PurchaseOrderDetailResponse,
    on_released: Callback<i64>,
    on_cancel: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(ReleasePurchaseOrderRequest, String)>);
    let toasts = use_toast_bus();
    let summary = detail.summary.clone();
    let asn_summary = detail.summary.clone();
    let asn_lines = detail.lines.clone();
    let asn_open = RwSignal::new(false);
    let purchase_order_id = summary.purchase_order_id;
    let can_release = summary.status == PurchaseOrderStatus::Draft;
    let can_create_asn = summary.status == PurchaseOrderStatus::Released
        && summary.total_available_to_notify_quantity > 0;
    let can_cancel = summary.cancellation_ready;
    let cancellation = summary.cancellation_reason.map(|reason| {
        let note = summary.cancellation_note.clone();
        let at = summary.cancelled_at.clone();
        view! { <section class="purchase-order-cancellation-evidence"><div><span>"Cancellation"</span><strong>{cancellation_reason_label(reason)}</strong></div>{note.map(|value| view! { <p>{value}</p> })}<small>{at.as_deref().map(short_timestamp).unwrap_or_else(|| "Time unavailable".into())}</small></section> }
    });
    let release = move |_| {
        if pending.get_untracked() {
            return;
        }
        let request = ReleasePurchaseOrderRequest {
            expected_revision: Revision::new(summary.revision.get()).unwrap_or(summary.revision),
        };
        let key = retry
            .get_untracked()
            .filter(|(saved, _)| saved == &request)
            .map_or_else(api::new_idempotency_key, |(_, key)| key);
        retry.set(Some((request.clone(), key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::release_purchase_order(purchase_order_id, &request, &key).await {
                Ok(_) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success("Purchase order released.");
                    on_released.run(purchase_order_id);
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
    view! {
        <Show when=move || asn_open.get()>
            <CreatePurchaseOrderAsnPanel
                summary=asn_summary.clone()
                lines=asn_lines.clone()
                on_close=Callback::new(move |_| asn_open.set(false))
                on_created=Callback::new(move |_| {
                    asn_open.set(false);
                    on_released.run(purchase_order_id);
                })
                on_unauthorized
            />
        </Show>
        <div class="purchase-order-detail-content" style:display=move || if asn_open.get() { "none" } else { "grid" }>
            <header class="detail-heading"><div><span class="eyebrow">{format!("Purchase order #{}", summary.purchase_order_id)}</span><h2>{summary.number.clone()}</h2><p>{summary.supplier.clone()}</p></div><span class=status_class(summary.status)>{status_label(summary.status)}</span></header>
            <dl class="summary-grid"><div><dt>"Client"</dt><dd>{summary.inventory_owner_name}</dd></div><div><dt>"Facility"</dt><dd>{summary.facility_name}</dd></div><div><dt>"Expected"</dt><dd>{summary.expected_by.as_deref().map(short_timestamp).unwrap_or_else(|| "Not supplied".into())}</dd></div><div><dt>"Ordered"</dt><dd>{format_quantity(summary.total_ordered_quantity)}</dd></div><div><dt>"ASN history"</dt><dd>{format_quantity(summary.total_historical_asn_quantity)}</dd></div><div><dt>"Active inbound"</dt><dd>{format_quantity(summary.total_active_inbound_quantity)}</dd></div><div><dt>"Available to notify"</dt><dd><strong>{format_quantity(summary.total_available_to_notify_quantity)}</strong></dd></div><div><dt>"Received"</dt><dd>{format_quantity(summary.total_received_quantity)}</dd></div><div><dt>"Exceptions"</dt><dd>{format_quantity(summary.total_rejected_quantity + summary.total_missing_quantity)}</dd></div><div class="summary-emphasis"><dt>"Open receipt"</dt><dd><strong>{format_quantity(summary.total_open_receipt_quantity)}</strong></dd></div></dl>
            <div class="detail-section-heading"><h3>"Ordered items"</h3><span>{format!("{} lines", detail.lines.len())}</span></div>
            <div class="table-scroll"><table class="dense-table document-progress-table"><thead><tr><th>"Item"</th><th>"Supply"</th><th>"Receipt"</th></tr></thead><tbody>{detail.lines.into_iter().map(|line| { let exceptions=line.rejected_quantity+line.missing_quantity; view! { <tr><td><strong>{line.item_description}</strong><small>{format!("Item #{} · {}", line.item_id, line.uom)}</small></td><td><dl class="line-metrics"><div><dt>"ASN history"</dt><dd>{format_quantity(line.historical_asn_quantity)}</dd></div><div><dt>"Active inbound"</dt><dd>{format_quantity(line.active_inbound_quantity)}</dd></div><div><dt>"Available"</dt><dd><strong>{format_quantity(line.available_to_notify_quantity)}</strong></dd></div></dl></td><td><dl class="line-metrics"><div><dt>"Ordered"</dt><dd>{format_quantity(line.ordered_quantity)}</dd></div><div><dt>"Received"</dt><dd>{format_quantity(line.received_quantity)}</dd></div><div><dt>"Exceptions"</dt><dd>{format_quantity(exceptions)}</dd></div><div><dt>"Open"</dt><dd><strong>{format_quantity(line.open_receipt_quantity)}</strong></dd></div></dl></td></tr> } }).collect_view()}</tbody></table></div>
            {cancellation}
            <Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
            <footer class="detail-actions"><a class="button quiet-action" href="/inbound-asns">"Open inbound ASNs"</a>{can_cancel.then(|| view! { <button class="button danger-action" type="button" on:click=move |_| on_cancel.run(())>"Cancel PO"</button> })}{can_release.then(|| view! { <button class="button primary-action" type="button" disabled=move || pending.get() on:click=release><Icon icon=UiIcon::Orders/><span>{move || if pending.get() { "Releasing..." } else { "Release PO" }}</span></button> })}{can_create_asn.then(|| view! { <button class="button primary-action" type="button" on:click=move |_| asn_open.set(true)><Icon icon=UiIcon::Add/><span>"Create ASN"</span></button> })}</footer>
        </div>
    }
}

#[component]
fn CancelPurchaseOrderDialog(
    detail: PurchaseOrderDetailResponse,
    on_close: Callback<()>,
    on_cancelled: Callback<CancelPurchaseOrderResponse>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new(PurchaseOrderCancellationReason::DemandCancelled);
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CancelPurchaseOrderRequest, String)>);
    let note_input = NodeRef::<html::Textarea>::new();
    let toasts = use_toast_bus();
    let purchase_order_id = detail.summary.purchase_order_id;
    let revision = detail.summary.revision;
    let number = detail.summary.number;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let note_value = optional_text(&note.get_untracked());
        if reason.get_untracked() == PurchaseOrderCancellationReason::Other && note_value.is_none()
        {
            error.set(Some("Enter a note for the Other reason.".into()));
            if let Some(input) = note_input.get() {
                let _ = input.focus();
            }
            return;
        }
        let request = CancelPurchaseOrderRequest {
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
            match api::cancel_purchase_order(purchase_order_id, &request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success("Purchase order cancelled.");
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
    view! {
        <div class="purchase-order-dialog-backdrop" role="presentation">
            <form class="purchase-order-dialog purchase-order-cancel-dialog" role="dialog" aria-modal="true" aria-labelledby="purchase-order-cancel-title" on:submit=submit>
                <header><div><span class="eyebrow">"Terminal supplier action"</span><h2 id="purchase-order-cancel-title">{format!("Cancel {number}")}</h2></div><button type="button" class="icon-button" title="Close" aria-label="Close" on:click=move |_| on_close.run(())><Icon icon=UiIcon::Close/></button></header>
                <p>"The purchase order remains in history and cannot source more ASNs. Every source ASN must already be cancelled."</p>
                <div class="form-grid"><label><span>"Reason"</span><select on:change=move |event| reason.set(parse_cancellation_reason(&event_target_value(&event)))><option value="demand_cancelled">"Demand cancelled"</option><option value="supplier_cancelled">"Supplier cancelled"</option><option value="duplicate_order">"Duplicate order"</option><option value="other">"Other"</option></select></label><label><span>"Note"</span><textarea node_ref=note_input maxlength="500" placeholder="Optional unless reason is Other" aria-describedby="purchase-order-cancel-error" aria-invalid=move || if error.get().is_some() { "true" } else { "false" } prop:value=move || note.get() on:input=move |event| { note.set(event_target_value(&event)); error.set(None); }></textarea></label></div>
                <Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p id="purchase-order-cancel-error" class="inline-command-error" role="alert" aria-live="assertive">{message}</p> })}</Show>
                <footer><button class="button quiet-action" type="button" on:click=move |_| on_close.run(())>"Go back"</button><button class="button danger-action" type="submit" disabled=move || pending.get()>{move || if pending.get() { "Cancelling..." } else { "Cancel PO" }}</button></footer>
            </form>
        </div>
    }
}

#[component]
fn CreatePurchaseOrderAsnPanel(
    summary: PurchaseOrderSummaryResponse,
    lines: Vec<PurchaseOrderLineResponse>,
    on_close: Callback<()>,
    on_created: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let number = RwSignal::new(String::new());
    let expected = RwSignal::new(String::new());
    let lines = RwSignal::new(
        lines
            .into_iter()
            .filter(|line| line.available_to_notify_quantity > 0)
            .map(|line| DraftAsnLine {
                purchase_order_line_id: line.line_id,
                description: line.item_description,
                uom: line.uom,
                available_to_notify_quantity: line.available_to_notify_quantity,
                expected_quantity: line.available_to_notify_quantity.to_string(),
                lot: String::new(),
                serial: String::new(),
                expiration: String::new(),
            })
            .collect::<Vec<_>>(),
    );
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CreatePurchaseOrderAsnRequest, String)>);
    let toasts = use_toast_bus();
    let purchase_order_id = summary.purchase_order_id;
    let purchase_order_revision = summary.revision;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let asn_number = number.get_untracked().trim().to_owned();
        if asn_number.is_empty() {
            error.set(Some("ASN number is required.".into()));
            return;
        }
        let expected_at = match parse_optional_timestamp(&expected.get_untracked()) {
            Ok(value) => value.map(|value| value.to_rfc3339()),
            Err(message) => {
                error.set(Some(format!("Expected arrival: {message}")));
                return;
            }
        };
        let mut request_lines = Vec::new();
        for line in lines.get_untracked() {
            let quantity = match line.expected_quantity.trim().parse::<i64>() {
                Ok(0) => continue,
                Ok(value) if value > 0 && value <= line.available_to_notify_quantity => value,
                _ => {
                    error.set(Some(format!(
                        "{} quantity must be between 0 and {}.",
                        line.description, line.available_to_notify_quantity
                    )));
                    return;
                }
            };
            let expiration = match parse_optional_timestamp(&line.expiration) {
                Ok(value) => value.map(|value| value.to_rfc3339()),
                Err(message) => {
                    error.set(Some(format!("{} expiration: {message}", line.description)));
                    return;
                }
            };
            request_lines.push(CreatePurchaseOrderAsnLineRequest {
                purchase_order_line_id: line.purchase_order_line_id,
                expected_quantity: quantity,
                lot: optional_text(&line.lot),
                serial: optional_text(&line.serial),
                expiration,
            });
        }
        if request_lines.is_empty() {
            error.set(Some(
                "Enter a quantity for at least one purchase-order line.".into(),
            ));
            return;
        }
        let request = CreatePurchaseOrderAsnRequest {
            expected_purchase_order_revision: purchase_order_revision,
            number: asn_number,
            expected_at,
            lines: request_lines,
        };
        let key = retry
            .get_untracked()
            .filter(|(saved, _)| saved == &request)
            .map_or_else(api::new_idempotency_key, |(_, key)| key);
        retry.set(Some((request.clone(), key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::create_purchase_order_asn(purchase_order_id, &request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success(format!("ASN {} created from this PO.", result.number));
                    on_created.run(result.asn_id);
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
    view! {
        <form class="purchase-order-form po-asn-form" on:submit=submit>
            <header class="detail-heading"><div><span class="eyebrow">{format!("{} · {} available to notify", summary.number, format_quantity(summary.total_available_to_notify_quantity))}</span><h2>"Create ASN from PO"</h2><p>{summary.supplier}</p></div><button class="text-button" type="button" on:click=move |_| on_close.run(())>"Close"</button></header>
            <div class="form-grid two-column"><label><span>"ASN number"</span><input required maxlength="120" autofocus prop:value=move || number.get() on:input=move |event| number.set(event_target_value(&event))/></label><label><span>"Expected arrival"</span><input type="datetime-local" prop:value=move || expected.get() on:input=move |event| expected.set(event_target_value(&event))/></label></div>
            <section class="po-line-builder"><div class="detail-section-heading"><h3>"Expected freight"</h3><span>"0 excludes a line"</span></div><div class="table-scroll"><table class="dense-table po-asn-lines"><thead><tr><th>"Item"</th><th class="numeric">"Available"</th><th class="numeric">"ASN qty"</th><th>"Lot"</th><th>"Serial"</th><th>"Expiration"</th></tr></thead><tbody>{move || lines.get().into_iter().enumerate().map(|(index, line)| view! { <tr><td><strong>{line.description}</strong><small>{line.uom}</small></td><td class="numeric">{format_quantity(line.available_to_notify_quantity)}</td><td><input aria-label="Expected quantity" class="compact-number" type="number" min="0" max=line.available_to_notify_quantity prop:value=line.expected_quantity on:input=move |event| lines.update(|values| values[index].expected_quantity=event_target_value(&event))/></td><td><input aria-label="Lot" placeholder="Optional" prop:value=line.lot on:input=move |event| lines.update(|values| values[index].lot=event_target_value(&event))/></td><td><input aria-label="Serial" placeholder="Optional" prop:value=line.serial on:input=move |event| lines.update(|values| values[index].serial=event_target_value(&event))/></td><td><input aria-label="Expiration" type="datetime-local" prop:value=line.expiration on:input=move |event| lines.update(|values| values[index].expiration=event_target_value(&event))/></td></tr> }).collect_view()}</tbody></table></div></section>
            <Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
            <footer class="detail-actions"><button class="button quiet-action" type="button" on:click=move |_| on_close.run(())>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || pending.get()><Icon icon=UiIcon::Add/><span>{move || if pending.get() { "Creating..." } else { "Create ASN" }}</span></button></footer>
        </form>
    }
}

#[component]
fn CreatePurchaseOrderPanel(
    access: AccessScopeWorkspace,
    on_close: Callback<()>,
    on_created: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let owner = RwSignal::new(
        access
            .inventory_owners
            .first()
            .map_or_else(String::new, |value| value.id.to_string()),
    );
    let facility = RwSignal::new(
        access
            .facilities
            .first()
            .map_or_else(String::new, |value| value.id.to_string()),
    );
    let number = RwSignal::new(String::new());
    let supplier = RwSignal::new(String::new());
    let expected = RwSignal::new(String::new());
    let items = RwSignal::new(Vec::<
        wareboxes_api_contract::v1::InboundLoadEntryItemResponse,
    >::new());
    let items_loading = RwSignal::new(false);
    let item_id = RwSignal::new(String::new());
    let quantity = RwSignal::new(String::new());
    let lines = RwSignal::new(Vec::<DraftLine>::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CreatePurchaseOrderRequest, String)>);
    let toasts = use_toast_bus();
    let clients = access.inventory_owners;
    let facilities = access.facilities;

    Effect::new(move |_| {
        request_owner_items(owner.get(), items, items_loading, item_id, on_unauthorized)
    });
    let add_line = move |_| {
        let Ok(selected_item_id) = item_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose an item.".into()));
            return;
        };
        let Ok(ordered_quantity) = quantity.get_untracked().parse::<i64>() else {
            error.set(Some("Enter a whole ordered quantity.".into()));
            return;
        };
        if ordered_quantity <= 0 {
            error.set(Some("Ordered quantity must be greater than zero.".into()));
            return;
        }
        if lines
            .get_untracked()
            .iter()
            .any(|line| line.item_id == selected_item_id)
        {
            error.set(Some("That item is already on this purchase order.".into()));
            return;
        }
        let Some(item) = items
            .get_untracked()
            .into_iter()
            .find(|value| value.item_id == selected_item_id)
        else {
            error.set(Some("Refresh the client item list.".into()));
            return;
        };
        lines.update(|values| {
            values.push(DraftLine {
                item_id: selected_item_id,
                description: item
                    .description
                    .unwrap_or_else(|| format!("Item #{selected_item_id}")),
                uom: item.uom,
                ordered_quantity,
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
        let Ok(owner_id) = owner.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a client.".into()));
            return;
        };
        let Ok(facility_id) = facility.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a facility.".into()));
            return;
        };
        let expected_by = match parse_optional_timestamp(&expected.get_untracked()) {
            Ok(value) => value.map(|value| value.to_rfc3339()),
            Err(message) => {
                error.set(Some(format!("Expected delivery: {message}")));
                return;
            }
        };
        let source_lines = lines.get_untracked();
        if number.get_untracked().trim().is_empty() || supplier.get_untracked().trim().is_empty() {
            error.set(Some("PO number and supplier are required.".into()));
            return;
        }
        if source_lines.is_empty() {
            error.set(Some("Add at least one ordered item.".into()));
            return;
        }
        let request = CreatePurchaseOrderRequest {
            inventory_owner_id: owner_id,
            facility_id,
            number: number.get_untracked().trim().into(),
            supplier: supplier.get_untracked().trim().into(),
            expected_by,
            lines: source_lines
                .into_iter()
                .map(|line| CreatePurchaseOrderLineRequest {
                    item_id: line.item_id,
                    ordered_quantity: line.ordered_quantity,
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
            match api::create_purchase_order(&request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success(format!("Purchase order {} created.", result.number));
                    on_created.run(result.purchase_order_id);
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
    view! {
        <form class="purchase-order-form" on:submit=submit>
            <header class="detail-heading"><div><span class="eyebrow">"Supplier demand"</span><h2>"New purchase order"</h2></div><button class="text-button" type="button" on:click=move |_| on_close.run(())>"Close"</button></header>
            <div class="form-grid two-column"><label><span>"Client"</span><select required disabled=move || !lines.get().is_empty() prop:value=move || owner.get() on:change=move |event| { owner.set(event_target_value(&event)); lines.set(Vec::new()); }>{clients.into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label><label><span>"Facility"</span><select required prop:value=move || facility.get() on:change=move |event| facility.set(event_target_value(&event))>{facilities.into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label><label><span>"PO number"</span><input required maxlength="120" prop:value=move || number.get() on:input=move |event| number.set(event_target_value(&event))/></label><label><span>"Supplier"</span><input required maxlength="200" prop:value=move || supplier.get() on:input=move |event| supplier.set(event_target_value(&event))/></label><label class="full-width"><span>"Expected delivery"</span><input type="datetime-local" prop:value=move || expected.get() on:input=move |event| expected.set(event_target_value(&event))/></label></div>
            <section class="po-line-builder"><div class="detail-section-heading"><h3>"Ordered items"</h3><span>{move || format!("{} lines", lines.get().len())}</span></div><div class="po-line-inputs"><select aria-label="Item" prop:value=move || item_id.get() on:change=move |event| item_id.set(event_target_value(&event))><option value="">{move || if items_loading.get() { "Loading items" } else { "Choose item" }}</option>{move || items.get().into_iter().map(|item| view! { <option value=item.item_id>{item.description.unwrap_or_else(|| format!("Item #{}", item.item_id))}</option> }).collect_view()}</select><input aria-label="Quantity" type="number" min="1" placeholder="Qty" prop:value=move || quantity.get() on:input=move |event| quantity.set(event_target_value(&event))/><button class="button secondary-action compact" type="button" on:click=add_line>"Add line"</button></div><div class="table-scroll"><table class="dense-table"><tbody>{move || lines.get().into_iter().enumerate().map(|(index, line)| view! { <tr><td><strong>{line.description}</strong><small>{line.uom}</small></td><td class="numeric">{format_quantity(line.ordered_quantity)}</td><td><button class="icon-button danger" type="button" title="Remove line" aria-label="Remove line" on:click=move |_| lines.update(|values| { values.remove(index); })><Icon icon=UiIcon::Remove/></button></td></tr> }).collect_view()}</tbody></table></div></section>
            <Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
            <footer class="detail-actions"><button class="button quiet-action" type="button" on:click=move |_| on_close.run(())>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || pending.get()>{move || if pending.get() { "Creating..." } else { "Create purchase order" }}</button></footer>
        </form>
    }
}

fn request_page(
    signals: PageSignals,
    cursor: Option<OpaqueCursor>,
    history: Vec<Option<OpaqueCursor>>,
) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    signals.error.set(None);
    let filters = api::PurchaseOrderFilters {
        facility_id: parse_optional_id(&signals.facility.get_untracked()),
        inventory_owner_id: parse_optional_id(&signals.owner.get_untracked()),
        status: match signals.status.get_untracked().as_str() {
            "draft" => Some(PurchaseOrderStatus::Draft),
            "released" => Some(PurchaseOrderStatus::Released),
            "cancelled" => Some(PurchaseOrderStatus::Cancelled),
            _ => None,
        },
        search: optional_text(&signals.search.get_untracked()),
    };
    leptos::task::spawn_local(async move {
        match api::purchase_orders(filters, cursor.as_ref()).await {
            Ok(value) if signals.generation.get_untracked() == generation => {
                signals.page.set(Some(value));
                signals.current_cursor.set(cursor);
                signals.cursor_history.set(history);
                signals.loading.set(false);
            }
            Ok(_) => {}
            Err(value) if value.unauthorized => signals.on_unauthorized.run(()),
            Err(value) if signals.generation.get_untracked() == generation => {
                signals.error.set(Some(value.message));
                signals.loading.set(false);
            }
            Err(_) => {}
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn request_detail(
    purchase_order_id: i64,
    selected_id: RwSignal<Option<i64>>,
    selected: RwSignal<Option<PurchaseOrderDetailResponse>>,
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
        match api::purchase_order_detail(purchase_order_id).await {
            Ok(value)
                if generation.get_untracked() == request_generation
                    && selected_id.get_untracked() == Some(purchase_order_id) =>
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
    let Ok(owner_id) = owner.parse::<i64>() else {
        items.set(Vec::new());
        return;
    };
    loading.set(true);
    leptos::task::spawn_local(async move {
        match api::inbound_load_entry_items(owner_id).await {
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

fn parse_optional_id(value: &str) -> Option<i64> {
    value.parse().ok()
}
fn status_label(status: PurchaseOrderStatus) -> &'static str {
    match status {
        PurchaseOrderStatus::Draft => "Draft",
        PurchaseOrderStatus::Released => "Released",
        PurchaseOrderStatus::Cancelled => "Cancelled",
    }
}
fn status_class(status: PurchaseOrderStatus) -> &'static str {
    match status {
        PurchaseOrderStatus::Draft => "status-chip info",
        PurchaseOrderStatus::Released => "status-chip success",
        PurchaseOrderStatus::Cancelled => "status-chip neutral",
    }
}
fn parse_cancellation_reason(value: &str) -> PurchaseOrderCancellationReason {
    match value {
        "supplier_cancelled" => PurchaseOrderCancellationReason::SupplierCancelled,
        "duplicate_order" => PurchaseOrderCancellationReason::DuplicateOrder,
        "other" => PurchaseOrderCancellationReason::Other,
        _ => PurchaseOrderCancellationReason::DemandCancelled,
    }
}
fn cancellation_reason_label(reason: PurchaseOrderCancellationReason) -> &'static str {
    match reason {
        PurchaseOrderCancellationReason::SupplierCancelled => "Supplier cancelled",
        PurchaseOrderCancellationReason::DuplicateOrder => "Duplicate order",
        PurchaseOrderCancellationReason::DemandCancelled => "Demand cancelled",
        PurchaseOrderCancellationReason::Other => "Other",
    }
}
fn short_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
