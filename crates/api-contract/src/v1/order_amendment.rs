use serde::{Deserialize, Serialize};

use super::{FulfillmentOrderDestination, Revision};

/// Complete resulting mutable header for an optimistic pre-execution amendment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmendFulfillmentOrderRequest {
    pub expected_revision: Revision,
    pub rush: bool,
    /// RFC 3339 timestamp, or `None` to clear the shipping deadline.
    pub ship_by: Option<String>,
    pub destination: FulfillmentOrderDestination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmendedFulfillmentOrderStatus {
    Open,
    Held,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmendFulfillmentOrderResponse {
    pub amendment_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub status: AmendedFulfillmentOrderStatus,
    pub revision: Revision,
    pub rush: bool,
    pub ship_by: Option<String>,
    pub destination: FulfillmentOrderDestination,
    pub amended_by: i64,
    pub amended_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn value() -> serde_json::Value {
        json!({
            "expected_revision": 3,
            "rush": true,
            "ship_by": null,
            "destination": {
                "recipient_name": "Receiving Team",
                "company": "Northstar Retail",
                "phone": null,
                "email": null,
                "line1": "200 New Way",
                "line2": null,
                "city": "Reno",
                "region": "NV",
                "postal_code": "89502",
                "country": "US"
            }
        })
    }

    #[test]
    fn amendment_request_is_strict_and_can_explicitly_clear_ship_by() {
        let request = serde_json::from_value::<AmendFulfillmentOrderRequest>(value()).unwrap();
        assert_eq!(request.expected_revision.get(), 3);
        assert_eq!(request.ship_by, None);

        let mut unknown = value();
        unknown["order_id"] = json!(17);
        assert!(serde_json::from_value::<AmendFulfillmentOrderRequest>(unknown).is_err());

        let mut invalid_revision = value();
        invalid_revision["expected_revision"] = json!(0);
        assert!(serde_json::from_value::<AmendFulfillmentOrderRequest>(invalid_revision).is_err());
    }
}
