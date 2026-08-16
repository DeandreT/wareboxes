use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::support_access::{
    ApproveSupportAccessCommand, ApproveSupportAccessResult, RejectSupportAccessCommand,
    RejectSupportAccessResult, RequestSupportAccessCommand, RequestSupportAccessResult,
    RevokeSupportAccessCommand, RevokeSupportAccessResult, APPROVE_SUPPORT_ACCESS_OPERATION,
    REJECT_SUPPORT_ACCESS_OPERATION, REQUEST_SUPPORT_ACCESS_OPERATION,
    REVOKE_SUPPORT_ACCESS_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    validate_support_access_window, SupportAccessGrantId, SupportAccessRevision,
    SupportAccessStatus, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};

use super::events::{self, SupportAccessEvent};

#[derive(Debug, Clone, Copy)]
struct LockedGrant {
    id: SupportAccessGrantId,
    tenant_id: TenantId,
    requested_by: UserId,
    status: SupportAccessStatus,
    revision: SupportAccessRevision,
    expires_at: Timestamp,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::internal(message.into())
}

fn revision_conflict() -> AppError {
    AppError::conflict("support access revision does not match expected revision")
}

async fn authorize_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_access: &TenantAccess,
    context: &CommandContext,
) -> AppResult<()> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    super::super::tenant_lifecycle::authorize_tx(tx, actor_access, context.actor_id).await
}

async fn lock_grant_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    support_access_grant_id: SupportAccessGrantId,
) -> AppResult<LockedGrant> {
    let row = sqlx::query(
        r#"SELECT id,tenant_id,requested_by_user_id,status,revision,expires_at
        FROM support_access_grants WHERE id=$1 FOR UPDATE"#,
    )
    .bind(support_access_grant_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("support access grant"))?;
    Ok(LockedGrant {
        id: SupportAccessGrantId::new(row.try_get("id")?)
            .map_err(|error| invalid(error.to_string()))?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        requested_by: UserId::new(row.try_get("requested_by_user_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        status: SupportAccessStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or_else(|| invalid("stored support access status is invalid"))?,
        revision: SupportAccessRevision::new(row.try_get("revision")?)
            .map_err(|error| invalid(error.to_string()))?,
        expires_at: row.try_get("expires_at")?,
    })
}

async fn grant_identity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    support_access_grant_id: SupportAccessGrantId,
) -> AppResult<(TenantId, UserId)> {
    let row =
        sqlx::query("SELECT tenant_id,requested_by_user_id FROM support_access_grants WHERE id=$1")
            .bind(support_access_grant_id.get())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| AppError::not_found("support access grant"))?;
    Ok((
        TenantId::new(row.try_get("tenant_id")?).map_err(|error| invalid(error.to_string()))?,
        UserId::new(row.try_get("requested_by_user_id")?)
            .map_err(|error| invalid(error.to_string()))?,
    ))
}

async fn validate_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &RequestSupportAccessCommand,
) -> AppResult<()> {
    command
        .access
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(tx, tenant_id).await?;
    let tenant_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM tenants WHERE id=$1 AND status='active' AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if !tenant_exists {
        return Err(AppError::not_found("tenant"));
    }
    if !command.access.all_facilities {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM facilities WHERE tenant_id=$1 AND id=ANY($2) AND deleted IS NULL",
        )
        .bind(tenant_id.get())
        .bind(
            command
                .access
                .facility_ids
                .iter()
                .map(|value| value.get())
                .collect::<Vec<_>>(),
        )
        .fetch_one(&mut **tx)
        .await?;
        if count
            != i64::try_from(command.access.facility_ids.len())
                .map_err(|_| AppError::bad_request("too many facilities"))?
        {
            return Err(AppError::not_found("facility"));
        }
    }
    if !command.access.all_inventory_owners {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM inventory_owners WHERE tenant_id=$1 AND id=ANY($2) AND deleted IS NULL",
        )
        .bind(tenant_id.get())
        .bind(
            command
                .access
                .inventory_owner_ids
                .iter()
                .map(|value| value.get())
                .collect::<Vec<_>>(),
        )
        .fetch_one(&mut **tx)
        .await?;
        if count
            != i64::try_from(command.access.inventory_owner_ids.len())
                .map_err(|_| AppError::bad_request("too many inventory owners"))?
        {
            return Err(AppError::not_found("inventory owner"));
        }
    }
    let permission_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM permissions
        WHERE tenant_id=$1 AND name=ANY($2) AND name<>'admin' AND deleted IS NULL"#,
    )
    .bind(tenant_id.get())
    .bind(&command.access.permission_names)
    .fetch_one(&mut **tx)
    .await?;
    if permission_count
        != i64::try_from(command.access.permission_names.len())
            .map_err(|_| AppError::bad_request("too many permissions"))?
    {
        return Err(AppError::not_found("permission"));
    }
    Ok(())
}

struct EventRecord<'a> {
    action: &'a str,
    actor_id: UserId,
    occurred_at: Timestamp,
    reason: Option<&'a str>,
    request_id: &'a str,
    evidence: &'a serde_json::Value,
}

async fn record_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    grant: LockedGrant,
    event: EventRecord<'_>,
) -> AppResult<()> {
    super::super::tenant_lifecycle::bind_platform_tenant_tx(tx, grant.tenant_id).await?;
    events::record_tx(
        tx,
        &SupportAccessEvent {
            support_access_grant_id: grant.id,
            tenant_id: grant.tenant_id,
            action: event.action,
            revision: grant.revision.get(),
            actor_id: event.actor_id,
            occurred_at: event.occurred_at,
            reason: event.reason,
            request_id: event.request_id,
            evidence: event.evidence,
        },
    )
    .await
}

pub async fn request(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &RequestSupportAccessCommand,
) -> AppResult<RequestSupportAccessResult> {
    let prepared = PreparedCommand::new_v1(context, REQUEST_SUPPORT_ACCESS_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let requested_at = now_iso();
    validate_support_access_window(requested_at, command.expires_at)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    validate_policy_tx(&mut tx, command.tenant_id, command).await?;
    let support_access_grant_id = SupportAccessGrantId::new(
        sqlx::query_scalar(
            r#"INSERT INTO support_access_grants
            (tenant_id,revision,status,reason,requested_at,requested_by_user_id,
             expires_at,all_facilities,all_inventory_owners)
            VALUES($1,1,'pending',$2,$3,$4,$5,$6,$7) RETURNING id"#,
        )
        .bind(command.tenant_id.get())
        .bind(command.reason.as_str())
        .bind(requested_at)
        .bind(context.actor_id.get())
        .bind(command.expires_at)
        .bind(command.access.all_facilities)
        .bind(command.access.all_inventory_owners)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| invalid(error.to_string()))?;
    if !command.access.all_facilities {
        sqlx::query(
            r#"INSERT INTO support_access_facilities
            (support_access_grant_id,tenant_id,facility_id)
            SELECT $1,$2,unnest($3::BIGINT[])"#,
        )
        .bind(support_access_grant_id.get())
        .bind(command.tenant_id.get())
        .bind(
            command
                .access
                .facility_ids
                .iter()
                .map(|value| value.get())
                .collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;
    }
    if !command.access.all_inventory_owners {
        sqlx::query(
            r#"INSERT INTO support_access_inventory_owners
            (support_access_grant_id,tenant_id,inventory_owner_id)
            SELECT $1,$2,unnest($3::BIGINT[])"#,
        )
        .bind(support_access_grant_id.get())
        .bind(command.tenant_id.get())
        .bind(
            command
                .access
                .inventory_owner_ids
                .iter()
                .map(|value| value.get())
                .collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        r#"INSERT INTO support_access_permissions
        (support_access_grant_id,tenant_id,permission_name)
        SELECT $1,$2,unnest($3::TEXT[])"#,
    )
    .bind(support_access_grant_id.get())
    .bind(command.tenant_id.get())
    .bind(&command.access.permission_names)
    .execute(&mut *tx)
    .await?;
    let grant = LockedGrant {
        id: support_access_grant_id,
        tenant_id: command.tenant_id,
        requested_by: context.actor_id,
        status: SupportAccessStatus::Pending,
        revision: SupportAccessRevision::new(1).map_err(|error| invalid(error.to_string()))?,
        expires_at: command.expires_at,
    };
    let evidence = serde_json::json!({
        "support_access_grant_id": support_access_grant_id.get(),
        "tenant_id": command.tenant_id.get(),
        "revision": 1,
        "status": "pending",
        "reason": command.reason.as_str(),
        "requested_at": requested_at,
        "requested_by_user_id": context.actor_id.get(),
        "expires_at": command.expires_at,
        "access": command.access,
    });
    record_event_tx(
        &mut tx,
        grant,
        EventRecord {
            action: "requested",
            actor_id: context.actor_id,
            occurred_at: requested_at,
            reason: Some(command.reason.as_str()),
            request_id: &context.request_id,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, support_access_grant_id).await?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(&mut tx, actor_access.tenant_id)
        .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn approve(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &ApproveSupportAccessCommand,
) -> AppResult<ApproveSupportAccessResult> {
    let prepared = PreparedCommand::new_v1(context, APPROVE_SUPPORT_ACCESS_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let (tenant_id, requested_by) =
        grant_identity_tx(&mut tx, command.support_access_grant_id).await?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(&mut tx, tenant_id).await?;
    super::super::access::lock_user_tx(&mut tx, tenant_id, requested_by.get()).await?;
    let current = lock_grant_tx(&mut tx, command.support_access_grant_id).await?;
    if current.revision != command.expected_revision {
        return Err(revision_conflict());
    }
    current
        .status
        .require_transition(SupportAccessStatus::Active)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    if current.requested_by == context.actor_id {
        return Err(AppError::forbidden());
    }
    let approved_at = now_iso();
    if current.expires_at <= approved_at {
        return Err(AppError::conflict("support access request has expired"));
    }
    let ordinary_membership: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM tenant_memberships
        WHERE tenant_id=$1 AND user_id=$2 AND deleted IS NULL AND NOT support_managed)"#,
    )
    .bind(current.tenant_id.get())
    .bind(current.requested_by.get())
    .fetch_one(&mut *tx)
    .await?;
    if ordinary_membership {
        return Err(AppError::conflict(
            "requester already has ordinary tenant access",
        ));
    }
    sqlx::query(
        r#"INSERT INTO tenant_memberships
        (tenant_id,user_id,created,is_default,all_facilities,
         all_inventory_owners,support_managed,deleted)
        VALUES($1,$2,$3,FALSE,FALSE,FALSE,TRUE,NULL)
        ON CONFLICT(tenant_id,user_id) DO UPDATE SET deleted=NULL,is_default=FALSE,
          all_facilities=FALSE,all_inventory_owners=FALSE,support_managed=TRUE"#,
    )
    .bind(current.tenant_id.get())
    .bind(current.requested_by.get())
    .bind(approved_at)
    .execute(&mut *tx)
    .await?;
    let revision = current
        .revision
        .checked_next()
        .ok_or_else(|| invalid("support access revision overflow"))?;
    sqlx::query(
        r#"UPDATE support_access_grants SET status='active',revision=$2,
        approved_at=$3,approved_by_user_id=$4 WHERE id=$1"#,
    )
    .bind(current.id.get())
    .bind(revision.get())
    .bind(approved_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let approved = LockedGrant {
        status: SupportAccessStatus::Active,
        revision,
        ..current
    };
    let evidence = serde_json::json!({
        "support_access_grant_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "revision": revision.get(),
        "status": "active",
        "requested_by_user_id": current.requested_by.get(),
        "approved_by_user_id": context.actor_id.get(),
        "approved_at": approved_at,
        "expires_at": current.expires_at,
    });
    record_event_tx(
        &mut tx,
        approved,
        EventRecord {
            action: "approved",
            actor_id: context.actor_id,
            occurred_at: approved_at,
            reason: None,
            request_id: &context.request_id,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, current.id).await?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(&mut tx, actor_access.tenant_id)
        .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn reject(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &RejectSupportAccessCommand,
) -> AppResult<RejectSupportAccessResult> {
    let prepared = PreparedCommand::new_v1(context, REJECT_SUPPORT_ACCESS_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let (tenant_id, requested_by) =
        grant_identity_tx(&mut tx, command.support_access_grant_id).await?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(&mut tx, tenant_id).await?;
    super::super::access::lock_user_tx(&mut tx, tenant_id, requested_by.get()).await?;
    let current = lock_grant_tx(&mut tx, command.support_access_grant_id).await?;
    if current.revision != command.expected_revision {
        return Err(revision_conflict());
    }
    current
        .status
        .require_transition(SupportAccessStatus::Rejected)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let rejected_at = now_iso();
    let revision = current
        .revision
        .checked_next()
        .ok_or_else(|| invalid("support access revision overflow"))?;
    sqlx::query(
        r#"UPDATE support_access_grants SET status='rejected',revision=$2,
        rejected_at=$3,rejected_by_user_id=$4,rejection_reason=$5 WHERE id=$1"#,
    )
    .bind(current.id.get())
    .bind(revision.get())
    .bind(rejected_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .execute(&mut *tx)
    .await?;
    let rejected = LockedGrant {
        status: SupportAccessStatus::Rejected,
        revision,
        ..current
    };
    let evidence = serde_json::json!({
        "support_access_grant_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "revision": revision.get(),
        "status": "rejected",
        "rejected_by_user_id": context.actor_id.get(),
        "rejected_at": rejected_at,
        "reason": command.reason.as_str(),
    });
    record_event_tx(
        &mut tx,
        rejected,
        EventRecord {
            action: "rejected",
            actor_id: context.actor_id,
            occurred_at: rejected_at,
            reason: Some(command.reason.as_str()),
            request_id: &context.request_id,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, current.id).await?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(&mut tx, actor_access.tenant_id)
        .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn revoke(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &RevokeSupportAccessCommand,
) -> AppResult<RevokeSupportAccessResult> {
    let prepared = PreparedCommand::new_v1(context, REVOKE_SUPPORT_ACCESS_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let (tenant_id, requested_by) =
        grant_identity_tx(&mut tx, command.support_access_grant_id).await?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(&mut tx, tenant_id).await?;
    super::super::access::lock_user_tx(&mut tx, tenant_id, requested_by.get()).await?;
    let current = lock_grant_tx(&mut tx, command.support_access_grant_id).await?;
    if current.revision != command.expected_revision {
        return Err(revision_conflict());
    }
    current
        .status
        .require_transition(SupportAccessStatus::Revoked)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let revoked_at = now_iso();
    let revision = current
        .revision
        .checked_next()
        .ok_or_else(|| invalid("support access revision overflow"))?;
    sqlx::query(
        r#"UPDATE support_access_grants SET status='revoked',revision=$2,
        revoked_at=$3,revoked_by_user_id=$4,revocation_reason=$5 WHERE id=$1"#,
    )
    .bind(current.id.get())
    .bind(revision.get())
    .bind(revoked_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE tenant_memberships SET deleted=$3
        WHERE tenant_id=$1 AND user_id=$2 AND support_managed AND deleted IS NULL"#,
    )
    .bind(current.tenant_id.get())
    .bind(current.requested_by.get())
    .bind(revoked_at)
    .execute(&mut *tx)
    .await?;
    let revoked = LockedGrant {
        status: SupportAccessStatus::Revoked,
        revision,
        ..current
    };
    let evidence = serde_json::json!({
        "support_access_grant_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "revision": revision.get(),
        "status": "revoked",
        "revoked_by_user_id": context.actor_id.get(),
        "revoked_at": revoked_at,
        "reason": command.reason.as_str(),
    });
    record_event_tx(
        &mut tx,
        revoked,
        EventRecord {
            action: "revoked",
            actor_id: context.actor_id,
            occurred_at: revoked_at,
            reason: Some(command.reason.as_str()),
            request_id: &context.request_id,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, current.id).await?;
    super::super::tenant_lifecycle::bind_platform_tenant_tx(&mut tx, actor_access.tenant_id)
        .await?;
    Ok(prepared.commit(tx, result).await?)
}
