//! API-facing authorization adapters.

use wareboxes_application::authorization::{has_any_named_permission, has_named_permission};
use wareboxes_domain::TenantId;

use crate::db::Db;
use crate::error::AppResult;

pub(crate) async fn ensure_self_role(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    email: &str,
) -> AppResult<()> {
    wareboxes_persistence_postgres::authorization::ensure_self_role(db, tenant_id, user_id, email)
        .await?;
    Ok(())
}

pub async fn user_has_permission(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    name: &str,
) -> AppResult<bool> {
    let permissions =
        wareboxes_persistence_postgres::authorization::get_user_permissions(db, tenant_id, user_id)
            .await?;
    Ok(has_named_permission(&permissions, name))
}

pub async fn user_has_any_permission(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    names: &[&str],
) -> AppResult<bool> {
    let permissions =
        wareboxes_persistence_postgres::authorization::get_user_permissions(db, tenant_id, user_id)
            .await?;
    Ok(has_any_named_permission(&permissions, names))
}
