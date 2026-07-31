#![cfg(feature = "ssr")]

mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use leptos::prelude::LeptosOptions;
use tower::ServiceExt;
use wareboxes_api::{repo, routes, state::AppState, web_app};
use wareboxes_core::dto::LoginRequest;

const HOST: &str = "wareboxes.test";
const ORIGIN: &str = "http://wareboxes.test";

fn login_request(email: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/web/auth/login")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&LoginRequest {
                email,
                password: "supersecret".to_owned(),
            })
            .unwrap(),
        ))
        .unwrap()
}

fn session_cookie(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn response_html(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn authenticated_overview_and_orders_are_rendered_with_scoped_data() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("ssr-operator@test.com").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let orders_permission =
        repo::permissions::add_permission(&fixture.db, access.tenant_id, "orders", Some("Orders"))
            .await
            .unwrap();
    let orders_role = repo::roles::add_role(
        &fixture.db,
        access.tenant_id,
        "SSR order operator",
        Some("SSR test role"),
    )
    .await
    .unwrap();
    repo::roles::add_role_permission(
        &fixture.db,
        access.tenant_id,
        orders_role,
        orders_permission,
    )
    .await
    .unwrap();
    repo::roles::add_role_to_user(&fixture.db, access.tenant_id, user.id, orders_role)
        .await
        .unwrap();

    let facility_id = fixture
        .facility(access.tenant_id, "SSR West Facility")
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(access.tenant_id, "SSR Retail Client")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, inventory_owner_id, facility_id)
        .await;
    let item_id = fixture.item(access.tenant_id, "SSR Widget", "each").await;
    fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id,
                item_id,
                qty: 37,
                key: "SSR-BIN-01",
            },
        )
        .await;
    fixture
        .order(access.tenant_id, "SSR-ORDER-1001", inventory_owner_id)
        .await;

    let state = AppState::new(fixture.db.clone());
    let api = routes::app(state.clone());
    let options = LeptosOptions::builder()
        .output_name("wareboxes-web")
        .build();
    let app = web_app::with_web_app_options(api, state, options);
    let anonymous = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::OK);
    let anonymous = response_html(anonymous).await;
    assert!(anonymous.contains("Sign in"));
    assert!(!anonymous.contains("SSR-ORDER-1001"));

    let login = app
        .clone()
        .oneshot(login_request(user.email.clone()))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = session_cookie(&login);

    let overview = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overview.status(), StatusCode::OK);
    let overview = response_html(overview).await;
    assert!(overview.contains("Warehouse control"));
    assert!(overview.contains("SSR-ORDER-1001"));
    assert!(overview.contains("SSR West Facility"));
    assert!(overview.contains(">37<"));
    assert!(!overview.contains("Loading operations"));

    let orders = app
        .oneshot(
            Request::builder()
                .uri("/orders")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(orders.status(), StatusCode::OK);
    let orders = response_html(orders).await;
    assert!(orders.contains("Order, client, destination"));
    assert!(orders.contains("SSR-ORDER-1001"));
    assert!(!orders.contains("Loading operations"));
}
