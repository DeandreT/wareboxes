use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes, A},
    StaticSegment,
};
use wareboxes_core::dto::{OrderPage, SessionUser};
use wareboxes_core::models::{Facility, InventoryBalance, InventoryOwner, Order};

use crate::api;
use crate::view_model::{
    facility_inventory, format_quantity, has_permission, open_order_count, user_name,
};

#[cfg(target_arch = "wasm32")]
const SESSION_STORAGE_KEY: &str = "wareboxes.web.session.v1";

#[derive(Clone)]
enum SessionState {
    Anonymous,
    Authenticated(Box<SessionUser>),
}

#[derive(Clone, Default)]
struct WorkspaceData {
    orders: Option<OrderPage>,
    balances: Vec<InventoryBalance>,
    facilities: Vec<Facility>,
    inventory_owners: Vec<InventoryOwner>,
}

#[derive(Clone)]
enum WorkspaceState {
    Loading,
    Ready(WorkspaceData),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Overview,
    Orders,
    Inventory,
}

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta
                    name="viewport"
                    content="width=device-width, initial-scale=1, viewport-fit=cover"
                />
                <meta name="theme-color" content="#171b1d"/>
                <link rel="icon" href="/favicon.svg"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    let session_state = RwSignal::new(SessionState::Anonymous);
    provide_context(session_state);

    #[cfg(all(feature = "hydrate", target_arch = "wasm32"))]
    Effect::new(move || {
        leptos::task::spawn_local(restore_session(session_state));
    });

    view! {
        <Stylesheet id="wareboxes-web" href="/pkg/wareboxes-web.css"/>
        <Title text="Wareboxes"/>
        <Router>
            {move || match session_state.get() {
                SessionState::Anonymous => view! { <LoginPage/> }.into_any(),
                SessionState::Authenticated(session) => {
                    view! { <OperationsApp session=*session/> }.into_any()
                }
            }}
        </Router>
    }
}

#[component]
fn Brand() -> impl IntoView {
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
fn LoginPage() -> impl IntoView {
    let session_state = expect_context::<RwSignal<SessionState>>();
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let email_value = email.get_untracked().trim().to_owned();
        let password_value = password.get_untracked();
        if email_value.is_empty() || password_value.is_empty() {
            error.set(Some("Enter your email and password.".to_owned()));
            return;
        }

        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::login(email_value, password_value).await {
                Ok(session) => {
                    store_session(&session);
                    session_state.set(SessionState::Authenticated(Box::new(session)));
                }
                Err(api_error) => {
                    error.set(Some(api_error.message));
                    pending.set(false);
                }
            }
        });
    };

    view! {
        <main class="login-page">
            <section class="login-context">
                <Brand/>
                <div>
                    <p class="eyebrow">"Warehouse operations"</p>
                    <h1>"Control work, inventory, and exceptions."</h1>
                    <p class="login-intro">
                        "Sign in to the tenant and facility scope assigned to your account."
                    </p>
                </div>
                <dl class="scope-definitions">
                    <div>
                        <dt>"Tenant"</dt>
                        <dd>"Your operating organization"</dd>
                    </div>
                    <div>
                        <dt>"Facility"</dt>
                        <dd>"Your authorized sites"</dd>
                    </div>
                    <div>
                        <dt>"Owner"</dt>
                        <dd>"The inventory you may manage"</dd>
                    </div>
                </dl>
            </section>

            <section class="login-form-region" aria-labelledby="sign-in-title">
                <form class="login-form" on:submit=submit>
                    <div class="form-heading">
                        <p class="eyebrow">"Secure access"</p>
                        <h2 id="sign-in-title">"Sign in"</h2>
                        <p>"Use your Wareboxes operator account."</p>
                    </div>

                    <label for="email">"Email"</label>
                    <input
                        id="email"
                        name="email"
                        type="email"
                        autocomplete="username"
                        autofocus
                        required
                        prop:value=move || email.get()
                        on:input=move |event| email.set(event_target_value(&event))
                    />

                    <label for="password">"Password"</label>
                    <input
                        id="password"
                        name="password"
                        type="password"
                        autocomplete="current-password"
                        required
                        prop:value=move || password.get()
                        on:input=move |event| password.set(event_target_value(&event))
                    />

                    {move || {
                        error.get().map(|message| {
                            view! {
                                <div class="form-error" role="alert">
                                    <strong>"Sign-in failed"</strong>
                                    <span>{message}</span>
                                </div>
                            }
                        })
                    }}

                    <button class="primary-action" type="submit" disabled=move || pending.get()>
                        {move || if pending.get() { "Signing in..." } else { "Sign in" }}
                    </button>
                </form>
            </section>
        </main>
    }
}

#[component]
fn OperationsApp(session: SessionUser) -> impl IntoView {
    let workspace_state = RwSignal::new(WorkspaceState::Loading);
    provide_context(workspace_state);
    provide_context(session.clone());
    request_workspace(session, workspace_state);

    view! {
        <Routes fallback=|| view! { <NotFoundPage/> }.into_any()>
            <Route path=StaticSegment("") view=OverviewPage/>
            <Route path=StaticSegment("orders") view=OrdersPage/>
            <Route path=StaticSegment("inventory") view=InventoryPage/>
        </Routes>
    }
}

fn request_workspace(session: SessionUser, state: RwSignal<WorkspaceState>) {
    state.set(WorkspaceState::Loading);
    leptos::task::spawn_local(async move {
        match load_workspace(&session).await {
            Ok(data) => state.set(WorkspaceState::Ready(data)),
            Err(error) if error.unauthorized => {
                clear_stored_session();
                let root = expect_context::<RwSignal<SessionState>>();
                root.set(SessionState::Anonymous);
            }
            Err(error) => state.set(WorkspaceState::Failed(error.message)),
        }
    });
}

async fn load_workspace(session: &SessionUser) -> Result<WorkspaceData, api::ApiError> {
    let orders = if has_permission(session, "orders") {
        Some(api::orders(session).await?)
    } else {
        None
    };

    if !has_permission(session, "wms") {
        return Ok(WorkspaceData {
            orders,
            ..WorkspaceData::default()
        });
    }

    let balances = api::balances(session).await?;
    let facilities = api::facilities(session).await?;
    let inventory_owners = api::inventory_owners(session).await?;
    Ok(WorkspaceData {
        orders,
        balances,
        facilities,
        inventory_owners,
    })
}

#[component]
fn OverviewPage() -> impl IntoView {
    view! {
        <PageFrame section=Section::Overview>
            <WorkspaceContent section=Section::Overview/>
        </PageFrame>
    }
}

#[component]
fn OrdersPage() -> impl IntoView {
    view! {
        <PageFrame section=Section::Orders>
            <WorkspaceContent section=Section::Orders/>
        </PageFrame>
    }
}

#[component]
fn InventoryPage() -> impl IntoView {
    view! {
        <PageFrame section=Section::Inventory>
            <WorkspaceContent section=Section::Inventory/>
        </PageFrame>
    }
}

#[component]
fn WorkspaceContent(section: Section) -> impl IntoView {
    let state = expect_context::<RwSignal<WorkspaceState>>();
    let session = expect_context::<SessionUser>();

    move || match state.get() {
        WorkspaceState::Loading => view! { <WorkspaceLoading/> }.into_any(),
        WorkspaceState::Failed(message) => {
            view! { <WorkspaceError message session=session.clone() state/> }.into_any()
        }
        WorkspaceState::Ready(data) => match section {
            Section::Overview => view! { <Overview data/> }.into_any(),
            Section::Orders if has_permission(&session, "orders") => {
                view! { <Orders data/> }.into_any()
            }
            Section::Inventory if has_permission(&session, "wms") => {
                view! { <Inventory data/> }.into_any()
            }
            _ => view! { <AccessDenied/> }.into_any(),
        },
    }
}

#[component]
fn PageFrame(section: Section, children: Children) -> impl IntoView {
    let session = expect_context::<SessionUser>();
    let session_state = expect_context::<RwSignal<SessionState>>();
    let display_name = user_name(&session);
    let initials = display_name
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let tenant_name = session.active_tenant.name.clone();
    let email = session.user.email.clone();
    let can_view_orders = has_permission(&session, "orders");
    let can_view_inventory = has_permission(&session, "wms");
    let session_for_logout = session.clone();

    let sign_out = move |_| {
        let session = session_for_logout.clone();
        clear_stored_session();
        session_state.set(SessionState::Anonymous);
        leptos::task::spawn_local(async move {
            api::logout(&session).await;
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
                    <A
                        href="/"
                        attr:class=nav_class(section == Section::Overview)
                        attr:aria-current=aria_current(section == Section::Overview)
                    >
                        "Overview"
                    </A>
                    {can_view_orders.then(|| {
                        view! {
                            <A
                                href="/orders"
                                attr:class=nav_class(section == Section::Orders)
                                attr:aria-current=aria_current(section == Section::Orders)
                            >
                                "Orders"
                            </A>
                        }
                    })}
                    {can_view_inventory.then(|| {
                        view! {
                            <A
                                href="/inventory"
                                attr:class=nav_class(section == Section::Inventory)
                                attr:aria-current=aria_current(section == Section::Inventory)
                            >
                                "Inventory"
                            </A>
                        }
                    })}
                </nav>
                <div class="sidebar-scope">
                    <span>"Active tenant"</span>
                    <strong>{tenant_name.clone()}</strong>
                    <small>{session.active_tenant.slug.clone()}</small>
                </div>
            </aside>

            <div class="app-region">
                <header class="topbar">
                    <div class="tenant-heading">
                        <span>"Tenant"</span>
                        <strong>{tenant_name}</strong>
                    </div>
                    <div class="identity">
                        <span class="avatar" aria-hidden="true">{initials}</span>
                        <span class="identity-copy">
                            <strong>{display_name}</strong>
                            <small>{email}</small>
                        </span>
                        <button class="quiet-action" type="button" on:click=sign_out>
                            "Sign out"
                        </button>
                    </div>
                </header>
                <main class="workspace">{children()}</main>
            </div>
        </div>
    }
}

fn nav_class(active: bool) -> &'static str {
    if active {
        "nav-link active"
    } else {
        "nav-link"
    }
}

fn aria_current(active: bool) -> Option<&'static str> {
    active.then_some("page")
}

#[component]
fn WorkspaceLoading() -> impl IntoView {
    view! {
        <section class="workspace-state" aria-live="polite">
            <span class="loading-line" aria-hidden="true"></span>
            <h1>"Loading operations"</h1>
            <p>"Retrieving your authorized warehouse data."</p>
        </section>
    }
}

#[component]
fn WorkspaceError(
    message: String,
    session: SessionUser,
    state: RwSignal<WorkspaceState>,
) -> impl IntoView {
    let retry = move |_| request_workspace(session.clone(), state);
    view! {
        <section class="workspace-state error-state" role="alert">
            <p class="eyebrow">"Connection error"</p>
            <h1>"Operations data is unavailable"</h1>
            <p>{message}</p>
            <button class="primary-action compact" type="button" on:click=retry>
                "Retry"
            </button>
        </section>
    }
}

#[component]
fn AccessDenied() -> impl IntoView {
    view! {
        <section class="workspace-state">
            <p class="eyebrow">"Access restricted"</p>
            <h1>"This workspace is outside your assigned permissions."</h1>
            <A href="/">"Return to overview"</A>
        </section>
    }
}

#[component]
fn Overview(data: WorkspaceData) -> impl IntoView {
    let session = expect_context::<SessionUser>();
    let can_view_orders = has_permission(&session, "orders");
    let can_view_inventory = has_permission(&session, "wms");
    let total_on_hand = data
        .balances
        .iter()
        .map(|balance| balance.qty_on_hand)
        .sum::<i64>();
    let total_reserved = data
        .balances
        .iter()
        .map(|balance| balance.qty_reserved)
        .sum::<i64>();
    let order_total = data.orders.as_ref().map_or(0, |orders| orders.page.total);
    let open_orders = data
        .orders
        .as_ref()
        .map_or(0, |orders| open_order_count(&orders.summaries));
    let recent_orders = data
        .orders
        .as_ref()
        .map(|orders| {
            orders
                .page
                .items
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let facility_totals = facility_inventory(&data.balances);
    let facility_totals_empty = facility_totals.is_empty();
    let facility_count = data.facilities.len();
    let inventory_owner_count = data.inventory_owners.len();
    let overview_class = if can_view_orders && can_view_inventory {
        "overview-grid"
    } else {
        "overview-grid single"
    };

    view! {
        <section class="page-heading">
            <div>
                <p class="eyebrow">"Operations overview"</p>
                <h1>"Warehouse control"</h1>
                <p>"Current demand, stock, and scope across your authorized operation."</p>
            </div>
            <RefreshButton/>
        </section>

        {(can_view_orders || can_view_inventory).then(|| {
            view! {
                <section class="metric-band" aria-label="Operational totals">
                    {can_view_orders.then(|| {
                        view! {
                            <Metric label="Open orders" value=format_quantity(open_orders) tone="blue"/>
                            <Metric label="Orders in view" value=format_quantity(order_total) tone="neutral"/>
                        }
                    })}
                    {can_view_inventory.then(|| {
                        view! {
                            <Metric label="On hand" value=format_quantity(total_on_hand) tone="green"/>
                            <Metric label="Reserved" value=format_quantity(total_reserved) tone="amber"/>
                        }
                    })}
                </section>
            }
        })}

        <section class=overview_class>
            {can_view_orders.then(|| {
                view! {
                    <div class="data-section">
                        <div class="section-title">
                            <div>
                                <p class="eyebrow">"Demand"</p>
                                <h2>"Recent orders"</h2>
                            </div>
                            <A href="/orders">"View orders"</A>
                        </div>
                        <OrderTable orders=recent_orders compact=true/>
                    </div>
                }
            })}

            {can_view_inventory.then(|| {
                view! {
                    <div class="data-section facility-section">
                        <div class="section-title">
                            <div>
                                <p class="eyebrow">"Inventory"</p>
                                <h2>"Facility position"</h2>
                            </div>
                            <A href="/inventory">"View inventory"</A>
                        </div>
                        <div class="facility-list">
                            {facility_totals
                                .into_iter()
                                .map(|facility| {
                                    view! {
                                        <div class="facility-row">
                                            <div>
                                                <strong>{facility.facility_name}</strong>
                                                <small>{format!("{} stock positions", facility.positions)}</small>
                                            </div>
                                            <dl>
                                                <div>
                                                    <dt>"On hand"</dt>
                                                    <dd>{format_quantity(facility.on_hand)}</dd>
                                                </div>
                                                <div>
                                                    <dt>"Reserved"</dt>
                                                    <dd>{format_quantity(facility.reserved)}</dd>
                                                </div>
                                                <div>
                                                    <dt>"Held"</dt>
                                                    <dd>{format_quantity(facility.held)}</dd>
                                                </div>
                                            </dl>
                                        </div>
                                    }
                                })
                                .collect_view()}
                            {facility_totals_empty.then(|| {
                                view! { <EmptyState message="No inventory positions are currently in scope."/> }
                            })}
                        </div>
                    </div>
                }
            })}
        </section>

        {can_view_inventory.then(|| {
            view! {
                <section class="scope-band">
                    <div>
                        <span>"Facilities"</span>
                        <strong>{facility_count}</strong>
                    </div>
                    <div>
                        <span>"Inventory owners"</span>
                        <strong>{inventory_owner_count}</strong>
                    </div>
                    <p>"Counts reflect the site and owner scope assigned to this account."</p>
                </section>
            }
        })}
    }
}

#[component]
fn Metric(label: &'static str, value: String, tone: &'static str) -> impl IntoView {
    view! {
        <div class=format!("metric {tone}")>
            <span>{label}</span>
            <strong>{value}</strong>
        </div>
    }
}

#[component]
fn RefreshButton() -> impl IntoView {
    let state = expect_context::<RwSignal<WorkspaceState>>();
    let session = expect_context::<SessionUser>();
    let refresh = move |_| request_workspace(session.clone(), state);
    view! {
        <button class="secondary-action" type="button" on:click=refresh>
            "Refresh"
        </button>
    }
}

#[component]
fn Orders(data: WorkspaceData) -> impl IntoView {
    let orders = data
        .orders
        .map(|orders| orders.page.items)
        .unwrap_or_default();
    view! {
        <section class="page-heading">
            <div>
                <p class="eyebrow">"Outbound"</p>
                <h1>"Orders"</h1>
                <p>"The latest orders across your authorized inventory owners."</p>
            </div>
            <RefreshButton/>
        </section>
        <section class="data-section page-data">
            <OrderTable orders compact=false/>
        </section>
    }
}

#[component]
fn OrderTable(orders: Vec<Order>, compact: bool) -> impl IntoView {
    let empty = orders.is_empty();
    view! {
        <div class="table-scroll">
            <table class="data-table">
                <thead>
                    <tr>
                        <th>"Order"</th>
                        <th>"Owner"</th>
                        <th>"Status"</th>
                        <th class="numeric">"Units"</th>
                        <th>"Destination"</th>
                    </tr>
                </thead>
                <tbody>
                    {orders
                        .into_iter()
                        .map(|order| {
                            let destination = [order.city.as_deref(), order.state.as_deref()]
                                .into_iter()
                                .flatten()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(", ");
                            let destination = if destination.is_empty() {
                                "Not assigned".to_owned()
                            } else {
                                destination
                            };
                            view! {
                                <tr>
                                    <td>
                                        <strong>{order.order_key}</strong>
                                        {order.rush.then(|| view! { <small class="rush">"Rush"</small> })}
                                    </td>
                                    <td>{order.inventory_owner_name.unwrap_or_else(|| "Unassigned".to_owned())}</td>
                                    <td>
                                        <span class=status_class(order.status.as_str())>
                                            {order.status.to_string()}
                                        </span>
                                    </td>
                                    <td class="numeric">{format_quantity(order.ordered_qty)}</td>
                                    <td>{destination}</td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
            {empty.then(|| view! { <EmptyState message="No orders are currently in scope."/> })}
            {compact.then(|| view! { <span class="compact-table-edge" aria-hidden="true"></span> })}
        </div>
    }
}

fn status_class(status: &str) -> &'static str {
    match status {
        "shipped" => "status shipped",
        "cancelled" | "void" => "status muted",
        "held" => "status held",
        "processing" | "awaiting shipment" => "status processing",
        _ => "status open",
    }
}

#[component]
fn Inventory(data: WorkspaceData) -> impl IntoView {
    let empty = data.balances.is_empty();
    view! {
        <section class="page-heading">
            <div>
                <p class="eyebrow">"Stock control"</p>
                <h1>"Inventory"</h1>
                <p>"Current balances by facility, location, item, status, and owner."</p>
            </div>
            <RefreshButton/>
        </section>
        <section class="data-section page-data">
            <div class="table-scroll">
                <table class="data-table">
                    <thead>
                        <tr>
                            <th>"Facility"</th>
                            <th>"Location"</th>
                            <th>"Item"</th>
                            <th>"Status"</th>
                            <th class="numeric">"On hand"</th>
                            <th class="numeric">"Reserved"</th>
                            <th class="numeric">"Held"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {data
                            .balances
                            .into_iter()
                            .map(|balance| {
                                view! {
                                    <tr>
                                        <td>{balance.facility_name.unwrap_or_else(|| {
                                            format!("Facility {}", balance.facility_id)
                                        })}</td>
                                        <td>{format!("#{}", balance.location_id)}</td>
                                        <td>{format!("#{}", balance.item_id)}</td>
                                        <td>
                                            <span class="status open">{balance.status.to_string()}</span>
                                        </td>
                                        <td class="numeric strong">
                                            {format_quantity(balance.qty_on_hand)}
                                        </td>
                                        <td class="numeric">
                                            {format_quantity(balance.qty_reserved)}
                                        </td>
                                        <td class="numeric">{format_quantity(balance.qty_held)}</td>
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
                {empty.then(|| {
                    view! { <EmptyState message="No inventory balances are currently in scope."/> }
                })}
            </div>
        </section>
    }
}

#[component]
fn EmptyState(message: &'static str) -> impl IntoView {
    view! { <p class="empty-state">{message}</p> }
}

#[component]
fn NotFoundPage() -> impl IntoView {
    view! {
        <section class="workspace-state">
            <p class="eyebrow">"Not found"</p>
            <h1>"That workspace does not exist."</h1>
            <A href="/">"Return to overview"</A>
        </section>
    }
}

#[cfg(all(feature = "hydrate", target_arch = "wasm32"))]
async fn restore_session(state: RwSignal<SessionState>) {
    let Some(mut session) = read_stored_session() else {
        return;
    };
    match api::restore(&session).await {
        Ok(user) => {
            session.user = user;
            store_session(&session);
            state.set(SessionState::Authenticated(Box::new(session)));
        }
        Err(_) => {
            clear_stored_session();
            state.set(SessionState::Anonymous);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn read_stored_session() -> Option<SessionUser> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let value = storage.get_item(SESSION_STORAGE_KEY).ok().flatten()?;
    serde_json::from_str(&value).ok()
}

#[cfg(target_arch = "wasm32")]
fn store_session(session: &SessionUser) {
    let Some(storage) = web_sys::window()
        .and_then(|window| window.local_storage().ok())
        .flatten()
    else {
        return;
    };
    if let Ok(value) = serde_json::to_string(session) {
        let _ = storage.set_item(SESSION_STORAGE_KEY, &value);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn store_session(_session: &SessionUser) {}

#[cfg(target_arch = "wasm32")]
fn clear_stored_session() {
    if let Some(storage) = web_sys::window()
        .and_then(|window| window.local_storage().ok())
        .flatten()
    {
        let _ = storage.remove_item(SESSION_STORAGE_KEY);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_stored_session() {}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use leptos::prelude::*;
    use leptos_router::location::RequestUrl;

    use super::App;

    #[test]
    fn server_render_contains_hydratable_sign_in() {
        let html = Owner::new().with(|| {
            provide_context(RequestUrl::new("/"));
            view! { <App/> }.to_html()
        });
        assert!(html.contains("Sign in"));
        assert!(html.contains("Warehouse operations"));
        assert!(html.contains("wareboxes"));
    }
}
