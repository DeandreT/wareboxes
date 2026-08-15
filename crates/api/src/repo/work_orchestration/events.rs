use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, Timestamp, UserId};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::AppResult;
use crate::repo::orders::next_outbox_sequence_tx;

pub(super) struct OrchestrationEvent<'a> {
    pub(super) inventory_owner_id: Option<InventoryOwnerId>,
    pub(super) facility_id: FacilityId,
    pub(super) actor_id: UserId,
    pub(super) aggregate_type: &'a str,
    pub(super) aggregate_id: i64,
    pub(super) ordering_key: String,
    pub(super) transition: &'a str,
    pub(super) occurred_at: Timestamp,
    pub(super) payload: &'a serde_json::Value,
}

pub(super) async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    event: OrchestrationEvent<'_>,
) -> AppResult<()> {
    let event_key = format!(
        "work_orchestration_{}:{}:{}",
        event.aggregate_type, event.aggregate_id, event.transition
    );
    let event_type = format!(
        "optimization.work_orchestration.{}.{}",
        event.aggregate_type, event.transition
    );
    let aggregate_id = event.aggregate_id.to_string();
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &event.ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: event.inventory_owner_id,
            facility_id: Some(event.facility_id),
            actor_user_id: Some(event.actor_id.get()),
            event_key: &event_key,
            aggregate_type: event.aggregate_type,
            aggregate_id: &aggregate_id,
            ordering_key: &event.ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: event.payload,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}
