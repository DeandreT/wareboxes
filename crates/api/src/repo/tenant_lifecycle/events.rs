use serde_json::Value;
use sqlx::Row;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{TenantId, TenantStatus, Timestamp, UserId};

use crate::error::{AppError, AppResult};

pub(super) struct TenantEvent<'a> {
    pub tenant_id: TenantId,
    pub action: &'a str,
    pub previous_status: Option<TenantStatus>,
    pub resulting_status: TenantStatus,
    pub revision: i64,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub reason: Option<&'a str>,
    pub revoked_session_count: i64,
    pub revoked_credential_count: i64,
    pub request_id: Option<&'a str>,
    pub evidence: &'a Value,
}

pub(super) async fn record_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: &TenantEvent<'_>,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO tenant_lifecycle_events
        (tenant_id,action,previous_status,resulting_status,tenant_revision,
         actor_user_id,occurred_at,reason,revoked_session_count,
         revoked_credential_count,request_id,evidence)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)"#,
    )
    .bind(event.tenant_id.get())
    .bind(event.action)
    .bind(event.previous_status.map(|status| status.as_str()))
    .bind(event.resulting_status.as_str())
    .bind(event.revision)
    .bind(event.actor_id.get())
    .bind(event.occurred_at)
    .bind(event.reason)
    .bind(event.revoked_session_count)
    .bind(event.revoked_credential_count)
    .bind(event.request_id)
    .bind(event.evidence)
    .execute(&mut **tx)
    .await?;

    let aggregate_id = event.tenant_id.get().to_string();
    let ordering_key = format!("tenant:{}", event.tenant_id.get());
    let event_key = format!("{ordering_key}:revision:{}", event.revision);
    let aggregate_sequence: i64 = sqlx::query_scalar(
        r#"SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences
        WHERE tenant_id=$1 AND ordering_key=$2),0)+1"#,
    )
    .bind(event.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    wareboxes_persistence_postgres::outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: None,
            event_key: &event_key,
            aggregate_type: "tenant",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: match event.action {
                "created" => "tenant.created.v1",
                "suspended" => "tenant.suspended.v1",
                "reactivated" => "tenant.reactivated.v1",
                _ => return Err(AppError::internal("unknown tenant lifecycle event action")),
            },
            schema_version: 1,
            payload: event.evidence,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}

pub(super) async fn revoke_credentials_for_suspension_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    occurred_at: Timestamp,
    reason: &str,
) -> AppResult<i64> {
    let revoked = sqlx::query(
        r#"WITH revoked AS (
          UPDATE service_account_credentials credential
          SET revoked_at=$3,revoked_by_user_id=$2,revocation_reason=$4
          WHERE credential.tenant_id=$1 AND credential.revoked_at IS NULL
          RETURNING credential.id,credential.service_account_id
        )
        SELECT revoked.id AS credential_id,revoked.service_account_id,
               account.revision AS account_revision
        FROM revoked JOIN service_accounts account
          ON account.tenant_id=$1 AND account.id=revoked.service_account_id
        ORDER BY revoked.id"#,
    )
    .bind(tenant_id.get())
    .bind(actor_id.get())
    .bind(occurred_at)
    .bind(reason)
    .fetch_all(&mut **tx)
    .await?;
    for row in &revoked {
        let credential_id: i64 = row.try_get("credential_id")?;
        let service_account_id: i64 = row.try_get("service_account_id")?;
        let account_revision: i64 = row.try_get("account_revision")?;
        let evidence = serde_json::json!({
            "source": "tenant_suspension",
            "tenant_id": tenant_id.get(),
            "service_account_id": service_account_id,
            "credential_id": credential_id,
            "account_revision": account_revision,
            "reason": reason,
            "revoked_at": occurred_at,
        });
        sqlx::query(
            r#"INSERT INTO service_account_events
            (tenant_id,service_account_id,credential_id,action,account_revision,
             actor_user_id,occurred_at,evidence)
            VALUES($1,$2,$3,'credential_revoked',$4,$5,$6,$7)"#,
        )
        .bind(tenant_id.get())
        .bind(service_account_id)
        .bind(credential_id)
        .bind(account_revision)
        .bind(actor_id.get())
        .bind(occurred_at)
        .bind(evidence)
        .execute(&mut **tx)
        .await?;
    }
    i64::try_from(revoked.len())
        .map_err(|_| AppError::internal("revoked credential count exceeds i64"))
}
