//! Allocation-backed desktop packing sessions and cartons.

mod abandonment;
mod carton;
mod content;
mod policy;
mod queue;
mod read_model;
mod removal;
mod reopening;
mod session;

pub use abandonment::abandon_session;
pub use carton::{close_carton, create_carton, void_carton};
pub use content::pack_picked_allocation;
pub use queue::{packing_queue, PackingQueueCursor, PackingQueueEntry, PackingQueuePage};
pub use read_model::{packing_session, packing_session_for_order};
pub use removal::remove_packed_content;
pub use reopening::reopen_carton_command;
pub use session::open_session;

use sqlx::Row;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::packing::PackSessionReadModel;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, OrderId, OrderRevision, OrderStatus, PackSessionId, TenantId,
};

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;
use wareboxes_persistence_postgres::outbox;

#[derive(Debug, Clone)]
pub(super) struct LockedOrder {
    pub inventory_owner_id: InventoryOwnerId,
    pub status: OrderStatus,
    pub revision: OrderRevision,
}

#[derive(Debug, Clone)]
pub(super) struct LockedSession {
    pub id: PackSessionId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub order_release_id: i64,
    pub packing_location_id: i64,
    pub state: String,
    pub revision: OrderRevision,
    pub expected_allocation_count: i64,
    pub packed_allocation_count: i64,
    pub expected_qty: i64,
    pub packed_qty: i64,
    pub open_carton_count: i64,
    pub closed_carton_count: i64,
    pub pack_policy: wareboxes_application::packing_decision_policy::PackDecisionPolicyReadModel,
}

pub(super) async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, status, revision
        FROM orders
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
          AND ($3 OR inventory_owner_id = ANY($4))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    let status: String = row.try_get("status")?;
    Ok(LockedOrder {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: OrderStatus::parse(&status)
            .ok_or_else(|| AppError::internal("order has an invalid status"))?,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

pub(super) async fn lock_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
    scope: &ScopeBindings,
) -> AppResult<LockedSession> {
    let row = sqlx::query(
        r#"
        SELECT id, order_id, inventory_owner_id, facility_id, order_release_id,
               packing_location_id, state, revision,
               expected_allocation_count, packed_allocation_count,
               expected_qty, packed_qty, open_carton_count, closed_carton_count,
               pack_policy_source, pack_configuration_id, pack_configuration_revision,
               pack_scope_level, pack_inventory_owner_id, pack_facility_id,
               require_station_scan, require_weight, allow_mixed_orders,
               pack_policy_hash
        FROM packing_sessions
        WHERE tenant_id = $1 AND id = $2
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing session"))?;
    let pack_policy = policy::decision_policy_from_session_row(&row)?;
    Ok(LockedSession {
        id: PackSessionId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: row.try_get("facility_id")?,
        order_release_id: row.try_get("order_release_id")?,
        packing_location_id: row.try_get("packing_location_id")?,
        state: row.try_get("state")?,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        expected_allocation_count: row.try_get("expected_allocation_count")?,
        packed_allocation_count: row.try_get("packed_allocation_count")?,
        expected_qty: row.try_get("expected_qty")?,
        packed_qty: row.try_get("packed_qty")?,
        open_carton_count: row.try_get("open_carton_count")?,
        closed_carton_count: row.try_get("closed_carton_count")?,
        pack_policy,
    })
}

pub(super) fn require_revision(
    order: &LockedOrder,
    session: Option<&LockedSession>,
    expected: OrderRevision,
) -> AppResult<OrderRevision> {
    if order.revision != expected || session.is_some_and(|value| value.revision != expected) {
        return Err(AppError::conflict("packing revision is stale"));
    }
    expected
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))
}

pub(super) async fn require_replayed_session_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &PackSessionReadModel,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM packing_sessions
            WHERE tenant_id = $1 AND id = $2 AND order_id = $3
              AND ($4 OR facility_id = ANY($5))
              AND ($6 OR inventory_owner_id = ANY($7))
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(result.session_id.get())
    .bind(result.order_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("packing session"))
    }
}

pub(super) async fn require_replayed_ids_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM packing_sessions
            WHERE tenant_id = $1 AND id = $2 AND order_id = $3
              AND ($4 OR facility_id = ANY($5))
              AND ($6 OR inventory_owner_id = ANY($7))
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .bind(order_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("packing session"))
    }
}

pub(super) async fn session_order_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
) -> AppResult<OrderId> {
    let id: i64 = sqlx::query_scalar(
        "SELECT order_id FROM packing_sessions WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing session"))?;
    OrderId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn enqueue_order_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    actor_user_id: i64,
    order_id: OrderId,
    event_type: &str,
    event_key: &str,
    payload: serde_json::Value,
    occurred_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    let facility_id =
        FacilityId::new(facility_id).map_err(|error| AppError::internal(error.to_string()))?;
    let aggregate_id = order_id.get().to_string();
    let ordering_key = format!("order:{}", order_id.get());
    let aggregate_sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_user_id),
            event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}
