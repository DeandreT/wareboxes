mod common;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::Json;
use common::*;
use tower::ServiceExt;
use wareboxes_core::dto::{LoginRequest, RegisterRequest};
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

#[tokio::test]
async fn auth_and_hierarchical_rbac() {
    let db = setup().await;

    let user = auth::register_user(&db, "admin@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    assert!(
        auth::verify_credentials(&db, "admin@test.com", "supersecret")
            .await
            .unwrap()
            .is_some()
    );
    assert!(auth::verify_credentials(&db, "admin@test.com", "wrong")
        .await
        .unwrap()
        .is_none());

    let token = auth::create_session(&db, user.id).await.unwrap();
    let stored_token: String = sqlx::query_scalar("SELECT token FROM sessions WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_ne!(stored_token, token);
    assert_eq!(stored_token.len(), 64);
    auth::destroy_session(&db, &token).await.unwrap();
    let remaining_sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = $1")
            .bind(user.id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(remaining_sessions, 0);

    // A child role inherits permissions assigned to its parent.
    let perm = repo::permissions::add_permission(&db, tenant_id, "orders", Some("Orders"))
        .await
        .unwrap();
    let parent = repo::roles::add_role(&db, tenant_id, "parent", Some("p"))
        .await
        .unwrap();
    let child = repo::roles::add_role(&db, tenant_id, "child", Some("c"))
        .await
        .unwrap();
    repo::roles::add_role_permission(&db, tenant_id, parent, perm)
        .await
        .unwrap();
    repo::roles::add_role_relationship(&db, tenant_id, parent, child)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, tenant_id, user.id, child)
        .await
        .unwrap();

    // Inherited through the role hierarchy (child -> parent's permission).
    assert!(
        permissions::user_has_permission(&db, tenant_id, user.id, "orders")
            .await
            .unwrap()
    );
    assert!(
        permissions::user_has_any_permission(&db, tenant_id, user.id, &["nope", "orders"])
            .await
            .unwrap()
    );
    assert!(
        !permissions::user_has_permission(&db, tenant_id, user.id, "missing")
            .await
            .unwrap()
    );

    // Closures resolved by get_role.
    let child_role = repo::roles::get_role(&db, tenant_id, child)
        .await
        .unwrap()
        .unwrap();
    assert!(child_role.parent_roles.iter().any(|r| r.id == parent));
    let parent_role = repo::roles::get_role(&db, tenant_id, parent)
        .await
        .unwrap()
        .unwrap();
    assert!(parent_role.child_roles.iter().any(|r| r.id == child));

    let other_user = auth::register_user(&db, "other-tenant@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let other_tenant = tenant_for_user(&db, other_user.id).await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(other_tenant.get())
    .bind(user.id)
    .execute(&db)
    .await
    .unwrap();
    assert!(
        !permissions::user_has_permission(&db, other_tenant, user.id, "orders")
            .await
            .unwrap()
    );
    assert!(
        !repo::roles::add_role_to_user(&db, other_tenant, user.id, parent)
            .await
            .unwrap()
    );
    assert!(
        !repo::roles::add_role_permission(&db, other_tenant, parent, perm)
            .await
            .unwrap()
    );
    let token = auth::create_session(&db, user.id).await.unwrap();
    let app = routes::app(AppState::new(db.clone()));
    let authorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/orders")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);
    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/orders")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, other_tenant.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::FORBIDDEN);

    let other_perm = repo::permissions::add_permission(&db, other_tenant, "orders", Some("Orders"))
        .await
        .unwrap();
    let other_role = repo::roles::add_role(&db, other_tenant, "parent", Some("p"))
        .await
        .unwrap();
    assert!(
        repo::roles::add_role_permission(&db, other_tenant, other_role, other_perm)
            .await
            .unwrap()
    );
    assert!(
        repo::roles::add_role_to_user(&db, other_tenant, user.id, other_role)
            .await
            .unwrap()
    );
    assert!(
        permissions::user_has_permission(&db, other_tenant, user.id, "orders")
            .await
            .unwrap()
    );
    let newly_authorized = app
        .oneshot(
            Request::builder()
                .uri("/api/orders")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, other_tenant.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(newly_authorized.status(), StatusCode::OK);

    // The per-user self role must be undeletable.
    let self_role = repo::roles::get_roles(&db, tenant_id, true, true)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == "admin@test.com")
        .unwrap();
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_id.get())
    .bind(other_user.id)
    .execute(&db)
    .await
    .unwrap();
    assert!(
        !repo::roles::add_role_to_user(&db, tenant_id, other_user.id, self_role.id)
            .await
            .unwrap()
    );
    assert!(
        !repo::roles::update_role(&db, tenant_id, self_role.id, Some("renamed"), None)
            .await
            .unwrap()
    );
    assert!(
        !repo::roles::set_role_deleted(&db, tenant_id, self_role.id, true)
            .await
            .unwrap()
    );
    assert!(
        !repo::roles::add_role_relationship(&db, tenant_id, parent, self_role.id)
            .await
            .unwrap()
    );
    let err = repo::roles::delete_user_role(&db, tenant_id, user.id, self_role.id)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::BadRequest(_))));
}

#[tokio::test]
async fn public_registration_is_disabled_by_default() {
    let db = setup().await;
    let state = AppState::new(db.clone());
    let request = RegisterRequest {
        email: "public@test.com".to_string(),
        password: "supersecret".to_string(),
        first_name: None,
        last_name: None,
    };

    let result = wareboxes_server::routes::auth::register(State(state), Json(request)).await;
    assert!(matches!(result, Err(AppError::Core(CoreError::Forbidden))));

    auth::register_user(&db, "login@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let login = LoginRequest {
        email: "login@test.com".to_string(),
        password: "supersecret".to_string(),
    };
    let result = wareboxes_server::routes::auth::login(State(AppState::new(db)), Json(login)).await;
    let session = result.unwrap().0;
    assert_eq!(session.user.email, "login@test.com");
    assert_eq!(session.active_tenant.user_id.get(), session.user.id);
    assert!(session.active_tenant.is_default);
}
