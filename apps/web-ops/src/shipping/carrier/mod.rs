use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CancelCarrierManifestRequest, CarrierAccountResponse, CarrierAccountStatus,
    CarrierManifestJobResponse, CarrierManifestJobStatus, ChangeCarrierAccountStatusRequest,
    CreateCarrierAccountRequest, OpaqueCursor, QueueCarrierManifestRequest,
    ReconfigureCarrierAccountRequest, RetryCarrierManifestRequest, Revision,
};

use crate::api;

const PAGE_SIZE: u16 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingCarrierAction {
    Queue(QueueCarrierManifestRequest, String),
    Cancel(CarrierManifestJobResponse, String),
    Retry(CarrierManifestJobResponse, String),
    Create(CreateCarrierAccountRequest, String),
    Reconfigure(i64, ReconfigureCarrierAccountRequest, String),
    Status(i64, ChangeCarrierAccountStatusRequest, String),
}

#[derive(Clone, Copy)]
struct CarrierSignals {
    accounts: RwSignal<Vec<CarrierAccountResponse>>,
    account_cursor: RwSignal<Option<OpaqueCursor>>,
    jobs: RwSignal<Vec<CarrierManifestJobResponse>>,
    job_cursor: RwSignal<Option<OpaqueCursor>>,
    selected_account: RwSignal<Option<i64>>,
    service_code: RwSignal<String>,
    loading: RwSignal<bool>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    retry_exact: RwSignal<Option<PendingCarrierAction>>,
    generation: RwSignal<u64>,
    notified: RwSignal<bool>,
    editing_account: RwSignal<Option<i64>>,
    display_name: RwSignal<String>,
    carrier_code: RwSignal<String>,
    account_key: RwSignal<String>,
}

#[component]
#[allow(clippy::too_many_arguments)]
pub(super) fn CarrierManifestPanel(
    shipment_id: i64,
    order_id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    shipment_revision: Revision,
    can_manage: bool,
    can_retry: bool,
    on_manifested: Callback<(i64, i64)>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let signals = CarrierSignals {
        accounts: RwSignal::new(Vec::new()),
        account_cursor: RwSignal::new(None),
        jobs: RwSignal::new(Vec::new()),
        job_cursor: RwSignal::new(None),
        selected_account: RwSignal::new(None),
        service_code: RwSignal::new(String::new()),
        loading: RwSignal::new(true),
        pending: RwSignal::new(false),
        error: RwSignal::new(None),
        retry_exact: RwSignal::new(None),
        generation: RwSignal::new(0),
        notified: RwSignal::new(false),
        editing_account: RwSignal::new(None),
        display_name: RwSignal::new(String::new()),
        carrier_code: RwSignal::new(String::new()),
        account_key: RwSignal::new(String::new()),
    };

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        load_workspace(
            shipment_id,
            inventory_owner_id,
            facility_id,
            can_manage,
            signals,
            on_manifested,
            order_id,
            on_unauthorized,
        );
    });

    let dispatch = Callback::new(move |action: PendingCarrierAction| {
        dispatch_action(
            shipment_id,
            action,
            signals,
            on_manifested,
            order_id,
            on_unauthorized,
        );
    });
    let queue = move |_| {
        let Some(account_id) = signals.selected_account.get_untracked() else {
            signals
                .error
                .set(Some("Select an active carrier account.".to_owned()));
            return;
        };
        dispatch.run(PendingCarrierAction::Queue(
            QueueCarrierManifestRequest {
                account_id,
                service_code: optional_text(&signals.service_code.get_untracked()),
                expected_shipment_revision: shipment_revision,
            },
            api::new_idempotency_key(),
        ));
    };
    let retry_exact = move |_| {
        if let Some(action) = signals.retry_exact.get_untracked() {
            dispatch.run(action);
        }
    };
    let save_account = move |_| {
        let display_name = signals.display_name.get_untracked().trim().to_owned();
        let account_key = signals.account_key.get_untracked().trim().to_owned();
        if display_name.is_empty() || account_key.is_empty() {
            signals.error.set(Some(
                "Account display name and gateway account key are required.".to_owned(),
            ));
            return;
        }
        if let Some(account_id) = signals.editing_account.get_untracked() {
            let Some(account) = signals
                .accounts
                .get_untracked()
                .into_iter()
                .find(|account| account.account_id == account_id)
            else {
                return;
            };
            dispatch.run(PendingCarrierAction::Reconfigure(
                account_id,
                ReconfigureCarrierAccountRequest {
                    display_name,
                    account_key,
                    expected_revision: account.revision,
                },
                api::new_idempotency_key(),
            ));
        } else {
            let carrier_code = signals.carrier_code.get_untracked().trim().to_owned();
            if carrier_code.is_empty() {
                signals
                    .error
                    .set(Some("Carrier code is required.".to_owned()));
                return;
            }
            dispatch.run(PendingCarrierAction::Create(
                CreateCarrierAccountRequest {
                    inventory_owner_id,
                    facility_id,
                    display_name,
                    carrier_code,
                    account_key,
                },
                api::new_idempotency_key(),
            ));
        }
    };
    let new_account = move |_| clear_account_editor(signals);
    let load_more_accounts = move |_| {
        let Some(cursor) = signals.account_cursor.get_untracked() else {
            return;
        };
        load_accounts(
            inventory_owner_id,
            facility_id,
            can_manage,
            Some(cursor),
            true,
            signals,
            on_unauthorized,
        );
    };
    let load_more_jobs = move |_| {
        let Some(cursor) = signals.job_cursor.get_untracked() else {
            return;
        };
        load_jobs(
            shipment_id,
            Some(cursor),
            true,
            signals,
            on_manifested,
            order_id,
            on_unauthorized,
        );
    };

    view! {
        <section class="carrier-manifest-panel">
            <header>
                <div><h3>"Carrier gateway"</h3><span>"Replay-safe labels and tracking"</span></div>
                <button type="button" class="button secondary-action compact" disabled=move || signals.loading.get() || signals.pending.get() on:click=move |_| {
                    load_workspace(shipment_id, inventory_owner_id, facility_id, can_manage, signals, on_manifested, order_id, on_unauthorized)
                }>"Refresh"</button>
            </header>
            <Show when=move || signals.error.get().is_some()>
                <div class="carrier-error" role="alert">
                    <span>{move || signals.error.get().unwrap_or_default()}</span>
                    <Show when=move || signals.retry_exact.get().is_some()>
                        <button type="button" class="button secondary-action compact" disabled=move || signals.pending.get() on:click=retry_exact>"Retry exact command"</button>
                    </Show>
                </div>
            </Show>
            <div class="carrier-queue-form">
                <label><span>"Account"</span><select
                    prop:value=move || signals.selected_account.get().map_or_else(String::new, |id| id.to_string())
                    disabled=move || signals.pending.get()
                    on:change=move |event| signals.selected_account.set(parse_id(&event_target_value(&event)))
                >
                    <option value="">"Choose active account"</option>
                    {move || signals.accounts.get().into_iter().filter(|account| account.status == CarrierAccountStatus::Active).map(|account| view! {
                        <option value=account.account_id.to_string()>{format!("{} · {}", account.display_name, account.carrier_code)}</option>
                    }).collect_view()}
                </select></label>
                <label><span>"Service"</span><input maxlength="100" placeholder="GROUND / NEXT_DAY" prop:value=move || signals.service_code.get() disabled=move || signals.pending.get() on:input=move |event| signals.service_code.set(event_target_value(&event))/></label>
                <button type="button" class="button primary-action" disabled=move || signals.pending.get() || active_job(&signals.jobs.get()).is_some() || signals.selected_account.get().is_none() on:click=queue>
                    {move || if signals.pending.get() { "Submitting..." } else { "Request carrier manifest" }}
                </button>
            </div>
            <Show when=move || signals.accounts.get().iter().all(|account| account.status != CarrierAccountStatus::Active) && !signals.loading.get()>
                <p class="carrier-empty">"No active account is configured for this client and facility."</p>
            </Show>
            <div class="carrier-job-history" aria-label="Carrier manifest history">
                <For each=move || signals.jobs.get() key=|job| job.job_id children=move |job| {
                    let cancel_job = StoredValue::new(job.clone());
                    let retry_job = StoredValue::new(job.clone());
                    let cancellable = matches!(job.status, CarrierManifestJobStatus::Queued | CarrierManifestJobStatus::RetryScheduled);
                    let retryable = job.status == CarrierManifestJobStatus::Failed && can_retry;
                    view! {
                        <article class="carrier-job">
                            <div><strong>{job_status_label(job.status)}</strong><span>{format!("attempts {} · rev {}", job.attempt_count, job.revision.get())}</span></div>
                            <code>{job.request_key.clone()}</code>
                            <small>{job.manifest_reference.clone().unwrap_or_else(|| job.last_error_message.clone().unwrap_or_else(|| job.requested_at.clone()))}</small>
                            <Show when=move || cancellable><button type="button" class="button secondary-action compact" disabled=move || signals.pending.get() on:click=move |_| dispatch.run(PendingCarrierAction::Cancel(cancel_job.get_value(), api::new_idempotency_key()))>"Cancel request"</button></Show>
                            <Show when=move || retryable><button type="button" class="button secondary-action compact" disabled=move || signals.pending.get() on:click=move |_| dispatch.run(PendingCarrierAction::Retry(retry_job.get_value(), api::new_idempotency_key()))>"Retry failed request"</button></Show>
                        </article>
                    }
                }/>
                <Show when=move || signals.job_cursor.get().is_some()><button type="button" class="button secondary-action compact" disabled=move || signals.loading.get() on:click=load_more_jobs>"Load older attempts"</button></Show>
            </div>
            <Show when=move || can_manage>
                <details class="carrier-account-manager">
                    <summary>"Carrier account configuration"</summary>
                    <div class="carrier-account-list">
                        <For each=move || signals.accounts.get() key=|account| account.account_id children=move |account| {
                            let edit = account.clone();
                            let status_account = account.clone();
                            let next_status = if account.status == CarrierAccountStatus::Active { CarrierAccountStatus::Disabled } else { CarrierAccountStatus::Active };
                            view! {
                                <div><span><strong>{account.display_name}</strong><small>{format!("{} · rev {} · {}", account.carrier_code, account.revision.get(), account_status_label(account.status))}</small></span>
                                    <button type="button" class="button secondary-action compact" on:click=move |_| edit_account(signals, &edit)>"Edit"</button>
                                    <button type="button" class="button secondary-action compact" on:click=move |_| dispatch.run(PendingCarrierAction::Status(status_account.account_id, ChangeCarrierAccountStatusRequest { status: next_status, expected_revision: status_account.revision }, api::new_idempotency_key()))>{if next_status == CarrierAccountStatus::Active { "Enable" } else { "Disable" }}</button>
                                </div>
                            }
                        }/>
                        <Show when=move || signals.account_cursor.get().is_some()><button type="button" class="button secondary-action compact" on:click=load_more_accounts>"Load more accounts"</button></Show>
                    </div>
                    <div class="carrier-account-form">
                        <label><span>"Display name"</span><input maxlength="200" prop:value=move || signals.display_name.get() on:input=move |event| signals.display_name.set(event_target_value(&event))/></label>
                        <label><span>"Carrier code"</span><input maxlength="100" disabled=move || signals.editing_account.get().is_some() prop:value=move || signals.carrier_code.get() on:input=move |event| signals.carrier_code.set(event_target_value(&event))/></label>
                        <label><span>"Gateway account key (non-secret)"</span><input maxlength="200" prop:value=move || signals.account_key.get() on:input=move |event| signals.account_key.set(event_target_value(&event))/></label>
                        <button type="button" class="button primary-action compact" disabled=move || signals.pending.get() on:click=save_account>{move || if signals.editing_account.get().is_some() { "Save revision" } else { "Create account" }}</button>
                        <button type="button" class="button secondary-action compact" on:click=new_account>"New account"</button>
                    </div>
                </details>
            </Show>
        </section>
    }
}

#[allow(clippy::too_many_arguments)]
fn load_workspace(
    shipment_id: i64,
    owner_id: i64,
    facility_id: i64,
    include_disabled: bool,
    signals: CarrierSignals,
    on_manifested: Callback<(i64, i64)>,
    order_id: i64,
    on_unauthorized: Callback<()>,
) {
    signals
        .generation
        .update(|value| *value = value.saturating_add(1));
    signals.loading.set(true);
    signals.error.set(None);
    load_accounts(
        owner_id,
        facility_id,
        include_disabled,
        None,
        false,
        signals,
        on_unauthorized,
    );
    load_jobs(
        shipment_id,
        None,
        false,
        signals,
        on_manifested,
        order_id,
        on_unauthorized,
    );
}

#[allow(clippy::too_many_arguments)]
fn load_accounts(
    owner_id: i64,
    facility_id: i64,
    include_disabled: bool,
    cursor: Option<OpaqueCursor>,
    append: bool,
    signals: CarrierSignals,
    on_unauthorized: Callback<()>,
) {
    let generation = signals.generation.get_untracked();
    leptos::task::spawn_local(async move {
        match api::carrier_accounts(
            owner_id,
            facility_id,
            include_disabled,
            cursor.as_ref(),
            PAGE_SIZE,
        )
        .await
        {
            Ok(page) if signals.generation.get_untracked() == generation => {
                signals.accounts.update(|accounts| {
                    if append {
                        append_unique_accounts(accounts, page.items)
                    } else {
                        *accounts = page.items
                    }
                });
                signals.account_cursor.set(page.next_cursor);
                if signals.selected_account.get_untracked().is_none() {
                    signals.selected_account.set(
                        signals
                            .accounts
                            .get_untracked()
                            .iter()
                            .find(|account| account.status == CarrierAccountStatus::Active)
                            .map(|account| account.account_id),
                    );
                }
            }
            Err(error) if signals.generation.get_untracked() == generation => {
                handle_error(error, signals, on_unauthorized, None)
            }
            _ => {}
        }
        if signals.generation.get_untracked() == generation {
            signals.loading.set(false);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn load_jobs(
    shipment_id: i64,
    cursor: Option<OpaqueCursor>,
    append: bool,
    signals: CarrierSignals,
    on_manifested: Callback<(i64, i64)>,
    order_id: i64,
    on_unauthorized: Callback<()>,
) {
    let generation = signals.generation.get_untracked();
    leptos::task::spawn_local(async move {
        match api::carrier_manifest_jobs(shipment_id, cursor.as_ref(), PAGE_SIZE).await {
            Ok(page) if signals.generation.get_untracked() == generation => {
                signals.jobs.update(|jobs| {
                    if append {
                        append_unique_jobs(jobs, page.items)
                    } else {
                        *jobs = page.items
                    }
                });
                signals.job_cursor.set(page.next_cursor);
                notify_manifested(signals, on_manifested, order_id, shipment_id);
                schedule_active_poll(
                    shipment_id,
                    generation,
                    signals,
                    on_manifested,
                    order_id,
                    on_unauthorized,
                );
            }
            Err(error) if signals.generation.get_untracked() == generation => {
                handle_error(error, signals, on_unauthorized, None)
            }
            _ => {}
        }
        if signals.generation.get_untracked() == generation {
            signals.loading.set(false);
        }
    });
}

fn dispatch_action(
    shipment_id: i64,
    action: PendingCarrierAction,
    signals: CarrierSignals,
    on_manifested: Callback<(i64, i64)>,
    order_id: i64,
    on_unauthorized: Callback<()>,
) {
    if signals.pending.get_untracked() {
        return;
    }
    signals.pending.set(true);
    signals.error.set(None);
    signals.retry_exact.set(None);
    let retained = action.clone();
    leptos::task::spawn_local(async move {
        let result = match action {
            PendingCarrierAction::Queue(request, key) => {
                api::queue_carrier_manifest(shipment_id, &request, &key)
                    .await
                    .map(ActionResult::Job)
            }
            PendingCarrierAction::Cancel(job, key) => api::cancel_carrier_manifest_job(
                shipment_id,
                job.job_id,
                &CancelCarrierManifestRequest {
                    expected_revision: job.revision,
                },
                &key,
            )
            .await
            .map(ActionResult::Job),
            PendingCarrierAction::Retry(job, key) => api::retry_carrier_manifest_job(
                shipment_id,
                job.job_id,
                &RetryCarrierManifestRequest {
                    expected_revision: job.revision,
                },
                &key,
            )
            .await
            .map(ActionResult::Job),
            PendingCarrierAction::Create(request, key) => {
                api::create_carrier_account(&request, &key)
                    .await
                    .map(ActionResult::Account)
            }
            PendingCarrierAction::Reconfigure(id, request, key) => {
                api::reconfigure_carrier_account(id, &request, &key)
                    .await
                    .map(ActionResult::Account)
            }
            PendingCarrierAction::Status(id, request, key) => {
                api::change_carrier_account_status(id, &request, &key)
                    .await
                    .map(ActionResult::Account)
            }
        };
        signals.pending.set(false);
        match result {
            Ok(ActionResult::Account(account)) => {
                upsert_account(signals.accounts, account);
                clear_account_editor(signals);
            }
            Ok(ActionResult::Job(job)) => {
                upsert_job(signals.jobs, job);
                notify_manifested(signals, on_manifested, order_id, shipment_id);
                schedule_active_poll(
                    shipment_id,
                    signals.generation.get_untracked(),
                    signals,
                    on_manifested,
                    order_id,
                    on_unauthorized,
                );
            }
            Err(error) => handle_error(error, signals, on_unauthorized, Some(retained)),
        }
    });
}

enum ActionResult {
    Account(CarrierAccountResponse),
    Job(CarrierManifestJobResponse),
}

fn handle_error(
    error: api::ApiError,
    signals: CarrierSignals,
    on_unauthorized: Callback<()>,
    retry: Option<PendingCarrierAction>,
) {
    if error.unauthorized {
        on_unauthorized.run(());
    }
    signals
        .retry_exact
        .set(error.ambiguous_outcome.then_some(retry).flatten());
    signals.error.set(Some(error.message));
}

#[cfg(target_arch = "wasm32")]
fn schedule_active_poll(
    shipment_id: i64,
    generation: u64,
    signals: CarrierSignals,
    on_manifested: Callback<(i64, i64)>,
    order_id: i64,
    on_unauthorized: Callback<()>,
) {
    use std::time::Duration;
    if active_job(&signals.jobs.get_untracked()).is_none() {
        return;
    }
    set_timeout(
        move || {
            if signals.generation.get_untracked() == generation {
                load_jobs(
                    shipment_id,
                    None,
                    false,
                    signals,
                    on_manifested,
                    order_id,
                    on_unauthorized,
                );
            }
        },
        Duration::from_secs(2),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn schedule_active_poll(
    _shipment_id: i64,
    _generation: u64,
    _signals: CarrierSignals,
    _on_manifested: Callback<(i64, i64)>,
    _order_id: i64,
    _on_unauthorized: Callback<()>,
) {
}

fn notify_manifested(
    signals: CarrierSignals,
    callback: Callback<(i64, i64)>,
    order_id: i64,
    shipment_id: i64,
) {
    if !signals.notified.get_untracked()
        && signals
            .jobs
            .get_untracked()
            .iter()
            .any(|job| job.status == CarrierManifestJobStatus::Succeeded)
    {
        signals.notified.set(true);
        callback.run((order_id, shipment_id));
    }
}

fn active_job(jobs: &[CarrierManifestJobResponse]) -> Option<&CarrierManifestJobResponse> {
    jobs.iter().find(|job| {
        matches!(
            job.status,
            CarrierManifestJobStatus::Queued
                | CarrierManifestJobStatus::Processing
                | CarrierManifestJobStatus::RetryScheduled
        )
    })
}

fn upsert_job(signal: RwSignal<Vec<CarrierManifestJobResponse>>, job: CarrierManifestJobResponse) {
    signal.update(|jobs| {
        jobs.retain(|item| item.job_id != job.job_id);
        jobs.insert(0, job);
    });
}
fn upsert_account(signal: RwSignal<Vec<CarrierAccountResponse>>, account: CarrierAccountResponse) {
    signal.update(|accounts| {
        accounts.retain(|item| item.account_id != account.account_id);
        accounts.insert(0, account);
    });
}
fn append_unique_jobs(
    current: &mut Vec<CarrierManifestJobResponse>,
    incoming: Vec<CarrierManifestJobResponse>,
) {
    for item in incoming {
        if current.iter().all(|value| value.job_id != item.job_id) {
            current.push(item);
        }
    }
}
fn append_unique_accounts(
    current: &mut Vec<CarrierAccountResponse>,
    incoming: Vec<CarrierAccountResponse>,
) {
    for item in incoming {
        if current
            .iter()
            .all(|value| value.account_id != item.account_id)
        {
            current.push(item);
        }
    }
}
fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn parse_id(value: &str) -> Option<i64> {
    value.parse().ok().filter(|value| *value > 0)
}
fn clear_account_editor(signals: CarrierSignals) {
    signals.editing_account.set(None);
    signals.display_name.set(String::new());
    signals.carrier_code.set(String::new());
    signals.account_key.set(String::new());
}
fn edit_account(signals: CarrierSignals, account: &CarrierAccountResponse) {
    signals.editing_account.set(Some(account.account_id));
    signals.display_name.set(account.display_name.clone());
    signals.carrier_code.set(account.carrier_code.clone());
    signals.account_key.set(account.account_key.clone());
}

const fn account_status_label(status: CarrierAccountStatus) -> &'static str {
    match status {
        CarrierAccountStatus::Active => "active",
        CarrierAccountStatus::Disabled => "disabled",
    }
}
const fn job_status_label(status: CarrierManifestJobStatus) -> &'static str {
    match status {
        CarrierManifestJobStatus::Queued => "Queued",
        CarrierManifestJobStatus::Processing => "Processing",
        CarrierManifestJobStatus::RetryScheduled => "Retry scheduled",
        CarrierManifestJobStatus::Succeeded => "Succeeded",
        CarrierManifestJobStatus::Failed => "Failed",
        CarrierManifestJobStatus::Cancelled => "Cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn carrier_states_have_explicit_operator_labels() {
        assert_eq!(
            job_status_label(CarrierManifestJobStatus::RetryScheduled),
            "Retry scheduled"
        );
        assert_eq!(
            account_status_label(CarrierAccountStatus::Disabled),
            "disabled"
        );
        assert_eq!(optional_text("  GROUND ").as_deref(), Some("GROUND"));
    }
}
