use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use wareboxes_core::dto::WebSessionContext;

use crate::administration::AdministrationArea;
use crate::api;
use crate::app::{Section, SessionState};
use crate::components::{Icon, NavItem, UiIcon};
use crate::preferences::DisplayOptionsMenu;
use crate::toast::{use_toast_bus, ToastViewport};
use crate::view_model::{has_permission, user_name};

#[component]
pub(crate) fn Brand() -> impl IntoView {
    view! {
        <span class="brand">
            <span class="brand-mark" aria-hidden="true">
                <i></i><i></i><i></i><i></i>
            </span>
            <span>"wareboxes"</span>
        </span>
    }
}

#[component]
pub(crate) fn PageFrame(section: Section, children: Children) -> impl IntoView {
    let session = expect_context::<WebSessionContext>();
    let session_state = expect_context::<RwSignal<SessionState>>();
    let display_name = user_name(&session);
    let initials = display_name
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let tenant_name = session.active_tenant.name.clone();
    let tenant_id = session.active_tenant.tenant_id.get();
    let email = session.user.email.clone();
    let can_view_orders = has_permission(&session, "orders");
    let can_view_inventory = has_permission(&session, "wms");
    let can_supervise_wms = has_permission(&session, "wms_supervisor");
    let can_administer = has_permission(&session, "admin");
    let available_tenants = session.available_tenants.clone();
    let tenant_count = available_tenants.len();
    let tenant_options = available_tenants
        .into_iter()
        .map(|tenant| {
            view! {
                <option value=tenant.tenant_id.to_string()>{tenant.name}</option>
            }
        })
        .collect_view();
    let selected_tenant = RwSignal::new(tenant_id.to_string());
    let switching_tenant = RwSignal::new(false);
    let tenant_error = RwSignal::new(None::<String>);
    let navigate = use_navigate();
    let active_tenant_id = tenant_id;
    let toasts = use_toast_bus();

    let switch_tenant = move |event| {
        let value = event_target_value(&event);
        selected_tenant.set(value.clone());
        let Ok(selected_id) = value.parse::<i64>() else {
            selected_tenant.set(active_tenant_id.to_string());
            return;
        };
        if selected_id == active_tenant_id || switching_tenant.get_untracked() {
            return;
        }
        switching_tenant.set(true);
        tenant_error.set(None);
        let navigate = navigate.clone();
        leptos::task::spawn_local(async move {
            match api::select_tenant(selected_id).await {
                Ok(context) => {
                    let organization_name = context.active_tenant.name.clone();
                    session_state.set(SessionState::Authenticated(Box::new(context)));
                    toasts.info(format!("Switched to {organization_name}."));
                    navigate("/", Default::default());
                }
                Err(error) if error.unauthorized => {
                    session_state.set(SessionState::Anonymous(Some(
                        "Your session ended. Sign in to continue.".to_owned(),
                    )));
                }
                Err(error) => {
                    selected_tenant.set(active_tenant_id.to_string());
                    tenant_error.set(Some(error.message));
                    switching_tenant.set(false);
                }
            }
        });
    };

    let sign_out = move |_| {
        session_state.set(SessionState::Anonymous(None));
        leptos::task::spawn_local(async move {
            api::logout().await;
        });
    };

    view! {
        <div class="app-shell">
            <aside class="sidebar">
                <a class="sidebar-brand" href="/" aria-label="Wareboxes overview">
                    <Brand/>
                </a>
                <nav aria-label="Operations">
                    <p>"Operations"</p>
                    <NavItem
                        href="/"
                        label="Overview"
                        icon=UiIcon::Overview
                        active=section == Section::Overview
                    />
                    {can_view_orders.then(|| {
                        view! {
                            <NavItem
                                href="/orders"
                                label="Orders"
                                icon=UiIcon::Orders
                                active=section == Section::Orders
                            />
                        }
                    })}
                    {can_view_inventory.then(|| {
                        view! {
                            <NavItem
                                href="/packing"
                                label="Packing"
                                icon=UiIcon::Packing
                                active=section == Section::Packing
                            />
                            <NavItem
                                href="/shipping"
                                label="Shipping"
                                icon=UiIcon::Shipping
                                active=section == Section::Shipping
                            />
                            <NavItem
                                href="/outbound-loads"
                                label="Outbound loads"
                                icon=UiIcon::Shipping
                                active=section == Section::OutboundLoads
                            />
                            <NavItem
                                href="/loads"
                                label="Inbound loads"
                                icon=UiIcon::Loads
                                active=section == Section::Loads
                            />
                            <NavItem
                                href="/inventory"
                                label="Inventory"
                                icon=UiIcon::Inventory
                                active=section == Section::Inventory
                            />
                            <NavItem
                                href="/inventory/holds"
                                label="Quantity holds"
                                icon=UiIcon::Holds
                                active=section == Section::InventoryHolds
                            />
                            <NavItem
                                href="/inventory/disposition"
                                label="Disposition"
                                icon=UiIcon::Disposition
                                active=section == Section::InventoryDisposition
                            />
                            <NavItem
                                href="/inventory/control"
                                label="Inventory control"
                                icon=UiIcon::Inventory
                                active=section == Section::InventoryIntegrity
                            />
                            <NavItem
                                href="/catalog"
                                label="Master data"
                                icon=UiIcon::Catalog
                                active=section == Section::Catalog
                            />
                        }
                    })}
                    {can_supervise_wms.then(|| {
                        view! {
                            <NavItem
                                href="/replenishment"
                                label="Replenishment"
                                icon=UiIcon::Replenishment
                                active=section == Section::Replenishment
                            />
                        }
                    })}
                    <p class="nav-group">"Context"</p>
                    <NavItem
                        href="/access"
                        label="Access"
                        icon=UiIcon::Access
                        active=section == Section::Access
                    />
                    {can_administer.then(|| {
                        view! {
                            <p class="nav-group">"Administration"</p>
                            <NavItem
                                href="/administration/clients"
                                label="Clients"
                                icon=UiIcon::Clients
                                active=section
                                    == Section::Administration(AdministrationArea::Clients)
                            />
                            <NavItem
                                href="/administration/employees"
                                label="Employees"
                                icon=UiIcon::Employees
                                active=section
                                    == Section::Administration(AdministrationArea::Employees)
                            />
                            <NavItem
                                href="/administration/count-plans"
                                label="Count plans"
                                icon=UiIcon::Counts
                                active=section
                                    == Section::Administration(AdministrationArea::CountPlans)
                            />
                            <NavItem
                                href="/administration/users"
                                label="Users"
                                icon=UiIcon::Users
                                active=section
                                    == Section::Administration(AdministrationArea::Users)
                            />
                            <NavItem
                                href="/administration/roles"
                                label="Roles"
                                icon=UiIcon::Roles
                                active=section
                                    == Section::Administration(AdministrationArea::Roles)
                            />
                            <NavItem
                                href="/administration/permissions"
                                label="Permissions"
                                icon=UiIcon::Permissions
                                active=section
                                    == Section::Administration(AdministrationArea::Permissions)
                            />
                        }
                    })}
                </nav>
                <div class="sidebar-scope">
                    <span>"Active organization"</span>
                    <strong>{tenant_name.clone()}</strong>
                    <small>{scope_summary(&session)}</small>
                </div>
            </aside>

            <div class="app-region">
                <header class="topbar">
                    <div class="tenant-control">
                        <Icon icon=UiIcon::Building/>
                        <label for="tenant-selector">"Organization"</label>
                        <select
                            id="tenant-selector"
                            prop:value=move || selected_tenant.get()
                            disabled=move || switching_tenant.get() || tenant_count <= 1
                            on:change=switch_tenant
                        >
                            {tenant_options}
                        </select>
                        {move || {
                            tenant_error.get().map(|message| {
                                view! {
                                    <span class="tenant-switch-error" role="alert">
                                        {message}
                                    </span>
                                }
                            })
                        }}
                    </div>
                    <div class="identity">
                        <DisplayOptionsMenu/>
                        <span class="avatar" aria-hidden="true">{initials}</span>
                        <span class="identity-copy">
                            <strong>{display_name}</strong>
                            <small>{email}</small>
                        </span>
                        <button class="button quiet-action" type="button" on:click=sign_out>
                            <Icon icon=UiIcon::SignOut/>
                            <span>"Sign out"</span>
                        </button>
                    </div>
                </header>
                <main class="workspace">
                    <ToastViewport/>
                    {children()}
                </main>
            </div>
        </div>
    }
}

fn scope_summary(session: &WebSessionContext) -> String {
    let facilities = if session.active_tenant.site_scope.all_facilities {
        "All facilities".to_owned()
    } else {
        format!(
            "{} facilities",
            session.active_tenant.site_scope.facility_ids.len()
        )
    };
    let owners = if session.active_tenant.owner_scope.all_inventory_owners {
        "all clients".to_owned()
    } else {
        format!(
            "{} clients",
            session.active_tenant.owner_scope.inventory_owner_ids.len()
        )
    };
    format!("{facilities}, {owners}")
}
