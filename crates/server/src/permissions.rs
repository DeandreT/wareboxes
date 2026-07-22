//! RBAC resolution, ported from `app/utils/permissions.ts` and the recursive
//! role-hierarchy CTEs in `app/utils/roles.ts` / `users.ts`.
//!
//! The `WITH RECURSIVE` query is written once here.

use sqlx::Row;
use wareboxes_core::models::Permission;

use crate::db::{now_iso, Db};
use crate::error::AppResult;

/// All permissions a user has via their roles and every ancestor role.
/// Names are upper-cased to match the original behaviour.
pub async fn get_user_permissions(db: &Db, user_id: i64) -> AppResult<Vec<Permission>> {
    let rows = sqlx::query(
        r#"
        WITH RECURSIVE role_hierarchy AS (
            SELECT r.id AS id, r.parent_id AS parent_id
            FROM user_roles ur
            INNER JOIN roles r ON r.id = ur.role_id
            WHERE ur.user_id = $1
              AND ur.deleted IS NULL
              AND r.deleted IS NULL
            UNION
            SELECT r.id AS id, r.parent_id AS parent_id
            FROM roles r
            INNER JOIN role_hierarchy rh ON rh.id = r.parent_id
            WHERE r.deleted IS NULL
        )
        SELECT DISTINCT p.id AS id,
               UPPER(p.name) AS name,
               p.description AS description,
               p.created AS created,
               p.deleted AS deleted
        FROM permissions p
        INNER JOIN role_permissions rp ON rp.permission_id = p.id
        INNER JOIN role_hierarchy rh ON rh.id = rp.role_id
        WHERE p.deleted IS NULL
          AND rp.deleted IS NULL
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;

    rows.iter()
        .map(|r| {
            Ok(Permission {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                description: r.try_get("description")?,
                created: r.try_get("created")?,
                deleted: r.try_get("deleted")?,
            })
        })
        .collect()
}

/// Ensure the per-user "self role" exists (role named after the user's email,
/// described as "Self role"). Mirrors `addRoleAndUserRole` being called lazily
/// inside `userHasPermission`.
pub async fn ensure_self_role(db: &Db, user_id: i64, email: &str) -> AppResult<()> {
    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT r.id
        FROM user_roles ur
        INNER JOIN roles r ON r.id = ur.role_id
        WHERE ur.user_id = $1 AND r.name = $2 AND ur.deleted IS NULL
        "#,
    )
    .bind(user_id)
    .bind(email)
    .fetch_optional(db)
    .await?;

    if existing.is_some() {
        return Ok(());
    }

    let now = now_iso();
    let role_id: i64 = sqlx::query_scalar(
        "INSERT INTO roles (name, description, created) VALUES ($1, 'Self role', $2) RETURNING id",
    )
    .bind(email)
    .bind(now)
    .fetch_one(db)
    .await?;

    sqlx::query("INSERT INTO user_roles (user_id, role_id, created) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(role_id)
        .bind(now)
        .execute(db)
        .await?;

    Ok(())
}

fn has_admin(perms: &[Permission]) -> bool {
    perms.iter().any(|p| p.name.eq_ignore_ascii_case("admin"))
}

pub async fn user_has_permission(db: &Db, user_id: i64, name: &str) -> AppResult<bool> {
    let perms = get_user_permissions(db, user_id).await?;
    if has_admin(&perms) {
        return Ok(true);
    }
    Ok(perms.iter().any(|p| p.name.eq_ignore_ascii_case(name)))
}

pub async fn user_has_any_permission(db: &Db, user_id: i64, names: &[&str]) -> AppResult<bool> {
    let perms = get_user_permissions(db, user_id).await?;
    if has_admin(&perms) {
        return Ok(true);
    }
    Ok(names
        .iter()
        .any(|n| perms.iter().any(|p| p.name.eq_ignore_ascii_case(n))))
}
