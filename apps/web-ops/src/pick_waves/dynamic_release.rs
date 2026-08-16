use leptos::prelude::*;
use lucide_leptos::{RefreshCw, RotateCcw, Sparkle, X};
use wareboxes_api_contract::v1::{
    DynamicReleaseReadinessResponse, DynamicReleaseRunResponse, RunDynamicReleaseRequest,
};
use wareboxes_api_contract::web::access::{AccessOwnerFacility, AccessScopeWorkspace};
use wareboxes_core::models::Location;

use crate::api;
use crate::toast::use_toast_bus;

#[derive(Clone)]
struct SavedRun {
    request: RunDynamicReleaseRequest,
    key: String,
}

#[component]
pub(super) fn DynamicReleaseControl(
    access: AccessScopeWorkspace,
    locations: Vec<Location>,
    on_completed: Callback<DynamicReleaseRunResponse>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let facilities = StoredValue::new(access.facilities);
    let owners = StoredValue::new(access.inventory_owners);
    let assignments = StoredValue::new(access.owner_facilities);
    let locations = StoredValue::new(locations);
    let open = RwSignal::new(false);
    let facility_id = RwSignal::new(None::<i64>);
    let owner_id = RwSignal::new(None::<i64>);
    let destination_id = RwSignal::new(None::<i64>);
    let preview = RwSignal::new(None::<DynamicReleaseReadinessResponse>);
    let loading = RwSignal::new(false);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let preview_generation = RwSignal::new(0_u64);
    let retry = RwSignal::new(None::<SavedRun>);
    let toasts = use_toast_bus();

    let open_dialog = move |_| {
        preview_generation.update(|value| *value = value.saturating_add(1));
        facility_id.set(None);
        owner_id.set(None);
        destination_id.set(None);
        preview.set(None);
        loading.set(false);
        error.set(None);
        open.set(true);
    };
    let close_dialog = move |_| {
        if !pending.get_untracked() {
            preview_generation.update(|value| *value = value.saturating_add(1));
            loading.set(false);
            open.set(false);
        }
    };
    let load_preview = Callback::new(move |_| {
        let (Some(facility), Some(owner)) = (facility_id.get_untracked(), owner_id.get_untracked())
        else {
            error.set(Some("Select an exact facility and client scope.".into()));
            return;
        };
        let generation = preview_generation.get_untracked().saturating_add(1);
        preview_generation.set(generation);
        loading.set(true);
        preview.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::dynamic_release_readiness(facility, owner).await {
                Ok(value) if preview_generation.get_untracked() == generation => {
                    preview.set(Some(value));
                    loading.set(false);
                }
                Ok(_) => {}
                Err(value) if value.unauthorized => on_unauthorized.run(()),
                Err(value) if preview_generation.get_untracked() == generation => {
                    error.set(Some(value.message));
                    loading.set(false);
                }
                Err(_) => {}
            }
        });
    });
    let dispatch = Callback::new(move |saved: SavedRun| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        error.set(None);
        retry.set(Some(saved.clone()));
        leptos::task::spawn_local(async move {
            match api::run_dynamic_release(&saved.request, &saved.key).await {
                Ok(result) => {
                    let released = result.selected_order_count;
                    retry.set(None);
                    pending.set(false);
                    open.set(false);
                    toasts.success(format!("Dynamically released {released} order(s)."));
                    on_completed.run(result);
                }
                Err(value) if value.unauthorized => on_unauthorized.run(()),
                Err(value) => {
                    if !value.ambiguous_outcome {
                        retry.set(None);
                        load_preview.run(());
                    }
                    error.set(Some(value.message.clone()));
                    pending.set(false);
                    toasts.error(value.message);
                }
            }
        });
    });
    let submit = Callback::new(move |_| {
        let (Some(readiness), Some(destination)) =
            (preview.get_untracked(), destination_id.get_untracked())
        else {
            error.set(Some(
                "Preview the queue and select a staging destination.".into(),
            ));
            return;
        };
        if readiness.selected_order_count == 0 {
            error.set(Some(
                "No allocation-ready orders are available to release.".into(),
            ));
            return;
        }
        dispatch.run(SavedRun {
            request: RunDynamicReleaseRequest {
                facility_id: readiness.facility_id,
                inventory_owner_id: readiness.inventory_owner_id,
                destination_location_id: destination,
                expected_policy: readiness.policy.expectation(),
            },
            key: api::new_idempotency_key(),
        });
    });

    view! {
        <button type="button" class="button secondary-action" on:click=open_dialog>
            <Sparkle size=15/>"Dynamic release"
        </button>
        <Show when=move || open.get()>
            <div class="pick-wave-dialog-backdrop">
                <section class="pick-wave-dialog wide" role="dialog" aria-modal="true" aria-labelledby="dynamic-release-title">
                    <header><div><span class="eyebrow">"Policy-bound queue"</span><h2 id="dynamic-release-title">"Dynamic release"</h2></div><button type="button" class="icon-button" aria-label="Close" on:click=close_dialog><X size=16/></button></header>
                    <p>"Preview the canonical allocation-ready queue. Rush orders lead, then earliest ship-by, creation time, and order ID. The effective wave policy controls the release cap."</p>
                    <fieldset disabled=move || pending.get()>
                        <div class="pick-wave-form-grid">
                            <label><span>"Facility"</span><select prop:value=move || optional_id(facility_id.get()) on:change=move |event| { preview_generation.update(|value| *value=value.saturating_add(1)); loading.set(false); facility_id.set(parse_id(&event_target_value(&event))); owner_id.set(None); destination_id.set(None); preview.set(None); error.set(None); }><option value="">"Select facility"</option>{facilities.with_value(|items| items.iter().map(|item| view! { <option value=item.id>{item.name.clone()}</option> }).collect_view())}</select></label>
                            <label><span>"Client"</span><select prop:value=move || optional_id(owner_id.get()) on:change=move |event| { preview_generation.update(|value| *value=value.saturating_add(1)); loading.set(false); owner_id.set(parse_id(&event_target_value(&event))); preview.set(None); error.set(None); }><option value="">"Select client"</option>{move || owners.with_value(|items| items.iter().filter(|item| assignments.with_value(|links| exact_assignment(links, item.id, facility_id.get()))).map(|item| view! { <option value=item.id>{item.name.clone()}</option> }).collect_view())}</select></label>
                            <label><span>"Staging destination"</span><select prop:value=move || optional_id(destination_id.get()) on:change=move |event| destination_id.set(parse_id(&event_target_value(&event)))><option value="">"Select staging lane"</option>{move || locations.with_value(|items| items.iter().filter(|location| is_destination(location, facility_id.get())).map(|location| view! { <option value=location.id>{location.name.clone().unwrap_or_else(|| format!("Location #{}", location.id))}</option> }).collect_view())}</select></label>
                        </div>
                    </fieldset>
                    <div class="page-actions"><button type="button" class="button secondary-action" disabled=move || loading.get() || pending.get() || facility_id.get().is_none() || owner_id.get().is_none() on:click=move |_| load_preview.run(())><RefreshCw size=14/>{move || if loading.get() { "Loading queue" } else { "Preview queue" }}</button></div>
                    {move || preview.get().map(|value| view! {
                        <div class="pick-wave-detail-content">
                            <div class="pick-wave-facts"><span><small>"Eligible"</small><strong>{value.eligible_order_count}</strong></span><span><small>"Selected"</small><strong>{value.selected_order_count}</strong></span><span><small>"Deferred by cap"</small><strong>{value.deferred_order_count}</strong></span><span><small>"Policy limit"</small><strong>{value.policy.max_orders}</strong></span><span><small>"Snapshot"</small><strong>{value.input_snapshot_at.clone()}</strong></span></div>
                            <div class="table-scroll"><table class="data-table"><thead><tr><th>"Rank"</th><th>"Order"</th><th>"Priority"</th><th>"Ship by"</th><th>"Created"</th><th class="numeric">"Allocated"</th></tr></thead><tbody>{value.selected_orders.into_iter().map(|order| view! { <tr><td>{order.rank}</td><td><strong>{order.order_key}</strong><small class="cell-detail">{format!("#{} · rev {}",order.order_id,order.revision.get())}</small></td><td>{if order.rush { "Rush" } else { "Standard" }}</td><td>{order.ship_by.unwrap_or_else(|| "Not scheduled".into())}</td><td>{order.order_created_at}</td><td class="numeric">{format!("{} / {}",order.allocated_quantity,order.demand_quantity)}</td></tr> }).collect_view()}</tbody></table></div>
                        </div>
                    })}
                    <Show when=move || retry.get().is_some()><div class="pick-wave-retry" role="status"><span>"The command outcome is unknown. Retry the exact request."</span><button type="button" class="button secondary-action" disabled=move || pending.get() on:click=move |_| { if let Some(saved)=retry.get_untracked(){ dispatch.run(saved); } }><RotateCcw size=14/>"Retry exact command"</button></div></Show>
                    <Show when=move || error.get().is_some()><p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p></Show>
                    <footer><button type="button" class="button secondary-action" on:click=close_dialog>"Back"</button><button type="button" class="button primary-action" disabled=move || pending.get() || retry.get().is_some() || preview.get().is_none() || destination_id.get().is_none() on:click=move |_| submit.run(())>{move || if pending.get(){"Releasing"}else{"Release selected queue"}}</button></footer>
                </section>
            </div>
        </Show>
    }
}

fn exact_assignment(
    assignments: &[AccessOwnerFacility],
    owner_id: i64,
    facility_id: Option<i64>,
) -> bool {
    facility_id.is_some_and(|facility_id| {
        assignments
            .iter()
            .any(|item| item.inventory_owner_id == owner_id && item.facility_id == facility_id)
    })
}

fn is_destination(location: &Location, facility_id: Option<i64>) -> bool {
    facility_id == Some(location.facility_id)
        && location.active
        && !location.pickable
        && !location.receivable
        && location
            .barcode
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|value| *value > 0)
}

fn optional_id(value: Option<i64>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_owner_facility_choices_do_not_cross_product_scopes() {
        let assignments = vec![AccessOwnerFacility {
            inventory_owner_id: 23,
            facility_id: 17,
        }];
        assert!(exact_assignment(&assignments, 23, Some(17)));
        assert!(!exact_assignment(&assignments, 23, Some(18)));
        assert!(!exact_assignment(&assignments, 24, Some(17)));
    }
}
