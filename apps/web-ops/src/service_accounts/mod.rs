mod display;
mod forms;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ChangeServiceAccountStatusRequest, CreateServiceAccountRequest,
    IssueServiceAccountCredentialRequest, OpaqueCursor, RevokeServiceAccountCredentialRequest,
    ServiceAccountEventPage, ServiceAccountEventPageRequest, ServiceAccountOptionsResponse,
    ServiceAccountPage, ServiceAccountPageRequest, ServiceAccountResponse, ServiceAccountStatus,
    UpdateServiceAccountAccessRequest,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone)]
enum Dialog {
    Create,
    Access(ServiceAccountResponse),
    Issue(ServiceAccountResponse),
    Status(ServiceAccountResponse),
    Revoke(ServiceAccountResponse, i64),
}

#[derive(Clone)]
enum PendingCommand {
    Create(CreateServiceAccountRequest, String),
    Access(i64, UpdateServiceAccountAccessRequest, String),
    Status(i64, ChangeServiceAccountStatusRequest, String),
    Issue(i64, IssueServiceAccountCredentialRequest, String),
    Revoke(i64, i64, RevokeServiceAccountCredentialRequest, String),
}

#[derive(Clone, Copy)]
struct Signals {
    accounts: RwSignal<ServiceAccountPage>,
    options: RwSignal<Vec<String>>,
    can_delegate_all_facilities: RwSignal<bool>,
    can_delegate_all_owners: RwSignal<bool>,
    events: RwSignal<ServiceAccountEventPage>,
    selected: RwSignal<Option<ServiceAccountResponse>>,
    status: RwSignal<Option<ServiceAccountStatus>>,
    loading: RwSignal<bool>,
    loaded: RwSignal<bool>,
    detail_loading: RwSignal<bool>,
    events_loading: RwSignal<bool>,
    list_generation: RwSignal<u64>,
    detail_generation: RwSignal<u64>,
    event_generation: RwSignal<u64>,
    error: RwSignal<Option<String>>,
    dialog: RwSignal<Option<Dialog>>,
    command_pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    revealed_secret: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn ServiceAccountsWorkspace(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let access = StoredValue::new(access);
    let signals = Signals {
        accounts: RwSignal::new(ServiceAccountPage::new(Vec::new(), None)),
        options: RwSignal::new(Vec::new()),
        can_delegate_all_facilities: RwSignal::new(false),
        can_delegate_all_owners: RwSignal::new(false),
        events: RwSignal::new(ServiceAccountEventPage::new(Vec::new(), None)),
        selected: RwSignal::new(None),
        status: RwSignal::new(None),
        loading: RwSignal::new(true),
        loaded: RwSignal::new(false),
        detail_loading: RwSignal::new(false),
        events_loading: RwSignal::new(false),
        list_generation: RwSignal::new(0),
        detail_generation: RwSignal::new(0),
        event_generation: RwSignal::new(0),
        error: RwSignal::new(None),
        dialog: RwSignal::new(None),
        command_pending: RwSignal::new(false),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        revealed_secret: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = forms::Drafts::new();
    Effect::new(move |_| refresh(signals));

    let create = move |_| {
        drafts.reset_create();
        signals.command_error.set(None);
        signals.retry.set(None);
        signals.dialog.set(Some(Dialog::Create));
    };
    let apply = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        signals.selected.set(None);
        signals
            .events
            .set(ServiceAccountEventPage::new(Vec::new(), None));
        refresh(signals);
    };
    let retry = move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    };

    view! {
        <section class="service-accounts-workspace">
            <header class="page-heading service-accounts-heading"><div><p class="eyebrow">"Organization security"</p><h1>"Service accounts"</h1><p>"Distinct non-human integration identities with explicit tenant, facility, client, permission, and credential lifecycle evidence."</p></div><div><button class="button primary-action" type="button" on:click=create>"Create service account"</button><button class="button secondary-action" type="button" disabled=move || signals.loading.get() on:click=move |_| refresh(signals)><Icon icon=UiIcon::Refresh/><span>"Refresh"</span></button></div></header>

            <form class="service-account-toolbar" on:submit=apply><label><span>"Status"</span><select prop:value=move || status_wire(signals.status.get()) on:change=move |event| signals.status.set(parse_status(&event_target_value(&event)))><option value="">"All accounts"</option><option value="active">"Active"</option><option value="disabled">"Disabled"</option></select></label><button class="button secondary-action compact" type="submit">"Apply"</button></form>

            {move || signals.revealed_secret.get().map(|secret| view! { <section class="service-account-secret" role="status"><div><strong>"Copy this credential now"</strong><span>"It cannot be recovered after this page is closed or refreshed."</span></div><input aria-label="Issued service-account bearer token" readonly prop:value=secret/><button class="text-button danger" type="button" on:click=move |_| signals.revealed_secret.set(None)>"I stored it securely"</button></section> })}

            <Show when=move || signals.error.get().is_some()><section class="service-account-error" role="alert"><span>{move || signals.error.get().unwrap_or_default()}</span><button class="text-button" type="button" on:click=move |_| refresh(signals)>"Retry reads"</button></section></Show>

            <div class="service-account-layout">
                {move || account_panel(signals)}
                {move || detail_panel(signals, access, drafts)}
            </div>

            <Show when=move || signals.retry.get().is_some()><section class="service-account-retry"><span>"The last command did not complete. Retrying preserves the exact request and idempotency key."</span><button class="button secondary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=retry>"Retry exact command"</button></section></Show>
            {move || signals.dialog.get().map(|dialog| forms::dialog(signals, drafts, access, dialog))}
        </section>
    }
}

fn account_panel(signals: Signals) -> AnyView {
    if signals.loading.get() && !signals.loaded.get() {
        return loading("Loading service accounts");
    }
    let page = signals.accounts.get();
    let next = page.next_cursor.clone();
    let item_count = page.items.len();
    let content = if page.items.is_empty() {
        empty(
            "No service accounts",
            "Create a dedicated identity before connecting an external system.",
        )
    } else {
        view! {
            <div class="table-scroll">
                <table class="dense-table">
                    <thead><tr><th>"Account"</th><th>"Scope"</th><th>"Permissions"</th><th>"Credentials"</th><th>"Last used"</th><th></th></tr></thead>
                    <tbody>
                        {page.items.into_iter().map(|account| {
                            let id = account.service_account_id;
                            let selected = signals.selected.get().as_ref().is_some_and(|value| value.service_account_id == id);
                            let active_credentials = account.credentials.iter().filter(|credential| credential.revoked_at.is_none()).count();
                            let credential_count = account.credentials.len();
                            view! {
                                <tr class:selected=selected>
                                    <td><strong>{account.name}</strong><small>{format!("#{id} · rev {}", account.revision.get())}</small><span class=display::status_class(account.status)>{display::status_label(account.status)}</span></td>
                                    <td>{display::scope_summary(&account.access)}</td>
                                    <td><div class="permission-list">{account.access.permission_names.into_iter().map(|name| view! { <span>{name}</span> }).collect_view()}</div></td>
                                    <td>{format!("{credential_count} total · {active_credentials} active")}</td>
                                    <td>{account.last_used_at.as_deref().map(display::short_timestamp).unwrap_or_else(|| "Never".into())}</td>
                                    <td><button class="button secondary-action compact" type="button" disabled=move || signals.detail_loading.get() on:click=move |_| load_detail(signals, id)>"Manage"</button></td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
        }.into_any()
    };
    view! {
        <section class="service-account-list">
            <header><div><p class="eyebrow">"Managed principals"</p><h2>"Integration identities"</h2></div><span>{format!("{item_count} in view")}</span></header>
            {content}
            {next.map(|cursor| view! { <button class="button secondary-action compact load-more" type="button" disabled=move || signals.loading.get() on:click=move |_| load_accounts(signals, Some(cursor.clone()), true)>"Load more"</button> })}
        </section>
    }.into_any()
}

fn detail_panel(
    signals: Signals,
    access: StoredValue<AccessScopeWorkspace>,
    drafts: forms::Drafts,
) -> AnyView {
    if signals.detail_loading.get() {
        return loading("Loading identity evidence");
    }
    let Some(account) = signals.selected.get() else {
        return view! { <section class="service-account-detail-empty"><strong>"Select an account to inspect and manage it."</strong><span>"Credentials, exact access, last-use evidence, and lifecycle events appear here."</span></section> }.into_any();
    };
    let access_account = account.clone();
    let issue_account = account.clone();
    let status_account = account.clone();
    let is_active = account.status == ServiceAccountStatus::Active;
    let description = account
        .description
        .clone()
        .unwrap_or_else(|| "No description".into());
    view! {
        <section class="service-account-detail">
            <header><div><p class="eyebrow">"Principal evidence"</p><h2>{account.name.clone()}</h2><span>{description}</span></div><span class=display::status_class(account.status)>{display::status_label(account.status)}</span></header>
            <div class="service-account-metadata">
                <div><span>"Revision"</span><strong>{account.revision.get()}</strong></div>
                <div><span>"Created"</span><strong>{display::short_timestamp(&account.created_at)}</strong><small>{format!("actor #{}", account.created_by)}</small></div>
                <div><span>"Updated"</span><strong>{display::short_timestamp(&account.updated_at)}</strong><small>{format!("actor #{}", account.updated_by)}</small></div>
                <div><span>"Last authenticated"</span><strong>{account.last_used_at.as_deref().map(display::short_timestamp).unwrap_or_else(|| "Never".into())}</strong></div>
            </div>
            {access.with_value(|scope| display::account_scope(scope, &account.access))}
            <div class="service-account-actions">
                <button class="button secondary-action compact" type="button" on:click=move |_| {
                    drafts.reset_access(Some(&access_account));
                    signals.command_error.set(None);
                    signals.dialog.set(Some(Dialog::Access(access_account.clone())));
                }>"Edit access"</button>
                {is_active.then(|| view! {
                    <button class="button primary-action compact" type="button" on:click=move |_| {
                        drafts.reset_credential();
                        signals.command_error.set(None);
                        signals.dialog.set(Some(Dialog::Issue(issue_account.clone())));
                    }>"Issue credential"</button>
                })}
                <button class=if is_active { "button danger-action compact" } else { "button primary-action compact" } type="button" on:click=move |_| {
                    drafts.reset_reason();
                    signals.command_error.set(None);
                    signals.dialog.set(Some(Dialog::Status(status_account.clone())));
                }>{if is_active { "Disable" } else { "Enable" }}</button>
            </div>
            {credential_panel(signals, drafts, account.clone())}
            {event_panel(signals, account.service_account_id)}
        </section>
    }.into_any()
}

fn credential_panel(
    signals: Signals,
    drafts: forms::Drafts,
    account: ServiceAccountResponse,
) -> AnyView {
    let rows = account.credentials.clone();
    let content = if rows.is_empty() {
        empty(
            "No credentials",
            "Issue a credential only when the receiving system is ready to store it.",
        )
    } else {
        view! {
            <div class="table-scroll">
                <table class="dense-table">
                    <thead><tr><th>"Label / prefix"</th><th>"Created"</th><th>"Expiry"</th><th>"Last used"</th><th>"State"</th><th></th></tr></thead>
                    <tbody>{rows.clone().into_iter().map(|credential| {
                        let id = credential.credential_id;
                        let revoke_account = account.clone();
                        let active = credential.revoked_at.is_none();
                        view! {
                            <tr>
                                <td><strong>{credential.label}</strong><small>{credential.token_prefix}</small></td>
                                <td>{display::short_timestamp(&credential.created_at)}<small>{format!("actor #{}", credential.created_by)}</small></td>
                                <td>{credential.expires_at.as_deref().map(display::short_timestamp).unwrap_or_else(|| "No expiry".into())}</td>
                                <td>{credential.last_used_at.as_deref().map(display::short_timestamp).unwrap_or_else(|| "Never".into())}</td>
                                <td>{if active { "Active" } else { "Revoked" }}{credential.revocation_reason.map(|value| view! { <small>{value}</small> })}</td>
                                <td>{active.then(|| view! { <button class="text-button danger" type="button" on:click=move |_| {
                                    drafts.reset_reason();
                                    signals.command_error.set(None);
                                    signals.dialog.set(Some(Dialog::Revoke(revoke_account.clone(), id)));
                                }>"Revoke"</button> })}</td>
                            </tr>
                        }
                    }).collect_view()}</tbody>
                </table>
            </div>
        }.into_any()
    };
    view! {
        <section class="service-account-evidence-panel">
            <header><h3>"Credential lifecycle"</h3><span>{format!("{} credentials", rows.len())}</span></header>
            {content}
        </section>
    }.into_any()
}

fn event_panel(signals: Signals, account_id: i64) -> AnyView {
    if signals.events_loading.get() {
        return loading("Loading lifecycle events");
    }
    let page = signals.events.get();
    let next = page.next_cursor.clone();
    let item_count = page.items.len();
    let content = if page.items.is_empty() {
        empty(
            "No lifecycle events",
            "Event evidence will appear after the account is created or changed.",
        )
    } else {
        view! {
            <ol class="service-account-events">
                {page.items.into_iter().map(|event| {
                    let evidence = serde_json::to_string_pretty(&event.evidence)
                        .unwrap_or_else(|_| "Evidence could not be rendered".into());
                    view! {
                        <li><span class="history-marker"></span><div><strong>{display::event_label(&event.action)}</strong><small>{format!("Revision {} · actor #{} · {}", event.account_revision.get(), event.actor_id, display::short_timestamp(&event.occurred_at))}</small>{event.credential_id.map(|id| view! { <span>{format!("Credential #{id}")}</span> })}<details><summary>"Evidence"</summary><pre>{evidence}</pre></details></div></li>
                    }
                }).collect_view()}
            </ol>
        }.into_any()
    };
    view! {
        <section class="service-account-evidence-panel">
            <header><h3>"Immutable lifecycle events"</h3><span>{format!("{item_count} in view")}</span></header>
            {content}
            {next.map(|cursor| view! { <button class="button secondary-action compact load-more" type="button" disabled=move || signals.events_loading.get() on:click=move |_| load_events(signals, account_id, Some(cursor.clone()), true)>"Load more events"</button> })}
        </section>
    }.into_any()
}

fn refresh(signals: Signals) {
    signals.selected.set(None);
    signals
        .events
        .set(ServiceAccountEventPage::new(Vec::new(), None));
    load_accounts(signals, None, false);
    load_options(signals);
}

fn load_accounts(signals: Signals, cursor: Option<OpaqueCursor>, append: bool) {
    signals.list_generation.update(|value| *value += 1);
    let generation = signals.list_generation.get_untracked();
    signals.loading.set(true);
    signals.error.set(None);
    let request = ServiceAccountPageRequest {
        status: signals.status.get_untracked(),
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::service_accounts(&request).await {
            Ok(page) if signals.list_generation.get_untracked() == generation => {
                if append {
                    signals.accounts.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.accounts.set(page);
                }
                signals.loaded.set(true);
            }
            Err(error) if signals.list_generation.get_untracked() == generation => {
                handle_read_error(signals, error);
            }
            _ => {}
        }
        if signals.list_generation.get_untracked() == generation {
            signals.loading.set(false);
        }
    });
}

fn load_options(signals: Signals) {
    leptos::task::spawn_local(async move {
        match api::service_account_options().await {
            Ok(ServiceAccountOptionsResponse {
                permission_names,
                can_delegate_all_facilities,
                can_delegate_all_inventory_owners,
            }) => {
                signals.options.set(permission_names);
                signals
                    .can_delegate_all_facilities
                    .set(can_delegate_all_facilities);
                signals
                    .can_delegate_all_owners
                    .set(can_delegate_all_inventory_owners);
            }
            Err(error) => handle_read_error(signals, error),
        }
    });
}

fn load_detail(signals: Signals, id: i64) {
    signals.detail_generation.update(|value| *value += 1);
    let generation = signals.detail_generation.get_untracked();
    signals.detail_loading.set(true);
    signals.event_generation.update(|value| *value += 1);
    signals
        .events
        .set(ServiceAccountEventPage::new(Vec::new(), None));
    leptos::task::spawn_local(async move {
        match api::service_account(id).await {
            Ok(account) if signals.detail_generation.get_untracked() == generation => {
                signals.selected.set(Some(account));
                load_events(signals, id, None, false);
            }
            Err(error) if signals.detail_generation.get_untracked() == generation => {
                handle_read_error(signals, error);
            }
            _ => {}
        }
        if signals.detail_generation.get_untracked() == generation {
            signals.detail_loading.set(false);
        }
    });
}

fn load_events(signals: Signals, id: i64, cursor: Option<OpaqueCursor>, append: bool) {
    signals.event_generation.update(|value| *value += 1);
    let generation = signals.event_generation.get_untracked();
    signals.events_loading.set(true);
    let request = ServiceAccountEventPageRequest {
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::service_account_events(id, &request).await {
            Ok(page) if signals.event_generation.get_untracked() == generation => {
                if append {
                    signals.events.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.events.set(page);
                }
            }
            Err(error) if signals.event_generation.get_untracked() == generation => {
                handle_read_error(signals, error);
            }
            _ => {}
        }
        if signals.event_generation.get_untracked() == generation {
            signals.events_loading.set(false);
        }
    });
}

fn dispatch(signals: Signals, command: PendingCommand) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.command_error.set(None);
    signals.retry.set(Some(command.clone()));
    leptos::task::spawn_local(async move {
        let issued_secret = match &command {
            PendingCommand::Issue(_, request, _) => Some(request.bearer_token.clone()),
            _ => None,
        };
        match execute(&command).await {
            Ok(account) => {
                signals.toasts.success("Service account updated.");
                if let Some(secret) = issued_secret {
                    signals.revealed_secret.set(Some(secret));
                }
                signals.retry.set(None);
                signals.dialog.set(None);
                signals.selected.set(Some(account.clone()));
                refresh_account_in_page(signals, account.clone());
                load_events(signals, account.service_account_id, None, false);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                signals.toasts.error(error.message.clone());
                signals.command_error.set(Some(error.message));
            }
        }
        signals.command_pending.set(false);
    });
}

async fn execute(command: &PendingCommand) -> Result<ServiceAccountResponse, api::ApiError> {
    match command {
        PendingCommand::Create(request, key) => api::create_service_account(request, key).await,
        PendingCommand::Access(id, request, key) => {
            api::update_service_account_access(*id, request, key).await
        }
        PendingCommand::Status(id, request, key) => {
            api::change_service_account_status(*id, request, key).await
        }
        PendingCommand::Issue(id, request, key) => {
            api::issue_service_account_credential(*id, request, key)
                .await
                .map(|value| value.service_account)
        }
        PendingCommand::Revoke(id, credential_id, request, key) => {
            api::revoke_service_account_credential(*id, *credential_id, request, key).await
        }
    }
}

fn refresh_account_in_page(signals: Signals, account: ServiceAccountResponse) {
    let status_filter = signals.status.get_untracked();
    signals.accounts.update(|page| {
        if !matches_status_filter(status_filter, account.status) {
            page.items
                .retain(|value| value.service_account_id != account.service_account_id);
            return;
        }

        if let Some(current) = page
            .items
            .iter_mut()
            .find(|value| value.service_account_id == account.service_account_id)
        {
            *current = account;
        } else {
            page.items.insert(0, account);
        }
    });
}

fn matches_status_filter(
    filter: Option<ServiceAccountStatus>,
    status: ServiceAccountStatus,
) -> bool {
    filter.is_none_or(|expected| expected == status)
}

fn handle_read_error(signals: Signals, error: api::ApiError) {
    if error.unauthorized {
        signals.on_unauthorized.run(());
    } else {
        signals.error.set(Some(error.message));
    }
}

fn status_wire(value: Option<ServiceAccountStatus>) -> &'static str {
    match value {
        Some(ServiceAccountStatus::Active) => "active",
        Some(ServiceAccountStatus::Disabled) => "disabled",
        None => "",
    }
}

fn parse_status(value: &str) -> Option<ServiceAccountStatus> {
    match value {
        "active" => Some(ServiceAccountStatus::Active),
        "disabled" => Some(ServiceAccountStatus::Disabled),
        _ => None,
    }
}

fn loading(label: &'static str) -> AnyView {
    view! { <section class="service-account-state" aria-busy="true"><span class="loading-line"></span><strong>{label}</strong></section> }.into_any()
}

fn empty(title: &'static str, description: &'static str) -> AnyView {
    view! { <section class="service-account-state"><strong>{title}</strong><span>{description}</span></section> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_filter_is_exact() {
        assert_eq!(status_wire(Some(ServiceAccountStatus::Active)), "active");
        assert_eq!(
            parse_status("disabled"),
            Some(ServiceAccountStatus::Disabled)
        );
        assert_eq!(parse_status("unknown"), None);
        assert!(matches_status_filter(None, ServiceAccountStatus::Active));
        assert!(matches_status_filter(
            Some(ServiceAccountStatus::Disabled),
            ServiceAccountStatus::Disabled
        ));
        assert!(!matches_status_filter(
            Some(ServiceAccountStatus::Active),
            ServiceAccountStatus::Disabled
        ));
    }
}
