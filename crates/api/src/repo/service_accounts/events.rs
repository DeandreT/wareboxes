use wareboxes_domain::{ServiceAccountId, TenantId, Timestamp, UserId};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::AppResult;
use crate::repo::orders::next_outbox_sequence_tx;

pub(super) struct ServiceAccountEvent<'a> {
    pub tenant_id: TenantId,
    pub service_account_id: ServiceAccountId,
    pub credential_id: Option<i64>,
    pub action: &'a str,
    pub revision: i64,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub evidence: &'a serde_json::Value,
}

pub(super) async fn record_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: &ServiceAccountEvent<'_>,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO service_account_events
        (tenant_id,service_account_id,credential_id,action,account_revision,
         actor_user_id,occurred_at,evidence)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8)"#,
    )
    .bind(event.tenant_id.get())
    .bind(event.service_account_id.get())
    .bind(event.credential_id)
    .bind(event.action)
    .bind(event.revision)
    .bind(event.actor_id.get())
    .bind(event.occurred_at)
    .bind(event.evidence)
    .execute(&mut **tx)
    .await?;

    let event_key = format!(
        "service_account:{}:{}:{}",
        event.service_account_id.get(),
        event.revision,
        event.action
    );
    let ordering_key = format!("service_account:{}", event.service_account_id.get());
    let aggregate_id = event.service_account_id.get().to_string();
    let event_type = format!("identity.service_account.{}", event.action);
    let sequence = next_outbox_sequence_tx(tx, event.tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: Some(event.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "service_account",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: event.evidence,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}
