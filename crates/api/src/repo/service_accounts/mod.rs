//! Tenant-administered, non-human integration identities.

mod access;
mod events;
mod query;

pub use query::{by_id, event_page, page, permission_options};

use rand::distributions::Alphanumeric;
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::service_account::{
    ChangeServiceAccountStatusCommand, ChangeServiceAccountStatusResult,
    CreateServiceAccountCommand, CreateServiceAccountResult, IssueServiceAccountCredentialCommand,
    IssuedServiceAccountCredential, RevokeServiceAccountCredentialCommand,
    RevokeServiceAccountCredentialResult, UpdateServiceAccountAccessCommand,
    UpdateServiceAccountAccessResult, CHANGE_SERVICE_ACCOUNT_STATUS_OPERATION,
    CREATE_SERVICE_ACCOUNT_OPERATION, ISSUE_SERVICE_ACCOUNT_CREDENTIAL_OPERATION,
    REVOKE_SERVICE_ACCOUNT_CREDENTIAL_OPERATION, UPDATE_SERVICE_ACCOUNT_ACCESS_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{ServiceAccountCredentialId, ServiceAccountId, ServiceAccountStatus};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

const ADMIN_PERMISSION: &str = "admin";

fn conflict_revision() -> AppError {
    AppError::conflict("service account revision does not match expected revision")
}

async fn authorize_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    actor_id: i64,
) -> AppResult<()> {
    lock_current_scope_tx(tx, access.tenant_id, actor_id).await?;
    require_permission_tx(tx, access.tenant_id, actor_id, ADMIN_PERMISSION).await?;
    let actor_id = wareboxes_domain::UserId::new(actor_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    self::access::bind_actor_tx(tx, actor_id).await
}

fn new_subject_email() -> String {
    let identity = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("service-account-{identity}@identity.wareboxes.invalid")
}

pub async fn create(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &CreateServiceAccountCommand,
) -> AppResult<CreateServiceAccountResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    if command.tenant_id != actor_access.tenant_id {
        return Err(AppError::not_found("service account"));
    }
    command
        .access
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let prepared = PreparedCommand::new_v1(context, CREATE_SERVICE_ACCOUNT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context.actor_id.get()).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    self::access::validate_access_tx(&mut tx, actor_access, &command.access).await?;
    let occurred_at = now_iso();
    let principal_user_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO users(created,email,nick_name)
        VALUES($1,$2,$3) RETURNING id"#,
    )
    .bind(occurred_at)
    .bind(new_subject_email())
    .bind(command.name.as_str())
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO tenant_memberships
        (tenant_id,user_id,created,is_default,all_facilities,all_inventory_owners)
        VALUES($1,$2,$3,FALSE,FALSE,FALSE)"#,
    )
    .bind(actor_access.tenant_id.get())
    .bind(principal_user_id)
    .bind(occurred_at)
    .execute(&mut *tx)
    .await?;
    let service_account_id = ServiceAccountId::new(
        sqlx::query_scalar(
            r#"INSERT INTO service_accounts
            (tenant_id,principal_user_id,name,description,status,revision,
             all_facilities,all_inventory_owners,created_at,created_by_user_id,
             updated_at,updated_by_user_id)
            VALUES($1,$2,$3,$4,'active',1,$5,$6,$7,$8,$7,$8) RETURNING id"#,
        )
        .bind(actor_access.tenant_id.get())
        .bind(principal_user_id)
        .bind(command.name.as_str())
        .bind(command.description.as_ref().map(|value| value.as_str()))
        .bind(command.access.all_facilities)
        .bind(command.access.all_inventory_owners)
        .bind(occurred_at)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    self::access::replace_access_tx(
        &mut tx,
        actor_access.tenant_id,
        service_account_id,
        context.actor_id,
        occurred_at,
        &command.access,
    )
    .await?;
    let evidence = serde_json::json!({
        "service_account_id": service_account_id.get(),
        "name": command.name.as_str(),
        "status": "active",
        "revision": 1,
        "access": command.access,
        "occurred_at": occurred_at,
    });
    events::record_event_tx(
        &mut tx,
        &events::ServiceAccountEvent {
            tenant_id: actor_access.tenant_id,
            service_account_id,
            credential_id: None,
            action: "created",
            revision: 1,
            actor_id: context.actor_id,
            occurred_at,
            evidence: &evidence,
        },
    )
    .await?;
    let result = query::read_tx(&mut tx, actor_access.tenant_id, service_account_id).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn update_access(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &UpdateServiceAccountAccessCommand,
) -> AppResult<UpdateServiceAccountAccessResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    command
        .access
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let prepared =
        PreparedCommand::new_v1(context, UPDATE_SERVICE_ACCOUNT_ACCESS_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context.actor_id.get()).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    self::access::validate_access_tx(&mut tx, actor_access, &command.access).await?;
    let row = sqlx::query(
        "SELECT revision FROM service_accounts WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("service account"))?;
    let current_revision: i64 = row.try_get("revision")?;
    if current_revision != command.expected_revision.get() {
        return Err(conflict_revision());
    }
    let revision = command
        .expected_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("service account revision overflow"))?;
    let occurred_at = now_iso();
    self::access::replace_access_tx(
        &mut tx,
        actor_access.tenant_id,
        command.service_account_id,
        context.actor_id,
        occurred_at,
        &command.access,
    )
    .await?;
    sqlx::query(
        r#"UPDATE service_accounts SET all_facilities=$3,all_inventory_owners=$4,
        revision=$5,updated_at=$6,updated_by_user_id=$7 WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .bind(command.access.all_facilities)
    .bind(command.access.all_inventory_owners)
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let evidence = serde_json::json!({
        "service_account_id": command.service_account_id.get(),
        "revision": revision.get(), "access": command.access, "occurred_at": occurred_at,
    });
    events::record_event_tx(
        &mut tx,
        &events::ServiceAccountEvent {
            tenant_id: actor_access.tenant_id,
            service_account_id: command.service_account_id,
            credential_id: None,
            action: "access_updated",
            revision: revision.get(),
            actor_id: context.actor_id,
            occurred_at,
            evidence: &evidence,
        },
    )
    .await?;
    let result =
        query::read_tx(&mut tx, actor_access.tenant_id, command.service_account_id).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn change_status(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &ChangeServiceAccountStatusCommand,
) -> AppResult<ChangeServiceAccountStatusResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, CHANGE_SERVICE_ACCOUNT_STATUS_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context.actor_id.get()).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        "SELECT status,revision FROM service_accounts WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("service account"))?;
    let current_status: String = row.try_get("status")?;
    let current_revision: i64 = row.try_get("revision")?;
    if current_revision != command.expected_revision.get() {
        return Err(conflict_revision());
    }
    if current_status == command.status.as_str() {
        return Err(AppError::conflict(
            "service account already has requested status",
        ));
    }
    let revision = command
        .expected_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("service account revision overflow"))?;
    let occurred_at = now_iso();
    let mut revoked_credential_count = 0;
    let action = match command.status {
        ServiceAccountStatus::Disabled => {
            revoked_credential_count = sqlx::query(
                r#"UPDATE service_account_credentials SET revoked_at=$3,
                revoked_by_user_id=$4,revocation_reason=$5
                WHERE tenant_id=$1 AND service_account_id=$2 AND revoked_at IS NULL"#,
            )
            .bind(actor_access.tenant_id.get())
            .bind(command.service_account_id.get())
            .bind(occurred_at)
            .bind(context.actor_id.get())
            .bind(command.reason.as_str())
            .execute(&mut *tx)
            .await?
            .rows_affected();
            sqlx::query(
                r#"UPDATE service_accounts SET status='disabled',revision=$3,updated_at=$4,
                updated_by_user_id=$5,disabled_at=$4,disabled_by_user_id=$5,disabled_reason=$6
                WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(actor_access.tenant_id.get())
            .bind(command.service_account_id.get())
            .bind(revision.get())
            .bind(occurred_at)
            .bind(context.actor_id.get())
            .bind(command.reason.as_str())
            .execute(&mut *tx)
            .await?;
            "disabled"
        }
        ServiceAccountStatus::Active => {
            sqlx::query(
                r#"UPDATE service_accounts SET status='active',revision=$3,updated_at=$4,
                updated_by_user_id=$5,disabled_at=NULL,disabled_by_user_id=NULL,
                disabled_reason=NULL WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(actor_access.tenant_id.get())
            .bind(command.service_account_id.get())
            .bind(revision.get())
            .bind(occurred_at)
            .bind(context.actor_id.get())
            .execute(&mut *tx)
            .await?;
            "enabled"
        }
    };
    let evidence = serde_json::json!({
        "service_account_id": command.service_account_id.get(), "revision": revision.get(),
        "status": command.status.as_str(), "reason": command.reason.as_str(),
        "revoked_credential_count": revoked_credential_count,
        "occurred_at": occurred_at,
    });
    events::record_event_tx(
        &mut tx,
        &events::ServiceAccountEvent {
            tenant_id: actor_access.tenant_id,
            service_account_id: command.service_account_id,
            credential_id: None,
            action,
            revision: revision.get(),
            actor_id: context.actor_id,
            occurred_at,
            evidence: &evidence,
        },
    )
    .await?;
    let result =
        query::read_tx(&mut tx, actor_access.tenant_id, command.service_account_id).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn issue_credential(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &IssueServiceAccountCredentialCommand,
) -> AppResult<IssuedServiceAccountCredential> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, ISSUE_SERVICE_ACCOUNT_CREDENTIAL_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context.actor_id.get()).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        "SELECT status,revision FROM service_accounts WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("service account"))?;
    let status: String = row.try_get("status")?;
    if status != ServiceAccountStatus::Active.as_str() {
        return Err(AppError::conflict("service account is disabled"));
    }
    let current_revision: i64 = row.try_get("revision")?;
    if current_revision != command.expected_revision.get() {
        return Err(conflict_revision());
    }
    let occurred_at = now_iso();
    if command
        .expires_at
        .is_some_and(|expires| expires <= occurred_at)
    {
        return Err(AppError::bad_request(
            "credential expiry must be in the future",
        ));
    }
    let revision = command
        .expected_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("service account revision overflow"))?;
    let token_prefix = command.bearer_token.prefix().to_owned();
    let token_hash = hex::encode(Sha256::digest(command.bearer_token.as_str().as_bytes()));
    let credential_id = ServiceAccountCredentialId::new(
        sqlx::query_scalar(
            r#"INSERT INTO service_account_credentials
            (tenant_id,service_account_id,label,token_prefix,token_hash,created_at,
             created_by_user_id,expires_at)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id"#,
        )
        .bind(actor_access.tenant_id.get())
        .bind(command.service_account_id.get())
        .bind(command.label.as_str())
        .bind(&token_prefix)
        .bind(token_hash)
        .bind(occurred_at)
        .bind(context.actor_id.get())
        .bind(command.expires_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    sqlx::query(
        r#"UPDATE service_accounts SET revision=$3,updated_at=$4,updated_by_user_id=$5
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let evidence = serde_json::json!({
        "service_account_id": command.service_account_id.get(),
        "credential_id": credential_id.get(), "token_prefix": token_prefix,
        "label": command.label.as_str(), "expires_at": command.expires_at,
        "revision": revision.get(), "occurred_at": occurred_at,
    });
    events::record_event_tx(
        &mut tx,
        &events::ServiceAccountEvent {
            tenant_id: actor_access.tenant_id,
            service_account_id: command.service_account_id,
            credential_id: Some(credential_id.get()),
            action: "credential_issued",
            revision: revision.get(),
            actor_id: context.actor_id,
            occurred_at,
            evidence: &evidence,
        },
    )
    .await?;
    let service_account =
        query::read_tx(&mut tx, actor_access.tenant_id, command.service_account_id).await?;
    let credential = service_account
        .credentials
        .iter()
        .find(|credential| credential.credential_id == credential_id)
        .cloned()
        .ok_or_else(|| AppError::internal("issued service account credential is missing"))?;
    let result = IssuedServiceAccountCredential {
        service_account,
        credential,
    };
    Ok(prepared.commit(tx, result).await?)
}

pub async fn revoke_credential(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &RevokeServiceAccountCredentialCommand,
) -> AppResult<RevokeServiceAccountCredentialResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        REVOKE_SERVICE_ACCOUNT_CREDENTIAL_OPERATION,
        command,
    )?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context.actor_id.get()).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM service_accounts WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("service account"))?;
    if revision != command.expected_revision.get() {
        return Err(conflict_revision());
    }
    let credential_row = sqlx::query(
        r#"SELECT revoked_at FROM service_account_credentials
        WHERE tenant_id=$1 AND service_account_id=$2 AND id=$3 FOR UPDATE"#,
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .bind(command.credential_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("service account credential"))?;
    let credential_revoked: Option<wareboxes_domain::Timestamp> =
        credential_row.try_get("revoked_at")?;
    if credential_revoked.is_some() {
        return Err(AppError::conflict(
            "service account credential is already revoked",
        ));
    }
    let next_revision = command
        .expected_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("service account revision overflow"))?;
    let occurred_at = now_iso();
    sqlx::query(
        r#"UPDATE service_account_credentials SET revoked_at=$4,revoked_by_user_id=$5,
        revocation_reason=$6 WHERE tenant_id=$1 AND service_account_id=$2 AND id=$3"#,
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .bind(command.credential_id.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE service_accounts SET revision=$3,updated_at=$4,updated_by_user_id=$5
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(actor_access.tenant_id.get())
    .bind(command.service_account_id.get())
    .bind(next_revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let evidence = serde_json::json!({
        "service_account_id": command.service_account_id.get(),
        "credential_id": command.credential_id.get(), "revision": next_revision.get(),
        "reason": command.reason.as_str(), "occurred_at": occurred_at,
    });
    events::record_event_tx(
        &mut tx,
        &events::ServiceAccountEvent {
            tenant_id: actor_access.tenant_id,
            service_account_id: command.service_account_id,
            credential_id: Some(command.credential_id.get()),
            action: "credential_revoked",
            revision: next_revision.get(),
            actor_id: context.actor_id,
            occurred_at,
            evidence: &evidence,
        },
    )
    .await?;
    let result =
        query::read_tx(&mut tx, actor_access.tenant_id, command.service_account_id).await?;
    Ok(prepared.commit(tx, result).await?)
}
