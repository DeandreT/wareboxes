use serde::{Deserialize, Serialize};

use super::Revision;

/// Durable business reason for cancelling a fulfillment order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderCancellationReason {
    ClientRequest,
    DuplicateOrder,
    DataCorrection,
    InventoryUnavailable,
    FulfillmentException,
    Other,
}

/// Parameters for an optimistic, replay-safe order cancellation command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelOrderRequest {
    pub expected_revision: Revision,
    pub reason: OrderCancellationReason,
    pub note: Option<String>,
}

/// Stable order states exposed by the cancellation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderCancellationStatus {
    Cancelled,
    Held,
    Open,
    Processing,
}

/// Replay-stable result of one committed cancellation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelOrderResponse {
    pub cancellation_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub previous_status: OrderCancellationStatus,
    pub status: OrderCancellationStatus,
    pub revision: Revision,
    pub reason: OrderCancellationReason,
    pub note: Option<String>,
    pub released_hold_count: i64,
    pub released_reservation_count: i64,
    pub released_allocation_count: i64,
    pub released_quantity: i64,
    pub cancelled_pick_task_count: i64,
    pub cancelled_pick_content_count: i64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn cancellation_request_is_strict_revisioned_and_snake_case() {
        let request = serde_json::from_value::<CancelOrderRequest>(json!({
            "expected_revision": 3,
            "reason": "client_request",
            "note": "Client closed the sales order"
        }))
        .unwrap();
        assert_eq!(request.expected_revision.get(), 3);
        assert_eq!(request.reason, OrderCancellationReason::ClientRequest);

        assert!(serde_json::from_value::<CancelOrderRequest>(json!({
            "expected_revision": 0,
            "reason": "client_request",
            "note": null
        }))
        .is_err());
        assert!(serde_json::from_value::<CancelOrderRequest>(json!({
            "expected_revision": 3,
            "reason": "customer_request",
            "note": null
        }))
        .is_err());
        assert!(serde_json::from_value::<CancelOrderRequest>(json!({
            "expected_revision": 3,
            "reason": "client_request",
            "note": null,
            "force": true
        }))
        .is_err());
    }

    #[test]
    fn cancellation_response_has_a_stable_auditable_shape() {
        let response = CancelOrderResponse {
            cancellation_id: 31,
            order_id: 7,
            inventory_owner_id: 9,
            previous_status: OrderCancellationStatus::Held,
            status: OrderCancellationStatus::Cancelled,
            revision: Revision::new(4).unwrap(),
            reason: OrderCancellationReason::InventoryUnavailable,
            note: Some("Stock will not arrive in time".into()),
            released_hold_count: 2,
            released_reservation_count: 3,
            released_allocation_count: 2,
            released_quantity: 12,
            cancelled_pick_task_count: 2,
            cancelled_pick_content_count: 2,
        };

        let encoded = serde_json::to_value(&response).unwrap();
        assert_eq!(
            encoded,
            json!({
                "cancellation_id": 31,
                "order_id": 7,
                "inventory_owner_id": 9,
                "previous_status": "held",
                "status": "cancelled",
                "revision": 4,
                "reason": "inventory_unavailable",
                "note": "Stock will not arrive in time",
                "released_hold_count": 2,
                "released_reservation_count": 3,
                "released_allocation_count": 2,
                "released_quantity": 12,
                "cancelled_pick_task_count": 2,
                "cancelled_pick_content_count": 2
            })
        );
        assert_eq!(
            serde_json::from_value::<CancelOrderResponse>(encoded).unwrap(),
            response
        );
        assert!(serde_json::from_value::<CancelOrderResponse>(json!({
            "cancellation_id": 31,
            "order_id": 7,
            "inventory_owner_id": 9,
            "previous_status": "open",
            "status": "cancelled",
            "revision": 4,
            "reason": "client_request",
            "note": null,
            "released_hold_count": 0,
            "released_reservation_count": 0,
            "released_allocation_count": 0,
            "released_quantity": 0,
            "recovery_task_id": 11
        }))
        .is_err());
    }
}
