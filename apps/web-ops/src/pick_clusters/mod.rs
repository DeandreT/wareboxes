mod view;

use leptos::prelude::*;
use lucide_leptos::{Boxes, Plus, RefreshCw, RotateCcw, X};
use wareboxes_api_contract::v1::{
    CancelPickClusterRequest, ChangePickCartStatusRequest, CreatePickCartRequest, PickCartStatus,
    PickClusterResponse, PickClusterStatus, PickClusterTaskAssignmentRequest,
    PickClusterWorkspaceResponse, PickRouteMode, PlanPickClusterRequest,
};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};

use crate::api;
use crate::toast::{use_toast_bus, ToastBus};

use self::view::{cart_status_label, cluster_status_label, next_cart_status};

#[derive(Clone, Copy)]
struct Signals {
    workspace: RwSignal<Option<PickClusterWorkspaceResponse>>,
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    include_history: RwSignal<bool>,
    loading: RwSignal<bool>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    selected_cart_id: RwSignal<Option<i64>>,
    assignments: RwSignal<Vec<(i64, i64)>>,
    show_create_cart: RwSignal<bool>,
    retire_cart: RwSignal<Option<(i64, i64, String)>>,
    retry: RwSignal<Option<SavedCommand>>,
}

#[derive(Clone, Copy)]
struct Drafts {
    cart_barcode: RwSignal<String>,
    cart_name: RwSignal<String>,
    cart_slots: RwSignal<String>,
    cancellation_note: RwSignal<String>,
}

#[derive(Clone)]
enum SavedCommand {
    CreateCart(CreatePickCartRequest, String),
    ChangeCart(i64, ChangePickCartStatusRequest, String),
    Plan(PlanPickClusterRequest, String),
    Cancel(i64, CancelPickClusterRequest, String),
}

#[component]
pub(crate) fn PickClustersWorkspace(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let default_facility = single_id(&access.facilities);
    let default_owner = single_id(&access.inventory_owners);
    let facilities = StoredValue::new(access.facilities);
    let owners = StoredValue::new(access.inventory_owners);
    let signals = Signals {
        workspace: RwSignal::new(None),
        facility_id: RwSignal::new(default_facility),
        owner_id: RwSignal::new(default_owner),
        include_history: RwSignal::new(false),
        loading: RwSignal::new(true),
        pending: RwSignal::new(false),
        error: RwSignal::new(None),
        generation: RwSignal::new(0),
        selected_cart_id: RwSignal::new(None),
        assignments: RwSignal::new(Vec::new()),
        show_create_cart: RwSignal::new(false),
        retire_cart: RwSignal::new(None),
        retry: RwSignal::new(None),
    };
    let drafts = Drafts {
        cart_barcode: RwSignal::new(String::new()),
        cart_name: RwSignal::new(String::new()),
        cart_slots: RwSignal::new("A, B, C, D".into()),
        cancellation_note: RwSignal::new(String::new()),
    };
    let toasts = use_toast_bus();
    Effect::new(move |_| request_workspace(signals, on_unauthorized));

    let refresh = move |_| request_workspace(signals, on_unauthorized);
    let create_cart = move |_| {
        let Some(facility_id) = signals.facility_id.get_untracked() else {
            signals.error.set(Some("Select a facility first.".into()));
            return;
        };
        let slot_codes = drafts
            .cart_slots
            .get_untracked()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let request = CreatePickCartRequest {
            facility_id,
            barcode: drafts.cart_barcode.get_untracked().trim().to_owned(),
            name: drafts.cart_name.get_untracked().trim().to_owned(),
            slot_codes,
        };
        if request.barcode.is_empty() || request.name.is_empty() || request.slot_codes.len() < 2 {
            signals.error.set(Some(
                "Cart barcode, name, and at least two comma-separated slots are required.".into(),
            ));
            return;
        }
        dispatch(
            SavedCommand::CreateCart(request, api::new_idempotency_key()),
            signals,
            drafts,
            toasts,
            on_unauthorized,
        );
    };
    let plan = move |_| {
        let (Some(facility_id), Some(inventory_owner_id), Some(cart_id)) = (
            signals.facility_id.get_untracked(),
            signals.owner_id.get_untracked(),
            signals.selected_cart_id.get_untracked(),
        ) else {
            signals
                .error
                .set(Some("Select facility, client, and an active cart.".into()));
            return;
        };
        let assignments = signals
            .assignments
            .get_untracked()
            .into_iter()
            .filter(|(_, slot_id)| *slot_id > 0)
            .map(|(task_id, slot_id)| PickClusterTaskAssignmentRequest { task_id, slot_id })
            .collect::<Vec<_>>();
        let workspace = signals.workspace.get_untracked();
        let order_count = workspace
            .as_ref()
            .map(|workspace| selected_order_count(workspace, &assignments))
            .unwrap_or_default();
        if assignments.len() < 2 || order_count < 2 {
            signals.error.set(Some(
                "Assign at least two tasks from two different orders to cart slots.".into(),
            ));
            return;
        }
        if !workspace
            .as_ref()
            .is_some_and(|workspace| assignments_are_slot_consistent(workspace, &assignments))
        {
            signals.error.set(Some(
                "Use one cart slot per order and do not mix different orders in one slot.".into(),
            ));
            return;
        }
        dispatch(
            SavedCommand::Plan(
                PlanPickClusterRequest {
                    inventory_owner_id,
                    facility_id,
                    cart_id,
                    assignments,
                },
                api::new_idempotency_key(),
            ),
            signals,
            drafts,
            toasts,
            on_unauthorized,
        );
    };

    view! {
        <section class="pick-cluster-panel data-section">
            <header class="pick-cluster-heading">
                <div><span class="eyebrow">"Multi-order execution"</span><h2><Boxes size=18/>"Cart picking"</h2><p>"Combine homogeneous work into batch routes and bind every order to a physical cart slot."</p></div>
                <div class="page-actions"><button type="button" class="icon-button" title="Refresh clusters" disabled=move || signals.loading.get() on:click=refresh><RefreshCw size=15/></button><button type="button" class="button secondary-action" on:click=move |_| signals.show_create_cart.set(true)><Plus size=14/>"New cart"</button></div>
            </header>
            <div class="pick-cluster-toolbar">
                <label><span>"Facility"</span><select prop:value=move || optional_value(signals.facility_id.get()) on:change=move |event| { signals.facility_id.set(parse_id(&event_target_value(&event))); signals.selected_cart_id.set(None); request_workspace(signals,on_unauthorized); }><option value="">"Select facility"</option>{resource_options(facilities)}</select></label>
                <label><span>"Client"</span><select prop:value=move || optional_value(signals.owner_id.get()) on:change=move |event| { signals.owner_id.set(parse_id(&event_target_value(&event))); request_workspace(signals,on_unauthorized); }><option value="">"Select client"</option>{resource_options(owners)}</select></label>
                <label class="checkbox-label"><input type="checkbox" checked=move || signals.include_history.get() on:change=move |event| { signals.include_history.set(event_target_checked(&event)); request_workspace(signals,on_unauthorized); }/><span>"History"</span></label>
            </div>
            <Show when=move || signals.retry.get().is_some()><div class="pick-cluster-retry"><span>"The last command outcome is unknown."</span><button type="button" class="button secondary-action" on:click=move |_| { if let Some(command)=signals.retry.get_untracked() { dispatch(command,signals,drafts,toasts,on_unauthorized); } }><RotateCcw size=14/>"Retry exact command"</button></div></Show>
            <Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show>
            {move || render_workspace(signals,drafts,toasts,on_unauthorized,Callback::new(plan))}
        </section>
        <Show when=move || signals.show_create_cart.get()><div class="pick-cluster-dialog-backdrop"><section class="pick-cluster-dialog" role="dialog" aria-modal="true"><header><div><span class="eyebrow">"Physical equipment"</span><h2>"Create pick cart"</h2></div><button type="button" class="icon-button" on:click=move |_| signals.show_create_cart.set(false)><X size=16/></button></header><fieldset disabled=move || signals.pending.get()><label><span>"Cart barcode"</span><input maxlength="80" prop:value=move || drafts.cart_barcode.get() on:input=move |event| drafts.cart_barcode.set(event_target_value(&event))/></label><label><span>"Display name"</span><input maxlength="120" prop:value=move || drafts.cart_name.get() on:input=move |event| drafts.cart_name.set(event_target_value(&event))/></label><label><span>"Slot codes"</span><input prop:value=move || drafts.cart_slots.get() on:input=move |event| drafts.cart_slots.set(event_target_value(&event))/><small>"Comma-separated; slots are immutable after creation."</small></label></fieldset><footer><button type="button" class="button secondary-action" on:click=move |_| signals.show_create_cart.set(false)>"Back"</button><button type="button" class="button primary-action" disabled=move || signals.pending.get() on:click=create_cart>"Create cart"</button></footer></section></div></Show>
        <Show when=move || signals.retire_cart.get().is_some()><div class="pick-cluster-dialog-backdrop"><section class="pick-cluster-dialog" role="alertdialog" aria-modal="true"><header><div><span class="eyebrow">"Irreversible equipment change"</span><h2>"Retire pick cart"</h2></div><button type="button" class="icon-button" on:click=move |_| signals.retire_cart.set(None)><X size=16/></button></header><p>{move || signals.retire_cart.get().map(|(_,_,name)| format!("Retire {name}? It cannot be returned to service.")).unwrap_or_default()}</p><footer><button type="button" class="button secondary-action" on:click=move |_| signals.retire_cart.set(None)>"Keep cart"</button><button type="button" class="button danger-action" disabled=move || signals.pending.get() on:click=move |_| { if let Some((cart_id,expected_revision,_))=signals.retire_cart.get_untracked() { dispatch(SavedCommand::ChangeCart(cart_id,ChangePickCartStatusRequest{expected_revision,status:PickCartStatus::Retired},api::new_idempotency_key()),signals,drafts,toasts,on_unauthorized); } }>"Retire permanently"</button></footer></section></div></Show>
    }
}

fn render_workspace(
    signals: Signals,
    drafts: Drafts,
    toasts: ToastBus,
    on_unauthorized: Callback<()>,
    plan: Callback<()>,
) -> AnyView {
    if signals.loading.get() {
        return view! { <div class="workspace-state compact"><h3>"Loading cluster workspace"</h3></div> }.into_any();
    }
    let Some(workspace) = signals.workspace.get() else {
        if signals.error.get().is_some() {
            return view! { <div class="workspace-empty"><h3>"Cart workspace unavailable"</h3><p>"Review the error above, then refresh this workspace."</p></div> }.into_any();
        }
        return view! { <div class="workspace-empty"><h3>"Select an exact scope"</h3><p>"Choose one facility and one client to load eligible released tasks."</p></div> }.into_any();
    };
    let active_carts = workspace
        .carts
        .iter()
        .filter(|cart| cart.status == PickCartStatus::Active)
        .cloned()
        .collect::<Vec<_>>();
    let selected_cart = signals.selected_cart_id.get().and_then(|id| {
        workspace
            .carts
            .iter()
            .find(|cart| cart.cart_id == id)
            .cloned()
    });
    let batch_summary = selected_batch_summary(&workspace, &signals.assignments.get());
    view! {
        <div class="pick-cluster-grid">
            <section><header><h3>"Carts"</h3><span>{workspace.carts.len()}</span></header><div class="pick-cluster-card-list">{workspace.carts.into_iter().map(|cart| {
                let toggle=cart.clone();
                let retirement=cart.clone();
                view! { <article class:selected=move || signals.selected_cart_id.get()==Some(cart.cart_id)><button type="button" class="pick-cluster-card-main" on:click=move |_| { if cart.status==PickCartStatus::Active { signals.selected_cart_id.set(Some(cart.cart_id)); signals.assignments.set(Vec::new()); } }><strong>{cart.name.clone()}</strong><span>{cart.barcode.clone()}</span><small>{format!("{} slots · {}",cart.slots.len(),cart_status_label(cart.status))}</small></button><div class="pick-cluster-cart-actions">{next_cart_status(toggle.status).map(|status| { let request=ChangePickCartStatusRequest{expected_revision:toggle.revision,status}; let label=if status==PickCartStatus::Active {"Return to service"} else {"Take out of service"}; view! { <button type="button" class="link-button" disabled=move || signals.pending.get() on:click=move |_| dispatch(SavedCommand::ChangeCart(toggle.cart_id,request,api::new_idempotency_key()),signals,drafts,toasts,on_unauthorized)>{label}</button> } })}{(retirement.status!=PickCartStatus::Retired).then(|| view! { <button type="button" class="link-button pick-cluster-retire" disabled=move || signals.pending.get() on:click=move |_| signals.retire_cart.set(Some((retirement.cart_id,retirement.revision,retirement.name.clone())))>"Retire"</button> })}</div></article> }
            }).collect_view()}</div></section>
            <section><header><h3>"Eligible tasks"</h3><span>{workspace.candidates.len()}</span></header>{if let Some(cart)=selected_cart { let plan_label=if batch_summary.is_some() {"Plan batch"} else {"Plan cluster"}; view! { <div class="pick-cluster-plan-head"><div><strong>{format!("Route on {}",cart.barcode)}</strong>{batch_summary.clone().map(|summary| view! { <small class="cell-detail">{summary}</small> })}</div><button type="button" class="button primary-action" disabled=move || signals.pending.get() on:click=move |_| plan.run(())>{plan_label}</button></div><div class="table-scroll"><table class="data-table"><thead><tr><th>"Order / task"</th><th>"Source"</th><th>"Item"</th><th>"Slot"</th></tr></thead><tbody>{workspace.candidates.into_iter().map(|candidate| { let task_id=candidate.task_id; let slots=cart.slots.clone(); view! { <tr><td><strong>{candidate.order_key}</strong><small class="cell-detail">{format!("Task #{}",candidate.task_id)}</small></td><td>{candidate.source_location_barcode}<small class="cell-detail">{format!("Balance #{}",candidate.source_inventory_balance_id)}</small></td><td><strong>{candidate.item_description}</strong><small class="cell-detail">{format!("{} {} · batch #{} · {}",candidate.planned_quantity,candidate.uom,candidate.item_batch_id,candidate.inventory_status)}</small></td><td><select aria-label=format!("Cart slot for task {}",task_id) on:change=move |event| set_assignment(signals.assignments,task_id,parse_id(&event_target_value(&event)).unwrap_or(0))><option value="">"Not assigned"</option>{slots.into_iter().map(|slot| view! { <option value=slot.slot_id>{slot.code}</option> }).collect_view()}</select></td></tr> }}).collect_view()}</tbody></table></div> }.into_any() } else if active_carts.is_empty() { view! { <div class="workspace-empty"><p>"Create or reactivate a cart before planning."</p></div> }.into_any() } else { view! { <div class="workspace-empty"><p>"Select an active cart to assign tasks."</p></div> }.into_any() }}</section>
            <section class="pick-cluster-runs"><header><h3>"Routes"</h3><span>{workspace.clusters.len()}</span></header><div class="pick-cluster-card-list">{workspace.clusters.into_iter().map(|cluster| render_cluster(cluster,signals,drafts,toasts,on_unauthorized)).collect_view()}</div></section>
        </div>
    }.into_any()
}

fn render_cluster(
    cluster: PickClusterResponse,
    signals: Signals,
    drafts: Drafts,
    toasts: ToastBus,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let cancel = cluster.clone();
    let mode = match cluster.mode {
        PickRouteMode::ClusterCart => "Cluster cart".to_owned(),
        PickRouteMode::BatchCart => format!(
            "Batch · {} {} from {} · balance #{} · batch #{} · {}",
            cluster.batch_total_quantity.unwrap_or_default(),
            cluster.batch_uom.as_deref().unwrap_or("?"),
            cluster
                .batch_source_location_barcode
                .as_deref()
                .unwrap_or("?"),
            cluster
                .batch_source_inventory_balance_id
                .unwrap_or_default(),
            cluster.batch_item_batch_id.unwrap_or_default(),
            cluster.batch_inventory_status.as_deref().unwrap_or("?")
        ),
    };
    view! { <article class="pick-cluster-run"><header><div><strong>{format!("Route #{} · {}",cluster.cluster_id,cluster.cart_barcode)}</strong><small>{format!("{} · {} · rev {}",mode,cluster_status_label(cluster.status),cluster.revision)}</small></div><span>{format!("{}/{}",cluster.completed_task_count,cluster.task_count)}</span></header><div class="pick-cluster-route">{cluster.members.iter().map(|member| view! { <span><b>{member.sequence}</b><strong>{member.source_location_barcode.clone()}</strong><small>{format!("{} → slot {}",member.order_key,member.slot_code)}</small></span> }).collect_view()}</div>{matches!(cancel.status,PickClusterStatus::Planned|PickClusterStatus::InProgress).then(|| view! { <footer><input maxlength="500" placeholder="Cancellation note" prop:value=move || drafts.cancellation_note.get() on:input=move |event| drafts.cancellation_note.set(event_target_value(&event))/><button type="button" class="button danger-action" disabled=move || signals.pending.get() on:click=move |_| { let note=drafts.cancellation_note.get_untracked().trim().to_owned(); if note.is_empty() { signals.error.set(Some("A cancellation note is required.".into())); } else { dispatch(SavedCommand::Cancel(cancel.cluster_id,CancelPickClusterRequest{expected_revision:cancel.revision,note},api::new_idempotency_key()),signals,drafts,toasts,on_unauthorized); } }>"Cancel"</button></footer> })}</article> }
}

fn selected_batch_summary(
    workspace: &PickClusterWorkspaceResponse,
    assignments: &[(i64, i64)],
) -> Option<String> {
    let selected = workspace
        .candidates
        .iter()
        .filter(|candidate| {
            assignments
                .iter()
                .any(|(task_id, _)| *task_id == candidate.task_id)
        })
        .collect::<Vec<_>>();
    let first = *selected.first()?;
    if selected.len() < 2
        || !selected.iter().all(|candidate| {
            candidate.source_inventory_balance_id == first.source_inventory_balance_id
                && candidate.source_location_id == first.source_location_id
                && candidate.item_batch_id == first.item_batch_id
                && candidate.uom == first.uom
                && candidate.inventory_status == first.inventory_status
        })
    {
        return None;
    }
    let total = selected.iter().try_fold(0_i64, |total, candidate| {
        total.checked_add(candidate.planned_quantity)
    })?;
    Some(format!(
        "Batch route · {total} {} from {} · batch #{}",
        first.uom, first.source_location_barcode, first.item_batch_id
    ))
}

fn request_workspace(signals: Signals, on_unauthorized: Callback<()>) {
    let generation = signals.generation.get_untracked().saturating_add(1);
    signals.generation.set(generation);
    signals.assignments.set(Vec::new());
    signals.retire_cart.set(None);
    signals.error.set(None);
    let (Some(facility_id), Some(owner_id)) = (
        signals.facility_id.get_untracked(),
        signals.owner_id.get_untracked(),
    ) else {
        signals.workspace.set(None);
        signals.loading.set(false);
        return;
    };
    signals.loading.set(true);
    let history = signals.include_history.get_untracked();
    leptos::task::spawn_local(async move {
        match api::pick_cluster_workspace(facility_id, owner_id, history).await {
            Ok(workspace) if signals.generation.get_untracked() == generation => {
                signals.workspace.set(Some(workspace));
                signals.loading.set(false);
            }
            Ok(_) => {}
            Err(_) if signals.generation.get_untracked() != generation => {}
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => {
                signals.error.set(Some(error.message));
                signals.loading.set(false);
            }
        }
    });
}

fn dispatch(
    command: SavedCommand,
    signals: Signals,
    drafts: Drafts,
    toasts: ToastBus,
    on_unauthorized: Callback<()>,
) {
    if signals.pending.get_untracked() {
        return;
    }
    signals.pending.set(true);
    signals.error.set(None);
    signals.retry.set(None);
    let retry = command.clone();
    leptos::task::spawn_local(async move {
        let result = match &command {
            SavedCommand::CreateCart(request, key) => api::create_pick_cart(request, key)
                .await
                .map(|_| "Cart created"),
            SavedCommand::ChangeCart(id, request, key) => {
                api::change_pick_cart_status(*id, request, key)
                    .await
                    .map(|_| "Cart status changed")
            }
            SavedCommand::Plan(request, key) => api::plan_pick_cluster(request, key)
                .await
                .map(|_| "Cluster route planned"),
            SavedCommand::Cancel(id, request, key) => api::cancel_pick_cluster(*id, request, key)
                .await
                .map(|_| "Cluster route cancelled"),
        };
        match result {
            Ok(message) => {
                signals.pending.set(false);
                signals.show_create_cart.set(false);
                signals.retire_cart.set(None);
                drafts.cart_barcode.set(String::new());
                drafts.cart_name.set(String::new());
                drafts.cancellation_note.set(String::new());
                toasts.success(message);
                request_workspace(signals, on_unauthorized);
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => {
                signals.pending.set(false);
                if error.ambiguous_outcome {
                    signals.retry.set(Some(retry));
                }
                signals.error.set(Some(error.message.clone()));
                toasts.error(error.message);
            }
        }
    });
}

fn set_assignment(assignments: RwSignal<Vec<(i64, i64)>>, task_id: i64, slot_id: i64) {
    assignments.update(|items| {
        items.retain(|(id, _)| *id != task_id);
        if slot_id > 0 {
            items.push((task_id, slot_id));
        }
    });
}

fn selected_order_count(
    workspace: &PickClusterWorkspaceResponse,
    assignments: &[PickClusterTaskAssignmentRequest],
) -> usize {
    workspace
        .candidates
        .iter()
        .filter(|candidate| {
            assignments
                .iter()
                .any(|assignment| assignment.task_id == candidate.task_id)
        })
        .map(|candidate| candidate.order_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

fn assignments_are_slot_consistent(
    workspace: &PickClusterWorkspaceResponse,
    assignments: &[PickClusterTaskAssignmentRequest],
) -> bool {
    let task_orders = workspace
        .candidates
        .iter()
        .map(|candidate| (candidate.task_id, candidate.order_id))
        .collect::<std::collections::BTreeMap<_, _>>();
    let pairs = assignments
        .iter()
        .map(|assignment| {
            task_orders
                .get(&assignment.task_id)
                .copied()
                .map(|order_id| (order_id, assignment.slot_id))
        })
        .collect::<Option<Vec<_>>>();
    pairs.is_some_and(|pairs| order_slot_pairs_are_consistent(&pairs))
}

fn order_slot_pairs_are_consistent(pairs: &[(i64, i64)]) -> bool {
    let mut order_slots = std::collections::BTreeMap::new();
    let mut slot_orders = std::collections::BTreeMap::new();
    pairs.iter().all(|(order_id, slot_id)| {
        order_slots
            .insert(*order_id, *slot_id)
            .is_none_or(|existing_slot_id| existing_slot_id == *slot_id)
            && slot_orders
                .insert(*slot_id, *order_id)
                .is_none_or(|existing_order_id| existing_order_id == *order_id)
    })
}
fn single_id(resources: &[AccessScopeResource]) -> Option<i64> {
    (resources.len() == 1).then(|| resources[0].id)
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}
fn optional_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}
fn resource_options(resources: StoredValue<Vec<AccessScopeResource>>) -> impl IntoView {
    resources.with_value(|items| {
        items
            .iter()
            .map(|item| view! { <option value=item.id>{item.name.clone()}</option> })
            .collect_view()
    })
}

#[cfg(test)]
mod tests {
    use super::{order_slot_pairs_are_consistent, selected_batch_summary};
    use wareboxes_api_contract::v1::{PickClusterCandidateResponse, PickClusterWorkspaceResponse};

    fn candidate(
        task_id: i64,
        source_location_id: i64,
        item_batch_id: i64,
    ) -> PickClusterCandidateResponse {
        PickClusterCandidateResponse {
            task_id,
            order_id: task_id + 100,
            order_key: format!("ORDER-{task_id}"),
            source_location_id,
            source_inventory_balance_id: 40,
            source_location_barcode: format!("SOURCE-{source_location_id}"),
            source_location_name: None,
            source_travel_sequence: 1,
            item_id: 10,
            item_batch_id,
            item_description: "Widget".into(),
            uom: "each".into(),
            inventory_status: "available".into(),
            planned_quantity: task_id,
            priority: 50,
            ship_by: None,
            created_at: "2026-08-16T00:00:00Z".into(),
        }
    }

    #[test]
    fn cluster_slots_never_mix_orders_or_split_one_order() {
        assert!(order_slot_pairs_are_consistent(&[
            (10, 1),
            (10, 1),
            (20, 2)
        ]));
        assert!(!order_slot_pairs_are_consistent(&[(10, 1), (10, 2)]));
        assert!(!order_slot_pairs_are_consistent(&[(10, 1), (20, 1)]));
    }

    #[test]
    fn batch_label_requires_one_frozen_source_and_item_batch() {
        let mut workspace = PickClusterWorkspaceResponse {
            carts: Vec::new(),
            candidates: vec![candidate(2, 20, 30), candidate(3, 20, 30)],
            clusters: Vec::new(),
        };
        assert_eq!(
            selected_batch_summary(&workspace, &[(2, 1), (3, 2)]).as_deref(),
            Some("Batch route · 5 each from SOURCE-20 · batch #30")
        );
        workspace.candidates[1].item_batch_id = 31;
        assert_eq!(selected_batch_summary(&workspace, &[(2, 1), (3, 2)]), None);
    }
}
