//! Application contracts for optimistic, replay-safe fulfillment order cancellation.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CancellationNote, InventoryOwnerId, OrderCancellationDetails, OrderCancellationError,
    OrderCancellationId, OrderCancellationReason, OrderId, OrderRevision, OrderStatus,
};

/// Stable idempotency operation for the first order cancellation command schema.
pub const ORDER_CANCELLATION_OPERATION: &str = "order.cancel.v1";

/// Cancels one order at the revision observed by the operator.
///
/// The inventory owner and affected facilities are deliberately absent. They are
/// derived from the scoped, locked order and its commitments in the transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelOrderCommand {
    order_id: OrderId,
    expected_revision: OrderRevision,
    reason: OrderCancellationReason,
    note: Option<CancellationNote>,
}

impl CancelOrderCommand {
    pub fn new(
        order_id: OrderId,
        expected_revision: OrderRevision,
        reason: OrderCancellationReason,
        note: Option<CancellationNote>,
    ) -> Result<Self, OrderCancellationError> {
        let cancellation = OrderCancellationDetails::new(reason, note)?;
        Ok(Self {
            order_id,
            expected_revision,
            reason: cancellation.reason(),
            note: cancellation.into_note(),
        })
    }

    pub const fn order_id(&self) -> OrderId {
        self.order_id
    }

    pub const fn expected_revision(&self) -> OrderRevision {
        self.expected_revision
    }

    pub const fn reason(&self) -> OrderCancellationReason {
        self.reason
    }

    pub fn note(&self) -> Option<&CancellationNote> {
        self.note.as_ref()
    }
}

/// Replay-stable result of one committed cancellation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelOrderResult {
    pub cancellation_id: OrderCancellationId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub previous_status: OrderStatus,
    pub status: OrderStatus,
    pub revision: OrderRevision,
    pub reason: OrderCancellationReason,
    pub note: Option<CancellationNote>,
    pub released_hold_count: i64,
    pub released_reservation_count: i64,
    pub released_allocation_count: i64,
    pub released_quantity: i64,
    pub cancelled_pick_task_count: i64,
    pub cancelled_pick_content_count: i64,
    pub reversed_pick_confirmation_count: i64,
    pub released_outbound_container_count: i64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn command_derives_scope_and_requires_context_for_other() {
        let command = CancelOrderCommand::new(
            OrderId::new(7).unwrap(),
            OrderRevision::new(3).unwrap(),
            OrderCancellationReason::ClientRequest,
            None,
        )
        .unwrap();

        assert_eq!(command.order_id().get(), 7);
        assert_eq!(command.expected_revision().get(), 3);
        assert_eq!(command.reason(), OrderCancellationReason::ClientRequest);
        assert_eq!(command.note(), None);
        assert_eq!(
            serde_json::to_value(&command).unwrap(),
            json!({
                "order_id": 7,
                "expected_revision": 3,
                "reason": "client_request",
                "note": null
            })
        );

        assert_eq!(
            CancelOrderCommand::new(
                OrderId::new(7).unwrap(),
                OrderRevision::new(3).unwrap(),
                OrderCancellationReason::Other,
                None,
            ),
            Err(OrderCancellationError::NoteRequired)
        );
    }

    #[test]
    fn result_preserves_auditable_release_totals() {
        let result = CancelOrderResult {
            cancellation_id: OrderCancellationId::new(31).unwrap(),
            order_id: OrderId::new(7).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(9).unwrap(),
            previous_status: OrderStatus::Held,
            status: OrderStatus::Cancelled,
            revision: OrderRevision::new(4).unwrap(),
            reason: OrderCancellationReason::InventoryUnavailable,
            note: Some(CancellationNote::new("Stock will not arrive in time").unwrap()),
            released_hold_count: 2,
            released_reservation_count: 3,
            released_allocation_count: 2,
            released_quantity: 12,
            cancelled_pick_task_count: 2,
            cancelled_pick_content_count: 2,
            reversed_pick_confirmation_count: 1,
            released_outbound_container_count: 1,
        };

        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(encoded["cancellation_id"], 31);
        assert_eq!(encoded["previous_status"], "held");
        assert_eq!(encoded["status"], "cancelled");
        assert_eq!(encoded["released_quantity"], 12);
        assert_eq!(encoded["cancelled_pick_task_count"], 2);
        assert_eq!(encoded["reversed_pick_confirmation_count"], 1);
        assert_eq!(encoded["released_outbound_container_count"], 1);
        assert_eq!(
            serde_json::from_value::<CancelOrderResult>(encoded).unwrap(),
            result
        );
    }
}
