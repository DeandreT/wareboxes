//! Ported from `app/utils/users.ts`. `userRoles` are the direct (non-deleted)
//! role assignments; `userPermissions` is the recursive hierarchy resolution
//! (delegated to `crate::permissions`).

use sqlx::Row;
use wareboxes_core::models::{Role, User};
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::AppResult;
use crate::permissions::get_user_permissions;

fn map_user(row: &sqlx::postgres::PgRow) -> AppResult<User> {
    Ok(User {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        first_name: row.try_get("first_name")?,
        last_name: row.try_get("last_name")?,
        email: row.try_get("email")?,
        nick_name: row.try_get("nick_name")?,
        phone: row.try_get("phone")?,
        user_roles: Vec::new(),
        user_permissions: Vec::new(),
    })
}

async fn direct_roles(db: &Db, tenant_id: TenantId, user_id: i64) -> AppResult<Vec<Role>> {
    let rows = sqlx::query(
        r#"
        SELECT r.id AS id, r.created AS created, r.deleted AS deleted,
               r.name AS name, r.description AS description, r.parent_id AS parent_id,
               r.self_user_id AS self_user_id
        FROM user_roles ur
        INNER JOIN roles r ON r.tenant_id = ur.tenant_id AND r.id = ur.role_id
        WHERE ur.tenant_id = $1
          AND ur.user_id = $2
          AND ur.deleted IS NULL
          AND r.deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(Role {
                id: r.try_get("id")?,
                created: r.try_get("created")?,
                deleted: r.try_get("deleted")?,
                name: r.try_get("name")?,
                description: r.try_get("description")?,
                parent_id: r.try_get("parent_id")?,
                self_user_id: r.try_get("self_user_id")?,
                ..Default::default()
            })
        })
        .collect()
}

pub async fn enrich_for_tenant(db: &Db, tenant_id: TenantId, mut user: User) -> AppResult<User> {
    user.user_roles = direct_roles(db, tenant_id, user.id).await?;
    user.user_permissions = get_user_permissions(db, tenant_id, user.id).await?;
    Ok(user)
}

pub async fn get_users(db: &Db, tenant_id: TenantId, show_deleted: bool) -> AppResult<Vec<User>> {
    let sql = if show_deleted {
        r#"
        SELECT user_account.id, user_account.created, membership.deleted,
               user_account.first_name, user_account.last_name, user_account.email,
               user_account.nick_name, user_account.phone
        FROM tenant_memberships membership
        INNER JOIN users user_account ON user_account.id = membership.user_id
        WHERE membership.tenant_id = $1
        ORDER BY user_account.id
        "#
    } else {
        r#"
        SELECT user_account.id, user_account.created, membership.deleted,
               user_account.first_name, user_account.last_name, user_account.email,
               user_account.nick_name, user_account.phone
        FROM tenant_memberships membership
        INNER JOIN users user_account ON user_account.id = membership.user_id
        WHERE membership.tenant_id = $1
          AND membership.deleted IS NULL
          AND user_account.deleted IS NULL
        ORDER BY user_account.id
        "#
    };
    let rows = sqlx::query(sql).bind(tenant_id.get()).fetch_all(db).await?;
    let mut users = Vec::with_capacity(rows.len());
    for row in &rows {
        users.push(enrich_for_tenant(db, tenant_id, map_user(row)?).await?);
    }
    Ok(users)
}

pub async fn get_user_by_id(db: &Db, id: i64, include_deleted: bool) -> AppResult<Option<User>> {
    let row = sqlx::query(
        "SELECT id, created, deleted, first_name, last_name, email, nick_name, phone FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let user = map_user(&row)?;
    if user.deleted.is_some() && !include_deleted {
        return Ok(None);
    }
    Ok(Some(user))
}

pub async fn get_user_by_email(
    db: &Db,
    email: &str,
    include_deleted: bool,
) -> AppResult<Option<User>> {
    let row = sqlx::query(
        "SELECT id, created, deleted, first_name, last_name, email, nick_name, phone FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let user = map_user(&row)?;
    if user.deleted.is_some() && !include_deleted {
        return Ok(None);
    }
    Ok(Some(user))
}

pub async fn update_user(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    first_name: Option<&str>,
    last_name: Option<&str>,
    nick_name: Option<&str>,
    phone: Option<&str>,
) -> AppResult<bool> {
    let res = sqlx::query(
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
    .bind(id)
    .bind(tenant_id.get())
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_user_membership_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE tenant_memberships SET deleted = $1 WHERE tenant_id = $2 AND user_id = $3",
    )
    .bind(if deleted { Some(now_iso()) } else { None })
    .bind(tenant_id.get())
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}
