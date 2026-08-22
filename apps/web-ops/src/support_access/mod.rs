mod display;
mod forms;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ApproveSupportAccessRequest, OpaqueCursor, RejectSupportAccessRequest,
    RequestSupportAccessRequest, RevokeSupportAccessRequest, SupportAccessEventPage,
    SupportAccessEventPageRequest, SupportAccessOptionsResponse, SupportAccessPage,
    SupportAccessPageRequest, SupportAccessResponse, SupportAccessStatus, TenantLifecyclePage,
};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone)]
pub(super) enum Dialog {
    Request,
    Approve(Box<SupportAccessResponse>),
    Reject(Box<SupportAccessResponse>),
    Revoke(Box<SupportAccessResponse>),
}

#[derive(Clone)]
pub(super) enum PendingCommand {
    Request(RequestSupportAccessRequest, String),
    Approve(i64, ApproveSupportAccessRequest, String),
    Reject(i64, RejectSupportAccessRequest, String),
    Revoke(i64, RevokeSupportAccessRequest, String),
}

#[derive(Clone, Copy)]
pub(super) struct Signals {
    current_user_id: i64,
    grants: RwSignal<SupportAccessPage>,
    tenants: RwSignal<TenantLifecyclePage>,
    events: RwSignal<SupportAccessEventPage>,
    selected: RwSignal<Option<SupportAccessResponse>>,
    tenant_filter: RwSignal<Option<i64>>,
    status_filter: RwSignal<Option<SupportAccessStatus>>,
    applied_tenant_filter: RwSignal<Option<i64>>,
    applied_status_filter: RwSignal<Option<SupportAccessStatus>>,
    loading: RwSignal<bool>,
    loaded: RwSignal<bool>,
    tenant_loading: RwSignal<bool>,
    event_loading: RwSignal<bool>,
    list_generation: RwSignal<u64>,
    event_generation: RwSignal<u64>,
    error: RwSignal<Option<String>>,
    dialog: RwSignal<Option<Dialog>>,
    options: RwSignal<Option<SupportAccessOptionsResponse>>,
    options_loading: RwSignal<bool>,
    options_generation: RwSignal<u64>,
    command_pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn SupportAccessWorkspace(
    initial_page: Option<SupportAccessPage>,
    initial_tenants: Option<TenantLifecyclePage>,
    current_user_id: i64,
    can_manage: bool,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let has_initial = initial_page.is_some() && initial_tenants.is_some();
    let signals = Signals {
        current_user_id,
        grants: RwSignal::new(
            initial_page.unwrap_or_else(|| SupportAccessPage::new(Vec::new(), None)),
        ),
        tenants: RwSignal::new(
            initial_tenants.unwrap_or_else(|| TenantLifecyclePage::new(Vec::new(), None)),
        ),
        events: RwSignal::new(SupportAccessEventPage::new(Vec::new(), None)),
        selected: RwSignal::new(None),
        tenant_filter: RwSignal::new(None),
        status_filter: RwSignal::new(None),
        applied_tenant_filter: RwSignal::new(None),
        applied_status_filter: RwSignal::new(None),
        loading: RwSignal::new(!has_initial),
        loaded: RwSignal::new(has_initial),
        tenant_loading: RwSignal::new(false),
        event_loading: RwSignal::new(false),
        list_generation: RwSignal::new(0),
        event_generation: RwSignal::new(0),
        error: RwSignal::new(None),
        dialog: RwSignal::new(None),
        options: RwSignal::new(None),
        options_loading: RwSignal::new(false),
        options_generation: RwSignal::new(0),
        command_pending: RwSignal::new(false),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = forms::Drafts::new();
    Effect::new(move |_| {
        if !has_initial {
            refresh(signals);
            load_tenants(signals, None, false);
        }
    });
    let open_request = move |_| {
        drafts.reset_request();
        signals.options.set(None);
        signals.command_error.set(None);
        signals.retry.set(None);
        signals.dialog.set(Some(Dialog::Request));
    };
    let apply = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        clear_selection(signals);
        apply_filters(signals);
    };
    let retry = move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    };

    view! {
        <section class="support-access-workspace">
            <header class="page-heading support-access-heading"><div><p class="eyebrow">"Platform security"</p><h1>"Support access"</h1><p>"Request and approve time-bounded, read-only tenant access with exact facility, client, and permission scope. Every transition is immutable and expires without a cleanup job."</p></div><div>{can_manage.then(||view!{<button class="button primary-action" type="button" on:click=open_request>"Request access"</button>})}<button class="button secondary-action" type="button" disabled=move || signals.loading.get() on:click=move |_| refresh(signals)><Icon icon=UiIcon::Refresh/><span>"Refresh"</span></button></div></header>

            {(!can_manage).then(|| view! { <section class="support-access-warning"><strong>"Active support session is read-only."</strong><span>"Switch to your ordinary platform tenant before requesting, approving, rejecting, or revoking access."</span></section> })}

            <form class="support-access-toolbar" on:submit=apply><label><span>"Tenant"</span><select prop:value=move || signals.tenant_filter.get().map_or_else(String::new,|value|value.to_string()) on:change=move |event| signals.tenant_filter.set(event_target_value(&event).parse().ok())><option value="">"All tenants"</option>{move || signals.tenants.get().items.into_iter().map(|tenant| view! { <option value=tenant.tenant_id.to_string()>{tenant.name}</option> }).collect_view()}</select></label><label><span>"Status"</span><select prop:value=move || status_wire(signals.status_filter.get()) on:change=move |event| signals.status_filter.set(parse_status(&event_target_value(&event)))><option value="">"All states"</option><option value="pending">"Pending"</option><option value="active">"Active"</option><option value="expired">"Expired"</option><option value="rejected">"Rejected"</option><option value="revoked">"Revoked"</option></select></label><button class="button secondary-action compact" type="submit">"Apply"</button></form>

            <Show when=move || signals.error.get().is_some()><section class="support-access-error" role="alert"><span>{move || signals.error.get().unwrap_or_default()}</span><button class="text-button" type="button" on:click=move |_| refresh(signals)>"Retry reads"</button></section></Show>
            <section class="support-access-metrics">{move || metrics(signals.grants.get())}</section>
            <div class="support-access-layout">{move || grant_panel(signals)}{move || evidence_panel(signals,can_manage)}</div>
            <Show when=move || signals.retry.get().is_some()><section class="support-access-retry"><span>"The last command did not complete. Retrying preserves the exact request and idempotency key."</span><button class="button secondary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=retry>"Retry exact command"</button></section></Show>
            {move || signals.dialog.get().map(|dialog| forms::dialog(signals,drafts,dialog))}
        </section>
    }
}

fn metrics(page: SupportAccessPage) -> AnyView {
    let pending = page
        .items
        .iter()
        .filter(|value| value.status == SupportAccessStatus::Pending)
        .count();
    let active = page
        .items
        .iter()
        .filter(|value| value.status == SupportAccessStatus::Active)
        .count();
    let expired = page
        .items
        .iter()
        .filter(|value| value.status == SupportAccessStatus::Expired)
        .count();
    view! { <div><span>"Pending visible"</span><strong>{pending}</strong></div><div><span>"Active visible"</span><strong>{active}</strong></div><div><span>"Expired visible"</span><strong>{expired}</strong></div><div><span>"Grants loaded"</span><strong>{page.items.len()}</strong></div> }.into_any()
}

fn grant_panel(signals: Signals) -> AnyView {
    if signals.loading.get() && !signals.loaded.get() {
        return state("Loading support grants", true);
    }
    let page = signals.grants.get();
    let next = page.next_cursor.clone();
    let count = page.items.len();
    let content = if page.items.is_empty() {
        state("No support access matches these filters.", false)
    } else {
        view! { <div class="table-scroll"><table class="dense-table"><caption class="sr-only">"Support access grants in the current filtered page"</caption><thead><tr><th>"Tenant / requester"</th><th>"State"</th><th>"Exact scope"</th><th>"Permissions"</th><th>"Expires"</th><th></th></tr></thead><tbody>{page.items.into_iter().map(|grant| { let id=grant.support_access_grant_id; let selected=signals.selected.get().is_some_and(|value|value.support_access_grant_id==id); view! { <tr class:selected=selected><td><strong>{grant.tenant_name.clone()}</strong><small>{grant.requested_by_email.clone()}</small></td><td><span class=display::status_class(grant.status)>{display::status_label(grant.status)}</span></td><td>{display::scope_summary(&grant)}</td><td>{display::permission_summary(&grant)}</td><td>{display::short_timestamp(&grant.expires_at)}</td><td><button class="text-button" type="button" on:click=move |_| select_grant(signals,grant.clone())>"Inspect"</button></td></tr> } }).collect_view()}</tbody></table></div> }.into_any()
    };
    view! { <section class="support-access-panel grant-list"><header><div><h2>"Governed grants"</h2><span>{format!("{count} loaded")}</span></div>{next.map(|cursor|view!{<button class="text-button" type="button" disabled=move || signals.loading.get() on:click=move |_| load_page(signals,Some(cursor.clone()),true)>"Load more"</button>})}</header>{content}</section> }.into_any()
}

fn evidence_panel(signals: Signals, can_manage: bool) -> AnyView {
    let Some(grant) = signals.selected.get() else {
        return view! { <section class="support-access-panel evidence empty-detail"><Icon icon=UiIcon::Access/><strong>"Select a grant"</strong><span>"Inspect two-person approval, exact delegated scope, expiry, and immutable evidence."</span></section> }.into_any();
    };
    let approve = grant.clone();
    let reject = grant.clone();
    let revoke = grant.clone();
    let events = signals.events.get();
    let next = events.next_cursor.clone();
    view! { <section class="support-access-panel evidence"><header><div><p class="eyebrow">{grant.tenant_slug.clone()}</p><h2>{format!("Grant #{}",grant.support_access_grant_id)}</h2><span class=display::status_class(grant.status)>{display::status_label(grant.status)}</span></div><div class="support-actions">{(can_manage && grant.status==SupportAccessStatus::Pending && grant.requested_by!=signals.current_user_id).then(||view!{<button class="button primary-action compact" type="button" on:click=move |_| open_dialog(signals,Dialog::Approve(Box::new(approve.clone())))>"Approve"</button>})}{(can_manage && grant.status==SupportAccessStatus::Pending).then(||view!{<button class="button secondary-action compact" type="button" on:click=move |_| open_dialog(signals,Dialog::Reject(Box::new(reject.clone())))>"Reject"</button>})}{(can_manage && grant.status==SupportAccessStatus::Active).then(||view!{<button class="button danger-action compact" type="button" on:click=move |_| open_dialog(signals,Dialog::Revoke(Box::new(revoke.clone())))>"Revoke"</button>})}</div></header>
        <section class="support-access-warning"><strong>"Tenant operations are read-only by construction"</strong><span>"Tenant-scoped operational routes accept only GET, HEAD, and OPTIONS while this grant is active. Tenant administration is never delegable."</span></section>
        <dl class="support-access-facts"><div><dt>"Requested by"</dt><dd>{grant.requested_by_email.clone()}</dd></div><div><dt>"Requested"</dt><dd>{display::short_timestamp(&grant.requested_at)}</dd></div><div><dt>"Expires"</dt><dd>{display::short_timestamp(&grant.expires_at)}</dd></div><div><dt>"Revision"</dt><dd>{grant.revision.get()}</dd></div><div><dt>"Facility IDs"</dt><dd>{if grant.access.all_facilities { "All".into() } else { format_ids(&grant.access.facility_ids) }}</dd></div><div><dt>"Client IDs"</dt><dd>{if grant.access.all_inventory_owners { "All".into() } else { format_ids(&grant.access.inventory_owner_ids) }}</dd></div><div><dt>"Permissions"</dt><dd>{display::permission_summary(&grant)}</dd></div><div><dt>"Reason"</dt><dd>{grant.reason.clone()}</dd></div></dl>
        <section class="support-access-events"><header><div><h3>"Immutable evidence"</h3><span>{format!("{} events loaded",events.items.len())}</span></div>{next.map(|cursor|{let id=grant.support_access_grant_id;view!{<button class="text-button" type="button" disabled=move || signals.event_loading.get() on:click=move |_| load_events(signals,id,Some(cursor.clone()),true)>"Load more"</button>}})}</header>{if signals.event_loading.get() && events.items.is_empty(){state("Loading evidence",true)}else if events.items.is_empty(){state("No evidence is available.",false)}else{view!{<ol>{events.items.into_iter().map(|event|view!{<li><div><strong>{display::action_label(&event.action)}</strong><span>{display::short_timestamp(&event.occurred_at)}</span></div><p>{event.reason.unwrap_or_else(||"Two-person approval recorded".into())}</p><small>{format!("Revision {} · Actor #{}",event.grant_revision.get(),event.actor_id)}</small></li>}).collect_view()}</ol>}.into_any()}}</section>
    </section> }.into_any()
}

fn open_dialog(signals: Signals, dialog: Dialog) {
    signals.command_error.set(None);
    signals.retry.set(None);
    signals.dialog.set(Some(dialog));
}

fn select_grant(signals: Signals, grant: SupportAccessResponse) {
    let id = grant.support_access_grant_id;
    signals.selected.set(Some(grant));
    signals
        .events
        .set(SupportAccessEventPage::new(Vec::new(), None));
    load_events(signals, id, None, false);
}

fn clear_selection(signals: Signals) {
    signals.event_generation.update(|value| *value += 1);
    signals.event_loading.set(false);
    signals.selected.set(None);
    signals
        .events
        .set(SupportAccessEventPage::new(Vec::new(), None));
}

fn refresh(signals: Signals) {
    load_page(signals, None, false);
}

fn apply_filters(signals: Signals) {
    signals
        .applied_tenant_filter
        .set(signals.tenant_filter.get_untracked());
    signals
        .applied_status_filter
        .set(signals.status_filter.get_untracked());
    load_page(signals, None, false);
}

fn load_page(signals: Signals, cursor: Option<OpaqueCursor>, append: bool) {
    signals.list_generation.update(|value| *value += 1);
    let generation = signals.list_generation.get_untracked();
    signals.loading.set(true);
    signals.error.set(None);
    let selected_id = if append {
        None
    } else {
        signals
            .selected
            .get_untracked()
            .map(|grant| grant.support_access_grant_id)
    };
    let restore_event_generation = selected_id.map(|_| {
        clear_selection(signals);
        signals.event_generation.get_untracked()
    });
    let request = SupportAccessPageRequest {
        tenant_id: signals.applied_tenant_filter.get_untracked(),
        status: signals.applied_status_filter.get_untracked(),
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::support_access_page(&request).await {
            Ok(page) if signals.list_generation.get_untracked() == generation => {
                let refreshed_selection = selected_id.and_then(|id| {
                    page.items
                        .iter()
                        .find(|grant| grant.support_access_grant_id == id)
                        .cloned()
                });
                if append {
                    signals.grants.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.grants.set(page);
                }
                // A later row selection owns the detail pane; the list response
                // may refresh its rows but must not replace that newer intent.
                if let Some(grant) = refreshed_selection.filter(|_| {
                    selection_can_be_restored(
                        restore_event_generation,
                        signals.event_generation.get_untracked(),
                        signals.selected.get_untracked().is_some(),
                    )
                }) {
                    select_grant(signals, grant);
                }
            }
            Err(error) if signals.list_generation.get_untracked() == generation => {
                handle_error(signals, error)
            }
            _ => {}
        }
        if signals.list_generation.get_untracked() == generation {
            signals.loading.set(false);
            signals.loaded.set(true);
        }
    });
}

fn load_tenants(signals: Signals, cursor: Option<OpaqueCursor>, append: bool) {
    signals.tenant_loading.set(true);
    let request = wareboxes_api_contract::v1::TenantLifecyclePageRequest {
        status: Some(wareboxes_api_contract::v1::TenantStatus::Active),
        search: None,
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::tenant_lifecycle_page(&request).await {
            Ok(page) => {
                if append {
                    signals.tenants.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.tenants.set(page);
                }
            }
            Err(error) => handle_error(signals, error),
        }
        signals.tenant_loading.set(false);
    });
}

pub(super) fn load_options(signals: Signals, tenant_id: i64) {
    signals.options_generation.update(|value| *value += 1);
    let generation = signals.options_generation.get_untracked();
    signals.options_loading.set(true);
    signals.options.set(None);
    leptos::task::spawn_local(async move {
        match api::support_access_options(tenant_id).await {
            Ok(options) if signals.options_generation.get_untracked() == generation => {
                signals.options.set(Some(options));
            }
            Err(error) if signals.options_generation.get_untracked() == generation => {
                handle_error(signals, error)
            }
            _ => {}
        }
        if signals.options_generation.get_untracked() == generation {
            signals.options_loading.set(false);
        }
    });
}

fn load_events(signals: Signals, id: i64, cursor: Option<OpaqueCursor>, append: bool) {
    signals.event_generation.update(|value| *value += 1);
    let generation = signals.event_generation.get_untracked();
    signals.event_loading.set(true);
    let request = SupportAccessEventPageRequest {
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::support_access_events(id, &request).await {
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
                handle_error(signals, error)
            }
            _ => {}
        }
        if signals.event_generation.get_untracked() == generation {
            signals.event_loading.set(false);
        }
    });
}

pub(super) fn dispatch(signals: Signals, command: PendingCommand) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.command_error.set(None);
    signals.retry.set(Some(command.clone()));
    leptos::task::spawn_local(async move {
        match execute(&command).await {
            Ok(grant) => {
                signals.toasts.success("Support access updated.");
                signals.retry.set(None);
                signals.dialog.set(None);
                refresh_grant(signals, grant.clone());
                select_grant(signals, grant);
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

async fn execute(command: &PendingCommand) -> Result<SupportAccessResponse, api::ApiError> {
    match command {
        PendingCommand::Request(request, key) => api::request_support_access(request, key).await,
        PendingCommand::Approve(id, request, key) => {
            api::approve_support_access(*id, request, key).await
        }
        PendingCommand::Reject(id, request, key) => {
            api::reject_support_access(*id, request, key).await
        }
        PendingCommand::Revoke(id, request, key) => {
            api::revoke_support_access(*id, request, key).await
        }
    }
}

fn refresh_grant(signals: Signals, grant: SupportAccessResponse) {
    let tenant_id = signals.applied_tenant_filter.get_untracked();
    let status = signals.applied_status_filter.get_untracked();
    let matches = matches_filters(tenant_id, status, grant.tenant_id, grant.status);
    signals.grants.update(|page| {
        page.items
            .retain(|value| value.support_access_grant_id != grant.support_access_grant_id);
        if matches {
            page.items.insert(0, grant);
        }
    });
}

fn matches_filters(
    filter_tenant_id: Option<i64>,
    filter_status: Option<SupportAccessStatus>,
    tenant_id: i64,
    status: SupportAccessStatus,
) -> bool {
    filter_tenant_id.is_none_or(|filter| filter == tenant_id)
        && filter_status.is_none_or(|filter| filter == status)
}

fn selection_can_be_restored(
    expected_event_generation: Option<u64>,
    current_event_generation: u64,
    has_newer_selection: bool,
) -> bool {
    !has_newer_selection && expected_event_generation == Some(current_event_generation)
}

fn handle_error(signals: Signals, error: api::ApiError) {
    if error.unauthorized {
        signals.on_unauthorized.run(());
    } else {
        signals.error.set(Some(error.message));
    }
}

fn format_ids(values: &[i64]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn status_wire(value: Option<SupportAccessStatus>) -> &'static str {
    match value {
        Some(SupportAccessStatus::Pending) => "pending",
        Some(SupportAccessStatus::Active) => "active",
        Some(SupportAccessStatus::Rejected) => "rejected",
        Some(SupportAccessStatus::Revoked) => "revoked",
        Some(SupportAccessStatus::Expired) => "expired",
        None => "",
    }
}

fn parse_status(value: &str) -> Option<SupportAccessStatus> {
    match value {
        "pending" => Some(SupportAccessStatus::Pending),
        "active" => Some(SupportAccessStatus::Active),
        "rejected" => Some(SupportAccessStatus::Rejected),
        "revoked" => Some(SupportAccessStatus::Revoked),
        "expired" => Some(SupportAccessStatus::Expired),
        _ => None,
    }
}

fn state(label: &'static str, loading: bool) -> AnyView {
    view! { <section class="support-access-state" aria-busy=loading><Show when=move || loading><span class="loading-line"></span></Show><strong>{label}</strong></section> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_wire_round_trips_every_filter() {
        for status in [
            SupportAccessStatus::Pending,
            SupportAccessStatus::Active,
            SupportAccessStatus::Rejected,
            SupportAccessStatus::Revoked,
            SupportAccessStatus::Expired,
        ] {
            assert_eq!(parse_status(status_wire(Some(status))), Some(status));
        }
    }

    #[test]
    fn reconciliation_respects_applied_tenant_and_status_filters() {
        assert!(matches_filters(
            Some(42),
            Some(SupportAccessStatus::Active),
            42,
            SupportAccessStatus::Active,
        ));
        assert!(!matches_filters(
            Some(7),
            None,
            42,
            SupportAccessStatus::Active,
        ));
        assert!(!matches_filters(
            None,
            Some(SupportAccessStatus::Pending),
            42,
            SupportAccessStatus::Active,
        ));
    }

    #[test]
    fn list_reconciliation_never_replaces_newer_detail_intent() {
        assert!(selection_can_be_restored(Some(8), 8, false));
        assert!(!selection_can_be_restored(Some(8), 9, false));
        assert!(!selection_can_be_restored(Some(8), 8, true));
        assert!(!selection_can_be_restored(None, 8, false));
    }
}
