use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CancelCrossDockWorkRequest, CrossDockCancellationReason, CrossDockPlanningOptionPage,
    CrossDockPlanningOptionResponse, CrossDockWorkPage, CrossDockWorkResponse, CrossDockWorkStatus,
    PlanCrossDockWorkRequest,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api::{self, CrossDockFilters};
use crate::components::{Icon, UiIcon};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CrossDockTab {
    Planning,
    Work,
}

#[derive(Clone, PartialEq, Eq)]
struct PlanAttempt {
    order_id: i64,
    request: PlanCrossDockWorkRequest,
    key: String,
}

#[derive(Clone, PartialEq, Eq)]
struct CancelAttempt {
    work_id: i64,
    request: CancelCrossDockWorkRequest,
    key: String,
}

#[component]
pub(crate) fn CrossDockWorkspace(
    initial_options: Option<CrossDockPlanningOptionPage>,
    initial_work: Option<CrossDockWorkPage>,
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let tab = RwSignal::new(CrossDockTab::Planning);
    let options = RwSignal::new(initial_options);
    let work = RwSignal::new(initial_work);
    let facility_filter = RwSignal::new(String::new());
    let owner_filter = RwSignal::new(String::new());
    let work_status = RwSignal::new(String::new());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let generation = RwSignal::new(0_u64);
    let plan_target = RwSignal::new(None::<CrossDockPlanningOptionResponse>);
    let cancel_target = RwSignal::new(None::<CrossDockWorkResponse>);
    let access = StoredValue::new(access);

    let refresh = Callback::new(move |()| {
        refresh_pages(
            options,
            work,
            loading,
            error,
            generation,
            facility_filter,
            owner_filter,
            work_status,
            on_unauthorized,
        )
    });
    let apply = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        refresh.run(());
    };

    view! {
        <section class="page-heading cross-dock-heading">
            <div>
                <p class="eyebrow">"Inbound flow"</p>
                <h1>"Cross-dock"</h1>
                <p>"Match newly received stock to open order demand and execute scanner work."</p>
            </div>
            <button
                type="button"
                class="button secondary-action compact"
                title="Refresh cross-dock data"
                disabled=move || loading.get()
                on:click=move |_| refresh.run(())
            >
                <Icon icon=UiIcon::Refresh/>
                <span>"Refresh"</span>
            </button>
        </section>

        <form class="cross-dock-toolbar" on:submit=apply>
            <div class="segmented-control" role="tablist" aria-label="Cross-dock views">
                <button id="cross-dock-planning-tab" type="button" role="tab" aria-controls="cross-dock-planning-panel" class:active=move || tab.get() == CrossDockTab::Planning aria-selected=move || (tab.get() == CrossDockTab::Planning).to_string() on:click=move |_| tab.set(CrossDockTab::Planning)>"Planning"</button>
                <button id="cross-dock-work-tab" type="button" role="tab" aria-controls="cross-dock-work-panel" class:active=move || tab.get() == CrossDockTab::Work aria-selected=move || (tab.get() == CrossDockTab::Work).to_string() on:click=move |_| tab.set(CrossDockTab::Work)>"Work queue"</button>
            </div>
            <label>
                <span class="sr-only">"Facility"</span>
                <select aria-label="Facility" prop:value=move || facility_filter.get() on:change=move |event| facility_filter.set(event_target_value(&event))>
                    <option value="">"All facilities"</option>
                    {move || access.get_value().facilities.into_iter().map(|facility| view! { <option value=facility.id.to_string()>{facility.name}</option> }).collect_view()}
                </select>
            </label>
            <label>
                <span class="sr-only">"Client"</span>
                <select aria-label="Client" prop:value=move || owner_filter.get() on:change=move |event| owner_filter.set(event_target_value(&event))>
                    <option value="">"All clients"</option>
                    {move || access.get_value().inventory_owners.into_iter().map(|owner| view! { <option value=owner.id.to_string()>{owner.name}</option> }).collect_view()}
                </select>
            </label>
            <Show when=move || tab.get() == CrossDockTab::Work>
                <label>
                    <span class="sr-only">"Work status"</span>
                    <select aria-label="Work status" prop:value=move || work_status.get() on:change=move |event| work_status.set(event_target_value(&event))>
                        <option value="">"All work"</option>
                        <option value="pending">"Pending"</option>
                        <option value="in_progress">"In progress"</option>
                        <option value="completed">"Completed"</option>
                        <option value="cancelled">"Cancelled"</option>
                    </select>
                </label>
            </Show>
            <button type="submit" class="button secondary-action compact" disabled=move || loading.get()>"Apply"</button>
        </form>

        <Show when=move || error.get().is_some()>
            <p class="inline-command-error cross-dock-error" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        <section id="cross-dock-planning-panel" class="cross-dock-panel" role="tabpanel" aria-labelledby="cross-dock-planning-tab" hidden=move || tab.get() != CrossDockTab::Planning>
                <header><h2>"Actionable demand"</h2><span>{move || options.get().map_or(0, |page| page.items.len())}</span></header>
                <div class="cross-dock-table-scroll" aria-busy=move || loading.get().to_string()>
                    <table>
                        <caption class="sr-only">"Received stock eligible for cross-dock planning"</caption>
                        <thead><tr><th>"Order"</th><th>"Client"</th><th>"Item"</th><th>"Receipt"</th><th>"Source"</th><th class="numeric">"Available"</th><th class="numeric">"Demand"</th><th class="numeric">"Plan"</th><th aria-label="Actions"></th></tr></thead>
                        <tbody>
                            {move || match options.get() {
                                Some(page) if page.items.is_empty() => view! { <tr><td colspan="9" class="empty-row" role="status" aria-live="polite">"No eligible received stock currently matches open order demand."</td></tr> }.into_any(),
                                Some(page) => page.items.into_iter().map(|option| {
                                let plan = option.clone();
                                view! { <tr>
                                    <td><strong>{option.order_key}</strong><small>{format!("Line {}", option.order_line_key)}</small></td>
                                    <td>{option.inventory_owner_name}</td>
                                    <td><strong>{option.item_description.clone().unwrap_or_else(|| format!("Item #{}",option.item_id))}</strong><small>{format!("{} / {}",option.primary_sku.clone().unwrap_or_else(||"No SKU".into()),option.uom)}</small></td>
                                    <td><strong>{option.inbound_load_reference.clone().unwrap_or_else(||format!("Load #{}",option.inbound_load_id))}</strong><small>{lot_serial_label(option.lot.as_deref(),option.serial.as_deref())}</small></td>
                                    <td><strong>{option.source_receiving_location.barcode}</strong><small>{option.facility_name}</small></td>
                                    <td class="numeric">{format_quantity(option.source_free_quantity)}</td>
                                    <td class="numeric">{format_quantity(option.unallocated_quantity)}</td>
                                    <td class="numeric"><strong>{format_quantity(option.maximum_plan_quantity)}</strong></td>
                                    <td><button type="button" class="button primary-action compact" disabled=move || loading.get() on:click=move |_| plan_target.set(Some(plan.clone()))><Icon icon=UiIcon::Add/><span>"Plan"</span></button></td>
                                </tr> }
                            }).collect_view().into_any(),
                                None => view! { <tr><td colspan="9" class="empty-row" role="status" aria-live="polite">"Loading planning options..."</td></tr> }.into_any(),
                            }}
                        </tbody>
                    </table>
                </div>
        </section>

        <section id="cross-dock-work-panel" class="cross-dock-panel" role="tabpanel" aria-labelledby="cross-dock-work-tab" hidden=move || tab.get() != CrossDockTab::Work>
                <header><h2>"Execution work"</h2><span>{move || work.get().map_or(0, |page| page.items.len())}</span></header>
                <div class="cross-dock-table-scroll" aria-busy=move || loading.get().to_string()>
                    <table>
                        <caption class="sr-only">"Cross-dock execution work matching the active filters"</caption>
                        <thead><tr><th>"Work"</th><th>"State"</th><th>"Order"</th><th>"Item"</th><th>"Route"</th><th class="numeric">"Qty"</th><th>"Client / facility"</th><th aria-label="Actions"></th></tr></thead>
                        <tbody>
                            {move || match work.get() {
                                Some(page) if page.items.is_empty() => view! { <tr><td colspan="8" class="empty-row" role="status" aria-live="polite">"No cross-dock work matches these filters."</td></tr> }.into_any(),
                                Some(page) => page.items.into_iter().map(|entry| {
                                let cancel = StoredValue::new(entry.clone());
                                let pending = entry.status == CrossDockWorkStatus::Pending;
                                view! { <tr>
                                    <td><strong>{format!("#{}",entry.work_id)}</strong><small>{format!("Plan #{}",entry.plan_id)}</small></td>
                                    <td><span class=status_class(entry.status)>{status_label(entry.status)}</span></td>
                                    <td><strong>{entry.order_key}</strong><small>{format!("Line {}",entry.order_line_key)}</small></td>
                                    <td><strong>{entry.item_description.clone().unwrap_or_else(||format!("Item #{}",entry.item_id))}</strong><small>{entry.primary_sku.clone().unwrap_or_else(||entry.uom.clone())}</small></td>
                                    <td><strong>{format!("{} -> {}",entry.source_receiving_location.barcode,entry.destination_pick_face.barcode)}</strong><small>{lot_serial_label(entry.lot.as_deref(),entry.serial.as_deref())}</small></td>
                                    <td class="numeric"><strong>{format_quantity(entry.quantity)}</strong><small>{entry.uom}</small></td>
                                    <td><strong>{entry.inventory_owner_name}</strong><small>{entry.facility_name}</small></td>
                                    <td><Show when=move || pending><button type="button" class="button danger-action compact" on:click=move |_| cancel_target.set(Some(cancel.get_value()))>"Cancel"</button></Show></td>
                                </tr> }
                            }).collect_view().into_any(),
                                None => view! { <tr><td colspan="8" class="empty-row" role="status" aria-live="polite">"Loading cross-dock work..."</td></tr> }.into_any(),
                            }}
                        </tbody>
                    </table>
                </div>
        </section>

        <Show when=move || plan_target.get().is_some()>
            {move || plan_target.get().map(|target| view! { <PlanDialog target on_close=Callback::new(move |()| plan_target.set(None)) on_saved=refresh on_unauthorized/> })}
        </Show>
        <Show when=move || cancel_target.get().is_some()>
            {move || cancel_target.get().map(|target| view! { <CancelDialog target on_close=Callback::new(move |()| cancel_target.set(None)) on_saved=refresh on_unauthorized/> })}
        </Show>
    }
}

#[component]
fn PlanDialog(
    target: CrossDockPlanningOptionResponse,
    on_close: Callback<()>,
    on_saved: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let destination = RwSignal::new(
        target
            .destination_pick_faces
            .first()
            .map_or_else(String::new, |location| location.location_id.to_string()),
    );
    let quantity = RwSignal::new(target.maximum_plan_quantity.to_string());
    let priority = RwSignal::new("25".to_owned());
    let instructions = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let retry = RwSignal::new(None::<PlanAttempt>);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();
    let target_for_submit = target.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let attempt = if let Some(attempt) = retry.get_untracked() {
            attempt
        } else {
            let Some(destination_id) = parse_positive(&destination.get_untracked()) else {
                error.set(Some("Choose a destination pick face.".into()));
                return;
            };
            let Some(quantity_value) = parse_positive(&quantity.get_untracked()) else {
                error.set(Some("Quantity must be positive.".into()));
                return;
            };
            if quantity_value > target_for_submit.maximum_plan_quantity {
                error.set(Some(format!(
                    "Quantity cannot exceed {}.",
                    target_for_submit.maximum_plan_quantity
                )));
                return;
            }
            let Some(priority_value) = priority
                .get_untracked()
                .parse::<i64>()
                .ok()
                .filter(|value| (0..=100).contains(value))
            else {
                error.set(Some("Priority must be 0 through 100.".into()));
                return;
            };
            PlanAttempt {
                order_id: target_for_submit.order_id,
                request: PlanCrossDockWorkRequest {
                    order_line_id: target_for_submit.order_line_id,
                    expected_order_revision: target_for_submit.order_revision,
                    source_receipt_inventory_transaction_id: target_for_submit
                        .source_receipt_inventory_transaction_id,
                    destination_pick_face_location_id: destination_id,
                    quantity: quantity_value,
                    priority: priority_value,
                    assigned_user_id: None,
                    due_at: None,
                    instructions: trimmed(&instructions.get_untracked()),
                },
                key: api::new_idempotency_key(),
            }
        };
        retry.set(Some(attempt.clone()));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let response =
                api::plan_cross_dock_work(attempt.order_id, &attempt.request, &attempt.key).await;
            if retry.get_untracked().as_ref() != Some(&attempt) {
                return;
            }
            pending.set(false);
            match response {
                Ok(result) => {
                    retry.set(None);
                    toasts.success(format!("Cross-dock work #{} planned.", result.work_id));
                    on_saved.run(());
                    on_close.run(());
                }
                Err(value) if value.unauthorized => {
                    retry.set(None);
                    on_unauthorized.run(());
                }
                Err(value) if value.ambiguous_outcome => error.set(Some(format!(
                    "{} Retry sends the exact saved plan.",
                    value.message
                ))),
                Err(value) => {
                    retry.set(None);
                    error.set(Some(value.message));
                    on_saved.run(());
                }
            }
        });
    };
    let locked = move || pending.get() || retry.get().is_some();
    view! { <div class="cross-dock-dialog-backdrop"><form class="cross-dock-dialog" role="dialog" aria-modal="true" aria-labelledby="cross-dock-plan-title" on:submit=submit>
        <header><div><p class="eyebrow">{target.order_key.clone()}</p><h2 id="cross-dock-plan-title">"Plan cross-dock work"</h2></div><button type="button" class="icon-button" aria-label="Close" disabled=locked on:click=move |_|on_close.run(())><Icon icon=UiIcon::Close/></button></header>
        <div class="cross-dock-summary"><span><small>"Item"</small><strong>{target.item_description.clone().unwrap_or_else(||format!("Item #{}",target.item_id))}</strong></span><span><small>"Source"</small><strong>{target.source_receiving_location.barcode.clone()}</strong></span><span><small>"Maximum"</small><strong>{format!("{} {}",format_quantity(target.maximum_plan_quantity),target.uom)}</strong></span></div>
        <div class="cross-dock-form-grid"><label><span>"Destination pick face"</span><select required disabled=locked prop:value=move||destination.get() on:change=move|event|destination.set(event_target_value(&event))>{target.destination_pick_faces.into_iter().map(|location|view!{<option value=location.location_id.to_string()>{location_label(&location)}</option>}).collect_view()}</select></label><label><span>"Quantity"</span><input type="number" min="1" max=target.maximum_plan_quantity disabled=locked prop:value=move||quantity.get() on:input=move|event|quantity.set(event_target_value(&event))/></label><label><span>"Priority"</span><input type="number" min="0" max="100" disabled=locked prop:value=move||priority.get() on:input=move|event|priority.set(event_target_value(&event))/></label><label class="wide"><span>"Instructions"</span><input maxlength="1000" disabled=locked prop:value=move||instructions.get() on:input=move|event|instructions.set(event_target_value(&event)) placeholder="Optional operator note"/></label></div>
        <Show when=move||error.get().is_some()><p class="inline-command-error" role="alert">{move||error.get().unwrap_or_default()}</p></Show>
        <footer><button type="button" class="button secondary-action compact" disabled=locked on:click=move |_|on_close.run(())>"Cancel"</button><button type="submit" class="button primary-action compact" disabled=move||pending.get()>{move||if pending.get(){"Planning..."}else if retry.get().is_some(){"Retry plan"}else{"Plan work"}}</button></footer>
    </form></div> }
}

#[component]
fn CancelDialog(
    target: CrossDockWorkResponse,
    on_close: Callback<()>,
    on_saved: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new("demand_changed".to_owned());
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let retry = RwSignal::new(None::<CancelAttempt>);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();
    let target_for_submit = target.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let attempt = if let Some(value) = retry.get_untracked() {
            value
        } else {
            let reason_value = match reason.get_untracked().as_str() {
                "demand_changed" => CrossDockCancellationReason::DemandChanged,
                "receipt_reassigned" => CrossDockCancellationReason::ReceiptReassigned,
                "operational_change" => CrossDockCancellationReason::OperationalChange,
                _ => CrossDockCancellationReason::Other,
            };
            let note_value = trimmed(&note.get_untracked());
            if reason_value == CrossDockCancellationReason::Other && note_value.is_none() {
                error.set(Some("A note is required for Other.".into()));
                return;
            }
            CancelAttempt {
                work_id: target_for_submit.work_id,
                request: CancelCrossDockWorkRequest {
                    expected_order_revision: target_for_submit.order_revision,
                    reason: reason_value,
                    note: note_value,
                },
                key: api::new_idempotency_key(),
            }
        };
        retry.set(Some(attempt.clone()));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let response =
                api::cancel_cross_dock_work(attempt.work_id, &attempt.request, &attempt.key).await;
            if retry.get_untracked().as_ref() != Some(&attempt) {
                return;
            }
            pending.set(false);
            match response {
                Ok(_) => {
                    retry.set(None);
                    toasts.success(format!("Cross-dock work #{} cancelled.", attempt.work_id));
                    on_saved.run(());
                    on_close.run(());
                }
                Err(value) if value.unauthorized => {
                    retry.set(None);
                    on_unauthorized.run(());
                }
                Err(value) if value.ambiguous_outcome => error.set(Some(format!(
                    "{} Retry sends the exact saved cancellation.",
                    value.message
                ))),
                Err(value) => {
                    retry.set(None);
                    error.set(Some(value.message));
                    on_saved.run(());
                }
            }
        });
    };
    let locked = move || pending.get() || retry.get().is_some();
    view! {<div class="cross-dock-dialog-backdrop"><form class="cross-dock-dialog compact-dialog" role="alertdialog" aria-modal="true" aria-labelledby="cross-dock-cancel-title" on:submit=submit><header><div><p class="eyebrow">{format!("Work #{}",target.work_id)}</p><h2 id="cross-dock-cancel-title">"Cancel cross-dock work"</h2></div><button type="button" class="icon-button" aria-label="Close" disabled=locked on:click=move |_|on_close.run(())><Icon icon=UiIcon::Close/></button></header><p>"The receipt remains available and the demand returns to allocation planning."</p><div class="cross-dock-form-grid"><label><span>"Reason"</span><select disabled=locked prop:value=move||reason.get() on:change=move|event|reason.set(event_target_value(&event))><option value="demand_changed">"Demand changed"</option><option value="receipt_reassigned">"Receipt reassigned"</option><option value="operational_change">"Operational change"</option><option value="other">"Other"</option></select></label><label><span>"Note"</span><input maxlength="500" disabled=locked prop:value=move||note.get() on:input=move|event|note.set(event_target_value(&event)) placeholder="Optional unless Other"/></label></div><Show when=move||error.get().is_some()><p class="inline-command-error" role="alert">{move||error.get().unwrap_or_default()}</p></Show><footer><button type="button" class="button secondary-action compact" disabled=locked on:click=move |_|on_close.run(())>"Keep work"</button><button type="submit" class="button danger-action compact" disabled=move||pending.get()>{move||if pending.get(){"Cancelling..."}else if retry.get().is_some(){"Retry cancellation"}else{"Cancel work"}}</button></footer></form></div>}
}

#[allow(clippy::too_many_arguments)]
fn refresh_pages(
    options: RwSignal<Option<CrossDockPlanningOptionPage>>,
    work: RwSignal<Option<CrossDockWorkPage>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    facility: RwSignal<String>,
    owner: RwSignal<String>,
    status: RwSignal<String>,
    on_unauthorized: Callback<()>,
) {
    let next = generation.get_untracked().saturating_add(1);
    generation.set(next);
    loading.set(true);
    error.set(None);
    let filters = CrossDockFilters {
        facility_id: parse_positive(&facility.get_untracked()),
        inventory_owner_id: parse_positive(&owner.get_untracked()),
        order_id: None,
        status: parse_status(&status.get_untracked()),
    };
    leptos::task::spawn_local(async move {
        let option_result = api::cross_dock_planning_options(filters, None).await;
        let work_result = api::cross_dock_work(filters, None).await;
        if generation.get_untracked() != next {
            return;
        }
        loading.set(false);
        match (option_result, work_result) {
            (Ok(option_page), Ok(work_page)) => {
                options.set(Some(option_page));
                work.set(Some(work_page));
            }
            (Err(value), _) | (_, Err(value)) if value.unauthorized => on_unauthorized.run(()),
            (Err(value), _) | (_, Err(value)) => error.set(Some(value.message)),
        }
    });
}

fn parse_positive(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|value| *value > 0)
}
fn trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn parse_status(value: &str) -> Option<CrossDockWorkStatus> {
    match value {
        "pending" => Some(CrossDockWorkStatus::Pending),
        "in_progress" => Some(CrossDockWorkStatus::InProgress),
        "completed" => Some(CrossDockWorkStatus::Completed),
        "cancelled" => Some(CrossDockWorkStatus::Cancelled),
        _ => None,
    }
}
fn status_label(value: CrossDockWorkStatus) -> &'static str {
    match value {
        CrossDockWorkStatus::Pending => "Pending",
        CrossDockWorkStatus::InProgress => "In progress",
        CrossDockWorkStatus::Completed => "Completed",
        CrossDockWorkStatus::Cancelled => "Cancelled",
    }
}
fn status_class(value: CrossDockWorkStatus) -> &'static str {
    match value {
        CrossDockWorkStatus::Pending => "status-badge status-open",
        CrossDockWorkStatus::InProgress => "status-badge status-held",
        CrossDockWorkStatus::Completed => "status-badge status-shipped",
        CrossDockWorkStatus::Cancelled => "status-badge status-cancelled",
    }
}
fn lot_serial_label(lot: Option<&str>, serial: Option<&str>) -> String {
    match (lot, serial) {
        (Some(lot), Some(serial)) => format!("Lot {lot} / Serial {serial}"),
        (Some(lot), None) => format!("Lot {lot}"),
        (None, Some(serial)) => format!("Serial {serial}"),
        (None, None) => "Uncontrolled stock".into(),
    }
}
fn location_label(value: &wareboxes_api_contract::v1::CrossDockLocationResponse) -> String {
    value.name.as_ref().map_or_else(
        || value.barcode.clone(),
        |name| format!("{name} / {}", value.barcode),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers_keep_filters_and_optional_notes_stable() {
        assert_eq!(
            parse_status("in_progress"),
            Some(CrossDockWorkStatus::InProgress)
        );
        assert_eq!(trimmed("  note  ").as_deref(), Some("note"));
        assert_eq!(parse_positive("0"), None);
    }
}
