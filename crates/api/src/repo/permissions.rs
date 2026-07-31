//! Ported from `app/utils/permissions.ts` (the CRUD half).

use sqlx::Row;
use wareboxes_core::models::Permission;
use wareboxes_domain::TenantId;

use crate::db::{begin_tenant_transaction, now_iso, Db};
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

pub async fn get_permissions(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<Permission>> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let sql = if show_deleted {
        "SELECT id, created, deleted, name, description FROM permissions WHERE tenant_id = $1 ORDER BY created DESC"
    } else {
        "SELECT id, created, deleted, name, description FROM permissions WHERE tenant_id = $1 AND deleted IS NULL ORDER BY created DESC"
    };
    let rows = sqlx::query(sql)
        .bind(tenant_id.get())
        .fetch_all(&mut *tx)
        .await?;
    let permissions = rows.iter().map(map).collect();
    tx.commit().await?;
    permissions
}

pub async fn find_by_name(
    db: &Db,
    tenant_id: TenantId,
    name: &str,
) -> AppResult<Option<Permission>> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let row = sqlx::query(
        "SELECT id, created, deleted, name, description FROM permissions WHERE tenant_id = $1 AND name = $2",
    )
    .bind(tenant_id.get())
    .bind(name)
    .fetch_optional(&mut *tx)
    .await?;
    let permission = row.as_ref().map(map).transpose()?;
    tx.commit().await?;
    Ok(permission)
}

/// Insert, or if the (unique) name already exists, revive it. Mirrors the
/// `onConflictDoUpdate { set: { deleted: null } }` behaviour.
pub async fn add_permission(
    db: &Db,
    tenant_id: TenantId,
    name: &str,
    description: Option<&str>,
) -> AppResult<i64> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO permissions (tenant_id, name, description, created)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (tenant_id, name) DO UPDATE
        SET deleted = NULL
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(name)
    .bind(description)
    .bind(now_iso())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn update_permission(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let res = sqlx::query(
        r#"
        UPDATE permissions
        SET name = COALESCE($1, name),
            description = COALESCE($2, description)
        WHERE tenant_id = $3 AND id = $4
        "#,
    )
    .bind(name)
    .bind(description)
    .bind(tenant_id.get())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_deleted(db: &Db, tenant_id: TenantId, id: i64, deleted: bool) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let res = sqlx::query("UPDATE permissions SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(tenant_id.get())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}
