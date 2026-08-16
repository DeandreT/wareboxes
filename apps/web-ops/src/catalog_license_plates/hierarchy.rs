use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ChangeLicensePlateParentRequest, LicensePlateHierarchyAction, LicensePlateHierarchyResponse,
};
use wareboxes_core::models::LicensePlate;

use crate::api;
use crate::toast::use_toast_bus;

use super::{CatalogStore, InlineError};

#[component]
pub(super) fn LicensePlateHierarchyPanel(
    store: CatalogStore,
    plate: LicensePlate,
    plates: Vec<LicensePlate>,
) -> impl IntoView {
    let hierarchy = RwSignal::new(None::<LicensePlateHierarchyResponse>);
    let loading = RwSignal::new(true);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let selected_parent = RwSignal::new(String::new());
    let reason = RwSignal::new(String::new());
    let toasts = use_toast_bus();
    let plate_id = plate.id;

    Effect::new(move |_| {
        loading.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::license_plate_hierarchy(plate_id).await {
                Ok(value) => {
                    hierarchy.set(Some(value));
                    loading.set(false);
                }
                Err(api_error) if api_error.unauthorized => store.on_unauthorized.run(()),
                Err(api_error) => {
                    error.set(Some(api_error.message));
                    loading.set(false);
                }
            }
        });
    });

    let candidates = parent_candidates(&plate, &plates);
    let submit = move |parent_license_plate_id: Option<i64>| {
        if pending.get_untracked() {
            return;
        }
        let reason_value = reason.get_untracked().trim().to_owned();
        if reason_value.is_empty() {
            error.set(Some("Record why this physical nesting changed.".into()));
            return;
        }
        pending.set(true);
        error.set(None);
        let request = ChangeLicensePlateParentRequest {
            parent_license_plate_id,
            expected_revision: plate.hierarchy_revision,
            reason: reason_value,
        };
        let key = api::new_idempotency_key();
        leptos::task::spawn_local(async move {
            match api::change_license_plate_parent(plate_id, &request, &key).await {
                Ok(result) => {
                    let message = result.parent_license_plate_id.map_or_else(
                        || format!("License plate #{plate_id} detached from its parent."),
                        |parent_id| format!("License plate #{plate_id} attached to #{parent_id}."),
                    );
                    toasts.success(message);
                    reason.set(String::new());
                    pending.set(false);
                    store.refresh();
                    loading.set(true);
                    match api::license_plate_hierarchy(plate_id).await {
                        Ok(value) => {
                            hierarchy.set(Some(value));
                            loading.set(false);
                        }
                        Err(api_error) => {
                            error.set(Some(api_error.message));
                            loading.set(false);
                        }
                    }
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
    let attach = move |_| {
        let Ok(parent_id) = selected_parent.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a parent container.".into()));
            return;
        };
        submit(Some(parent_id));
    };
    let detach = move |_| submit(None);

    view! {
        <section class="catalog-subsection plate-hierarchy">
            <div class="catalog-subheading">
                <h3>"Container hierarchy"</h3>
                <span>{format!("Revision {}", plate.hierarchy_revision)}</span>
            </div>
            {move || if loading.get() {
                view! { <p class="catalog-empty compact">"Loading hierarchy evidence…"</p> }.into_any()
            } else if let Some(value) = hierarchy.get() {
                hierarchy_view(value).into_any()
            } else {
                view! { <p class="catalog-empty compact">"Hierarchy evidence is unavailable."</p> }.into_any()
            }}
            {if plate.deleted.is_none() {
                if plate.parent_license_plate_id.is_some() {
                    view! {
                        <div class="plate-hierarchy-command">
                            <label><span>"Required reason"</span><input prop:value=move || reason.get() on:input=move |event| reason.set(event_target_value(&event)) placeholder="Why is this subtree being removed?"/></label>
                            <button class="button danger-action compact" type="button" disabled=move || pending.get() on:click=detach>
                                {move || if pending.get() { "Saving…" } else { "Detach subtree" }}
                            </button>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="plate-hierarchy-command">
                            <label><span>"Parent container"</span><select prop:value=move || selected_parent.get() on:change=move |event| selected_parent.set(event_target_value(&event))><option value="">"Choose compatible parent"</option>{candidates.into_iter().map(|candidate|view!{<option value=candidate.id.to_string()>{plate_label(&candidate)}</option>}).collect_view()}</select></label>
                            <label><span>"Required reason"</span><input prop:value=move || reason.get() on:input=move |event| reason.set(event_target_value(&event)) placeholder="Why is this container being nested?"/></label>
                            <button class="button secondary-action compact" type="button" disabled=move || pending.get() on:click=attach>
                                {move || if pending.get() { "Saving…" } else { "Attach to parent" }}
                            </button>
                        </div>
                    }.into_any()
                }
            } else {
                ().into_any()
            }}
            <InlineError error/>
        </section>
    }
}

fn parent_candidates(plate: &LicensePlate, plates: &[LicensePlate]) -> Vec<LicensePlate> {
    let mut candidates = plates
        .iter()
        .filter(|candidate| {
            candidate.id != plate.id
                && candidate.deleted.is_none()
                && candidate.inventory_owner_id == plate.inventory_owner_id
                && candidate.facility_id == plate.facility_id
                && candidate.location_id == plate.location_id
                && !plate.descendant_license_plate_ids.contains(&candidate.id)
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_unstable_by(|left, right| {
        plate_label(left)
            .to_ascii_lowercase()
            .cmp(&plate_label(right).to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates
}

fn plate_label(plate: &LicensePlate) -> String {
    format!(
        "{} · #{} · level {}",
        plate.barcode.as_deref().unwrap_or("Unlabeled"),
        plate.id,
        plate.hierarchy_depth
    )
}

fn hierarchy_view(value: LicensePlateHierarchyResponse) -> impl IntoView {
    let node = value.node;
    let path = value
        .ancestors
        .iter()
        .map(|ancestor| {
            ancestor
                .barcode
                .clone()
                .unwrap_or_else(|| format!("#{}", ancestor.license_plate_id))
        })
        .chain(std::iter::once(
            node.barcode
                .clone()
                .unwrap_or_else(|| format!("#{}", node.license_plate_id)),
        ))
        .collect::<Vec<_>>()
        .join(" › ");
    let children = if node.direct_child_ids.is_empty() {
        "None".into()
    } else {
        node.direct_child_ids
            .iter()
            .map(|id| format!("#{id}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    view! {
        <dl class="plate-hierarchy-facts">
            <div><dt>"Path"</dt><dd>{path}</dd></div>
            <div><dt>"Direct children"</dt><dd>{children}</dd></div>
            <div><dt>"Contained plates"</dt><dd>{node.descendant_ids.len()}</dd></div>
            <div><dt>"Contained units"</dt><dd>{node.contained_unit_quantity}</dd></div>
        </dl>
        <div class="plate-hierarchy-history">
            <strong>"Relationship history"</strong>
            {if value.events.is_empty() {
                view! { <p class="catalog-empty compact">"No parent changes recorded."</p> }.into_any()
            } else {
                view! { <ol>{value.events.into_iter().map(|event| {
                    let label = match event.action {
                        LicensePlateHierarchyAction::Attached => format!("Attached to #{}", event.parent_license_plate_id.unwrap_or_default()),
                        LicensePlateHierarchyAction::Detached => format!("Detached from #{}", event.previous_parent_license_plate_id.unwrap_or_default()),
                    };
                    view! { <li><span><b>{label}</b><small>{format!("Revision {} · actor #{} · {}",event.resulting_revision,event.actor_user_id,event.occurred_at)}</small></span><p>{event.reason}</p></li> }
                }).collect_view()}</ol> }.into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate(id: i64, parent: Option<i64>, descendants: Vec<i64>) -> LicensePlate {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "tenant_id": 1,
            "inventory_owner_id": 2,
            "created": "2026-01-01T00:00:00Z",
            "deleted": null,
            "barcode": format!("LP-{id}"),
            "facility_id": 3,
            "location_id": 4,
            "dims_id": null,
            "parent_license_plate_id": parent,
            "hierarchy_revision": 0,
            "hierarchy_depth": 0,
            "root_license_plate_id": id,
            "child_license_plate_ids": [],
            "descendant_license_plate_ids": descendants,
            "contained_unit_quantity": 0,
            "hierarchy_updated_at": null,
            "hierarchy_updated_by_user_id": null,
            "contents": []
        }))
        .unwrap()
    }

    #[test]
    fn parent_options_exclude_self_descendants_and_scope_mismatches() {
        let child = plate(1, None, vec![3]);
        let same_scope = plate(2, None, Vec::new());
        let descendant = plate(3, Some(1), Vec::new());
        let mut other_location = plate(4, None, Vec::new());
        other_location.location_id = Some(99);
        assert_eq!(
            parent_candidates(
                &child,
                &[child.clone(), same_scope, descendant, other_location]
            )
            .into_iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>(),
            vec![2]
        );
    }
}
