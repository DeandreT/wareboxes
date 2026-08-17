use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes, A},
    StaticSegment,
};
use wareboxes_api_contract::v1::{
    AutomationWorkspaceResponse, CrossDockPlanningOptionPage, CrossDockWorkPage,
    CycleCountCandidatePage, CycleCountPolicyPage, CycleCountVariancePage, CycleCountWorkPage,
    InventoryBalanceResponse, InventoryHoldResponse, InventoryHoldStatus, OpaqueCursor,
    OutboundLoadQueuePage, PackingQueuePage, PickWavePage, PutawayCandidatePage, PutawayWorkPage,
    ReplenishmentPolicyPage, ReplenishmentQueuePage, ShippingQueuePage, SupportAccessPage,
    TenantLifecyclePage,
};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};
use wareboxes_core::dto::{OrderPage, WebSessionContext};
use wareboxes_core::models::{Item, Load, Location};

use crate::administration::{AdministrationArea, AdministrationWorkspace};
use crate::api;
use crate::app_frame::{Brand, PageFrame};
use crate::automation::AutomationWorkspace;
use crate::catalog::CatalogWorkbench;
use crate::components::{Icon, SearchField, UiIcon};
use crate::cross_dock::CrossDockWorkspace;
use crate::customer_portal::CustomerPortal;
use crate::customer_returns::CustomerReturnsWorkspace;
use crate::cycle_count::CycleCountWorkspace;
use crate::fulfillment::{LoadsWorkbench, OrdersWorkbench};
use crate::inbound_asns::InboundAsnWorkspace;
use crate::inventory::InventoryWorkspace;
use crate::inventory_disposition::InventoryDispositionWorkbench;
use crate::inventory_holds::QuantityHoldsWorkbench;
use crate::inventory_integrity::InventoryIntegrityWorkbench;
use crate::labor::LaborWorkspace;
use crate::orders::OrderTable;
use crate::outbound_loads::OutboundLoadsWorkspace;
use crate::packing::PackingWorkspace;
use crate::pick_waves::PickWavesWorkspace;
use crate::preferences::provide_display_preferences;
use crate::purchase_orders::PurchaseOrdersWorkspace;
use crate::putaway::PutawayWorkspace;
use crate::replenishment::ReplenishmentWorkspace;
use crate::service_accounts::ServiceAccountsWorkspace;
use crate::shipping::ShippingWorkspace;
use crate::slotting::SlottingWorkspace;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::support_access::SupportAccessWorkspace;
use crate::tenant_lifecycle::TenantLifecycleWorkspace;
use crate::toast::ToastProvider;
use crate::transfer_orders::TransferOrdersWorkspace;
use crate::value_added_work::ValueAddedWorkWorkspace;
use crate::vendor_returns::VendorReturnsWorkspace;
use crate::view_model::{facility_inventory, format_quantity, has_permission, open_order_count};
use crate::work_orchestration::WorkOrchestrationWorkspace;
use crate::yard::YardWorkspace;

const SESSION_BOOTSTRAP_ID: &str = "wareboxes-session-bootstrap";
const WORKSPACE_BOOTSTRAP_ID: &str = "wareboxes-workspace-bootstrap";

#[derive(Clone, Default)]
pub struct InitialWebSession(pub Option<WebSessionContext>);

#[derive(Clone, Default)]
pub struct InitialWebWorkspace(pub Option<WorkspaceBootstrap>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceBootstrapSection {
    Overview,
    Orders,
    PickWaves,
    Packing,
    Shipping,
    OutboundLoads,
    Putaway,
    CycleCounts,
    CrossDock,
    Inventory,
    InventoryIntegrity,
    Replenishment,
    Slotting,
    WorkOrchestration,
    Automation,
    ServiceAccounts,
    TenantLifecycle,
    SupportAccess,
    Access,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceBootstrapData {
    pub orders: Option<OrderPage>,
    pub pick_waves: Option<PickWavePage>,
    pub packing_queue: Option<PackingQueuePage>,
    pub shipping_queue: Option<ShippingQueuePage>,
    pub outbound_load_queue: Option<OutboundLoadQueuePage>,
    pub putaway_candidates: Option<PutawayCandidatePage>,
    pub putaway_work: Option<PutawayWorkPage>,
    pub cycle_count_candidates: Option<CycleCountCandidatePage>,
    pub cycle_count_work: Option<CycleCountWorkPage>,
    pub cycle_count_policies: Option<CycleCountPolicyPage>,
    pub cycle_count_variances: Option<CycleCountVariancePage>,
    pub cross_dock_planning_options: Option<CrossDockPlanningOptionPage>,
    pub cross_dock_work: Option<CrossDockWorkPage>,
    pub replenishment_policies: Option<ReplenishmentPolicyPage>,
    pub replenishment_queue: Option<ReplenishmentQueuePage>,
    pub automation_workspace: Option<AutomationWorkspaceResponse>,
    pub tenant_lifecycle_page: Option<TenantLifecyclePage>,
    pub support_access_page: Option<SupportAccessPage>,
    pub balances: Vec<InventoryBalanceResponse>,
    pub balance_next_cursor: Option<OpaqueCursor>,
    pub access: AccessScopeWorkspace,
    pub locations: Vec<Location>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "payload")]
pub enum WorkspaceBootstrapContent {
    Ready(Box<WorkspaceBootstrapData>),
    Failed(String),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceBootstrap {
    pub section: WorkspaceBootstrapSection,
    pub content: WorkspaceBootstrapContent,
}

#[derive(Clone)]
pub(crate) enum SessionState {
    Anonymous(Option<String>),
    Authenticated(Box<WebSessionContext>),
}

#[derive(Clone, Default)]
struct WorkspaceData {
    orders: Option<OrderPage>,
    pick_waves: Option<PickWavePage>,
    packing_queue: Option<PackingQueuePage>,
    shipping_queue: Option<ShippingQueuePage>,
    outbound_load_queue: Option<OutboundLoadQueuePage>,
    putaway_candidates: Option<PutawayCandidatePage>,
    putaway_work: Option<PutawayWorkPage>,
    cycle_count_candidates: Option<CycleCountCandidatePage>,
    cycle_count_work: Option<CycleCountWorkPage>,
    cycle_count_policies: Option<CycleCountPolicyPage>,
    cycle_count_variances: Option<CycleCountVariancePage>,
    cross_dock_planning_options: Option<CrossDockPlanningOptionPage>,
    cross_dock_work: Option<CrossDockWorkPage>,
    replenishment_policies: Option<ReplenishmentPolicyPage>,
    replenishment_queue: Option<ReplenishmentQueuePage>,
    automation_workspace: Option<AutomationWorkspaceResponse>,
    tenant_lifecycle_page: Option<TenantLifecyclePage>,
    support_access_page: Option<SupportAccessPage>,
    balances: Vec<InventoryBalanceResponse>,
    balance_next_cursor: Option<OpaqueCursor>,
    holds: Vec<InventoryHoldResponse>,
    hold_next_cursor: Option<OpaqueCursor>,
    access: AccessScopeWorkspace,
    loads: Vec<Load>,
    catalog_items: Vec<Item>,
    locations: Vec<Location>,
}

impl From<WorkspaceBootstrapData> for WorkspaceData {
    fn from(bootstrap: WorkspaceBootstrapData) -> Self {
        Self {
            orders: bootstrap.orders,
            pick_waves: bootstrap.pick_waves,
            packing_queue: bootstrap.packing_queue,
            shipping_queue: bootstrap.shipping_queue,
            outbound_load_queue: bootstrap.outbound_load_queue,
            putaway_candidates: bootstrap.putaway_candidates,
            putaway_work: bootstrap.putaway_work,
            cycle_count_candidates: bootstrap.cycle_count_candidates,
            cycle_count_work: bootstrap.cycle_count_work,
            cycle_count_policies: bootstrap.cycle_count_policies,
            cycle_count_variances: bootstrap.cycle_count_variances,
            cross_dock_planning_options: bootstrap.cross_dock_planning_options,
            cross_dock_work: bootstrap.cross_dock_work,
            replenishment_policies: bootstrap.replenishment_policies,
            replenishment_queue: bootstrap.replenishment_queue,
            automation_workspace: bootstrap.automation_workspace,
            tenant_lifecycle_page: bootstrap.tenant_lifecycle_page,
            support_access_page: bootstrap.support_access_page,
            balances: bootstrap.balances,
            balance_next_cursor: bootstrap.balance_next_cursor,
            access: bootstrap.access,
            locations: bootstrap.locations,
            ..Self::default()
        }
    }
}

#[derive(Clone)]
enum WorkspaceState {
    Loading,
    Ready(WorkspaceData),
    Refreshing(WorkspaceData),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Overview,
    Orders,
    PickWaves,
    Packing,
    Shipping,
    OutboundLoads,
    Yard,
    Labor,
    CustomerPortal,
    Putaway,
    CycleCounts,
    CrossDock,
    Loads,
    PurchaseOrders,
    TransferOrders,
    InboundAsns,
    CustomerReturns,
    VendorReturns,
    ValueAddedWork,
    Catalog,
    Inventory,
    InventoryHolds,
    InventoryDisposition,
    InventoryIntegrity,
    Replenishment,
    Slotting,
    WorkOrchestration,
    Automation,
    ServiceAccounts,
    TenantLifecycle,
    SupportAccess,
    Access,
    Administration(AdministrationArea),
}

impl Section {
    fn bootstrap_section(self) -> Option<WorkspaceBootstrapSection> {
        match self {
            Self::Overview => Some(WorkspaceBootstrapSection::Overview),
            Self::Orders => Some(WorkspaceBootstrapSection::Orders),
            Self::PickWaves => Some(WorkspaceBootstrapSection::PickWaves),
            Self::Packing => Some(WorkspaceBootstrapSection::Packing),
            Self::Shipping => Some(WorkspaceBootstrapSection::Shipping),
            Self::OutboundLoads => Some(WorkspaceBootstrapSection::OutboundLoads),
            Self::Putaway => Some(WorkspaceBootstrapSection::Putaway),
            Self::CycleCounts => Some(WorkspaceBootstrapSection::CycleCounts),
            Self::CrossDock => Some(WorkspaceBootstrapSection::CrossDock),
            Self::Inventory => Some(WorkspaceBootstrapSection::Inventory),
            Self::InventoryIntegrity => Some(WorkspaceBootstrapSection::InventoryIntegrity),
            Self::Replenishment => Some(WorkspaceBootstrapSection::Replenishment),
            Self::Slotting => Some(WorkspaceBootstrapSection::Slotting),
            Self::WorkOrchestration => Some(WorkspaceBootstrapSection::WorkOrchestration),
            Self::Automation => Some(WorkspaceBootstrapSection::Automation),
            Self::ServiceAccounts => Some(WorkspaceBootstrapSection::ServiceAccounts),
            Self::TenantLifecycle => Some(WorkspaceBootstrapSection::TenantLifecycle),
            Self::SupportAccess => Some(WorkspaceBootstrapSection::SupportAccess),
            Self::Access => Some(WorkspaceBootstrapSection::Access),
            _ => None,
        }
    }

    #[cfg(any(target_arch = "wasm32", all(test, feature = "ssr")))]
    fn supports_workspace_refresh(self) -> bool {
        matches!(
            self,
            Self::Overview | Self::Inventory | Self::InventoryDisposition | Self::Access
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResourceSort {
    Name,
    Id,
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
                <title>"Wareboxes"</title>
                <link rel="icon" href="/favicon.svg"/>
                <link rel="stylesheet" href="/pkg/wareboxes-web.css"/>
                <link rel="stylesheet" href="/holds.css"/>
                <link rel="stylesheet" href="/presentation.css"/>
                <link rel="stylesheet" href="/workspace-layout.css"/>
                <link rel="stylesheet" href="/disposition.css"/>
                <link rel="stylesheet" href="/inventory-integrity.css"/>
                <link rel="stylesheet" href="/inventory-rollups.css"/>
                <link rel="stylesheet" href="/fulfillment.css"/>
                <link rel="stylesheet" href="/inbound-asns.css"/>
                <link rel="stylesheet" href="/purchase-orders.css"/>
                <link rel="stylesheet" href="/transfer-orders.css"/>
                <link rel="stylesheet" href="/pick-shortages.css"/>
                <link rel="stylesheet" href="/order-allocation.css"/>
                <link rel="stylesheet" href="/packing.css"/>
                <link rel="stylesheet" href="/pick-waves.css"/>
                <link rel="stylesheet" href="/pick-clusters/workspace.css"/>
                <link rel="stylesheet" href="/pick-zones/workspace.css"/>
                <link rel="stylesheet" href="/shipping.css"/>
                <link rel="stylesheet" href="/shipping/carrier.css"/>
                <link rel="stylesheet" href="/outbound-loads.css"/>
                <link rel="stylesheet" href="/yard.css"/>
                <link rel="stylesheet" href="/labor.css"/>
                <link rel="stylesheet" href="/value-added-work.css"/>
                <link rel="stylesheet" href="/customer-portal.css"/>
                <link rel="stylesheet" href="/replenishment.css"/>
                <link rel="stylesheet" href="/slotting.css"/>
                <link rel="stylesheet" href="/work-orchestration/workspace.css"/>
                <link rel="stylesheet" href="/automation/workspace.css"/>
                <link rel="stylesheet" href="/service-accounts/workspace.css"/>
                <link rel="stylesheet" href="/tenant-lifecycle/workspace.css"/>
                <link rel="stylesheet" href="/support-access/workspace.css"/>
                <link rel="stylesheet" href="/putaway.css"/>
                <link rel="stylesheet" href="/cycle-count.css"/>
                <link rel="stylesheet" href="/cross-dock.css"/>
                <link rel="stylesheet" href="/catalog.css"/>
                <link rel="stylesheet" href="/administration.css"/>
                <link rel="stylesheet" href="/integration-monitor.css"/>
                <script src="/presentation-init.js"></script>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_display_preferences();
    let initial_session = initial_web_session();
    let initial_workspace = initial_web_workspace();
    let session_bootstrap = serialize_bootstrap(&initial_session);
    let workspace_bootstrap = serialize_bootstrap(&initial_workspace);
    let session_state = RwSignal::new(match initial_session {
        Some(session) => SessionState::Authenticated(Box::new(session)),
        None => SessionState::Anonymous(None),
    });
    provide_context(session_state);
    provide_context(InitialWebWorkspace(initial_workspace));

    view! {
        <script
            id=SESSION_BOOTSTRAP_ID
            type="application/json"
            inner_html=session_bootstrap
        ></script>
        <script
            id=WORKSPACE_BOOTSTRAP_ID
            type="application/json"
            inner_html=workspace_bootstrap
        ></script>
        <ToastProvider>
            <Router>
                <Routes fallback=|| view! { <NotFoundPage/> }.into_any()>
                    <Route path=StaticSegment("") view=OverviewPage/>
                    <Route path=StaticSegment("orders") view=OrdersPage/>
                    <Route path=StaticSegment("pick-waves") view=PickWavesPage/>
                    <Route path=StaticSegment("packing") view=PackingPage/>
                    <Route path=StaticSegment("shipping") view=ShippingPage/>
                    <Route path=StaticSegment("outbound-loads") view=OutboundLoadsPage/>
                    <Route path=StaticSegment("yard") view=YardPage/>
                    <Route path=StaticSegment("labor") view=LaborPage/>
                    <Route path=StaticSegment("portal") view=CustomerPortalPage/>
                    <Route path=StaticSegment("putaway") view=PutawayPage/>
                    <Route path=StaticSegment("cycle-counts") view=CycleCountsPage/>
                    <Route path=StaticSegment("cross-dock") view=CrossDockPage/>
                    <Route path=StaticSegment("loads") view=LoadsPage/>
                    <Route path=StaticSegment("purchase-orders") view=PurchaseOrdersPage/>
                    <Route path=StaticSegment("transfer-orders") view=TransferOrdersPage/>
                    <Route path=StaticSegment("inbound-asns") view=InboundAsnsPage/>
                    <Route path=StaticSegment("customer-returns") view=CustomerReturnsPage/>
                    <Route path=StaticSegment("vendor-returns") view=VendorReturnsPage/>
                    <Route path=StaticSegment("value-added-work") view=ValueAddedWorkPage/>
                    <Route path=StaticSegment("catalog") view=CatalogPage/>
                    <Route path=StaticSegment("inventory") view=InventoryPage/>
                    <Route path=StaticSegment("replenishment") view=ReplenishmentPage/>
                    <Route path=StaticSegment("slotting") view=SlottingPage/>
                    <Route path=StaticSegment("work-orchestration") view=WorkOrchestrationPage/>
                    <Route path=StaticSegment("automation") view=AutomationPage/>
                    <Route path=(StaticSegment("administration"), StaticSegment("service-accounts")) view=ServiceAccountsPage/>
                    <Route path=(StaticSegment("platform"), StaticSegment("tenants")) view=TenantLifecyclePage/>
                    <Route path=(StaticSegment("platform"), StaticSegment("support-access")) view=SupportAccessPage/>
                    <Route path=(StaticSegment("inventory"), StaticSegment("holds")) view=InventoryHoldsPage/>
                    <Route
                        path=(StaticSegment("inventory"), StaticSegment("disposition"))
                        view=InventoryDispositionPage
                    />
                    <Route
                        path=(StaticSegment("inventory"), StaticSegment("control"))
                        view=InventoryIntegrityPage
                    />
                    <Route path=StaticSegment("access") view=AccessPage/>
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("clients"))
                        view=ClientsPage
                    />
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("users"))
                        view=UsersPage
                    />
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("roles"))
                        view=RolesPage
                    />
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("permissions"))
                        view=PermissionsPage
                    />
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("employees"))
                        view=EmployeesPage
                    />
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("integrations"))
                        view=IntegrationsPage
                    />
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("count-plans"))
                        view=CountPlansPage
                    />
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("configuration"))
                        view=ConfigurationPage
                    />
                    <Route
                        path=(StaticSegment("administration"), StaticSegment("billing"))
                        view=BillingPage
                    />
                </Routes>
            </Router>
        </ToastProvider>
    }
}

#[component]
fn LoginPage(notice: Option<String>) -> impl IntoView {
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
                        "Sign in to the organization, facility, and client scope assigned to your operator profile."
                    </p>
                </div>
                <dl class="scope-definitions">
                    <div>
                        <dt>"Organization"</dt>
                        <dd>"Your warehouse operation"</dd>
                    </div>
                    <div>
                        <dt>"Facility"</dt>
                        <dd>"Your authorized sites"</dd>
                    </div>
                    <div>
                        <dt>"Client"</dt>
                        <dd>"The stock you may manage"</dd>
                    </div>
                </dl>
            </section>

            <section class="login-form-region" aria-labelledby="sign-in-title">
                <form class="login-form" on:submit=submit>
                    <div class="form-heading">
                        <p class="eyebrow">"Secure access"</p>
                        <h2 id="sign-in-title">"Sign in"</h2>
                        <p>"Use your Wareboxes operator profile."</p>
                    </div>

                    {notice.map(|message| {
                        view! {
                            <div class="session-notice" role="status">{message}</div>
                        }
                    })}

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

                    <button class="button primary-action" type="submit" disabled=move || pending.get()>
                        {move || if pending.get() { "Signing in..." } else { "Sign in" }}
                    </button>
                </form>
            </section>
        </main>
    }
}

#[component]
fn AuthenticatedPage(section: Section) -> impl IntoView {
    let session_state = expect_context::<RwSignal<SessionState>>();

    move || match session_state.get() {
        SessionState::Anonymous(notice) => view! { <LoginPage notice/> }.into_any(),
        SessionState::Authenticated(session) => {
            view! { <AuthenticatedWorkspace session=*session section/> }.into_any()
        }
    }
}

#[component]
fn AuthenticatedWorkspace(session: WebSessionContext, section: Section) -> impl IntoView {
    let initial_state = initial_workspace_state(section);
    #[cfg(target_arch = "wasm32")]
    let needs_initial_request = initial_state.is_none();
    let workspace_state = RwSignal::new(initial_state.unwrap_or(WorkspaceState::Loading));
    provide_context(workspace_state);
    provide_context(session.clone());
    #[cfg(target_arch = "wasm32")]
    {
        if needs_initial_request {
            request_workspace(session.clone(), section, workspace_state);
        }
        install_workspace_auto_refresh(session, section, workspace_state);
    }

    view! {
        <PageFrame section>
            <WorkspaceContent section/>
        </PageFrame>
    }
}

fn initial_workspace_state(section: Section) -> Option<WorkspaceState> {
    let bootstrap = use_context::<InitialWebWorkspace>()?.0?;
    if section.bootstrap_section()? != bootstrap.section {
        return None;
    }
    Some(match bootstrap.content {
        WorkspaceBootstrapContent::Ready(data) => WorkspaceState::Ready((*data).into()),
        WorkspaceBootstrapContent::Failed(message) => WorkspaceState::Failed(message),
    })
}

#[cfg(target_arch = "wasm32")]
fn install_workspace_auto_refresh(
    session: WebSessionContext,
    section: Section,
    state: RwSignal<WorkspaceState>,
) {
    use std::time::Duration;

    if !section.supports_workspace_refresh() {
        return;
    }
    let Some(owner) = Owner::current() else {
        return;
    };
    let Ok(handle) = set_interval_with_handle(
        move || {
            if workspace_is_ready_for_auto_refresh(&state) {
                owner.with(|| request_workspace(session.clone(), section, state));
            }
        },
        Duration::from_secs(30),
    ) else {
        return;
    };
    on_cleanup(move || handle.clear());
}

#[cfg(any(target_arch = "wasm32", all(test, feature = "ssr")))]
fn workspace_is_ready_for_auto_refresh(state: &RwSignal<WorkspaceState>) -> bool {
    matches!(state.get_untracked(), WorkspaceState::Ready(_))
}

fn request_workspace(
    session: WebSessionContext,
    section: Section,
    state: RwSignal<WorkspaceState>,
) {
    let previous = match state.get_untracked() {
        WorkspaceState::Ready(data) | WorkspaceState::Refreshing(data) => Some(data),
        WorkspaceState::Loading | WorkspaceState::Failed(_) => None,
    };
    state.set(previous.map_or(WorkspaceState::Loading, WorkspaceState::Refreshing));
    leptos::task::spawn_local(async move {
        match load_workspace(&session, section).await {
            Ok(data) => state.set(WorkspaceState::Ready(data)),
            Err(error) if error.unauthorized => {
                let root = expect_context::<RwSignal<SessionState>>();
                root.set(SessionState::Anonymous(Some(
                    "Your session ended. Sign in to continue.".to_owned(),
                )));
            }
            Err(error) => state.set(WorkspaceState::Failed(error.message)),
        }
    });
}

async fn load_workspace(
    session: &WebSessionContext,
    section: Section,
) -> Result<WorkspaceData, api::ApiError> {
    let mut data = WorkspaceData::default();
    match section {
        Section::Overview => {
            data.access = api::access().await?;
            if has_permission(session, "orders") {
                data.orders = Some(api::orders().await?);
            }
            if has_permission(session, "wms") {
                let page = api::balances(None).await?;
                data.balances = page.items;
                data.balance_next_cursor = page.next_cursor;
            }
        }
        Section::Orders if has_permission(session, "orders") => {
            data.orders = Some(api::orders_workbench().await?);
            data.access = api::access().await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::PickWaves if has_permission(session, "wms_supervisor") => {
            data.pick_waves = Some(
                api::pick_waves(
                    None,
                    None,
                    wareboxes_api_contract::v1::PickWaveSort::PlannedAt,
                    wareboxes_api_contract::v1::PickWaveSortDirection::Desc,
                    None,
                )
                .await?,
            );
            data.orders = Some(
                api::internal_get(
                    "/api/orders?limit=100&offset=0&status=open&sort=order&direction=asc",
                )
                .await?,
            );
            data.access = api::access().await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::Packing if has_permission(session, "wms") => {
            data.packing_queue = Some(api::packing_queue(None, None).await?);
            data.access = api::access().await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::Shipping if has_permission(session, "wms") => {
            data.shipping_queue =
                Some(api::internal_get("/api/v1/shipping-queue?limit=100").await?);
            data.access = api::access().await?;
        }
        Section::OutboundLoads if has_permission(session, "wms") => {
            data.outbound_load_queue =
                Some(api::internal_get("/api/v1/outbound-loads?limit=100").await?);
            data.shipping_queue =
                Some(api::internal_get("/api/v1/shipping-queue?limit=100").await?);
            data.access = api::access().await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::Yard if has_permission(session, "wms") => {
            data.access = api::access().await?;
        }
        Section::Labor if has_permission(session, "labor_view") => {
            data.access = api::access().await?;
        }
        Section::Putaway if has_permission(session, "wms") => {
            data.putaway_candidates = Some(
                api::putaway_candidates(
                    None,
                    None,
                    None,
                    wareboxes_api_contract::v1::PutawayCandidateSort::default(),
                    wareboxes_api_contract::v1::PutawaySortDirection::default(),
                    None,
                )
                .await?,
            );
            data.putaway_work = Some(
                api::putaway_work(
                    None,
                    None,
                    None,
                    None,
                    wareboxes_api_contract::v1::PutawayWorkSort::default(),
                    wareboxes_api_contract::v1::PutawaySortDirection::default(),
                    None,
                )
                .await?,
            );
            data.access = api::access().await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::CycleCounts if has_permission(session, "wms_supervisor") => {
            data.cycle_count_candidates = Some(
                api::cycle_count_candidates(
                    None,
                    None,
                    None,
                    wareboxes_api_contract::v1::CycleCountCandidateSort::default(),
                    wareboxes_api_contract::v1::CycleCountSortDirection::default(),
                    None,
                )
                .await?,
            );
            data.cycle_count_work = Some(
                api::cycle_count_work(
                    None,
                    None,
                    None,
                    wareboxes_api_contract::v1::CycleCountWorkSort::default(),
                    wareboxes_api_contract::v1::CycleCountSortDirection::Desc,
                    None,
                )
                .await?,
            );
            data.cycle_count_policies = Some(api::cycle_count_policies(None, None, None).await?);
            data.cycle_count_variances =
                Some(api::cycle_count_variances(None, None, None, None).await?);
            data.access = api::access().await?;
        }
        Section::Loads if has_permission(session, "wms") => {
            data.loads =
                api::internal_get("/api/loads?offset=0&limit=100&sort=appointment&direction=asc")
                    .await?;
            data.access = api::access().await?;
            data.catalog_items = api::internal_get("/api/items?show_deleted=false").await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::InboundAsns if has_permission(session, "wms") => {
            data.access = api::access().await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::CustomerReturns if has_permission(session, "wms") => {
            data.access = api::access().await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::VendorReturns if has_permission(session, "wms") => {
            data.access = api::access().await?;
        }
        Section::ValueAddedWork if has_permission(session, "wms") => {
            data.access = api::access().await?;
        }
        Section::PurchaseOrders if has_permission(session, "wms") => {
            data.access = api::access().await?;
        }
        Section::TransferOrders if has_permission(session, "wms") => {
            data.access = api::access().await?;
        }
        Section::Catalog if has_permission(session, "wms") => {}
        Section::Inventory if has_permission(session, "wms") => {
            let page = api::balances(None).await?;
            data.balances = page.items;
            data.balance_next_cursor = page.next_cursor;
        }
        Section::InventoryHolds if has_permission(session, "wms") => {
            let balance_page = api::sorted_balances(
                None,
                wareboxes_api_contract::v1::InventoryBalanceSort::Facility,
                wareboxes_api_contract::v1::InventorySortDirection::Ascending,
                None,
            )
            .await?;
            let hold_page = api::holds(InventoryHoldStatus::Active, None).await?;
            data.balances = balance_page.items;
            data.balance_next_cursor = balance_page.next_cursor;
            data.holds = hold_page.items;
            data.hold_next_cursor = hold_page.next_cursor;
        }
        Section::InventoryDisposition if has_permission(session, "wms") => {
            let page = api::balances(None).await?;
            data.balances = page.items;
            data.balance_next_cursor = page.next_cursor;
        }
        Section::InventoryIntegrity if has_permission(session, "wms") => {
            data.access = api::access().await?;
        }
        Section::Replenishment if has_permission(session, "wms_supervisor") => {
            data.replenishment_policies = Some(
                api::replenishment_policies(api::ReplenishmentPolicyFilters::default(), None)
                    .await?,
            );
            data.replenishment_queue = Some(
                api::replenishment_queue(api::ReplenishmentQueueFilters::default(), None).await?,
            );
            data.access = api::access().await?;
        }
        Section::Slotting if has_permission(session, "wms") => {
            data.access = api::access().await?;
        }
        Section::WorkOrchestration if has_permission(session, "wms") => {
            data.access = api::access().await?;
            data.locations = api::internal_get("/api/locations?show_deleted=false").await?;
        }
        Section::Automation if has_permission(session, "wms_supervisor") => {
            data.automation_workspace = Some(api::automation_workspace(None, false).await?);
            data.access = api::access().await?;
        }
        Section::ServiceAccounts if has_permission(session, "admin") => {
            data.access = api::access().await?;
        }
        Section::TenantLifecycle if session.is_platform_administrator => {
            data.tenant_lifecycle_page = Some(
                api::tenant_lifecycle_page(
                    &wareboxes_api_contract::v1::TenantLifecyclePageRequest {
                        status: None,
                        search: None,
                        cursor: None,
                        limit: wareboxes_api_contract::v1::PageLimit::default(),
                    },
                )
                .await?,
            );
        }
        Section::SupportAccess if session.is_platform_administrator => {
            data.support_access_page = Some(
                api::support_access_page(&wareboxes_api_contract::v1::SupportAccessPageRequest {
                    tenant_id: None,
                    status: None,
                    cursor: None,
                    limit: wareboxes_api_contract::v1::PageLimit::default(),
                })
                .await?,
            );
            data.tenant_lifecycle_page = Some(
                api::tenant_lifecycle_page(
                    &wareboxes_api_contract::v1::TenantLifecyclePageRequest {
                        status: Some(wareboxes_api_contract::v1::TenantStatus::Active),
                        search: None,
                        cursor: None,
                        limit: wareboxes_api_contract::v1::PageLimit::default(),
                    },
                )
                .await?,
            );
        }
        Section::CrossDock if has_permission(session, "wms_supervisor") => {
            data.cross_dock_planning_options = Some(
                api::cross_dock_planning_options(api::CrossDockFilters::default(), None).await?,
            );
            data.cross_dock_work =
                Some(api::cross_dock_work(api::CrossDockFilters::default(), None).await?);
            data.access = api::access().await?;
        }
        Section::Access => {
            data.access = api::access().await?;
        }
        Section::Administration(_) if has_permission(session, "admin") => {}
        Section::CustomerPortal if has_permission(session, "customer_portal") => {}
        Section::Orders
        | Section::PickWaves
        | Section::Packing
        | Section::Shipping
        | Section::OutboundLoads
        | Section::Yard
        | Section::Labor
        | Section::CustomerPortal
        | Section::Putaway
        | Section::CycleCounts
        | Section::CrossDock
        | Section::Loads
        | Section::PurchaseOrders
        | Section::TransferOrders
        | Section::InboundAsns
        | Section::CustomerReturns
        | Section::VendorReturns
        | Section::ValueAddedWork
        | Section::Catalog
        | Section::Inventory
        | Section::InventoryHolds
        | Section::InventoryDisposition
        | Section::InventoryIntegrity
        | Section::Replenishment
        | Section::Slotting
        | Section::WorkOrchestration
        | Section::Automation
        | Section::ServiceAccounts
        | Section::TenantLifecycle
        | Section::SupportAccess
        | Section::Administration(_) => {}
    }
    Ok(data)
}

#[component]
fn OverviewPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Overview/> }
}

#[component]
fn OrdersPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Orders/> }
}

#[component]
fn PickWavesPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::PickWaves/> }
}

#[component]
fn PackingPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Packing/> }
}

#[component]
fn ShippingPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Shipping/> }
}

#[component]
fn OutboundLoadsPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::OutboundLoads/> }
}

#[component]
fn YardPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Yard/> }
}

#[component]
fn LaborPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Labor/> }
}

#[component]
fn CustomerPortalPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::CustomerPortal/> }
}

#[component]
fn PutawayPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Putaway/> }
}

#[component]
fn CycleCountsPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::CycleCounts/> }
}

#[component]
fn LoadsPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Loads/> }
}

#[component]
fn InboundAsnsPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::InboundAsns/> }
}

#[component]
fn CustomerReturnsPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::CustomerReturns/> }
}

#[component]
fn VendorReturnsPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::VendorReturns/> }
}

#[component]
fn ValueAddedWorkPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::ValueAddedWork/> }
}

#[component]
fn PurchaseOrdersPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::PurchaseOrders/> }
}

#[component]
fn TransferOrdersPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::TransferOrders/> }
}

#[component]
fn CatalogPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Catalog/> }
}

#[component]
fn InventoryPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Inventory/> }
}

#[component]
fn InventoryHoldsPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::InventoryHolds/> }
}

#[component]
fn InventoryDispositionPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::InventoryDisposition/> }
}

#[component]
fn InventoryIntegrityPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::InventoryIntegrity/> }
}

#[component]
fn ReplenishmentPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Replenishment/> }
}

#[component]
fn SlottingPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Slotting/> }
}

#[component]
fn WorkOrchestrationPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::WorkOrchestration/> }
}

#[component]
fn AutomationPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Automation/> }
}

#[component]
fn ServiceAccountsPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::ServiceAccounts/> }
}

#[component]
fn TenantLifecyclePage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::TenantLifecycle/> }
}

#[component]
fn SupportAccessPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::SupportAccess/> }
}

#[component]
fn CrossDockPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::CrossDock/> }
}

#[component]
fn AccessPage() -> impl IntoView {
    view! { <AuthenticatedPage section=Section::Access/> }
}

#[component]
fn ClientsPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::Clients)/>
    }
}

#[component]
fn UsersPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::Users)/>
    }
}

#[component]
fn RolesPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::Roles)/>
    }
}

#[component]
fn PermissionsPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::Permissions)/>
    }
}

#[component]
fn EmployeesPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::Employees)/>
    }
}

#[component]
fn CountPlansPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::CountPlans)/>
    }
}

#[component]
fn ConfigurationPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::Configuration)/>
    }
}

#[component]
fn BillingPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::Billing)/>
    }
}

#[component]
fn IntegrationsPage() -> impl IntoView {
    view! {
        <AuthenticatedPage section=Section::Administration(AdministrationArea::Integrations)/>
    }
}

#[component]
fn WorkspaceContent(section: Section) -> impl IntoView {
    let state = expect_context::<RwSignal<WorkspaceState>>();
    let session = expect_context::<WebSessionContext>();

    move || {
        match state.get() {
        WorkspaceState::Loading => view! { <WorkspaceLoading/> }.into_any(),
        WorkspaceState::Failed(message) => {
            view! { <WorkspaceError message session=session.clone() section state/> }.into_any()
        }
        WorkspaceState::Ready(data) | WorkspaceState::Refreshing(data) => match section {
            Section::Overview => view! { <Overview data/> }.into_any(),
            Section::Orders if has_permission(&session, "orders") => {
                view! { <Orders data on_unauthorized=session_expired_callback()/> }.into_any()
            }
            Section::PickWaves if has_permission(&session, "wms_supervisor") => view! {
                <PickWavesWorkspace
                    initial_page=data.pick_waves.unwrap_or_else(|| PickWavePage::new(Vec::new(), None))
                    initial_orders=data.orders.unwrap_or_else(empty_order_page)
                    access=data.access
                    locations=data.locations
                    on_unauthorized=session_expired_callback()
                />
            }.into_any(),
            Section::Packing if has_permission(&session, "wms") => view! {
                <PackingWorkspace
                    initial_queue=data.packing_queue.unwrap_or_else(|| PackingQueuePage::new(Vec::new(), None))
                    access=data.access
                    locations=data.locations
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::Shipping if has_permission(&session, "wms") => view! {
                <ShippingWorkspace
                    initial_queue=data.shipping_queue.unwrap_or_else(|| ShippingQueuePage::new(Vec::new(), None))
                    access=data.access
                    can_configure_origins=has_permission(&session, "admin")
                    can_configure_qa=has_permission(&session, "wms_supervisor")
                    can_manage_carriers=has_permission(&session, "admin")
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::OutboundLoads if has_permission(&session, "wms") => view! {
                <OutboundLoadsWorkspace
                    initial_queue=data.outbound_load_queue.unwrap_or_else(|| OutboundLoadQueuePage::new(Vec::new(), None))
                    shipping_queue=data.shipping_queue.unwrap_or_else(|| ShippingQueuePage::new(Vec::new(), None))
                    access=data.access
                    locations=data.locations
                    can_supervise=has_permission(&session, "wms_supervisor")
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::Yard if has_permission(&session, "wms") => view! {
                <YardWorkspace
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::Labor if has_permission(&session, "labor_view") => view! {
                <LaborWorkspace
                    access=data.access
                    can_execute=has_permission(&session, "labor_execute") || has_permission(&session, "labor_supervise")
                    can_configure=has_permission(&session, "labor_configure")
                    can_manage_equipment=has_permission(&session, "labor_equipment")
                    can_certify=has_permission(&session, "labor_certify")
                    can_supervise=has_permission(&session, "labor_supervise")
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::CustomerPortal if has_permission(&session, "customer_portal") => view! {
                <CustomerPortal on_unauthorized=session_expired_callback()/>
            }
            .into_any(),
            Section::Putaway if has_permission(&session, "wms") => view! {
                <PutawayWorkspace
                    initial_candidates=data.putaway_candidates
                        .unwrap_or_else(|| PutawayCandidatePage::new(Vec::new(), None))
                    initial_work=data.putaway_work
                        .unwrap_or_else(|| PutawayWorkPage::new(Vec::new(), None))
                    access=data.access
                    locations=data.locations
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::CycleCounts if has_permission(&session, "wms_supervisor") => view! {
                <CycleCountWorkspace
                    initial_candidates=data.cycle_count_candidates
                        .unwrap_or_else(|| CycleCountCandidatePage::new(Vec::new(), None))
                    initial_work=data.cycle_count_work
                        .unwrap_or_else(|| CycleCountWorkPage::new(Vec::new(), None))
                    initial_policies=data.cycle_count_policies
                        .unwrap_or_else(|| CycleCountPolicyPage::new(Vec::new(), None))
                    initial_variances=data.cycle_count_variances
                        .unwrap_or_else(|| CycleCountVariancePage::new(Vec::new(), None))
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::Loads if has_permission(&session, "wms") => {
                view! { <Loads data on_unauthorized=session_expired_callback()/> }.into_any()
            }
            Section::InboundAsns if has_permission(&session, "wms") => view! {
                <InboundAsnWorkspace
                    access=data.access
                    locations=data.locations
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::CustomerReturns if has_permission(&session, "wms") => view! {
                <CustomerReturnsWorkspace
                    access=data.access
                    locations=data.locations
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::VendorReturns if has_permission(&session, "wms") => view! {
                <VendorReturnsWorkspace
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::ValueAddedWork if has_permission(&session, "wms") => view! {
                <ValueAddedWorkWorkspace
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::PurchaseOrders if has_permission(&session, "wms") => view! {
                <PurchaseOrdersWorkspace
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::TransferOrders if has_permission(&session, "wms") => view! {
                <TransferOrdersWorkspace
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::Catalog if has_permission(&session, "wms") => {
                view! { <CatalogWorkbench on_unauthorized=session_expired_callback() can_supervise=has_permission(&session,"wms_supervisor")/> }.into_any()
            }
            Section::Inventory if has_permission(&session, "wms") => {
                view! { <Inventory data/> }.into_any()
            }
            Section::InventoryHolds if has_permission(&session, "wms") => {
                let session_state = expect_context::<RwSignal<SessionState>>();
                let on_unauthorized = Callback::new(move |_| {
                    session_state.set(SessionState::Anonymous(Some(
                        "Your session ended. Sign in to continue.".to_owned(),
                    )));
                });
                view! {
                    <InventoryHolds
                        data
                        can_inspect_receipts=has_permission(&session, "wms_supervisor")
                        on_unauthorized
                    />
                }
                .into_any()
            }
            Section::InventoryDisposition if has_permission(&session, "wms") => {
                let session_state = expect_context::<RwSignal<SessionState>>();
                let on_unauthorized = Callback::new(move |_| {
                    session_state.set(SessionState::Anonymous(Some(
                        "Your session ended. Sign in to continue.".to_owned(),
                    )));
                });
                view! { <InventoryDisposition data on_unauthorized/> }.into_any()
            }
            Section::InventoryIntegrity if has_permission(&session, "wms") => {
                let session_state = expect_context::<RwSignal<SessionState>>();
                let on_unauthorized = Callback::new(move |_| {
                    session_state.set(SessionState::Anonymous(Some(
                        "Your session ended. Sign in to continue.".to_owned(),
                    )));
                });
                view! { <InventoryIntegrity access=data.access on_unauthorized/> }.into_any()
            }
            Section::Replenishment if has_permission(&session, "wms_supervisor") => view! {
                <ReplenishmentWorkspace
                    initial_policies=data.replenishment_policies
                    initial_work=data.replenishment_queue
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::Slotting if has_permission(&session, "wms") => view! {
                <SlottingWorkspace
                    access=data.access
                    can_supervise=has_permission(&session, "wms_supervisor")
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::WorkOrchestration if has_permission(&session, "wms") => view! {
                <WorkOrchestrationWorkspace
                    access=data.access
                    locations=data.locations
                    can_supervise=has_permission(&session, "wms_supervisor")
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::Automation if has_permission(&session, "wms_supervisor") => view! {
                <AutomationWorkspace
                    initial_workspace=data.automation_workspace.unwrap_or(AutomationWorkspaceResponse {
                        devices: Vec::new(), commands: Vec::new(), heartbeats: Vec::new(), truncated: false,
                    })
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::ServiceAccounts if has_permission(&session, "admin") => view! {
                <ServiceAccountsWorkspace
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::TenantLifecycle if session.is_platform_administrator => view! {
                <TenantLifecycleWorkspace
                    initial_page=data.tenant_lifecycle_page
                    current_tenant_id=session.active_tenant.tenant_id.get()
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::SupportAccess if session.is_platform_administrator => view! {
                <SupportAccessWorkspace
                    initial_page=data.support_access_page
                    initial_tenants=data.tenant_lifecycle_page
                    current_user_id=session.user.id
                    can_manage=session.active_support_access_id.is_none()
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::CrossDock if has_permission(&session, "wms_supervisor") => view! {
                <CrossDockWorkspace
                    initial_options=data.cross_dock_planning_options
                    initial_work=data.cross_dock_work
                    access=data.access
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            Section::Access => view! { <Access data/> }.into_any(),
            Section::Administration(area) if has_permission(&session, "admin") => view! {
                <AdministrationWorkspace
                    area
                    on_unauthorized=session_expired_callback()
                />
            }
            .into_any(),
            _ => view! { <AccessDenied/> }.into_any(),
        },
    }
    }
}

fn session_expired_callback() -> Callback<()> {
    let session_state = expect_context::<RwSignal<SessionState>>();
    Callback::new(move |_| {
        session_state.set(SessionState::Anonymous(Some(
            "Your session ended. Sign in to continue.".to_owned(),
        )));
    })
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
    session: WebSessionContext,
    section: Section,
    state: RwSignal<WorkspaceState>,
) -> impl IntoView {
    let retry = move |_| request_workspace(session.clone(), section, state);
    view! {
        <section class="workspace-state error-state" role="alert">
            <Icon icon=UiIcon::Alert/>
            <p class="eyebrow">"Connection error"</p>
            <h1>"Operations data is unavailable"</h1>
            <p>{message}</p>
            <button class="button primary-action compact" type="button" on:click=retry>
                <Icon icon=UiIcon::Refresh/>
                <span>"Retry"</span>
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
    let session = expect_context::<WebSessionContext>();
    let can_view_orders = has_permission(&session, "orders");
    let can_view_inventory = has_permission(&session, "wms");
    let total_on_hand = data
        .balances
        .iter()
        .map(|balance| balance.quantity.on_hand)
        .sum::<i64>();
    let total_reserved = data
        .balances
        .iter()
        .map(|balance| balance.quantity.reserved)
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
    let facility_count = data.access.facilities.len();
    let inventory_owner_count = data.access.inventory_owners.len();
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
            <RefreshButton section=Section::Overview/>
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
                            <Metric label="On hand in view" value=format_quantity(total_on_hand) tone="green"/>
                            <Metric label="Reserved in view" value=format_quantity(total_reserved) tone="amber"/>
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

        <section class="scope-band">
            <div>
                <span>"Facilities"</span>
                <strong>{facility_count}</strong>
            </div>
            <div>
                <span>"Clients"</span>
                <strong>{inventory_owner_count}</strong>
            </div>
            <p>"Counts reflect the exact access scope assigned to this operator profile."</p>
        </section>
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
fn RefreshButton(section: Section) -> impl IntoView {
    let state = expect_context::<RwSignal<WorkspaceState>>();
    let session = expect_context::<WebSessionContext>();
    let refresh = move |_| request_workspace(session.clone(), section, state);
    let pending = move || {
        matches!(
            state.get(),
            WorkspaceState::Loading | WorkspaceState::Refreshing(_)
        )
    };
    view! {
        <button
            class="button secondary-action"
            type="button"
            on:click=refresh
            disabled=pending
            aria-busy=move || pending().to_string()
        >
            <Icon icon=UiIcon::Refresh/>
            <span>{move || if pending() { "Refreshing" } else { "Refresh" }}</span>
        </button>
    }
}

#[component]
fn Orders(data: WorkspaceData, on_unauthorized: Callback<()>) -> impl IntoView {
    let initial_page = data.orders.unwrap_or_else(empty_order_page);
    view! {
        <OrdersWorkbench
            initial_page
            access=data.access
            locations=data.locations
            on_unauthorized
        />
    }
}

fn empty_order_page() -> OrderPage {
    OrderPage {
        page: wareboxes_core::dto::Paged::new(Vec::new(), 0, 100, 0),
        summaries: Vec::new(),
    }
}

#[component]
fn Loads(data: WorkspaceData, on_unauthorized: Callback<()>) -> impl IntoView {
    view! {
        <LoadsWorkbench
            initial_loads=data.loads
            access=data.access
            catalog_items=data.catalog_items
            locations=data.locations
            on_unauthorized
        />
    }
}

#[component]
fn Inventory(data: WorkspaceData) -> impl IntoView {
    let session_state = expect_context::<RwSignal<SessionState>>();
    let on_unauthorized = Callback::new(move |_| {
        session_state.set(SessionState::Anonymous(Some(
            "Your session ended. Sign in to continue.".to_owned(),
        )));
    });

    view! {
        <section class="viewport-page inventory-page">
            <section class="page-heading">
                <div>
                    <h1>"Inventory"</h1>
                    <p>"Current balances by facility, location, item, status, and client."</p>
                </div>
                <RefreshButton section=Section::Inventory/>
            </section>
            <InventoryWorkspace
                initial_balances=data.balances
                initial_cursor=data.balance_next_cursor
                on_unauthorized
            />
        </section>
    }
}

#[component]
fn InventoryHolds(
    data: WorkspaceData,
    can_inspect_receipts: bool,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    view! {
        <section class="viewport-page inventory-holds-page">
            <section class="page-heading">
                <div>
                    <h1>"Quantity holds"</h1>
                    <p>"Restrict and release specific quantities without changing inventory disposition."</p>
                </div>
                <RefreshButton section=Section::InventoryHolds/>
            </section>
            <QuantityHoldsWorkbench
                initial_balances=data.balances
                initial_balance_cursor=data.balance_next_cursor
                initial_holds=data.holds
                initial_hold_cursor=data.hold_next_cursor
                can_inspect_receipts
                on_unauthorized
            />
        </section>
    }
}

#[component]
fn InventoryDisposition(data: WorkspaceData, on_unauthorized: Callback<()>) -> impl IntoView {
    view! {
        <section class="viewport-page inventory-disposition-page">
            <section class="page-heading">
                <div>
                    <p class="eyebrow">"Inventory control"</p>
                    <h1>"Disposition"</h1>
                    <p>"Move uncommitted stock between available, hold, damaged, and quarantine status."</p>
                </div>
                <RefreshButton section=Section::InventoryDisposition/>
            </section>
            <InventoryDispositionWorkbench
                initial_balances=data.balances
                initial_cursor=data.balance_next_cursor
                on_unauthorized
            />
        </section>
    }
}

#[component]
fn InventoryIntegrity(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let session = expect_context::<WebSessionContext>();
    let can_manage_recalls = has_permission(&session, "wms_supervisor");
    view! {
        <section class="page-heading">
            <div>
                <p class="eyebrow">"Inventory control"</p>
                <h1>"Inventory trace, aging, and control"</h1>
                <p>"Trace stock age, reconcile projections, and direct scanner-confirmed facility moves."</p>
            </div>
        </section>
        <InventoryIntegrityWorkbench access on_unauthorized can_manage_recalls/>
    }
}

#[component]
fn Access(data: WorkspaceData) -> impl IntoView {
    let session = expect_context::<WebSessionContext>();
    let facility_scope = if session.active_tenant.site_scope.all_facilities {
        "All facilities"
    } else {
        "Assigned facilities"
    };
    let owner_scope = if session.active_tenant.owner_scope.all_inventory_owners {
        "All clients"
    } else {
        "Assigned clients"
    };

    view! {
        <section class="page-heading">
            <div>
                <p class="eyebrow">"Access context"</p>
                <h1>"Warehouse access"</h1>
                <p>"Facilities and clients available in the selected organization."</p>
            </div>
            <RefreshButton section=Section::Access/>
        </section>
        <section class="access-grid">
            <ScopeResourceList
                title="Facilities"
                scope_label=facility_scope
                resources=data.access.facilities
            />
            <ScopeResourceList
                title="Clients"
                scope_label=owner_scope
                resources=data.access.inventory_owners
            />
        </section>
    }
}

#[component]
fn ScopeResourceList(
    title: &'static str,
    scope_label: &'static str,
    resources: Vec<AccessScopeResource>,
) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let sort = RwSignal::new(SortSpec {
        key: ResourceSort::Name,
        direction: SortDirection::Ascending,
    });
    let count = resources.len();
    view! {
        <section class="data-section scope-resource-section">
            <div class="section-title scope-resource-title">
                <div>
                    <p class="eyebrow">{scope_label}</p>
                    <h2>{title} <span>{count}</span></h2>
                </div>
                <SearchField
                    label=format!("Filter {title}")
                    placeholder="Filter"
                    value=filter
                />
            </div>
            <div class="table-scroll">
                <table class="data-table scope-table">
                    <caption class="sr-only">{format!("{title} in the current access scope")}</caption>
                    <thead>
                        <tr>
                            <SortableHeader
                                label="Name"
                                active=move || sort.get().key == ResourceSort::Name
                                direction=move || sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    SortSpec::select(sort, ResourceSort::Name)
                                })
                            />
                            <SortableHeader
                                label="ID"
                                active=move || sort.get().key == ResourceSort::Id
                                direction=move || sort.get().direction
                                on_sort=Callback::new(move |_| {
                                    SortSpec::select(sort, ResourceSort::Id)
                                })
                                numeric=true
                            />
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let query = filter.get().trim().to_ascii_lowercase();
                            let mut matching_resources = resources
                                .iter()
                                .filter(|resource| {
                                    query.is_empty()
                                        || resource.name.to_ascii_lowercase().contains(&query)
                                        || resource.id.to_string().contains(&query)
                                })
                                .collect::<Vec<_>>();
                            let spec = sort.get();
                            matching_resources.sort_by(|left, right| {
                                let ordering = match spec.key {
                                    ResourceSort::Name => left
                                        .name
                                        .to_ascii_lowercase()
                                        .cmp(&right.name.to_ascii_lowercase()),
                                    ResourceSort::Id => left.id.cmp(&right.id),
                                }
                                .then_with(|| left.id.cmp(&right.id));
                                if spec.direction == SortDirection::Ascending {
                                    ordering
                                } else {
                                    ordering.reverse()
                                }
                            });
                            if matching_resources.is_empty() {
                                let message = if query.is_empty() {
                                    "No resources are assigned in this scope."
                                } else {
                                    "No matching resources."
                                };
                                view! {
                                    <tr>
                                        <td class="table-empty-row" colspan="2">
                                            {message}
                                        </td>
                                    </tr>
                                }
                                    .into_any()
                            } else {
                                matching_resources
                                    .into_iter()
                                    .map(|resource| {
                                        view! {
                                            <tr>
                                                <td><strong>{resource.name.clone()}</strong></td>
                                                <td class="numeric">{resource.id}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()
                                    .into_any()
                            }
                        }}
                    </tbody>
                </table>
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

#[cfg(not(target_arch = "wasm32"))]
fn initial_web_session() -> Option<WebSessionContext> {
    use_context::<InitialWebSession>().and_then(|initial| initial.0)
}

#[cfg(target_arch = "wasm32")]
fn initial_web_session() -> Option<WebSessionContext> {
    browser_bootstrap(SESSION_BOOTSTRAP_ID)
}

#[cfg(not(target_arch = "wasm32"))]
fn initial_web_workspace() -> Option<WorkspaceBootstrap> {
    use_context::<InitialWebWorkspace>().and_then(|initial| initial.0)
}

#[cfg(target_arch = "wasm32")]
fn initial_web_workspace() -> Option<WorkspaceBootstrap> {
    browser_bootstrap(WORKSPACE_BOOTSTRAP_ID)
}

#[cfg(target_arch = "wasm32")]
fn browser_bootstrap<T: serde::de::DeserializeOwned>(element_id: &str) -> Option<T> {
    let document = web_sys::window()?.document()?;
    let raw = document.get_element_by_id(element_id)?.text_content()?;
    serde_json::from_str::<Option<T>>(&raw).ok().flatten()
}

fn serialize_bootstrap<T: serde::Serialize>(value: &Option<T>) -> String {
    escape_bootstrap_json(serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned()))
}

fn escape_bootstrap_json(json: String) -> String {
    json.replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use leptos::prelude::*;
    use leptos_router::location::RequestUrl;

    use super::{
        escape_bootstrap_json, workspace_is_ready_for_auto_refresh, App, Section, WorkspaceData,
        WorkspaceState,
    };

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

    #[test]
    fn session_bootstrap_cannot_close_its_script_element() {
        let escaped = escape_bootstrap_json(r#""</script>&""#.to_owned());

        assert_eq!(escaped, r#""\u003c/script\u003e\u0026""#);
    }

    #[test]
    fn automatic_refresh_only_runs_for_ready_data_backed_workspaces() {
        Owner::new().with(|| {
            let state = RwSignal::new(WorkspaceState::Loading);
            assert!(!workspace_is_ready_for_auto_refresh(&state));
            state.set(WorkspaceState::Ready(WorkspaceData::default()));
            assert!(workspace_is_ready_for_auto_refresh(&state));
            state.set(WorkspaceState::Refreshing(WorkspaceData::default()));
            assert!(!workspace_is_ready_for_auto_refresh(&state));
        });

        assert!(Section::Overview.supports_workspace_refresh());
        assert!(!Section::InventoryHolds.supports_workspace_refresh());
        assert!(!Section::Orders.supports_workspace_refresh());
        assert!(!Section::Packing.supports_workspace_refresh());
        assert!(!Section::Shipping.supports_workspace_refresh());
        assert!(!Section::Loads.supports_workspace_refresh());
        assert!(!Section::Catalog.supports_workspace_refresh());
        assert!(
            !Section::Administration(crate::administration::AdministrationArea::Users)
                .supports_workspace_refresh()
        );
    }
}
