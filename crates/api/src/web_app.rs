use axum::http::Request;
use axum::middleware;
use axum::Router;
use leptos::prelude::{get_configuration, provide_context, LeptosOptions};
use leptos_axum::{generate_route_list, LeptosRoutes};
use tower_http::trace::TraceLayer;
use wareboxes_core::dto::WebSessionContext;
use wareboxes_web_ops::app::{
    shell, App, InitialWebSession, InitialWebWorkspace, WorkspaceBootstrap,
    WorkspaceBootstrapContent, WorkspaceBootstrapData, WorkspaceBootstrapSection,
};

use crate::error::AppResult;
use crate::routes;
use crate::state::AppState;
use crate::{auth, observability, repo, request_context, traffic};

pub fn with_web_app(api: Router, state: AppState) -> anyhow::Result<Router> {
    let configuration = get_configuration(None)?;
    let mut options = configuration.leptos_options;
    if options.output_name.is_empty() {
        options.output_name = "wareboxes-web".into();
    }
    Ok(with_web_app_options(api, state, options))
}

pub fn with_web_app_options(api: Router, state: AppState, options: LeptosOptions) -> Router {
    let app_routes = generate_route_list(App);
    let render_options = options.clone();
    let render_state = state.clone();
    let handler = move |request: axum::extract::Request| {
        let options = render_options.clone();
        let state = render_state.clone();
        let requested_section = section_for_path(request.uri().path());
        let session_token = auth::web_session_token(request.headers(), &state.security);
        async move {
            let initial_session = restore_session(session_token, &state).await;
            let initial_workspace = match (initial_session.as_ref(), requested_section) {
                (Some(session), Some(section)) => {
                    Some(match workspace_bootstrap(&state, session, section).await {
                        Ok(data) => WorkspaceBootstrap {
                            section,
                            content: WorkspaceBootstrapContent::Ready(Box::new(data)),
                        },
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                path = %request.uri().path(),
                                "could not load initial web workspace for SSR"
                            );
                            WorkspaceBootstrap {
                                section,
                                content: WorkspaceBootstrapContent::Failed(
                                    "Operations data could not be loaded. Retry from this page."
                                        .to_owned(),
                                ),
                            }
                        }
                    })
                }
                _ => None,
            };
            let handler = leptos_axum::render_app_to_stream_with_context(
                move || {
                    provide_context(InitialWebSession(initial_session.clone()));
                    provide_context(InitialWebWorkspace(initial_workspace.clone()));
                },
                move || shell(options.clone()),
            );
            handler(request).await
        }
    };
    let web = Router::new()
        .leptos_routes_with_handler(app_routes, handler)
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(options)
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                let request_id = request
                    .headers()
                    .get(request_context::REQUEST_ID_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("unknown");
                tracing::info_span!(
                    "http_request",
                    method = %request.method(),
                    uri = %request.uri(),
                    %request_id
                )
            }),
        )
        .layer(middleware::from_fn_with_state(
            state.traffic.clone(),
            traffic::enforce,
        ))
        .layer(middleware::from_fn_with_state(
            state.metrics.clone(),
            observability::observe_request,
        ))
        .layer(middleware::from_fn(request_context::assign_request_id));
    api.merge(web)
}

async fn restore_session(token: Option<String>, state: &AppState) -> Option<WebSessionContext> {
    let token = token?;
    match auth::web_session_context_for_token(&state.db, &state.security, &token).await {
        Ok(session) => session,
        Err(error) => {
            tracing::warn!(%error, "could not restore web session for SSR");
            None
        }
    }
}

fn section_for_path(path: &str) -> Option<WorkspaceBootstrapSection> {
    match path {
        "/" => Some(WorkspaceBootstrapSection::Overview),
        "/orders" | "/orders/" => Some(WorkspaceBootstrapSection::Orders),
        "/pick-waves" | "/pick-waves/" => Some(WorkspaceBootstrapSection::PickWaves),
        "/packing" | "/packing/" => Some(WorkspaceBootstrapSection::Packing),
        "/shipping" | "/shipping/" => Some(WorkspaceBootstrapSection::Shipping),
        "/outbound-loads" | "/outbound-loads/" => Some(WorkspaceBootstrapSection::OutboundLoads),
        "/putaway" | "/putaway/" => Some(WorkspaceBootstrapSection::Putaway),
        "/cycle-counts" | "/cycle-counts/" => Some(WorkspaceBootstrapSection::CycleCounts),
        "/cross-dock" | "/cross-dock/" => Some(WorkspaceBootstrapSection::CrossDock),
        "/replenishment" | "/replenishment/" => Some(WorkspaceBootstrapSection::Replenishment),
        "/slotting" | "/slotting/" => Some(WorkspaceBootstrapSection::Slotting),
        "/work-orchestration" | "/work-orchestration/" => {
            Some(WorkspaceBootstrapSection::WorkOrchestration)
        }
        "/administration/service-accounts" | "/administration/service-accounts/" => {
            Some(WorkspaceBootstrapSection::ServiceAccounts)
        }
        "/platform/tenants" | "/platform/tenants/" => {
            Some(WorkspaceBootstrapSection::TenantLifecycle)
        }
        "/platform/support-access" | "/platform/support-access/" => {
            Some(WorkspaceBootstrapSection::SupportAccess)
        }
        "/inventory" | "/inventory/" => Some(WorkspaceBootstrapSection::Inventory),
        "/inventory/control" | "/inventory/control/" => {
            Some(WorkspaceBootstrapSection::InventoryIntegrity)
        }
        "/access" | "/access/" => Some(WorkspaceBootstrapSection::Access),
        _ => None,
    }
}

async fn workspace_bootstrap(
    state: &AppState,
    session: &WebSessionContext,
    section: WorkspaceBootstrapSection,
) -> AppResult<WorkspaceBootstrapData> {
    let access = &session.active_tenant;
    match section {
        WorkspaceBootstrapSection::Overview => {
            let load_access = routes::access::workspace_for_access(state, access);
            let load_orders = async {
                if has_permission(session, "orders") {
                    repo::orders::get_orders_page_in_scope(&state.db, access, 50, 0, None, None)
                        .await
                        .map(Some)
                } else {
                    Ok(None)
                }
            };
            let load_balances = async {
                if has_permission(session, "wms") {
                    routes::v1::inventory_balances::page_for_access(
                        state,
                        access,
                        &routes::v1::inventory_balances::BalancePageOptions {
                            offset: 0,
                            limit: 100,
                            query: None,
                            sort: wareboxes_api_contract::v1::InventoryBalanceSort::Position,
                            direction:
                                wareboxes_api_contract::v1::InventorySortDirection::Ascending,
                            movable_only: false,
                        },
                    )
                    .await
                    .map(Some)
                } else {
                    Ok(None)
                }
            };
            let (access_workspace, orders, balances) =
                tokio::try_join!(load_access, load_orders, load_balances)?;
            let (balances, balance_next_cursor) =
                balances.map_or_else(|| (Vec::new(), None), |page| (page.items, page.next_cursor));
            Ok(WorkspaceBootstrapData {
                orders,
                pick_waves: None,
                packing_queue: None,
                shipping_queue: None,
                outbound_load_queue: None,
                putaway_candidates: None,
                putaway_work: None,
                cycle_count_candidates: None,
                cycle_count_work: None,
                cycle_count_policies: None,
                cycle_count_variances: None,
                cross_dock_planning_options: None,
                cross_dock_work: None,
                replenishment_policies: None,
                replenishment_queue: None,
                tenant_lifecycle_page: None,
                support_access_page: None,
                balances,
                balance_next_cursor,
                access: access_workspace,
                locations: Vec::new(),
            })
        }
        WorkspaceBootstrapSection::Orders => {
            if !has_permission(session, "orders") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let load_locations = routes::locations::list_for_access(state, access, false);
            let (orders, access_workspace, locations) = tokio::try_join!(
                repo::orders::get_orders_page_in_scope_sorted(
                    &state.db,
                    access,
                    100,
                    0,
                    None,
                    None,
                    repo::orders::OrderPageSort::Order,
                    repo::orders::OrderPageSortDirection::Descending,
                ),
                routes::access::workspace_for_access(state, access),
                load_locations,
            )?;
            Ok(WorkspaceBootstrapData {
                orders: Some(orders),
                access: access_workspace,
                locations,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::PickWaves => {
            if !has_permission(session, "wms_supervisor") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let load_locations = routes::locations::list_for_access(state, access, false);
            let (pick_waves, orders, access_workspace, locations) = tokio::try_join!(
                routes::v1::pick_waves::page_for_access(state, access, None, None, 100),
                repo::orders::get_orders_page_in_scope_sorted(
                    &state.db,
                    access,
                    100,
                    0,
                    Some(wareboxes_core::models::OrderStatus::Open),
                    None,
                    repo::orders::OrderPageSort::Order,
                    repo::orders::OrderPageSortDirection::Ascending,
                ),
                routes::access::workspace_for_access(state, access),
                load_locations,
            )?;
            Ok(WorkspaceBootstrapData {
                pick_waves: Some(pick_waves),
                orders: Some(orders),
                access: access_workspace,
                locations,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::Packing => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let (packing_queue, access_workspace, locations) = tokio::try_join!(
                routes::v1::packing::page_for_access(state, access, None, None, 100),
                routes::access::workspace_for_access(state, access),
                routes::locations::list_for_access(state, access, false),
            )?;
            Ok(WorkspaceBootstrapData {
                packing_queue: Some(packing_queue),
                access: access_workspace,
                locations,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::Shipping => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let (shipping_queue, access_workspace) = tokio::try_join!(
                routes::v1::shipping_queue::page_for_access(state, access, None, None, 100),
                routes::access::workspace_for_access(state, access),
            )?;
            Ok(WorkspaceBootstrapData {
                shipping_queue: Some(shipping_queue),
                access: access_workspace,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::OutboundLoads => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let (outbound_load_queue, shipping_queue, access_workspace, locations) = tokio::try_join!(
                routes::v1::outbound_loads::page_for_access(state, access, None, None, 100,),
                routes::v1::shipping_queue::page_for_access(state, access, None, None, 100,),
                routes::access::workspace_for_access(state, access),
                routes::locations::list_for_access(state, access, false),
            )?;
            Ok(WorkspaceBootstrapData {
                outbound_load_queue: Some(outbound_load_queue),
                shipping_queue: Some(shipping_queue),
                access: access_workspace,
                locations,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::Putaway => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let ((putaway_candidates, putaway_work), access_workspace, locations) = tokio::try_join!(
                routes::v1::putaway::pages_for_access(state, access, 100),
                routes::access::workspace_for_access(state, access),
                routes::locations::list_for_access(state, access, false),
            )?;
            Ok(WorkspaceBootstrapData {
                putaway_candidates: Some(putaway_candidates),
                putaway_work: Some(putaway_work),
                access: access_workspace,
                locations,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::CycleCounts => {
            if !has_permission(session, "wms_supervisor") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let (
                (cycle_count_candidates, cycle_count_work),
                (cycle_count_policies, cycle_count_variances),
                access_workspace,
            ) = tokio::try_join!(
                routes::v1::cycle_count::pages_for_access(state, access, 100),
                routes::v1::cycle_count::control_pages_for_access(state, access, 100),
                routes::access::workspace_for_access(state, access),
            )?;
            Ok(WorkspaceBootstrapData {
                cycle_count_candidates: Some(cycle_count_candidates),
                cycle_count_work: Some(cycle_count_work),
                cycle_count_policies: Some(cycle_count_policies),
                cycle_count_variances: Some(cycle_count_variances),
                access: access_workspace,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::CrossDock => {
            if !has_permission(session, "wms_supervisor") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let ((planning_options, work), access_workspace) = tokio::try_join!(
                routes::v1::cross_dock::pages_for_access(state, access, 100),
                routes::access::workspace_for_access(state, access),
            )?;
            Ok(WorkspaceBootstrapData {
                cross_dock_planning_options: Some(planning_options),
                cross_dock_work: Some(work),
                access: access_workspace,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::Inventory => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let page = routes::v1::inventory_balances::page_for_access(
                state,
                access,
                &routes::v1::inventory_balances::BalancePageOptions {
                    offset: 0,
                    limit: 100,
                    query: None,
                    sort: wareboxes_api_contract::v1::InventoryBalanceSort::Position,
                    direction: wareboxes_api_contract::v1::InventorySortDirection::Ascending,
                    movable_only: false,
                },
            )
            .await?;
            Ok(WorkspaceBootstrapData {
                balances: page.items,
                balance_next_cursor: page.next_cursor,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::InventoryIntegrity => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            Ok(WorkspaceBootstrapData {
                access: routes::access::workspace_for_access(state, access).await?,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::Replenishment => {
            if !has_permission(session, "wms_supervisor") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let ((replenishment_policies, replenishment_queue), access_workspace) = tokio::try_join!(
                routes::v1::replenishment::pages_for_access(state, access, 100),
                routes::access::workspace_for_access(state, access),
            )?;
            Ok(WorkspaceBootstrapData {
                replenishment_policies: Some(replenishment_policies),
                replenishment_queue: Some(replenishment_queue),
                access: access_workspace,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::Slotting => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            Ok(WorkspaceBootstrapData {
                access: routes::access::workspace_for_access(state, access).await?,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::WorkOrchestration => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let (access_workspace, locations) = tokio::try_join!(
                routes::access::workspace_for_access(state, access),
                routes::locations::list_for_access(state, access, false),
            )?;
            Ok(WorkspaceBootstrapData {
                access: access_workspace,
                locations,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::ServiceAccounts => {
            if !has_permission(session, "admin") {
                return Ok(WorkspaceBootstrapData::default());
            }
            Ok(WorkspaceBootstrapData {
                access: routes::access::workspace_for_access(state, access).await?,
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::TenantLifecycle => {
            if !session.is_platform_administrator {
                return Ok(WorkspaceBootstrapData::default());
            }
            let page = repo::tenant_lifecycle::page(
                &state.db,
                access,
                &wareboxes_application::tenant_lifecycle::TenantLifecyclePageQuery {
                    status: None,
                    search: None,
                    cursor: None,
                    limit: wareboxes_api_contract::v1::PageLimit::default().get(),
                },
            )
            .await?;
            let items = page
                .items
                .into_iter()
                .map(routes::v1::tenant_lifecycle::map_response_for_web)
                .collect::<AppResult<Vec<_>>>()?;
            Ok(WorkspaceBootstrapData {
                tenant_lifecycle_page: Some(wareboxes_api_contract::v1::TenantLifecyclePage::new(
                    items,
                    page.next_cursor
                        .map(routes::v1::tenant_lifecycle::encode_cursor_for_web)
                        .transpose()?,
                )),
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::SupportAccess => {
            if !session.is_platform_administrator {
                return Ok(WorkspaceBootstrapData::default());
            }
            let support_query = wareboxes_application::support_access::SupportAccessPageQuery {
                tenant_id: None,
                status: None,
                cursor: None,
                limit: wareboxes_api_contract::v1::PageLimit::default().get(),
            };
            let tenant_query = wareboxes_application::tenant_lifecycle::TenantLifecyclePageQuery {
                status: Some(wareboxes_domain::TenantStatus::Active),
                search: None,
                cursor: None,
                limit: wareboxes_api_contract::v1::PageLimit::default().get(),
            };
            let support_page = repo::support_access::page(&state.db, access, &support_query);
            let tenant_page = repo::tenant_lifecycle::page(&state.db, access, &tenant_query);
            let (support_page, tenant_page) = tokio::try_join!(support_page, tenant_page)?;
            let support_items = support_page
                .items
                .into_iter()
                .map(routes::v1::support_access::map_response_for_web)
                .collect::<AppResult<Vec<_>>>()?;
            let tenant_items = tenant_page
                .items
                .into_iter()
                .map(routes::v1::tenant_lifecycle::map_response_for_web)
                .collect::<AppResult<Vec<_>>>()?;
            Ok(WorkspaceBootstrapData {
                support_access_page: Some(wareboxes_api_contract::v1::SupportAccessPage::new(
                    support_items,
                    support_page
                        .next_cursor
                        .map(routes::v1::support_access::encode_cursor_for_web)
                        .transpose()?,
                )),
                tenant_lifecycle_page: Some(wareboxes_api_contract::v1::TenantLifecyclePage::new(
                    tenant_items,
                    tenant_page
                        .next_cursor
                        .map(routes::v1::tenant_lifecycle::encode_active_cursor_for_web)
                        .transpose()?,
                )),
                ..WorkspaceBootstrapData::default()
            })
        }
        WorkspaceBootstrapSection::Access => Ok(WorkspaceBootstrapData {
            access: routes::access::workspace_for_access(state, access).await?,
            ..WorkspaceBootstrapData::default()
        }),
    }
}

fn has_permission(session: &WebSessionContext, permission: &str) -> bool {
    session.user.user_permissions.iter().any(|candidate| {
        candidate.name.eq_ignore_ascii_case("admin")
            || candidate.name.eq_ignore_ascii_case(permission)
    })
}

#[cfg(test)]
mod tests {
    use super::{section_for_path, WorkspaceBootstrapSection};

    #[test]
    fn only_data_backed_routes_receive_a_workspace_bootstrap() {
        assert_eq!(
            section_for_path("/"),
            Some(WorkspaceBootstrapSection::Overview)
        );
        assert_eq!(
            section_for_path("/orders"),
            Some(WorkspaceBootstrapSection::Orders)
        );
        assert_eq!(
            section_for_path("/pick-waves"),
            Some(WorkspaceBootstrapSection::PickWaves)
        );
        assert_eq!(
            section_for_path("/packing"),
            Some(WorkspaceBootstrapSection::Packing)
        );
        assert_eq!(
            section_for_path("/shipping"),
            Some(WorkspaceBootstrapSection::Shipping)
        );
        assert_eq!(
            section_for_path("/outbound-loads"),
            Some(WorkspaceBootstrapSection::OutboundLoads)
        );
        assert_eq!(
            section_for_path("/putaway"),
            Some(WorkspaceBootstrapSection::Putaway)
        );
        assert_eq!(
            section_for_path("/cycle-counts"),
            Some(WorkspaceBootstrapSection::CycleCounts)
        );
        assert_eq!(
            section_for_path("/replenishment"),
            Some(WorkspaceBootstrapSection::Replenishment)
        );
        assert_eq!(
            section_for_path("/slotting"),
            Some(WorkspaceBootstrapSection::Slotting)
        );
        assert_eq!(
            section_for_path("/work-orchestration"),
            Some(WorkspaceBootstrapSection::WorkOrchestration)
        );
        assert_eq!(
            section_for_path("/administration/service-accounts"),
            Some(WorkspaceBootstrapSection::ServiceAccounts)
        );
        assert_eq!(
            section_for_path("/platform/tenants"),
            Some(WorkspaceBootstrapSection::TenantLifecycle)
        );
        assert_eq!(
            section_for_path("/platform/support-access"),
            Some(WorkspaceBootstrapSection::SupportAccess)
        );
        assert_eq!(
            section_for_path("/cross-dock"),
            Some(WorkspaceBootstrapSection::CrossDock)
        );
        assert_eq!(
            section_for_path("/inventory/control"),
            Some(WorkspaceBootstrapSection::InventoryIntegrity)
        );
        assert_eq!(
            section_for_path("/inventory/holds"),
            None,
            "a route must not consume another workspace's bootstrap"
        );
    }
}
