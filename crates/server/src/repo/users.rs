//! Ported from `app/utils/users.ts`. `userRoles` are the direct (non-deleted)
//! role assignments; `userPermissions` is the recursive hierarchy resolution
//! (delegated to `crate::permissions`).

use sqlx::Row;
use wareboxes_core::models::{Role, User};

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

async fn direct_roles(db: &Db, user_id: i64) -> AppResult<Vec<Role>> {
    let rows = sqlx::query(
        r#"
        SELECT r.id AS id, r.created AS created, r.deleted AS deleted,
               r.name AS name, r.description AS description, r.parent_id AS parent_id
        FROM user_roles ur
        INNER JOIN roles r ON r.id = ur.role_id
        WHERE ur.user_id = $1 AND ur.deleted IS NULL AND r.deleted IS NULL
        "#,
    )
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
                ..Default::default()
            })
        })
        .collect()
}

async fn enrich(db: &Db, mut user: User) -> AppResult<User> {
    user.user_roles = direct_roles(db, user.id).await?;
    user.user_permissions = get_user_permissions(db, user.id).await?;
    Ok(user)
}

pub async fn get_users(db: &Db, show_deleted: bool) -> AppResult<Vec<User>> {
    let sql = if show_deleted {
        "SELECT id, created, deleted, first_name, last_name, email, nick_name, phone FROM users ORDER BY id"
    } else {
        "SELECT id, created, deleted, first_name, last_name, email, nick_name, phone FROM users WHERE deleted IS NULL ORDER BY id"
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    let mut users = Vec::with_capacity(rows.len());
    for row in &rows {
        users.push(enrich(db, map_user(row)?).await?);
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
    Ok(Some(enrich(db, user).await?))
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
    Ok(Some(enrich(db, user).await?))
}

pub async fn update_user(
    db: &Db,
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
        "#,
    )
    .bind(first_name)
    .bind(last_name)
    .bind(nick_name)
    .bind(phone)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_user_deleted(db: &Db, id: i64, deleted: bool) -> AppResult<bool> {
    let res = sqlx::query("UPDATE users SET deleted = $1 WHERE id = $2")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
