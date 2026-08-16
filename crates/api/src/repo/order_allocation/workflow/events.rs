use wareboxes_application::order_allocation::{
    AllocationPolicyReadModel, PlanOrderAllocationCommand, ORDER_ALLOCATION_OPERATION,
};
use wareboxes_domain::{
    AllocationOutcome, AllocationQuantity, AllocationRunId, InventoryAllocationId,
    InventoryOwnerId, InventoryReservationId, LicensePlateId, OrderRevision, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::{LockedCandidate, LockedOrderLine, PlannedLine};
use crate::error::{AppError, AppResult};

#[allow(clippy::too_many_arguments)]
pub(super) async fn enqueue_reservation_created_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    command: &PlanOrderAllocationCommand,
    actor_user_id: i64,
    line: &LockedOrderLine,
    reservation_id: InventoryReservationId,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let event_key = format!("inventory-reservation:{reservation_id}:created");
    let aggregate_id = reservation_id.to_string();
    let ordering_key = format!("inventory-reservation:{reservation_id}");
    let payload = serde_json::json!({
        "reservation_id": reservation_id.get(),
        "order_id": command.order_id.get(),
        "order_item_id": line.id.get(),
        "line_key": line.line_key,
        "inventory_owner_id": inventory_owner_id.get(),
        "facility_id": command.facility_id.get(),
        "item_id": line.item_id,
        "uom": line.uom,
        "quantity": line.quantity.get(),
        "source": ORDER_ALLOCATION_OPERATION,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(command.facility_id),
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "inventory_reservation",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: 1,
            event_type: "inventory.reservation.created",
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn enqueue_allocation_created_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    actor_user_id: i64,
    command: &PlanOrderAllocationCommand,
    policy: &AllocationPolicyReadModel,
    allocation_run_id: AllocationRunId,
    planned_line: &PlannedLine,
    candidate: &LockedCandidate,
    allocation_id: InventoryAllocationId,
    quantity: AllocationQuantity,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let event_key = format!("inventory-allocation:{allocation_id}:created");
    let aggregate_id = allocation_id.to_string();
    let ordering_key = format!("inventory-allocation:{allocation_id}");
    let payload = serde_json::json!({
        "allocation_id": allocation_id.get(),
        "allocation_run_id": allocation_run_id.get(),
        "reservation_id": planned_line.reservation_id.get(),
        "order_id": command.order_id.get(),
        "order_item_id": planned_line.line.id.get(),
        "inventory_balance_id": candidate.inventory_balance_id.get(),
        "inventory_owner_id": inventory_owner_id.get(),
        "facility_id": command.facility_id.get(),
        "location_id": candidate.location_id.get(),
        "license_plate_id": candidate.license_plate_id.map(LicensePlateId::get),
        "item_batch_id": candidate.item_batch_id.get(),
        "item_id": planned_line.line.item_id,
        "uom": planned_line.line.uom,
        "inventory_status": "available",
        "quantity": quantity.get(),
        "allocation_policy": policy,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(command.facility_id),
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "inventory_allocation",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: 1,
            event_type: "inventory.allocation.created",
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::repo::order_allocation) async fn enqueue_order_allocation_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    actor_user_id: i64,
    command: &PlanOrderAllocationCommand,
    policy: &AllocationPolicyReadModel,
    allocation_run_id: AllocationRunId,
    outcome: AllocationOutcome,
    revision: OrderRevision,
    demand_quantity: i64,
    newly_allocated_quantity: i64,
    allocated_quantity: i64,
    shortage_quantity: i64,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let ordering_key = format!("order:{}", command.order_id);
    let aggregate_sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "order:{}:allocation-run:{}",
        command.order_id, allocation_run_id
    );
    let aggregate_id = command.order_id.to_string();
    let payload = serde_json::json!({
        "order_id": command.order_id.get(),
        "inventory_owner_id": inventory_owner_id.get(),
        "facility_id": command.facility_id.get(),
        "allocation_run_id": allocation_run_id.get(),
        "strategy": policy.strategy.as_str(),
        "allocation_policy": policy,
        "outcome": outcome.as_str(),
        "revision": revision.get(),
        "demand_quantity": demand_quantity,
        "newly_allocated_quantity": newly_allocated_quantity,
        "allocated_quantity": allocated_quantity,
        "shortage_quantity": shortage_quantity,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(command.facility_id),
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: "order.allocation.planned",
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

async fn next_outbox_sequence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    ordering_key: &str,
) -> AppResult<i64> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("outbox-sequence:{tenant_id}:{ordering_key}"))
        .execute(&mut **tx)
        .await?;
    let last_sequence: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT last_sequence
        FROM outbox_aggregate_sequences
        WHERE tenant_id = $1 AND ordering_key = $2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(ordering_key)
    .fetch_optional(&mut **tx)
    .await?;
    last_sequence
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| AppError::internal("outbox aggregate sequence exceeds i64"))
}
