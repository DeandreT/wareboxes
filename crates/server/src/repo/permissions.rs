//! Ported from `app/utils/permissions.ts` (the CRUD half).

use sqlx::Row;
use wareboxes_core::models::Permission;

use crate::db::{now_iso, Db};
use crate::error::AppResult;

fn map(row: &sqlx::postgres::PgRow) -> AppResult<Permission> {
    Ok(Permission {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
    })
}

pub async fn get_permissions(db: &Db, show_deleted: bool) -> AppResult<Vec<Permission>> {
    let sql = if show_deleted {
        "SELECT id, created, deleted, name, description FROM permissions ORDER BY created DESC"
    } else {
        "SELECT id, created, deleted, name, description FROM permissions WHERE deleted IS NULL ORDER BY created DESC"
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    rows.iter().map(map).collect()
}

pub async fn find_by_name(db: &Db, name: &str) -> AppResult<Option<Permission>> {
    let row = sqlx::query(
        "SELECT id, created, deleted, name, description FROM permissions WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(db)
    .await?;
    row.as_ref().map(map).transpose()
}

/// Insert, or if the (unique) name already exists, revive it. Mirrors the
/// `onConflictDoUpdate { set: { deleted: null } }` behaviour.
pub async fn add_permission(db: &Db, name: &str, description: Option<&str>) -> AppResult<i64> {
    if let Some(existing) = find_by_name(db, name).await? {
        sqlx::query("UPDATE permissions SET deleted = NULL WHERE id = $1")
            .bind(existing.id)
            .execute(db)
            .await?;
        return Ok(existing.id);
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO permissions (name, description, created) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(description)
    .bind(now_iso())
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn update_permission(
    db: &Db,
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
) -> AppResult<bool> {
    let res = sqlx::query(
        r#"
        UPDATE permissions
        SET name = COALESCE($1, name),
            description = COALESCE($2, description)
        WHERE id = $3
        "#,
    )
    .bind(name)
    .bind(description)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_deleted(db: &Db, id: i64, deleted: bool) -> AppResult<bool> {
    let res = sqlx::query("UPDATE permissions SET deleted = $1 WHERE id = $2")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
