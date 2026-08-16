use serde_json::Value;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{SupportAccessGrantId, TenantId, Timestamp, UserId};

use crate::error::AppResult;

pub(super) struct SupportAccessEvent<'a> {
    pub support_access_grant_id: SupportAccessGrantId,
    pub tenant_id: TenantId,
    pub action: &'a str,
    pub revision: i64,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub reason: Option<&'a str>,
    pub request_id: &'a str,
    pub evidence: &'a Value,
}

pub(super) async fn record_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: &SupportAccessEvent<'_>,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO support_access_events
        (support_access_grant_id,tenant_id,action,grant_revision,actor_user_id,
         occurred_at,reason,request_id,evidence)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
    )
    .bind(event.support_access_grant_id.get())
    .bind(event.tenant_id.get())
    .bind(event.action)
    .bind(event.revision)
    .bind(event.actor_id.get())
    .bind(event.occurred_at)
    .bind(event.reason)
    .bind(event.request_id)
    .bind(event.evidence)
    .execute(&mut **tx)
    .await?;

    let aggregate_id = event.support_access_grant_id.get().to_string();
    let ordering_key = format!("support-access:{}", event.support_access_grant_id.get());
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
            aggregate_type: "support_access",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: match event.action {
                "requested" => "support_access.requested.v1",
                "approved" => "support_access.approved.v1",
                "rejected" => "support_access.rejected.v1",
                "revoked" => "support_access.revoked.v1",
                _ => "support_access.unknown.v1",
            },
            schema_version: 1,
            payload: event.evidence,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}
