use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    BackorderPolicyMode, BackorderReason, ConfigureBackorderPolicyRequest,
    OrderAllocationReadinessResponse, OrderAllocationReadinessStatus, SplitOrderBackorderRequest,
};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogMode {
    Policy,
    Split,
}

#[derive(Clone)]
struct PolicyAttempt {
    request: ConfigureBackorderPolicyRequest,
    key: String,
}

#[derive(Clone)]
struct SplitAttempt {
    request: SplitOrderBackorderRequest,
    key: String,
}

#[component]
pub(super) fn BackorderControls(
    order_id: i64,
    readiness: RwSignal<Option<OrderAllocationReadinessResponse>>,
    allocation_pending: RwSignal<bool>,
    release_pending: RwSignal<bool>,
    stream_pending: RwSignal<bool>,
    on_changed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let dialog = RwSignal::new(None::<DialogMode>);
    let policy_mode = RwSignal::new(BackorderPolicyMode::Block);
    let reason = RwSignal::new(BackorderReason::InventoryUnavailable);
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let policy_retry = RwSignal::new(None::<PolicyAttempt>);
    let split_retry = RwSignal::new(None::<SplitAttempt>);
    let toasts = use_toast_bus();

    let open_policy = move |_| {
        let Some(current) = readiness.get_untracked() else {
            return;
        };
        policy_mode.set(
            current
                .backorder_policy
                .map_or(BackorderPolicyMode::Block, |policy| policy.mode),
        );
        error.set(None);
        policy_retry.set(None);
        dialog.set(Some(DialogMode::Policy));
    };
    let open_split = move |_| {
        error.set(None);
        split_retry.set(None);
        reason.set(BackorderReason::InventoryUnavailable);
        note.set(String::new());
        dialog.set(Some(DialogMode::Split));
    };
    let close = move |_| {
        if !pending.get_untracked() {
            dialog.set(None);
            error.set(None);
            policy_retry.set(None);
            split_retry.set(None);
        }
    };

    let submit_policy = move |_| {
        if pending.get_untracked() {
            return;
        }
        let attempt = if let Some(attempt) = policy_retry.get_untracked() {
            attempt
        } else {
            let Some(current) = readiness.get_untracked() else {
                error.set(Some(
                    "Allocation readiness is no longer available.".to_owned(),
                ));
                return;
            };
            PolicyAttempt {
                request: ConfigureBackorderPolicyRequest {
                    inventory_owner_id: current.inventory_owner_id,
                    facility_id: current.facility_id,
                    mode: policy_mode.get_untracked(),
                    expected_revision: current
                        .backorder_policy
                        .as_ref()
                        .map(|policy| policy.revision),
                },
                key: api::new_idempotency_key(),
            }
        };
        policy_retry.set(Some(attempt.clone()));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::configure_backorder_policy(&attempt.request, &attempt.key).await {
                Ok(result) => {
                    pending.set(false);
                    policy_retry.set(None);
                    dialog.set(None);
                    toasts.success(format!(
                        "Backorder policy set to {} at revision {}.",
                        policy_label(result.mode),
                        result.revision.get()
                    ));
                    on_changed.run(result.facility_id);
                }
                Err(api_error) if api_error.unauthorized => {
                    pending.set(false);
                    policy_retry.set(None);
                    dialog.set(None);
                    on_unauthorized.run(());
                }
                Err(api_error) => {
                    pending.set(false);
                    if !api_error.ambiguous_outcome {
                        policy_retry.set(None);
                    }
                    error.set(Some(command_error(
                        api_error.message.clone(),
                        api_error.ambiguous_outcome,
                    )));
                    toasts.error(api_error.message);
                    on_changed.run(attempt.request.facility_id);
                }
            }
        });
    };

    let submit_split = move |_| {
        if pending.get_untracked() {
            return;
        }
        let attempt = if let Some(attempt) = split_retry.get_untracked() {
            attempt
        } else {
            let Some(current) = readiness.get_untracked() else {
                error.set(Some(
                    "Allocation readiness is no longer available.".to_owned(),
                ));
                return;
            };
            let Some(policy) = current.backorder_policy.as_ref() else {
                error.set(Some("Configure a split-shortage policy first.".to_owned()));
                return;
            };
            let note_value = note.get_untracked();
            let note_value = (!note_value.trim().is_empty()).then(|| note_value.trim().to_owned());
            if reason.get_untracked() == BackorderReason::Other && note_value.is_none() {
                error.set(Some("Other requires a note.".to_owned()));
                return;
            }
            SplitAttempt {
                request: SplitOrderBackorderRequest {
                    facility_id: current.facility_id,
                    expected_order_revision: current.revision,
                    expected_policy_revision: policy.revision,
                    reason: reason.get_untracked(),
                    note: note_value,
                },
                key: api::new_idempotency_key(),
            }
        };
        split_retry.set(Some(attempt.clone()));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::split_order_backorder(order_id, &attempt.request, &attempt.key).await {
                Ok(result) => {
                    pending.set(false);
                    split_retry.set(None);
                    dialog.set(None);
                    toasts.success(format!(
                        "Backorder {} created with {} unit(s).",
                        result.child_order_key,
                        format_quantity(result.newly_backordered_quantity)
                    ));
                    on_changed.run(result.facility_id);
                }
                Err(api_error) if api_error.unauthorized => {
                    pending.set(false);
                    split_retry.set(None);
                    dialog.set(None);
                    on_unauthorized.run(());
                }
                Err(api_error) => {
                    pending.set(false);
                    if !api_error.ambiguous_outcome {
                        split_retry.set(None);
                    }
                    error.set(Some(command_error(
                        api_error.message.clone(),
                        api_error.ambiguous_outcome,
                    )));
                    toasts.error(api_error.message);
                    on_changed.run(attempt.request.facility_id);
                }
            }
        });
    };

    let can_split = move || {
        readiness.get().is_some_and(|current| {
            current.status == OrderAllocationReadinessStatus::Ready
                && current.allocated_quantity > 0
                && current.shortage_quantity > 0
                && current
                    .backorder_policy
                    .is_some_and(|policy| policy.mode == BackorderPolicyMode::SplitShortage)
        })
    };

    view! {
        <Show when=move || readiness.get().is_some()>
            <div class="allocation-backorder-toolbar">
                <div>
                    <strong>"Backorder"</strong>
                    <span>{move || policy_summary(readiness.get())}</span>
                </div>
                <button
                    type="button"
                    class="button secondary-action"
                    disabled=move || {
                        pending.get()
                            || allocation_pending.get()
                            || release_pending.get()
                            || stream_pending.get()
                    }
                    on:click=open_policy
                >
                    <Icon icon=UiIcon::Orders/>
                    "Policy"
                </button>
                <Show when=can_split>
                    <button
                        type="button"
                        class="button primary-action"
                        disabled=move || {
                            pending.get()
                                || allocation_pending.get()
                                || release_pending.get()
                                || stream_pending.get()
                        }
                        on:click=open_split
                    >
                        <Icon icon=UiIcon::Release/>
                        {move || readiness.get().map_or_else(
                            || "Backorder short".to_owned(),
                            |state| format!("Backorder {} short", format_quantity(state.shortage_quantity)),
                        )}
                    </button>
                </Show>
            </div>
        </Show>

        <Show when=move || dialog.get().is_some()>
            <div class="allocation-dialog-backdrop">
                <section class="allocation-dialog" role="dialog" aria-modal="true" aria-labelledby="allocation-dialog-title">
                    <header>
                        <div>
                            <span class="eyebrow">"Allocation control"</span>
                            <h2 id="allocation-dialog-title">{move || dialog_title(dialog.get())}</h2>
                        </div>
                        <button type="button" class="icon-button" aria-label="Close" disabled=move || pending.get() on:click=close>
                            <Icon icon=UiIcon::Close/>
                        </button>
                    </header>
                    <div class="allocation-dialog-body">
                        <Show when=move || dialog.get() == Some(DialogMode::Policy)>
                            <label>
                                <span>"Shortage policy"</span>
                                <select
                                    disabled=move || pending.get() || policy_retry.get().is_some()
                                    prop:value=move || policy_value(policy_mode.get())
                                    on:change=move |event| policy_mode.set(parse_policy(&event_target_value(&event)))
                                >
                                    <option value="block">"Block release"</option>
                                    <option value="split_shortage">"Split shortage to child order"</option>
                                </select>
                            </label>
                            <p>"The policy is versioned for this client and facility. Split mode preserves the original reservation while moving only the current shortage to a child order."</p>
                        </Show>
                        <Show when=move || dialog.get() == Some(DialogMode::Split)>
                            <dl class="allocation-dialog-totals">
                                <div><dt>"Original"</dt><dd>{move || readiness.get().map_or(0, |state| state.original_demand_quantity)}</dd></div>
                                <div><dt>"Allocated"</dt><dd>{move || readiness.get().map_or(0, |state| state.allocated_quantity)}</dd></div>
                                <div><dt>"New backorder"</dt><dd>{move || readiness.get().map_or(0, |state| state.shortage_quantity)}</dd></div>
                            </dl>
                            <label>
                                <span>"Reason"</span>
                                <select disabled=move || pending.get() || split_retry.get().is_some() prop:value=move || reason_value(reason.get()) on:change=move |event| reason.set(parse_reason(&event_target_value(&event)))>
                                    <option value="inventory_unavailable">"Inventory unavailable"</option>
                                    <option value="client_requested">"Client requested"</option>
                                    <option value="service_level">"Service level"</option>
                                    <option value="other">"Other"</option>
                                </select>
                            </label>
                            <label>
                                <span>"Note"</span>
                                <textarea maxlength="500" disabled=move || pending.get() || split_retry.get().is_some() prop:value=move || note.get() on:input=move |event| note.set(event_target_value(&event))></textarea>
                            </label>
                        </Show>
                        <Show when=move || error.get().is_some()>
                            <p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
                        </Show>
                        <Show when=move || policy_retry.get().is_some() || split_retry.get().is_some()>
                            <p class="inline-command-note" role="status">"The exact request and idempotency key are retained for an ambiguous retry."</p>
                        </Show>
                    </div>
                    <footer>
                        <button type="button" class="button secondary-action" disabled=move || pending.get() on:click=close>"Cancel"</button>
                        <button type="button" class="button primary-action" disabled=move || pending.get() on:click=move |event| {
                            if dialog.get_untracked() == Some(DialogMode::Policy) { submit_policy(event) } else { submit_split(event) }
                        }>{move || submit_label(dialog.get(), pending.get(), policy_retry.get().is_some() || split_retry.get().is_some())}</button>
                    </footer>
                </section>
            </div>
        </Show>
    }
}

fn policy_summary(readiness: Option<OrderAllocationReadinessResponse>) -> String {
    readiness
        .and_then(|state| state.backorder_policy)
        .map_or_else(
            || "No policy configured".to_owned(),
            |policy| {
                format!(
                    "{} · rev. {}",
                    policy_label(policy.mode),
                    policy.revision.get()
                )
            },
        )
}

const fn policy_label(mode: BackorderPolicyMode) -> &'static str {
    match mode {
        BackorderPolicyMode::Block => "Block release",
        BackorderPolicyMode::SplitShortage => "Split shortage",
    }
}

const fn policy_value(mode: BackorderPolicyMode) -> &'static str {
    match mode {
        BackorderPolicyMode::Block => "block",
        BackorderPolicyMode::SplitShortage => "split_shortage",
    }
}

fn parse_policy(value: &str) -> BackorderPolicyMode {
    if value == "split_shortage" {
        BackorderPolicyMode::SplitShortage
    } else {
        BackorderPolicyMode::Block
    }
}

const fn reason_value(reason: BackorderReason) -> &'static str {
    match reason {
        BackorderReason::InventoryUnavailable => "inventory_unavailable",
        BackorderReason::ClientRequested => "client_requested",
        BackorderReason::ServiceLevel => "service_level",
        BackorderReason::Other => "other",
    }
}

fn parse_reason(value: &str) -> BackorderReason {
    match value {
        "client_requested" => BackorderReason::ClientRequested,
        "service_level" => BackorderReason::ServiceLevel,
        "other" => BackorderReason::Other,
        _ => BackorderReason::InventoryUnavailable,
    }
}

fn dialog_title(mode: Option<DialogMode>) -> &'static str {
    match mode {
        Some(DialogMode::Policy) => "Backorder policy",
        Some(DialogMode::Split) => "Create backorder",
        None => "Backorder",
    }
}

fn submit_label(mode: Option<DialogMode>, pending: bool, retry: bool) -> &'static str {
    if pending {
        "Submitting"
    } else if retry {
        "Retry command"
    } else if mode == Some(DialogMode::Policy) {
        "Save policy"
    } else {
        "Create backorder"
    }
}

fn command_error(message: String, ambiguous: bool) -> String {
    if ambiguous {
        format!("{message} The result is unknown; retry sends the original command.")
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_mappings_are_total_and_other_is_distinct() {
        assert_eq!(
            parse_policy(policy_value(BackorderPolicyMode::SplitShortage)),
            BackorderPolicyMode::SplitShortage
        );
        assert_eq!(
            parse_reason(reason_value(BackorderReason::Other)),
            BackorderReason::Other
        );
    }
}
