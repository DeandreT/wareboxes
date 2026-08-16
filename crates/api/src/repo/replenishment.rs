//! Versioned replenishment policy, planning, execution, and read models.

mod cancellation;
mod claim;
mod confirmation;
mod decision_policy;
mod planning;
mod policy;
mod read_model;

pub use cancellation::*;
pub use claim::*;
pub use confirmation::*;
pub use planning::*;
pub use policy::*;
pub use read_model::*;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, LocationId, ReplenishmentLevel,
    ReplenishmentPolicyDefinition, ReplenishmentPolicyId, ReplenishmentPolicyRevision,
    ReplenishmentPolicyScope, ReplenishmentPolicyThresholds, ReplenishmentReserveSourceLocationIds,
    ReplenishmentScanValue, ReplenishmentUom, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;

#[derive(Debug, Clone)]
struct PolicyRow {
    id: ReplenishmentPolicyId,
    definition: ReplenishmentPolicyDefinition,
    revision: ReplenishmentPolicyRevision,
    effective_to: Option<Timestamp>,
}

impl PolicyRow {
    fn scope(&self) -> &ReplenishmentPolicyScope {
        self.definition.scope()
    }
}

fn policy_from_row(row: &sqlx::postgres::PgRow, source_ids: Vec<i64>) -> AppResult<PolicyRow> {
    let tenant_id = TenantId::new(row.try_get("tenant_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let item_id = CatalogItemId::new(row.try_get("item_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let pick_face_location_id = LocationId::new(row.try_get("pick_face_location_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let source_ids = source_ids
        .into_iter()
        .map(|id| LocationId::new(id).map_err(|error| AppError::internal(error.to_string())))
        .collect::<AppResult<Vec<_>>>()?;
    let definition = ReplenishmentPolicyDefinition::new(
        ReplenishmentPolicyScope {
            tenant_id,
            inventory_owner_id,
            facility_id,
            item_id,
            uom: ReplenishmentUom::new(row.try_get::<String, _>("uom")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            pick_face_location_id,
        },
        ReplenishmentPolicyThresholds::new(
            level(row.try_get("minimum_qty")?)?,
            level(row.try_get("target_qty")?)?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        ReplenishmentReserveSourceLocationIds::new(source_ids)
            .map_err(|error| AppError::internal(error.to_string()))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(PolicyRow {
        id: ReplenishmentPolicyId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        definition,
        revision: ReplenishmentPolicyRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        effective_to: row.try_get("effective_to")?,
    })
}

fn level(value: i64) -> AppResult<ReplenishmentLevel> {
    ReplenishmentLevel::new(value).map_err(|error| AppError::internal(error.to_string()))
}

fn scan(value: String, label: &str) -> AppResult<ReplenishmentScanValue> {
    ReplenishmentScanValue::new(value)
        .map_err(|_| AppError::conflict(format!("{label} is not scannable")))
}

fn require_scope(scope: &ScopeBindings, owner_id: i64, facility_id: i64) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found("replenishment resource"))
    }
}

async fn require_stored_policy_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored = sqlx::query(
        r#"
        SELECT (result_json->>'policy_id')::BIGINT AS policy_id
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(stored) = stored else {
        return Ok(());
    };
    let policy_id = stored
        .try_get::<Option<i64>, _>("policy_id")?
        .ok_or_else(|| AppError::internal("stored replenishment policy result is invalid"))?;
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id
        FROM replenishment_policies
        WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(policy_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment policy"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}

async fn require_stored_work_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored = sqlx::query(
        r#"
        SELECT (result_json->>'work_id')::BIGINT AS work_id
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(stored) = stored else {
        return Ok(());
    };
    let Some(work_id) = stored.try_get::<Option<i64>, _>("work_id")? else {
        return Ok(());
    };
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id
        FROM work_tasks
        WHERE tenant_id=$1 AND id=$2 AND task_type='replenishment'
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(work_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment work"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}

async fn policy_sources_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ReplenishmentPolicyId,
) -> AppResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT source_location_id
        FROM replenishment_policy_sources
        WHERE tenant_id = $1 AND policy_id = $2
        ORDER BY source_sequence
        "#,
    )
    .bind(tenant_id.get())
    .bind(policy_id.get())
    .fetch_all(&mut **tx)
    .await?)
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_id: i64,
    aggregate_type: &str,
    aggregate_id: i64,
    event_type: &str,
    event_suffix: &str,
    payload: &serde_json::Value,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let ordering_key = format!("{aggregate_type}:{aggregate_id}");
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!("{ordering_key}:{event_suffix}");
    let aggregate_id = aggregate_id.to_string();
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_id),
            event_key: &event_key,
            aggregate_type,
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}
