//! Demand-backed cross-dock planning and execution.

mod cancellation;
mod claim;
mod confirmation;
mod planning;
mod read_model;

pub use cancellation::cancel_work;
pub use claim::{claim_by_id, claim_next, current_claim, heartbeat_claim, release_claim};
pub use confirmation::confirm_work;
pub use planning::plan_work;
pub use read_model::{planning_option_page, work_page};

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, Timestamp};
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;

fn require_scope(scope: &ScopeBindings, owner_id: i64, facility_id: i64) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found("cross-dock work"))
    }
}

async fn require_stored_work_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored = sqlx::query(
        r#"SELECT (result_json->>'work_id')::BIGINT AS work_id
           FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3"#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(stored) = stored else { return Ok(()) };
    let Some(work_id) = stored.try_get::<Option<i64>, _>("work_id")? else {
        return Ok(());
    };
    require_work_visible_tx(tx, prepared.tenant_id(), work_id, scope).await
}

async fn require_work_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM cross_dock_tasks WHERE tenant_id=$1 AND task_id=$2",
    )
    .bind(tenant_id.get())
    .bind(work_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("cross-dock work"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_id: i64,
    order_id: i64,
    event_type: &str,
    event_suffix: &str,
    payload: &serde_json::Value,
    created: Timestamp,
) -> AppResult<()> {
    let ordering_key = format!("order:{order_id}");
    let aggregate_sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!("{ordering_key}:{event_suffix}");
    let aggregate_id = order_id.to_string();
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner_id),
            facility_id: Some(facility_id),
            event_key: &event_key,
            event_type,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            schema_version: 1,
            actor_user_id: Some(actor_id),
            occurred_at: created,
            payload,
        },
    )
    .await?;
    Ok(())
}
