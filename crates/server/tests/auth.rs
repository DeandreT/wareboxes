mod common;

use axum::extract::State;
use axum::Json;
use common::*;
use wareboxes_core::dto::{LoginRequest, RegisterRequest};
use wareboxes_server::state::AppState;

#[tokio::test]
async fn auth_and_hierarchical_rbac() {
    let db = setup().await;

    let user = auth::register_user(&db, "admin@test.com", "supersecret", None, None)
        .await
        .unwrap();
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

    // Role hierarchy: `child.parent_id = parent`. The ported recursive CTE
    // resolves a user's roles plus their descendants, so a holder of `parent`
    // inherits permissions assigned to `child`.
    let perm = repo::permissions::add_permission(&db, "orders", Some("Orders"))
        .await
        .unwrap();
    let parent = repo::roles::add_role(&db, "parent", Some("p"))
        .await
        .unwrap();
    let child = repo::roles::add_role(&db, "child", Some("c"))
        .await
        .unwrap();
    repo::roles::add_role_permission(&db, child, perm)
        .await
        .unwrap();
    repo::roles::add_role_relationship(&db, parent, child)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, user.id, parent)
        .await
        .unwrap();

    // Inherited through the role hierarchy (parent -> child's permission).
    assert!(permissions::user_has_permission(&db, user.id, "orders")
        .await
        .unwrap());
    assert!(
        permissions::user_has_any_permission(&db, user.id, &["nope", "orders"])
            .await
            .unwrap()
    );
    assert!(!permissions::user_has_permission(&db, user.id, "missing")
        .await
        .unwrap());

    // Closures resolved by get_role.
    let child_role = repo::roles::get_role(&db, child).await.unwrap().unwrap();
    assert!(child_role.parent_roles.iter().any(|r| r.id == parent));
    let parent_role = repo::roles::get_role(&db, parent).await.unwrap().unwrap();
    assert!(parent_role.child_roles.iter().any(|r| r.id == child));

    // The per-user self role must be undeletable.
    let self_role = repo::roles::get_roles(&db, true, true)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == "admin@test.com")
        .unwrap();
    let err = repo::roles::delete_user_role(&db, user.id, self_role.id)
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
