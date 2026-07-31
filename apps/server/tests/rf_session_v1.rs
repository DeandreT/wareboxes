mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CreateRfSessionRequest, CreateRfSessionResponse, ErrorReason, ErrorResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn session_request(email: &str, password: &str) -> Request<Body> {
    let body = serde_json::to_vec(&CreateRfSessionRequest {
        email: email.into(),
        password: password.into(),
    })
    .unwrap();

    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/rf/sessions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn rf_session_returns_operator_default_tenant_and_restricted_scopes() {
    let fixture = Fixture::new().await;
    let email = "rf-session-operator@test.com";
    let user = auth::register_user(&fixture.db, email, "supersecret", None, None)
        .await
        .unwrap();
    let initial_access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let tenant_id = initial_access.tenant_id;
    let facility_id = fixture.facility(tenant_id, "RF Session Facility").await;
    let inventory_owner_id = fixture.inventory_owner(tenant_id, "RF Session Owner").await;
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: false,
            facility_ids: vec![facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![inventory_owner_id],
        },
    )
    .await
    .unwrap());

    let app = routes::app(AppState::new(fixture.db.clone()));
    let response = app
        .oneshot(session_request(email, "supersecret"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let session = response_json::<CreateRfSessionResponse>(response).await;
    assert!(!session.token.is_empty());
    assert_eq!(session.operator_id, user.id);
    assert_eq!(session.tenant.tenant_id, tenant_id.get());
    assert_eq!(session.tenant.name, initial_access.name);
    assert!(!session.tenant.site_scope.all_facilities);
    assert_eq!(session.tenant.site_scope.facility_ids, vec![facility_id]);
    assert!(!session.tenant.owner_scope.all_inventory_owners);
    assert_eq!(
        session.tenant.owner_scope.inventory_owner_ids,
        vec![inventory_owner_id]
    );

    let authenticated_access = auth::default_tenant_for_session(&fixture.db, &session.token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(authenticated_access.tenant_id, tenant_id);
    assert_eq!(authenticated_access.user_id.get(), user.id);
}

#[tokio::test]
async fn rf_session_rejects_invalid_credentials_with_v1_error() {
    let fixture = Fixture::new().await;
    let email = "rf-session-invalid@test.com";
    auth::register_user(&fixture.db, email, "supersecret", None, None)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db));

    for (attempted_email, attempted_password) in [
        (email, "incorrect-password"),
        ("unknown-rf-operator@test.com", "supersecret"),
    ] {
        let response = app
            .clone()
            .oneshot(session_request(attempted_email, attempted_password))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let error = response_json::<ErrorResponse>(response).await;
        assert_eq!(error.reason, ErrorReason::Unauthorized);
        assert_eq!(error.message, "unauthorized");
        assert!(!error.request_id.is_empty());
    }
}

#[tokio::test]
async fn rf_session_validates_credential_shape_before_authentication() {
    let fixture = Fixture::new().await;
    let app = routes::app(AppState::new(fixture.db));
    let attempts = [
        ("", "", "must be nonempty"),
        (
            " operator@test.com",
            "supersecret ",
            "must not have leading or trailing whitespace",
        ),
    ];

    for (email, password, expected_reason) in attempts {
        let response = app
            .clone()
            .oneshot(session_request(email, password))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let error = response_json::<ErrorResponse>(response).await;
        assert_eq!(error.reason, ErrorReason::ValidationFailed);
        assert_eq!(error.message, "validation failed");
        assert_eq!(error.violations.len(), 2);
        assert_eq!(error.violations[0].field, "email");
        assert_eq!(error.violations[0].reason, expected_reason);
        assert_eq!(error.violations[1].field, "password");
        assert_eq!(error.violations[1].reason, expected_reason);
    }

    let oversized_email = "e".repeat(255);
    let oversized_password = "p".repeat(1_025);
    let response = app
        .oneshot(session_request(&oversized_email, &oversized_password))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error = response_json::<ErrorResponse>(response).await;
    assert_eq!(error.reason, ErrorReason::ValidationFailed);
    assert_eq!(error.violations.len(), 2);
    assert_eq!(error.violations[0].field, "email");
    assert_eq!(error.violations[0].reason, "must not exceed 254 characters");
    assert_eq!(error.violations[1].field, "password");
    assert_eq!(
        error.violations[1].reason,
        "must not exceed 1024 characters"
    );
}
