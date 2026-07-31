mod common;

use axum::extract::State;
use axum::Json;
use common::*;
use wareboxes_api::{routes, state::AppState};
use wareboxes_core::dto::LoginRequest;
use wareboxes_domain::UserId;

fn typed_user_id(value: i64) -> UserId {
    UserId::new(value).unwrap()
}

async fn add_membership(db: &db::Db, tenant_id: TenantId, user_id: UserId) {
    let mut tx = tenant_tx(db, tenant_id).await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_id.get())
    .bind(user_id.get())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn identity_lookup_and_tenant_listing_keep_deletion_scopes_distinct() {
    let fixture = Fixture::new().await;
    let tenant_admin = fixture.user("identity-admin@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, tenant_admin.id).await;
    let member = fixture.user("identity-member@test.com").await;
    let member_id = typed_user_id(member.id);
    add_membership(&fixture.db, tenant_id, member_id).await;

    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "inventory.read",
        Some("Read inventory"),
    )
    .await
    .unwrap();
    let parent = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "inventory-reader",
        None,
    )
    .await
    .unwrap();
    let child = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "cycle-counter",
        None,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        parent,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_relationship(
        &fixture.db,
        tenant_id,
        parent,
        child,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        member.id,
        child,
    )
    .await
    .unwrap();

    let users =
        wareboxes_persistence_postgres::users::get_tenant_users(&fixture.db, tenant_id, false)
            .await
            .unwrap();
    let tenant_member = users
        .iter()
        .find(|user| user.identity.id == member_id)
        .unwrap();
    assert!(tenant_member.membership_deleted.is_none());
    assert!(tenant_member.identity.deleted.is_none());
    assert!(tenant_member
        .direct_roles
        .iter()
        .any(|role| role.id == child));
    assert!(tenant_member
        .permissions
        .iter()
        .any(|permission| permission.name == "INVENTORY.READ"));

    assert!(
        wareboxes_persistence_postgres::users::set_user_membership_deleted(
            &fixture.db,
            tenant_id,
            member_id,
            true,
        )
        .await
        .unwrap()
    );
    assert!(
        wareboxes_persistence_postgres::users::get_tenant_users(&fixture.db, tenant_id, false,)
            .await
            .unwrap()
            .iter()
            .all(|user| user.identity.id != member_id)
    );
    let deleted_membership = wareboxes_persistence_postgres::users::get_tenant_user(
        &fixture.db,
        tenant_id,
        member_id,
        true,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(deleted_membership.membership_deleted.is_some());
    assert!(deleted_membership.identity.deleted.is_none());

    assert!(
        wareboxes_persistence_postgres::users::set_user_membership_deleted(
            &fixture.db,
            tenant_id,
            member_id,
            false,
        )
        .await
        .unwrap()
    );
    let admin_db = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE users SET deleted = CURRENT_TIMESTAMP WHERE id = $1")
        .bind(member.id)
        .execute(&admin_db)
        .await
        .unwrap();
    let globally_deleted = wareboxes_persistence_postgres::users::get_tenant_user(
        &fixture.db,
        tenant_id,
        member_id,
        true,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(globally_deleted.identity.deleted.is_some());
    assert!(globally_deleted.membership_deleted.is_none());
    assert!(
        wareboxes_persistence_postgres::users::find_user_by_id(&fixture.db, member_id, false,)
            .await
            .unwrap()
            .is_none()
    );
    assert!(wareboxes_persistence_postgres::users::find_user_by_email(
        &fixture.db,
        &member.email,
        true,
    )
    .await
    .unwrap()
    .is_some());
}

#[tokio::test]
async fn tenant_admin_profile_updates_preserve_current_global_profile_semantics() {
    let fixture = Fixture::new().await;
    let member = fixture.user("shared-profile@test.com").await;
    let member_id = typed_user_id(member.id);
    let home_tenant = tenant_for_user(&fixture.db, member.id).await;
    let second_admin = fixture.user("second-admin@test.com").await;
    let second_tenant = tenant_for_user(&fixture.db, second_admin.id).await;
    let unrelated_admin = fixture.user("unrelated-admin@test.com").await;
    let unrelated_tenant = tenant_for_user(&fixture.db, unrelated_admin.id).await;
    add_membership(&fixture.db, second_tenant, member_id).await;

    assert!(wareboxes_persistence_postgres::users::update_user(
        &fixture.db,
        second_tenant,
        member_id,
        Some("Shared"),
        Some("Operator"),
        None,
        None,
    )
    .await
    .unwrap());
    let identity =
        wareboxes_persistence_postgres::users::find_user_by_id(&fixture.db, member_id, false)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(identity.first_name.as_deref(), Some("Shared"));
    let home_projection = wareboxes_persistence_postgres::users::get_tenant_user(
        &fixture.db,
        home_tenant,
        member_id,
        false,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        home_projection.identity.last_name.as_deref(),
        Some("Operator")
    );

    assert!(!wareboxes_persistence_postgres::users::update_user(
        &fixture.db,
        unrelated_tenant,
        member_id,
        Some("Blocked"),
        None,
        None,
        None,
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn settings_are_user_scoped_and_login_maps_identity_authorization() {
    let fixture = Fixture::new().await;
    let user = fixture.user("identity-login@test.com").await;
    let other = fixture.user("identity-settings-other@test.com").await;
    let user_id = typed_user_id(user.id);
    let other_id = typed_user_id(other.id);
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;

    assert!(
        !wareboxes_persistence_postgres::settings::get_user_settings(&fixture.db, user_id)
            .await
            .unwrap()
            .light_mode
    );
    assert!(
        wareboxes_persistence_postgres::settings::upsert_user_settings(&fixture.db, user_id, true,)
            .await
            .unwrap()
            .light_mode
    );
    assert!(
        !wareboxes_persistence_postgres::settings::get_user_settings(&fixture.db, other_id)
            .await
            .unwrap()
            .light_mode
    );

    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "receiving.execute",
        None,
    )
    .await
    .unwrap();
    let role =
        wareboxes_persistence_postgres::roles::add_role(&fixture.db, tenant_id, "receiver", None)
            .await
            .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user.id, role)
        .await
        .unwrap();

    let login = routes::auth::login(
        State(AppState::new(fixture.db.clone())),
        Json(LoginRequest {
            email: user.email,
            password: "supersecret".to_owned(),
        }),
    )
    .await
    .unwrap()
    .0;
    assert!(login.settings.light_mode);
    assert!(login.user.user_roles.iter().any(|item| item.id == role));
    assert!(login
        .user
        .user_permissions
        .iter()
        .any(|item| item.name == "RECEIVING.EXECUTE"));

    assert!(
        !wareboxes_persistence_postgres::settings::upsert_user_settings(
            &fixture.db,
            user_id,
            false,
        )
        .await
        .unwrap()
        .light_mode
    );
}
