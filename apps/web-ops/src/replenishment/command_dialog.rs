use leptos::prelude::*;
use lucide_leptos::{ArchiveX, Pencil, Play, RefreshCw, Save, X};
use wareboxes_api_contract::v1::{
    PlanReplenishmentRequest, ReplenishmentPolicyReadinessEntryResponse,
    RetireReplenishmentPolicyRequest,
};

use super::model::{
    build_policy_request, item_label, location_label, CommandSignals, PolicyCommandAttempt,
    PolicyCommandResult, PolicyDialogMode, PolicyRequestInput, ReplenishmentReferenceData,
};
use crate::api;
use crate::view_model::format_quantity;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PolicyDraft {
    inventory_owner_id: String,
    facility_id: String,
    item_id: String,
    uom: String,
    pick_face_location_id: String,
    minimum_quantity: String,
    target_quantity: String,
    reserve_source_location_ids: Vec<i64>,
    source_search: String,
}

impl PolicyDraft {
    fn new(
        policy: Option<&ReplenishmentPolicyReadinessEntryResponse>,
        references: &ReplenishmentReferenceData,
    ) -> Self {
        if let Some(policy) = policy {
            return Self {
                inventory_owner_id: policy.inventory_owner_id.to_string(),
                facility_id: policy.facility_id.to_string(),
                item_id: policy.item_id.to_string(),
                uom: policy.uom.clone(),
                pick_face_location_id: policy.pick_face.location_id.to_string(),
                minimum_quantity: policy.minimum_quantity.to_string(),
                target_quantity: policy.target_quantity.to_string(),
                reserve_source_location_ids: policy.reserve_source_location_ids.as_slice().to_vec(),
                source_search: String::new(),
            };
        }

        let facility_id = references
            .access
            .facilities
            .first()
            .map_or_else(String::new, |facility| facility.id.to_string());
        let inventory_owner_id = references
            .access
            .inventory_owners
            .first()
            .map_or_else(String::new, |owner| owner.id.to_string());
        Self {
            facility_id,
            inventory_owner_id,
            minimum_quantity: "0".to_owned(),
            target_quantity: "1".to_owned(),
            ..Self::default()
        }
    }
}

#[component]
pub(super) fn PolicyCommandDialog(
    mode: PolicyDialogMode,
    references: RwSignal<ReplenishmentReferenceData>,
    references_loading: RwSignal<bool>,
    references_error: RwSignal<Option<String>>,
    signals: CommandSignals,
    on_success: Callback<PolicyCommandResult>,
) -> impl IntoView {
    let policy = match &mode {
        PolicyDialogMode::Configure(policy) => policy.as_ref(),
        PolicyDialogMode::Plan(policy) | PolicyDialogMode::Retire(policy) => Some(policy),
    };
    let draft = RwSignal::new(PolicyDraft::new(policy, &references.get_untracked()));
    let is_retry = move || signals.retry.get().is_some();
    let fields_locked = move || signals.pending.get() || is_retry() || signals.invalidated.get();
    let close_locked = move || signals.pending.get() || is_retry();
    let mode_for_submit = mode.clone();
    let mode_for_button = StoredValue::new(mode.clone());

    let close = move |_| {
        if !close_locked() {
            signals.error.set(None);
            signals.invalidated.set(false);
            signals.dialog.set(None);
        }
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if signals.pending.get_untracked() || signals.invalidated.get_untracked() {
            return;
        }
        let attempt = if let Some(retry) = signals.retry.get_untracked() {
            retry
        } else {
            match command_attempt(&mode_for_submit, &draft.get_untracked()) {
                Ok(attempt) => attempt,
                Err(message) => {
                    signals.error.set(Some(message));
                    return;
                }
            }
        };
        dispatch_command(attempt, signals, on_success);
    };

    let (title, subtitle, icon) = dialog_heading(&mode);
    let danger = matches!(&mode, PolicyDialogMode::Retire(_));
    let role = if danger { "alertdialog" } else { "dialog" };

    view! {
        <div class="replenishment-dialog-backdrop">
            <section
                class="replenishment-dialog"
                class:danger=danger
                role=role
                aria-modal="true"
                aria-labelledby="replenishment-dialog-title"
            >
                <header class="replenishment-dialog-heading">
                    <span class="replenishment-dialog-icon" aria-hidden="true">{icon}</span>
                    <div>
                        <h2 id="replenishment-dialog-title">{title}</h2>
                        <span>{subtitle}</span>
                    </div>
                    <button
                        type="button"
                        class="replenishment-dialog-close"
                        title="Close"
                        aria-label="Close replenishment command"
                        disabled=close_locked
                        on:click=close
                    >
                        <X size=16/>
                    </button>
                </header>
                <form class="replenishment-dialog-form" on:submit=submit>
                    {match mode {
                        PolicyDialogMode::Configure(policy) => view! {
                            <ConfigurePolicyFields
                                policy
                                draft
                                references
                                references_loading
                                references_error
                                locked=Signal::derive(fields_locked)
                            />
                        }.into_any(),
                        PolicyDialogMode::Plan(policy) => view! {
                            <PlanConfirmation policy/>
                        }.into_any(),
                        PolicyDialogMode::Retire(policy) => view! {
                            <RetireConfirmation policy/>
                        }.into_any(),
                    }}
                    <Show when=move || signals.error.get().is_some()>
                        <p class="inline-command-error replenishment-command-error" role="alert">
                            {move || signals.error.get().unwrap_or_default()}
                        </p>
                    </Show>
                    <Show when=is_retry>
                        <p class="replenishment-retry-note" role="status">
                            "The original request and idempotency key are retained. Retry sends that exact command."
                        </p>
                    </Show>
                    <Show when=move || signals.invalidated.get()>
                        <p class="replenishment-refresh-note" role="status">
                            "Authoritative policy and work data were refreshed. Close this dialog and reopen the command from the current revision."
                        </p>
                    </Show>
                    <div class="form-actions replenishment-dialog-actions">
                        <button
                            type="button"
                            class="button secondary-action"
                            disabled=close_locked
                            on:click=close
                        >
                            "Cancel"
                        </button>
                        <button
                            type="submit"
                            class=move || submit_class(&mode_for_button.get_value())
                            disabled=move || signals.pending.get() || signals.invalidated.get()
                        >
                            {move || submit_icon(&mode_for_button.get_value(), is_retry())}
                            {move || submit_label(&mode_for_button.get_value(), signals.pending.get(), is_retry())}
                        </button>
                    </div>
                </form>
            </section>
        </div>
    }
}

#[component]
fn ConfigurePolicyFields(
    policy: Option<ReplenishmentPolicyReadinessEntryResponse>,
    draft: RwSignal<PolicyDraft>,
    references: RwSignal<ReplenishmentReferenceData>,
    references_loading: RwSignal<bool>,
    references_error: RwSignal<Option<String>>,
    locked: Signal<bool>,
) -> impl IntoView {
    let reconfiguring = policy.is_some();
    let owner_options = references
        .get_untracked()
        .access
        .inventory_owners
        .into_iter()
        .map(|owner| view! { <option value=owner.id.to_string()>{owner.name}</option> })
        .collect_view();
    let facility_options = references
        .get_untracked()
        .access
        .facilities
        .into_iter()
        .map(|facility| view! { <option value=facility.id.to_string()>{facility.name}</option> })
        .collect_view();
    let on_item_change = move |event| {
        let item_id = event_target_value(&event);
        let uom = item_id.parse::<i64>().ok().and_then(|item_id| {
            references
                .get_untracked()
                .items
                .into_iter()
                .find(|item| item.id == item_id)
                .map(|item| item.packaging_unit)
        });
        draft.update(|draft| {
            draft.item_id = item_id;
            if let Some(uom) = uom {
                draft.uom = uom;
            }
        });
    };
    let on_facility_change = move |event| {
        let facility_id = event_target_value(&event);
        draft.update(|draft| {
            draft.facility_id = facility_id;
            draft.pick_face_location_id.clear();
            draft.reserve_source_location_ids.clear();
        });
    };

    view! {
        <p class="replenishment-dialog-intro">
            {if reconfiguring {
                "Replace the active version. Scope identities stay fixed; thresholds and explicit reserve sources may change."
            } else {
                "Create one active min/target policy for this client, facility, item, UOM, and pick face."
            }}
        </p>
        <div class="replenishment-policy-form-grid">
            <label>
                <span>"Client"</span>
                <select
                    required=true
                    disabled=move || locked.get() || reconfiguring
                    prop:value=move || draft.get().inventory_owner_id
                    on:change=move |event| draft.update(|value| value.inventory_owner_id = event_target_value(&event))
                >
                    <option value="">"Select client"</option>
                    {owner_options}
                </select>
            </label>
            <label>
                <span>"Facility"</span>
                <select
                    required=true
                    disabled=move || locked.get() || reconfiguring
                    prop:value=move || draft.get().facility_id
                    on:change=on_facility_change
                >
                    <option value="">"Select facility"</option>
                    {facility_options}
                </select>
            </label>
            <label class="wide">
                <span>"Item"</span>
                <select
                    required=true
                    disabled=move || locked.get() || reconfiguring || references_loading.get()
                    prop:value=move || draft.get().item_id
                    on:change=on_item_change
                >
                    <option value="">"Select item"</option>
                    {move || references.get().items.into_iter().map(|item| {
                        view! { <option value=item.id.to_string()>{item_label(&item)}</option> }
                    }).collect_view()}
                </select>
            </label>
            <label>
                <span>"UOM"</span>
                <input
                    required=true
                    maxlength="32"
                    disabled=move || locked.get() || reconfiguring
                    prop:value=move || draft.get().uom
                    on:input=move |event| draft.update(|value| value.uom = event_target_value(&event))
                />
            </label>
            <label>
                <span>"Pick face"</span>
                <select
                    required=true
                    disabled=move || locked.get() || reconfiguring || references_loading.get()
                    prop:value=move || draft.get().pick_face_location_id
                    on:change=move |event| draft.update(|value| value.pick_face_location_id = event_target_value(&event))
                >
                    <option value="">"Select pick face"</option>
                    {move || {
                        let facility_id = draft.get().facility_id.parse::<i64>().ok();
                        references.get().locations.into_iter()
                            .filter(|location| {
                                location.active
                                    && location.pickable
                                    && !location.receivable
                                    && location.barcode.is_some()
                                    && Some(location.facility_id) == facility_id
                            })
                            .map(|location| view! {
                                <option value=location.id.to_string()>{location_label(&location)}</option>
                            }).collect_view()
                    }}
                </select>
            </label>
            <label>
                <span>"Minimum"</span>
                <input
                    type="number"
                    min="0"
                    step="1"
                    required=true
                    disabled=locked
                    prop:value=move || draft.get().minimum_quantity
                    on:input=move |event| draft.update(|value| value.minimum_quantity = event_target_value(&event))
                />
            </label>
            <label>
                <span>"Target"</span>
                <input
                    type="number"
                    min="1"
                    step="1"
                    required=true
                    disabled=locked
                    prop:value=move || draft.get().target_quantity
                    on:input=move |event| draft.update(|value| value.target_quantity = event_target_value(&event))
                />
            </label>
        </div>
        <section class="replenishment-source-picker" aria-labelledby="reserve-source-title">
            <div class="replenishment-source-heading">
                <div>
                    <h3 id="reserve-source-title">"Eligible reserve sources"</h3>
                    <span>{move || format!("{} selected", draft.get().reserve_source_location_ids.len())}</span>
                </div>
                <input
                    type="search"
                    placeholder="Filter locations"
                    aria-label="Filter reserve source locations"
                    disabled=locked
                    prop:value=move || draft.get().source_search
                    on:input=move |event| draft.update(|value| value.source_search = event_target_value(&event))
                />
            </div>
            <div class="replenishment-source-list">
                {move || {
                    let state = draft.get();
                    let facility_id = state.facility_id.parse::<i64>().ok();
                    let pick_face_id = state.pick_face_location_id.parse::<i64>().ok();
                    let query = state.source_search.trim().to_ascii_lowercase();
                    let locations = references.get().locations.into_iter()
                        .filter(|location| {
                            location.active
                                && !location.pickable
                                && !location.receivable
                                && location.barcode.is_some()
                                && Some(location.facility_id) == facility_id
                        })
                        .filter(|location| Some(location.id) != pick_face_id)
                        .filter(|location| query.is_empty() || location_label(location).to_ascii_lowercase().contains(&query))
                        .collect::<Vec<_>>();
                    if locations.is_empty() {
                        view! { <p class="empty-state compact">"No matching active locations."</p> }.into_any()
                    } else {
                        locations.into_iter().map(|location| {
                            let location_id = location.id;
                            view! {
                                <label>
                                    <input
                                        type="checkbox"
                                        disabled=locked
                                        prop:checked=move || draft.get().reserve_source_location_ids.contains(&location_id)
                                        on:change=move |event| {
                                            let checked = event_target_checked(&event);
                                            draft.update(|value| {
                                                value.reserve_source_location_ids.retain(|id| *id != location_id);
                                                if checked {
                                                    value.reserve_source_location_ids.push(location_id);
                                                }
                                            });
                                        }
                                    />
                                    <span>{location_label(&location)}</span>
                                </label>
                            }
                        }).collect_view().into_any()
                    }
                }}
            </div>
        </section>
        <Show when=move || references_loading.get()>
            <p class="replenishment-reference-state" role="status">"Loading item and location choices..."</p>
        </Show>
        <Show when=move || references_error.get().is_some()>
            <p class="inline-command-error replenishment-reference-state" role="alert">
                {move || references_error.get().unwrap_or_default()}
            </p>
        </Show>
    }
}

#[component]
fn PlanConfirmation(policy: ReplenishmentPolicyReadinessEntryResponse) -> impl IntoView {
    let item = policy
        .item_description
        .clone()
        .unwrap_or_else(|| format!("Item #{}", policy.item_id));
    view! {
        <p class="replenishment-dialog-intro">
            "The server will lock inventory and recompute this projection. Quantity is not accepted from the browser."
        </p>
        <dl class="replenishment-plan-facts">
            <div><dt>"Client / facility"</dt><dd>{format!("{} / {}", policy.inventory_owner_name, policy.facility_name)}</dd></div>
            <div><dt>"Item / pick face"</dt><dd>{format!("{} / {}", item, policy.pick_face.barcode)}</dd></div>
            <div><dt>"Pick-face free"</dt><dd>{format_quantity(policy.snapshot.pick_face_free)}</dd></div>
            <div><dt>"Active inbound"</dt><dd>{format_quantity(policy.snapshot.active_inbound)}</dd></div>
            <div><dt>"Projected free"</dt><dd>{format_quantity(policy.snapshot.projected_free)}</dd></div>
            <div><dt>"Unallocated demand"</dt><dd>{format_quantity(policy.snapshot.unallocated_demand)}</dd></div>
            <div><dt>"Reserve free"</dt><dd>{format_quantity(policy.snapshot.reserve_free)}</dd></div>
            <div><dt>"Target gap"</dt><dd>{format_quantity(policy.target_gap)}</dd></div>
        </dl>
        <div class="replenishment-impact-line">
            <strong>{format!("Suggested: {} {}", format_quantity(policy.suggested_quantity), policy.uom)}</strong>
            <span>{format!("{} remaining / revision {}", format_quantity(policy.suggested_remaining_quantity), policy.revision.get())}</span>
        </div>
    }
}

#[component]
fn RetireConfirmation(policy: ReplenishmentPolicyReadinessEntryResponse) -> impl IntoView {
    let item = policy
        .item_description
        .unwrap_or_else(|| format!("Item #{}", policy.item_id));
    view! {
        <div class="replenishment-retire-warning">
            <strong>"Retire this active policy?"</strong>
            <p>
                {format!("{} / {} / {} at {}", policy.inventory_owner_name, policy.facility_name, item, policy.pick_face.barcode)}
            </p>
            <p>
                "The policy will no longer be eligible for planning. Existing replenishment work is not cancelled by this command."
            </p>
            <span>{format!("Current revision {}", policy.revision.get())}</span>
        </div>
    }
}

fn command_attempt(
    mode: &PolicyDialogMode,
    draft: &PolicyDraft,
) -> Result<PolicyCommandAttempt, String> {
    match mode {
        PolicyDialogMode::Configure(policy) => {
            let request = build_policy_request(PolicyRequestInput {
                owner: &draft.inventory_owner_id,
                facility: &draft.facility_id,
                item: &draft.item_id,
                uom: &draft.uom,
                pick_face: &draft.pick_face_location_id,
                minimum: &draft.minimum_quantity,
                target: &draft.target_quantity,
                reserve_sources: draft.reserve_source_location_ids.clone(),
                expected_revision: policy.as_ref().map(|policy| policy.revision),
            })?;
            Ok(PolicyCommandAttempt::Configure {
                request,
                idempotency_key: api::new_idempotency_key(),
            })
        }
        PolicyDialogMode::Plan(policy) => Ok(PolicyCommandAttempt::Plan {
            policy_id: policy.policy_id,
            request: PlanReplenishmentRequest {
                expected_policy_revision: policy.revision,
            },
            idempotency_key: api::new_idempotency_key(),
        }),
        PolicyDialogMode::Retire(policy) => Ok(PolicyCommandAttempt::Retire {
            policy_id: policy.policy_id,
            request: RetireReplenishmentPolicyRequest {
                expected_revision: policy.revision,
            },
            idempotency_key: api::new_idempotency_key(),
        }),
    }
}

fn dispatch_command(
    attempt: PolicyCommandAttempt,
    signals: CommandSignals,
    on_success: Callback<PolicyCommandResult>,
) {
    signals.retry.set(Some(attempt.clone()));
    signals.pending.set(true);
    signals.error.set(None);
    leptos::task::spawn_local(async move {
        let response = match &attempt {
            PolicyCommandAttempt::Configure {
                request,
                idempotency_key,
            } => api::configure_replenishment_policy(request, idempotency_key)
                .await
                .map(PolicyCommandResult::Configured),
            PolicyCommandAttempt::Plan {
                policy_id,
                request,
                idempotency_key,
            } => api::plan_replenishment(*policy_id, request, idempotency_key)
                .await
                .map(PolicyCommandResult::Planned),
            PolicyCommandAttempt::Retire {
                policy_id,
                request,
                idempotency_key,
            } => api::retire_replenishment_policy(*policy_id, request, idempotency_key)
                .await
                .map(PolicyCommandResult::Retired),
        };
        if signals.retry.get_untracked().as_ref() != Some(&attempt) {
            return;
        }
        signals.pending.set(false);
        match response {
            Ok(result) => {
                signals.retry.set(None);
                signals.error.set(None);
                signals.dialog.set(None);
                on_success.run(result);
            }
            Err(error) if error.unauthorized => {
                signals.retry.set(None);
                signals.dialog.set(None);
                signals.on_unauthorized.run(());
            }
            Err(error) if error.ambiguous_outcome => {
                signals.error.set(Some(format!(
                    "{} The result is unknown; retry the retained command.",
                    error.message
                )));
                signals.toasts.error(error.message);
            }
            Err(error) => {
                signals.retry.set(None);
                signals.invalidated.set(true);
                signals.error.set(Some(format!(
                    "{} No retry was retained; authoritative data is being refreshed.",
                    error.message
                )));
                signals.toasts.error(error.message);
                signals.on_authoritative_refresh.run(());
            }
        }
    });
}

fn dialog_heading(mode: &PolicyDialogMode) -> (&'static str, String, AnyView) {
    match mode {
        PolicyDialogMode::Configure(Some(policy)) => (
            "Reconfigure policy",
            format!(
                "Policy #{} / Revision {}",
                policy.policy_id,
                policy.revision.get()
            ),
            view! { <Pencil size=17/> }.into_any(),
        ),
        PolicyDialogMode::Configure(None) => (
            "Configure policy",
            "Create an active replenishment policy".to_owned(),
            view! { <Save size=17/> }.into_any(),
        ),
        PolicyDialogMode::Plan(policy) => (
            "Run replenishment plan",
            format!(
                "Policy #{} / Revision {}",
                policy.policy_id,
                policy.revision.get()
            ),
            view! { <Play size=17/> }.into_any(),
        ),
        PolicyDialogMode::Retire(policy) => (
            "Retire policy",
            format!(
                "Policy #{} / Revision {}",
                policy.policy_id,
                policy.revision.get()
            ),
            view! { <ArchiveX size=17/> }.into_any(),
        ),
    }
}

fn submit_icon(mode: &PolicyDialogMode, retry: bool) -> AnyView {
    if retry {
        view! { <RefreshCw size=15/> }.into_any()
    } else {
        match mode {
            PolicyDialogMode::Configure(_) => view! { <Save size=15/> }.into_any(),
            PolicyDialogMode::Plan(_) => view! { <Play size=15/> }.into_any(),
            PolicyDialogMode::Retire(_) => view! { <ArchiveX size=15/> }.into_any(),
        }
    }
}

fn submit_label(mode: &PolicyDialogMode, pending: bool, retry: bool) -> &'static str {
    if pending {
        return "Working";
    }
    if retry {
        return "Retry exact command";
    }
    match mode {
        PolicyDialogMode::Configure(Some(_)) => "Save new version",
        PolicyDialogMode::Configure(None) => "Create policy",
        PolicyDialogMode::Plan(_) => "Run plan",
        PolicyDialogMode::Retire(_) => "Retire policy",
    }
}

fn submit_class(mode: &PolicyDialogMode) -> &'static str {
    if matches!(mode, PolicyDialogMode::Retire(_)) {
        "button danger-action"
    } else {
        "button primary-action"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{
        ReplenishmentLocationResponse, ReplenishmentPlanningOutcome,
        ReplenishmentPlanningSnapshotResponse, ReplenishmentPolicyStatus,
        ReplenishmentReserveSourceLocationIds, Revision,
    };

    fn policy() -> ReplenishmentPolicyReadinessEntryResponse {
        ReplenishmentPolicyReadinessEntryResponse {
            policy_id: 7,
            revision: Revision::new(3).unwrap(),
            status: ReplenishmentPolicyStatus::Active,
            inventory_owner_id: 2,
            inventory_owner_name: "Northwind".into(),
            facility_id: 3,
            facility_name: "Reno".into(),
            item_id: 4,
            item_description: Some("Widget".into()),
            primary_sku: Some("WIDGET".into()),
            uom: "each".into(),
            pick_face: ReplenishmentLocationResponse {
                location_id: 10,
                barcode: "PICK-10".into(),
                name: None,
            },
            minimum_quantity: 5,
            target_quantity: 20,
            reserve_source_location_ids: ReplenishmentReserveSourceLocationIds::new(vec![11])
                .unwrap(),
            snapshot: ReplenishmentPlanningSnapshotResponse {
                pick_face_free: 2,
                active_inbound: 0,
                projected_free: 2,
                unallocated_demand: 8,
                reserve_free: 18,
            },
            required_level: 20,
            target_gap: 18,
            suggested_outcome: ReplenishmentPlanningOutcome::FullyPlanned,
            suggested_quantity: 18,
            suggested_remaining_quantity: 0,
            active_work_count: 0,
            active_work_quantity: 0,
            latest_plan: None,
        }
    }

    #[test]
    fn planning_attempt_contains_only_policy_revision_and_exact_key() {
        let attempt =
            command_attempt(&PolicyDialogMode::Plan(policy()), &PolicyDraft::default()).unwrap();
        match attempt {
            PolicyCommandAttempt::Plan {
                policy_id,
                request,
                idempotency_key,
            } => {
                assert_eq!(policy_id, 7);
                assert_eq!(request.expected_policy_revision.get(), 3);
                assert!(!idempotency_key.is_empty());
            }
            _ => panic!("expected plan attempt"),
        }
    }

    #[test]
    fn reconfigure_draft_preserves_scope_and_revision() {
        let policy = policy();
        let draft = PolicyDraft::new(Some(&policy), &ReplenishmentReferenceData::default());
        let attempt = command_attempt(&PolicyDialogMode::Configure(Some(policy)), &draft).unwrap();
        match attempt {
            PolicyCommandAttempt::Configure { request, .. } => {
                assert_eq!(request.inventory_owner_id, 2);
                assert_eq!(request.expected_revision.unwrap().get(), 3);
                assert_eq!(request.reserve_source_location_ids.as_slice(), &[11]);
            }
            _ => panic!("expected configure attempt"),
        }
    }
}
