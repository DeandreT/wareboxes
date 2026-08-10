use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ConfigureStorageZoneRequest, OpaqueCursor, RetireStorageZoneRequest, Revision, StorageZonePage,
    StorageZonePurpose, StorageZoneResponse, StorageZoneStatus,
};
use wareboxes_core::models::{Facility, Location};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

use super::{label_or_id, CatalogStore};

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingCommand {
    Configure {
        request: ConfigureStorageZoneRequest,
        key: String,
    },
    Retire {
        storage_zone_id: i64,
        request: RetireStorageZoneRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "browser callbacks consume authorization and toast signals"
    )
)]
struct Signals {
    page: RwSignal<StorageZonePage>,
    facility_id: RwSignal<Option<i64>>,
    purpose: RwSignal<Option<StorageZonePurpose>>,
    status: RwSignal<StorageZoneStatus>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
    generation: RwSignal<u64>,
    loading: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    retry: RwSignal<Option<PendingCommand>>,
    selected: RwSignal<Option<StorageZoneResponse>>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[derive(Clone, Copy)]
struct Drafts {
    open: RwSignal<bool>,
    facility_id: RwSignal<Option<i64>>,
    code: RwSignal<String>,
    name: RwSignal<String>,
    purpose: RwSignal<StorageZonePurpose>,
    travel_sequence: RwSignal<u32>,
    location_ids: RwSignal<Vec<i64>>,
    expected_revision: RwSignal<Option<Revision>>,
    confirm_retire: RwSignal<bool>,
}

#[component]
pub(super) fn StorageZoneCatalog(store: CatalogStore, can_supervise: bool) -> impl IntoView {
    let signals = Signals {
        page: RwSignal::new(StorageZonePage::new(Vec::new(), None)),
        facility_id: RwSignal::new(None),
        purpose: RwSignal::new(None),
        status: RwSignal::new(StorageZoneStatus::Active),
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
        facility_id: RwSignal::new(None),
        code: RwSignal::new(String::new()),
        name: RwSignal::new(String::new()),
        purpose: RwSignal::new(StorageZonePurpose::Reserve),
        travel_sequence: RwSignal::new(0),
        location_ids: RwSignal::new(Vec::new()),
        expected_revision: RwSignal::new(None),
        confirm_retire: RwSignal::new(false),
    };
    load_first_page(signals);

    let refresh = Callback::new(move |_| load_first_page(signals));
    let select = Callback::new(move |zone: StorageZoneResponse| {
        drafts.confirm_retire.set(false);
        signals.error.set(None);
        signals.selected.set(Some(zone));
    });
    let open_create = Callback::new(move |_| {
        drafts.facility_id.set(signals.facility_id.get_untracked());
        drafts.code.set(String::new());
        drafts.name.set(String::new());
        drafts.purpose.set(StorageZonePurpose::Reserve);
        drafts.travel_sequence.set(0);
        drafts.location_ids.set(Vec::new());
        drafts.expected_revision.set(None);
        signals.retry.set(None);
        signals.error.set(None);
        drafts.open.set(true);
    });
    let open_edit = Callback::new(move |zone: StorageZoneResponse| {
        drafts.facility_id.set(Some(zone.facility_id));
        drafts.code.set(zone.code);
        drafts.name.set(zone.name);
        drafts.purpose.set(zone.purpose);
        drafts.travel_sequence.set(zone.travel_sequence);
        drafts.location_ids.set(
            zone.locations
                .iter()
                .map(|location| location.location_id)
                .collect(),
        );
        drafts.expected_revision.set(Some(zone.revision));
        signals.retry.set(None);
        signals.error.set(None);
        drafts.open.set(true);
    });
    let submit = Callback::new(move |_| submit_configuration(signals, drafts, store));
    let retry = Callback::new(move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch_command(signals, drafts, store, command);
        }
    });
    let retire = Callback::new(move |zone: StorageZoneResponse| {
        let command = PendingCommand::Retire {
            storage_zone_id: zone.storage_zone_id,
            request: RetireStorageZoneRequest {
                expected_revision: zone.revision,
            },
            key: api::new_idempotency_key(),
        };
        dispatch_command(signals, drafts, store, command);
    });

    view! {
        <div class="catalog-layout storage-zone-layout">
            <section class="data-section catalog-browser">
                <div class="catalog-toolbar storage-zone-toolbar">
                    <label><span class="sr-only">"Facility"</span><select prop:value=move || option_id(signals.facility_id.get()) on:change=move |event| { signals.facility_id.set(parse_id(&event_target_value(&event))); reset_filtered_page(signals); }><option value="">"All facilities"</option>{move || facility_options(&store.data.get().facilities)}</select></label>
                    <label><span class="sr-only">"Purpose"</span><select prop:value=move || signals.purpose.get().map_or("all",purpose_wire) on:change=move |event| { signals.purpose.set(parse_purpose(&event_target_value(&event))); reset_filtered_page(signals); }><option value="all">"All purposes"</option>{purpose_options()}</select></label>
                    <label><span class="sr-only">"Status"</span><select prop:value=move || status_wire(signals.status.get()) on:change=move |event| { signals.status.set(if event_target_value(&event)=="retired" { StorageZoneStatus::Retired } else { StorageZoneStatus::Active }); reset_filtered_page(signals); }><option value="active">"Active"</option><option value="retired">"Retired history"</option></select></label>
                    <span class="catalog-count">{move || format!("{} zones",signals.page.get().items.len())}</span>
                    <button class="button secondary-action compact" type="button" disabled=move || signals.loading.get() on:click=move |_| refresh.run(())>"Refresh"</button>
                    {can_supervise.then(|| view! { <button class="button primary-action compact" type="button" on:click=move |_| open_create.run(())>"New zone"</button> })}
                </div>
                <div class="table-scroll catalog-table-scroll">
                    <table class="data-table catalog-table storage-zone-table">
                        <caption class="sr-only">"Facility storage zones ordered by travel sequence"</caption>
                        <thead><tr><th class="numeric">"Seq"</th><th>"Code"</th><th>"Purpose"</th><th>"Name"</th><th>"Facility"</th><th class="numeric">"Locations"</th><th>"Status"</th></tr></thead>
                        <tbody>{move || { let items=signals.page.get().items; if items.is_empty(){ view!{<tr><td class="table-empty-row" colspan="7">{if signals.loading.get(){"Loading zones..."}else{"No zones match this view."}}</td></tr>}.into_any() } else { items.into_iter().map(|zone| { let selected=signals.selected.get().as_ref().is_some_and(|value|value.storage_zone_id==zone.storage_zone_id); let row=zone.clone(); view!{<tr class:selected=selected><td class="numeric strong">{zone.travel_sequence}</td><td><button class="catalog-row-link" type="button" on:click=move |_| select.run(row.clone())>{zone.code}</button></td><td><span class="catalog-badge">{purpose_label(zone.purpose)}</span></td><td>{zone.name}</td><td>{zone.facility_name}</td><td class="numeric">{zone.locations.len()}</td><td><span class=if zone.status==StorageZoneStatus::Active{"status-chip success"}else{"status-chip neutral"}>{status_label(zone.status)}</span></td></tr>} }).collect_view().into_any() } }}</tbody>
                    </table>
                </div>
                <footer class="table-footer"><span>{move || if signals.loading.get(){"Refreshing...".into()}else{format!("{} on this page",signals.page.get().items.len())}}</span><button class="button secondary-action compact" type="button" disabled=move || signals.loading.get() || signals.history.get().is_empty() on:click=move |_| previous_page(signals)>"Previous"</button><button class="button secondary-action compact" type="button" disabled=move || signals.loading.get() || signals.page.get().next_cursor.is_none() on:click=move |_| next_page(signals)>"Next"</button></footer>
            </section>
            <aside class="data-section catalog-editor" aria-label="Storage zone details">
                {move || signals.selected.get().map(|zone| zone_detail(zone,can_supervise,signals,drafts,open_edit,retire,retry)).unwrap_or_else(|| view!{<div class="catalog-editor-empty"><strong>"Select a storage zone"</strong><p>"Review purpose, travel order, and exact member locations."</p></div>}.into_any())}
            </aside>
        </div>
        <Show when=move || drafts.open.get()>{move || configuration_dialog(store,signals,drafts,submit,retry)}</Show>
    }
}

fn zone_detail(
    zone: StorageZoneResponse,
    can_supervise: bool,
    signals: Signals,
    drafts: Drafts,
    open_edit: Callback<StorageZoneResponse>,
    retire: Callback<StorageZoneResponse>,
    retry: Callback<()>,
) -> AnyView {
    let editable = can_supervise && zone.status == StorageZoneStatus::Active;
    let edit_zone = zone.clone();
    let retire_zone = StoredValue::new(zone.clone());
    view! { <div class="catalog-editor-form storage-zone-detail"><header class="catalog-editor-heading"><div><p class="eyebrow">{zone.facility_name.clone()}</p><h2>{format!("{} / {}",zone.code,zone.name)}</h2></div><span class=if zone.status==StorageZoneStatus::Active{"status-chip success"}else{"status-chip neutral"}>{status_label(zone.status)}</span></header><dl class="catalog-summary-grid"><div><dt>"Purpose"</dt><dd>{purpose_label(zone.purpose)}</dd></div><div><dt>"Travel sequence"</dt><dd>{zone.travel_sequence}</dd></div><div><dt>"Revision"</dt><dd>{zone.revision.get()}</dd></div><div><dt>"Locations"</dt><dd>{zone.locations.len()}</dd></div></dl><section class="storage-zone-members"><h3>"Member locations"</h3><table class="data-table"><thead><tr><th>"Scan code"</th><th>"Name"</th><th>"Type"</th><th>"Capabilities"</th></tr></thead><tbody>{zone.locations.into_iter().map(|location| view!{<tr><td><code>{location.barcode}</code></td><td>{location.name.unwrap_or_else(||"-".into())}</td><td>{location.location_type}</td><td>{location_capabilities(location.pickable,location.receivable)}</td></tr>}).collect_view()}</tbody></table></section><Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show><Show when=move || signals.retry.get().is_some()><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| retry.run(())>"Retry exact command"</button></Show>{editable.then(|| view!{<footer class="catalog-editor-actions"><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| open_edit.run(edit_zone.clone())>"Reconfigure"</button><Show when=move || !drafts.confirm_retire.get()><button class="button danger-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| drafts.confirm_retire.set(true)>"Retire zone"</button></Show><Show when=move || drafts.confirm_retire.get()><span class="destructive-confirm"><span>"Retire this zone?"</span><button class="button secondary-action compact" type="button" on:click=move |_| drafts.confirm_retire.set(false)>"Keep"</button><button class="button danger-action compact" type="button" disabled=move || signals.command_pending.get() on:click=move |_| retire.run(retire_zone.get_value())>"Confirm"</button></span></Show></footer>})}</div> }.into_any()
}

fn configuration_dialog(
    store: CatalogStore,
    signals: Signals,
    drafts: Drafts,
    submit: Callback<()>,
    retry: Callback<()>,
) -> AnyView {
    let editing = drafts.expected_revision.get().is_some();
    view! { <div class="modal-backdrop" role="presentation"><section class="modal-panel storage-zone-dialog" role="dialog" aria-modal="true" aria-labelledby="storage-zone-dialog-title"><header><div><p class="eyebrow">"Facility topology"</p><h2 id="storage-zone-dialog-title">{if editing{"Reconfigure storage zone"}else{"New storage zone"}}</h2></div><button class="icon-button" type="button" aria-label="Close storage zone dialog" disabled=move || signals.command_pending.get() on:click=move |_| drafts.open.set(false)><Icon icon=UiIcon::Close/></button></header><fieldset disabled=move || signals.command_pending.get()><div class="storage-zone-form-grid"><label><span>"Facility"</span><select required disabled=editing prop:value=move || option_id(drafts.facility_id.get()) on:change=move |event| { drafts.facility_id.set(parse_id(&event_target_value(&event))); drafts.location_ids.set(Vec::new()); }><option value="">"Select facility"</option>{facility_options(&store.data.get().facilities)}</select></label><label><span>"Code"</span><input required maxlength="32" disabled=editing prop:value=move || drafts.code.get() on:input=move |event| drafts.code.set(event_target_value(&event)) /></label><label><span>"Name"</span><input required maxlength="120" prop:value=move || drafts.name.get() on:input=move |event| drafts.name.set(event_target_value(&event)) /></label><label><span>"Purpose"</span><select prop:value=move || purpose_wire(drafts.purpose.get()) on:change=move |event| { if let Some(value)=parse_purpose(&event_target_value(&event)){drafts.purpose.set(value);drafts.location_ids.set(Vec::new());} }>{purpose_options()}</select></label><label><span>"Travel sequence"</span><input type="number" min="0" max="4294967295" prop:value=move || drafts.travel_sequence.get() on:input=move |event| { if let Ok(value)=event_target_value(&event).parse(){drafts.travel_sequence.set(value);} } /></label></div><section class="storage-zone-location-picker"><div><h3>"Locations"</h3><span>{move || format!("{} selected",drafts.location_ids.get().len())}</span></div><div>{move || { let current_zone_id=if drafts.expected_revision.get().is_some(){signals.selected.get().as_ref().map(|zone|zone.storage_zone_id)}else{None}; eligible_locations(&store.data.get().locations,drafts.facility_id.get(),drafts.purpose.get(),current_zone_id).into_iter().map(|location| { let id=location.id; let checked=drafts.location_ids.get().contains(&id); view!{<label class="storage-zone-location-option"><input type="checkbox" prop:checked=checked on:change=move |event| toggle_location(drafts.location_ids,id,event_target_checked(&event)) /><span><strong>{label_or_id(location.name.as_deref().or(location.barcode.as_deref()),"Location",id)}</strong><small>{format!("{} / {}",location.barcode.unwrap_or_else(||"No scan".into()),location.r#type)}</small></span></label>} }).collect_view() }}</div></section></fieldset><Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show><Show when=move || signals.retry.get().is_some()><p class="catalog-command-note">"Retry sends the exact saved configuration and idempotency key."</p></Show><footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| drafts.open.set(false)>"Cancel"</button><button class="button primary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| if signals.retry.get_untracked().is_some(){retry.run(())}else{submit.run(())}>{move || if signals.command_pending.get(){"Saving..."}else if signals.retry.get().is_some(){"Retry save"}else{"Save zone"}}</button></footer></section></div> }.into_any()
}

fn submit_configuration(signals: Signals, drafts: Drafts, store: CatalogStore) {
    let Some(facility_id) = drafts.facility_id.get_untracked() else {
        signals.error.set(Some("Select a facility.".into()));
        return;
    };
    let code = drafts.code.get_untracked().trim().to_owned();
    let name = drafts.name.get_untracked().trim().to_owned();
    let location_ids = drafts.location_ids.get_untracked();
    if code.is_empty() || name.is_empty() || location_ids.is_empty() {
        signals.error.set(Some(
            "Code, name, and at least one compatible location are required.".into(),
        ));
        return;
    }
    let command = PendingCommand::Configure {
        request: ConfigureStorageZoneRequest {
            facility_id,
            code,
            name,
            purpose: drafts.purpose.get_untracked(),
            travel_sequence: drafts.travel_sequence.get_untracked(),
            location_ids,
            expected_revision: drafts.expected_revision.get_untracked(),
        },
        key: api::new_idempotency_key(),
    };
    dispatch_command(signals, drafts, store, command);
}

fn dispatch_command(
    signals: Signals,
    drafts: Drafts,
    store: CatalogStore,
    command: PendingCommand,
) {
    signals.command_pending.set(true);
    signals.error.set(None);
    signals.retry.set(Some(command.clone()));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, drafts, store, command);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = match &command {
            PendingCommand::Configure { request, key } => {
                api::configure_storage_zone(request, key).await
            }
            PendingCommand::Retire {
                storage_zone_id,
                request,
                key,
            } => api::retire_storage_zone(*storage_zone_id, request, key).await,
        };
        signals.command_pending.set(false);
        match result {
            Ok(zone) => {
                let retired = matches!(command, PendingCommand::Retire { .. });
                signals.retry.set(None);
                signals.selected.set(Some(zone));
                drafts.open.set(false);
                drafts.confirm_retire.set(false);
                signals.toasts.success(if retired {
                    "Storage zone retired."
                } else {
                    "Storage zone saved."
                });
                store.refresh();
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
    if let Some(cursor) = cursor {
        signals.cursor.set(Some(cursor.clone()));
        load_page(signals, Some(cursor));
    } else {
        signals.cursor.set(None);
        load_page(signals, None);
    }
}
fn load_page(signals: Signals, cursor: Option<OpaqueCursor>) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.loading.set(true);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, cursor, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::storage_zones(
            signals.facility_id.get_untracked(),
            signals.purpose.get_untracked(),
            Some(signals.status.get_untracked()),
            cursor.as_ref(),
        )
        .await
        {
            Ok(page) if signals.generation.get_untracked() == generation => {
                if let Some(selected) = signals.selected.get_untracked() {
                    signals.selected.set(
                        page.items
                            .iter()
                            .find(|zone| zone.storage_zone_id == selected.storage_zone_id)
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

fn eligible_locations(
    values: &[Location],
    facility_id: Option<i64>,
    purpose: StorageZonePurpose,
    current_zone_id: Option<i64>,
) -> Vec<Location> {
    let mut result = values
        .iter()
        .filter(|location| {
            Some(location.facility_id) == facility_id
                && location.deleted.is_none()
                && location.active
                && location
                    .barcode
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty())
                && (location.storage_zone_id.is_none()
                    || location.storage_zone_id == current_zone_id)
                && location_matches_purpose(location, purpose)
        })
        .cloned()
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.barcode.cmp(&right.barcode))
            .then_with(|| left.id.cmp(&right.id))
    });
    result
}
fn location_matches_purpose(location: &Location, purpose: StorageZonePurpose) -> bool {
    match purpose {
        StorageZonePurpose::Receiving => location.receivable && !location.pickable,
        StorageZonePurpose::Pick => location.pickable && !location.receivable,
        _ => !location.pickable && !location.receivable,
    }
}
fn toggle_location(signal: RwSignal<Vec<i64>>, id: i64, checked: bool) {
    signal.update(|values| {
        values.retain(|value| *value != id);
        if checked {
            values.push(id);
            values.sort_unstable();
        }
    });
}
fn facility_options(values: &[Facility]) -> AnyView {
    values
        .iter()
        .filter(|value| value.deleted.is_none())
        .map(|value| view! {<option value=value.id>{value.name.clone()}</option>})
        .collect_view()
        .into_any()
}
fn purpose_options() -> AnyView {
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
    .into_iter()
    .map(|purpose| view! {<option value=purpose_wire(purpose)>{purpose_label(purpose)}</option>})
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
    match value {
        "receiving" => Some(StorageZonePurpose::Receiving),
        "reserve" => Some(StorageZonePurpose::Reserve),
        "pick" => Some(StorageZonePurpose::Pick),
        "staging" => Some(StorageZonePurpose::Staging),
        "packing" => Some(StorageZonePurpose::Packing),
        "shipping" => Some(StorageZonePurpose::Shipping),
        "quarantine" => Some(StorageZonePurpose::Quarantine),
        "damage" => Some(StorageZonePurpose::Damage),
        _ => None,
    }
}
const fn status_wire(value: StorageZoneStatus) -> &'static str {
    match value {
        StorageZoneStatus::Active => "active",
        StorageZoneStatus::Retired => "retired",
    }
}
const fn status_label(value: StorageZoneStatus) -> &'static str {
    match value {
        StorageZoneStatus::Active => "Active",
        StorageZoneStatus::Retired => "Retired",
    }
}
fn location_capabilities(pickable: bool, receivable: bool) -> &'static str {
    match (pickable, receivable) {
        (true, true) => "Pick / Receive",
        (true, false) => "Pick",
        (false, true) => "Receive",
        (false, false) => "Storage",
    }
}
fn option_id(value: Option<i64>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatibility_is_explicit() {
        let location: Location = serde_json::from_value(serde_json::json!({
            "id": 1,
            "tenant_id": 1,
            "created": "2026-08-10T12:00:00Z",
            "deleted": null,
            "facility_id": 1,
            "facility_name": null,
            "parent_location_id": null,
            "barcode": "A",
            "name": null,
            "type": "bin",
            "active": true,
            "pickable": true,
            "receivable": false
        }))
        .unwrap();
        assert!(location_matches_purpose(
            &location,
            StorageZonePurpose::Pick
        ));
        assert!(!location_matches_purpose(
            &location,
            StorageZonePurpose::Reserve
        ));
    }
}
