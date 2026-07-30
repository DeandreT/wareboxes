mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, Response, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_core::dto::{
    AccessScopeWorkspace, LoginRequest, SelectTenantRequest, WebSessionContext,
};
use wareboxes_server::config::SecurityConfig;
use wareboxes_server::{routes, state::AppState};

const HOST: &str = "wareboxes.test";
const ORIGIN: &str = "http://wareboxes.test";

fn json_request<T: serde::Serialize>(uri: &str, value: &T) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(value).unwrap()))
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: Response<Body>) -> T {
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn session_cookie(response: &Response<Body>) -> String {
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

async fn add_tenant_membership(db: &db::Db, user_id: i64, slug: &str, name: &str) -> TenantId {
    let admin_db = admin_db_for(db).await;
    let tenant_id: i64 =
        sqlx::query_scalar("INSERT INTO tenants (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(slug)
            .bind(name)
            .fetch_one(&admin_db)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_id)
    .bind(user_id)
    .execute(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;
    TenantId::new(tenant_id).unwrap()
}

#[tokio::test]
async fn web_session_is_cookie_bound_and_tenant_switching_is_membership_scoped() {
    let fixture = Fixture::new().await;
    let user = fixture.user("web-context@test.com").await;
    let default_tenant = tenant_for_user(&fixture.db, user.id).await;
    let other_tenant =
        add_tenant_membership(&fixture.db, user.id, "web-context-b", "Web Context B").await;

    fixture
        .facility(default_tenant, "Default tenant facility")
        .await;
    fixture
        .inventory_owner(default_tenant, "Default tenant owner")
        .await;
    fixture
        .facility(other_tenant, "Selected tenant facility")
        .await;
    fixture
        .inventory_owner(other_tenant, "Selected tenant owner")
        .await;

    let app = routes::app(AppState::new(fixture.db.clone()));
    let login = app
        .clone()
        .oneshot(json_request(
            "/api/web/auth/login",
            &LoginRequest {
                email: user.email.clone(),
                password: "supersecret".to_owned(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let set_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));
    let cookie = session_cookie(&login);
    let login_body: serde_json::Value = response_json(login).await;
    assert!(login_body.get("token").is_none());
    let context: WebSessionContext = serde_json::from_value(login_body).unwrap();
    assert_eq!(context.active_tenant.tenant_id, default_tenant);
    assert_eq!(context.available_tenants.len(), 2);

    let access = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/web/access")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(access.status(), StatusCode::OK);
    let access: AccessScopeWorkspace = response_json(access).await;
    assert_eq!(access.facilities[0].name, "Default tenant facility");
    assert_eq!(access.inventory_owners[0].name, "Default tenant owner");

    let cross_origin = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/web/auth/tenant")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, "https://attacker.test")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&SelectTenantRequest {
                        tenant_id: other_tenant.get(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_origin.status(), StatusCode::FORBIDDEN);

    let guessed_tenant = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/web/auth/tenant")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&SelectTenantRequest { tenant_id: 999_999 }).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guessed_tenant.status(), StatusCode::FORBIDDEN);

    let switched = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/web/auth/tenant")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&SelectTenantRequest {
                        tenant_id: other_tenant.get(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(switched.status(), StatusCode::OK);
    let context: WebSessionContext = response_json(switched).await;
    assert_eq!(context.active_tenant.tenant_id, other_tenant);

    let selected_access = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/web/access")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let selected_access: AccessScopeWorkspace = response_json(selected_access).await;
    assert_eq!(
        selected_access.facilities[0].name,
        "Selected tenant facility"
    );
    assert_eq!(
        selected_access.inventory_owners[0].name,
        "Selected tenant owner"
    );

    let web_token = cookie.split_once('=').unwrap().1;
    let bearer_reuse = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/web/access")
                .header(header::AUTHORIZATION, format!("Bearer {web_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bearer_reuse.status(), StatusCode::UNAUTHORIZED);

    let original_cookie = app
        .oneshot(
            Request::builder()
                .uri("/api/web/auth/session")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(original_cookie.status(), StatusCode::OK);
    let context: WebSessionContext = response_json(original_cookie).await;
    assert_eq!(context.active_tenant.tenant_id, other_tenant);
}

#[tokio::test]
async fn web_sessions_enforce_idle_and_absolute_expiration() {
    let fixture = Fixture::new().await;
    let user = fixture.user("web-expiry@test.com").await;
    let security = SecurityConfig::default();
    let admin_db = admin_db_for(&fixture.db).await;

    let idle_token = auth::create_web_session(
        &fixture.db,
        user.id,
        security.web_session_absolute_ttl_seconds,
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE sessions SET last_seen_at = CURRENT_TIMESTAMP - INTERVAL '2 hours' WHERE user_id = $1",
    )
    .bind(user.id)
    .execute(&admin_db)
    .await
    .unwrap();
    assert!(
        auth::web_session_context_for_token(&fixture.db, &security, &idle_token)
            .await
            .unwrap()
            .is_none()
    );

    let expired_token = auth::create_web_session(
        &fixture.db,
        user.id,
        security.web_session_absolute_ttl_seconds,
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE sessions SET expires = CURRENT_TIMESTAMP - INTERVAL '1 second' WHERE user_id = $1",
    )
    .bind(user.id)
    .execute(&admin_db)
    .await
    .unwrap();
    assert!(
        auth::web_session_context_for_token(&fixture.db, &security, &expired_token)
            .await
            .unwrap()
            .is_none()
    );
}
