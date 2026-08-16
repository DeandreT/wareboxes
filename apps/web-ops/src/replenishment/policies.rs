use leptos::prelude::*;
use lucide_leptos::{ArchiveX, Pencil, Play, Plus};
use wareboxes_api_contract::v1::{
    ReplenishmentDecisionPolicyResponse, ReplenishmentDecisionPolicySource,
    ReplenishmentPlanningOutcome, ReplenishmentPolicyReadinessEntryResponse,
};

use super::model::{
    compact_timestamp, planning_outcome_class, planning_outcome_label, PolicyPageSignals,
    PolicySort,
};
use crate::sorting::SortableHeader;
use crate::view_model::format_quantity;

#[component]
pub(super) fn PoliciesPanel(
    signals: PolicyPageSignals,
    commands_locked: Signal<bool>,
    on_configure: Callback<Option<ReplenishmentPolicyReadinessEntryResponse>>,
    on_plan: Callback<ReplenishmentPolicyReadinessEntryResponse>,
    on_retire: Callback<ReplenishmentPolicyReadinessEntryResponse>,
    on_previous: Callback<()>,
    on_next: Callback<()>,
    on_sort: Callback<PolicySort>,
) -> impl IntoView {
    view! {
        <section class="data-section replenishment-policy-section">
            <div class="replenishment-summary-strip" aria-label="Policy page summary">
                <SummaryFact
                    label="Policies"
                    value=Signal::derive(move || signals.page.get().map_or(0, |page| page.items.len()).to_string())
                />
                <SummaryFact
                    label="Needs action"
                    value=Signal::derive(move || signals.page.get().map_or(0, |page| page.items.iter().filter(|policy| policy.target_gap > 0).count()).to_string())
                />
                <SummaryFact
                    label="No reserve"
                    value=Signal::derive(move || signals.page.get().map_or(0, |page| page.items.iter().filter(|policy| policy.suggested_outcome == ReplenishmentPlanningOutcome::InsufficientReserve).count()).to_string())
                />
                <SummaryFact
                    label="Active work"
                    value=Signal::derive(move || signals.page.get().map_or(0_i64, |page| page.items.iter().map(|policy| policy.active_work_count).sum()).to_string())
                />
                <button
                    type="button"
                    class="button primary-action replenishment-new-policy"
                    disabled=commands_locked
                    on:click=move |_| on_configure.run(None)
                >
                    <Plus size=15/>
                    "New policy"
                </button>
            </div>
            <div class="table-scroll replenishment-policy-scroll">
                <table class="data-table replenishment-policy-table">
                    <caption class="sr-only">"Active replenishment policies and live readiness"</caption>
                    <thead>
                        <tr>
                            <SortableHeader label="Client" active=move || signals.sort.get().key == PolicySort::Client direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Client))/>
                            <SortableHeader label="Facility" active=move || signals.sort.get().key == PolicySort::Facility direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Facility))/>
                            <SortableHeader label="Item" active=move || signals.sort.get().key == PolicySort::Item direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Item))/>
                            <SortableHeader label="Pick face" active=move || signals.sort.get().key == PolicySort::PickFace direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::PickFace))/>
                            <th class="numeric">"Effective min / target"</th>
                            <SortableHeader label="Projected" active=move || signals.sort.get().key == PolicySort::Projected direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Projected)) numeric=true/>
                            <SortableHeader label="Demand" active=move || signals.sort.get().key == PolicySort::Demand direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Demand)) numeric=true/>
                            <SortableHeader label="Reserve" active=move || signals.sort.get().key == PolicySort::Reserve direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Reserve)) numeric=true/>
                            <SortableHeader label="Gap / Plan" active=move || signals.sort.get().key == PolicySort::Gap direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Gap)) numeric=true/>
                            <SortableHeader label="State" active=move || signals.sort.get().key == PolicySort::Outcome direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Outcome))/>
                            <SortableHeader label="Work" active=move || signals.sort.get().key == PolicySort::Work direction=move || signals.sort.get().direction on_sort=Callback::new(move |_| on_sort.run(PolicySort::Work)) numeric=true/>
                            <th class="replenishment-actions-heading"><span class="sr-only">"Policy actions"</span></th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let policies = signals.page.get().map_or_else(Vec::new, |page| page.items);
                            if policies.is_empty() && !signals.loading.get() {
                                view! {
                                    <tr><td class="table-empty-row" colspan="12">"No active policies match this scope."</td></tr>
                                }.into_any()
                            } else {
                                policies.into_iter().map(|policy| {
                                    let for_plan = policy.clone();
                                    let for_edit = policy.clone();
                                    let for_retire = policy.clone();
                                    let item = policy.item_description.clone().unwrap_or_else(|| format!("Item #{}", policy.item_id));
                                    let sku = policy.primary_sku.clone().unwrap_or_else(|| format!("ID {}", policy.item_id));
                                    let pick_face = policy.pick_face.name.clone().unwrap_or_else(|| policy.pick_face.barcode.clone());
                                    let latest = policy.latest_plan.as_ref().map_or_else(
                                        || "Never planned".to_owned(),
                                        |plan| format!("Plan #{} / {}", plan.plan_id, compact_timestamp(&plan.planned_at)),
                                    );
                                    let suggested = suggested_text(&policy);
                                    let decision_rule = decision_policy_label(&policy.decision_policy);
                                    let inbound = inbound_text(
                                        policy.snapshot.pick_face_free,
                                        policy.observed_active_inbound,
                                        policy.snapshot.active_inbound,
                                    );
                                    view! {
                                        <tr>
                                            <td><strong>{policy.inventory_owner_name}</strong><small class="cell-detail">{format!("Client #{}", policy.inventory_owner_id)}</small></td>
                                            <td>{policy.facility_name}<small class="cell-detail">{format!("Facility #{}", policy.facility_id)}</small></td>
                                            <td><strong>{item}</strong><small class="cell-detail">{format!("{} / {}", sku, policy.uom)}</small></td>
                                            <td><span class="mono">{pick_face}</span><small class="cell-detail">{policy.pick_face.barcode}</small></td>
                                            <td class="numeric"><strong>{format!("{} / {}", format_quantity(policy.decision_policy.effective_minimum_quantity), format_quantity(policy.decision_policy.effective_target_quantity))}</strong><small class="cell-detail numeric-detail">{decision_rule}</small></td>
                                            <td class="numeric"><strong>{format_quantity(policy.snapshot.projected_free)}</strong><small class="cell-detail numeric-detail">{inbound}</small></td>
                                            <td class="numeric">{format_quantity(policy.snapshot.unallocated_demand)}</td>
                                            <td class="numeric">{format_quantity(policy.snapshot.reserve_free)}<small class="cell-detail numeric-detail">{format!("{} sources", policy.reserve_source_location_ids.as_slice().len())}</small></td>
                                            <td class="numeric strong">{format_quantity(policy.target_gap)}<small class="cell-detail numeric-detail">{suggested}</small></td>
                                            <td><span class=planning_outcome_class(policy.suggested_outcome)>{planning_outcome_label(policy.suggested_outcome)}</span><small class="cell-detail">{latest}</small></td>
                                            <td class="numeric">{format_quantity(policy.active_work_quantity)}<small class="cell-detail numeric-detail">{format!("{} tasks", policy.active_work_count)}</small></td>
                                            <td class="replenishment-row-actions">
                                                <button type="button" title="Run plan" aria-label=format!("Run plan for policy {}", policy.policy_id) disabled=commands_locked on:click=move |_| on_plan.run(for_plan.clone())><Play size=15/></button>
                                                <button type="button" title="Reconfigure" aria-label=format!("Reconfigure policy {}", policy.policy_id) disabled=commands_locked on:click=move |_| on_configure.run(Some(for_edit.clone()))><Pencil size=15/></button>
                                                <button type="button" title="Retire" aria-label=format!("Retire policy {}", policy.policy_id) disabled=commands_locked on:click=move |_| on_retire.run(for_retire.clone())><ArchiveX size=15/></button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view().into_any()
                            }
                        }}
                    </tbody>
                </table>
                <Show when=move || signals.loading.get()>
                    <div class="replenishment-table-loading" role="status">"Refreshing policy readiness..."</div>
                </Show>
            </div>
            <Show when=move || signals.error.get().is_some()>
                <p class="inline-command-error replenishment-page-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p>
            </Show>
            <div class="table-footer">
                <span>{move || signals.page.get().map_or_else(|| "Loading policies...".to_owned(), |page| format!("{} policies on this page", page.items.len()))}</span>
                <button type="button" class="button secondary-action" disabled=move || signals.loading.get() || signals.cursor_history.get().is_empty() on:click=move |_| on_previous.run(())>"Previous"</button>
                <button type="button" class="button secondary-action" disabled=move || signals.loading.get() || !signals.page.get().is_some_and(|page| page.has_more()) on:click=move |_| on_next.run(())>"Next"</button>
            </div>
        </section>
    }
}

#[component]
fn SummaryFact(label: &'static str, value: Signal<String>) -> impl IntoView {
    view! { <span><small>{label}</small><strong>{move || value.get()}</strong></span> }
}

fn suggested_text(policy: &ReplenishmentPolicyReadinessEntryResponse) -> String {
    match policy.suggested_outcome {
        ReplenishmentPlanningOutcome::NotNeeded => "0 planned".to_owned(),
        ReplenishmentPlanningOutcome::InsufficientReserve => {
            format!(
                "{} unfilled",
                format_quantity(policy.suggested_remaining_quantity)
            )
        }
        ReplenishmentPlanningOutcome::PartiallyPlanned => format!(
            "{} plan / {} left",
            format_quantity(policy.suggested_quantity),
            format_quantity(policy.suggested_remaining_quantity)
        ),
        ReplenishmentPlanningOutcome::FullyPlanned => {
            format!("{} planned", format_quantity(policy.suggested_quantity))
        }
    }
}

fn inbound_text(pick_face_free: i64, observed_inbound: i64, included_inbound: i64) -> String {
    if observed_inbound == 0 {
        format!("{} free / no inbound", format_quantity(pick_face_free))
    } else if included_inbound == 0 {
        format!(
            "{} free / {} inbound excluded",
            format_quantity(pick_face_free),
            format_quantity(observed_inbound)
        )
    } else {
        format!(
            "{} free + {} inbound",
            format_quantity(pick_face_free),
            format_quantity(included_inbound)
        )
    }
}

pub(super) fn decision_policy_label(policy: &ReplenishmentDecisionPolicyResponse) -> String {
    match policy.source {
        ReplenishmentDecisionPolicySource::ProductDefault => {
            "Product default / operational thresholds".to_owned()
        }
        ReplenishmentDecisionPolicySource::Configuration => format!(
            "Rule #{} r{} / {}%-{}% of operational target",
            policy.configuration_id.unwrap_or_default(),
            policy
                .configuration_revision
                .map_or(0, |revision| revision.get()),
            policy.minimum_percent.unwrap_or_default(),
            policy.target_percent.unwrap_or_default(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{ConfigurationScope, Revision};

    #[test]
    fn inbound_copy_makes_projection_conservation_visible() {
        assert_eq!(inbound_text(4, 0, 0), "4 free / no inbound");
        assert_eq!(inbound_text(4, 6, 6), "4 free + 6 inbound");
        assert_eq!(inbound_text(4, 6, 0), "4 free / 6 inbound excluded");
    }

    #[test]
    fn configured_rule_label_exposes_identity_revision_and_percentage_basis() {
        let policy = ReplenishmentDecisionPolicyResponse {
            source: ReplenishmentDecisionPolicySource::Configuration,
            configuration_id: Some(12),
            configuration_revision: Some(Revision::new(4).unwrap()),
            configuration_scope: Some(ConfigurationScope::Tenant),
            minimum_percent: Some(30),
            target_percent: Some(80),
            include_inbound_projection: false,
            operational_minimum_quantity: 2,
            operational_target_quantity: 10,
            effective_minimum_quantity: 3,
            effective_target_quantity: 8,
            policy_hash: "0".repeat(64),
        };

        assert_eq!(
            decision_policy_label(&policy),
            "Rule #12 r4 / 30%-80% of operational target"
        );
    }
}
