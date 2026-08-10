use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ConfigureItemStoragePolicyRequest, ItemStoragePolicyPage, ItemStoragePolicyResponse,
    ItemStoragePolicyStatus, OpaqueCursor, RetireItemStoragePolicyRequest, Revision,
    StorageZonePurpose,
};
use wareboxes_core::models::{Facility, InventoryOwner, Item};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};
use crate::workspace_layout::{SplitPaneHandle, SplitPaneState};

use super::{label_or_id, CatalogStore};

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingCommand {
    Configure {
        request: ConfigureItemStoragePolicyRequest,
        key: String,
    },
    Retire {
        policy_id: i64,
        request: RetireItemStoragePolicyRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser callbacks consume workspace signals")
)]
struct Signals {
    page: RwSignal<ItemStoragePolicyPage>,
    owner_id: RwSignal<Option<i64>>,
    facility_id: RwSignal<Option<i64>>,
    item_id: RwSignal<Option<i64>>,
    purpose: RwSignal<Option<StorageZonePurpose>>,
    status: RwSignal<ItemStoragePolicyStatus>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
    generation: RwSignal<u64>,
    loading: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    retry: RwSignal<Option<PendingCommand>>,
    selected: RwSignal<Option<ItemStoragePolicyResponse>>,
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
    purposes: RwSignal<Vec<StorageZonePurpose>>,
    capacity: RwSignal<String>,
    expected_revision: RwSignal<Option<Revision>>,
    confirm_retire: RwSignal<bool>,
}

#[component]
pub(super) fn ItemStoragePolicyCatalog(
    store: CatalogStore,
    can_supervise: bool,
    layout: SplitPaneState,
) -> impl IntoView {
    let signals = Signals {
        page: RwSignal::new(ItemStoragePolicyPage::new(Vec::new(), None)),
        owner_id: RwSignal::new(None),
        facility_id: RwSignal::new(None),
        item_id: RwSignal::new(None),
        purpose: RwSignal::new(None),
        status: RwSignal::new(ItemStoragePolicyStatus::Active),
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
        purposes: RwSignal::new(vec![StorageZonePurpose::Reserve]),
        capacity: RwSignal::new(String::new()),
        expected_revision: RwSignal::new(None),
        confirm_retire: RwSignal::new(false),
    };
    load_first_page(signals);

    let refresh = Callback::new(move |_| load_first_page(signals));
    let select = Callback::new(move |policy: ItemStoragePolicyResponse| {
        drafts.confirm_retire.set(false);
        signals.error.set(None);
        signals.selected.set(Some(policy));
        layout.show_detail();
    });
    let open_create = Callback::new(move |_| {
        drafts.owner_id.set(signals.owner_id.get_untracked());
        drafts.facility_id.set(signals.facility_id.get_untracked());
        drafts.item_id.set(signals.item_id.get_untracked());
        drafts.purposes.set(vec![StorageZonePurpose::Reserve]);
        drafts.capacity.set(String::new());
        drafts.expected_revision.set(None);
        signals.retry.set(None);
        signals.error.set(None);
        drafts.open.set(true);
    });
    let open_edit = Callback::new(move |policy: ItemStoragePolicyResponse| {
        drafts.owner_id.set(Some(policy.inventory_owner_id));
        drafts.facility_id.set(Some(policy.facility_id));
        drafts.item_id.set(Some(policy.item_id));
        drafts.purposes.set(policy.allowed_zone_purposes);
        drafts.capacity.set(
            policy
                .max_quantity_per_location
                .map_or_else(String::new, |value| value.to_string()),
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
    let retire = Callback::new(move |policy: ItemStoragePolicyResponse| {
        dispatch_command(
            signals,
            drafts,
            PendingCommand::Retire {
                policy_id: policy.item_storage_policy_id,
                request: RetireItemStoragePolicyRequest {
                    expected_revision: policy.revision,
                },
                key: api::new_idempotency_key(),
            },
        );
    });

    view! {
        <div class="catalog-layout item-storage-policy-layout split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <section class="data-section catalog-browser split-master">
                <div class="catalog-toolbar item-storage-policy-toolbar">
                    <label><span class="sr-only">"Client"</span><select prop:value=move || option_id(signals.owner_id.get()) on:change=move |event| { signals.owner_id.set(parse_id(&event_target_value(&event))); reset_filtered_page(signals); }><option value="">"All clients"</option>{move || owner_options(&store.data.get().clients)}</select></label>
                    <label><span class="sr-only">"Facility"</span><select prop:value=move || option_id(signals.facility_id.get()) on:change=move |event| { signals.facility_id.set(parse_id(&event_target_value(&event))); reset_filtered_page(signals); }><option value="">"All facilities"</option>{move || facility_options(&store.data.get().facilities)}</select></label>
                    <label><span class="sr-only">"Item"</span><select prop:value=move || option_id(signals.item_id.get()) on:change=move |event| { signals.item_id.set(parse_id(&event_target_value(&event))); reset_filtered_page(signals); }><option value="">"All items"</option>{move || item_options(&store.data.get().items)}</select></label>
                    <label><span class="sr-only">"Purpose"</span><select prop:value=move || signals.purpose.get().map_or("all",purpose_wire) on:change=move |event| { signals.purpose.set(parse_purpose(&event_target_value(&event))); reset_filtered_page(signals); }><option value="all">"All purposes"</option>{purpose_options()}</select></label>
                    <label><span class="sr-only">"Status"</span><select prop:value=move || status_wire(signals.status.get()) on:change=move |event| { signals.status.set(parse_status(&event_target_value(&event))); reset_filtered_page(signals); }><option value="active">"Active"</option><option value="retired">"Retired history"</option></select></label>
                    <button class="icon-button" type="button" title="Refresh" aria-label="Refresh item storage policies" disabled=move || signals.loading.get() on:click=move |_| refresh.run(())><Icon icon=UiIcon::Refresh/></button>
                    {can_supervise.then(|| view! { <button class="button primary-action compact" type="button" on:click=move |_| open_create.run(())>"New policy"</button> })}
                </div>
                <div class="table-scroll catalog-table-scroll">
                    <table class="data-table catalog-table item-storage-policy-table">
                        <caption class="sr-only">"Item storage policies"</caption>
                        <thead><tr><th>"Item"</th><th>"Client"</th><th>"Facility"</th><th>"UOM"</th><th>"Allowed zones"</th><th class="numeric">"Per location"</th><th>"Status"</th></tr></thead>
                        <tbody>{move || policy_rows(signals, select)}</tbody>
                    </table>
                </div>
                <footer class="table-footer"><span>{move || if signals.loading.get(){"Refreshing...".into()}else{format!("{} on this page",signals.page.get().items.len())}}</span><button class="button secondary-action compact" type="button" disabled=move || signals.loading.get() || signals.history.get().is_empty() on:click=move |_| previous_page(signals)>"Previous"</button><button class="button secondary-action compact" type="button" disabled=move || signals.loading.get() || signals.page.get().next_cursor.is_none() on:click=move |_| next_page(signals)>"Next"</button></footer>
            </section>
            <SplitPaneHandle layout/>
            <aside class="data-section catalog-editor split-detail" aria-label="Item storage policy details">
                {move || signals.selected.get().map(|policy| policy_detail(policy,can_supervise,signals,drafts,open_edit,retire,retry)).unwrap_or_else(|| view!{<div class="catalog-editor-empty"><strong>"Select a storage policy"</strong><p>"Review allowed zone purposes and per-location capacity for an item."</p></div>}.into_any())}
            </aside>
        </div>
        <Show when=move || drafts.open.get()>{move || configuration_dialog(store,signals,drafts,submit,retry)}</Show>
    }
}

fn policy_rows(signals: Signals, select: Callback<ItemStoragePolicyResponse>) -> AnyView {
    let items = signals.page.get().items;
    if items.is_empty() {
        return view! {<tr><td class="table-empty-row" colspan="7">{if signals.loading.get(){"Loading policies..."}else{"No policies match this view."}}</td></tr>}.into_any();
    }
    items
        .into_iter()
        .map(|policy| {
            let selected = signals.selected.get().as_ref().is_some_and(|value| {
                value.item_storage_policy_id == policy.item_storage_policy_id
            });
            let row = policy.clone();
            view! {
                <tr class:selected=selected>
                    <td><button class="catalog-row-link" type="button" on:click=move |_| select.run(row.clone())>{policy.item_description}</button></td>
                    <td>{policy.inventory_owner_name}</td><td>{policy.facility_name}</td><td>{policy.uom}</td>
                    <td><span class="policy-purpose-summary">{purpose_summary(&policy.allowed_zone_purposes)}</span></td>
                    <td class="numeric">{policy.max_quantity_per_location.map_or_else(||"No limit".into(),|value|value.to_string())}</td>
                    <td><span class=if policy.status==ItemStoragePolicyStatus::Active{"status-chip success"}else{"status-chip neutral"}>{status_label(policy.status)}</span></td>
                </tr>
            }
        })
        .collect_view()
        .into_any()
}

fn policy_detail(
    policy: ItemStoragePolicyResponse,
    can_supervise: bool,
    signals: Signals,
    drafts: Drafts,
    open_edit: Callback<ItemStoragePolicyResponse>,
    retire: Callback<ItemStoragePolicyResponse>,
    retry: Callback<()>,
) -> AnyView {
    let editable = can_supervise && policy.status == ItemStoragePolicyStatus::Active;
    let edit_policy = policy.clone();
    let retire_policy = StoredValue::new(policy.clone());
    view! {
        <div class="catalog-editor-form item-storage-policy-detail">
            <header class="catalog-editor-heading"><div><p class="eyebrow">{policy.inventory_owner_name.clone()}</p><h2>{policy.item_description.clone()}</h2><small>{policy.facility_name.clone()}</small></div><span class=if policy.status==ItemStoragePolicyStatus::Active{"status-chip success"}else{"status-chip neutral"}>{status_label(policy.status)}</span></header>
            <dl class="catalog-summary-grid"><div><dt>"UOM"</dt><dd>{policy.uom.clone()}</dd></div><div><dt>"Revision"</dt><dd>{policy.revision.get()}</dd></div><div><dt>"Per-location cap"</dt><dd>{policy.max_quantity_per_location.map_or_else(||"No limit".into(),|value|format!("{value} {}",policy.uom))}</dd></div><div><dt>"Allowed purposes"</dt><dd>{policy.allowed_zone_purposes.len()}</dd></div></dl>
            <section class="policy-purpose-list"><h3>"Allowed storage zones"</h3><div>{policy.allowed_zone_purposes.iter().copied().map(|purpose|view!{<span class="catalog-badge">{purpose_label(purpose)}</span>}).collect_view()}</div></section>
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
        <div class="modal-backdrop" role="presentation"><section class="modal-panel item-storage-policy-dialog" role="dialog" aria-modal="true" aria-labelledby="item-storage-policy-dialog-title">
            <header><div><p class="eyebrow">"Putaway governance"</p><h2 id="item-storage-policy-dialog-title">{if editing{"Reconfigure item storage policy"}else{"New item storage policy"}}</h2></div><button class="icon-button" type="button" aria-label="Close item storage policy dialog" disabled=move || signals.command_pending.get() on:click=move |_| drafts.open.set(false)><Icon icon=UiIcon::Close/></button></header>
            <fieldset disabled=move || signals.command_pending.get()><div class="item-storage-policy-form-grid"><label><span>"Client"</span><select required disabled=editing prop:value=move || option_id(drafts.owner_id.get()) on:change=move |event| drafts.owner_id.set(parse_id(&event_target_value(&event)))><option value="">"Select client"</option>{move || owner_options(&store.data.get().clients)}</select></label><label><span>"Facility"</span><select required disabled=editing prop:value=move || option_id(drafts.facility_id.get()) on:change=move |event| drafts.facility_id.set(parse_id(&event_target_value(&event)))><option value="">"Select facility"</option>{move || facility_options(&store.data.get().facilities)}</select></label><label><span>"Item"</span><select required disabled=editing prop:value=move || option_id(drafts.item_id.get()) on:change=move |event| drafts.item_id.set(parse_id(&event_target_value(&event)))><option value="">"Select item"</option>{move || item_options(&store.data.get().items)}</select></label><label><span>"Maximum per location"</span><input type="number" min="1" placeholder="No limit" prop:value=move || drafts.capacity.get() on:input=move |event| drafts.capacity.set(event_target_value(&event))/></label></div>
            <section class="policy-purpose-picker"><div><h3>"Allowed storage-zone purposes"</h3><span>{move || format!("{} selected",drafts.purposes.get().len())}</span></div><div>{all_purposes().into_iter().map(|purpose|{let checked=drafts.purposes.get().contains(&purpose);view!{<label><input type="checkbox" prop:checked=checked on:change=move |event| toggle_purpose(drafts.purposes,purpose,event_target_checked(&event))/><span>{purpose_label(purpose)}</span></label>}}).collect_view()}</div></section></fieldset>
            <Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show><Show when=move || signals.retry.get().is_some()><p class="catalog-command-note">"Retry sends the exact saved policy and idempotency key."</p></Show>
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| drafts.open.set(false)>"Cancel"</button><button class="button primary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| if signals.retry.get_untracked().is_some(){retry.run(())}else{submit.run(())}>{move || if signals.command_pending.get(){"Saving..."}else if signals.retry.get().is_some(){"Retry save"}else{"Save policy"}}</button></footer>
        </section></div>
    }.into_any()
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
    let purposes = drafts.purposes.get_untracked();
    if purposes.is_empty() {
        signals
            .error
            .set(Some("Select at least one storage-zone purpose.".into()));
        return;
    }
    let capacity = drafts.capacity.get_untracked();
    let max_quantity_per_location = if capacity.trim().is_empty() {
        None
    } else {
        match capacity.parse::<i64>() {
            Ok(value) if value > 0 => Some(value),
            _ => {
                signals.error.set(Some(
                    "Maximum per location must be a positive whole number.".into(),
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
            request: ConfigureItemStoragePolicyRequest {
                inventory_owner_id,
                facility_id,
                item_id,
                uom: item.packaging_unit,
                allowed_zone_purposes: purposes,
                max_quantity_per_location,
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
                api::configure_item_storage_policy(request, key).await
            }
            PendingCommand::Retire {
                policy_id,
                request,
                key,
            } => api::retire_item_storage_policy(*policy_id, request, key).await,
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
                    "Item storage policy retired."
                } else {
                    "Item storage policy saved."
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
        match api::item_storage_policies(
            signals.owner_id.get_untracked(),
            signals.facility_id.get_untracked(),
            signals.item_id.get_untracked(),
            signals.purpose.get_untracked(),
            signals.status.get_untracked(),
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
                                policy.item_storage_policy_id == selected.item_storage_policy_id
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

fn toggle_purpose(
    signal: RwSignal<Vec<StorageZonePurpose>>,
    purpose: StorageZonePurpose,
    checked: bool,
) {
    signal.update(|values| {
        values.retain(|value| *value != purpose);
        if checked {
            values.push(purpose);
            values.sort_unstable();
        }
    });
}
fn purpose_summary(values: &[StorageZonePurpose]) -> String {
    values
        .iter()
        .copied()
        .map(purpose_label)
        .collect::<Vec<_>>()
        .join(", ")
}
fn all_purposes() -> [StorageZonePurpose; 8] {
    [
        StorageZonePurpose::Receiving,
        StorageZonePurpose::Reserve,
        StorageZonePurpose::Pick,
        StorageZonePurpose::Staging,
        StorageZonePurpose::Packing,
        StorageZonePurpose::Shipping,
        StorageZonePurpose::Quarantine,
        StorageZonePurpose::Damage,
    ]
}
fn purpose_options() -> AnyView {
    all_purposes()
        .into_iter()
        .map(
            |purpose| view! {<option value=purpose_wire(purpose)>{purpose_label(purpose)}</option>},
        )
        .collect_view()
        .into_any()
}
const fn purpose_wire(value: StorageZonePurpose) -> &'static str {
    match value {
        StorageZonePurpose::Receiving => "receiving",
        StorageZonePurpose::Reserve => "reserve",
        StorageZonePurpose::Pick => "pick",
        StorageZonePurpose::Staging => "staging",
        StorageZonePurpose::Packing => "packing",
        StorageZonePurpose::Shipping => "shipping",
        StorageZonePurpose::Quarantine => "quarantine",
        StorageZonePurpose::Damage => "damage",
    }
}
const fn purpose_label(value: StorageZonePurpose) -> &'static str {
    match value {
        StorageZonePurpose::Receiving => "Receiving",
        StorageZonePurpose::Reserve => "Reserve",
        StorageZonePurpose::Pick => "Pick",
        StorageZonePurpose::Staging => "Staging",
        StorageZonePurpose::Packing => "Packing",
        StorageZonePurpose::Shipping => "Shipping",
        StorageZonePurpose::Quarantine => "Quarantine",
        StorageZonePurpose::Damage => "Damage",
    }
}
fn parse_purpose(value: &str) -> Option<StorageZonePurpose> {
    all_purposes()
        .into_iter()
        .find(|purpose| purpose_wire(*purpose) == value)
}
const fn status_wire(value: ItemStoragePolicyStatus) -> &'static str {
    match value {
        ItemStoragePolicyStatus::Active => "active",
        ItemStoragePolicyStatus::Retired => "retired",
    }
}
const fn status_label(value: ItemStoragePolicyStatus) -> &'static str {
    match value {
        ItemStoragePolicyStatus::Active => "Active",
        ItemStoragePolicyStatus::Retired => "Retired",
    }
}
fn parse_status(value: &str) -> ItemStoragePolicyStatus {
    if value == "retired" {
        ItemStoragePolicyStatus::Retired
    } else {
        ItemStoragePolicyStatus::Active
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
    fn purposes_are_canonical_and_readable() {
        let signal = RwSignal::new(vec![StorageZonePurpose::Pick]);
        toggle_purpose(signal, StorageZonePurpose::Reserve, true);
        toggle_purpose(signal, StorageZonePurpose::Pick, false);
        assert_eq!(signal.get_untracked(), vec![StorageZonePurpose::Reserve]);
        assert_eq!(
            purpose_summary(&[StorageZonePurpose::Reserve, StorageZonePurpose::Pick]),
            "Reserve, Pick"
        );
    }
}
