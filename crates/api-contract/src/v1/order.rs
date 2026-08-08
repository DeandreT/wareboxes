use serde::{Deserialize, Serialize};

use super::Revision;

/// Shipping destination captured with a fulfillment order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FulfillmentOrderDestination {
    pub recipient_name: String,
    pub company: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
}

/// One independently addressable line of fulfillment demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateFulfillmentOrderLineRequest {
    pub line_key: String,
    pub item_id: i64,
    pub quantity: i64,
    pub requested_uom: String,
}

/// Creates an open fulfillment order and all of its demand lines atomically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateFulfillmentOrderRequest {
    pub inventory_owner_id: i64,
    pub order_key: String,
    #[serde(default)]
    pub rush: bool,
    /// RFC 3339 timestamp, or `None` when the order has no shipping deadline.
    pub ship_by: Option<String>,
    pub destination: FulfillmentOrderDestination,
    pub lines: Vec<CreateFulfillmentOrderLineRequest>,
}

/// Initial state of a newly accepted fulfillment order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatedFulfillmentOrderStatus {
    Open,
}

/// Server identity assigned to one accepted client demand line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatedFulfillmentOrderLine {
    pub order_line_id: i64,
    pub line_key: String,
}

/// Result of atomically creating an order header and its demand lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateFulfillmentOrderResponse {
    pub order_id: i64,
    pub order_key: String,
    pub status: CreatedFulfillmentOrderStatus,
    pub revision: Revision,
    pub lines: Vec<CreatedFulfillmentOrderLine>,
}

/// Active owner-linked item available to the fulfillment order-entry workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderEntryItemResponse {
    pub item_id: i64,
    pub description: Option<String>,
    pub requested_uom: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request_value() -> serde_json::Value {
        json!({
            "inventory_owner_id": 7,
            "order_key": "SO-1001",
            "rush": true,
            "ship_by": "2027-08-12T17:00:00Z",
            "destination": {
                "recipient_name": "Receiving Team",
                "company": "Northstar Retail",
                "phone": "+1 775 555 0100",
                "email": "receiving@example.com",
                "line1": "125 Shipping Lane",
                "line2": "Dock 4",
                "city": "Reno",
                "region": "NV",
                "postal_code": "89502",
                "country": "US"
            },
            "lines": [{
                "line_key": "1",
                "item_id": 41,
                "quantity": 12,
                "requested_uom": "case"
            }]
        })
    }

    #[test]
    fn fulfillment_order_creation_contract_has_an_exact_nested_shape() {
        let request = serde_json::from_value::<CreateFulfillmentOrderRequest>(request_value())
            .expect("valid order creation request");
        assert_eq!(request.order_key, "SO-1001");
        assert_eq!(request.ship_by.as_deref(), Some("2027-08-12T17:00:00Z"));
        assert_eq!(request.destination.recipient_name, "Receiving Team");
        assert_eq!(request.destination.region, "NV");
        assert_eq!(request.lines[0].requested_uom, "case");

        let mut unknown_header = request_value();
        unknown_header["tenant_id"] = json!(99);
        assert!(serde_json::from_value::<CreateFulfillmentOrderRequest>(unknown_header).is_err());

        let mut unknown_destination = request_value();
        unknown_destination["destination"]["address_id"] = json!(81);
        assert!(
            serde_json::from_value::<CreateFulfillmentOrderRequest>(unknown_destination).is_err()
        );

        let mut unknown_line = request_value();
        unknown_line["lines"][0]["allocated_quantity"] = json!(4);
        assert!(serde_json::from_value::<CreateFulfillmentOrderRequest>(unknown_line).is_err());
    }

    #[test]
    fn fulfillment_order_creation_response_returns_stable_line_correlation() {
        let response = CreateFulfillmentOrderResponse {
            order_id: 101,
            order_key: "SO-1001".into(),
            status: CreatedFulfillmentOrderStatus::Open,
            revision: Revision::new(1).unwrap(),
            lines: vec![
                CreatedFulfillmentOrderLine {
                    order_line_id: 201,
                    line_key: "1".into(),
                },
                CreatedFulfillmentOrderLine {
                    order_line_id: 202,
                    line_key: "2".into(),
                },
            ],
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "order_id": 101,
                "order_key": "SO-1001",
                "status": "open",
                "revision": 1,
                "lines": [
                    {"order_line_id": 201, "line_key": "1"},
                    {"order_line_id": 202, "line_key": "2"}
                ]
            })
        );
    }

    #[test]
    fn order_entry_item_contract_is_narrow_and_exact() {
        let item = OrderEntryItemResponse {
            item_id: 41,
            description: Some("Case-picked item".into()),
            requested_uom: "case".into(),
        };
        assert_eq!(
            serde_json::to_value(item).unwrap(),
            json!({
                "item_id": 41,
                "description": "Case-picked item",
                "requested_uom": "case"
            })
        );
        assert!(serde_json::from_value::<OrderEntryItemResponse>(json!({
            "item_id": 41,
            "description": null,
            "requested_uom": "case",
            "inventory_quantity": 500
        }))
        .is_err());
    }
}
