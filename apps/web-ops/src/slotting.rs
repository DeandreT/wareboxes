use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AcceptSlottingRecommendationRequest, ConfigureSlottingProfileRequest,
    DismissSlottingRecommendationRequest, OpaqueCursor, RunSlottingRequest, SlottingAdvisoryMode,
    SlottingDismissalReason, SlottingProfilePage, SlottingProfilePageRequest,
    SlottingProfileResponse, SlottingRecommendationPage, SlottingRecommendationPageRequest,
    SlottingRecommendationReason, SlottingRecommendationResponse, SlottingRecommendationStatus,
    SlottingRunResponse,
};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};
use crate::view_model::format_quantity;

#[derive(Clone)]
enum Dialog {
    Configure(Option<SlottingProfileResponse>),
    Accept(SlottingRecommendationResponse),
    Dismiss(SlottingRecommendationResponse),
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build dispatches slotting commands")
)]
enum PendingCommand {
    Configure(ConfigureSlottingProfileRequest, String),
    Run(RunSlottingRequest, String),
    Accept(i64, AcceptSlottingRecommendationRequest, String),
    Dismiss(i64, DismissSlottingRecommendationRequest, String),
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "browser build handles slotting API outcomes")
)]
struct Signals {
    profiles: RwSignal<SlottingProfilePage>,
    recommendations: RwSignal<SlottingRecommendationPage>,
    profiles_loaded: RwSignal<bool>,
    recommendations_loaded: RwSignal<bool>,
    profiles_loading: RwSignal<bool>,
    recommendations_loading: RwSignal<bool>,
    profile_generation: RwSignal<u64>,
    recommendation_generation: RwSignal<u64>,
    error: RwSignal<Option<String>>,
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    include_history: RwSignal<bool>,
    status: RwSignal<Option<SlottingRecommendationStatus>>,
    dialog: RwSignal<Option<Dialog>>,
    command_pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    last_run: RwSignal<Option<SlottingRunResponse>>,
    mode: RwSignal<SlottingAdvisoryMode>,
    lookback: RwSignal<String>,
    demand_weight: RwSignal<String>,
    travel_weight: RwSignal<String>,
    activity_weight: RwSignal<String>,
    minimum_demand: RwSignal<String>,
    maximum_recommendations: RwSignal<String>,
    default_priority: RwSignal<String>,
    action_priority: RwSignal<String>,
    action_note: RwSignal<String>,
    dismissal_reason: RwSignal<SlottingDismissalReason>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn SlottingWorkspace(
    access: AccessScopeWorkspace,
    can_supervise: bool,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let access = StoredValue::new(access);
    let signals = Signals {
        profiles: RwSignal::new(SlottingProfilePage::new(Vec::new(), None)),
        recommendations: RwSignal::new(SlottingRecommendationPage::new(Vec::new(), None)),
        profiles_loaded: RwSignal::new(false),
        recommendations_loaded: RwSignal::new(false),
        profiles_loading: RwSignal::new(false),
        recommendations_loading: RwSignal::new(false),
        profile_generation: RwSignal::new(0),
        recommendation_generation: RwSignal::new(0),
        error: RwSignal::new(None),
        facility_id: RwSignal::new(None),
        owner_id: RwSignal::new(None),
        include_history: RwSignal::new(false),
        status: RwSignal::new(Some(SlottingRecommendationStatus::Pending)),
        dialog: RwSignal::new(None),
        command_pending: RwSignal::new(false),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        last_run: RwSignal::new(None),
        mode: RwSignal::new(SlottingAdvisoryMode::Enabled),
        lookback: RwSignal::new("30".into()),
        demand_weight: RwSignal::new("100".into()),
        travel_weight: RwSignal::new("50".into()),
        activity_weight: RwSignal::new("25".into()),
        minimum_demand: RwSignal::new("1".into()),
        maximum_recommendations: RwSignal::new("100".into()),
        default_priority: RwSignal::new("20".into()),
        action_priority: RwSignal::new(String::new()),
        action_note: RwSignal::new(String::new()),
        dismissal_reason: RwSignal::new(SlottingDismissalReason::OperationalConstraint),
        on_unauthorized,
        toasts: use_toast_bus(),
    };

    Effect::new(move |_| refresh_all(signals));

    let apply_filters = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        refresh_all(signals);
    };
    let refresh = move |_| refresh_all(signals);
    let new_profile = move |_| {
        signals.command_error.set(None);
        signals.retry.set(None);
        reset_profile_draft(signals, None);
        signals.dialog.set(Some(Dialog::Configure(None)));
    };
    let retry = move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    };

    view! {
        <section class="slotting-workspace">
            <header class="page-heading slotting-heading">
                <div>
                    <p class="eyebrow">"Inventory optimization"</p>
                    <h1>"Slotting advisory"</h1>
                    <p>"Versioned policy, reproducible scoring evidence, and controlled relocation work."</p>
                </div>
                <div class="slotting-heading-actions">
                    {can_supervise.then(|| view! {
                        <button class="button primary-action" type="button" on:click=new_profile>
                            "Configure profile"
                        </button>
                    })}
                    <button
                        class="button secondary-action"
                        type="button"
                        disabled=move || signals.profiles_loading.get() || signals.recommendations_loading.get()
                        on:click=refresh
                    >
                        <Icon icon=UiIcon::Refresh/>
                        <span>"Refresh"</span>
                    </button>
                </div>
            </header>

            <form class="slotting-toolbar" on:submit=apply_filters>
                <label>
                    <span>"Facility"</span>
                    <select
                        prop:value=move || option_id(signals.facility_id.get())
                        on:change=move |event| signals.facility_id.set(parse_id(&event_target_value(&event)))
                    >
                        <option value="">"All authorized facilities"</option>
                        {access.with_value(|value| scope_options(&value.facilities))}
                    </select>
                </label>
                <label>
                    <span>"Client"</span>
                    <select
                        prop:value=move || option_id(signals.owner_id.get())
                        on:change=move |event| signals.owner_id.set(parse_id(&event_target_value(&event)))
                    >
                        <option value="">"All authorized clients"</option>
                        {access.with_value(|value| scope_options(&value.inventory_owners))}
                    </select>
                </label>
                <label>
                    <span>"Recommendation status"</span>
                    <select
                        prop:value=move || status_wire(signals.status.get())
                        on:change=move |event| signals.status.set(parse_status(&event_target_value(&event)))
                    >
                        <option value="">"All decisions"</option>
                        <option value="pending">"Pending"</option>
                        <option value="accepted">"Accepted"</option>
                        <option value="dismissed">"Dismissed"</option>
                    </select>
                </label>
                <label class="slotting-history-toggle">
                    <input
                        type="checkbox"
                        prop:checked=move || signals.include_history.get()
                        on:change=move |event| signals.include_history.set(event_target_checked(&event))
                    />
                    <span>"Profile history"</span>
                </label>
                <button class="button secondary-action compact" type="submit">"Apply"</button>
            </form>

            {move || metrics(signals)}

            {move || signals.last_run.get().map(run_evidence)}

            <Show when=move || signals.error.get().is_some()>
                <div class="slotting-error" role="alert">
                    <span>{move || signals.error.get().unwrap_or_default()}</span>
                    <button class="text-button" type="button" on:click=refresh>"Retry reads"</button>
                </div>
            </Show>

            {move || profile_panel(signals, access, can_supervise)}
            {move || recommendation_panel(signals, access, can_supervise)}

            {move || signals.dialog.get().map(|dialog| command_dialog(signals, access, dialog))}

            <Show when=move || signals.command_error.get().is_some() && signals.dialog.get().is_none()>
                <div class="slotting-error command" role="alert">
                    <span>{move || signals.command_error.get().unwrap_or_default()}</span>
                    <Show when=move || signals.retry.get().is_some()>
                        <button
                            class="button secondary-action compact"
                            type="button"
                            disabled=move || signals.command_pending.get()
                            on:click=retry
                        >"Retry exact command"</button>
                    </Show>
                </div>
            </Show>
        </section>
    }
}

fn metrics(signals: Signals) -> AnyView {
    let profiles = signals.profiles.get();
    let recommendations = signals.recommendations.get();
    let active = profiles
        .items
        .iter()
        .filter(|row| row.effective_to.is_none() && row.mode == SlottingAdvisoryMode::Enabled)
        .count();
    let pending = recommendations
        .items
        .iter()
        .filter(|row| row.status == SlottingRecommendationStatus::Pending)
        .count();
    let accepted = recommendations
        .items
        .iter()
        .filter(|row| row.status == SlottingRecommendationStatus::Accepted)
        .count();
    let represented_quantity = recommendations
        .items
        .iter()
        .map(|row| row.recommended_quantity)
        .sum::<i64>();
    view! {
        <section class="slotting-metrics" aria-label="Slotting totals">
            <div><span>"Active profiles"</span><strong>{active}</strong></div>
            <div><span>"Pending decisions"</span><strong>{pending}</strong></div>
            <div><span>"Accepted in page"</span><strong>{accepted}</strong></div>
            <div><span>"Units represented"</span><strong>{format_quantity(represented_quantity)}</strong></div>
        </section>
    }.into_any()
}

fn profile_panel(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_supervise: bool,
) -> AnyView {
    if signals.profiles_loading.get() && !signals.profiles_loaded.get() {
        return loading_state("Loading slotting profiles");
    }
    let page = signals.profiles.get();
    let next = page.next_cursor.clone();
    if page.items.is_empty() {
        return empty_state(
            "No slotting profiles",
            "Configure an owner/facility profile to enable advisory recommendations.",
        );
    }
    view! {
        <section class="slotting-panel">
            <header><div><p class="eyebrow">"Configuration"</p><h2>"Profiles"</h2></div><span>{format!("{} in view", page.items.len())}</span></header>
            <div class="table-scroll">
                <table class="dense-table slotting-profile-table">
                    <thead><tr><th>"Scope"</th><th>"Mode"</th><th>"Demand window"</th><th>"Score weights"</th><th>"Limits"</th><th>"Version evidence"</th><th></th></tr></thead>
                    <tbody>{page.items.into_iter().map(|profile| profile_row(signals, access, can_supervise, profile)).collect_view()}</tbody>
                </table>
            </div>
            {next.map(|cursor| view! {
                <button class="button secondary-action compact load-more" type="button" disabled=move || signals.profiles_loading.get() on:click=move |_| load_profiles(signals, Some(cursor.clone()), true)>"Load more profiles"</button>
            })}
        </section>
    }.into_any()
}

fn profile_row(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_supervise: bool,
    profile: SlottingProfileResponse,
) -> AnyView {
    let configure = profile.clone();
    let run = profile.clone();
    let active = profile.effective_to.is_none();
    let enabled = profile.mode == SlottingAdvisoryMode::Enabled;
    let scope = access.with_value(|value| {
        format!(
            "{} · {}",
            scope_name(&value.inventory_owners, profile.inventory_owner_id),
            scope_name(&value.facilities, profile.facility_id)
        )
    });
    view! {
        <tr>
            <td><strong>{scope}</strong><small>{format!("Profile #{}", profile.slotting_profile_id)}</small></td>
            <td><span class=mode_class(profile.mode)>{mode_label(profile.mode)}</span>{(!active).then(|| view! { <small>"Superseded"</small> })}</td>
            <td>{format!("{} days", profile.demand_lookback_days)}<small>{format!("Minimum {} units", profile.minimum_demand_quantity)}</small></td>
            <td>{format!("D{} / T{} / A{}", profile.demand_weight, profile.travel_weight, profile.activity_weight)}</td>
            <td>{format!("{} recommendations", profile.max_recommendations)}<small>{format!("Task priority {}", profile.default_task_priority)}</small></td>
            <td><strong>{format!("Revision {}", profile.revision.get())}</strong><small>{format!("Actor #{} · {}", profile.configured_by, short_timestamp(&profile.configured_at))}</small><small>{format!("Effective {}{}", short_timestamp(&profile.effective_from), profile.effective_to.as_deref().map(|value| format!(" – {}", short_timestamp(value))).unwrap_or_default())}</small></td>
            <td>{(can_supervise && active).then(|| view! {
                <div class="slotting-row-actions">
                    <button class="text-button" type="button" disabled=move || signals.command_pending.get() on:click=move |_| { reset_profile_draft(signals, Some(&configure)); signals.dialog.set(Some(Dialog::Configure(Some(configure.clone())))); }>"Supersede"</button>
                    {enabled.then(|| view! { <button class="button primary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=move |_| dispatch(signals, PendingCommand::Run(RunSlottingRequest { inventory_owner_id: run.inventory_owner_id, facility_id: run.facility_id, expected_profile_revision: run.revision }, api::new_idempotency_key()))>"Run advisory"</button> })}
                </div>
            })}</td>
        </tr>
    }.into_any()
}

fn recommendation_panel(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_supervise: bool,
) -> AnyView {
    if signals.recommendations_loading.get() && !signals.recommendations_loaded.get() {
        return loading_state("Loading recommendations");
    }
    let page = signals.recommendations.get();
    let next = page.next_cursor.clone();
    if page.items.is_empty() {
        return empty_state(
            "No recommendations in this view",
            "Run an enabled profile or broaden the scope and decision filters.",
        );
    }
    view! {
        <section class="slotting-panel recommendations">
            <header><div><p class="eyebrow">"Decision queue"</p><h2>"Recommendations"</h2></div><span>{format!("{} in view", page.items.len())}</span></header>
            <div class="slotting-recommendation-list">
                {page.items.into_iter().map(|row| recommendation_card(signals, access, can_supervise, row)).collect_view()}
            </div>
            {next.map(|cursor| view! {
                <button class="button secondary-action compact load-more" type="button" disabled=move || signals.recommendations_loading.get() on:click=move |_| load_recommendations(signals, Some(cursor.clone()), true)>"Load more recommendations"</button>
            })}
        </section>
    }.into_any()
}

fn recommendation_card(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    can_supervise: bool,
    row: SlottingRecommendationResponse,
) -> AnyView {
    let accept = row.clone();
    let dismiss = row.clone();
    let pending = row.status == SlottingRecommendationStatus::Pending;
    let scope = access.with_value(|value| {
        format!(
            "{} · {}",
            scope_name(&value.inventory_owners, row.inventory_owner_id),
            scope_name(&value.facilities, row.facility_id)
        )
    });
    let capacity = row
        .evidence
        .destination_capacity
        .map_or_else(|| "Unbounded".into(), format_quantity);
    view! {
        <article class="slotting-recommendation">
            <header>
                <div><span class=status_class(row.status)>{status_label(row.status)}</span><strong>{row.item_description.clone().unwrap_or_else(|| format!("Item #{}", row.item_id))}</strong><small>{format!("{} · {} · run #{}", row.uom, scope, row.slotting_run_id)}</small></div>
                <div class="slotting-score"><span>"Score"</span><strong>{row.score.total}</strong><small>{reason_label(row.reason)}</small></div>
            </header>
            <div class="slotting-move">
                <div><span>"Source"</span><strong>{row.source_location_label.clone()}</strong><small>{format!("{} · balance #{}", row.source_zone_code, row.source_inventory_balance_id)}</small></div>
                <span class="slotting-arrow">"→"</span>
                <div><span>"Destination"</span><strong>{row.destination_location_label.clone()}</strong><small>{row.destination_zone_code.clone()}</small></div>
                <div class="slotting-quantity"><span>"Move"</span><strong>{format_quantity(row.recommended_quantity)}</strong><small>{row.uom.clone()}</small></div>
            </div>
            <details class="slotting-evidence">
                <summary>"Frozen scoring and inventory evidence"</summary>
                <dl>
                    <div><dt>"Demand"</dt><dd>{format!("{} outstanding / {} historical", row.evidence.outstanding_demand_quantity, row.evidence.historical_pick_quantity)}</dd></div>
                    <div><dt>"Pick activity"</dt><dd>{format!("{} picks", row.evidence.historical_pick_count)}</dd></div>
                    <div><dt>"Travel sequence"</dt><dd>{format!("{} → {}", row.evidence.source_travel_sequence, row.evidence.destination_travel_sequence)}</dd></div>
                    <div><dt>"Source stock"</dt><dd>{format!("{} on hand / {} movable", row.evidence.source_on_hand, row.evidence.source_movable_quantity)}</dd></div>
                    <div><dt>"Destination"</dt><dd>{format!("{} on hand + {} planned / {} capacity", row.evidence.destination_on_hand, row.evidence.destination_inbound_planned_quantity, capacity)}</dd></div>
                    <div><dt>"Score components"</dt><dd>{format!("Demand {} · travel {} · activity {}", row.score.demand_component, row.score.travel_component, row.score.activity_component)}</dd></div>
                    <div><dt>"Storage policy"</dt><dd>{format!("#{} revision {}", row.item_storage_policy_id, row.item_storage_policy_revision)}</dd></div>
                    <div><dt>"Recommendation"</dt><dd>{format!("#{} revision {} · {}", row.slotting_recommendation_id, row.revision.get(), short_timestamp(&row.created_at))}</dd></div>
                </dl>
            </details>
            {(row.decided_at.is_some() || row.inventory_relocation_task_id.is_some()).then(|| view! {
                <div class="slotting-decision-evidence">
                    <strong>"Decision evidence"</strong>
                    <span>{row.decided_by.map_or_else(|| "Actor unavailable".into(), |id| format!("Actor #{id}"))}</span>
                    <span>{row.decided_at.as_deref().map(short_timestamp).unwrap_or_else(|| "Not decided".into())}</span>
                    {row.dismissal_reason.map(|value| view! { <span>{dismissal_label(value)}</span> })}
                    {row.dismissal_note.clone().map(|value| view! { <span>{value}</span> })}
                    {row.inventory_relocation_task_id.map(|id| view! { <span>{format!("Relocation task #{id}")}</span> })}
                </div>
            })}
            {(can_supervise && pending).then(|| view! {
                <footer>
                    <button class="button primary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=move |_| { reset_action_draft(signals); signals.dialog.set(Some(Dialog::Accept(accept.clone()))); }>"Accept and create work"</button>
                    <button class="button secondary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=move |_| { reset_action_draft(signals); signals.dialog.set(Some(Dialog::Dismiss(dismiss.clone()))); }>"Dismiss"</button>
                </footer>
            })}
        </article>
    }.into_any()
}

fn command_dialog(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    dialog: Dialog,
) -> AnyView {
    let title = match &dialog {
        Dialog::Configure(Some(_)) => "Supersede slotting profile",
        Dialog::Configure(None) => "Configure slotting profile",
        Dialog::Accept(_) => "Accept recommendation",
        Dialog::Dismiss(_) => "Dismiss recommendation",
    };
    let submit_dialog = dialog.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        match build_command(signals, &submit_dialog) {
            Ok(command) => dispatch(signals, command),
            Err(message) => signals.command_error.set(Some(message)),
        }
    };
    view! {
        <div class="slotting-dialog-backdrop" role="presentation">
            <section class="slotting-dialog" role="dialog" aria-modal="true" aria-label=title>
                <header><div><p class="eyebrow">"Controlled command"</p><h2>{title}</h2></div><button class="icon-button" type="button" aria-label="Close" disabled=move || signals.command_pending.get() on:click=move |_| close_dialog(signals)>"×"</button></header>
                <form on:submit=submit>
                    {dialog_fields(signals, access, dialog)}
                    <Show when=move || signals.command_error.get().is_some()>
                        <p class="slotting-form-error" role="alert">{move || signals.command_error.get().unwrap_or_default()}</p>
                    </Show>
                    <Show when=move || signals.retry.get().is_some()>
                        <p class="slotting-retry-evidence">"Outcome unknown. Submitting again retries the exact saved request and idempotency key."</p>
                    </Show>
                    <footer>
                        <button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| close_dialog(signals)>"Cancel"</button>
                        <button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>{move || if signals.command_pending.get() { "Submitting…" } else if signals.retry.get().is_some() { "Retry exact command" } else { "Submit" }}</button>
                    </footer>
                </form>
            </section>
        </div>
    }.into_any()
}

fn dialog_fields(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    dialog: Dialog,
) -> AnyView {
    match dialog {
        Dialog::Configure(existing) => {
            let locked = existing.is_some();
            let existing_scope = existing.as_ref().map(|row| (row.inventory_owner_id, row.facility_id));
            let owner_options = access.with_value(|value| scope_options(&value.inventory_owners));
            let facility_options = access.with_value(|value| scope_options(&value.facilities));
            view! {
                <div class="slotting-form-grid">
                    <label><span>"Client"</span><select required disabled=locked prop:value=move || option_id(existing_scope.map(|value| value.0).or(signals.owner_id.get())) on:change=move |event| signals.owner_id.set(parse_id(&event_target_value(&event)))><option value="">"Select client"</option>{owner_options}</select></label>
                    <label><span>"Facility"</span><select required disabled=locked prop:value=move || option_id(existing_scope.map(|value| value.1).or(signals.facility_id.get())) on:change=move |event| signals.facility_id.set(parse_id(&event_target_value(&event)))><option value="">"Select facility"</option>{facility_options}</select></label>
                    <label><span>"Advisory mode"</span><select prop:value=move || mode_wire(signals.mode.get()) on:change=move |event| signals.mode.set(parse_mode(&event_target_value(&event)))><option value="enabled">"Enabled"</option><option value="disabled">"Disabled / safe fallback"</option></select></label>
                    {number_input("Demand lookback days (1–365)", signals.lookback)}
                    {number_input("Demand weight (1–10,000)", signals.demand_weight)}
                    {number_input("Travel weight (1–10,000)", signals.travel_weight)}
                    {number_input("Activity weight (1–10,000)", signals.activity_weight)}
                    {number_input("Minimum demand quantity", signals.minimum_demand)}
                    {number_input("Maximum recommendations (1–1,000)", signals.maximum_recommendations)}
                    {number_input("Default relocation priority", signals.default_priority)}
                </div>
            }.into_any()
        }
        Dialog::Accept(row) => view! {
            <div class="slotting-action-summary"><strong>{row.item_description.unwrap_or_else(|| format!("Item #{}", row.item_id))}</strong><span>{format!("Move {} {} from {} to {}", row.recommended_quantity, row.uom, row.source_location_label, row.destination_location_label)}</span><small>{format!("Recommendation #{} revision {}", row.slotting_recommendation_id, row.revision.get())}</small></div>
            <div class="slotting-form-grid compact">
                <label><span>"Task priority override"</span><input type="number" min="0" max="65535" placeholder="Use profile default" prop:value=move || signals.action_priority.get() on:input=move |event| signals.action_priority.set(event_target_value(&event))/></label>
                <label class="wide"><span>"Relocation instructions"</span><textarea maxlength="500" placeholder="Optional execution guidance" prop:value=move || signals.action_note.get() on:input=move |event| signals.action_note.set(event_target_value(&event))></textarea></label>
            </div>
        }.into_any(),
        Dialog::Dismiss(row) => view! {
            <div class="slotting-action-summary"><strong>{row.item_description.unwrap_or_else(|| format!("Item #{}", row.item_id))}</strong><span>{format!("{} → {} · {} {}", row.source_location_label, row.destination_location_label, row.recommended_quantity, row.uom)}</span><small>{format!("Recommendation #{} revision {}", row.slotting_recommendation_id, row.revision.get())}</small></div>
            <div class="slotting-form-grid compact">
                <label><span>"Dismissal reason"</span><select prop:value=move || dismissal_wire(signals.dismissal_reason.get()) on:change=move |event| signals.dismissal_reason.set(parse_dismissal(&event_target_value(&event)))>{dismissal_options()}</select></label>
                <label class="wide"><span>"Decision note"</span><textarea maxlength="500" placeholder="Required for Other; useful operational evidence for every dismissal" prop:value=move || signals.action_note.get() on:input=move |event| signals.action_note.set(event_target_value(&event))></textarea></label>
            </div>
        }.into_any(),
    }
}

fn build_command(signals: Signals, dialog: &Dialog) -> Result<PendingCommand, String> {
    if let Some(saved) = signals.retry.get_untracked() {
        return Ok(saved);
    }
    let key = api::new_idempotency_key();
    match dialog {
        Dialog::Configure(existing) => {
            let (owner_id, facility_id, expected_revision) = existing.as_ref().map_or(
                (
                    signals.owner_id.get_untracked(),
                    signals.facility_id.get_untracked(),
                    None,
                ),
                |row| {
                    (
                        Some(row.inventory_owner_id),
                        Some(row.facility_id),
                        Some(row.revision),
                    )
                },
            );
            let request = ConfigureSlottingProfileRequest {
                inventory_owner_id: owner_id
                    .ok_or_else(|| "Select an inventory owner.".to_owned())?,
                facility_id: facility_id.ok_or_else(|| "Select a facility.".to_owned())?,
                mode: signals.mode.get_untracked(),
                demand_lookback_days: parsed::<u16>(signals.lookback, "demand lookback")?,
                demand_weight: parsed::<u32>(signals.demand_weight, "demand weight")?,
                travel_weight: parsed::<u32>(signals.travel_weight, "travel weight")?,
                activity_weight: parsed::<u32>(signals.activity_weight, "activity weight")?,
                minimum_demand_quantity: parsed::<i64>(signals.minimum_demand, "minimum demand")?,
                max_recommendations: parsed::<u16>(
                    signals.maximum_recommendations,
                    "maximum recommendations",
                )?,
                default_task_priority: parsed::<u16>(signals.default_priority, "default priority")?,
                expected_revision,
            };
            validate_profile(&request)?;
            Ok(PendingCommand::Configure(request, key))
        }
        Dialog::Accept(row) => Ok(PendingCommand::Accept(
            row.slotting_recommendation_id,
            AcceptSlottingRecommendationRequest {
                expected_revision: row.revision,
                task_priority: optional_parsed::<u16>(signals.action_priority, "task priority")?,
                instructions: optional_note(signals.action_note.get_untracked()),
            },
            key,
        )),
        Dialog::Dismiss(row) => {
            let note = optional_note(signals.action_note.get_untracked());
            if signals.dismissal_reason.get_untracked() == SlottingDismissalReason::Other
                && note.is_none()
            {
                return Err("Enter a decision note when the reason is Other.".into());
            }
            Ok(PendingCommand::Dismiss(
                row.slotting_recommendation_id,
                DismissSlottingRecommendationRequest {
                    expected_revision: row.revision,
                    reason: signals.dismissal_reason.get_untracked(),
                    note,
                },
                key,
            ))
        }
    }
}

fn dispatch(signals: Signals, command: PendingCommand) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.command_error.set(None);
    signals.retry.set(Some(command.clone()));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, command);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = match &command {
            PendingCommand::Configure(request, key) => {
                api::configure_slotting_profile(request, key)
                    .await
                    .map(|_| None)
            }
            PendingCommand::Run(request, key) => api::run_slotting(request, key).await.map(Some),
            PendingCommand::Accept(id, request, key) => {
                api::accept_slotting_recommendation(*id, request, key)
                    .await
                    .map(|_| None)
            }
            PendingCommand::Dismiss(id, request, key) => {
                api::dismiss_slotting_recommendation(*id, request, key)
                    .await
                    .map(|_| None)
            }
        };
        signals.command_pending.set(false);
        match result {
            Ok(run) => {
                signals.retry.set(None);
                signals.dialog.set(None);
                signals.command_error.set(None);
                if let Some(run) = run {
                    signals.toasts.success(format!(
                        "Slotting run #{} generated {} recommendations.",
                        run.slotting_run_id, run.recommendation_count
                    ));
                    signals.last_run.set(Some(run));
                } else {
                    signals.toasts.success("Slotting workspace updated.");
                }
                refresh_all(signals);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None);
                }
                signals.command_error.set(Some(error.message.clone()));
                signals.toasts.error(error.message);
            }
        }
    });
}

fn refresh_all(signals: Signals) {
    signals.error.set(None);
    load_profiles(signals, None, false);
    load_recommendations(signals, None, false);
}

fn load_profiles(signals: Signals, cursor: Option<OpaqueCursor>, append: bool) {
    let generation = signals.profile_generation.get_untracked().wrapping_add(1);
    signals.profile_generation.set(generation);
    signals.profiles_loading.set(true);
    let request = SlottingProfilePageRequest {
        inventory_owner_id: signals.owner_id.get_untracked(),
        facility_id: signals.facility_id.get_untracked(),
        include_history: signals.include_history.get_untracked(),
        cursor,
        limit: Default::default(),
    };
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, request, append, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::slotting_profiles(&request).await {
            Ok(mut page) if signals.profile_generation.get_untracked() == generation => {
                if append {
                    let mut current = signals.profiles.get_untracked();
                    current.items.append(&mut page.items);
                    current.next_cursor = page.next_cursor;
                    signals.profiles.set(current);
                } else {
                    signals.profiles.set(page);
                }
                signals.profiles_loaded.set(true);
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) if signals.profile_generation.get_untracked() == generation => {
                signals.error.set(Some(error.message));
            }
            Err(_) => {}
        }
        if signals.profile_generation.get_untracked() == generation {
            signals.profiles_loading.set(false);
        }
    });
}

fn load_recommendations(signals: Signals, cursor: Option<OpaqueCursor>, append: bool) {
    let generation = signals
        .recommendation_generation
        .get_untracked()
        .wrapping_add(1);
    signals.recommendation_generation.set(generation);
    signals.recommendations_loading.set(true);
    let request = SlottingRecommendationPageRequest {
        inventory_owner_id: signals.owner_id.get_untracked(),
        facility_id: signals.facility_id.get_untracked(),
        slotting_run_id: None,
        status: signals.status.get_untracked(),
        cursor,
        limit: Default::default(),
    };
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (signals, request, append, generation);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        match api::slotting_recommendations(&request).await {
            Ok(mut page) if signals.recommendation_generation.get_untracked() == generation => {
                if append {
                    let mut current = signals.recommendations.get_untracked();
                    current.items.append(&mut page.items);
                    current.next_cursor = page.next_cursor;
                    signals.recommendations.set(current);
                } else {
                    signals.recommendations.set(page);
                }
                signals.recommendations_loaded.set(true);
            }
            Ok(_) => {}
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) if signals.recommendation_generation.get_untracked() == generation => {
                signals.error.set(Some(error.message));
            }
            Err(_) => {}
        }
        if signals.recommendation_generation.get_untracked() == generation {
            signals.recommendations_loading.set(false);
        }
    });
}

fn reset_profile_draft(signals: Signals, profile: Option<&SlottingProfileResponse>) {
    signals
        .mode
        .set(profile.map_or(SlottingAdvisoryMode::Enabled, |row| row.mode));
    signals
        .lookback
        .set(profile.map_or_else(|| "30".into(), |row| row.demand_lookback_days.to_string()));
    signals
        .demand_weight
        .set(profile.map_or_else(|| "100".into(), |row| row.demand_weight.to_string()));
    signals
        .travel_weight
        .set(profile.map_or_else(|| "50".into(), |row| row.travel_weight.to_string()));
    signals
        .activity_weight
        .set(profile.map_or_else(|| "25".into(), |row| row.activity_weight.to_string()));
    signals
        .minimum_demand
        .set(profile.map_or_else(|| "1".into(), |row| row.minimum_demand_quantity.to_string()));
    signals
        .maximum_recommendations
        .set(profile.map_or_else(|| "100".into(), |row| row.max_recommendations.to_string()));
    signals
        .default_priority
        .set(profile.map_or_else(|| "20".into(), |row| row.default_task_priority.to_string()));
}

fn reset_action_draft(signals: Signals) {
    signals.action_priority.set(String::new());
    signals.action_note.set(String::new());
    signals
        .dismissal_reason
        .set(SlottingDismissalReason::OperationalConstraint);
    signals.command_error.set(None);
    signals.retry.set(None);
}

fn close_dialog(signals: Signals) {
    if !signals.command_pending.get_untracked() {
        signals.dialog.set(None);
        signals.command_error.set(None);
        signals.retry.set(None);
    }
}

fn validate_profile(request: &ConfigureSlottingProfileRequest) -> Result<(), String> {
    if !(1..=365).contains(&request.demand_lookback_days) {
        return Err("Demand lookback must be between 1 and 365 days.".into());
    }
    if [
        request.demand_weight,
        request.travel_weight,
        request.activity_weight,
    ]
    .into_iter()
    .any(|value| !(1..=10_000).contains(&value))
    {
        return Err("Every score weight must be between 1 and 10,000.".into());
    }
    if request.minimum_demand_quantity <= 0 {
        return Err("Minimum demand must be positive.".into());
    }
    if !(1..=1_000).contains(&request.max_recommendations) {
        return Err("Maximum recommendations must be between 1 and 1,000.".into());
    }
    Ok(())
}

fn parsed<T: std::str::FromStr>(signal: RwSignal<String>, label: &str) -> Result<T, String> {
    signal
        .get_untracked()
        .trim()
        .parse::<T>()
        .map_err(|_| format!("Enter a valid {label}."))
}

fn optional_parsed<T: std::str::FromStr>(
    signal: RwSignal<String>,
    label: &str,
) -> Result<Option<T>, String> {
    let value = signal.get_untracked();
    if value.trim().is_empty() {
        Ok(None)
    } else {
        value
            .trim()
            .parse::<T>()
            .map(Some)
            .map_err(|_| format!("Enter a valid {label}."))
    }
}

fn optional_note(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn number_input(label: &'static str, signal: RwSignal<String>) -> AnyView {
    view! { <label><span>{label}</span><input type="number" min="0" required prop:value=move || signal.get() on:input=move |event| signal.set(event_target_value(&event))/></label> }.into_any()
}

fn loading_state(label: &'static str) -> AnyView {
    view! { <section class="slotting-state"><span class="loading-line"></span><h2>{label}</h2></section> }.into_any()
}

fn empty_state(title: &'static str, message: &'static str) -> AnyView {
    view! { <section class="slotting-state"><h2>{title}</h2><p>{message}</p></section> }.into_any()
}

fn run_evidence(run: SlottingRunResponse) -> AnyView {
    let configuration = serde_json::to_string_pretty(&run.configuration_snapshot)
        .unwrap_or_else(|_| "Configuration snapshot unavailable".into());
    view! { <section class="slotting-run-evidence"><strong>{format!("Run #{}", run.slotting_run_id)}</strong><span>{format!("{} candidates · {} recommendations", run.candidate_count, run.recommendation_count)}</span><span>{format!("Profile #{} revision {}", run.slotting_profile_id, run.profile_revision.get())}</span><span>{format!("Demand from {} · snapshot {}", short_timestamp(&run.demand_window_started_at), short_timestamp(&run.input_snapshot_at))}</span><span>{format!("Actor #{} · {}", run.generated_by, short_timestamp(&run.generated_at))}</span><details><summary>"Frozen configuration"</summary><pre>{configuration}</pre></details></section> }.into_any()
}

fn scope_options(values: &[AccessScopeResource]) -> AnyView {
    values
        .iter()
        .map(|row| view! { <option value=row.id>{row.name.clone()}</option> })
        .collect_view()
        .into_any()
}

fn scope_name(values: &[AccessScopeResource], id: i64) -> String {
    values
        .iter()
        .find(|row| row.id == id)
        .map_or_else(|| format!("#{id}"), |row| row.name.clone())
}

fn option_id(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}
fn mode_wire(value: SlottingAdvisoryMode) -> &'static str {
    match value {
        SlottingAdvisoryMode::Enabled => "enabled",
        SlottingAdvisoryMode::Disabled => "disabled",
    }
}
fn parse_mode(value: &str) -> SlottingAdvisoryMode {
    if value == "disabled" {
        SlottingAdvisoryMode::Disabled
    } else {
        SlottingAdvisoryMode::Enabled
    }
}
fn mode_label(value: SlottingAdvisoryMode) -> &'static str {
    match value {
        SlottingAdvisoryMode::Enabled => "Enabled",
        SlottingAdvisoryMode::Disabled => "Disabled",
    }
}
fn mode_class(value: SlottingAdvisoryMode) -> &'static str {
    match value {
        SlottingAdvisoryMode::Enabled => "status-badge success",
        SlottingAdvisoryMode::Disabled => "status-badge neutral",
    }
}
fn status_wire(value: Option<SlottingRecommendationStatus>) -> &'static str {
    match value {
        Some(SlottingRecommendationStatus::Pending) => "pending",
        Some(SlottingRecommendationStatus::Accepted) => "accepted",
        Some(SlottingRecommendationStatus::Dismissed) => "dismissed",
        None => "",
    }
}
fn parse_status(value: &str) -> Option<SlottingRecommendationStatus> {
    match value {
        "pending" => Some(SlottingRecommendationStatus::Pending),
        "accepted" => Some(SlottingRecommendationStatus::Accepted),
        "dismissed" => Some(SlottingRecommendationStatus::Dismissed),
        _ => None,
    }
}
fn status_label(value: SlottingRecommendationStatus) -> &'static str {
    match value {
        SlottingRecommendationStatus::Pending => "Pending",
        SlottingRecommendationStatus::Accepted => "Accepted",
        SlottingRecommendationStatus::Dismissed => "Dismissed",
    }
}
fn status_class(value: SlottingRecommendationStatus) -> &'static str {
    match value {
        SlottingRecommendationStatus::Pending => "status-badge warning",
        SlottingRecommendationStatus::Accepted => "status-badge success",
        SlottingRecommendationStatus::Dismissed => "status-badge neutral",
    }
}
fn reason_label(value: SlottingRecommendationReason) -> &'static str {
    match value {
        SlottingRecommendationReason::ForwardPickDemand => "Forward-pick demand",
        SlottingRecommendationReason::TravelReduction => "Travel reduction",
        SlottingRecommendationReason::CapacityRebalance => "Capacity rebalance",
    }
}
fn dismissal_wire(value: SlottingDismissalReason) -> &'static str {
    match value {
        SlottingDismissalReason::CapacityChanged => "capacity_changed",
        SlottingDismissalReason::OperationalConstraint => "operational_constraint",
        SlottingDismissalReason::ItemStrategy => "item_strategy",
        SlottingDismissalReason::StaleEvidence => "stale_evidence",
        SlottingDismissalReason::DuplicateWork => "duplicate_work",
        SlottingDismissalReason::Other => "other",
    }
}
fn parse_dismissal(value: &str) -> SlottingDismissalReason {
    match value {
        "capacity_changed" => SlottingDismissalReason::CapacityChanged,
        "item_strategy" => SlottingDismissalReason::ItemStrategy,
        "stale_evidence" => SlottingDismissalReason::StaleEvidence,
        "duplicate_work" => SlottingDismissalReason::DuplicateWork,
        "other" => SlottingDismissalReason::Other,
        _ => SlottingDismissalReason::OperationalConstraint,
    }
}
fn dismissal_label(value: SlottingDismissalReason) -> &'static str {
    match value {
        SlottingDismissalReason::CapacityChanged => "Capacity changed",
        SlottingDismissalReason::OperationalConstraint => "Operational constraint",
        SlottingDismissalReason::ItemStrategy => "Item strategy",
        SlottingDismissalReason::StaleEvidence => "Stale evidence",
        SlottingDismissalReason::DuplicateWork => "Duplicate work",
        SlottingDismissalReason::Other => "Other",
    }
}
fn dismissal_options() -> AnyView {
    [
        SlottingDismissalReason::CapacityChanged,
        SlottingDismissalReason::OperationalConstraint,
        SlottingDismissalReason::ItemStrategy,
        SlottingDismissalReason::StaleEvidence,
        SlottingDismissalReason::DuplicateWork,
        SlottingDismissalReason::Other,
    ]
    .into_iter()
    .map(|value| view! { <option value=dismissal_wire(value)>{dismissal_label(value)}</option> })
    .collect_view()
    .into_any()
}
fn short_timestamp(value: &str) -> String {
    value
        .replace('T', " ")
        .trim_end_matches('Z')
        .chars()
        .take(19)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::Revision;

    fn request() -> ConfigureSlottingProfileRequest {
        ConfigureSlottingProfileRequest {
            inventory_owner_id: 1,
            facility_id: 2,
            mode: SlottingAdvisoryMode::Enabled,
            demand_lookback_days: 30,
            demand_weight: 100,
            travel_weight: 50,
            activity_weight: 25,
            minimum_demand_quantity: 1,
            max_recommendations: 100,
            default_task_priority: 20,
            expected_revision: Some(Revision::new(3).unwrap()),
        }
    }

    #[test]
    fn profile_validation_matches_domain_bounds() {
        assert!(validate_profile(&request()).is_ok());
        let mut invalid = request();
        invalid.travel_weight = 0;
        assert_eq!(
            validate_profile(&invalid),
            Err("Every score weight must be between 1 and 10,000.".into())
        );
        invalid = request();
        invalid.demand_lookback_days = 366;
        assert_eq!(
            validate_profile(&invalid),
            Err("Demand lookback must be between 1 and 365 days.".into())
        );
    }

    #[test]
    fn decision_labels_are_complete_and_stable() {
        assert_eq!(
            reason_label(SlottingRecommendationReason::TravelReduction),
            "Travel reduction"
        );
        assert_eq!(
            dismissal_wire(SlottingDismissalReason::DuplicateWork),
            "duplicate_work"
        );
        assert_eq!(
            parse_status("accepted"),
            Some(SlottingRecommendationStatus::Accepted)
        );
    }
}
