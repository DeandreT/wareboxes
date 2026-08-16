use wareboxes_domain::{AutomationCommandId, FacilityId, TenantId, Timestamp};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::AppResult;
use crate::repo::orders::next_outbox_sequence_tx;

pub(crate) struct AutomationEvent<'a> {
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub actor_user_id: i64,
    pub aggregate_type: &'a str,
    pub aggregate_id: String,
    pub event_type: &'a str,
    pub event_key: String,
    pub payload: &'a serde_json::Value,
    pub occurred_at: Timestamp,
}

pub(crate) struct CommandHistoryEvent<'a> {
    pub tenant_id: TenantId,
    pub command_id: AutomationCommandId,
    pub transition: &'a str,
    pub actor_user_id: i64,
    pub service_account_id: Option<i64>,
    pub occurred_at: Timestamp,
    pub evidence: serde_json::Value,
}

pub(crate) async fn insert_command_history_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: CommandHistoryEvent<'_>,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO automation_command_events
        (tenant_id,command_id,transition,actor_user_id,service_account_id,occurred_at,evidence)
        VALUES($1,$2,$3,$4,$5,$6,$7)"#,
    )
    .bind(event.tenant_id.get())
    .bind(event.command_id.get())
    .bind(event.transition)
    .bind(event.actor_user_id)
    .bind(event.service_account_id)
    .bind(event.occurred_at)
    .bind(event.evidence)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn insert_outbox_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: AutomationEvent<'_>,
) -> AppResult<()> {
    let ordering_key = format!("{}:{}", event.aggregate_type, event.aggregate_id);
    let aggregate_sequence = next_outbox_sequence_tx(tx, event.tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: None,
            facility_id: Some(event.facility_id),
            actor_user_id: Some(event.actor_user_id),
            event_key: &event.event_key,
            aggregate_type: event.aggregate_type,
            aggregate_id: &event.aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: event.event_type,
            schema_version: 1,
            payload: event.payload,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}
