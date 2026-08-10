use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ConfigureItemTraceabilityPolicyRequest, ItemTraceabilityPolicyPage,
    ItemTraceabilityPolicyResponse, ItemTraceabilityPolicyStatus, OpaqueCursor,
    RetireItemTraceabilityPolicyRequest, Revision, TraceabilityRequirement,
};
use wareboxes_core::models::{Facility, InventoryOwner, Item};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

use super::{label_or_id, CatalogStore};

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingCommand {
    Configure {
        request: ConfigureItemTraceabilityPolicyRequest,
        key: String,
    },
    Retire {
        policy_id: i64,
        request: RetireItemTraceabilityPolicyRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser callbacks consume workspace signals")
)]
struct Signals {
    page: RwSignal<ItemTraceabilityPolicyPage>,
    owner_id: RwSignal<Option<i64>>,
    facility_id: RwSignal<Option<i64>>,
    item_id: RwSignal<Option<i64>>,
    status: RwSignal<ItemTraceabilityPolicyStatus>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
    generation: RwSignal<u64>,
    loading: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    retry: RwSignal<Option<PendingCommand>>,
    selected: RwSignal<Option<ItemTraceabilityPolicyResponse>>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[derive(Clone, Copy)]
struct Drafts {
    open: RwSignal<bool>,
    owner_id: RwSignal<Option<i64>>,
    facility_id: RwSignal<Option<i64>>,
    item_id: RwSignal<Option<i64>>,
    lot: RwSignal<TraceabilityRequirement>,
    serial: RwSignal<TraceabilityRequirement>,
    expiration: RwSignal<TraceabilityRequirement>,
    shelf_life: RwSignal<String>,
    expected_revision: RwSignal<Option<Revision>>,
    confirm_retire: RwSignal<bool>,
}

#[component]
pub(super) fn ItemTraceabilityPolicyCatalog(
    store: CatalogStore,
    can_supervise: bool,
) -> impl IntoView {
    let signals = Signals {
        page: RwSignal::new(ItemTraceabilityPolicyPage::new(Vec::new(), None)),
        owner_id: RwSignal::new(None),
        facility_id: RwSignal::new(None),
        item_id: RwSignal::new(None),
        status: RwSignal::new(ItemTraceabilityPolicyStatus::Active),
        cursor: RwSignal::new(None),
        history: RwSignal::new(Vec::new()),
        generation: RwSignal::new(0),
        loading: RwSignal::new(false),
        command_pending: RwSignal::new(false),
        retry: RwSignal::new(None),
        selected: RwSignal::new(None),
        error: RwSignal::new(None),
        on_unauthorized: store.on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = Drafts {
        open: RwSignal::new(false),
        owner_id: RwSignal::new(None),
        facility_id: RwSignal::new(None),
        item_id: RwSignal::new(None),
        lot: RwSignal::new(TraceabilityRequirement::NotTracked),
        serial: RwSignal::new(TraceabilityRequirement::NotTracked),
        expiration: RwSignal::new(TraceabilityRequirement::NotTracked),
        shelf_life: RwSignal::new(String::new()),
        expected_revision: RwSignal::new(None),
        confirm_retire: RwSignal::new(false),
    };
    load_first_page(signals);

    let refresh = Callback::new(move |_| load_first_page(signals));
    let select = Callback::new(move |policy: ItemTraceabilityPolicyResponse| {
        drafts.confirm_retire.set(false);
        signals.error.set(None);
        signals.selected.set(Some(policy));
    });
    let open_create = Callback::new(move |_| {
        drafts.owner_id.set(signals.owner_id.get_untracked());
        drafts.facility_id.set(signals.facility_id.get_untracked());
        drafts.item_id.set(signals.item_id.get_untracked());
        drafts.lot.set(TraceabilityRequirement::NotTracked);
        drafts.serial.set(TraceabilityRequirement::NotTracked);
        drafts.expiration.set(TraceabilityRequirement::NotTracked);
        drafts.shelf_life.set(String::new());
        drafts.expected_revision.set(None);
        signals.retry.set(None);
        signals.error.set(None);
        drafts.open.set(true);
    });
    let open_edit = Callback::new(move |policy: ItemTraceabilityPolicyResponse| {
        drafts.owner_id.set(Some(policy.inventory_owner_id));
        drafts.facility_id.set(Some(policy.facility_id));
        drafts.item_id.set(Some(policy.item_id));
        drafts.lot.set(policy.lot);
        drafts.serial.set(policy.serial);
        drafts.expiration.set(policy.expiration);
        drafts.shelf_life.set(
            policy
                .minimum_shelf_life_days
                .map_or_else(String::new, |days| days.to_string()),
        );
        drafts.expected_revision.set(Some(policy.revision));
        signals.retry.set(None);
        signals.error.set(None);
        drafts.open.set(true);
    });
    let submit = Callback::new(move |_| submit_configuration(store, signals, drafts));
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch_command(signals, drafts, command);
        }
    });
    let retire = Callback::new(move |policy: ItemTraceabilityPolicyResponse| {
        dispatch_command(
            signals,
            drafts,
            PendingCommand::Retire {
                policy_id: policy.item_traceability_policy_id,
                request: RetireItemTraceabilityPolicyRequest {
                    expected_revision: policy.revision,
                },
                key: api::new_idempotency_key(),
            },
        );
    });

    view! {
        <div class="catalog-layout item-traceability-policy-layout">
            <section class="data-section catalog-browser">
                <div class="catalog-toolbar item-traceability-policy-toolbar">
                    <label><span class="sr-only">"Client"</span><select prop:value=move || option_id(signals.owner_id.get()) on:change=move |event| { signals.owner_id.set(parse_id(&event_target_value(&event))); reset_filtered_page(signals); }><option value="">"All clients"</option>{move || owner_options(&store.data.get().clients)}</select></label>
                    <label><span class="sr-only">"Facility"</span><select prop:value=move || option_id(signals.facility_id.get()) on:change=move |event| { signals.facility_id.set(parse_id(&event_target_value(&event))); reset_filtered_page(signals); }><option value="">"All facilities"</option>{move || facility_options(&store.data.get().facilities)}</select></label>
                    <label><span class="sr-only">"Item"</span><select prop:value=move || option_id(signals.item_id.get()) on:change=move |event| { signals.item_id.set(parse_id(&event_target_value(&event))); reset_filtered_page(signals); }><option value="">"All items"</option>{move || item_options(&store.data.get().items)}</select></label>
                    <label><span class="sr-only">"Status"</span><select prop:value=move || status_wire(signals.status.get()) on:change=move |event| { signals.status.set(parse_status(&event_target_value(&event))); reset_filtered_page(signals); }><option value="active">"Active"</option><option value="retired">"Retired history"</option></select></label>
                    <button class="icon-button" type="button" title="Refresh" aria-label="Refresh item traceability policies" disabled=move || signals.loading.get() on:click=move |_| refresh.run(())><Icon icon=UiIcon::Refresh/></button>
                    {can_supervise.then(|| view! { <button class="button primary-action compact" type="button" on:click=move |_| open_create.run(())>"New policy"</button> })}
                </div>
                <div class="table-scroll catalog-table-scroll">
                    <table class="data-table catalog-table item-traceability-policy-table">
                        <caption class="sr-only">"Item traceability policies"</caption>
                        <thead><tr><th>"Item"</th><th>"Client"</th><th>"Facility"</th><th>"UOM"</th><th>"Lot"</th><th>"Serial"</th><th>"Expiration"</th><th class="numeric">"Min life"</th><th>"Status"</th></tr></thead>
                        <tbody>{move || policy_rows(signals, select)}</tbody>
                    </table>
                </div>
                <footer class="table-footer"><span>{move || if signals.loading.get(){"Refreshing...".into()}else{format!("{} on this page",signals.page.get().items.len())}}</span><button class="button secondary-action compact" type="button" disabled=move || signals.loading.get() || signals.history.get().is_empty() on:click=move |_| previous_page(signals)>"Previous"</button><button class="button secondary-action compact" type="button" disabled=move || signals.loading.get() || signals.page.get().next_cursor.is_none() on:click=move |_| next_page(signals)>"Next"</button></footer>
            </section>
            <aside class="data-section catalog-editor" aria-label="Item traceability policy details">
                {move || signals.selected.get().map(|policy| policy_detail(policy,can_supervise,signals,drafts,open_edit,retire,retry)).unwrap_or_else(|| view!{<div class="catalog-editor-empty"><strong>"Select a traceability policy"</strong><p>"Review required item identities and receiving shelf life."</p></div>}.into_any())}
            </aside>
        </div>
        <Show when=move || drafts.open.get()>{move || configuration_dialog(store,signals,drafts,submit,retry)}</Show>
    }
}

fn policy_rows(signals: Signals, select: Callback<ItemTraceabilityPolicyResponse>) -> AnyView {
    let items = signals.page.get().items;
    if items.is_empty() {
        return view! {<tr><td class="table-empty-row" colspan="9">{if signals.loading.get(){"Loading policies..."}else{"No policies match this view."}}</td></tr>}.into_any();
    }
    items
        .into_iter()
        .map(|policy| {
            let selected = signals.selected.get().as_ref().is_some_and(|value| {
                value.item_traceability_policy_id == policy.item_traceability_policy_id
            });
            let row = policy.clone();
            view! {
                <tr class:selected=selected>
                    <td><button class="catalog-row-link" type="button" on:click=move |_| select.run(row.clone())>{policy.item_description}</button></td>
                    <td>{policy.inventory_owner_name}</td><td>{policy.facility_name}</td><td>{policy.uom}</td>
                    <td>{requirement_label(policy.lot)}</td><td>{requirement_label(policy.serial)}</td><td>{requirement_label(policy.expiration)}</td>
                    <td class="numeric">{policy.minimum_shelf_life_days.map_or_else(||"—".into(),|days|format!("{days}d"))}</td>
                    <td><span class=if policy.status==ItemTraceabilityPolicyStatus::Active{"status-chip success"}else{"status-chip neutral"}>{status_label(policy.status)}</span></td>
                </tr>
            }
        })
        .collect_view()
        .into_any()
}

#[allow(clippy::too_many_arguments)]
fn policy_detail(
    policy: ItemTraceabilityPolicyResponse,
    can_supervise: bool,
    signals: Signals,
    drafts: Drafts,
    open_edit: Callback<ItemTraceabilityPolicyResponse>,
    retire: Callback<ItemTraceabilityPolicyResponse>,
    retry: Callback<()>,
) -> AnyView {
    let editable = can_supervise && policy.status == ItemTraceabilityPolicyStatus::Active;
    let edit_policy = policy.clone();
    let retire_policy = StoredValue::new(policy.clone());
    view! {
        <div class="catalog-editor-form item-traceability-policy-detail">
            <header class="catalog-editor-heading"><div><p class="eyebrow">{policy.inventory_owner_name.clone()}</p><h2>{policy.item_description.clone()}</h2><small>{policy.facility_name.clone()}</small></div><span class=if policy.status==ItemTraceabilityPolicyStatus::Active{"status-chip success"}else{"status-chip neutral"}>{status_label(policy.status)}</span></header>
            <dl class="catalog-summary-grid"><div><dt>"UOM"</dt><dd>{policy.uom.clone()}</dd></div><div><dt>"Revision"</dt><dd>{policy.revision.get()}</dd></div><div><dt>"Lot"</dt><dd>{requirement_label(policy.lot)}</dd></div><div><dt>"Serial"</dt><dd>{requirement_label(policy.serial)}</dd></div><div><dt>"Expiration"</dt><dd>{requirement_label(policy.expiration)}</dd></div><div><dt>"Minimum shelf life"</dt><dd>{policy.minimum_shelf_life_days.map_or_else(||"Not set".into(),|days|format!("{days} days"))}</dd></div></dl>
            <p class="catalog-command-note">"Serial-controlled inventory is limited to one on-hand unit per serial identity. Shelf life is measured when the batch is created."</p>
            <Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show>
            <Show when=move || signals.retry.get().is_some()><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| retry.run(())>"Retry exact command"</button></Show>
            {editable.then(|| view!{<footer class="catalog-editor-actions"><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| open_edit.run(edit_policy.clone())>"Reconfigure"</button><Show when=move || !drafts.confirm_retire.get()><button class="button danger-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| drafts.confirm_retire.set(true)>"Retire policy"</button></Show><Show when=move || drafts.confirm_retire.get()><span class="destructive-confirm"><span>"Retire this policy?"</span><button class="button secondary-action compact" type="button" on:click=move |_| drafts.confirm_retire.set(false)>"Keep"</button><button class="button danger-action compact" type="button" disabled=move || signals.command_pending.get() on:click=move |_| retire.run(retire_policy.get_value())>"Confirm"</button></span></Show></footer>})}
        </div>
    }.into_any()
}

fn configuration_dialog(
    store: CatalogStore,
    signals: Signals,
    drafts: Drafts,
    submit: Callback<()>,
    retry: Callback<()>,
) -> AnyView {
    let editing = drafts.expected_revision.get().is_some();
    view! {
        <div class="modal-backdrop" role="presentation"><section class="modal-panel item-traceability-policy-dialog" role="dialog" aria-modal="true" aria-labelledby="item-traceability-policy-dialog-title">
            <header><div><p class="eyebrow">"Inventory identity governance"</p><h2 id="item-traceability-policy-dialog-title">{if editing{"Reconfigure traceability policy"}else{"New traceability policy"}}</h2></div><button class="icon-button" type="button" aria-label="Close traceability policy dialog" disabled=move || signals.command_pending.get() on:click=move |_| drafts.open.set(false)><Icon icon=UiIcon::Close/></button></header>
            <fieldset disabled=move || signals.command_pending.get()><div class="item-traceability-policy-form-grid"><label><span>"Client"</span><select required disabled=editing prop:value=move || option_id(drafts.owner_id.get()) on:change=move |event| drafts.owner_id.set(parse_id(&event_target_value(&event)))><option value="">"Select client"</option>{move || owner_options(&store.data.get().clients)}</select></label><label><span>"Facility"</span><select required disabled=editing prop:value=move || option_id(drafts.facility_id.get()) on:change=move |event| drafts.facility_id.set(parse_id(&event_target_value(&event)))><option value="">"Select facility"</option>{move || facility_options(&store.data.get().facilities)}</select></label><label><span>"Item"</span><select required disabled=editing prop:value=move || option_id(drafts.item_id.get()) on:change=move |event| drafts.item_id.set(parse_id(&event_target_value(&event)))><option value="">"Select item"</option>{move || item_options(&store.data.get().items)}</select></label><label><span>"Lot identity"</span>{requirement_select(drafts.lot)}</label><label><span>"Serial identity"</span>{requirement_select(drafts.serial)}</label><label><span>"Expiration"</span>{requirement_select(drafts.expiration)}</label><label class="shelf-life-field"><span>"Minimum shelf life (days)"</span><input type="number" min="0" max="36500" placeholder="No minimum" disabled=move || drafts.expiration.get()!=TraceabilityRequirement::Required prop:value=move || drafts.shelf_life.get() on:input=move |event| drafts.shelf_life.set(event_target_value(&event))/></label></div></fieldset>
            <Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show><Show when=move || signals.retry.get().is_some()><p class="catalog-command-note">"Retry sends the exact saved policy and idempotency key."</p></Show>
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| drafts.open.set(false)>"Cancel"</button><button class="button primary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| if signals.retry.get_untracked().is_some(){retry.run(())}else{submit.run(())}>{move || if signals.command_pending.get(){"Saving..."}else if signals.retry.get().is_some(){"Retry save"}else{"Save policy"}}</button></footer>
        </section></div>
    }.into_any()
}

fn requirement_select(signal: RwSignal<TraceabilityRequirement>) -> AnyView {
    view! {<select prop:value=move || requirement_wire(signal.get()) on:change=move |event| signal.set(parse_requirement(&event_target_value(&event)))><option value="not_tracked">"Not tracked"</option><option value="required">"Required"</option></select>}.into_any()
}

fn submit_configuration(store: CatalogStore, signals: Signals, drafts: Drafts) {
    let Some(inventory_owner_id) = drafts.owner_id.get_untracked() else {
        signals.error.set(Some("Select a client.".into()));
        return;
    };
    let Some(facility_id) = drafts.facility_id.get_untracked() else {
        signals.error.set(Some("Select a facility.".into()));
        return;
    };
    let Some(item_id) = drafts.item_id.get_untracked() else {
        signals.error.set(Some("Select an item.".into()));
        return;
    };
    let expiration = drafts.expiration.get_untracked();
    let shelf_life = drafts.shelf_life.get_untracked();
    let minimum_shelf_life_days =
        if expiration == TraceabilityRequirement::NotTracked || shelf_life.trim().is_empty() {
            None
        } else {
            match shelf_life.parse::<u32>() {
                Ok(days) if days <= 36_500 => Some(days),
                _ => {
                    signals.error.set(Some(
                        "Minimum shelf life must be a whole number from 0 to 36500.".into(),
                    ));
                    return;
                }
            }
        };
    let Some(item) = store
        .data
        .get_untracked()
        .items
        .into_iter()
        .find(|item| item.id == item_id)
    else {
        signals
            .error
            .set(Some("The selected item is no longer available.".into()));
        return;
    };
    dispatch_command(
        signals,
        drafts,
        PendingCommand::Configure {
            request: ConfigureItemTraceabilityPolicyRequest {
                inventory_owner_id,
                facility_id,
                item_id,
                uom: item.packaging_unit,
                lot: drafts.lot.get_untracked(),
                serial: drafts.serial.get_untracked(),
                expiration,
                minimum_shelf_life_days,
                expected_revision: drafts.expected_revision.get_untracked(),
            },
            key: api::new_idempotency_key(),
        },
    );
}

fn dispatch_command(signals: Signals, drafts: Drafts, command: PendingCommand) {
    signals.command_pending.set(true);
    signals.error.set(None);
    signals.retry.set(Some(command.clone()));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, drafts, command);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = match &command {
            PendingCommand::Configure { request, key } => {
                api::configure_item_traceability_policy(request, key).await
            }
            PendingCommand::Retire {
                policy_id,
                request,
                key,
            } => api::retire_item_traceability_policy(*policy_id, request, key).await,
        };
        signals.command_pending.set(false);
        match result {
            Ok(policy) => {
                let retired = matches!(command, PendingCommand::Retire { .. });
                signals.retry.set(None);
                signals.selected.set(Some(policy));
                drafts.open.set(false);
                drafts.confirm_retire.set(false);
                signals.toasts.success(if retired {
                    "Item traceability policy retired."
                } else {
                    "Item traceability policy saved."
                });
                load_first_page(signals);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None);
                    load_first_page(signals);
                }
                signals.error.set(Some(if error.ambiguous_outcome {
                    format!("{} Retry sends the exact saved command.", error.message)
                } else {
                    error.message
                }));
            }
        }
    });
}

fn load_first_page(signals: Signals) {
    signals.cursor.set(None);
    signals.history.set(Vec::new());
    load_page(signals, None);
}
fn reset_filtered_page(signals: Signals) {
    signals.selected.set(None);
    signals.error.set(None);
    load_first_page(signals);
}
fn next_page(signals: Signals) {
    if let Some(cursor) = signals.page.get_untracked().next_cursor {
        signals
            .history
            .update(|history| history.push(signals.cursor.get_untracked()));
        signals.cursor.set(Some(cursor.clone()));
        load_page(signals, Some(cursor));
    }
}
fn previous_page(signals: Signals) {
    let cursor = signals.history.get_untracked().last().cloned().flatten();
    signals.history.update(|history| {
        history.pop();
    });
    signals.cursor.set(cursor.clone());
    load_page(signals, cursor);
}
fn load_page(signals: Signals, cursor: Option<OpaqueCursor>) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, cursor, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::item_traceability_policies(
            api::ItemTraceabilityPolicyFilters {
                inventory_owner_id: signals.owner_id.get_untracked(),
                facility_id: signals.facility_id.get_untracked(),
                item_id: signals.item_id.get_untracked(),
                lot: None,
                serial: None,
                expiration: None,
                status: signals.status.get_untracked(),
            },
            cursor.as_ref(),
        )
        .await
        {
            Ok(page) if signals.generation.get_untracked() == generation => {
                if let Some(selected) = signals.selected.get_untracked() {
                    signals.selected.set(
                        page.items
                            .iter()
                            .find(|policy| {
                                policy.item_traceability_policy_id
                                    == selected.item_traceability_policy_id
                            })
                            .cloned(),
                    );
                }
                signals.page.set(page);
                signals.loading.set(false);
                signals.error.set(None);
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                signals.loading.set(false);
                signals.error.set(Some(error.message));
            }
        }
    });
}

const fn requirement_wire(value: TraceabilityRequirement) -> &'static str {
    match value {
        TraceabilityRequirement::NotTracked => "not_tracked",
        TraceabilityRequirement::Required => "required",
    }
}
const fn requirement_label(value: TraceabilityRequirement) -> &'static str {
    match value {
        TraceabilityRequirement::NotTracked => "Not tracked",
        TraceabilityRequirement::Required => "Required",
    }
}
fn parse_requirement(value: &str) -> TraceabilityRequirement {
    if value == "required" {
        TraceabilityRequirement::Required
    } else {
        TraceabilityRequirement::NotTracked
    }
}
const fn status_wire(value: ItemTraceabilityPolicyStatus) -> &'static str {
    match value {
        ItemTraceabilityPolicyStatus::Active => "active",
        ItemTraceabilityPolicyStatus::Retired => "retired",
    }
}
const fn status_label(value: ItemTraceabilityPolicyStatus) -> &'static str {
    match value {
        ItemTraceabilityPolicyStatus::Active => "Active",
        ItemTraceabilityPolicyStatus::Retired => "Retired",
    }
}
fn parse_status(value: &str) -> ItemTraceabilityPolicyStatus {
    if value == "retired" {
        ItemTraceabilityPolicyStatus::Retired
    } else {
        ItemTraceabilityPolicyStatus::Active
    }
}
fn option_id(value: Option<i64>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}
fn owner_options(values: &[InventoryOwner]) -> AnyView {
    values
        .iter()
        .filter(|value| value.deleted.is_none())
        .map(|value| view! {<option value=value.id>{value.name.clone()}</option>})
        .collect_view()
        .into_any()
}
fn facility_options(values: &[Facility]) -> AnyView {
    values.iter().filter(|value|value.deleted.is_none()).map(|value|view!{<option value=value.id>{label_or_id(value.name.as_deref(),"Facility",value.id)}</option>}).collect_view().into_any()
}
fn item_options(values: &[Item]) -> AnyView {
    values.iter().filter(|value|value.deleted.is_none()).map(|value|view!{<option value=value.id>{label_or_id(value.description.as_deref(),"Item",value.id)}</option>}).collect_view().into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_values_are_explicit() {
        assert_eq!(
            parse_requirement("required"),
            TraceabilityRequirement::Required
        );
        assert_eq!(
            requirement_label(TraceabilityRequirement::NotTracked),
            "Not tracked"
        );
    }
}
