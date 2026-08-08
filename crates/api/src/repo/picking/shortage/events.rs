use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::picking::ReportPickShortageResult;
use wareboxes_domain::{
    ActualPickQuantity, InventoryAllocationId, InventoryOwnerId, OrderId, OrderRevision,
    PickQuantity, PickShortageId, PickShortageRevision, PickShortageStatus, PickTaskId, TenantId,
    Timestamp,
};
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::orders::next_outbox_sequence_tx;

#[derive(Debug)]
pub(in crate::repo::picking) struct ParentShortageTransition {
    pub(super) shortage_id: PickShortageId,
    pub(super) inventory_owner_id: InventoryOwnerId,
    pub(super) facility_id: i64,
    pub(super) order_id: OrderId,
    pub(super) revision: PickShortageRevision,
    pub(super) status: PickShortageStatus,
    pub(super) reallocated_quantity: ActualPickQuantity,
    pub(super) recovery_terminal_quantity: ActualPickQuantity,
    pub(super) remaining_to_allocate_quantity: ActualPickQuantity,
    pub(super) trigger_task_id: PickTaskId,
    pub(super) trigger_source_allocation_id: InventoryAllocationId,
    pub(super) terminal_quantity: PickQuantity,
    pub(super) occurred_at: Timestamp,
}

pub(super) async fn enqueue_shortage_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    result: &ReportPickShortageResult,
) -> AppResult<()> {
    let facility_id = wareboxes_domain::FacilityId::new(facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("pick-shortage:{}", result.shortage_id.get());
    let aggregate_id = result.shortage_id.get().to_string();
    let ordering_key = format!("order:{}", result.order_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let payload = serde_json::json!({
        "pick_shortage_id": result.shortage_id,
        "pick_task_id": result.task_id,
        "pick_content_id": result.content_id,
        "order_id": result.order_id,
        "planned_quantity": result.quantities.planned(),
        "picked_quantity": result.quantities.picked(),
        "short_quantity": result.quantities.short(),
        "reason": result.details.reason(),
        "inventory_hold_id": result.hold.hold_id,
        "inventory_transaction_id": result.movement.as_ref().map(|value| value.inventory_transaction_id),
        "order_revision": result.order_revision,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(result.reported_by.get()),
            event_key: &event_key,
            aggregate_type: "pick_shortage",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.pick.shortage_reported",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.reported_at,
        },
    )
    .await?;
    Ok(())
}

pub(in crate::repo::picking) async fn enqueue_parent_shortage_transition_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    order_revision: OrderRevision,
    transition: &ParentShortageTransition,
) -> AppResult<()> {
    let facility_id = wareboxes_domain::FacilityId::new(transition.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!(
        "pick-shortage:{}:recovery:{}",
        transition.shortage_id.get(),
        transition.revision.get()
    );
    let aggregate_id = transition.shortage_id.get().to_string();
    let ordering_key = format!("order:{}", transition.order_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let payload = serde_json::json!({
        "pick_shortage_id": transition.shortage_id,
        "shortage_revision": transition.revision,
        "shortage_status": transition.status,
        "order_id": transition.order_id,
        "order_revision": order_revision,
        "reallocated_quantity": transition.reallocated_quantity,
        "recovery_terminal_quantity": transition.recovery_terminal_quantity,
        "remaining_to_allocate_quantity": transition.remaining_to_allocate_quantity,
        "trigger_pick_task_id": transition.trigger_task_id,
        "trigger_source_inventory_allocation_id": transition.trigger_source_allocation_id,
        "terminal_quantity": transition.terminal_quantity,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(transition.inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "pick_shortage",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.pick.shortage_recovery_progressed",
            schema_version: 1,
            payload: &payload,
            occurred_at: transition.occurred_at,
        },
    )
    .await?;
    Ok(())
}
