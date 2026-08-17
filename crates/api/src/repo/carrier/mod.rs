mod accounts;
mod jobs;
mod mapping;

pub use accounts::{
    change_status, create, list, reconfigure, CarrierAccountPage, CarrierAccountPageFilter,
};
pub use jobs::{
    cancel, get_job, list_jobs, queue, retry, CarrierManifestJobPage, CarrierManifestJobPageFilter,
};

use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, Timestamp};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::AppResult;
use crate::repo::orders::next_outbox_sequence_tx;

pub(crate) async fn bind_actor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_user_id: i64,
) -> AppResult<()> {
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(actor_user_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) struct CarrierEvent<'a> {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub actor_user_id: i64,
    pub aggregate_type: &'a str,
    pub aggregate_id: String,
    pub event_type: &'a str,
    pub event_key: String,
    pub payload: &'a serde_json::Value,
    pub occurred_at: Timestamp,
}

pub(crate) async fn insert_outbox_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: CarrierEvent<'_>,
) -> AppResult<()> {
    let ordering_key = format!("{}:{}", event.aggregate_type, event.aggregate_id);
    let aggregate_sequence = next_outbox_sequence_tx(tx, event.tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: Some(event.inventory_owner_id),
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
