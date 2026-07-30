use leptos::prelude::*;
use wareboxes_core::dto::{AddPermission, PermissionIdRequest, UpdatePermission};
use wareboxes_core::models::Permission;

use super::super::{
    optional_text, status_class, DeletedToggle, InlineCommandError, WorkbenchError,
    WorkbenchLoading,
};
use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PermissionSort {
    Name,
    Description,
    Status,
}

#[component]
pub fn PermissionsWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let permissions = RwSignal::new(Vec::<Permission>::new());
    let loading = RwSignal::new(true);
    let load_error = RwSignal::new(None::<String>);
    let command_error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);
    let show_deleted = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let selected_id = RwSignal::new(None::<i64>);
    let edit_name = RwSignal::new(String::new());
    let edit_description = RwSignal::new(String::new());
    let new_name = RwSignal::new(String::new());
    let new_description = RwSignal::new(String::new());
    let sort = RwSignal::new(SortSpec {
        key: PermissionSort::Name,
        direction: SortDirection::Ascending,
    });
    let toasts = use_toast_bus();

    let refresh = Callback::new(move |_| {
        refresh_permissions(
            show_deleted.get_untracked(),
            permissions,
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
        let description = optional_text(&new_description.get_untracked());
        if name.len() < 3 || description.as_ref().is_some_and(|value| value.len() < 3) {
            command_error.set(Some(
                "Permission names and descriptions must be at least three characters.".to_owned(),
            ));
            return;
        }
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>(
                "/api/permissions/add",
                &AddPermission {
                    name: name.clone(),
                    description,
                },
            )
            .await
            {
                Ok(_) => {
                    new_name.set(String::new());
                    new_description.set(String::new());
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
        let Some(permission_id) = selected_id.get_untracked() else {
            return;
        };
        if pending.get_untracked() {
            return;
        }
        let name = edit_name.get_untracked().trim().to_owned();
        let description = optional_text(&edit_description.get_untracked());
        if name.len() < 3 || description.as_ref().is_some_and(|value| value.len() < 3) {
            command_error.set(Some(
                "Permission names and descriptions must be at least three characters.".to_owned(),
            ));
            return;
        }
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(
                "/api/permissions/update",
                &UpdatePermission {
                    permission_id,
                    name: Some(name.clone()),
                    description,
                },
            )
            .await
            {
                Ok(true) => {
                    toasts.success(format!("{name} updated."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected permission no longer exists.".to_owned();
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

    let set_active = move |permission: Permission, active: bool| {
        if pending.get_untracked() {
            return;
        }
        let path = if active {
            "/api/permissions/restore"
        } else {
            "/api/permissions/delete"
        };
        let name = permission.name.clone();
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(
                path,
                &PermissionIdRequest {
                    permission_id: permission.id,
                },
            )
            .await
            {
                Ok(true) => {
                    if !active && selected_id.get_untracked() == Some(permission.id) {
                        selected_id.set(None);
                    }
                    let state = if active { "reactivated" } else { "deactivated" };
                    toasts.success(format!("{name} {state}."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected permission no longer exists.".to_owned();
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
                <summary>"Create permission"</summary>
                <form class="admin-create-form" on:submit=create>
                    <div class="admin-form-grid">
                        <label><span>"Permission key"</span><input type="text" minlength="3" required prop:value=move || new_name.get() on:input=move |event| new_name.set(event_target_value(&event))/></label>
                        <label><span>"Description"</span><input type="text" minlength="3" prop:value=move || new_description.get() on:input=move |event| new_description.set(event_target_value(&event))/></label>
                    </div>
                    <div class="admin-form-actions"><button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Create permission"</button></div>
                </form>
            </details>
            <div class="admin-toolbar">
                <SearchField label="Filter permissions".to_owned() placeholder="Filter permissions" value=filter/>
                <div class="admin-toolbar-actions">
                    <DeletedToggle show_deleted/>
                    <button type="button" class="button secondary-action compact" on:click=move |_| refresh.run(())>"Refresh"</button>
                </div>
            </div>
            <InlineCommandError message=command_error.read_only()/>
            {move || {
                if loading.get() {
                    view! { <WorkbenchLoading label="permissions"/> }.into_any()
                } else if let Some(message) = load_error.get() {
                    view! { <WorkbenchError message retry=refresh/> }.into_any()
                } else {
                    view! {
                        <div class="admin-split">
                            <section class="admin-list">
                                <div class="table-scroll">
                                    <table class="data-table admin-table">
                                        <caption class="sr-only">"Permissions in this organization"</caption>
                                        <thead><tr>
                                            <SortableHeader label="Permission" active=move || sort.get().key == PermissionSort::Name direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PermissionSort::Name))/>
                                            <SortableHeader label="Description" active=move || sort.get().key == PermissionSort::Description direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PermissionSort::Description))/>
                                            <SortableHeader label="Status" active=move || sort.get().key == PermissionSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PermissionSort::Status))/>
                                            <th scope="col" class="action-column">"Actions"</th>
                                        </tr></thead>
                                        <tbody>
                                            {move || {
                                                let query = filter.get().trim().to_ascii_lowercase();
                                                let mut rows = permissions.get().into_iter().filter(|permission| {
                                                    query.is_empty()
                                                        || permission.name.to_ascii_lowercase().contains(&query)
                                                        || permission.description.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&query)
                                                }).collect::<Vec<_>>();
                                                sort_permissions(&mut rows, sort.get());
                                                if rows.is_empty() {
                                                    view! { <tr><td class="table-empty-row" colspan="4">"No matching permissions."</td></tr> }.into_any()
                                                } else {
                                                    rows.into_iter().map(|permission| {
                                                        let edit_permission = permission.clone();
                                                        let active_permission = permission.clone();
                                                        let inactive = permission.deleted.is_some();
                                                        view! {
                                                            <tr class:selected-row=selected_id.get() == Some(permission.id)>
                                                                <td><strong>{permission.name.clone()}</strong></td>
                                                                <td>{permission.description.clone().unwrap_or_default()}</td>
                                                                <td><span class=status_class(inactive)>{if inactive { "Inactive" } else { "Active" }}</span></td>
                                                                <td class="action-column"><div class="admin-row-actions">
                                                                    <button type="button" class="table-action" on:click=move |_| {
                                                                        selected_id.set(Some(edit_permission.id));
                                                                        edit_name.set(edit_permission.name.clone());
                                                                        edit_description.set(edit_permission.description.clone().unwrap_or_default());
                                                                        command_error.set(None);
                                                                    }>"Edit"</button>
                                                                    <button type="button" class=if inactive { "table-action" } else { "table-action danger" } disabled=move || pending.get() on:click=move |_| set_active(active_permission.clone(), inactive)>{if inactive { "Reactivate" } else { "Deactivate" }}</button>
                                                                </div></td>
                                                            </tr>
                                                        }
                                                    }).collect_view().into_any()
                                                }
                                            }}
                                        </tbody>
                                    </table>
                                </div>
                            </section>
                            <section class="admin-editor" aria-label="Permission editor">
                                {move || selected_id.get().map_or_else(
                                    || view! { <div class="admin-editor-placeholder">"Select a permission to edit its key and description."</div> }.into_any(),
                                    |id| view! {
                                        <div class="admin-editor-heading"><h2>"Edit permission"</h2><span>{format!("#{id}")}</span></div>
                                        <form class="admin-form" on:submit=save>
                                            <label><span>"Permission key"</span><input type="text" minlength="3" required prop:value=move || edit_name.get() on:input=move |event| edit_name.set(event_target_value(&event))/></label>
                                            <label><span>"Description"</span><textarea minlength="3" prop:value=move || edit_description.get() on:input=move |event| edit_description.set(event_target_value(&event))></textarea></label>
                                            <div class="admin-form-actions">
                                                <button type="button" class="button quiet-action compact" on:click=move |_| selected_id.set(None)>"Cancel"</button>
                                                <button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Save"</button>
                                            </div>
                                        </form>
                                    }.into_any()
                                )}
                            </section>
                        </div>
                    }.into_any()
                }
            }}
        </section>
    }
}

fn refresh_permissions(
    show_deleted: bool,
    permissions: RwSignal<Vec<Permission>>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    loading.set(true);
    load_error.set(None);
    leptos::task::spawn_local(async move {
        match api::internal_get::<Vec<Permission>>(&format!(
            "/api/permissions?show_deleted={show_deleted}"
        ))
        .await
        {
            Ok(new_permissions) => permissions.set(new_permissions),
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => load_error.set(Some(error.message)),
        }
        loading.set(false);
    });
}

fn sort_permissions(rows: &mut [Permission], spec: SortSpec<PermissionSort>) {
    rows.sort_by(|left, right| {
        let ordering = match spec.key {
            PermissionSort::Name => left
                .name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase()),
            PermissionSort::Description => left
                .description
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .cmp(
                    &right
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                ),
            PermissionSort::Status => left.deleted.is_some().cmp(&right.deleted.is_some()),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}
