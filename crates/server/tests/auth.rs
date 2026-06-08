mod common;

use common::*;

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
