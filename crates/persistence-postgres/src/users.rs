use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::authorization::{PermissionReadModel, RoleReadModel};
use wareboxes_application::identity::{TenantUserReadModel, UserIdentityReadModel};
use wareboxes_domain::{TenantId, UserId};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::{PersistenceError, PersistenceResult};

fn user_id(value: i64) -> PersistenceResult<UserId> {
    UserId::new(value).map_err(|error| PersistenceError::invalid_data(error.to_string()))
}

fn map_identity(row: &sqlx::postgres::PgRow) -> PersistenceResult<UserIdentityReadModel> {
    Ok(UserIdentityReadModel {
        id: user_id(row.try_get("id")?)?,
        created: row.try_get("created")?,
        deleted: row.try_get("user_deleted")?,
        first_name: row.try_get("first_name")?,
        last_name: row.try_get("last_name")?,
        email: row.try_get("email")?,
        nick_name: row.try_get("nick_name")?,
        phone: row.try_get("phone")?,
    })
}

fn map_role(row: &sqlx::postgres::PgRow) -> PersistenceResult<RoleReadModel> {
    Ok(RoleReadModel {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        parent_id: row.try_get("parent_id")?,
        self_user_id: row.try_get("self_user_id")?,
        parent_roles: Vec::new(),
        child_roles: Vec::new(),
        role_permissions: Vec::new(),
    })
}

async fn tenant_users(
    db: &Db,
    tenant_id: TenantId,
    selected_user_id: Option<UserId>,
    show_deleted: bool,
) -> PersistenceResult<Vec<TenantUserReadModel>> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT user_account.id, user_account.created,
               user_account.deleted AS user_deleted,
               membership.deleted AS membership_deleted,
               user_account.first_name, user_account.last_name, user_account.email,
               user_account.nick_name, user_account.phone
        FROM tenant_memberships membership
        INNER JOIN users user_account ON user_account.id = membership.user_id
        WHERE membership.tenant_id = $1
          AND ($2 OR (membership.deleted IS NULL AND user_account.deleted IS NULL))
          AND ($3::BIGINT IS NULL OR user_account.id = $3)
        ORDER BY user_account.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .bind(selected_user_id.map(UserId::get))
    .fetch_all(&mut *tx)
    .await?;

    let mut users = Vec::with_capacity(rows.len());
    for row in &rows {
        users.push(TenantUserReadModel {
            identity: map_identity(row)?,
            membership_deleted: row.try_get("membership_deleted")?,
            direct_roles: Vec::new(),
            permissions: Vec::new(),
        });
    }

    let user_ids = users
        .iter()
        .map(|user| user.identity.id.get())
        .collect::<Vec<_>>();
    if user_ids.is_empty() {
        tx.commit().await?;
        return Ok(users);
    }

    let role_rows = sqlx::query(
        r#"
        SELECT user_role.user_id, role.id, role.created, role.deleted, role.name,
               role.description, role.parent_id, role.self_user_id
        FROM user_roles user_role
        INNER JOIN roles role
            ON role.tenant_id = user_role.tenant_id AND role.id = user_role.role_id
        WHERE user_role.tenant_id = $1
          AND user_role.user_id = ANY($2)
          AND user_role.deleted IS NULL
          AND role.deleted IS NULL
        ORDER BY user_role.user_id, role.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(&user_ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut roles_by_user: HashMap<UserId, Vec<RoleReadModel>> = HashMap::new();
    for row in &role_rows {
        roles_by_user
            .entry(user_id(row.try_get("user_id")?)?)
            .or_default()
            .push(map_role(row)?);
    }

    let permission_rows = sqlx::query(
        r#"
        WITH RECURSIVE role_hierarchy AS (
            SELECT user_role.user_id, role.id, role.parent_id
            FROM user_roles user_role
            INNER JOIN roles role
                ON role.tenant_id = user_role.tenant_id AND role.id = user_role.role_id
            WHERE user_role.tenant_id = $1
              AND user_role.user_id = ANY($2)
              AND user_role.deleted IS NULL
              AND role.deleted IS NULL
            UNION
            SELECT hierarchy.user_id, role.id, role.parent_id
            FROM role_hierarchy hierarchy
            INNER JOIN roles role ON role.id = hierarchy.parent_id
            WHERE role.tenant_id = $1 AND role.deleted IS NULL
        )
        SELECT DISTINCT hierarchy.user_id, permission.id, permission.created,
               permission.deleted, UPPER(permission.name) AS name,
               permission.description
        FROM role_hierarchy hierarchy
        INNER JOIN role_permissions role_permission
            ON role_permission.tenant_id = $1
           AND role_permission.role_id = hierarchy.id
        INNER JOIN permissions permission
            ON permission.tenant_id = role_permission.tenant_id
           AND permission.id = role_permission.permission_id
        WHERE role_permission.deleted IS NULL
          AND permission.deleted IS NULL
        ORDER BY hierarchy.user_id, permission.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(&user_ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut permissions_by_user: HashMap<UserId, Vec<PermissionReadModel>> = HashMap::new();
    for row in &permission_rows {
        permissions_by_user
            .entry(user_id(row.try_get("user_id")?)?)
            .or_default()
            .push(PermissionReadModel {
                id: row.try_get("id")?,
                created: row.try_get("created")?,
                deleted: row.try_get("deleted")?,
                name: row.try_get("name")?,
                description: row.try_get("description")?,
            });
    }
    tx.commit().await?;

    for user in &mut users {
        user.direct_roles = roles_by_user.remove(&user.identity.id).unwrap_or_default();
        user.permissions = permissions_by_user
            .remove(&user.identity.id)
            .unwrap_or_default();
    }
    Ok(users)
}

pub async fn get_tenant_users(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> PersistenceResult<Vec<TenantUserReadModel>> {
    tenant_users(db, tenant_id, None, show_deleted).await
}

pub async fn get_tenant_user(
    db: &Db,
    tenant_id: TenantId,
    user_id: UserId,
    include_deleted: bool,
) -> PersistenceResult<Option<TenantUserReadModel>> {
    Ok(tenant_users(db, tenant_id, Some(user_id), include_deleted)
        .await?
        .into_iter()
        .next())
}

pub async fn find_user_by_id(
    db: &Db,
    id: UserId,
    include_deleted: bool,
) -> PersistenceResult<Option<UserIdentityReadModel>> {
    let row = sqlx::query(
        r#"
        SELECT id, created, deleted AS user_deleted, first_name, last_name, email,
               nick_name, phone
        FROM users
        WHERE id = $1 AND ($2 OR deleted IS NULL)
        "#,
    )
    .bind(id.get())
    .bind(include_deleted)
    .fetch_optional(db)
    .await?;
    row.as_ref().map(map_identity).transpose()
}

pub async fn find_user_by_email(
    db: &Db,
    email: &str,
    include_deleted: bool,
) -> PersistenceResult<Option<UserIdentityReadModel>> {
    let row = sqlx::query(
        r#"
        SELECT id, created, deleted AS user_deleted, first_name, last_name, email,
               nick_name, phone
        FROM users
        WHERE email = $1 AND ($2 OR deleted IS NULL)
        "#,
    )
    .bind(email)
    .bind(include_deleted)
    .fetch_optional(db)
    .await?;
    row.as_ref().map(map_identity).transpose()
}

/// Updates the global profile after verifying an active membership in the tenant.
pub async fn update_user(
    db: &Db,
    tenant_id: TenantId,
    id: UserId,
    first_name: Option<&str>,
    last_name: Option<&str>,
    nick_name: Option<&str>,
    phone: Option<&str>,
) -> PersistenceResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let result = sqlx::query(
        r#"
        UPDATE users
        SET first_name = COALESCE($1, first_name),
            last_name = COALESCE($2, last_name),
            nick_name = COALESCE($3, nick_name),
            phone = COALESCE($4, phone)
        WHERE id = $5
          AND EXISTS (
              SELECT 1 FROM tenant_memberships
              WHERE tenant_id = $6 AND user_id = users.id AND deleted IS NULL
          )
        "#,
    )
    .bind(first_name)
    .bind(last_name)
    .bind(nick_name)
    .bind(phone)
    .bind(id.get())
    .bind(tenant_id.get())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_user_membership_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: UserId,
    deleted: bool,
) -> PersistenceResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let result = sqlx::query(
        "UPDATE tenant_memberships SET deleted = $1 WHERE tenant_id = $2 AND user_id = $3",
    )
    .bind(if deleted { Some(now_iso()) } else { None })
    .bind(tenant_id.get())
    .bind(id.get())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}
