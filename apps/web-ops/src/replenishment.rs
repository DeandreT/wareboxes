use leptos::prelude::*;
use lucide_leptos::X;
use wareboxes_api_contract::v1::{
    OpaqueCursor, PlanReplenishmentResponse, ReplenishmentPolicyPage, ReplenishmentPolicySort,
    ReplenishmentPolicySortDirection, ReplenishmentQueuePage, ReplenishmentWorkSort,
    ReplenishmentWorkSortDirection,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::{Item, Location};

mod command_dialog;
mod model;
mod policies;
mod work_cancellation;
mod work_queue;

use command_dialog::PolicyCommandDialog;
use model::{
    item_label, location_label, planning_outcome_class, planning_outcome_label, CommandSignals,
    PolicyCommandResult, PolicyDialogMode, PolicyPageSignals, PolicySort,
    ReplenishmentReferenceData, ReplenishmentTab, ScopeFilters, WorkPageSignals, WorkSort,
};
use policies::PoliciesPanel;
use work_cancellation::WorkCancellationDialog;
use work_queue::WorkQueuePanel;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::sorting::{SortDirection, SortSpec};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

#[derive(Clone, Copy)]
struct RequestContext {
    policies: PolicyPageSignals,
    work: WorkPageSignals,
    filters: ScopeFilters,
    on_unauthorized: Callback<()>,
}

#[component]
pub(crate) fn ReplenishmentWorkspace(
    initial_policies: Option<ReplenishmentPolicyPage>,
    initial_work: Option<ReplenishmentQueuePage>,
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let policies = PolicyPageSignals::new(initial_policies);
    let work = WorkPageSignals::new(initial_work);
    let filters = ScopeFilters::new();
    let tab = RwSignal::new(ReplenishmentTab::Policies);
    let filter_error = RwSignal::new(None::<String>);
    let references = RwSignal::new(ReplenishmentReferenceData {
        access: access.clone(),
        ..ReplenishmentReferenceData::default()
    });
    let references_loading = RwSignal::new(false);
    let references_error = RwSignal::new(None::<String>);
    let references_generation = RwSignal::new(0_u64);
    let dialog = RwSignal::new(None::<PolicyDialogMode>);
    let command_pending = RwSignal::new(false);
    let command_retry = RwSignal::new(None::<model::PolicyCommandAttempt>);
    let command_error = RwSignal::new(None::<String>);
    let command_invalidated = RwSignal::new(false);
    let last_plan = RwSignal::new(None::<PlanReplenishmentResponse>);
    let cancellation_target =
        RwSignal::new(None::<wareboxes_api_contract::v1::ReplenishmentQueueEntryResponse>);
    let toasts = use_toast_bus();
    let scoped_access = StoredValue::new(access);
    let requests = RequestContext {
        policies,
        work,
        filters,
        on_unauthorized,
    };
    let authoritative_refresh = Callback::new(move |()| refresh_first_pages(requests));
    let commands = CommandSignals {
        dialog,
        pending: command_pending,
        retry: command_retry,
        error: command_error,
        invalidated: command_invalidated,
        toasts,
        on_unauthorized,
        on_authoritative_refresh: authoritative_refresh,
    };

    Effect::new(move |_| {
        if policies.page.get_untracked().is_none() {
            request_policy_page(requests, None, Vec::new());
        }
        if work.page.get_untracked().is_none() {
            request_work_page(requests, None, Vec::new());
        }
        request_references(
            references,
            references_loading,
            references_error,
            references_generation,
            on_unauthorized,
        );
    });

    #[cfg(target_arch = "wasm32")]
    install_work_poll(requests, tab, commands, cancellation_target);

    let apply_filters = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if let Err(message) = filters.validate() {
            filter_error.set(Some(message));
            return;
        }
        filter_error.set(None);
        refresh_first_pages(requests);
    };
    let refresh = move |_| {
        request_policy_page(
            requests,
            policies.current_cursor.get_untracked(),
            policies.cursor_history.get_untracked(),
        );
        request_work_page(
            requests,
            work.current_cursor.get_untracked(),
            work.cursor_history.get_untracked(),
        );
        request_references(
            references,
            references_loading,
            references_error,
            references_generation,
            on_unauthorized,
        );
    };
    let policy_previous = Callback::new(move |()| {
        if policies.loading.get_untracked() {
            return;
        }
        let mut history = policies.cursor_history.get_untracked();
        if let Some(previous) = history.pop() {
            request_policy_page(requests, previous, history);
        }
    });
    let policy_next = Callback::new(move |()| {
        if policies.loading.get_untracked() {
            return;
        }
        if let Some(next) = policies
            .page
            .get_untracked()
            .and_then(|page| page.next_cursor)
        {
            let mut history = policies.cursor_history.get_untracked();
            history.push(policies.current_cursor.get_untracked());
            request_policy_page(requests, Some(next), history);
        }
    });
    let work_previous = Callback::new(move |()| {
        if work.loading.get_untracked() {
            return;
        }
        let mut history = work.cursor_history.get_untracked();
        if let Some(previous) = history.pop() {
            request_work_page(requests, previous, history);
        }
    });
    let work_next = Callback::new(move |()| {
        if work.loading.get_untracked() {
            return;
        }
        if let Some(next) = work.page.get_untracked().and_then(|page| page.next_cursor) {
            let mut history = work.cursor_history.get_untracked();
            history.push(work.current_cursor.get_untracked());
            request_work_page(requests, Some(next), history);
        }
    });
    let work_sort = Callback::new(move |key| {
        SortSpec::select(work.sort, key);
        request_work_page(requests, None, Vec::new());
    });
    let policy_sort = Callback::new(move |key| {
        SortSpec::select(policies.sort, key);
        request_policy_page(requests, None, Vec::new());
    });
    let open_configure = Callback::new(move |policy| {
        let current_references = references.get_untracked();
        if !references_loading.get_untracked()
            && (references_error.get_untracked().is_some()
                || current_references.items.is_empty()
                || current_references.locations.is_empty())
        {
            request_references(
                references,
                references_loading,
                references_error,
                references_generation,
                on_unauthorized,
            );
        }
        command_error.set(None);
        command_invalidated.set(false);
        dialog.set(Some(PolicyDialogMode::Configure(policy)));
    });
    let open_plan = Callback::new(move |policy| {
        command_error.set(None);
        command_invalidated.set(false);
        dialog.set(Some(PolicyDialogMode::Plan(policy)));
    });
    let open_retire = Callback::new(move |policy| {
        command_error.set(None);
        command_invalidated.set(false);
        dialog.set(Some(PolicyDialogMode::Retire(policy)));
    });
    let on_success = Callback::new(move |result| {
        command_invalidated.set(false);
        match result {
            PolicyCommandResult::Configured(result) => {
                toasts.success(format!(
                    "Policy #{} saved at revision {}.",
                    result.policy_id,
                    result.revision.get()
                ));
            }
            PolicyCommandResult::Planned(result) => {
                toasts.success(plan_result_summary(&result));
                last_plan.set(Some(result));
            }
            PolicyCommandResult::Retired(result) => {
                toasts.success(format!("Policy #{} retired.", result.policy_id));
            }
        }
        refresh_first_pages(requests);
    });
    let commands_locked =
        Signal::derive(move || command_pending.get() || command_retry.get().is_some());
    let open_cancellation = Callback::new(move |work| cancellation_target.set(Some(work)));
    let cancellation_refresh = Callback::new(move |()| refresh_first_pages(requests));
    let cancellation_complete = Callback::new(move |_| {
        cancellation_target.set(None);
        refresh_first_pages(requests);
    });

    view! {
        <section class="page-heading replenishment-page-heading">
            <div>
                <p class="eyebrow">"Inventory flow"</p>
                <h1>"Replenishment"</h1>
                <p>"Live pick-face readiness, explicit planning, and execution work."</p>
            </div>
            <button
                type="button"
                class="button secondary-action replenishment-refresh"
                title="Refresh policies and work"
                disabled=move || policies.loading.get() || work.loading.get()
                on:click=refresh
            >
                <Icon icon=UiIcon::Refresh/>
                <span>"Refresh"</span>
            </button>
        </section>

        <form class="replenishment-toolbar" on:submit=apply_filters>
            <div class="segmented-control replenishment-tabs" role="tablist" aria-label="Replenishment views">
                <button type="button" role="tab" class:active=move || tab.get() == ReplenishmentTab::Policies aria-selected=move || (tab.get() == ReplenishmentTab::Policies).to_string() on:click=move |_| tab.set(ReplenishmentTab::Policies)>"Policies"</button>
                <button type="button" role="tab" class:active=move || tab.get() == ReplenishmentTab::Work aria-selected=move || (tab.get() == ReplenishmentTab::Work).to_string() on:click=move |_| tab.set(ReplenishmentTab::Work)>"Work queue"</button>
            </div>
            <div class="replenishment-filter-fields">
                <label>
                    <span class="sr-only">"Facility"</span>
                    <select aria-label="Facility" prop:value=move || filters.facility_id.get() on:change=move |event| {
                        filters.facility_id.set(event_target_value(&event));
                        filters.pick_face_location_id.set(String::new());
                    }>
                        <option value="">"All facilities"</option>
                        {scoped_access.get_value().facilities.into_iter().map(|facility| view! { <option value=facility.id.to_string()>{facility.name}</option> }).collect_view()}
                    </select>
                </label>
                <label>
                    <span class="sr-only">"Client"</span>
                    <select aria-label="Client" prop:value=move || filters.inventory_owner_id.get() on:change=move |event| filters.inventory_owner_id.set(event_target_value(&event))>
                        <option value="">"All clients"</option>
                        {scoped_access.get_value().inventory_owners.into_iter().map(|owner| view! { <option value=owner.id.to_string()>{owner.name}</option> }).collect_view()}
                    </select>
                </label>
                <label>
                    <span class="sr-only">"Item"</span>
                    <select
                        aria-label="Item"
                        disabled=move || references_loading.get()
                        prop:value=move || filters.item_id.get()
                        on:change=move |event| filters.item_id.set(event_target_value(&event))
                    >
                        <option value="">{move || if references_loading.get() { "Loading items" } else { "All items" }}</option>
                        {move || sorted_filter_items(references.get().items).into_iter().map(|item| {
                            let item_id = item.id;
                            view! { <option value=item_id.to_string()>{item_label(&item)}</option> }
                        }).collect_view()}
                    </select>
                </label>
                <label>
                    <span class="sr-only">"Pick face"</span>
                    <select
                        aria-label="Pick face"
                        disabled=move || references_loading.get()
                        prop:value=move || filters.pick_face_location_id.get()
                        on:change=move |event| filters.pick_face_location_id.set(event_target_value(&event))
                    >
                        <option value="">{move || if references_loading.get() { "Loading pick faces" } else { "All pick faces" }}</option>
                        {move || eligible_filter_pick_faces(
                            references.get().locations,
                            filters.facility_id.get().parse::<i64>().ok().filter(|value| *value > 0),
                        ).into_iter().map(|location| {
                            let location_id = location.id;
                            view! { <option value=location_id.to_string()>{pick_face_filter_label(&location)}</option> }
                        }).collect_view()}
                    </select>
                </label>
                <Show when=move || tab.get() == ReplenishmentTab::Work>
                    <label>
                        <span class="sr-only">"Work status"</span>
                        <select aria-label="Work status" prop:value=move || filters.work_status.get() on:change=move |event| filters.work_status.set(event_target_value(&event))>
                            <option value="">"Open work"</option>
                            <option value="pending">"Pending"</option>
                            <option value="claimed">"Claimed"</option>
                            <option value="completed">"Completed"</option>
                            <option value="cancelled">"Cancelled"</option>
                        </select>
                    </label>
                </Show>
                <button type="submit" class="button secondary-action" disabled=move || policies.loading.get() || work.loading.get()>"Apply"</button>
            </div>
        </form>
        <Show when=move || filter_error.get().is_some()>
            <p class="inline-command-error replenishment-filter-error" role="alert">{move || filter_error.get().unwrap_or_default()}</p>
        </Show>
        <Show when=move || references_error.get().is_some()>
            <p class="inline-command-error replenishment-filter-error" role="alert">{move || references_error.get().unwrap_or_default()}</p>
        </Show>

        <Show when=move || last_plan.get().is_some()>
            {move || last_plan.get().map(|plan| view! {
                <PlanResultBand plan on_close=Callback::new(move |()| last_plan.set(None))/>
            })}
        </Show>

        <Show when=move || tab.get() == ReplenishmentTab::Policies>
            <PoliciesPanel
                signals=policies
                commands_locked
                on_configure=open_configure
                on_plan=open_plan
                on_retire=open_retire
                on_previous=policy_previous
                on_next=policy_next
                on_sort=policy_sort
            />
        </Show>
        <Show when=move || tab.get() == ReplenishmentTab::Work>
            <WorkQueuePanel
                signals=work
                on_previous=work_previous
                on_next=work_next
                on_cancel=open_cancellation
                on_sort=work_sort
            />
        </Show>

        <Show when=move || dialog.get().is_some()>
            {move || dialog.get().map(|mode| view! {
                <PolicyCommandDialog
                    mode
                    references
                    references_loading
                    references_error
                    signals=commands
                    on_success
                />
            })}
        </Show>
        <Show when=move || cancellation_target.get().is_some()>
            {move || cancellation_target.get().map(|work| view! {
                <WorkCancellationDialog
                    work
                    on_close=Callback::new(move |()| cancellation_target.set(None))
                    on_cancelled=cancellation_complete
                    on_authoritative_refresh=cancellation_refresh
                    on_unauthorized
                />
            })}
        </Show>
    }
}

fn sorted_filter_items(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by_cached_key(|item| item_label(item).to_ascii_lowercase());
    items
}

fn eligible_filter_pick_faces(
    mut locations: Vec<Location>,
    facility_id: Option<i64>,
) -> Vec<Location> {
    locations.retain(|location| {
        location.deleted.is_none()
            && location.active
            && location.pickable
            && !location.receivable
            && location.barcode.is_some()
            && facility_id.is_none_or(|facility_id| location.facility_id == facility_id)
    });
    locations.sort_by_cached_key(|location| pick_face_filter_label(location).to_ascii_lowercase());
    locations
}

fn pick_face_filter_label(location: &Location) -> String {
    location.facility_name.as_ref().map_or_else(
        || location_label(location),
        |facility| format!("{facility} / {}", location_label(location)),
    )
}

#[component]
fn PlanResultBand(plan: PlanReplenishmentResponse, on_close: Callback<()>) -> impl IntoView {
    view! {
        <section class="replenishment-plan-result" role="status">
            <span class=planning_outcome_class(plan.outcome)>{planning_outcome_label(plan.outcome)}</span>
            <div>
                <strong>{format!("Plan #{} / Policy #{}", plan.plan_id, plan.policy_id)}</strong>
                <span>{format!(
                    "{} {} planned across {} tasks; {} remains. Projected free was {} with {} inbound.",
                    format_quantity(plan.planned_quantity),
                    plan.uom,
                    plan.work.len(),
                    format_quantity(plan.remaining_quantity),
                    format_quantity(plan.snapshot.projected_free),
                    format_quantity(plan.snapshot.active_inbound),
                )}</span>
            </div>
            <button type="button" title="Dismiss plan result" aria-label="Dismiss plan result" on:click=move |_| on_close.run(())><X size=15/></button>
        </section>
    }
}

fn refresh_first_pages(context: RequestContext) {
    request_policy_page(context, None, Vec::new());
    request_work_page(context, None, Vec::new());
}

fn request_policy_page(
    context: RequestContext,
    cursor: Option<OpaqueCursor>,
    history: Vec<Option<OpaqueCursor>>,
) {
    context
        .policies
        .generation
        .update(|generation| *generation = generation.saturating_add(1));
    let generation = context.policies.generation.get_untracked();
    let sort = context.policies.sort.get_untracked();
    context.policies.loading.set(true);
    context.policies.error.set(None);
    leptos::task::spawn_local(async move {
        let response = api::replenishment_policies(
            api::ReplenishmentPolicyFilters {
                facility_id: context.filters.facility(),
                inventory_owner_id: context.filters.owner(),
                item_id: context.filters.item(),
                pick_face_location_id: context.filters.pick_face(),
                sort: map_policy_sort(sort.key),
                direction: map_policy_sort_direction(sort.direction),
            },
            cursor.as_ref(),
        )
        .await;
        if context.policies.generation.get_untracked() != generation {
            return;
        }
        context.policies.loading.set(false);
        match response {
            Ok(page) => {
                context.policies.current_cursor.set(cursor);
                context.policies.cursor_history.set(history);
                context.policies.page.set(Some(page));
            }
            Err(error) if error.unauthorized => context.on_unauthorized.run(()),
            Err(error) => context.policies.error.set(Some(error.message)),
        }
    });
}

fn request_work_page(
    context: RequestContext,
    cursor: Option<OpaqueCursor>,
    history: Vec<Option<OpaqueCursor>>,
) {
    context
        .work
        .generation
        .update(|generation| *generation = generation.saturating_add(1));
    let generation = context.work.generation.get_untracked();
    let sort = context.work.sort.get_untracked();
    context.work.loading.set(true);
    context.work.error.set(None);
    leptos::task::spawn_local(async move {
        let response = api::replenishment_queue(
            api::ReplenishmentQueueFilters {
                facility_id: context.filters.facility(),
                inventory_owner_id: context.filters.owner(),
                item_id: context.filters.item(),
                pick_face_location_id: context.filters.pick_face(),
                status: context.filters.status(),
                sort: map_work_sort(sort.key),
                direction: map_work_sort_direction(sort.direction),
            },
            cursor.as_ref(),
        )
        .await;
        if context.work.generation.get_untracked() != generation {
            return;
        }
        context.work.loading.set(false);
        match response {
            Ok(page) => {
                context.work.current_cursor.set(cursor);
                context.work.cursor_history.set(history);
                context.work.page.set(Some(page));
            }
            Err(error) if error.unauthorized => context.on_unauthorized.run(()),
            Err(error) => context.work.error.set(Some(error.message)),
        }
    });
}

const fn map_policy_sort(sort: PolicySort) -> ReplenishmentPolicySort {
    match sort {
        PolicySort::Client => ReplenishmentPolicySort::InventoryOwner,
        PolicySort::Facility => ReplenishmentPolicySort::Facility,
        PolicySort::Item => ReplenishmentPolicySort::Item,
        PolicySort::PickFace => ReplenishmentPolicySort::PickFace,
        PolicySort::Projected => ReplenishmentPolicySort::Projected,
        PolicySort::Demand => ReplenishmentPolicySort::Demand,
        PolicySort::Reserve => ReplenishmentPolicySort::Reserve,
        PolicySort::Gap => ReplenishmentPolicySort::TargetGap,
        PolicySort::Outcome => ReplenishmentPolicySort::Outcome,
        PolicySort::Work => ReplenishmentPolicySort::ActiveWork,
    }
}

const fn map_policy_sort_direction(direction: SortDirection) -> ReplenishmentPolicySortDirection {
    match direction {
        SortDirection::Ascending => ReplenishmentPolicySortDirection::Ascending,
        SortDirection::Descending => ReplenishmentPolicySortDirection::Descending,
    }
}

const fn map_work_sort(sort: WorkSort) -> ReplenishmentWorkSort {
    match sort {
        WorkSort::Created => ReplenishmentWorkSort::Created,
        WorkSort::Priority => ReplenishmentWorkSort::Priority,
        WorkSort::Client => ReplenishmentWorkSort::InventoryOwner,
        WorkSort::Facility => ReplenishmentWorkSort::Facility,
        WorkSort::Item => ReplenishmentWorkSort::Item,
        WorkSort::Source => ReplenishmentWorkSort::Source,
        WorkSort::Destination => ReplenishmentWorkSort::Destination,
        WorkSort::Quantity => ReplenishmentWorkSort::Quantity,
        WorkSort::Status => ReplenishmentWorkSort::Status,
        WorkSort::Lease => ReplenishmentWorkSort::Lease,
    }
}

const fn map_work_sort_direction(direction: SortDirection) -> ReplenishmentWorkSortDirection {
    match direction {
        SortDirection::Ascending => ReplenishmentWorkSortDirection::Ascending,
        SortDirection::Descending => ReplenishmentWorkSortDirection::Descending,
    }
}

fn request_references(
    references: RwSignal<ReplenishmentReferenceData>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    on_unauthorized: Callback<()>,
) {
    if loading.get_untracked() {
        return;
    }
    generation.update(|generation| *generation = generation.saturating_add(1));
    let request_generation = generation.get_untracked();
    loading.set(true);
    error.set(None);
    leptos::task::spawn_local(async move {
        let items = api::internal_get::<Vec<Item>>("/api/items?show_deleted=false").await;
        let locations = match items {
            Ok(items) => {
                match api::internal_get::<Vec<Location>>("/api/locations?show_deleted=false").await
                {
                    Ok(locations) => Ok((items, locations)),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        if generation.get_untracked() != request_generation {
            return;
        }
        loading.set(false);
        match locations {
            Ok((items, locations)) => references.update(|references| {
                references.items = items;
                references.locations = locations;
            }),
            Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
            Err(api_error) => error.set(Some(format!(
                "Item and location choices are unavailable: {}",
                api_error.message
            ))),
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn install_work_poll(
    context: RequestContext,
    tab: RwSignal<ReplenishmentTab>,
    commands: CommandSignals,
    cancellation_target: RwSignal<
        Option<wareboxes_api_contract::v1::ReplenishmentQueueEntryResponse>,
    >,
) {
    use std::time::Duration;

    let Some(owner) = Owner::current() else {
        return;
    };
    let Ok(handle) = set_interval_with_handle(
        move || {
            if tab.get_untracked() != ReplenishmentTab::Work
                || context.work.loading.get_untracked()
                || commands.pending.get_untracked()
                || commands.retry.get_untracked().is_some()
                || cancellation_target.get_untracked().is_some()
            {
                return;
            }
            owner.with(|| {
                request_work_page(
                    context,
                    context.work.current_cursor.get_untracked(),
                    context.work.cursor_history.get_untracked(),
                );
            });
        },
        Duration::from_secs(15),
    ) else {
        return;
    };
    on_cleanup(move || handle.clear());
}

fn plan_result_summary(plan: &PlanReplenishmentResponse) -> String {
    format!(
        "Plan #{}: {} {} planned, {} remaining ({}).",
        plan.plan_id,
        format_quantity(plan.planned_quantity),
        plan.uom,
        format_quantity(plan.remaining_quantity),
        planning_outcome_label(plan.outcome).to_ascii_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{
        ReplenishmentPlanningOutcome, ReplenishmentPlanningSnapshotResponse, Revision,
    };

    #[test]
    fn plan_summary_keeps_zero_and_partial_outcomes_explicit() {
        let mut plan = plan_result(ReplenishmentPlanningOutcome::InsufficientReserve, 0, 12);
        assert!(plan_result_summary(&plan).contains("0 each planned, 12 remaining (no reserve)"));
        plan.outcome = ReplenishmentPlanningOutcome::PartiallyPlanned;
        plan.planned_quantity = 8;
        plan.remaining_quantity = 4;
        assert!(plan_result_summary(&plan).contains("8 each planned, 4 remaining (partial)"));
    }

    #[test]
    fn pick_face_filter_uses_current_facility_and_execution_eligibility() {
        let choices = eligible_filter_pick_faces(
            vec![
                location(1, 10, true, true, false, true),
                location(2, 10, true, false, false, true),
                location(3, 20, true, true, false, true),
                location(4, 10, false, true, false, true),
                location(5, 10, true, true, true, true),
                location(6, 10, true, true, false, false),
            ],
            Some(10),
        );

        assert_eq!(
            choices
                .iter()
                .map(|location| location.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            pick_face_filter_label(&choices[0]),
            "Facility 10 / Pick face 1 / PF-1"
        );
    }

    fn plan_result(
        outcome: ReplenishmentPlanningOutcome,
        planned_quantity: i64,
        remaining_quantity: i64,
    ) -> PlanReplenishmentResponse {
        PlanReplenishmentResponse {
            plan_id: 5,
            policy_id: 6,
            policy_revision: Revision::new(2).unwrap(),
            inventory_owner_id: 7,
            facility_id: 8,
            item_id: 9,
            uom: "each".into(),
            pick_face_location_id: 10,
            snapshot: ReplenishmentPlanningSnapshotResponse {
                pick_face_free: 1,
                active_inbound: 2,
                projected_free: 3,
                unallocated_demand: 4,
                reserve_free: planned_quantity,
            },
            required_level: 15,
            target_gap: 12,
            planned_quantity,
            remaining_quantity,
            outcome,
            work: Vec::new(),
            planned_by: 11,
            planned_at: "2026-08-08T12:00:00Z".into(),
        }
    }

    fn location(
        id: i64,
        facility_id: i64,
        active: bool,
        pickable: bool,
        receivable: bool,
        scannable: bool,
    ) -> Location {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "tenant_id": 1,
            "created": "2026-08-10T12:00:00Z",
            "deleted": null,
            "facility_id": facility_id,
            "facility_name": format!("Facility {facility_id}"),
            "parent_location_id": null,
            "barcode": scannable.then(|| format!("PF-{id}")),
            "name": format!("Pick face {id}"),
            "type": "pick_face",
            "active": active,
            "pickable": pickable,
            "receivable": receivable
        }))
        .unwrap()
    }
}
