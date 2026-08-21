use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CancelInboundAsnRequest, CancelInboundAsnResponse, CreateInboundAsnLineRequest,
    CreateInboundAsnRequest, InboundAsnCancellationReason, InboundAsnDetailResponse,
    InboundAsnExecutionStatus, InboundAsnPage, InboundAsnStatus, OpaqueCursor,
    PlanInboundAsnLoadRequest, PlanInboundAsnLoadResponse, Revision,
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
    page: RwSignal<Option<InboundAsnPage>>,
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
    expected_quantity: i64,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<String>,
}

#[component]
pub(crate) fn InboundAsnWorkspace(
    access: AccessScopeWorkspace,
    locations: Vec<Location>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let page = RwSignal::new(None::<InboundAsnPage>);
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
    let selected = RwSignal::new(None::<InboundAsnDetailResponse>);
    let detail_loading = RwSignal::new(false);
    let detail_error = RwSignal::new(None::<String>);
    let detail_generation = RwSignal::new(0_u64);
    let create_open = RwSignal::new(false);
    let plan_open = RwSignal::new(false);
    let cancel_open = RwSignal::new(false);
    let layout = SplitPaneState::new("inbound-asns", 700);
    let scoped_access = StoredValue::new(access.clone());
    let scoped_locations = StoredValue::new(locations.clone());
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

    let apply_filters = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        request_page(page_signals, None, Vec::new());
    };
    let refresh = move |_| {
        request_page(
            page_signals,
            current_cursor.get_untracked(),
            cursor_history.get_untracked(),
        );
        if let Some(asn_id) = selected_id.get_untracked() {
            request_detail(
                asn_id,
                selected_id,
                selected,
                detail_loading,
                detail_error,
                detail_generation,
                on_unauthorized,
            );
        }
    };
    let open_detail = move |asn_id: i64| {
        create_open.set(false);
        plan_open.set(false);
        cancel_open.set(false);
        layout.show_detail();
        selected_id.set(Some(asn_id));
        request_detail(
            asn_id,
            selected_id,
            selected,
            detail_loading,
            detail_error,
            detail_generation,
            on_unauthorized,
        );
    };
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
        if let Some(cursor) = page.get_untracked().and_then(|current| current.next_cursor) {
            let mut history = cursor_history.get_untracked();
            history.push(current_cursor.get_untracked());
            request_page(page_signals, Some(cursor), history);
        }
    };
    let created = Callback::new(move |asn_id: i64| {
        create_open.set(false);
        request_page(page_signals, None, Vec::new());
        open_detail(asn_id);
    });
    let planned = Callback::new(move |result: PlanInboundAsnLoadResponse| {
        plan_open.set(false);
        request_page(page_signals, None, Vec::new());
        open_detail(result.asn_id);
    });
    let cancelled = Callback::new(move |result: CancelInboundAsnResponse| {
        cancel_open.set(false);
        request_page(page_signals, None, Vec::new());
        open_detail(result.asn_id);
    });

    view! {
        <div class="inbound-asn-workspace split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <h1 class="sr-only">"Inbound ASNs"</h1>
            <section class="data-section inbound-asn-list split-master">
                <form class="inbound-asn-toolbar" on:submit=apply_filters>
                    <PaneControls layout master_label="ASN table" detail_label="ASN detail"/>
                    <div class="toolbar-summary">
                        <strong>{move || page.get().map_or(0, |value| value.items.len())}</strong>
                        <span>"source documents"</span>
                    </div>
                    <SearchField label="Search ASNs".to_owned() placeholder="ASN, PO, or supplier" value=search/>
                    <label><span class="sr-only">"Client"</span><select prop:value=move || owner.get() on:change=move |event| owner.set(event_target_value(&event))><option value="">"All clients"</option>{scoped_access.get_value().inventory_owners.into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span class="sr-only">"Facility"</span><select prop:value=move || facility.get() on:change=move |event| facility.set(event_target_value(&event))><option value="">"All facilities"</option>{scoped_access.get_value().facilities.into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span class="sr-only">"Status"</span><select prop:value=move || status.get() on:change=move |event| status.set(event_target_value(&event))><option value="">"All statuses"</option><option value="open">"Open"</option><option value="planned">"Planned"</option><option value="cancelled">"Cancelled"</option></select></label>
                    <button class="button secondary-action compact" type="submit" disabled=move || loading.get()>"Apply"</button>
                    <button class="icon-button" type="button" title="Refresh ASNs" aria-label="Refresh ASNs" disabled=move || loading.get() on:click=refresh><Icon icon=UiIcon::Refresh/></button>
                    <button class="button primary-action compact" type="button" on:click=move |_| { plan_open.set(false); create_open.set(true); layout.show_detail(); }><Icon icon=UiIcon::Add/><span>"New ASN"</span></button>
                </form>
                <div class="table-scroll">
                    <table class="dense-table inbound-asn-table">
                        <thead><tr><th>"ASN"</th><th>"Source PO"</th><th>"Document"</th><th>"Execution"</th><th class="numeric">"Expected"</th><th class="numeric">"Received"</th><th class="numeric">"Open"</th><th>"Due"</th><th>"Supplier"</th><th>"Client"</th><th>"Facility"</th><th class="numeric">"Lines"</th></tr></thead>
                        <tbody>
                            {move || page.get().map(|current| current.items.into_iter().map(|entry| {
                                let asn_id = entry.asn_id;
                                let active = selected_id.get() == Some(asn_id) && !create_open.get();
                                view! { <tr class:active-row=active><td><button type="button" class="row-link" on:click=move |_| open_detail(asn_id)>{entry.number}</button></td><td>{entry.purchase_order_number.unwrap_or_else(|| "Independent".into())}</td><td><span class=status_class(entry.status)>{status_label(entry.status)}</span></td><td>{entry.execution_status.map(execution_status_label).unwrap_or("Not planned")}</td><td class="numeric">{format_quantity(entry.total_expected_quantity)}</td><td class="numeric">{format_quantity(entry.total_received_quantity)}</td><td class="numeric"><strong>{format_quantity(entry.total_remaining_quantity)}</strong></td><td>{entry.expected_at.as_deref().map(short_wire_timestamp).unwrap_or_else(|| "Not supplied".into())}</td><td>{entry.supplier}</td><td>{entry.inventory_owner_name}</td><td>{entry.facility_name}</td><td class="numeric">{entry.line_count}</td></tr> }
                            }).collect_view())}
                        </tbody>
                    </table>
                    <Show when=move || !loading.get() && page.get().is_some_and(|value| value.items.is_empty())><p class="empty-state">"No advance shipping notices match these filters."</p></Show>
                </div>
                <Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
                <footer class="table-pagination"><span>{move || page.get().map_or_else(|| "No records".into(), |value| format!("{} records on this page", value.items.len()))}</span><div><button class="button quiet-action compact" type="button" disabled=move || loading.get() || cursor_history.get().is_empty() on:click=previous>"Previous"</button><button class="button quiet-action compact" type="button" disabled=move || loading.get() || page.get().and_then(|value| value.next_cursor).is_none() on:click=next>"Next"</button></div></footer>
            </section>
            <SplitPaneHandle layout/>
            <section class="data-section inbound-asn-detail split-detail">
                <Show when=move || create_open.get() fallback=move || view! {
                    <Show when=move || selected.get().is_some() fallback=move || view! { <div class="detail-empty"><h2>"ASN details"</h2><p>"Select a source document to inspect expected freight and load-planning evidence."</p></div> }>
                        {move || selected.get().map(|detail| view! { <AsnDetail detail=detail.clone() on_plan=Callback::new(move |_| plan_open.set(true)) on_cancel=Callback::new(move |_| cancel_open.set(true))/> })}
                    </Show>
                }>
                    <CreateAsnPanel access=access.clone() on_close=Callback::new(move |_| create_open.set(false)) on_created=created on_unauthorized/>
                </Show>
                <Show when=move || detail_loading.get()><div class="panel-loading">"Loading ASN..."</div></Show>
                <Show when=move || detail_error.get().is_some()>{move || detail_error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
            </section>
        </div>
        <Show when=move || plan_open.get() && selected.get().is_some()>{move || selected.get().map(|detail| view! { <PlanLoadDialog detail locations=scoped_locations.get_value() on_close=Callback::new(move |_| plan_open.set(false)) on_planned=planned on_unauthorized/> })}</Show>
        <Show when=move || cancel_open.get() && selected.get().is_some()>{move || selected.get().map(|detail| view! { <CancelAsnDialog detail on_close=Callback::new(move |_| cancel_open.set(false)) on_cancelled=cancelled on_unauthorized/> })}</Show>
    }
}

#[component]
fn AsnDetail(
    detail: InboundAsnDetailResponse,
    on_plan: Callback<()>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let can_plan = detail.summary.status == InboundAsnStatus::Open;
    let can_cancel = can_plan;
    let cancellation = detail.summary.cancellation_reason.map(|reason| {
        let note = detail.summary.cancellation_note.clone();
        let at = detail.summary.cancelled_at.clone();
        view! { <section class="asn-cancellation-evidence"><div><span>"Cancellation"</span><strong>{cancellation_reason_label(reason)}</strong></div>{note.map(|value| view! { <p>{value}</p> })}<small>{at.as_deref().map(short_wire_timestamp).unwrap_or_else(|| "Time unavailable".into())}</small></section> }
    });
    view! {
        <div class="inbound-asn-detail-content">
            <header class="detail-heading"><div><span class="eyebrow">{format!("ASN #{}", detail.summary.asn_id)}</span><h2>{detail.summary.number.clone()}</h2><p>{detail.summary.supplier.clone()}</p></div><span class=status_class(detail.summary.status)>{status_label(detail.summary.status)}</span></header>
            <dl class="summary-grid"><div><dt>"Client"</dt><dd>{detail.summary.inventory_owner_name}</dd></div><div><dt>"Facility"</dt><dd>{detail.summary.facility_name}</dd></div><div><dt>"Source"</dt><dd>{detail.summary.purchase_order_number.map(|number| format!("Purchase order {number}")).unwrap_or_else(|| "Independent ASN".into())}</dd></div><div><dt>"Due"</dt><dd>{detail.summary.expected_at.as_deref().map(short_wire_timestamp).unwrap_or_else(|| "Not supplied".into())}</dd></div><div><dt>"Load state"</dt><dd>{detail.summary.execution_status.map(execution_status_label).unwrap_or("Not planned")}</dd></div><div><dt>"Expected"</dt><dd>{format_quantity(detail.summary.total_expected_quantity)}</dd></div><div><dt>"Received"</dt><dd>{format_quantity(detail.summary.total_received_quantity)}</dd></div><div><dt>"Exceptions"</dt><dd>{format_quantity(detail.summary.total_rejected_quantity + detail.summary.total_missing_quantity)}</dd></div><div><dt>"Open"</dt><dd><strong>{format_quantity(detail.summary.total_remaining_quantity)}</strong></dd></div></dl>
            <div class="detail-section-heading"><h3>"Expected freight"</h3><span>{format!("{} lines", detail.lines.len())}</span></div>
            <div class="table-scroll"><table class="dense-table document-progress-table"><thead><tr><th>"Item"</th><th>"Identity"</th><th>"Receipt"</th></tr></thead><tbody>{detail.lines.into_iter().map(|line| { let exceptions=line.rejected_quantity+line.missing_quantity; view! { <tr><td><strong>{line.item_description}</strong><small>{format!("Item #{} · {}", line.item_id, line.uom)}</small></td><td>{identity_label(line.lot.as_deref(), line.serial.as_deref(), line.expiration.as_deref())}</td><td><dl class="line-metrics"><div><dt>"Expected"</dt><dd>{format_quantity(line.expected_quantity)}</dd></div><div><dt>"Received"</dt><dd>{format_quantity(line.received_quantity)}</dd></div><div><dt>"Exceptions"</dt><dd>{format_quantity(exceptions)}</dd></div><div><dt>"Open"</dt><dd><strong>{format_quantity(line.remaining_quantity)}</strong></dd></div></dl></td></tr> } }).collect_view()}</tbody></table></div>
            {cancellation}
            <footer class="detail-actions"><a class="button quiet-action" href="/loads">"Open inbound loads"</a>{can_cancel.then(|| view! { <button class="button danger-action" type="button" on:click=move |_| on_cancel.run(())>"Cancel ASN"</button> })}{can_plan.then(|| view! { <button class="button primary-action" type="button" on:click=move |_| on_plan.run(())><Icon icon=UiIcon::Loads/><span>"Plan load"</span></button> })}</footer>
        </div>
    }
}

#[component]
fn CancelAsnDialog(
    detail: InboundAsnDetailResponse,
    on_close: Callback<()>,
    on_cancelled: Callback<CancelInboundAsnResponse>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new(InboundAsnCancellationReason::SupplierCancelled);
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CancelInboundAsnRequest, String)>);
    let toasts = use_toast_bus();
    let asn_id = detail.summary.asn_id;
    let revision = detail.summary.revision;
    let number = detail.summary.number;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let note_value = optional_text(&note.get_untracked());
        if reason.get_untracked() == InboundAsnCancellationReason::Other && note_value.is_none() {
            error.set(Some("Enter a note for the Other reason.".into()));
            return;
        }
        let request = CancelInboundAsnRequest {
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
            match api::cancel_inbound_asn(asn_id, &request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success("Advance shipping notice cancelled.");
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
    view! { <div class="inbound-asn-dialog-backdrop" role="presentation"><form class="inbound-asn-dialog asn-cancel-dialog" role="dialog" aria-modal="true" aria-labelledby="asn-cancel-title" on:submit=submit><header><div><span class="eyebrow">"Terminal source action"</span><h2 id="asn-cancel-title">{format!("Cancel {number}")}</h2></div><button type="button" class="icon-button" title="Close" aria-label="Close" on:click=move |_| on_close.run(())><Icon icon=UiIcon::Close/></button></header><p>"The notice remains in history, and its unreceived quantity becomes available for a replacement ASN."</p><div class="form-grid"><label><span>"Reason"</span><select on:change=move |event| reason.set(parse_cancellation_reason(&event_target_value(&event)))><option value="supplier_cancelled">"Supplier cancelled"</option><option value="duplicate_notice">"Duplicate notice"</option><option value="order_changed">"Purchase order changed"</option><option value="other">"Other"</option></select></label><label><span>"Note"</span><textarea maxlength="500" placeholder="Optional unless reason is Other" prop:value=move || note.get() on:input=move |event| note.set(event_target_value(&event))></textarea></label></div><Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show><footer><button class="button quiet-action" type="button" on:click=move |_| on_close.run(())>"Go back"</button><button class="button danger-action" type="submit" disabled=move || pending.get()>{move || if pending.get() { "Cancelling..." } else { "Cancel ASN" }}</button></footer></form></div> }
}

#[component]
fn CreateAsnPanel(
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
    let lot = RwSignal::new(String::new());
    let serial = RwSignal::new(String::new());
    let expiration = RwSignal::new(String::new());
    let lines = RwSignal::new(Vec::<DraftLine>::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(CreateInboundAsnRequest, String)>);
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
        let Ok(expected_quantity) = quantity.get_untracked().parse::<i64>() else {
            error.set(Some("Enter a whole expected quantity.".into()));
            return;
        };
        if expected_quantity <= 0 {
            error.set(Some("Expected quantity must be greater than zero.".into()));
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
        let expiration_value = match parse_optional_timestamp(&expiration.get_untracked()) {
            Ok(value) => value.map(|value| value.to_rfc3339()),
            Err(message) => {
                error.set(Some(format!("Expiration: {message}")));
                return;
            }
        };
        if lines.get_untracked().iter().any(|line| {
            line.item_id == selected_item_id
                && line.lot == optional_text(&lot.get_untracked())
                && line.serial == optional_text(&serial.get_untracked())
                && line.expiration == expiration_value
        }) {
            error.set(Some("That item identity is already present.".into()));
            return;
        }
        lines.update(|values| {
            values.push(DraftLine {
                item_id: selected_item_id,
                description: item
                    .description
                    .unwrap_or_else(|| format!("Item #{selected_item_id}")),
                uom: item.uom,
                expected_quantity,
                lot: optional_text(&lot.get_untracked()),
                serial: optional_text(&serial.get_untracked()),
                expiration: expiration_value,
            })
        });
        quantity.set(String::new());
        lot.set(String::new());
        serial.set(String::new());
        expiration.set(String::new());
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
            Ok(value) => value.map(|value| value.to_rfc3339()),
            Err(message) => {
                error.set(Some(format!("Expected arrival: {message}")));
                return;
            }
        };
        let source_lines = lines.get_untracked();
        if number.get_untracked().trim().is_empty() || supplier.get_untracked().trim().is_empty() {
            error.set(Some("ASN number and supplier are required.".into()));
            return;
        }
        if source_lines.is_empty() {
            error.set(Some("Add at least one expected freight line.".into()));
            return;
        }
        let request = CreateInboundAsnRequest {
            inventory_owner_id: owner_id,
            facility_id,
            number: number.get_untracked().trim().into(),
            supplier: supplier.get_untracked().trim().into(),
            expected_at,
            lines: source_lines
                .into_iter()
                .map(|line| CreateInboundAsnLineRequest {
                    item_id: line.item_id,
                    expected_quantity: line.expected_quantity,
                    lot: line.lot,
                    serial: line.serial,
                    expiration: line.expiration,
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
            match api::create_inbound_asn(&request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success(format!("ASN {} created.", result.number));
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
        <form class="inbound-asn-form asn-create-form" on:submit=submit>
            <header class="detail-heading">
                <div><span class="eyebrow">"Source intake"</span><h2>"New advance shipping notice"</h2></div>
                <button class="text-button" type="button" on:click=move |_| on_close.run(())>"Close"</button>
            </header>
            <section class="asn-form-section" aria-labelledby="asn-source-document-title">
                <div class="detail-section-heading asn-form-section-heading">
                    <div><span class="eyebrow">"Source document"</span><h3 id="asn-source-document-title">"Notice details"</h3></div>
                    <p>"Identify the supplier, destination, and expected arrival."</p>
                </div>
                <div class="form-grid two-column asn-document-grid">
                    <label><span>"Client"</span><select required disabled=move || !lines.get().is_empty() prop:value=move || owner.get() on:change=move |event| { owner.set(event_target_value(&event)); lines.set(Vec::new()); }>{clients.into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span>"Facility"</span><select required prop:value=move || facility.get() on:change=move |event| facility.set(event_target_value(&event))>{facilities.into_iter().map(|item| view! { <option value=item.id>{item.name}</option> }).collect_view()}</select></label>
                    <label><span>"ASN number"</span><input required maxlength="120" placeholder="Supplier notice number" prop:value=move || number.get() on:input=move |event| number.set(event_target_value(&event))/></label>
                    <label><span>"Supplier"</span><input required maxlength="200" placeholder="Supplier or origin" prop:value=move || supplier.get() on:input=move |event| supplier.set(event_target_value(&event))/></label>
                    <label class="wide asn-arrival-field"><span>"Expected arrival"</span><input type="datetime-local" prop:value=move || expected.get() on:input=move |event| expected.set(event_target_value(&event))/></label>
                </div>
            </section>
            <section class="asn-form-section asn-line-builder" aria-labelledby="asn-expected-freight-title">
                <div class="detail-section-heading">
                    <div><span class="eyebrow">"Line authorization"</span><h3 id="asn-expected-freight-title">"Expected freight"</h3></div>
                    <span class="asn-line-count">{move || format!("{} lines", lines.get().len())}</span>
                </div>
                <div class="asn-line-inputs">
                    <label class="asn-line-item-field"><span>"Item"</span><select prop:value=move || item_id.get() on:change=move |event| item_id.set(event_target_value(&event))><option value="">{move || if items_loading.get() { "Loading items" } else { "Choose item" }}</option>{move || items.get().into_iter().map(|item| view! { <option value=item.item_id>{item.description.unwrap_or_else(|| format!("Item #{}", item.item_id))}</option> }).collect_view()}</select></label>
                    <label class="asn-line-quantity-field"><span>"Quantity"</span><input type="number" min="1" placeholder="Qty" prop:value=move || quantity.get() on:input=move |event| quantity.set(event_target_value(&event))/></label>
                    <label class="asn-line-lot-field"><span>"Lot"</span><input placeholder="Optional" prop:value=move || lot.get() on:input=move |event| lot.set(event_target_value(&event))/></label>
                    <label class="asn-line-serial-field"><span>"Serial"</span><input placeholder="Optional" prop:value=move || serial.get() on:input=move |event| serial.set(event_target_value(&event))/></label>
                    <label class="asn-line-expiration-field"><span>"Expiration"</span><input type="datetime-local" prop:value=move || expiration.get() on:input=move |event| expiration.set(event_target_value(&event))/></label>
                    <button class="button secondary-action compact" type="button" on:click=add_line><Icon icon=UiIcon::Add/><span>"Add line"</span></button>
                </div>
                <Show when=move || !lines.get().is_empty() fallback=|| view! { <p class="asn-lines-empty">"Add expected freight to build the receiving document."</p> }>
                    <div class="table-scroll asn-draft-line-scroll">
                        <table class="data-table asn-draft-line-table">
                            <caption class="sr-only">"Expected freight lines on this advance shipping notice"</caption>
                            <thead><tr><th>"Item"</th><th>"Identity"</th><th class="numeric">"Expected"</th><th class="icon-column"><span class="sr-only">"Actions"</span></th></tr></thead>
                            <tbody>{move || lines.get().into_iter().enumerate().map(|(index, line)| view! { <tr><td><strong>{line.description}</strong><small>{line.uom}</small></td><td>{identity_label(line.lot.as_deref(), line.serial.as_deref(), line.expiration.as_deref())}</td><td class="numeric"><strong>{format_quantity(line.expected_quantity)}</strong></td><td class="icon-column"><button class="icon-button danger" type="button" title="Remove line" aria-label="Remove line" on:click=move |_| lines.update(|values| { values.remove(index); })><Icon icon=UiIcon::Remove/></button></td></tr> }).collect_view()}</tbody>
                        </table>
                    </div>
                </Show>
            </section>
            <Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show>
            <footer class="detail-actions"><button class="button quiet-action" type="button" on:click=move |_| on_close.run(())>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || pending.get()>{move || if pending.get() { "Creating..." } else { "Create ASN" }}</button></footer>
        </form>
    }
}

#[component]
fn PlanLoadDialog(
    detail: InboundAsnDetailResponse,
    locations: Vec<Location>,
    on_close: Callback<()>,
    on_planned: Callback<PlanInboundAsnLoadResponse>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let dock = RwSignal::new(String::new());
    let carrier = RwSignal::new(String::new());
    let trailer = RwSignal::new(String::new());
    let seal = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(PlanInboundAsnLoadRequest, String)>);
    let toasts = use_toast_bus();
    let facility_id = detail.summary.facility_id;
    let asn_id = detail.summary.asn_id;
    let revision = detail.summary.revision;
    let docks = locations
        .into_iter()
        .filter(|location| {
            location.facility_id == facility_id
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
        let request = PlanInboundAsnLoadRequest {
            expected_revision: Revision::new(revision.get()).unwrap_or(revision),
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
            match api::plan_inbound_asn_load(asn_id, &request, &key).await {
                Ok(result) => {
                    pending.set(false);
                    retry.set(None);
                    toasts.success(format!(
                        "Inbound load {} planned from ASN.",
                        result.execution_barcode
                    ));
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
    view! { <div class="inbound-asn-dialog-backdrop" role="presentation"><form class="inbound-asn-dialog" role="dialog" aria-modal="true" aria-labelledby="asn-plan-title" on:submit=submit><header><div><span class="eyebrow">"Source-bound planning"</span><h2 id="asn-plan-title">{format!("Plan load from {}", detail.summary.number)}</h2></div><button type="button" class="icon-button" title="Close" aria-label="Close" on:click=move |_| on_close.run(())><Icon icon=UiIcon::Close/></button></header><div class="form-grid two-column"><label class="full-width"><span>"Receiving dock"</span><select required prop:value=move || dock.get() on:change=move |event| dock.set(event_target_value(&event))><option value="">"Choose dock"</option>{docks.into_iter().map(|location| { let label=location.name.or(location.barcode).unwrap_or_else(|| format!("Dock #{}", location.id)); view! { <option value=location.id>{label}</option> } }).collect_view()}</select></label><label><span>"Carrier"</span><input prop:value=move || carrier.get() on:input=move |event| carrier.set(event_target_value(&event))/></label><label><span>"Trailer"</span><input prop:value=move || trailer.get() on:input=move |event| trailer.set(event_target_value(&event))/></label><label><span>"Seal"</span><input prop:value=move || seal.get() on:input=move |event| seal.set(event_target_value(&event))/></label><div class="asn-plan-summary"><strong>{format_quantity(detail.summary.total_expected_quantity)}</strong><span>{format!(" units across {} source lines", detail.summary.line_count)}</span></div></div><Show when=move || error.get().is_some()>{move || error.get().map(|message| view! { <p class="inline-command-error">{message}</p> })}</Show><footer><button class="button quiet-action" type="button" on:click=move |_| on_close.run(())>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || pending.get()>{move || if pending.get() { "Planning..." } else { "Plan inbound load" }}</button></footer></form></div> }
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
    let filters = api::InboundAsnFilters {
        facility_id: parse_optional_id(&signals.facility.get_untracked()),
        inventory_owner_id: parse_optional_id(&signals.owner.get_untracked()),
        status: match signals.status.get_untracked().as_str() {
            "open" => Some(InboundAsnStatus::Open),
            "planned" => Some(InboundAsnStatus::Planned),
            "cancelled" => Some(InboundAsnStatus::Cancelled),
            _ => None,
        },
        search: optional_text(&signals.search.get_untracked()),
    };
    leptos::task::spawn_local(async move {
        match api::inbound_asns(filters, cursor.as_ref()).await {
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

fn request_detail(
    asn_id: i64,
    selected_id: RwSignal<Option<i64>>,
    selected: RwSignal<Option<InboundAsnDetailResponse>>,
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
        match api::inbound_asn_detail(asn_id).await {
            Ok(value)
                if generation.get_untracked() == request_generation
                    && selected_id.get_untracked() == Some(asn_id) =>
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
fn status_label(status: InboundAsnStatus) -> &'static str {
    match status {
        InboundAsnStatus::Open => "Open",
        InboundAsnStatus::Planned => "Planned",
        InboundAsnStatus::Cancelled => "Cancelled",
    }
}
fn status_class(status: InboundAsnStatus) -> &'static str {
    match status {
        InboundAsnStatus::Open => "status-chip info",
        InboundAsnStatus::Planned => "status-chip success",
        InboundAsnStatus::Cancelled => "status-chip neutral",
    }
}
fn parse_cancellation_reason(value: &str) -> InboundAsnCancellationReason {
    match value {
        "duplicate_notice" => InboundAsnCancellationReason::DuplicateNotice,
        "order_changed" => InboundAsnCancellationReason::OrderChanged,
        "other" => InboundAsnCancellationReason::Other,
        _ => InboundAsnCancellationReason::SupplierCancelled,
    }
}
fn cancellation_reason_label(reason: InboundAsnCancellationReason) -> &'static str {
    match reason {
        InboundAsnCancellationReason::SupplierCancelled => "Supplier cancelled",
        InboundAsnCancellationReason::DuplicateNotice => "Duplicate notice",
        InboundAsnCancellationReason::OrderChanged => "Purchase order changed",
        InboundAsnCancellationReason::Other => "Other",
    }
}
fn execution_status_label(status: InboundAsnExecutionStatus) -> &'static str {
    match status {
        InboundAsnExecutionStatus::Planned => "Planned",
        InboundAsnExecutionStatus::Scheduled => "Scheduled",
        InboundAsnExecutionStatus::Arrived => "Arrived",
        InboundAsnExecutionStatus::Receiving => "Receiving",
        InboundAsnExecutionStatus::Received => "Received",
        InboundAsnExecutionStatus::Rejected => "Rejected",
        InboundAsnExecutionStatus::Closed => "Closed",
        InboundAsnExecutionStatus::Cancelled => "Cancelled",
    }
}
fn short_wire_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}
fn identity_label(lot: Option<&str>, serial: Option<&str>, expiration: Option<&str>) -> String {
    let mut values = Vec::new();
    if let Some(value) = lot {
        values.push(format!("Lot {value}"));
    }
    if let Some(value) = serial {
        values.push(format!("Serial {value}"));
    }
    if let Some(value) = expiration {
        values.push(format!("Exp {}", short_wire_timestamp(value)));
    }
    if values.is_empty() {
        "No controlled identity".into()
    } else {
        values.join(" · ")
    }
}
