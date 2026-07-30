use leptos::prelude::*;
use wareboxes_core::dto::{
    AddAuditLocationCount, AddAuditWave, AuditLocationCountIdRequest, AuditLocationCountUpdate,
    AuditWaveIdRequest, AuditWaveUpdate,
};
use wareboxes_core::models::{
    AuditLocationCount, AuditWave, Facility, InventoryOwner, Item, Location,
};

use super::{
    facility_label, optional_text, selected_id, status_class, ClientPicker, DeletedToggle,
    FacilityPicker, InlineCommandError, WorkbenchError, WorkbenchLoading,
};
use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlanSort {
    Name,
    Facility,
    Client,
    Status,
}

#[component]
pub fn CountPlansWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let plans = RwSignal::new(Vec::<AuditWave>::new());
    let facilities = RwSignal::new(Vec::<Facility>::new());
    let clients = RwSignal::new(Vec::<InventoryOwner>::new());
    let locations = RwSignal::new(Vec::<Location>::new());
    let items = RwSignal::new(Vec::<Item>::new());
    let counts = RwSignal::new(Vec::<AuditLocationCount>::new());
    let loading = RwSignal::new(true);
    let counts_loading = RwSignal::new(false);
    let load_error = RwSignal::new(None::<String>);
    let command_error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);
    let show_deleted = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let selected_plan_id = RwSignal::new(None::<i64>);
    let edit_name = RwSignal::new(String::new());
    let edit_description = RwSignal::new(String::new());
    let new_name = RwSignal::new(String::new());
    let new_description = RwSignal::new(String::new());
    let new_facility = RwSignal::new(String::new());
    let new_client = RwSignal::new(String::new());
    let count_location = RwSignal::new(String::new());
    let count_item = RwSignal::new(String::new());
    let count_uom = RwSignal::new("EA".to_owned());
    let count_lot = RwSignal::new(String::new());
    let count_serial = RwSignal::new(String::new());
    let count_quantity = RwSignal::new("0".to_owned());
    let editing_count = RwSignal::new(None::<(i64, i64)>);
    let sort = RwSignal::new(SortSpec {
        key: PlanSort::Name,
        direction: SortDirection::Ascending,
    });
    let toasts = use_toast_bus();

    let refresh = Callback::new(move |_| {
        refresh_plans(
            show_deleted.get_untracked(),
            plans,
            facilities,
            clients,
            locations,
            items,
            loading,
            load_error,
            on_unauthorized,
        );
    });
    Effect::new(move || {
        let _ = show_deleted.get();
        refresh.run(());
    });

    let load_counts = Callback::new(move |plan_id: i64| {
        counts_loading.set(true);
        leptos::task::spawn_local(async move {
            match api::internal_get::<Vec<AuditLocationCount>>(&format!(
                "/api/audits/{plan_id}/counts"
            ))
            .await
            {
                Ok(new_counts) => counts.set(new_counts),
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            counts_loading.set(false);
        });
    });

    let create_plan = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(facility_id) = selected_id(&new_facility.get_untracked()) else {
            command_error.set(Some("Choose a facility.".to_owned()));
            return;
        };
        let Some(inventory_owner_id) = selected_id(&new_client.get_untracked()) else {
            command_error.set(Some("Choose a client.".to_owned()));
            return;
        };
        let name = new_name.get_untracked().trim().to_owned();
        if name.is_empty() || pending.get_untracked() {
            command_error.set(Some("Enter a plan name.".to_owned()));
            return;
        }
        let request = AddAuditWave {
            facility_id,
            inventory_owner_id,
            name: name.clone(),
            description: optional_text(&new_description.get_untracked()),
        };
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/audits/add", &request).await {
                Ok(_) => {
                    new_name.set(String::new());
                    new_description.set(String::new());
                    new_facility.set(String::new());
                    new_client.set(String::new());
                    toasts.success(format!("{name} created."));
                    refresh.run(());
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            pending.set(false);
        });
    };

    let save_plan = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(audit_wave_id) = selected_plan_id.get_untracked() else {
            return;
        };
        let name = edit_name.get_untracked().trim().to_owned();
        if name.is_empty() || pending.get_untracked() {
            command_error.set(Some("Enter a plan name.".to_owned()));
            return;
        }
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(
                "/api/audits/update",
                &AuditWaveUpdate {
                    audit_wave_id,
                    name: Some(name.clone()),
                    description: optional_text(&edit_description.get_untracked()),
                },
            )
            .await
            {
                Ok(true) => {
                    toasts.success(format!("{name} updated."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected count plan no longer exists.".to_owned();
                    toasts.error(message.clone());
                    command_error.set(Some(message));
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            pending.set(false);
        });
    };

    let set_plan_active = move |plan: AuditWave, active: bool| {
        if pending.get_untracked() {
            return;
        }
        let path = if active {
            "/api/audits/restore"
        } else {
            "/api/audits/delete"
        };
        let name = plan
            .name
            .clone()
            .unwrap_or_else(|| format!("Plan #{}", plan.id));
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(
                path,
                &AuditWaveIdRequest {
                    audit_wave_id: plan.id,
                },
            )
            .await
            {
                Ok(true) => {
                    if !active && selected_plan_id.get_untracked() == Some(plan.id) {
                        selected_plan_id.set(None);
                        counts.set(Vec::new());
                    }
                    let state = if active { "reactivated" } else { "deactivated" };
                    toasts.success(format!("{name} {state}."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected count plan no longer exists.".to_owned();
                    toasts.error(message.clone());
                    command_error.set(Some(message));
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            pending.set(false);
        });
    };

    let save_count = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(audit_wave_id) = selected_plan_id.get_untracked() else {
            return;
        };
        let Some(location_id) = selected_id(&count_location.get_untracked()) else {
            command_error.set(Some("Choose a location.".to_owned()));
            return;
        };
        let Some(item_id) = selected_id(&count_item.get_untracked()) else {
            command_error.set(Some("Choose an item.".to_owned()));
            return;
        };
        let Ok(count) = count_quantity.get_untracked().parse::<i64>() else {
            command_error.set(Some("Enter a whole-number count.".to_owned()));
            return;
        };
        let uom = count_uom.get_untracked().trim().to_owned();
        if count < 0 || uom.is_empty() || pending.get_untracked() {
            command_error.set(Some(
                "Count cannot be negative and UOM is required.".to_owned(),
            ));
            return;
        }
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            let result = if let Some((audit_location_count_id, expected_revision)) =
                editing_count.get_untracked()
            {
                api::internal_post::<_, bool>(
                    "/api/audits/counts/update",
                    &AuditLocationCountUpdate {
                        audit_location_count_id,
                        expected_revision,
                        count,
                    },
                )
                .await
                .map(|updated| updated.then_some(audit_location_count_id))
            } else {
                api::internal_post::<_, i64>(
                    "/api/audits/counts/add",
                    &AddAuditLocationCount {
                        audit_wave_id,
                        location_id,
                        item_id,
                        uom,
                        lot: optional_text(&count_lot.get_untracked()),
                        expiration: None,
                        serial: optional_text(&count_serial.get_untracked()),
                        count,
                    },
                )
                .await
                .map(Some)
            };
            match result {
                Ok(Some(_)) => {
                    editing_count.set(None);
                    count_location.set(String::new());
                    count_item.set(String::new());
                    count_lot.set(String::new());
                    count_serial.set(String::new());
                    count_quantity.set("0".to_owned());
                    toasts.success("Count line saved.");
                    load_counts.run(audit_wave_id);
                }
                Ok(None) => {
                    let message =
                        "The count line changed elsewhere. Refresh and try again.".to_owned();
                    toasts.error(message.clone());
                    command_error.set(Some(message));
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            pending.set(false);
        });
    };

    let set_count_active = move |count_line: AuditLocationCount, active: bool| {
        let Some(plan_id) = selected_plan_id.get_untracked() else {
            return;
        };
        if pending.get_untracked() {
            return;
        }
        let path = if active {
            "/api/audits/counts/restore"
        } else {
            "/api/audits/counts/delete"
        };
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(
                path,
                &AuditLocationCountIdRequest {
                    audit_location_count_id: count_line.id,
                    expected_revision: count_line.revision,
                },
            )
            .await
            {
                Ok(true) => {
                    let state = if active { "restored" } else { "removed" };
                    toasts.success(format!("Count line {state}."));
                    load_counts.run(plan_id);
                }
                Ok(false) => {
                    let message =
                        "The count line changed elsewhere. Refresh and try again.".to_owned();
                    toasts.error(message.clone());
                    command_error.set(Some(message));
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                }
            }
            pending.set(false);
        });
    };

    view! {
        <section class="admin-workbench">
            <details class="admin-create">
                <summary>"Create count plan"</summary>
                <form class="admin-create-form" on:submit=create_plan>
                    <div class="admin-form-grid">
                        <label><span>"Plan name"</span><input type="text" required prop:value=move || new_name.get() on:input=move |event| new_name.set(event_target_value(&event))/></label>
                        <label><span>"Description"</span><input type="text" prop:value=move || new_description.get() on:input=move |event| new_description.set(event_target_value(&event))/></label>
                        <FacilityPicker facilities=Signal::derive(move || facilities.get()) selected=new_facility id="new-count-facility" label="Facility"/>
                        <ClientPicker clients=Signal::derive(move || clients.get()) selected=new_client id="new-count-client" label="Client"/>
                    </div>
                    <div class="admin-form-actions"><button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Create plan"</button></div>
                </form>
            </details>
            <div class="admin-toolbar">
                <SearchField label="Filter count plans".to_owned() placeholder="Filter count plans" value=filter/>
                <div class="admin-toolbar-actions">
                    <DeletedToggle show_deleted/>
                    <button type="button" class="button secondary-action compact" on:click=move |_| refresh.run(())>"Refresh"</button>
                </div>
            </div>
            <InlineCommandError message=command_error.read_only()/>

            {move || {
                if loading.get() {
                    view! { <WorkbenchLoading label="count plans"/> }.into_any()
                } else if let Some(message) = load_error.get() {
                    view! { <WorkbenchError message retry=refresh/> }.into_any()
                } else {
                    view! {
                        <div class="admin-split">
                            <section class="admin-list">
                                <div class="table-scroll">
                                    <table class="data-table admin-table">
                                        <caption class="sr-only">"Inventory count plans"</caption>
                                        <thead><tr>
                                            <SortableHeader label="Plan" active=move || sort.get().key == PlanSort::Name direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlanSort::Name))/>
                                            <SortableHeader label="Facility" active=move || sort.get().key == PlanSort::Facility direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlanSort::Facility))/>
                                            <SortableHeader label="Client" active=move || sort.get().key == PlanSort::Client direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlanSort::Client))/>
                                            <SortableHeader label="Status" active=move || sort.get().key == PlanSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlanSort::Status))/>
                                            <th scope="col" class="action-column">"Actions"</th>
                                        </tr></thead>
                                        <tbody>{move || plan_rows(
                                            plans.get(),
                                            facilities.get(),
                                            clients.get(),
                                            filter.get(),
                                            sort.get(),
                                            selected_plan_id,
                                            edit_name,
                                            edit_description,
                                            command_error,
                                            pending,
                                            load_counts,
                                            set_plan_active,
                                        )}</tbody>
                                    </table>
                                </div>
                            </section>
                            <section class="admin-editor" aria-label="Count plan editor">
                                {move || {
                                    let selected = selected_plan_id.get().and_then(|id| plans.get().into_iter().find(|plan| plan.id == id));
                                    selected.map_or_else(
                                        || view! { <div class="admin-editor-placeholder">"Select a count plan to maintain its count lines."</div> }.into_any(),
                                        |plan| {
                                            let facility_id = plan.facility_id;
                                            view! {
                                                <div class="admin-editor-heading"><h2>{plan.name.clone().unwrap_or_else(|| format!("Plan #{}", plan.id))}</h2><span>{format!("#{}", plan.id)}</span></div>
                                                <form class="admin-form" on:submit=save_plan>
                                                    <label><span>"Plan name"</span><input type="text" required prop:value=move || edit_name.get() on:input=move |event| edit_name.set(event_target_value(&event))/></label>
                                                    <label><span>"Description"</span><textarea prop:value=move || edit_description.get() on:input=move |event| edit_description.set(event_target_value(&event))></textarea></label>
                                                    <div class="admin-form-actions"><button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Save plan"</button></div>
                                                </form>
                                                <form class="admin-form" on:submit=save_count>
                                                    <CountFields
                                                        locations=Signal::derive(move || locations.get().into_iter().filter(|location| location.facility_id == facility_id && location.deleted.is_none()).collect())
                                                        items=Signal::derive(move || items.get())
                                                        location=count_location
                                                        item=count_item
                                                        uom=count_uom
                                                        lot=count_lot
                                                        serial=count_serial
                                                        quantity=count_quantity
                                                        editing=editing_count
                                                    />
                                                    <div class="admin-form-actions">
                                                        <button type="button" class="button quiet-action compact" on:click=move |_| {
                                                            editing_count.set(None);
                                                            count_location.set(String::new());
                                                            count_item.set(String::new());
                                                            count_quantity.set("0".to_owned());
                                                        }>"Clear"</button>
                                                        <button type="submit" class="button secondary-action compact" disabled=move || pending.get()>{move || if editing_count.get().is_some() { "Update count" } else { "Add count" }}</button>
                                                    </div>
                                                </form>
                                                <CountLedger
                                                    counts
                                                    locations=Signal::derive(move || locations.get())
                                                    items=Signal::derive(move || items.get())
                                                    loading=counts_loading.read_only()
                                                    editing_count
                                                    count_location
                                                    count_item
                                                    count_uom
                                                    count_lot
                                                    count_serial
                                                    count_quantity
                                                    pending=pending.read_only()
                                                    set_active=Callback::new(move |(line, active)| set_count_active(line, active))
                                                />
                                            }.into_any()
                                        },
                                    )
                                }}
                            </section>
                        </div>
                    }.into_any()
                }
            }}
        </section>
    }
}

#[component]
fn CountFields(
    locations: Signal<Vec<Location>>,
    items: Signal<Vec<Item>>,
    location: RwSignal<String>,
    item: RwSignal<String>,
    uom: RwSignal<String>,
    lot: RwSignal<String>,
    serial: RwSignal<String>,
    quantity: RwSignal<String>,
    editing: RwSignal<Option<(i64, i64)>>,
) -> impl IntoView {
    view! {
        <div class="admin-form-grid">
            <label><span>"Location"</span><select required disabled=move || editing.get().is_some() prop:value=move || location.get() on:change=move |event| location.set(event_target_value(&event))>
                <option value="">"Select location"</option>
                {move || locations.get().into_iter().map(|location| view! { <option value=location.id.to_string()>{location_label(&location)}</option> }).collect_view()}
            </select></label>
            <label><span>"Item"</span><select required disabled=move || editing.get().is_some() prop:value=move || item.get() on:change=move |event| item.set(event_target_value(&event))>
                <option value="">"Select item"</option>
                {move || items.get().into_iter().filter(|item| item.deleted.is_none()).map(|item| view! { <option value=item.id.to_string()>{item_label(&item)}</option> }).collect_view()}
            </select></label>
            <label><span>"UOM"</span><input type="text" required disabled=move || editing.get().is_some() prop:value=move || uom.get() on:input=move |event| uom.set(event_target_value(&event))/></label>
            <label><span>"Count"</span><input type="number" min="0" step="1" required prop:value=move || quantity.get() on:input=move |event| quantity.set(event_target_value(&event))/></label>
            <label><span>"Lot"</span><input type="text" disabled=move || editing.get().is_some() prop:value=move || lot.get() on:input=move |event| lot.set(event_target_value(&event))/></label>
            <label><span>"Serial"</span><input type="text" disabled=move || editing.get().is_some() prop:value=move || serial.get() on:input=move |event| serial.set(event_target_value(&event))/></label>
        </div>
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn CountLedger(
    counts: RwSignal<Vec<AuditLocationCount>>,
    locations: Signal<Vec<Location>>,
    items: Signal<Vec<Item>>,
    loading: ReadSignal<bool>,
    editing_count: RwSignal<Option<(i64, i64)>>,
    count_location: RwSignal<String>,
    count_item: RwSignal<String>,
    count_uom: RwSignal<String>,
    count_lot: RwSignal<String>,
    count_serial: RwSignal<String>,
    count_quantity: RwSignal<String>,
    pending: ReadSignal<bool>,
    set_active: Callback<(AuditLocationCount, bool)>,
) -> impl IntoView {
    view! {
        <div class="table-scroll">
            <table class="data-table admin-table">
                <caption class="sr-only">"Count lines for the selected plan"</caption>
                <thead><tr><th>"Location"</th><th>"Item"</th><th class="numeric">"Expected"</th><th class="numeric">"Count"</th><th>"Approval"</th><th class="action-column">"Actions"</th></tr></thead>
                <tbody>{move || {
                    if loading.get() {
                        view! { <tr><td class="table-empty-row" colspan="6">"Loading count lines..."</td></tr> }.into_any()
                    } else if counts.get().is_empty() {
                        view! { <tr><td class="table-empty-row" colspan="6">"No count lines."</td></tr> }.into_any()
                    } else {
                        let location_rows = locations.get();
                        let item_rows = items.get();
                        counts.get().into_iter().map(|line| {
                            let edit_line = line.clone();
                            let active_line = line.clone();
                            let inactive = line.deleted.is_some();
                            let location_name = location_rows.iter().find(|location| location.id == line.location_id).map(location_label).unwrap_or_else(|| format!("Location #{}", line.location_id));
                            let item_name = item_rows.iter().find(|item| item.id == line.item_id).map(item_label).unwrap_or_else(|| format!("Item #{}", line.item_id));
                            view! {
                                <tr>
                                    <td>{location_name}</td>
                                    <td>{item_name}</td>
                                    <td class="numeric">{line.on_hand}</td>
                                    <td class="numeric">{line.count}</td>
                                    <td><span class="status processing">{line.approval_status.to_string()}</span></td>
                                    <td class="action-column"><div class="admin-row-actions">
                                        <button type="button" class="table-action" disabled=inactive on:click=move |_| {
                                            editing_count.set(Some((edit_line.id, edit_line.revision)));
                                            count_location.set(edit_line.location_id.to_string());
                                            count_item.set(edit_line.item_id.to_string());
                                            count_uom.set(edit_line.uom.clone());
                                            count_lot.set(edit_line.lot.clone().unwrap_or_default());
                                            count_serial.set(edit_line.serial.clone().unwrap_or_default());
                                            count_quantity.set(edit_line.count.to_string());
                                        }>"Edit"</button>
                                        <button type="button" class=if inactive { "table-action" } else { "table-action danger" } disabled=move || pending.get() on:click=move |_| set_active.run((active_line.clone(), inactive))>{if inactive { "Restore" } else { "Remove" }}</button>
                                    </div></td>
                                </tr>
                            }
                        }).collect_view().into_any()
                    }
                }}</tbody>
            </table>
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_rows(
    mut rows: Vec<AuditWave>,
    facilities: Vec<Facility>,
    clients: Vec<InventoryOwner>,
    filter: String,
    spec: SortSpec<PlanSort>,
    selected_plan_id: RwSignal<Option<i64>>,
    edit_name: RwSignal<String>,
    edit_description: RwSignal<String>,
    command_error: RwSignal<Option<String>>,
    pending: RwSignal<bool>,
    load_counts: Callback<i64>,
    set_active: impl Fn(AuditWave, bool) + Copy + Send + Sync + 'static,
) -> AnyView {
    let query = filter.trim().to_ascii_lowercase();
    rows.retain(|plan| {
        query.is_empty()
            || plan
                .name
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains(&query)
            || plan
                .description
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains(&query)
    });
    sort_plans(&mut rows, spec);
    if rows.is_empty() {
        return view! { <tr><td class="table-empty-row" colspan="5">"No matching count plans."</td></tr> }.into_any();
    }
    rows.into_iter().map(|plan| {
        let edit_plan = plan.clone();
        let active_plan = plan.clone();
        let inactive = plan.deleted.is_some();
        let facility = facilities.iter().find(|facility| facility.id == plan.facility_id).map(facility_label).unwrap_or_else(|| format!("Facility #{}", plan.facility_id));
        let client = clients.iter().find(|client| client.id == plan.inventory_owner_id).map(|client| client.name.clone()).unwrap_or_else(|| format!("Client #{}", plan.inventory_owner_id));
        view! {
            <tr class:selected-row=selected_plan_id.get() == Some(plan.id)>
                <td><div class="cell-stack"><strong>{plan.name.clone().unwrap_or_else(|| format!("Plan #{}", plan.id))}</strong><small>{plan.description.clone().unwrap_or_default()}</small></div></td>
                <td>{facility}</td>
                <td>{client}</td>
                <td><span class=status_class(inactive)>{if inactive { "Inactive" } else { "Active" }}</span></td>
                <td class="action-column"><div class="admin-row-actions">
                    <button type="button" class="table-action" on:click=move |_| {
                        selected_plan_id.set(Some(edit_plan.id));
                        edit_name.set(edit_plan.name.clone().unwrap_or_default());
                        edit_description.set(edit_plan.description.clone().unwrap_or_default());
                        command_error.set(None);
                        load_counts.run(edit_plan.id);
                    }>"Open"</button>
                    <button type="button" class=if inactive { "table-action" } else { "table-action danger" } disabled=move || pending.get() on:click=move |_| set_active(active_plan.clone(), inactive)>{if inactive { "Reactivate" } else { "Deactivate" }}</button>
                </div></td>
            </tr>
        }
    }).collect_view().into_any()
}

#[allow(clippy::too_many_arguments)]
fn refresh_plans(
    show_deleted: bool,
    plans: RwSignal<Vec<AuditWave>>,
    facilities: RwSignal<Vec<Facility>>,
    clients: RwSignal<Vec<InventoryOwner>>,
    locations: RwSignal<Vec<Location>>,
    items: RwSignal<Vec<Item>>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    loading.set(true);
    load_error.set(None);
    leptos::task::spawn_local(async move {
        let result = async {
            let plans = api::internal_get::<Vec<AuditWave>>(&format!(
                "/api/audits?show_deleted={show_deleted}"
            ))
            .await?;
            let facilities =
                api::internal_get::<Vec<Facility>>("/api/facilities?show_deleted=false").await?;
            let clients = api::internal_get::<Vec<InventoryOwner>>(
                "/api/inventory-owners?show_deleted=false",
            )
            .await?;
            let locations =
                api::internal_get::<Vec<Location>>("/api/locations?show_deleted=false").await?;
            let items = api::internal_get::<Vec<Item>>("/api/items?show_deleted=false").await?;
            Ok::<_, api::ApiError>((plans, facilities, clients, locations, items))
        }
        .await;
        match result {
            Ok((new_plans, new_facilities, new_clients, new_locations, new_items)) => {
                plans.set(new_plans);
                facilities.set(new_facilities);
                clients.set(new_clients);
                locations.set(new_locations);
                items.set(new_items);
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => load_error.set(Some(error.message)),
        }
        loading.set(false);
    });
}

fn location_label(location: &Location) -> String {
    location
        .name
        .clone()
        .or_else(|| location.barcode.clone())
        .unwrap_or_else(|| format!("Location #{}", location.id))
}

fn item_label(item: &Item) -> String {
    item.skus
        .first()
        .map(|sku| sku.name.clone())
        .or_else(|| item.description.clone())
        .unwrap_or_else(|| format!("Item #{}", item.id))
}

fn sort_plans(rows: &mut [AuditWave], spec: SortSpec<PlanSort>) {
    rows.sort_by(|left, right| {
        let ordering = match spec.key {
            PlanSort::Name => left
                .name
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .cmp(
                    &right
                        .name
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                ),
            PlanSort::Facility => left.facility_id.cmp(&right.facility_id),
            PlanSort::Client => left.inventory_owner_id.cmp(&right.inventory_owner_id),
            PlanSort::Status => left.deleted.is_some().cmp(&right.deleted.is_some()),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}
