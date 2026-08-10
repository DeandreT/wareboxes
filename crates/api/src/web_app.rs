use axum::Router;
use leptos::prelude::{get_configuration, provide_context, LeptosOptions};
use leptos_axum::{generate_route_list, LeptosRoutes};
use wareboxes_core::dto::WebSessionContext;
use wareboxes_web_ops::app::{
    shell, App, InitialWebSession, InitialWebWorkspace, WorkspaceBootstrap,
    WorkspaceBootstrapContent, WorkspaceBootstrapData, WorkspaceBootstrapSection,
};

use crate::error::AppResult;
use crate::routes;
use crate::state::AppState;
use crate::{auth, repo};

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
    let render_state = state;
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
        .with_state(options);
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
        "/replenishment" | "/replenishment/" => Some(WorkspaceBootstrapSection::Replenishment),
        "/inventory" | "/inventory/" => Some(WorkspaceBootstrapSection::Inventory),
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
                    routes::v1::inventory_balances::page_for_access(state, access, None, 100, None)
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
                replenishment_policies: None,
                replenishment_queue: None,
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
        WorkspaceBootstrapSection::Inventory => {
            if !has_permission(session, "wms") {
                return Ok(WorkspaceBootstrapData::default());
            }
            let page =
                routes::v1::inventory_balances::page_for_access(state, access, None, 100, None)
                    .await?;
            Ok(WorkspaceBootstrapData {
                balances: page.items,
                balance_next_cursor: page.next_cursor,
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
            section_for_path("/replenishment"),
            Some(WorkspaceBootstrapSection::Replenishment)
        );
        assert_eq!(
            section_for_path("/inventory/holds"),
            None,
            "a route must not consume another workspace's bootstrap"
        );
    }
}
