use leptos::prelude::*;
use wareboxes_core::dto::{
    AddInventoryOwner, InventoryOwnerIdRequest, InventoryOwnerUpdate,
    ReplaceInventoryOwnerFacilities,
};
use wareboxes_core::models::{Facility, InventoryOwner};

use super::{
    command_result, facility_names, optional_text, status_class, DeletedToggle, FacilityChecks,
    InlineCommandError, WorkbenchError, WorkbenchLoading,
};
use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClientSort {
    Name,
    Email,
    Facilities,
    Status,
}

#[component]
pub fn ClientsWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let clients = RwSignal::new(Vec::<InventoryOwner>::new());
    let facilities = RwSignal::new(Vec::<Facility>::new());
    let loading = RwSignal::new(true);
    let load_error = RwSignal::new(None::<String>);
    let command_error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);
    let show_deleted = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let selected_id = RwSignal::new(None::<i64>);
    let edit_name = RwSignal::new(String::new());
    let edit_email = RwSignal::new(String::new());
    let edit_facilities = RwSignal::new(Vec::<i64>::new());
    let new_name = RwSignal::new(String::new());
    let new_email = RwSignal::new(String::new());
    let new_facilities = RwSignal::new(Vec::<i64>::new());
    let sort = RwSignal::new(SortSpec {
        key: ClientSort::Name,
        direction: SortDirection::Ascending,
    });
    let toasts = use_toast_bus();

    let refresh = Callback::new(move |_| {
        refresh_clients(
            show_deleted.get_untracked(),
            clients,
            facilities,
            loading,
            load_error,
            on_unauthorized,
        );
    });
    Effect::new(move || {
        let _ = show_deleted.get();
        refresh.run(());
    });

    let create = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let name = new_name.get_untracked().trim().to_owned();
        let email = new_email.get_untracked().trim().to_owned();
        let facility_ids = new_facilities.get_untracked();
        if name.len() < 3 || email.is_empty() {
            command_error.set(Some(
                "Enter a client name of at least three characters and a valid email.".to_owned(),
            ));
            return;
        }
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            let request = AddInventoryOwner {
                name: name.clone(),
                email,
            };
            let result = async {
                let id =
                    api::internal_post::<_, i64>("/api/inventory-owners/add", &request).await?;
                if !facility_ids.is_empty() {
                    let assign = ReplaceInventoryOwnerFacilities {
                        inventory_owner_id: id,
                        facility_ids,
                    };
                    let assigned =
                        api::internal_post::<_, bool>("/api/inventory-owners/facilities", &assign)
                            .await?;
                    command_result(
                        assigned,
                        "The client was created but facilities were not assigned.",
                    )
                    .map_err(|message| api::ApiError {
                        message,
                        unauthorized: false,
                    })?;
                }
                Ok::<_, api::ApiError>(id)
            }
            .await;
            match result {
                Ok(_) => {
                    new_name.set(String::new());
                    new_email.set(String::new());
                    new_facilities.set(Vec::new());
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

    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(id) = selected_id.get_untracked() else {
            return;
        };
        if pending.get_untracked() {
            return;
        }
        let name = edit_name.get_untracked().trim().to_owned();
        let email = edit_email.get_untracked().trim().to_owned();
        let facility_ids = edit_facilities.get_untracked();
        if name.len() < 3 || email.is_empty() {
            command_error.set(Some(
                "Enter a client name of at least three characters and a valid email.".to_owned(),
            ));
            return;
        }
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            let result = async {
                let updated = api::internal_post::<_, bool>(
                    "/api/inventory-owners/update",
                    &InventoryOwnerUpdate {
                        inventory_owner_id: id,
                        name: Some(name.clone()),
                        email: Some(email),
                    },
                )
                .await?;
                command_result(updated, "The selected client no longer exists.").map_err(
                    |message| api::ApiError {
                        message,
                        unauthorized: false,
                    },
                )?;
                let assigned = api::internal_post::<_, bool>(
                    "/api/inventory-owners/facilities",
                    &ReplaceInventoryOwnerFacilities {
                        inventory_owner_id: id,
                        facility_ids,
                    },
                )
                .await?;
                command_result(assigned, "Facility assignments could not be saved.").map_err(
                    |message| api::ApiError {
                        message,
                        unauthorized: false,
                    },
                )
            }
            .await;
            match result {
                Ok(()) => {
                    toasts.success(format!("{name} updated."));
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

    let set_active = move |client: InventoryOwner, active: bool| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        command_error.set(None);
        let name = client.name.clone();
        let path = if active {
            "/api/inventory-owners/restore"
        } else {
            "/api/inventory-owners/delete"
        };
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(
                path,
                &InventoryOwnerIdRequest {
                    inventory_owner_id: client.id,
                },
            )
            .await
            {
                Ok(true) => {
                    if !active && selected_id.get_untracked() == Some(client.id) {
                        selected_id.set(None);
                    }
                    let state = if active { "reactivated" } else { "deactivated" };
                    toasts.success(format!("{name} {state}."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected client no longer exists.".to_owned();
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
                <summary>"Create client"</summary>
                <form class="admin-create-form" on:submit=create>
                    <div class="admin-form-grid">
                        <label>
                            <span>"Client name"</span>
                            <input
                                type="text"
                                minlength="3"
                                required
                                autocomplete="organization"
                                prop:value=move || new_name.get()
                                on:input=move |event| new_name.set(event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span>"Operations email"</span>
                            <input
                                type="email"
                                required
                                autocomplete="email"
                                prop:value=move || new_email.get()
                                on:input=move |event| new_email.set(event_target_value(&event))
                            />
                        </label>
                    </div>
                    <FacilityChecks
                        facilities=Signal::derive(move || facilities.get())
                        selected=new_facilities
                        legend="Assigned facilities"
                    />
                    <div class="admin-form-actions">
                        <button class="button primary-action compact" type="submit" disabled=move || pending.get()>
                            "Create client"
                        </button>
                    </div>
                </form>
            </details>

            <div class="admin-toolbar">
                <SearchField label="Filter clients".to_owned() placeholder="Filter clients" value=filter/>
                <div class="admin-toolbar-actions">
                    <DeletedToggle show_deleted/>
                    <button class="button secondary-action compact" type="button" on:click=move |_| refresh.run(())>
                        "Refresh"
                    </button>
                </div>
            </div>
            <InlineCommandError message=command_error.read_only()/>

            {move || {
                if loading.get() {
                    view! { <WorkbenchLoading label="clients"/> }.into_any()
                } else if let Some(message) = load_error.get() {
                    view! { <WorkbenchError message retry=refresh/> }.into_any()
                } else {
                    view! {
                        <div class="admin-split">
                            <section class="admin-list">
                                <div class="table-scroll">
                                    <table class="data-table admin-table">
                                        <caption class="sr-only">"Clients in this organization"</caption>
                                        <thead>
                                            <tr>
                                                <SortableHeader
                                                    label="Client"
                                                    active=move || sort.get().key == ClientSort::Name
                                                    direction=move || sort.get().direction
                                                    on_sort=Callback::new(move |_| SortSpec::select(sort, ClientSort::Name))
                                                />
                                                <SortableHeader
                                                    label="Email"
                                                    active=move || sort.get().key == ClientSort::Email
                                                    direction=move || sort.get().direction
                                                    on_sort=Callback::new(move |_| SortSpec::select(sort, ClientSort::Email))
                                                />
                                                <SortableHeader
                                                    label="Facilities"
                                                    active=move || sort.get().key == ClientSort::Facilities
                                                    direction=move || sort.get().direction
                                                    on_sort=Callback::new(move |_| SortSpec::select(sort, ClientSort::Facilities))
                                                />
                                                <SortableHeader
                                                    label="Status"
                                                    active=move || sort.get().key == ClientSort::Status
                                                    direction=move || sort.get().direction
                                                    on_sort=Callback::new(move |_| SortSpec::select(sort, ClientSort::Status))
                                                />
                                                <th class="action-column" scope="col">"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {move || {
                                                let query = filter.get().trim().to_ascii_lowercase();
                                                let all_facilities = facilities.get();
                                                let mut rows = clients
                                                    .get()
                                                    .into_iter()
                                                    .filter(|client| {
                                                        query.is_empty()
                                                            || client.name.to_ascii_lowercase().contains(&query)
                                                            || client.email.to_ascii_lowercase().contains(&query)
                                                    })
                                                    .collect::<Vec<_>>();
                                                sort_clients(&mut rows, sort.get());
                                                if rows.is_empty() {
                                                    view! {
                                                        <tr><td class="table-empty-row" colspan="5">"No matching clients."</td></tr>
                                                    }
                                                        .into_any()
                                                } else {
                                                    rows
                                                        .into_iter()
                                                        .map(|client| {
                                                            let select_client = client.clone();
                                                            let active_client = client.clone();
                                                            let facility_ids = client
                                                                .inventory_owner_facilities
                                                                .iter()
                                                                .map(|facility| facility.id)
                                                                .collect::<Vec<_>>();
                                                            let facility_copy = facility_ids.clone();
                                                            let facility_names = facility_names(&facility_ids, &all_facilities);
                                                            let facility_names_title = facility_names.clone();
                                                            let inactive = client.deleted.is_some();
                                                            let selected = selected_id.get() == Some(client.id);
                                                            view! {
                                                                <tr class:selected-row=selected>
                                                                    <td>
                                                                        <div class="cell-stack">
                                                                            <strong>{client.name.clone()}</strong>
                                                                            <small>{format!("Client #{}", client.id)}</small>
                                                                        </div>
                                                                    </td>
                                                                    <td>{client.email.clone()}</td>
                                                                    <td title=facility_names_title>{facility_names}</td>
                                                                    <td><span class=status_class(inactive)>{if inactive { "Inactive" } else { "Active" }}</span></td>
                                                                    <td class="action-column">
                                                                        <div class="admin-row-actions">
                                                                            <button
                                                                                type="button"
                                                                                class="table-action"
                                                                                on:click=move |_| {
                                                                                    selected_id.set(Some(select_client.id));
                                                                                    edit_name.set(select_client.name.clone());
                                                                                    edit_email.set(select_client.email.clone());
                                                                                    edit_facilities.set(facility_copy.clone());
                                                                                    command_error.set(None);
                                                                                }
                                                                            >
                                                                                "Edit"
                                                                            </button>
                                                                            <button
                                                                                type="button"
                                                                                class=if inactive { "table-action" } else { "table-action danger" }
                                                                                disabled=move || pending.get()
                                                                                on:click=move |_| set_active(active_client.clone(), inactive)
                                                                            >
                                                                                {if inactive { "Reactivate" } else { "Deactivate" }}
                                                                            </button>
                                                                        </div>
                                                                    </td>
                                                                </tr>
                                                            }
                                                        })
                                                        .collect_view()
                                                        .into_any()
                                                }
                                            }}
                                        </tbody>
                                    </table>
                                </div>
                            </section>

                            <section class="admin-editor" aria-label="Client editor">
                                {move || {
                                    selected_id.get().map_or_else(
                                        || view! { <div class="admin-editor-placeholder">"Select a client to edit contact and facility assignments."</div> }.into_any(),
                                        |id| {
                                            view! {
                                                <div class="admin-editor-heading">
                                                    <h2>"Edit client"</h2>
                                                    <span>{format!("#{id}")}</span>
                                                </div>
                                                <form class="admin-form" on:submit=save>
                                                    <label>
                                                        <span>"Client name"</span>
                                                        <input type="text" minlength="3" required prop:value=move || edit_name.get() on:input=move |event| edit_name.set(event_target_value(&event))/>
                                                    </label>
                                                    <label>
                                                        <span>"Operations email"</span>
                                                        <input type="email" required prop:value=move || edit_email.get() on:input=move |event| edit_email.set(event_target_value(&event))/>
                                                    </label>
                                                    <FacilityChecks
                                                        facilities=Signal::derive(move || facilities.get())
                                                        selected=edit_facilities
                                                        legend="Assigned facilities"
                                                    />
                                                    <div class="admin-form-actions">
                                                        <button type="button" class="button quiet-action compact" on:click=move |_| selected_id.set(None)>"Cancel"</button>
                                                        <button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Save"</button>
                                                    </div>
                                                </form>
                                            }
                                                .into_any()
                                        },
                                    )
                                }}
                            </section>
                        </div>
                    }
                        .into_any()
                }
            }}
        </section>
    }
}

fn refresh_clients(
    show_deleted: bool,
    clients: RwSignal<Vec<InventoryOwner>>,
    facilities: RwSignal<Vec<Facility>>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    loading.set(true);
    load_error.set(None);
    leptos::task::spawn_local(async move {
        let result = async {
            let clients = api::internal_get::<Vec<InventoryOwner>>(&format!(
                "/api/inventory-owners?show_deleted={show_deleted}"
            ))
            .await?;
            let facilities =
                api::internal_get::<Vec<Facility>>("/api/facilities?show_deleted=true").await?;
            Ok::<_, api::ApiError>((clients, facilities))
        }
        .await;
        match result {
            Ok((new_clients, new_facilities)) => {
                clients.set(new_clients);
                facilities.set(new_facilities);
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => load_error.set(Some(error.message)),
        }
        loading.set(false);
    });
}

fn sort_clients(clients: &mut [InventoryOwner], spec: SortSpec<ClientSort>) {
    clients.sort_by(|left, right| {
        let ordering = match spec.key {
            ClientSort::Name => left
                .name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase()),
            ClientSort::Email => left
                .email
                .to_ascii_lowercase()
                .cmp(&right.email.to_ascii_lowercase()),
            ClientSort::Facilities => left
                .inventory_owner_facilities
                .len()
                .cmp(&right.inventory_owner_facilities.len()),
            ClientSort::Status => left.deleted.is_some().cmp(&right.deleted.is_some()),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

#[allow(dead_code)]
fn client_update(inventory_owner_id: i64, name: &str, email: &str) -> InventoryOwnerUpdate {
    InventoryOwnerUpdate {
        inventory_owner_id,
        name: optional_text(name),
        email: optional_text(email),
    }
}

#[cfg(test)]
mod tests {
    use super::client_update;

    #[test]
    fn client_update_normalizes_optional_text() {
        let request = client_update(7, " Acme ", " dispatch@acme.test ");
        assert_eq!(request.inventory_owner_id, 7);
        assert_eq!(request.name.as_deref(), Some("Acme"));
        assert_eq!(request.email.as_deref(), Some("dispatch@acme.test"));
    }
}
