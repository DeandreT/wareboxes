use sqlx::Row;
use wareboxes_application::service_account::{
    ServiceAccountCredentialReadModel, ServiceAccountCursor, ServiceAccountEventCursor,
    ServiceAccountEventPage, ServiceAccountEventPageQuery, ServiceAccountEventReadModel,
    ServiceAccountPage, ServiceAccountPageQuery, ServiceAccountReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, ServiceAccountAccessPolicy, ServiceAccountCredentialId,
    ServiceAccountId, ServiceAccountRevision, ServiceAccountStatus, TenantId, UserId,
};

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

fn invalid_data(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

async fn credentials_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    service_account_id: ServiceAccountId,
) -> AppResult<Vec<ServiceAccountCredentialReadModel>> {
    let rows = sqlx::query(
        r#"SELECT id,label,token_prefix,created_at,created_by_user_id,expires_at,
        revoked_at,revoked_by_user_id,revocation_reason,last_used_at
        FROM service_account_credentials
        WHERE tenant_id=$1 AND service_account_id=$2
        ORDER BY created_at DESC,id DESC"#,
    )
    .bind(tenant_id.get())
    .bind(service_account_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ServiceAccountCredentialReadModel {
                credential_id: ServiceAccountCredentialId::new(row.try_get("id")?)
                    .map_err(invalid_data)?,
                label: row.try_get("label")?,
                token_prefix: row.try_get("token_prefix")?,
                created_at: row.try_get("created_at")?,
                created_by: UserId::new(row.try_get("created_by_user_id")?)
                    .map_err(invalid_data)?,
                expires_at: row.try_get("expires_at")?,
                revoked_at: row.try_get("revoked_at")?,
                revoked_by: row
                    .try_get::<Option<i64>, _>("revoked_by_user_id")?
                    .map(UserId::new)
                    .transpose()
                    .map_err(invalid_data)?,
                revocation_reason: row.try_get("revocation_reason")?,
                last_used_at: row.try_get("last_used_at")?,
            })
        })
        .collect()
}

pub(super) async fn from_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &sqlx::postgres::PgRow,
) -> AppResult<ServiceAccountReadModel> {
    let tenant_id = TenantId::new(row.try_get("tenant_id")?).map_err(invalid_data)?;
    let service_account_id = ServiceAccountId::new(row.try_get("id")?).map_err(invalid_data)?;
    let status: String = row.try_get("status")?;
    Ok(ServiceAccountReadModel {
        service_account_id,
        tenant_id,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: ServiceAccountStatus::parse(&status).ok_or_else(|| {
            AppError::internal(format!("unknown service account status: {status}"))
        })?,
        revision: ServiceAccountRevision::new(row.try_get("revision")?).map_err(invalid_data)?,
        access: ServiceAccountAccessPolicy {
            all_facilities: row.try_get("all_facilities")?,
            facility_ids: row
                .try_get::<Vec<i64>, _>("facility_ids")?
                .into_iter()
                .map(FacilityId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(invalid_data)?,
            all_inventory_owners: row.try_get("all_inventory_owners")?,
            inventory_owner_ids: row
                .try_get::<Vec<i64>, _>("inventory_owner_ids")?
                .into_iter()
                .map(InventoryOwnerId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(invalid_data)?,
            permission_names: row.try_get("permission_names")?,
        },
        created_at: row.try_get("created_at")?,
        created_by: UserId::new(row.try_get("created_by_user_id")?).map_err(invalid_data)?,
        updated_at: row.try_get("updated_at")?,
        updated_by: UserId::new(row.try_get("updated_by_user_id")?).map_err(invalid_data)?,
        disabled_at: row.try_get("disabled_at")?,
        disabled_by: row
            .try_get::<Option<i64>, _>("disabled_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(invalid_data)?,
        disabled_reason: row.try_get("disabled_reason")?,
        last_used_at: row.try_get("last_used_at")?,
        credentials: credentials_tx(tx, tenant_id, service_account_id).await?,
    })
}

pub(super) const SELECT_ACCOUNT: &str = r#"
SELECT account.*,
  ARRAY(SELECT scope.facility_id FROM service_account_facilities scope
    WHERE scope.tenant_id=account.tenant_id AND scope.service_account_id=account.id
      AND scope.revoked_at IS NULL ORDER BY scope.facility_id) AS facility_ids,
  ARRAY(SELECT scope.inventory_owner_id FROM service_account_inventory_owners scope
    WHERE scope.tenant_id=account.tenant_id AND scope.service_account_id=account.id
      AND scope.revoked_at IS NULL ORDER BY scope.inventory_owner_id) AS inventory_owner_ids,
  ARRAY(SELECT permission.name FROM service_account_permissions grant_record
    JOIN permissions permission ON permission.tenant_id=grant_record.tenant_id
      AND permission.id=grant_record.permission_id
    WHERE grant_record.tenant_id=account.tenant_id
      AND grant_record.service_account_id=account.id AND grant_record.revoked_at IS NULL
      AND permission.deleted IS NULL ORDER BY permission.name) AS permission_names
FROM service_accounts account
"#;

pub(super) async fn read_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    service_account_id: ServiceAccountId,
) -> AppResult<ServiceAccountReadModel> {
    let row = sqlx::query(&format!(
        "{SELECT_ACCOUNT} WHERE account.tenant_id=$1 AND account.id=$2"
    ))
    .bind(tenant_id.get())
    .bind(service_account_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("service account"))?;
    from_row_tx(tx, &row).await
}

pub async fn by_id(
    db: &Db,
    access: &TenantAccess,
    service_account_id: ServiceAccountId,
) -> AppResult<ServiceAccountReadModel> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "admin").await?;
    let result = read_tx(&mut tx, access.tenant_id, service_account_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn permission_options(db: &Db, access: &TenantAccess) -> AppResult<Vec<String>> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "admin").await?;
    let permissions = sqlx::query_scalar(
        r#"SELECT name FROM permissions WHERE tenant_id=$1 AND name<>'admin'
        AND deleted IS NULL ORDER BY name"#,
    )
    .bind(access.tenant_id.get())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(permissions)
}

pub async fn page(
    db: &Db,
    access: &TenantAccess,
    query: &ServiceAccountPageQuery,
) -> AppResult<ServiceAccountPage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "admin").await?;
    let status = query.status.map(ServiceAccountStatus::as_str);
    let (cursor_at, cursor_id) = query.cursor.map_or((None, None), |cursor| {
        (
            Some(cursor.after_created_at),
            Some(cursor.after_service_account_id.get()),
        )
    });
    let rows = sqlx::query(&format!(
        r#"{SELECT_ACCOUNT}
        WHERE account.tenant_id=$1 AND ($2::text IS NULL OR account.status=$2)
          AND ($3::timestamptz IS NULL OR (account.created_at,account.id)<($3,$4))
        ORDER BY account.created_at DESC,account.id DESC LIMIT $5"#
    ))
    .bind(access.tenant_id.get())
    .bind(status)
    .bind(cursor_at)
    .bind(cursor_id)
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let mut items = Vec::with_capacity(rows.len().min(usize::from(query.limit)));
    for row in rows.into_iter().take(usize::from(query.limit)) {
        items.push(from_row_tx(&mut tx, &row).await?);
    }
    let next_cursor = if has_more {
        items.last().map(|item| ServiceAccountCursor {
            after_created_at: item.created_at,
            after_service_account_id: item.service_account_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(ServiceAccountPage { items, next_cursor })
}

pub async fn event_page(
    db: &Db,
    access: &TenantAccess,
    query: &ServiceAccountEventPageQuery,
) -> AppResult<ServiceAccountEventPage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "admin").await?;
    if !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM service_accounts WHERE tenant_id=$1 AND id=$2)",
    )
    .bind(access.tenant_id.get())
    .bind(query.service_account_id.get())
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(AppError::not_found("service account"));
    }
    let (cursor_at, cursor_id) = query.cursor.map_or((None, None), |cursor| {
        (Some(cursor.after_occurred_at), Some(cursor.after_event_id))
    });
    let rows = sqlx::query(
        r#"SELECT id,service_account_id,credential_id,action,account_revision,
        actor_user_id,occurred_at,evidence FROM service_account_events
        WHERE tenant_id=$1 AND service_account_id=$2
          AND ($3::timestamptz IS NULL OR (occurred_at,id)<($3,$4))
        ORDER BY occurred_at DESC,id DESC LIMIT $5"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.service_account_id.get())
    .bind(cursor_at)
    .bind(cursor_id)
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .into_iter()
        .take(usize::from(query.limit))
        .map(|row| {
            Ok(ServiceAccountEventReadModel {
                event_id: row.try_get("id")?,
                service_account_id: ServiceAccountId::new(row.try_get("service_account_id")?)
                    .map_err(invalid_data)?,
                credential_id: row
                    .try_get::<Option<i64>, _>("credential_id")?
                    .map(ServiceAccountCredentialId::new)
                    .transpose()
                    .map_err(invalid_data)?,
                action: row.try_get("action")?,
                account_revision: ServiceAccountRevision::new(row.try_get("account_revision")?)
                    .map_err(invalid_data)?,
                actor_id: UserId::new(row.try_get("actor_user_id")?).map_err(invalid_data)?,
                occurred_at: row.try_get("occurred_at")?,
                evidence: row.try_get("evidence")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if has_more {
        items.last().map(|last| ServiceAccountEventCursor {
            after_occurred_at: last.occurred_at,
            after_event_id: last.event_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(ServiceAccountEventPage { items, next_cursor })
}
