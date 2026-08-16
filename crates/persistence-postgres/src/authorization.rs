//! Tenant-scoped authorization projections and self-role persistence.

use sqlx::Row;
use wareboxes_application::authorization::PermissionReadModel;
use wareboxes_domain::TenantId;

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::PersistenceResult;

/// All permissions a user has via their roles and every ancestor role.
/// Names are upper-cased to match the original behaviour.
pub async fn get_user_permissions(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
) -> PersistenceResult<Vec<PermissionReadModel>> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        WITH RECURSIVE role_hierarchy AS (
            SELECT r.id AS id, r.parent_id AS parent_id
            FROM user_roles ur
            INNER JOIN roles r ON r.tenant_id = ur.tenant_id AND r.id = ur.role_id
            WHERE ur.tenant_id = $1
              AND ur.user_id = $2
              AND ur.deleted IS NULL
              AND r.deleted IS NULL
            UNION
            SELECT r.id AS id, r.parent_id AS parent_id
            FROM role_hierarchy rh
            INNER JOIN roles r ON r.id = rh.parent_id
            WHERE r.tenant_id = $1 AND r.deleted IS NULL
        )
        SELECT DISTINCT p.id AS id,
               UPPER(p.name) AS name,
               p.description AS description,
               p.created AS created,
               p.deleted AS deleted
        FROM permissions p
        INNER JOIN role_permissions rp
            ON rp.tenant_id = p.tenant_id AND rp.permission_id = p.id
        INNER JOIN role_hierarchy rh ON rh.id = rp.role_id
        WHERE p.tenant_id = $1
          AND rp.tenant_id = $1
          AND p.deleted IS NULL
          AND rp.deleted IS NULL
        UNION
        SELECT DISTINCT p.id AS id,
               UPPER(p.name) AS name,
               p.description AS description,
               p.created AS created,
               p.deleted AS deleted
        FROM service_accounts account
        INNER JOIN service_account_permissions grant_record
            ON grant_record.tenant_id=account.tenant_id
           AND grant_record.service_account_id=account.id
           AND grant_record.revoked_at IS NULL
        INNER JOIN permissions p ON p.tenant_id=grant_record.tenant_id
           AND p.id=grant_record.permission_id
        WHERE account.tenant_id=$1 AND account.principal_user_id=$2
          AND account.status='active' AND p.deleted IS NULL
        UNION
        SELECT permission.id,permission.name,permission.description,
               permission.created,permission.deleted
        FROM public.active_support_access_permissions($1,$2) permission
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;

    let permissions = rows
        .iter()
        .map(|r| {
            Ok(PermissionReadModel {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                description: r.try_get("description")?,
                created: r.try_get("created")?,
                deleted: r.try_get("deleted")?,
            })
        })
        .collect();
    tx.commit().await?;
    permissions
}

/// Ensure the per-user "self role" exists (role named after the user's email,
/// described as "Self role"). Mirrors `addRoleAndUserRole` being called lazily
/// inside `userHasPermission`.
pub async fn ensure_self_role(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    email: &str,
) -> PersistenceResult<()> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let now = now_iso();
    let role_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO roles (tenant_id, name, description, self_user_id, created)
        VALUES ($1, $2, 'Self role', $3, $4)
        ON CONFLICT (tenant_id, self_user_id) DO UPDATE
        SET deleted = NULL, name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(email)
    .bind(user_id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO user_roles (tenant_id, user_id, role_id, created)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (tenant_id, user_id, role_id) DO UPDATE
        SET deleted = NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .bind(role_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}
