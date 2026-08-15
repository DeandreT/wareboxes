mod display;
mod forms;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ChangeTenantStatusRequest, CreateTenantRequest, OpaqueCursor, TenantLifecycleEventPage,
    TenantLifecycleEventPageRequest, TenantLifecyclePage, TenantLifecyclePageRequest,
    TenantLifecycleResponse, TenantStatus,
};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone)]
pub(super) enum Dialog {
    Create,
    Status(Box<TenantLifecycleResponse>),
}

#[derive(Clone)]
pub(super) enum PendingCommand {
    Create(CreateTenantRequest, String),
    Status(i64, ChangeTenantStatusRequest, String),
}

#[derive(Clone, Copy)]
pub(super) struct Signals {
    current_tenant_id: i64,
    tenants: RwSignal<TenantLifecyclePage>,
    events: RwSignal<TenantLifecycleEventPage>,
    selected: RwSignal<Option<TenantLifecycleResponse>>,
    status: RwSignal<Option<TenantStatus>>,
    search: RwSignal<String>,
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
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn TenantLifecycleWorkspace(
    initial_page: Option<TenantLifecyclePage>,
    current_tenant_id: i64,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let has_initial = initial_page.is_some();
    let signals = Signals {
        current_tenant_id,
        tenants: RwSignal::new(
            initial_page.unwrap_or_else(|| TenantLifecyclePage::new(Vec::new(), None)),
        ),
        events: RwSignal::new(TenantLifecycleEventPage::new(Vec::new(), None)),
        selected: RwSignal::new(None),
        status: RwSignal::new(None),
        search: RwSignal::new(String::new()),
        loading: RwSignal::new(!has_initial),
        loaded: RwSignal::new(has_initial),
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
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = forms::Drafts::new();
    Effect::new(move |_| {
        if !has_initial {
            refresh(signals);
        }
    });

    let create = move |_| {
        drafts.reset_create();
        signals.command_error.set(None);
        signals.retry.set(None);
        signals.dialog.set(Some(Dialog::Create));
    };
    let apply = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        invalidate_detail(signals);
        refresh(signals);
    };
    let retry = move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    };

    view! {
        <section class="tenant-lifecycle-workspace">
            <header class="page-heading tenant-lifecycle-heading"><div><p class="eyebrow">"Platform operations"</p><h1>"Tenant lifecycle"</h1><p>"Provision, suspend, and reactivate hard SaaS tenant boundaries with attributed access revocation and immutable evidence."</p></div><div><button class="button primary-action" type="button" on:click=create>"Provision tenant"</button><button class="button secondary-action" type="button" disabled=move || signals.loading.get() on:click=move |_| refresh(signals)><Icon icon=UiIcon::Refresh/><span>"Refresh"</span></button></div></header>

            <form class="tenant-lifecycle-toolbar" on:submit=apply><label><span>"Search"</span><input type="search" maxlength="120" placeholder="Name or slug" prop:value=move || signals.search.get() on:input=move |event| signals.search.set(event_target_value(&event))/></label><label><span>"Status"</span><select prop:value=move || status_wire(signals.status.get()) on:change=move |event| signals.status.set(parse_status(&event_target_value(&event)))><option value="">"All tenants"</option><option value="active">"Active"</option><option value="suspended">"Suspended"</option></select></label><button class="button secondary-action compact" type="submit">"Apply"</button></form>

            <Show when=move || signals.error.get().is_some()><section class="tenant-lifecycle-error" role="alert"><span>{move || signals.error.get().unwrap_or_default()}</span><button class="text-button" type="button" on:click=move |_| refresh(signals)>"Retry reads"</button></section></Show>
            <div class="tenant-lifecycle-layout">
                {move || tenant_panel(signals)}
                {move || detail_panel(signals, drafts)}
            </div>
            <Show when=move || signals.retry.get().is_some()><section class="tenant-lifecycle-retry"><span>"The last command did not complete. Retrying preserves the exact request body and idempotency key."</span><button class="button secondary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=retry>"Retry exact command"</button></section></Show>
            {move || signals.dialog.get().map(|dialog| forms::dialog(signals, drafts, dialog))}
        </section>
    }
}

fn tenant_panel(signals: Signals) -> AnyView {
    if signals.loading.get() && !signals.loaded.get() {
        return state("Loading tenants", true);
    }
    let page = signals.tenants.get();
    let next = page.next_cursor.clone();
    let item_count = page.items.len();
    let content = if page.items.is_empty() {
        state("No tenants match these filters.", false)
    } else {
        view! {
            <div class="table-scroll"><table class="dense-table"><thead><tr><th>"Tenant"</th><th>"Status"</th><th>"Footprint"</th><th>"Revision"</th><th></th></tr></thead><tbody>
                {page.items.into_iter().map(|tenant| { let id=tenant.tenant_id; let selected=signals.selected.get().is_some_and(|value|value.tenant_id==id); view! { <tr class:selected=selected><td><strong>{tenant.name.clone()}</strong><small>{tenant.slug.clone()}</small></td><td><span class=display::status_class(tenant.status)>{display::status_label(tenant.status)}</span></td><td>{display::footprint(&tenant)}</td><td>{tenant.revision.get()}</td><td><button class="text-button" type="button" on:click=move |_| load_detail(signals,id)>"Inspect"</button></td></tr> } }).collect_view()}
            </tbody></table></div>
        }.into_any()
    };
    view! { <section class="tenant-lifecycle-panel tenant-list"><header><div><h2>"Organizations"</h2><span>{format!("{item_count} loaded")}</span></div>{next.map(|cursor| view! { <button class="text-button" type="button" disabled=move || signals.loading.get() on:click=move |_| load_page(signals,Some(cursor.clone()),true)>"Load more"</button> })}</header>{content}</section> }.into_any()
}

fn detail_panel(signals: Signals, drafts: forms::Drafts) -> AnyView {
    if signals.detail_loading.get() {
        return state("Loading tenant evidence", true);
    }
    let Some(tenant) = signals.selected.get() else {
        return view! { <section class="tenant-lifecycle-panel tenant-detail empty-detail"><Icon icon=UiIcon::Building/><strong>"Select a tenant"</strong><span>"Inspect lifecycle evidence, initial authority, access footprint, and suspension effects."</span></section> }.into_any();
    };
    let tenant_for_status = tenant.clone();
    let current_tenant = tenant.tenant_id == signals.current_tenant_id;
    let events = signals.events.get();
    let next_events = events.next_cursor.clone();
    let event_count = events.items.len();
    view! {
        <section class="tenant-lifecycle-panel tenant-detail">
            <header><div><p class="eyebrow">{tenant.slug.clone()}</p><h2>{tenant.name.clone()}</h2><span class=display::status_class(tenant.status)>{display::status_label(tenant.status)}</span></div><button class=if tenant.status==TenantStatus::Active { "button danger-action compact" } else { "button primary-action compact" } type="button" disabled=current_tenant && tenant.status==TenantStatus::Active title=if current_tenant && tenant.status==TenantStatus::Active { "Switch to another active tenant before suspending this tenant" } else { "Change tenant status" } on:click=move |_| { drafts.reset_reason(); signals.command_error.set(None); signals.retry.set(None); signals.dialog.set(Some(Dialog::Status(Box::new(tenant_for_status.clone())))); }>{if tenant.status==TenantStatus::Active { "Suspend" } else { "Reactivate" }}</button></header>
            <dl class="tenant-lifecycle-facts"><div><dt>"Tenant ID"</dt><dd>{tenant.tenant_id}</dd></div><div><dt>"Revision"</dt><dd>{tenant.revision.get()}</dd></div><div><dt>"Created"</dt><dd>{display::short_timestamp(&tenant.created_at)}</dd></div><div><dt>"Initial administrator"</dt><dd>{tenant.initial_admin_email.clone().unwrap_or_else(|| "Legacy provisioning".into())}</dd></div><div><dt>"Members"</dt><dd>{tenant.active_member_count}</dd></div><div><dt>"Facilities / clients"</dt><dd>{format!("{} / {}",tenant.active_facility_count,tenant.active_inventory_owner_count)}</dd></div><div><dt>"Active integrations"</dt><dd>{tenant.active_service_account_count}</dd></div><div><dt>"Last status reason"</dt><dd>{tenant.status_reason.clone().unwrap_or_else(|| "Initial active state".into())}</dd></div></dl>
            <section class="tenant-lifecycle-evidence"><header><div><h3>"Lifecycle evidence"</h3><span>{format!("{event_count} events loaded")}</span></div>{next_events.map(|cursor| { let id=tenant.tenant_id; view! { <button class="text-button" type="button" disabled=move || signals.events_loading.get() on:click=move |_| load_events(signals,id,Some(cursor.clone()),true)>"Load more"</button> } })}</header>
                {if signals.events_loading.get() && events.items.is_empty() { state("Loading events",true) } else if events.items.is_empty() { state("No lifecycle events recorded.",false) } else { view! { <ol>{events.items.into_iter().map(|event| view! { <li><div><strong>{display::action_label(&event.action)}</strong><span>{display::short_timestamp(&event.occurred_at)}</span></div><p>{event.reason.clone().unwrap_or_else(|| "Tenant provisioned".into())}</p><dl><div><dt>"Revision"</dt><dd>{event.tenant_revision.get()}</dd></div><div><dt>"Actor"</dt><dd>{format!("User #{}",event.actor_id)}</dd></div><div><dt>"Sessions revoked"</dt><dd>{event.revoked_session_count}</dd></div><div><dt>"Credentials revoked"</dt><dd>{event.revoked_credential_count}</dd></div></dl><details><summary>"Evidence payload"</summary><pre>{serde_json::to_string_pretty(&event.evidence).unwrap_or_else(|_| "{}".into())}</pre></details></li> }).collect_view()}</ol> }.into_any() }}
            </section>
        </section>
    }.into_any()
}

fn invalidate_detail(signals: Signals) {
    signals.detail_generation.update(|value| *value += 1);
    signals.event_generation.update(|value| *value += 1);
    signals.selected.set(None);
    signals
        .events
        .set(TenantLifecycleEventPage::new(Vec::new(), None));
}

fn refresh(signals: Signals) {
    load_page(signals, None, false);
}

fn load_page(signals: Signals, cursor: Option<OpaqueCursor>, append: bool) {
    signals.list_generation.update(|value| *value += 1);
    let generation = signals.list_generation.get_untracked();
    signals.loading.set(true);
    signals.error.set(None);
    let search = signals.search.get_untracked().trim().to_owned();
    let request = TenantLifecyclePageRequest {
        status: signals.status.get_untracked(),
        search: (!search.is_empty()).then_some(search),
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::tenant_lifecycle_page(&request).await {
            Ok(page) if signals.list_generation.get_untracked() == generation => {
                if append {
                    signals.tenants.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.tenants.set(page);
                }
            }
            Err(error) if signals.list_generation.get_untracked() == generation => {
                handle_read_error(signals, error)
            }
            _ => {}
        }
        if signals.list_generation.get_untracked() == generation {
            signals.loading.set(false);
            signals.loaded.set(true);
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
        .set(TenantLifecycleEventPage::new(Vec::new(), None));
    leptos::task::spawn_local(async move {
        match api::tenant_lifecycle_detail(id).await {
            Ok(tenant) if signals.detail_generation.get_untracked() == generation => {
                signals.selected.set(Some(tenant));
                load_events(signals, id, None, false);
            }
            Err(error) if signals.detail_generation.get_untracked() == generation => {
                handle_read_error(signals, error)
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
    let request = TenantLifecycleEventPageRequest {
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::tenant_lifecycle_events(id, &request).await {
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
                handle_read_error(signals, error)
            }
            _ => {}
        }
        if signals.event_generation.get_untracked() == generation {
            signals.events_loading.set(false);
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
            Ok(tenant) => {
                signals.toasts.success("Tenant lifecycle updated.");
                signals.retry.set(None);
                signals.dialog.set(None);
                signals.selected.set(Some(tenant.clone()));
                refresh_tenant_in_page(signals, tenant.clone());
                load_events(signals, tenant.tenant_id, None, false);
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

async fn execute(command: &PendingCommand) -> Result<TenantLifecycleResponse, api::ApiError> {
    match command {
        PendingCommand::Create(request, key) => api::create_tenant(request, key).await,
        PendingCommand::Status(id, request, key) => {
            api::change_tenant_status(*id, request, key).await
        }
    }
}

fn refresh_tenant_in_page(signals: Signals, tenant: TenantLifecycleResponse) {
    let status = signals.status.get_untracked();
    let search = signals.search.get_untracked().trim().to_ascii_lowercase();
    signals.tenants.update(|page| {
        if !matches_filters(status, &search, &tenant) {
            page.items
                .retain(|value| value.tenant_id != tenant.tenant_id);
            return;
        }
        if let Some(current) = page
            .items
            .iter_mut()
            .find(|value| value.tenant_id == tenant.tenant_id)
        {
            *current = tenant;
        } else {
            page.items.insert(0, tenant);
        }
    });
}

fn matches_filters(
    status: Option<TenantStatus>,
    search: &str,
    tenant: &TenantLifecycleResponse,
) -> bool {
    status.is_none_or(|expected| expected == tenant.status)
        && (search.is_empty()
            || tenant.name.to_ascii_lowercase().contains(search)
            || tenant.slug.to_ascii_lowercase().contains(search))
}

fn handle_read_error(signals: Signals, error: api::ApiError) {
    if error.unauthorized {
        signals.on_unauthorized.run(());
    } else {
        signals.error.set(Some(error.message));
    }
}
fn status_wire(value: Option<TenantStatus>) -> &'static str {
    match value {
        Some(TenantStatus::Active) => "active",
        Some(TenantStatus::Suspended) => "suspended",
        None => "",
    }
}
fn parse_status(value: &str) -> Option<TenantStatus> {
    match value {
        "active" => Some(TenantStatus::Active),
        "suspended" => Some(TenantStatus::Suspended),
        _ => None,
    }
}
fn state(label: &'static str, loading: bool) -> AnyView {
    view! { <section class="tenant-lifecycle-state" aria-busy=loading><Show when=move || loading><span class="loading-line"></span></Show><strong>{label}</strong></section> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_pages_remove_nonmatching_transitions() {
        let tenant = TenantLifecycleResponse {
            tenant_id: 1,
            slug: "northwest".into(),
            name: "Northwest".into(),
            status: TenantStatus::Suspended,
            revision: wareboxes_api_contract::v1::Revision::new(2).unwrap(),
            created_at: "2026-08-15T00:00:00Z".into(),
            created_by: None,
            initial_admin_user_id: None,
            initial_admin_email: None,
            status_changed_at: None,
            status_changed_by: None,
            status_reason: None,
            active_member_count: 1,
            active_facility_count: 0,
            active_inventory_owner_count: 0,
            active_service_account_count: 0,
        };
        assert!(!matches_filters(Some(TenantStatus::Active), "", &tenant));
        assert!(matches_filters(
            Some(TenantStatus::Suspended),
            "north",
            &tenant
        ));
    }
}
