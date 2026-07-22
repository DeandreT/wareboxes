use sqlx::Row;
use wareboxes_core::models::{TenantAccess, TenantStatus};
use wareboxes_domain::{TenantId, UserId};

use crate::db::Db;
use crate::error::{AppError, AppResult};

fn tenant_access_from_row(row: &sqlx::postgres::PgRow) -> AppResult<TenantAccess> {
    let status: String = row.try_get("status")?;
    Ok(TenantAccess {
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        user_id: UserId::new(row.try_get("user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        slug: row.try_get("slug")?,
        name: row.try_get("name")?,
        status: TenantStatus::parse(&status)
            .ok_or_else(|| AppError::internal(format!("unknown tenant status: {status}")))?,
        is_default: row.try_get("is_default")?,
    })
}

pub async fn list_for_user(db: &Db, user_id: i64) -> AppResult<Vec<TenantAccess>> {
    let rows = sqlx::query(
        r#"
        SELECT
            membership.tenant_id,
            membership.user_id,
            membership.is_default,
            tenant.slug,
            tenant.name,
            tenant.status
        FROM tenant_memberships membership
        JOIN tenants tenant ON tenant.id = membership.tenant_id
        WHERE membership.user_id = $1
          AND membership.deleted IS NULL
          AND tenant.deleted IS NULL
          AND tenant.status = 'active'
        ORDER BY membership.is_default DESC, tenant.name, tenant.id
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;

    rows.iter().map(tenant_access_from_row).collect()
}

pub async fn default_for_user(db: &Db, user_id: i64) -> AppResult<Option<TenantAccess>> {
    let row = sqlx::query(
        r#"
        SELECT
            membership.tenant_id,
            membership.user_id,
            membership.is_default,
            tenant.slug,
            tenant.name,
            tenant.status
        FROM tenant_memberships membership
        JOIN tenants tenant ON tenant.id = membership.tenant_id
        WHERE membership.user_id = $1
          AND membership.deleted IS NULL
          AND tenant.deleted IS NULL
          AND tenant.status = 'active'
        ORDER BY membership.is_default DESC, tenant.name, tenant.id
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    row.as_ref().map(tenant_access_from_row).transpose()
}

pub async fn access_for_user(
    db: &Db,
    user_id: i64,
    tenant_id: TenantId,
) -> AppResult<Option<TenantAccess>> {
    let row = sqlx::query(
        r#"
        SELECT
            membership.tenant_id,
            membership.user_id,
            membership.is_default,
            tenant.slug,
            tenant.name,
            tenant.status
        FROM tenant_memberships membership
        JOIN tenants tenant ON tenant.id = membership.tenant_id
        WHERE membership.user_id = $1
          AND membership.tenant_id = $2
          AND membership.deleted IS NULL
          AND tenant.deleted IS NULL
          AND tenant.status = 'active'
        "#,
    )
    .bind(user_id)
    .bind(tenant_id.get())
    .fetch_optional(db)
    .await?;

    row.as_ref().map(tenant_access_from_row).transpose()
}

pub async fn ensure_default_for_user(
    db: &Db,
    user_id: i64,
    tenant_name: &str,
) -> AppResult<TenantId> {
    let mut transaction = db.begin().await?;

    // Serialize provisioning for a user so concurrent login/registration paths
    // cannot create multiple default memberships.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('tenant-provision:' || $1::TEXT, 0))",
    )
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;

    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT tenant_id
        FROM tenant_memberships
        WHERE user_id = $1 AND deleted IS NULL AND is_default
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?;

    if let Some(tenant_id) = existing {
        transaction.commit().await?;
        return TenantId::new(tenant_id).map_err(|error| AppError::internal(error.to_string()));
    }

    let slug = format!("tenant-{user_id}");
    let tenant_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO tenants (slug, name)
        VALUES ($1, $2)
        ON CONFLICT (slug) DO UPDATE
        SET deleted = NULL
        RETURNING id
        "#,
    )
    .bind(slug)
    .bind(tenant_name)
    .fetch_one(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO tenant_memberships (tenant_id, user_id, is_default)
        VALUES ($1, $2, TRUE)
        ON CONFLICT (tenant_id, user_id) DO UPDATE
        SET deleted = NULL, is_default = TRUE
        "#,
    )
    .bind(tenant_id)
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    TenantId::new(tenant_id).map_err(|error| AppError::internal(error.to_string()))
}
