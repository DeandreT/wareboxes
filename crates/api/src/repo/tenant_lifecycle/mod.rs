//! Platform-administered tenant provisioning and lifecycle transitions.

mod events;
mod provisioning;
mod query;

pub use query::{by_id, event_page, page};

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::tenant_lifecycle::{
    ChangeTenantStatusCommand, ChangeTenantStatusResult, CreateTenantCommand, CreateTenantResult,
    CHANGE_TENANT_STATUS_OPERATION, CREATE_TENANT_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{TenantStatus, UserId};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::lock_current_scope_tx;

pub async fn is_platform_administrator(db: &Db, user_id: i64) -> AppResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT platform_actor_is_administrator($1)")
            .bind(user_id)
            .fetch_one(db)
            .await?,
    )
}

async fn authorize_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_access: &TenantAccess,
    actor_id: UserId,
) -> AppResult<()> {
    lock_current_scope_tx(tx, actor_access.tenant_id, actor_id.get()).await?;
    let authorized: bool = sqlx::query_scalar("SELECT platform_actor_is_administrator($1)")
        .bind(actor_id.get())
        .fetch_one(&mut **tx)
        .await?;
    if !authorized {
        return Err(AppError::forbidden());
    }
    sqlx::query("SELECT set_config('wareboxes.platform_actor_user_id',$1,TRUE)")
        .bind(actor_id.get().to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,TRUE)")
        .bind(actor_id.get().to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn bind_platform_tenant_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
) -> AppResult<()> {
    let actor_id: Option<i64> = sqlx::query_scalar(
        "SELECT NULLIF(current_setting('wareboxes.platform_actor_user_id',TRUE),'')::BIGINT",
    )
    .fetch_one(&mut **tx)
    .await?;
    let authorized: bool = sqlx::query_scalar("SELECT platform_actor_is_administrator($1)")
        .bind(actor_id)
        .fetch_one(&mut **tx)
        .await?;
    if !authorized {
        return Err(AppError::forbidden());
    }
    sqlx::query("SELECT set_config('wareboxes.tenant_id',$1,TRUE)")
        .bind(tenant_id.get().to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn revision_conflict() -> AppError {
    AppError::conflict("tenant revision does not match expected revision")
}

pub async fn create(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &CreateTenantCommand,
) -> AppResult<CreateTenantResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_TENANT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context.actor_id).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("platform-tenant-create:{}", command.slug.as_str()))
        .execute(&mut *tx)
        .await?;
    let slug_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenants WHERE slug=$1)")
            .bind(command.slug.as_str())
            .fetch_one(&mut *tx)
            .await?;
    if slug_exists {
        return Err(AppError::conflict("tenant slug already exists"));
    }
    let administrator = sqlx::query(
        r#"SELECT subject.id,subject.email FROM users subject
        JOIN user_credentials credential ON credential.user_id=subject.id
        WHERE lower(subject.email)=lower($1) AND subject.deleted IS NULL
          AND NOT EXISTS(SELECT 1 FROM service_accounts account
            WHERE account.principal_user_id=subject.id)"#,
    )
    .bind(&command.administrator_email)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("interactive administrator"))?;
    let administrator_id: i64 = administrator.try_get("id")?;
    let administrator_email: String = administrator.try_get("email")?;
    let occurred_at = now_iso();
    let tenant_id = wareboxes_domain::TenantId::new(
        sqlx::query_scalar(
            r#"INSERT INTO tenants
            (created,slug,name,status,revision,created_by_user_id,initial_admin_user_id)
            VALUES($1,$2,$3,'active',1,$4,$5) RETURNING id"#,
        )
        .bind(occurred_at)
        .bind(command.slug.as_str())
        .bind(command.name.as_str())
        .bind(context.actor_id.get())
        .bind(administrator_id)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    bind_platform_tenant_tx(&mut tx, tenant_id).await?;
    provisioning::provision_initial_administrator_tx(
        &mut tx,
        tenant_id,
        administrator_id,
        &administrator_email,
        occurred_at,
    )
    .await?;
    let evidence = serde_json::json!({
        "tenant_id": tenant_id.get(),
        "slug": command.slug.as_str(),
        "name": command.name.as_str(),
        "status": "active",
        "revision": 1,
        "initial_administrator_user_id": administrator_id,
        "initial_administrator_email": administrator_email,
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    events::record_tx(
        &mut tx,
        &events::TenantEvent {
            tenant_id,
            action: "created",
            previous_status: None,
            resulting_status: TenantStatus::Active,
            revision: 1,
            actor_id: context.actor_id,
            occurred_at,
            reason: None,
            revoked_session_count: 0,
            revoked_credential_count: 0,
            request_id: Some(&context.request_id),
            evidence: &evidence,
        },
    )
    .await?;
    let result = query::read_tx(&mut tx, tenant_id).await?;
    bind_platform_tenant_tx(&mut tx, actor_access.tenant_id).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn change_status(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &ChangeTenantStatusCommand,
) -> AppResult<ChangeTenantStatusResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CHANGE_TENANT_STATUS_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context.actor_id).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    if command.status == TenantStatus::Suspended && command.tenant_id == actor_access.tenant_id {
        return Err(AppError::bad_request(
            "switch to another active tenant before suspending the current tenant",
        ));
    }
    let row = sqlx::query(
        "SELECT status,revision FROM tenants WHERE id=$1 AND deleted IS NULL FOR UPDATE",
    )
    .bind(command.tenant_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("tenant"))?;
    let current_status = TenantStatus::parse(&row.try_get::<String, _>("status")?)
        .ok_or_else(|| AppError::internal("stored tenant status is invalid"))?;
    let current_revision: i64 = row.try_get("revision")?;
    if current_revision != command.expected_revision.get() {
        return Err(revision_conflict());
    }
    current_status
        .require_transition(command.status)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let revision = command
        .expected_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("tenant revision overflow"))?;
    let occurred_at = now_iso();
    sqlx::query(
        r#"UPDATE tenants SET status=$2,revision=$3,modified_at=$4,
        status_changed_at=$4,status_changed_by_user_id=$5,status_reason=$6 WHERE id=$1"#,
    )
    .bind(command.tenant_id.get())
    .bind(command.status.as_str())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .execute(&mut *tx)
    .await?;
    bind_platform_tenant_tx(&mut tx, command.tenant_id).await?;
    let (revoked_session_count, revoked_credential_count) = if command.status
        == TenantStatus::Suspended
    {
        let sessions: i64 = sqlx::query_scalar("SELECT revoke_suspended_tenant_sessions($1,$2)")
            .bind(command.tenant_id.get())
            .bind(context.actor_id.get())
            .fetch_one(&mut *tx)
            .await?;
        let credentials = events::revoke_credentials_for_suspension_tx(
            &mut tx,
            command.tenant_id,
            context.actor_id,
            occurred_at,
            command.reason.as_str(),
        )
        .await?;
        (sessions, credentials)
    } else {
        (0, 0)
    };
    let action = if command.status == TenantStatus::Suspended {
        "suspended"
    } else {
        "reactivated"
    };
    let evidence = serde_json::json!({
        "tenant_id": command.tenant_id.get(),
        "previous_status": current_status.as_str(),
        "resulting_status": command.status.as_str(),
        "revision": revision.get(),
        "reason": command.reason.as_str(),
        "revoked_session_count": revoked_session_count,
        "revoked_credential_count": revoked_credential_count,
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    events::record_tx(
        &mut tx,
        &events::TenantEvent {
            tenant_id: command.tenant_id,
            action,
            previous_status: Some(current_status),
            resulting_status: command.status,
            revision: revision.get(),
            actor_id: context.actor_id,
            occurred_at,
            reason: Some(command.reason.as_str()),
            revoked_session_count,
            revoked_credential_count,
            request_id: Some(&context.request_id),
            evidence: &evidence,
        },
    )
    .await?;
    let result = query::read_tx(&mut tx, command.tenant_id).await?;
    bind_platform_tenant_tx(&mut tx, actor_access.tenant_id).await?;
    Ok(prepared.commit(tx, result).await?)
}
