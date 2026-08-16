use sqlx::Row;
use wareboxes_application::support_access::{
    SupportAccessCursor, SupportAccessEventCursor, SupportAccessEventPage,
    SupportAccessEventPageQuery, SupportAccessEventReadModel, SupportAccessOptionsReadModel,
    SupportAccessPage, SupportAccessPageQuery, SupportAccessReadModel, SupportAccessResourceOption,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, SupportAccessGrantId, SupportAccessPolicy, SupportAccessRevision,
    SupportAccessStatus, TenantId, UserId,
};

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::internal(message.into())
}

fn parse_status(value: &str) -> AppResult<SupportAccessStatus> {
    SupportAccessStatus::parse(value)
        .ok_or_else(|| invalid(format!("stored support access status is invalid: {value}")))
}

fn map_row(row: &sqlx::postgres::PgRow) -> AppResult<SupportAccessReadModel> {
    let facility_ids = row
        .try_get::<Vec<i64>, _>("facility_ids")?
        .into_iter()
        .map(|value| FacilityId::new(value).map_err(|error| invalid(error.to_string())))
        .collect::<AppResult<Vec<_>>>()?;
    let inventory_owner_ids = row
        .try_get::<Vec<i64>, _>("inventory_owner_ids")?
        .into_iter()
        .map(|value| InventoryOwnerId::new(value).map_err(|error| invalid(error.to_string())))
        .collect::<AppResult<Vec<_>>>()?;
    Ok(SupportAccessReadModel {
        support_access_grant_id: SupportAccessGrantId::new(row.try_get("support_access_grant_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        tenant_slug: row.try_get("tenant_slug")?,
        tenant_name: row.try_get("tenant_name")?,
        status: parse_status(&row.try_get::<String, _>("effective_status")?)?,
        revision: SupportAccessRevision::new(row.try_get("revision")?)
            .map_err(|error| invalid(error.to_string()))?,
        reason: row.try_get("reason")?,
        access: SupportAccessPolicy {
            all_facilities: row.try_get("all_facilities")?,
            facility_ids,
            all_inventory_owners: row.try_get("all_inventory_owners")?,
            inventory_owner_ids,
            permission_names: row.try_get("permission_names")?,
        },
        requested_at: row.try_get("requested_at")?,
        requested_by: UserId::new(row.try_get("requested_by_user_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        requested_by_email: row.try_get("requested_by_email")?,
        expires_at: row.try_get("expires_at")?,
        approved_at: row.try_get("approved_at")?,
        approved_by: row
            .try_get::<Option<i64>, _>("approved_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| invalid(error.to_string()))?,
        approved_by_email: row.try_get("approved_by_email")?,
        rejected_at: row.try_get("rejected_at")?,
        rejected_by: row
            .try_get::<Option<i64>, _>("rejected_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| invalid(error.to_string()))?,
        rejection_reason: row.try_get("rejection_reason")?,
        revoked_at: row.try_get("revoked_at")?,
        revoked_by: row
            .try_get::<Option<i64>, _>("revoked_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| invalid(error.to_string()))?,
        revocation_reason: row.try_get("revocation_reason")?,
    })
}

const READ_SQL: &str = r#"
SELECT grant_record.id AS support_access_grant_id,grant_record.tenant_id,
  tenant.slug AS tenant_slug,tenant.name AS tenant_name,
  CASE WHEN grant_record.status IN ('pending','active')
    AND grant_record.expires_at<=CURRENT_TIMESTAMP THEN 'expired'
    ELSE grant_record.status END AS effective_status,
  grant_record.revision,grant_record.reason,grant_record.requested_at,
  grant_record.requested_by_user_id,requester.email AS requested_by_email,
  grant_record.expires_at,grant_record.all_facilities,
  ARRAY(SELECT scope.facility_id FROM support_access_facilities scope
    WHERE scope.support_access_grant_id=grant_record.id ORDER BY scope.facility_id)
    AS facility_ids,
  grant_record.all_inventory_owners,
  ARRAY(SELECT scope.inventory_owner_id FROM support_access_inventory_owners scope
    WHERE scope.support_access_grant_id=grant_record.id ORDER BY scope.inventory_owner_id)
    AS inventory_owner_ids,
  ARRAY(SELECT scope.permission_name FROM support_access_permissions scope
    WHERE scope.support_access_grant_id=grant_record.id ORDER BY scope.permission_name)
    AS permission_names,
  grant_record.approved_at,grant_record.approved_by_user_id,
  approver.email AS approved_by_email,grant_record.rejected_at,
  grant_record.rejected_by_user_id,grant_record.rejection_reason,
  grant_record.revoked_at,grant_record.revoked_by_user_id,
  grant_record.revocation_reason
FROM support_access_grants grant_record
JOIN tenants tenant ON tenant.id=grant_record.tenant_id
JOIN users requester ON requester.id=grant_record.requested_by_user_id
LEFT JOIN users approver ON approver.id=grant_record.approved_by_user_id
"#;

pub(super) async fn read_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    support_access_grant_id: SupportAccessGrantId,
) -> AppResult<SupportAccessReadModel> {
    let row = sqlx::query(&format!(
        "{READ_SQL} WHERE grant_record.id=$1 AND tenant.deleted IS NULL"
    ))
    .bind(support_access_grant_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("support access grant"))?;
    map_row(&row)
}

pub async fn by_id(
    db: &Db,
    actor_access: &TenantAccess,
    support_access_grant_id: SupportAccessGrantId,
) -> AppResult<SupportAccessReadModel> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    super::super::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    let result = read_tx(&mut tx, support_access_grant_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn page(
    db: &Db,
    actor_access: &TenantAccess,
    query: &SupportAccessPageQuery,
) -> AppResult<SupportAccessPage> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    super::super::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    let rows = sqlx::query(
        r#"SELECT grant_record.id,grant_record.requested_at
        FROM support_access_grants grant_record
        WHERE ($1::BIGINT IS NULL OR grant_record.tenant_id=$1)
          AND ($2::TEXT IS NULL OR (CASE WHEN grant_record.status IN ('pending','active')
            AND grant_record.expires_at<=CURRENT_TIMESTAMP THEN 'expired'
            ELSE grant_record.status END)=$2)
          AND ($3::TIMESTAMPTZ IS NULL OR (grant_record.requested_at,grant_record.id)<($3,$4))
        ORDER BY grant_record.requested_at DESC,grant_record.id DESC LIMIT $5"#,
    )
    .bind(query.tenant_id.map(TenantId::get))
    .bind(query.status.map(SupportAccessStatus::as_str))
    .bind(query.cursor.map(|cursor| cursor.after_requested_at))
    .bind(
        query
            .cursor
            .map(|cursor| cursor.after_support_access_grant_id.get()),
    )
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut identities = rows
        .iter()
        .map(|row| {
            Ok((
                SupportAccessGrantId::new(row.try_get("id")?)
                    .map_err(|error| invalid(error.to_string()))?,
                row.try_get("requested_at")?,
            ))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if identities.len() > usize::from(query.limit) {
        identities.pop();
        identities.last().map(
            |(support_access_grant_id, requested_at)| SupportAccessCursor {
                after_requested_at: *requested_at,
                after_support_access_grant_id: *support_access_grant_id,
            },
        )
    } else {
        None
    };
    let mut items = Vec::with_capacity(identities.len());
    for (support_access_grant_id, _) in identities {
        items.push(read_tx(&mut tx, support_access_grant_id).await?);
    }
    tx.commit().await?;
    Ok(SupportAccessPage { items, next_cursor })
}

pub async fn options(
    db: &Db,
    actor_access: &TenantAccess,
    tenant_id: TenantId,
) -> AppResult<SupportAccessOptionsReadModel> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    super::super::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(&mut tx, tenant_id).await?;
    let tenant_name: String = sqlx::query_scalar(
        "SELECT name FROM tenants WHERE id=$1 AND status='active' AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("tenant"))?;
    let facilities = sqlx::query(
        "SELECT id,COALESCE(name,'Facility #'||id::TEXT) AS name FROM facilities WHERE tenant_id=$1 AND deleted IS NULL ORDER BY name,id",
    )
    .bind(tenant_id.get())
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| {
        Ok(SupportAccessResourceOption {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
        })
    })
    .collect::<AppResult<Vec<_>>>()?;
    let inventory_owners = sqlx::query(
        "SELECT id,name FROM inventory_owners WHERE tenant_id=$1 AND deleted IS NULL ORDER BY name,id",
    )
    .bind(tenant_id.get())
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| {
        Ok(SupportAccessResourceOption {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
        })
    })
    .collect::<AppResult<Vec<_>>>()?;
    let permission_names = sqlx::query_scalar(
        "SELECT name FROM permissions WHERE tenant_id=$1 AND name<>'admin' AND deleted IS NULL ORDER BY name",
    )
    .bind(tenant_id.get())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(SupportAccessOptionsReadModel {
        tenant_id,
        tenant_name,
        facilities,
        inventory_owners,
        permission_names,
    })
}

fn map_event(row: &sqlx::postgres::PgRow) -> AppResult<SupportAccessEventReadModel> {
    Ok(SupportAccessEventReadModel {
        event_id: row.try_get("event_id")?,
        support_access_grant_id: SupportAccessGrantId::new(row.try_get("support_access_grant_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        action: row.try_get("action")?,
        grant_revision: SupportAccessRevision::new(row.try_get("grant_revision")?)
            .map_err(|error| invalid(error.to_string()))?,
        actor_id: UserId::new(row.try_get("actor_user_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        occurred_at: row.try_get("occurred_at")?,
        reason: row.try_get("reason")?,
        evidence: row.try_get("evidence")?,
    })
}

pub async fn event_page(
    db: &Db,
    actor_access: &TenantAccess,
    query: &SupportAccessEventPageQuery,
) -> AppResult<SupportAccessEventPage> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    super::super::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    read_tx(&mut tx, query.support_access_grant_id).await?;
    let rows = sqlx::query(
        r#"SELECT id AS event_id,support_access_grant_id,tenant_id,action,
        grant_revision,actor_user_id,occurred_at,reason,evidence
        FROM support_access_events WHERE support_access_grant_id=$1
          AND ($2::TIMESTAMPTZ IS NULL OR (occurred_at,id)<($2,$3))
        ORDER BY occurred_at DESC,id DESC LIMIT $4"#,
    )
    .bind(query.support_access_grant_id.get())
    .bind(query.cursor.map(|cursor| cursor.after_occurred_at))
    .bind(query.cursor.map(|cursor| cursor.after_event_id))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows.iter().map(map_event).collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if items.len() > usize::from(query.limit) {
        items.pop();
        items.last().map(|event| SupportAccessEventCursor {
            after_occurred_at: event.occurred_at,
            after_event_id: event.event_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(SupportAccessEventPage { items, next_cursor })
}
