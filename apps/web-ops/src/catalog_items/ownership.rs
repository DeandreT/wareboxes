use leptos::prelude::*;
use wareboxes_core::dto::{AddInventoryOwnerItem, InventoryOwnerItemIdRequest};
use wareboxes_core::models::InventoryOwnerItem;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::use_toast_bus;

use super::super::{CatalogData, CatalogStore};

#[component]
pub(super) fn ItemOwnerAssignments(
    store: CatalogStore,
    item_id: i64,
    can_supervise: bool,
    inactive: bool,
) -> impl IntoView {
    let selected_owner_id = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();

    let add = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Some(inventory_owner_id) = selected_owner_id
            .get_untracked()
            .parse::<i64>()
            .ok()
            .filter(|owner_id| {
                available_owner_options(&store.data.get_untracked(), item_id)
                    .iter()
                    .any(|(id, _)| id == owner_id)
            })
        else {
            error.set(Some(
                "Select a client to make eligible for this item.".to_owned(),
            ));
            return;
        };
        pending.set(true);
        error.set(None);
        let request = AddInventoryOwnerItem {
            inventory_owner_id,
            item_id,
        };
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, InventoryOwnerItem>(
                "/api/items/inventory-owners/add",
                &request,
            )
            .await
            {
                Ok(assignment) => {
                    let owner_name =
                        owner_label(&store.data.get_untracked(), assignment.inventory_owner_id);
                    store.data.update(|data| {
                        data.item_owner_assignments
                            .retain(|current| current.id != assignment.id);
                        data.item_owner_assignments.push(assignment);
                    });
                    selected_owner_id.set(String::new());
                    pending.set(false);
                    toasts.success(format!("{owner_name} can now use this item."));
                }
                Err(api_error) if api_error.unauthorized => store.on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    error.set(Some(api_error.message));
                    pending.set(false);
                }
            }
        });
    };

    view! {
        <section class="catalog-subsection">
            <div class="catalog-subheading">
                <h3>"Client eligibility"</h3>
                <span>{move || assigned_owners(&store.data.get(), item_id).len()}</span>
            </div>
            {can_supervise.then(|| view! {
                <form class="inline-command owner-item-command" on:submit=add>
                    <label>
                        <span class="sr-only">"Eligible client"</span>
                        <select
                            required
                            disabled=inactive
                            prop:value=move || selected_owner_id.get()
                            on:change=move |event| selected_owner_id.set(event_target_value(&event))
                        >
                            <option value="">"Select client"</option>
                            {move || available_owner_options(&store.data.get(), item_id)
                                .into_iter()
                                .map(|(id, label)| view! { <option value=id.to_string()>{label}</option> })
                                .collect_view()}
                        </select>
                    </label>
                    <button
                        class="button secondary-action compact"
                        type="submit"
                        disabled=move || pending.get() || inactive
                    >
                        "Add client"
                    </button>
                </form>
            })}
            {move || error.get().map(|message| view! {
                <p class="inline-error" role="alert">{message}</p>
            })}
            <div class="identifier-list owner-item-list">
                {move || {
                    let rows = assigned_owners(&store.data.get(), item_id);
                    if rows.is_empty() {
                        view! {
                            <p class="catalog-empty">"No clients are explicitly eligible."</p>
                        }
                        .into_any()
                    } else {
                        rows.into_iter().map(|(assignment, label)| {
                            let assignment_id = assignment.id;
                            let remove_label = label.clone();
                            let remove = move |_| {
                                if pending.get_untracked() {
                                    return;
                                }
                                pending.set(true);
                                error.set(None);
                                let request = InventoryOwnerItemIdRequest {
                                    inventory_owner_item_id: assignment_id,
                                };
                                let success_label = remove_label.clone();
                                leptos::task::spawn_local(async move {
                                    match api::internal_post::<_, bool>(
                                        "/api/items/inventory-owners/delete",
                                        &request,
                                    )
                                    .await
                                    {
                                        Ok(true) => {
                                            store.data.update(|data| {
                                                data.item_owner_assignments
                                                    .retain(|current| current.id != assignment_id);
                                            });
                                            pending.set(false);
                                            toasts.success(format!(
                                                "{success_label} is no longer eligible for this item."
                                            ));
                                        }
                                        Ok(false) => {
                                            let message = "The client assignment is no longer available.".to_owned();
                                            error.set(Some(message.clone()));
                                            toasts.error(message);
                                            pending.set(false);
                                        }
                                        Err(api_error) if api_error.unauthorized => {
                                            store.on_unauthorized.run(())
                                        }
                                        Err(api_error) => {
                                            toasts.error(api_error.message.clone());
                                            error.set(Some(api_error.message));
                                            pending.set(false);
                                        }
                                    }
                                });
                            };
                            view! {
                                <div class="identifier-row owner-item-row">
                                    <strong>{label}</strong>
                                    <span>"Receiving, orders, and policy setup"</span>
                                    {can_supervise.then(|| view! {
                                        <button
                                            class="button barcode-action danger-action"
                                            type="button"
                                            aria-label=format!("Remove client eligibility {assignment_id}")
                                            title="Remove client eligibility"
                                            disabled=move || pending.get() || inactive
                                            on:click=remove
                                        >
                                            <Icon icon=UiIcon::Remove/>
                                        </button>
                                    })}
                                </div>
                            }
                        }).collect_view().into_any()
                    }
                }}
            </div>
        </section>
    }
}

fn owner_label(data: &CatalogData, owner_id: i64) -> String {
    data.clients
        .iter()
        .find(|owner| owner.id == owner_id)
        .map(|owner| owner.name.clone())
        .unwrap_or_else(|| format!("Client #{owner_id}"))
}

fn assigned_owners(data: &CatalogData, item_id: i64) -> Vec<(InventoryOwnerItem, String)> {
    let mut rows = data
        .item_owner_assignments
        .iter()
        .filter(|assignment| assignment.deleted.is_none() && assignment.item_id == item_id)
        .map(|assignment| {
            (
                assignment.clone(),
                owner_label(data, assignment.inventory_owner_id),
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left.1
            .to_ascii_lowercase()
            .cmp(&right.1.to_ascii_lowercase())
    });
    rows
}

fn available_owner_options(data: &CatalogData, item_id: i64) -> Vec<(i64, String)> {
    let mut options = data
        .clients
        .iter()
        .filter(|owner| {
            owner.deleted.is_none()
                && !data.item_owner_assignments.iter().any(|assignment| {
                    assignment.deleted.is_none()
                        && assignment.item_id == item_id
                        && assignment.inventory_owner_id == owner.id
                })
        })
        .map(|owner| (owner.id, owner.name.clone()))
        .collect::<Vec<_>>();
    options.sort_by(|left, right| {
        left.1
            .to_ascii_lowercase()
            .cmp(&right.1.to_ascii_lowercase())
    });
    options
}

#[cfg(test)]
mod tests {
    use super::{assigned_owners, available_owner_options};
    use crate::catalog::CatalogData;
    use serde_json::json;
    use wareboxes_core::models::{InventoryOwner, InventoryOwnerItem};

    fn owner(id: i64, name: &str) -> InventoryOwner {
        serde_json::from_value(json!({
            "id": id,
            "tenant_id": 1,
            "created": "1970-01-01T00:00:00Z",
            "deleted": null,
            "name": name,
            "email": format!("{id}@example.com"),
            "inventory_owner_facilities": []
        }))
        .unwrap()
    }

    #[test]
    fn eligibility_projection_separates_assigned_and_available_clients() {
        let data = CatalogData {
            clients: vec![owner(2, "Zulu"), owner(1, "Alpha")],
            item_owner_assignments: vec![serde_json::from_value::<InventoryOwnerItem>(json!({
                "id": 9,
                "tenant_id": 1,
                "created": "1970-01-01T00:00:00Z",
                "deleted": null,
                "inventory_owner_id": 2,
                "item_id": 7
            }))
            .unwrap()],
            ..CatalogData::default()
        };

        assert_eq!(assigned_owners(&data, 7)[0].1, "Zulu");
        assert_eq!(
            available_owner_options(&data, 7),
            vec![(1, "Alpha".to_owned())]
        );
    }
}
