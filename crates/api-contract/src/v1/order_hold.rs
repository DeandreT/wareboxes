use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderHoldReason {
    AddressReview,
    ComplianceReview,
    CustomerRequest,
    InventoryShortage,
    PaymentReview,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceOrderHoldRequest {
    pub reason: OrderHoldReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderHoldOrderStatus {
    Held,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceOrderHoldResponse {
    pub order_id: i64,
    pub hold_id: i64,
    pub order_status: OrderHoldOrderStatus,
    pub active_hold_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ReleaseOrderHoldRequest {
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseOrderHoldResponse {
    pub order_id: i64,
    pub hold_id: i64,
    pub order_status: OrderHoldOrderStatus,
    pub active_hold_count: i64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn order_hold_contract_is_typed_and_header_idempotency_stays_out_of_the_body() {
        let request = serde_json::from_value::<PlaceOrderHoldRequest>(json!({
            "reason": "customer_request",
            "note": "Pause until the client confirms the address"
        }))
        .unwrap();
        assert_eq!(request.reason, OrderHoldReason::CustomerRequest);
        assert!(serde_json::from_value::<PlaceOrderHoldRequest>(json!({
            "reason": "customer_request",
            "note": null,
            "idempotency_key": "body-key"
        }))
        .is_err());
        assert!(serde_json::from_value::<ReleaseOrderHoldRequest>(json!({
            "force": true
        }))
        .is_err());

        assert_eq!(
            serde_json::to_value(ReleaseOrderHoldResponse {
                order_id: 7,
                hold_id: 11,
                order_status: OrderHoldOrderStatus::Open,
                active_hold_count: 0,
            })
            .unwrap(),
            json!({
                "order_id": 7,
                "hold_id": 11,
                "order_status": "open",
                "active_hold_count": 0
            })
        );
    }
}
