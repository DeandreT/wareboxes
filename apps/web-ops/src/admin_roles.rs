use leptos::prelude::*;
use wareboxes_core::dto::{
    AddDeleteChildRole, AddDeleteRolePermission, AddRole, RoleIdRequest, UpdateRole,
};
use wareboxes_core::models::{Permission, Role};

use super::super::{
    optional_text, selected_id, status_class, DeletedToggle, InlineCommandError, WorkbenchError,
    WorkbenchLoading,
};
use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RoleSort {
    Name,
    Parent,
    Permissions,
    Status,
}

#[component]
pub fn RolesWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let roles = RwSignal::new(Vec::<Role>::new());
    let permissions = RwSignal::new(Vec::<Permission>::new());
    let loading = RwSignal::new(true);
    let load_error = RwSignal::new(None::<String>);
    let command_error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);
    let show_deleted = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let selected_role_id = RwSignal::new(None::<i64>);
    let edit_name = RwSignal::new(String::new());
    let edit_description = RwSignal::new(String::new());
    let edit_parent = RwSignal::new(String::new());
    let original_parent = RwSignal::new(None::<i64>);
    let permission_choice = RwSignal::new(String::new());
    let new_name = RwSignal::new(String::new());
    let new_description = RwSignal::new(String::new());
    let sort = RwSignal::new(SortSpec {
        key: RoleSort::Name,
        direction: SortDirection::Ascending,
    });
    let toasts = use_toast_bus();

    let refresh = Callback::new(move |_| {
        refresh_roles(
            show_deleted.get_untracked(),
            roles,
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
        if name.is_empty() {
            command_error.set(Some("Enter a role name.".to_owned()));
            return;
        }
        let request = AddRole {
            name: name.clone(),
            description: optional_text(&new_description.get_untracked()),
        };
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/roles/add", &request).await {
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
        let Some(role_id) = selected_role_id.get_untracked() else {
            return;
        };
        if pending.get_untracked() {
            return;
        }
        let name = edit_name.get_untracked().trim().to_owned();
        if name.is_empty() {
            command_error.set(Some("Enter a role name.".to_owned()));
            return;
        }
        let requested_parent = selected_id(&edit_parent.get_untracked());
        let prior_parent = original_parent.get_untracked();
        let request = UpdateRole {
            role_id,
            name: Some(name.clone()),
            description: optional_text(&edit_description.get_untracked()),
        };
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            let result = async {
                let updated = api::internal_post::<_, bool>("/api/roles/update", &request).await?;
                if !updated {
                    return Err(api::ApiError {
                        message: "The selected role no longer exists.".to_owned(),
                        unauthorized: false,
                        ambiguous_outcome: false,
                    });
                }
                if requested_parent != prior_parent {
                    let hierarchy_updated = if let Some(parent_id) = requested_parent {
                        api::internal_post::<_, bool>(
                            "/api/roles/children/add",
                            &AddDeleteChildRole {
                                role_id: parent_id,
                                child_role_id: role_id,
                            },
                        )
                        .await?
                    } else {
                        api::internal_post::<_, bool>(
                            "/api/roles/children/delete",
                            &AddDeleteChildRole {
                                role_id: prior_parent.unwrap_or(role_id),
                                child_role_id: role_id,
                            },
                        )
                        .await?
                    };
                    if !hierarchy_updated {
                        return Err(api::ApiError {
                            message: "The role was updated but its hierarchy could not be changed."
                                .to_owned(),
                            unauthorized: false,
                            ambiguous_outcome: false,
                        });
                    }
                }
                Ok::<_, api::ApiError>(())
            }
            .await;
            match result {
                Ok(()) => {
                    original_parent.set(requested_parent);
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

    let change_permission = move |grant: bool| {
        let Some(role_id) = selected_role_id.get_untracked() else {
            return;
        };
        let Some(permission_id) = selected_id(&permission_choice.get_untracked()) else {
            command_error.set(Some("Choose a permission.".to_owned()));
            return;
        };
        if pending.get_untracked() {
            return;
        }
        let path = if grant {
            "/api/roles/permissions/add"
        } else {
            "/api/roles/permissions/delete"
        };
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(
                path,
                &AddDeleteRolePermission {
                    role_id,
                    permission_id,
                },
            )
            .await
            {
                Ok(true) => {
                    let action = if grant { "granted" } else { "removed" };
                    toasts.success(format!("Direct permission {action}."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = if grant {
                        "The permission could not be granted."
                    } else {
                        "No direct grant exists for that permission."
                    }
                    .to_owned();
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

    let set_active = move |role: Role, active: bool| {
        if pending.get_untracked() || role.is_self_role() {
            return;
        }
        let path = if active {
            "/api/roles/restore"
        } else {
            "/api/roles/delete"
        };
        let name = role.name.clone();
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(path, &RoleIdRequest { role_id: role.id }).await {
                Ok(true) => {
                    if !active && selected_role_id.get_untracked() == Some(role.id) {
                        selected_role_id.set(None);
                    }
                    let state = if active { "reactivated" } else { "deactivated" };
                    toasts.success(format!("{name} {state}."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected role is not editable.".to_owned();
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
                <summary>"Create role"</summary>
                <form class="admin-create-form" on:submit=create>
                    <div class="admin-form-grid">
                        <label><span>"Role name"</span><input type="text" required prop:value=move || new_name.get() on:input=move |event| new_name.set(event_target_value(&event))/></label>
                        <label><span>"Description"</span><input type="text" prop:value=move || new_description.get() on:input=move |event| new_description.set(event_target_value(&event))/></label>
                    </div>
                    <div class="admin-form-actions"><button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Create role"</button></div>
                </form>
            </details>
            <div class="admin-toolbar">
                <SearchField label="Filter roles".to_owned() placeholder="Filter roles" value=filter/>
                <div class="admin-toolbar-actions">
                    <DeletedToggle show_deleted/>
                    <button type="button" class="button secondary-action compact" on:click=move |_| refresh.run(())>"Refresh"</button>
                </div>
            </div>
            <InlineCommandError message=command_error.read_only()/>

            {move || {
                if loading.get() {
                    view! { <WorkbenchLoading label="roles"/> }.into_any()
                } else if let Some(message) = load_error.get() {
                    view! { <WorkbenchError message retry=refresh/> }.into_any()
                } else {
                    view! {
                        <div class="admin-split">
                            <section class="admin-list">
                                <div class="table-scroll">
                                    <table class="data-table admin-table">
                                        <caption class="sr-only">"Roles in this organization"</caption>
                                        <thead>
                                            <tr>
                                                <SortableHeader label="Role" active=move || sort.get().key == RoleSort::Name direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, RoleSort::Name))/>
                                                <SortableHeader label="Parent" active=move || sort.get().key == RoleSort::Parent direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, RoleSort::Parent))/>
                                                <SortableHeader label="Effective permissions" active=move || sort.get().key == RoleSort::Permissions direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, RoleSort::Permissions))/>
                                                <SortableHeader label="Status" active=move || sort.get().key == RoleSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, RoleSort::Status))/>
                                                <th scope="col" class="action-column">"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {move || role_rows(
                                                roles.get(),
                                                filter.get(),
                                                sort.get(),
                                                selected_role_id,
                                                edit_name,
                                                edit_description,
                                                edit_parent,
                                                original_parent,
                                                permission_choice,
                                                command_error,
                                                pending,
                                                set_active,
                                            )}
                                        </tbody>
                                    </table>
                                </div>
                            </section>
                            <section class="admin-editor" aria-label="Role editor">
                                {move || {
                                    let selected = selected_role_id
                                        .get()
                                        .and_then(|id| roles.get().into_iter().find(|role| role.id == id));
                                    selected.map_or_else(
                                        || view! { <div class="admin-editor-placeholder">"Select a role to edit hierarchy and direct permission grants."</div> }.into_any(),
                                        |role| {
                                            let effective = role.role_permissions.iter().map(|permission| permission.name.clone()).collect::<Vec<_>>();
                                            view! {
                                                <div class="admin-editor-heading"><h2>{role.name}</h2><span>{format!("#{}", role.id)}</span></div>
                                                <form class="admin-form" on:submit=save>
                                                    <label><span>"Role name"</span><input type="text" required prop:value=move || edit_name.get() on:input=move |event| edit_name.set(event_target_value(&event))/></label>
                                                    <label><span>"Description"</span><textarea prop:value=move || edit_description.get() on:input=move |event| edit_description.set(event_target_value(&event))></textarea></label>
                                                    <label>
                                                        <span>"Parent role"</span>
                                                        <select prop:value=move || edit_parent.get() on:change=move |event| edit_parent.set(event_target_value(&event))>
                                                            <option value="">"No parent"</option>
                                                            {move || {
                                                                let current = selected_role_id.get();
                                                                roles.get().into_iter().filter(move |candidate| candidate.deleted.is_none() && Some(candidate.id) != current).map(|candidate| view! { <option value=candidate.id.to_string()>{candidate.name}</option> }).collect_view()
                                                            }}
                                                        </select>
                                                    </label>
                                                    <div class="admin-form-actions"><button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Save role"</button></div>
                                                </form>
                                                <section class="admin-form">
                                                    <label>
                                                        <span>"Permission"</span>
                                                        <select prop:value=move || permission_choice.get() on:change=move |event| permission_choice.set(event_target_value(&event))>
                                                            <option value="">"Select permission"</option>
                                                            {move || permissions.get().into_iter().filter(|permission| permission.deleted.is_none()).map(|permission| view! { <option value=permission.id.to_string()>{permission.name}</option> }).collect_view()}
                                                        </select>
                                                    </label>
                                                    <div class="admin-form-actions">
                                                        <button type="button" class="button quiet-action compact" disabled=move || pending.get() on:click=move |_| change_permission(false)>"Remove direct grant"</button>
                                                        <button type="button" class="button secondary-action compact" disabled=move || pending.get() on:click=move |_| change_permission(true)>"Grant permission"</button>
                                                    </div>
                                                    <div class="admin-chip-list">
                                                        {if effective.is_empty() {
                                                            view! { <span class="admin-cell-detail">"No effective permissions"</span> }.into_any()
                                                        } else {
                                                            effective.into_iter().map(|name| view! { <span class="admin-chip">{name}</span> }).collect_view().into_any()
                                                        }}
                                                    </div>
                                                </section>
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

#[allow(clippy::too_many_arguments)]
fn role_rows(
    mut rows: Vec<Role>,
    filter: String,
    spec: SortSpec<RoleSort>,
    selected_role_id: RwSignal<Option<i64>>,
    edit_name: RwSignal<String>,
    edit_description: RwSignal<String>,
    edit_parent: RwSignal<String>,
    original_parent: RwSignal<Option<i64>>,
    permission_choice: RwSignal<String>,
    command_error: RwSignal<Option<String>>,
    pending: RwSignal<bool>,
    set_active: impl Fn(Role, bool) + Copy + Send + Sync + 'static,
) -> AnyView {
    let query = filter.trim().to_ascii_lowercase();
    rows.retain(|role| {
        query.is_empty()
            || role.name.to_ascii_lowercase().contains(&query)
            || role
                .description
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains(&query)
            || role
                .role_permissions
                .iter()
                .any(|permission| permission.name.to_ascii_lowercase().contains(&query))
    });
    sort_roles(&mut rows, spec);
    if rows.is_empty() {
        return view! { <tr><td class="table-empty-row" colspan="5">"No matching roles."</td></tr> }
            .into_any();
    }
    let labels = rows.clone();
    rows.into_iter()
        .map(|role| {
            let edit_role = role.clone();
            let active_role = role.clone();
            let inactive = role.deleted.is_some();
            let parent = role
                .parent_id
                .and_then(|id| labels.iter().find(|candidate| candidate.id == id))
                .map(|parent| parent.name.clone())
                .unwrap_or_else(|| "None".to_owned());
            let permission_names = role
                .role_permissions
                .iter()
                .map(|permission| permission.name.clone())
                .collect::<Vec<_>>();
            view! {
                <tr class:selected-row=selected_role_id.get() == Some(role.id)>
                    <td><div class="cell-stack"><strong>{role.name.clone()}</strong><small>{role.description.clone().unwrap_or_else(|| format!("Role #{}", role.id))}</small></div></td>
                    <td>{parent}</td>
                    <td><div class="admin-chip-list">{permission_names.into_iter().take(4).map(|name| view! { <span class="admin-chip">{name}</span> }).collect_view()}</div></td>
                    <td><span class=status_class(inactive)>{if inactive { "Inactive" } else { "Active" }}</span></td>
                    <td class="action-column">
                        <div class="admin-row-actions">
                            <button
                                type="button"
                                class="table-action"
                                on:click=move |_| {
                                    selected_role_id.set(Some(edit_role.id));
                                    edit_name.set(edit_role.name.clone());
                                    edit_description.set(edit_role.description.clone().unwrap_or_default());
                                    edit_parent.set(edit_role.parent_id.map(|id| id.to_string()).unwrap_or_default());
                                    original_parent.set(edit_role.parent_id);
                                    permission_choice.set(String::new());
                                    command_error.set(None);
                                }
                            >
                                "Edit"
                            </button>
                            <button type="button" class=if inactive { "table-action" } else { "table-action danger" } disabled=move || pending.get() on:click=move |_| set_active(active_role.clone(), inactive)>
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

fn refresh_roles(
    show_deleted: bool,
    roles: RwSignal<Vec<Role>>,
    permissions: RwSignal<Vec<Permission>>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    loading.set(true);
    load_error.set(None);
    leptos::task::spawn_local(async move {
        let result = async {
            let roles = api::internal_get::<Vec<Role>>(&format!(
                "/api/roles?show_deleted={show_deleted}&show_self=false"
            ))
            .await?;
            let permissions =
                api::internal_get::<Vec<Permission>>("/api/permissions?show_deleted=true").await?;
            Ok::<_, api::ApiError>((roles, permissions))
        }
        .await;
        match result {
            Ok((new_roles, new_permissions)) => {
                roles.set(new_roles);
                permissions.set(new_permissions);
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => load_error.set(Some(error.message)),
        }
        loading.set(false);
    });
}

fn sort_roles(rows: &mut [Role], spec: SortSpec<RoleSort>) {
    rows.sort_by(|left, right| {
        let ordering = match spec.key {
            RoleSort::Name => left
                .name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase()),
            RoleSort::Parent => left.parent_id.cmp(&right.parent_id),
            RoleSort::Permissions => left
                .role_permissions
                .len()
                .cmp(&right.role_permissions.len()),
            RoleSort::Status => left.deleted.is_some().cmp(&right.deleted.is_some()),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}
