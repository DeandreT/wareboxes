use leptos::prelude::*;
use wareboxes_core::dto::{AddDeleteUserRole, UserIdRequest, UserUpdate};
use wareboxes_core::models::{Role, User};

use super::super::{
    optional_text, status_class, DeletedToggle, InlineCommandError, WorkbenchError,
    WorkbenchLoading,
};
use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;

#[derive(Clone, Copy, PartialEq, Eq)]
enum UserSort {
    Name,
    Email,
    Roles,
    Status,
}

#[component]
pub fn UsersWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let users = RwSignal::new(Vec::<User>::new());
    let roles = RwSignal::new(Vec::<Role>::new());
    let loading = RwSignal::new(true);
    let load_error = RwSignal::new(None::<String>);
    let command_error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);
    let show_deleted = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let selected_id = RwSignal::new(None::<i64>);
    let edit_first_name = RwSignal::new(String::new());
    let edit_last_name = RwSignal::new(String::new());
    let edit_nick_name = RwSignal::new(String::new());
    let edit_phone = RwSignal::new(String::new());
    let role_choice = RwSignal::new(String::new());
    let sort = RwSignal::new(SortSpec {
        key: UserSort::Name,
        direction: SortDirection::Ascending,
    });
    let toasts = use_toast_bus();

    let refresh = Callback::new(move |_| {
        refresh_users(
            show_deleted.get_untracked(),
            users,
            roles,
            loading,
            load_error,
            on_unauthorized,
        );
    });
    Effect::new(move || {
        let _ = show_deleted.get();
        refresh.run(());
    });

    let save_profile = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(user_id) = selected_id.get_untracked() else {
            return;
        };
        if pending.get_untracked() {
            return;
        }
        let request = UserUpdate {
            user_id,
            first_name: optional_text(&edit_first_name.get_untracked()),
            last_name: optional_text(&edit_last_name.get_untracked()),
            nick_name: optional_text(&edit_nick_name.get_untracked()),
            phone: optional_text(&edit_phone.get_untracked()),
        };
        if request.first_name.is_none()
            && request.last_name.is_none()
            && request.nick_name.is_none()
            && request.phone.is_none()
        {
            command_error.set(Some("Enter at least one profile value.".to_owned()));
            return;
        }
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>("/api/users/update", &request).await {
                Ok(true) => {
                    toasts.success("User profile updated.");
                    refresh.run(());
                }
                Ok(false) => {
                    let message =
                        "The selected user is no longer active in this organization.".to_owned();
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

    let change_role = move |assign: bool| {
        let Some(user_id) = selected_id.get_untracked() else {
            return;
        };
        let Some(role_id) = role_choice
            .get_untracked()
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
        else {
            command_error.set(Some("Choose a role.".to_owned()));
            return;
        };
        if pending.get_untracked() {
            return;
        }
        let path = if assign {
            "/api/users/roles/add"
        } else {
            "/api/users/roles/delete"
        };
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(path, &AddDeleteUserRole { user_id, role_id }).await
            {
                Ok(true) => {
                    let action = if assign { "assigned" } else { "removed" };
                    toasts.success(format!("Role {action}."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = if assign {
                        "The role could not be assigned."
                    } else {
                        "That direct role assignment was not found."
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

    let set_active = move |user: User, active: bool| {
        if pending.get_untracked() {
            return;
        }
        let path = if active {
            "/api/users/restore"
        } else {
            "/api/users/delete"
        };
        let label = user_display_name(&user);
        pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>(path, &UserIdRequest { user_id: user.id }).await {
                Ok(true) => {
                    if !active && selected_id.get_untracked() == Some(user.id) {
                        selected_id.set(None);
                    }
                    let state = if active { "reactivated" } else { "deactivated" };
                    toasts.success(format!("{label} {state}."));
                    refresh.run(());
                }
                Ok(false) => {
                    let message = "The selected user is no longer in this organization.".to_owned();
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
            <div class="admin-toolbar">
                <SearchField label="Filter users".to_owned() placeholder="Filter users" value=filter/>
                <div class="admin-toolbar-actions">
                    <DeletedToggle show_deleted/>
                    <button type="button" class="button secondary-action compact" on:click=move |_| refresh.run(())>"Refresh"</button>
                </div>
            </div>
            <InlineCommandError message=command_error.read_only()/>

            {move || {
                if loading.get() {
                    view! { <WorkbenchLoading label="users"/> }.into_any()
                } else if let Some(message) = load_error.get() {
                    view! { <WorkbenchError message retry=refresh/> }.into_any()
                } else {
                    view! {
                        <div class="admin-split">
                            <section class="admin-list">
                                <div class="table-scroll">
                                    <table class="data-table admin-table">
                                        <caption class="sr-only">"Users in this organization"</caption>
                                        <thead>
                                            <tr>
                                                <SortableHeader label="User" active=move || sort.get().key == UserSort::Name direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, UserSort::Name))/>
                                                <SortableHeader label="Email" active=move || sort.get().key == UserSort::Email direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, UserSort::Email))/>
                                                <SortableHeader label="Direct roles" active=move || sort.get().key == UserSort::Roles direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, UserSort::Roles))/>
                                                <SortableHeader label="Status" active=move || sort.get().key == UserSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, UserSort::Status))/>
                                                <th scope="col" class="action-column">"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {move || {
                                                let query = filter.get().trim().to_ascii_lowercase();
                                                let mut rows = users
                                                    .get()
                                                    .into_iter()
                                                    .filter(|user| {
                                                        query.is_empty()
                                                            || user_display_name(user).to_ascii_lowercase().contains(&query)
                                                            || user.email.to_ascii_lowercase().contains(&query)
                                                            || user.user_roles.iter().any(|role| role.name.to_ascii_lowercase().contains(&query))
                                                    })
                                                    .collect::<Vec<_>>();
                                                sort_users(&mut rows, sort.get());
                                                if rows.is_empty() {
                                                    view! { <tr><td class="table-empty-row" colspan="5">"No matching users."</td></tr> }.into_any()
                                                } else {
                                                    rows
                                                        .into_iter()
                                                        .map(|user| {
                                                            let edit_user = user.clone();
                                                            let active_user = user.clone();
                                                            let inactive = user.deleted.is_some();
                                                            let role_names = user.user_roles.iter().map(|role| role.name.clone()).collect::<Vec<_>>();
                                                            view! {
                                                                <tr class:selected-row=selected_id.get() == Some(user.id)>
                                                                    <td><div class="cell-stack"><strong>{user_display_name(&user)}</strong><small>{format!("User #{}", user.id)}</small></div></td>
                                                                    <td>{user.email.clone()}</td>
                                                                    <td><ChipList values=role_names empty="No direct roles"/></td>
                                                                    <td><span class=status_class(inactive)>{if inactive { "Inactive" } else { "Active" }}</span></td>
                                                                    <td class="action-column">
                                                                        <div class="admin-row-actions">
                                                                            <button
                                                                                type="button"
                                                                                class="table-action"
                                                                                on:click=move |_| {
                                                                                    selected_id.set(Some(edit_user.id));
                                                                                    edit_first_name.set(edit_user.first_name.clone().unwrap_or_default());
                                                                                    edit_last_name.set(edit_user.last_name.clone().unwrap_or_default());
                                                                                    edit_nick_name.set(edit_user.nick_name.clone().unwrap_or_default());
                                                                                    edit_phone.set(edit_user.phone.clone().unwrap_or_default());
                                                                                    role_choice.set(String::new());
                                                                                    command_error.set(None);
                                                                                }
                                                                            >
                                                                                "Edit"
                                                                            </button>
                                                                            <button type="button" class=if inactive { "table-action" } else { "table-action danger" } disabled=move || pending.get() on:click=move |_| set_active(active_user.clone(), inactive)>
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
                            <section class="admin-editor" aria-label="User editor">
                                {move || {
                                    let selected = selected_id
                                        .get()
                                        .and_then(|id| users.get().into_iter().find(|user| user.id == id));
                                    selected.map_or_else(
                                        || view! { <div class="admin-editor-placeholder">"Select a user to edit profile and role assignments."</div> }.into_any(),
                                        |user| {
                                            let direct_roles = user.user_roles.iter().map(|role| role.name.clone()).collect::<Vec<_>>();
                                            let effective_permissions = user.user_permissions.iter().map(|permission| permission.name.clone()).collect::<Vec<_>>();
                                            view! {
                                                <div class="admin-editor-heading"><h2>{user_display_name(&user)}</h2><span>{user.email}</span></div>
                                                <form class="admin-form" on:submit=save_profile>
                                                    <div class="admin-form-grid">
                                                        <label><span>"First name"</span><input type="text" prop:value=move || edit_first_name.get() on:input=move |event| edit_first_name.set(event_target_value(&event))/></label>
                                                        <label><span>"Last name"</span><input type="text" prop:value=move || edit_last_name.get() on:input=move |event| edit_last_name.set(event_target_value(&event))/></label>
                                                        <label><span>"Preferred name"</span><input type="text" prop:value=move || edit_nick_name.get() on:input=move |event| edit_nick_name.set(event_target_value(&event))/></label>
                                                        <label><span>"Phone"</span><input type="tel" prop:value=move || edit_phone.get() on:input=move |event| edit_phone.set(event_target_value(&event))/></label>
                                                    </div>
                                                    <div class="admin-form-actions">
                                                        <button type="submit" class="button primary-action compact" disabled=move || pending.get()>"Save profile"</button>
                                                    </div>
                                                </form>
                                                <section class="admin-form">
                                                    <label>
                                                        <span>"Role"</span>
                                                        <select prop:value=move || role_choice.get() on:change=move |event| role_choice.set(event_target_value(&event))>
                                                            <option value="">"Select role"</option>
                                                            {move || roles.get().into_iter().filter(|role| !role.is_self_role() && role.deleted.is_none()).map(|role| view! { <option value=role.id.to_string()>{role.name}</option> }).collect_view()}
                                                        </select>
                                                    </label>
                                                    <div class="admin-form-actions">
                                                        <button type="button" class="button quiet-action compact" disabled=move || pending.get() on:click=move |_| change_role(false)>"Remove direct role"</button>
                                                        <button type="button" class="button secondary-action compact" disabled=move || pending.get() on:click=move |_| change_role(true)>"Assign role"</button>
                                                    </div>
                                                    <div><span class="admin-cell-detail">"Direct roles"</span><ChipList values=direct_roles empty="None"/></div>
                                                    <div><span class="admin-cell-detail">"Effective permissions"</span><ChipList values=effective_permissions empty="None"/></div>
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

#[component]
fn ChipList(values: Vec<String>, empty: &'static str) -> impl IntoView {
    view! {
        <div class="admin-chip-list">
            {if values.is_empty() {
                view! { <span class="admin-cell-detail">{empty}</span> }.into_any()
            } else {
                values.into_iter().map(|value| view! { <span class="admin-chip">{value}</span> }).collect_view().into_any()
            }}
        </div>
    }
}

fn refresh_users(
    show_deleted: bool,
    users: RwSignal<Vec<User>>,
    roles: RwSignal<Vec<Role>>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    loading.set(true);
    load_error.set(None);
    leptos::task::spawn_local(async move {
        let result = async {
            let users =
                api::internal_get::<Vec<User>>(&format!("/api/users?show_deleted={show_deleted}"))
                    .await?;
            let roles =
                api::internal_get::<Vec<Role>>("/api/roles?show_deleted=false&show_self=false")
                    .await?;
            Ok::<_, api::ApiError>((users, roles))
        }
        .await;
        match result {
            Ok((new_users, new_roles)) => {
                users.set(new_users);
                roles.set(new_roles);
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => load_error.set(Some(error.message)),
        }
        loading.set(false);
    });
}

fn user_display_name(user: &User) -> String {
    user.nick_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            let name = [user.first_name.as_deref(), user.last_name.as_deref()]
                .into_iter()
                .flatten()
                .filter(|value| !value.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if name.is_empty() {
                user.email.clone()
            } else {
                name
            }
        })
}

fn sort_users(rows: &mut [User], spec: SortSpec<UserSort>) {
    rows.sort_by(|left, right| {
        let ordering = match spec.key {
            UserSort::Name => user_display_name(left)
                .to_ascii_lowercase()
                .cmp(&user_display_name(right).to_ascii_lowercase()),
            UserSort::Email => left
                .email
                .to_ascii_lowercase()
                .cmp(&right.email.to_ascii_lowercase()),
            UserSort::Roles => left.user_roles.len().cmp(&right.user_roles.len()),
            UserSort::Status => left.deleted.is_some().cmp(&right.deleted.is_some()),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}
