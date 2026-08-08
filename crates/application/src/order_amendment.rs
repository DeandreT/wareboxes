//! Replay-safe pre-execution fulfillment-order amendment contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    InventoryOwnerId, OrderAmendmentId, OrderId, OrderRevision, OrderStatus, ShippingDestination,
    Timestamp, UserId,
};

pub const AMEND_FULFILLMENT_ORDER_OPERATION: &str = "outbound.order.amend.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AmendFulfillmentOrderCommand {
    order_id: OrderId,
    expected_revision: OrderRevision,
    rush: bool,
    ship_by: Option<Timestamp>,
    destination: ShippingDestination,
}

impl AmendFulfillmentOrderCommand {
    pub const fn new(
        order_id: OrderId,
        expected_revision: OrderRevision,
        rush: bool,
        ship_by: Option<Timestamp>,
        destination: ShippingDestination,
    ) -> Self {
        Self {
            order_id,
            expected_revision,
            rush,
            ship_by,
            destination,
        }
    }

    pub const fn order_id(&self) -> OrderId {
        self.order_id
    }

    pub const fn expected_revision(&self) -> OrderRevision {
        self.expected_revision
    }

    pub const fn rush(&self) -> bool {
        self.rush
    }

    pub const fn ship_by(&self) -> Option<&Timestamp> {
        self.ship_by.as_ref()
    }

    pub const fn destination(&self) -> &ShippingDestination {
        &self.destination
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmendFulfillmentOrderResult {
    pub amendment_id: OrderAmendmentId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub order_status: OrderStatus,
    pub revision: OrderRevision,
    pub rush: bool,
    pub ship_by: Option<Timestamp>,
    pub destination: ShippingDestination,
    pub amended_by: UserId,
    pub amended_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wareboxes_domain::{ShippingRecipient, Timestamp};

    use super::*;

    #[test]
    fn command_hash_shape_includes_path_revision_and_complete_resulting_header() {
        let command = AmendFulfillmentOrderCommand::new(
            OrderId::new(17).unwrap(),
            OrderRevision::new(3).unwrap(),
            true,
            Some("2027-08-12T17:00:00Z".parse::<Timestamp>().unwrap()),
            ShippingDestination::new(
                ShippingRecipient::new("Receiving", None, None, None).unwrap(),
                "200 New Way",
                None,
                "Reno",
                "NV",
                "89502",
                "US",
            )
            .unwrap(),
        );

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "order_id": 17,
                "expected_revision": 3,
                "rush": true,
                "ship_by": "2027-08-12T17:00:00Z",
                "destination": {
                    "recipient": {"name": "Receiving", "company": null, "phone": null, "email": null},
                    "line1": "200 New Way",
                    "line2": null,
                    "city": "Reno",
                    "region": "NV",
                    "postal_code": "89502",
                    "country": "US"
                }
            })
        );
    }
}
