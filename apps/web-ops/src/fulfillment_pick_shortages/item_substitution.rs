use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ConfigureItemSubstitutionPolicyRequest, ItemSubstitutionPolicyResponse, ItemSubstitutionReason,
    OrderEntryItemResponse, PickShortageResponse, PickShortageStatus,
    RetireItemSubstitutionPolicyRequest, SubstitutePickShortageRequest,
};

use super::{refresh_after_command, DetailSignals, PendingShortageCommand};
use crate::api;
use crate::components::{Icon, UiIcon};
use crate::view_model::format_quantity;

pub(super) type SubstitutionRetry = (i64, SubstitutePickShortageRequest, String);

pub(super) fn open_substitution(shortage: PickShortageResponse, signals: DetailSignals) {
    let shortage_id = shortage.shortage_id;
    if signals.reallocation_retry.get_untracked().is_some()
        || signals.short_ship_retry.get_untracked().is_some()
        || signals
            .substitution_retry
            .get_untracked()
            .is_some_and(|(retry_id, _, _)| retry_id != shortage_id)
    {
        signals.substitution_error.set(Some(
            "Resolve the retained unknown command result before starting another disposition."
                .to_owned(),
        ));
        signals.substitution_open.set(Some(shortage_id));
        return;
    }
    if !signals
        .substitution_retry
        .get_untracked()
        .is_some_and(|(retry_id, _, _)| retry_id == shortage_id)
    {
        signals.substitution_policy_id.set(String::new());
        signals
            .substitution_reason
            .set("client_authorized".to_owned());
        signals.substitution_note.set(String::new());
    }
    signals.short_ship_open.set(None);
    signals.substitution_error.set(None);
    signals.substitution_open.set(Some(shortage_id));
    request_policies(shortage, signals);
}

#[component]
pub(super) fn ItemSubstitutionConfirmation(
    shortage: PickShortageResponse,
    signals: DetailSignals,
) -> impl IntoView {
    let shortage_id = shortage.shortage_id;
    let open_quantity = shortage.remaining_to_allocate_quantity;
    let confirmation_ref = NodeRef::<leptos::html::Form>::new();
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        let _ = signals.substitution_error.get();
        if let Some(form) = confirmation_ref.get() {
            form.scroll_into_view_with_bool(false);
        }
    });
    let retry_for_shortage = move || {
        signals
            .substitution_retry
            .get()
            .is_some_and(|(retry_id, _, _)| retry_id == shortage_id)
    };
    let pending = move || {
        signals.command_pending.get() == Some(PendingShortageCommand::Substitution(shortage_id))
    };
    let selected_policy = move || {
        let policy_id = signals.substitution_policy_id.get().parse::<i64>().ok()?;
        signals
            .substitution_policies
            .get()
            .into_iter()
            .find(|policy| policy.policy_id == policy_id)
    };
    let conversion = move || {
        selected_policy().map_or_else(
            || "Select an approved rule".to_owned(),
            |policy| substitution_impact(open_quantity, &policy),
        )
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        dispatch_substitution(shortage_id, signals);
    };
    let close = move |_| {
        signals.substitution_error.set(None);
        signals.substitution_open.set(None);
    };
    let policy_manager_shortage = shortage.clone();

    view! {
        <form
            node_ref=confirmation_ref
            class="confirmation-panel substitution-confirmation"
            role="alertdialog"
            aria-labelledby="substitution-title"
            aria-describedby="substitution-consequence substitution-hold"
            on:submit=submit
        >
            <div class="short-ship-heading">
                <Icon icon=UiIcon::Disposition/>
                <div>
                    <h3 id="substitution-title">"Use approved substitute"</h3>
                    <span>{format!("{} / Exception #{}", shortage.order_key, shortage_id)}</span>
                </div>
            </div>
            <dl class="short-ship-impact">
                <div><dt>"Replace open"</dt><dd>{format_quantity(open_quantity)}</dd></div>
                <div><dt>"Source UOM"</dt><dd>{shortage.uom.clone()}</dd></div>
                <div><dt>"Conversion"</dt><dd class="substitution-conversion">{conversion}</dd></div>
            </dl>
            <p id="substitution-consequence">
                "The approved rule creates normal FEFO pick work for the substitute item. The original item identity and accepted substitution remain visible through shipment documents."
            </p>
            <p id="substitution-hold" class="short-ship-hold-warning">
                <Icon icon=UiIcon::Holds/>
                {format!("Discrepancy hold #{} remains active on the suspect source stock.", shortage.hold.hold_id)}
            </p>
            <div class="substitution-fields">
                <label class="substitution-policy-field">
                    <span>"Approved rule"</span>
                    <select
                        disabled=move || pending() || retry_for_shortage() || signals.substitution_loading.get()
                        prop:value=move || signals.substitution_policy_id.get()
                        on:change=move |event| {
                            signals.substitution_policy_id.set(event_target_value(&event));
                            signals.substitution_error.set(None);
                        }
                    >
                        <option value="">{move || if signals.substitution_loading.get() { "Loading rules..." } else { "Select a substitute" }}</option>
                        {move || signals.substitution_policies.get().into_iter().map(|policy| {
                            let policy_id = policy.policy_id.to_string();
                            let label = policy_label(&policy);
                            view! { <option value=policy_id>{label}</option> }
                        }).collect_view()}
                    </select>
                </label>
                <label>
                    <span>"Reason"</span>
                    <select
                        disabled=move || pending() || retry_for_shortage()
                        prop:value=move || signals.substitution_reason.get()
                        on:change=move |event| {
                            signals.substitution_reason.set(event_target_value(&event));
                            signals.substitution_error.set(None);
                        }
                    >
                        <option value="client_authorized">"Client authorized"</option>
                        <option value="inventory_unavailable">"Inventory unavailable"</option>
                        <option value="service_recovery">"Service recovery"</option>
                        <option value="other">"Other"</option>
                    </select>
                </label>
                <label class="substitution-note-field">
                    <span>{move || if signals.substitution_reason.get() == "other" { "Note (required)" } else { "Note (optional)" }}</span>
                    <textarea
                        maxlength="500"
                        rows="2"
                        aria-required=move || (signals.substitution_reason.get() == "other").to_string()
                        disabled=move || pending() || retry_for_shortage()
                        prop:value=move || signals.substitution_note.get()
                        on:input=move |event| {
                            signals.substitution_note.set(event_target_value(&event));
                            signals.substitution_error.set(None);
                        }
                    ></textarea>
                </label>
            </div>
            <SubstitutionPolicyManager shortage=policy_manager_shortage signals/>
            <Show when=retry_for_shortage>
                <p class="shortage-retry-note" role="status">
                    "The original request is retained. Retry sends the exact request and idempotency key."
                </p>
            </Show>
            <Show when=move || signals.substitution_error.get().is_some()>
                <p class="inline-command-error" role="alert">
                    {move || signals.substitution_error.get().unwrap_or_default()}
                </p>
            </Show>
            <div class="form-actions">
                <button
                    type="submit"
                    class="button primary-action"
                    disabled=move || pending() || signals.substitution_policy_command_pending.get() || (!retry_for_shortage() && signals.substitution_loading.get())
                >
                    <Icon icon=UiIcon::Disposition/>
                    {move || if pending() { "Substituting" } else if retry_for_shortage() { "Retry substitution" } else { "Use substitute" }}
                </button>
                <button
                    type="button"
                    class="button secondary-action"
                    disabled=move || pending() || retry_for_shortage()
                    on:click=close
                >
                    "Keep recovering"
                </button>
            </div>
        </form>
    }
}

#[component]
fn SubstitutionPolicyManager(
    shortage: PickShortageResponse,
    signals: DetailSignals,
) -> impl IntoView {
    let search = RwSignal::new(String::new());
    let items = RwSignal::new(Vec::<OrderEntryItemResponse>::new());
    let selected_item_id = RwSignal::new(String::new());
    let source_quantity = RwSignal::new("1".to_owned());
    let substitute_quantity = RwSignal::new("1".to_owned());
    let searching = RwSignal::new(false);
    let pending = signals.substitution_policy_command_pending;
    let error = RwSignal::new(None::<String>);
    let configure_retry = RwSignal::new(None::<(ConfigureItemSubstitutionPolicyRequest, String)>);
    let retire_retry = RwSignal::new(None::<(i64, RetireItemSubstitutionPolicyRequest, String)>);
    let owner_id = shortage.inventory_owner_id;
    let source_uom = shortage.uom.clone();
    let retirement_shortage = StoredValue::new(shortage.clone());
    let search_items = move |_| {
        if searching.get_untracked() || pending.get_untracked() {
            return;
        }
        searching.set(true);
        error.set(None);
        let query = search.get_untracked();
        leptos::task::spawn_local(async move {
            match api::order_entry_items(owner_id, &query).await {
                Ok(next) => {
                    selected_item_id.set(
                        next.first()
                            .map(|item| item.item_id.to_string())
                            .unwrap_or_default(),
                    );
                    items.set(next);
                }
                Err(api_error) if api_error.unauthorized => signals.on_unauthorized.run(()),
                Err(api_error) => error.set(Some(api_error.message)),
            }
            searching.set(false);
        });
    };
    let configure_shortage = shortage.clone();
    let configure = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        dispatch_policy_configuration(
            configure_shortage.clone(),
            signals,
            items,
            selected_item_id,
            source_quantity,
            substitute_quantity,
            pending,
            error,
            configure_retry,
            retire_retry,
        );
    };
    let policy_count = move || signals.substitution_policies.get().len();

    view! {
        <details class="substitution-rule-manager">
            <summary>
                <span>"Approved rule management"</span>
                <strong>{move || format!("{} active", policy_count())}</strong>
            </summary>
            <div class="substitution-rule-list">
                <Show
                    when=move || !signals.substitution_policies.get().is_empty()
                    fallback=|| view! { <p class="shortage-action-state">"No active rule is configured for this source item."</p> }
                >
                    {move || signals.substitution_policies.get().into_iter().map(|policy| {
                        let policy_id = policy.policy_id;
                        let retrying = move || retire_retry.get().is_some_and(|(retry_id, _, _)| retry_id == policy_id);
                        let retire_policy = policy.clone();
                        let retire_shortage = retirement_shortage.get_value();
                        view! {
                            <div class="substitution-rule-row">
                                <span>{policy_label(&policy)}</span>
                                <button
                                    type="button"
                                    class="icon-button"
                                    title="Retire substitution rule"
                                    aria-label=format!("Retire substitution rule #{}", policy.policy_id)
                                    disabled=move || pending.get() || (retire_retry.get().is_some() && !retrying()) || configure_retry.get().is_some()
                                    on:click=move |_| dispatch_policy_retirement(
                                        retire_policy.clone(),
                                        retire_shortage.clone(),
                                        signals,
                                        pending,
                                        error,
                                        configure_retry,
                                        retire_retry,
                                    )
                                >
                                    <Icon icon=UiIcon::Remove/>
                                </button>
                            </div>
                        }
                    }).collect_view()}
                </Show>
            </div>
            <form class="substitution-rule-form" on:submit=configure>
                <div class="substitution-item-search">
                    <label>
                        <span>"Find substitute item"</span>
                        <input
                            type="search"
                            placeholder="Description or item"
                            disabled=move || pending.get() || configure_retry.get().is_some()
                            prop:value=move || search.get()
                            on:input=move |event| search.set(event_target_value(&event))
                        />
                    </label>
                    <button
                        type="button"
                        class="button secondary-action"
                        disabled=move || searching.get() || pending.get() || configure_retry.get().is_some()
                        on:click=search_items
                    >
                        <Icon icon=UiIcon::Search/>
                        {move || if searching.get() { "Searching" } else { "Find" }}
                    </button>
                </div>
                <label class="substitution-item-result">
                    <span>"Substitute item / UOM"</span>
                    <select
                        disabled=move || pending.get() || configure_retry.get().is_some()
                        prop:value=move || selected_item_id.get()
                        on:change=move |event| selected_item_id.set(event_target_value(&event))
                    >
                        <option value="">"Select an item"</option>
                        {move || items.get().into_iter().map(|item| {
                            let label = format!(
                                "{} / {}",
                                item.description.unwrap_or_else(|| format!("Item #{}", item.item_id)),
                                item.requested_uom,
                            );
                            view! { <option value=item.item_id.to_string()>{label}</option> }
                        }).collect_view()}
                    </select>
                </label>
                <label>
                    <span>{format!("Source quantity ({source_uom})")}</span>
                    <input
                        type="number"
                        min="1"
                        step="1"
                        disabled=move || pending.get() || configure_retry.get().is_some()
                        prop:value=move || source_quantity.get()
                        on:input=move |event| source_quantity.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Substitute quantity"</span>
                    <input
                        type="number"
                        min="1"
                        step="1"
                        disabled=move || pending.get() || configure_retry.get().is_some()
                        prop:value=move || substitute_quantity.get()
                        on:input=move |event| substitute_quantity.set(event_target_value(&event))
                    />
                </label>
                <button
                    type="submit"
                    class="button secondary-action"
                    disabled=move || pending.get() || retire_retry.get().is_some()
                >
                    <Icon icon=UiIcon::Add/>
                    {move || if pending.get() { "Saving" } else if configure_retry.get().is_some() { "Retry rule save" } else { "Save rule" }}
                </button>
            </form>
            <Show when=move || error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
        </details>
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch_policy_configuration(
    shortage: PickShortageResponse,
    signals: DetailSignals,
    items: RwSignal<Vec<OrderEntryItemResponse>>,
    selected_item_id: RwSignal<String>,
    source_quantity: RwSignal<String>,
    substitute_quantity: RwSignal<String>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    configure_retry: RwSignal<Option<(ConfigureItemSubstitutionPolicyRequest, String)>>,
    retire_retry: RwSignal<Option<(i64, RetireItemSubstitutionPolicyRequest, String)>>,
) {
    if pending.get_untracked() || retire_retry.get_untracked().is_some() {
        return;
    }
    let (request, idempotency_key) = match configure_retry.get_untracked() {
        Some(retry) => retry,
        None => {
            let Some(item_id) = positive_number(&selected_item_id.get_untracked()) else {
                error.set(Some("Select a substitute item.".to_owned()));
                return;
            };
            let Some(item) = items
                .get_untracked()
                .into_iter()
                .find(|item| item.item_id == item_id)
            else {
                error.set(Some("Refresh and select a substitute item.".to_owned()));
                return;
            };
            let Some(source_quantity) = positive_number(&source_quantity.get_untracked()) else {
                error.set(Some(
                    "Source quantity must be a positive whole number.".to_owned(),
                ));
                return;
            };
            let Some(substitute_quantity) = positive_number(&substitute_quantity.get_untracked())
            else {
                error.set(Some(
                    "Substitute quantity must be a positive whole number.".to_owned(),
                ));
                return;
            };
            if item.item_id == shortage.item_id && item.requested_uom == shortage.uom {
                error.set(Some(
                    "The substitute must use a different item or UOM.".to_owned(),
                ));
                return;
            }
            let expected_revision = signals
                .substitution_policies
                .get_untracked()
                .into_iter()
                .find(|policy| {
                    policy.substitute_item_id == item.item_id
                        && policy.substitute_uom == item.requested_uom
                })
                .map(|policy| policy.revision);
            (
                ConfigureItemSubstitutionPolicyRequest {
                    inventory_owner_id: shortage.inventory_owner_id,
                    facility_id: shortage.facility_id,
                    source_item_id: shortage.item_id,
                    source_uom: shortage.uom.clone(),
                    substitute_item_id: item.item_id,
                    substitute_uom: item.requested_uom,
                    source_quantity,
                    substitute_quantity,
                    expected_revision,
                },
                api::new_idempotency_key(),
            )
        }
    };
    configure_retry.set(Some((request.clone(), idempotency_key.clone())));
    pending.set(true);
    error.set(None);
    leptos::task::spawn_local(async move {
        let response = api::configure_item_substitution_policy(&request, &idempotency_key).await;
        pending.set(false);
        match response {
            Ok(policy) => {
                configure_retry.set(None);
                signals.toasts.success(format!(
                    "Approved substitution rule #{} revision {}.",
                    policy.policy_id,
                    policy.revision.get()
                ));
                request_policies(shortage, signals);
            }
            Err(api_error) if api_error.unauthorized => {
                configure_retry.set(None);
                signals.on_unauthorized.run(());
            }
            Err(api_error) if api_error.ambiguous_outcome => {
                error.set(Some(format!(
                    "{} The result is unknown; retry the exact saved rule command.",
                    api_error.message
                )));
            }
            Err(api_error) => {
                configure_retry.set(None);
                error.set(Some(api_error.message));
                request_policies(shortage, signals);
            }
        }
    });
}

fn dispatch_policy_retirement(
    policy: ItemSubstitutionPolicyResponse,
    shortage: PickShortageResponse,
    signals: DetailSignals,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    configure_retry: RwSignal<Option<(ConfigureItemSubstitutionPolicyRequest, String)>>,
    retire_retry: RwSignal<Option<(i64, RetireItemSubstitutionPolicyRequest, String)>>,
) {
    if pending.get_untracked() || configure_retry.get_untracked().is_some() {
        return;
    }
    let (policy_id, request, idempotency_key) = match retire_retry.get_untracked() {
        Some(retry) if retry.0 == policy.policy_id => retry,
        Some((retry_id, _, _)) => {
            error.set(Some(format!(
                "Resolve the unknown retirement for rule #{retry_id} first."
            )));
            return;
        }
        None => (
            policy.policy_id,
            RetireItemSubstitutionPolicyRequest {
                expected_revision: policy.revision,
            },
            api::new_idempotency_key(),
        ),
    };
    retire_retry.set(Some((policy_id, request, idempotency_key.clone())));
    pending.set(true);
    error.set(None);
    leptos::task::spawn_local(async move {
        let response =
            api::retire_item_substitution_policy(policy_id, &request, &idempotency_key).await;
        pending.set(false);
        match response {
            Ok(_) => {
                retire_retry.set(None);
                signals
                    .toasts
                    .success(format!("Retired substitution rule #{policy_id}."));
                request_policies(shortage, signals);
            }
            Err(api_error) if api_error.unauthorized => {
                retire_retry.set(None);
                signals.on_unauthorized.run(());
            }
            Err(api_error) if api_error.ambiguous_outcome => {
                error.set(Some(format!(
                    "{} The result is unknown; retry this exact retirement.",
                    api_error.message
                )));
            }
            Err(api_error) => {
                retire_retry.set(None);
                error.set(Some(api_error.message));
                request_policies(shortage, signals);
            }
        }
    });
}

fn request_policies(shortage: PickShortageResponse, signals: DetailSignals) {
    signals
        .substitution_generation
        .update(|generation| *generation = generation.saturating_add(1));
    let generation = signals.substitution_generation.get_untracked();
    signals.substitution_loading.set(true);
    signals.substitution_error.set(None);
    leptos::task::spawn_local(async move {
        let response = api::item_substitution_policies(
            shortage.inventory_owner_id,
            shortage.facility_id,
            shortage.item_id,
            true,
        )
        .await;
        if signals.substitution_generation.get_untracked() != generation
            || signals.substitution_open.get_untracked() != Some(shortage.shortage_id)
        {
            return;
        }
        signals.substitution_loading.set(false);
        match response {
            Ok(policies) => {
                if !signals
                    .substitution_retry
                    .get_untracked()
                    .is_some_and(|(retry_id, _, _)| retry_id == shortage.shortage_id)
                {
                    signals.substitution_policy_id.set(
                        policies
                            .first()
                            .map(|policy| policy.policy_id.to_string())
                            .unwrap_or_default(),
                    );
                }
                if policies.is_empty() {
                    signals.substitution_error.set(Some(
                        "No active substitution rule is configured for this item, client, and facility."
                            .to_owned(),
                    ));
                }
                signals.substitution_policies.set(policies);
            }
            Err(api_error) if api_error.unauthorized => signals.on_unauthorized.run(()),
            Err(api_error) => signals.substitution_error.set(Some(api_error.message)),
        }
    });
}

fn dispatch_substitution(shortage_id: i64, signals: DetailSignals) {
    if signals.command_pending.get_untracked().is_some()
        || signals.substitution_policy_command_pending.get_untracked()
    {
        return;
    }
    if let Some((retry_id, _, _)) = signals.reallocation_retry.get_untracked() {
        signals.substitution_error.set(Some(format!(
            "Resolve the unknown recovery result for pick exception #{retry_id} first."
        )));
        return;
    }
    if let Some((retry_id, _, _)) = signals.short_ship_retry.get_untracked() {
        signals.substitution_error.set(Some(format!(
            "Resolve the unknown short-shipment result for pick exception #{retry_id} first."
        )));
        return;
    }
    let (request, idempotency_key) = match signals.substitution_retry.get_untracked() {
        Some((retry_id, request, key)) if retry_id == shortage_id => (request, key),
        Some((retry_id, _, _)) => {
            signals.substitution_error.set(Some(format!(
                "Resolve the unknown substitution result for pick exception #{retry_id} first."
            )));
            return;
        }
        None => {
            let Some(shortage) = signals.selected.get_untracked() else {
                return;
            };
            if shortage.shortage_id != shortage_id
                || shortage.status != PickShortageStatus::AwaitingInventory
                || shortage.remaining_to_allocate_quantity <= 0
                || shortage.reallocated_quantity != shortage.recovery_terminal_quantity
            {
                signals.substitution_error.set(Some(
                    "This exception is no longer eligible for substitution. Refresh it.".to_owned(),
                ));
                return;
            }
            let policy_id = match signals
                .substitution_policy_id
                .get_untracked()
                .parse::<i64>()
                .ok()
                .filter(|value| *value > 0)
            {
                Some(policy_id) => policy_id,
                None => {
                    signals
                        .substitution_error
                        .set(Some("Select an approved substitution rule.".to_owned()));
                    return;
                }
            };
            let Some(policy) = signals
                .substitution_policies
                .get_untracked()
                .into_iter()
                .find(|policy| policy.policy_id == policy_id)
            else {
                signals
                    .substitution_error
                    .set(Some("Refresh the approved substitution rules.".to_owned()));
                return;
            };
            let Some(reason) = parse_reason(&signals.substitution_reason.get_untracked()) else {
                signals
                    .substitution_error
                    .set(Some("Select a substitution reason.".to_owned()));
                return;
            };
            let note = match validate_note(reason, &signals.substitution_note.get_untracked()) {
                Ok(note) => note,
                Err(message) => {
                    signals.substitution_error.set(Some(message));
                    return;
                }
            };
            (
                SubstitutePickShortageRequest {
                    policy_id,
                    expected_policy_revision: policy.revision,
                    expected_shortage_revision: shortage.shortage_revision,
                    expected_order_revision: shortage.order_revision,
                    reason,
                    note,
                },
                api::new_idempotency_key(),
            )
        }
    };
    signals.substitution_retry.set(Some((
        shortage_id,
        request.clone(),
        idempotency_key.clone(),
    )));
    signals
        .command_pending
        .set(Some(PendingShortageCommand::Substitution(shortage_id)));
    signals.substitution_error.set(None);

    leptos::task::spawn_local(async move {
        let response = api::substitute_pick_shortage(shortage_id, &request, &idempotency_key).await;
        if signals.command_pending.get_untracked()
            == Some(PendingShortageCommand::Substitution(shortage_id))
        {
            signals.command_pending.set(None);
        }
        match response {
            Ok(result) => {
                signals.substitution_retry.set(None);
                signals.substitution_open.set(None);
                signals.substitution_error.set(None);
                signals.toasts.success(format!(
                    "Substituted {} source units with {} {}. {} pick task{} created.",
                    format_quantity(result.accepted_source_quantity),
                    format_quantity(result.substitute_quantity),
                    result.substitute_uom,
                    result.work.len(),
                    if result.work.len() == 1 { "" } else { "s" },
                ));
                refresh_after_command(shortage_id, signals);
            }
            Err(api_error) if api_error.unauthorized => {
                signals.substitution_retry.set(None);
                signals.on_unauthorized.run(());
            }
            Err(api_error) if api_error.ambiguous_outcome => {
                signals.substitution_error.set(Some(format!(
                    "{} The result is unknown; retry the saved substitution command.",
                    api_error.message
                )));
                signals.toasts.error(api_error.message);
            }
            Err(api_error) => {
                signals.substitution_retry.set(None);
                signals.substitution_error.set(Some(format!(
                    "{} Authoritative policy and shortage revisions were refreshed.",
                    api_error.message
                )));
                signals.toasts.error(api_error.message);
                refresh_after_command(shortage_id, signals);
            }
        }
    });
}

fn parse_reason(value: &str) -> Option<ItemSubstitutionReason> {
    match value {
        "client_authorized" => Some(ItemSubstitutionReason::ClientAuthorized),
        "inventory_unavailable" => Some(ItemSubstitutionReason::InventoryUnavailable),
        "service_recovery" => Some(ItemSubstitutionReason::ServiceRecovery),
        "other" => Some(ItemSubstitutionReason::Other),
        _ => None,
    }
}

fn positive_number(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|value| *value > 0)
}

fn validate_note(reason: ItemSubstitutionReason, value: &str) -> Result<Option<String>, String> {
    let note = value.trim();
    if reason == ItemSubstitutionReason::Other && note.is_empty() {
        return Err("Add a note when the substitution reason is Other.".to_owned());
    }
    if note.chars().count() > 500 || note.chars().any(char::is_control) {
        return Err("Substitution notes must be 500 printable characters or fewer.".to_owned());
    }
    Ok((!note.is_empty()).then(|| note.to_owned()))
}

fn policy_label(policy: &ItemSubstitutionPolicyResponse) -> String {
    format!(
        "Item #{} / {} {} -> {} {} (rev {})",
        policy.substitute_item_id,
        policy.source_quantity,
        policy.source_uom,
        policy.substitute_quantity,
        policy.substitute_uom,
        policy.revision.get(),
    )
}

fn substitution_impact(open_quantity: i64, policy: &ItemSubstitutionPolicyResponse) -> String {
    open_quantity
        .checked_div(policy.source_quantity)
        .and_then(|factor| factor.checked_mul(policy.substitute_quantity))
        .filter(|_| open_quantity % policy.source_quantity == 0)
        .map_or_else(
            || "Not an exact conversion".to_owned(),
            |quantity| format!("{} {}", format_quantity(quantity), policy.substitute_uom),
        )
}

#[cfg(test)]
mod tests {
    use wareboxes_api_contract::v1::Revision;

    use super::*;

    fn policy(source_quantity: i64, substitute_quantity: i64) -> ItemSubstitutionPolicyResponse {
        ItemSubstitutionPolicyResponse {
            policy_id: 1,
            inventory_owner_id: 2,
            facility_id: 3,
            source_item_id: 4,
            source_uom: "case".to_owned(),
            substitute_item_id: 5,
            substitute_uom: "each".to_owned(),
            source_quantity,
            substitute_quantity,
            revision: Revision::new(1).unwrap(),
            active: true,
            configured_by: 6,
            configured_at: "2026-08-09T00:00:00Z".to_owned(),
            retired_by: None,
            retired_at: None,
        }
    }

    #[test]
    fn reasons_notes_and_conversion_fail_closed() {
        assert_eq!(
            parse_reason("service_recovery"),
            Some(ItemSubstitutionReason::ServiceRecovery)
        );
        assert_eq!(parse_reason("override"), None);
        assert!(validate_note(ItemSubstitutionReason::Other, " ").is_err());
        assert_eq!(
            validate_note(ItemSubstitutionReason::ClientAuthorized, " approved ").unwrap(),
            Some("approved".to_owned())
        );
        assert_eq!(substitution_impact(4, &policy(2, 3)), "6 each");
        assert_eq!(
            substitution_impact(3, &policy(2, 3)),
            "Not an exact conversion"
        );
    }
}
