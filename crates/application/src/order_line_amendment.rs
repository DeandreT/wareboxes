//! Replay-safe exact replacement of pre-execution fulfillment demand lines.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, FulfillmentOrderDemandLine, InventoryOwnerId, OrderId, OrderLineAmendmentId,
    OrderLineId, OrderLineKey, OrderQuantity, OrderRevision, OrderStatus, RequestedUom, Timestamp,
    UserId,
};

pub const REPLACE_FULFILLMENT_ORDER_LINES_OPERATION: &str = "outbound.order.lines.replace.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplacementOrderLine {
    line_key: String,
    item_id: i64,
    quantity: i64,
    requested_uom: String,
}

impl ReplacementOrderLine {
    pub fn new(
        line_key: impl Into<String>,
        item_id: i64,
        quantity: i64,
        requested_uom: impl Into<String>,
    ) -> Result<Self, wareboxes_domain::OrderCreationError> {
        let line_key = OrderLineKey::new(line_key)?;
        let item_id = CatalogItemId::new(item_id)?;
        let quantity = OrderQuantity::new(quantity)?;
        let requested_uom = RequestedUom::new(requested_uom)?;
        Ok(Self {
            line_key: line_key.to_string(),
            item_id: item_id.get(),
            quantity: quantity.get(),
            requested_uom: requested_uom.to_string(),
        })
    }

    pub fn as_domain(
        &self,
    ) -> Result<FulfillmentOrderDemandLine, wareboxes_domain::OrderCreationError> {
        Ok(FulfillmentOrderDemandLine::new(
            OrderLineKey::new(self.line_key.clone())?,
            CatalogItemId::new(self.item_id)?,
            OrderQuantity::new(self.quantity)?,
            RequestedUom::new(self.requested_uom.clone())?,
        ))
    }

    pub fn line_key(&self) -> &str {
        &self.line_key
    }
    pub const fn item_id(&self) -> i64 {
        self.item_id
    }
    pub const fn quantity(&self) -> i64 {
        self.quantity
    }
    pub fn requested_uom(&self) -> &str {
        &self.requested_uom
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplaceFulfillmentOrderLinesCommand {
    order_id: OrderId,
    expected_revision: OrderRevision,
    lines: Vec<ReplacementOrderLine>,
}

impl ReplaceFulfillmentOrderLinesCommand {
    pub const fn new(
        order_id: OrderId,
        expected_revision: OrderRevision,
        lines: Vec<ReplacementOrderLine>,
    ) -> Self {
        Self {
            order_id,
            expected_revision,
            lines,
        }
    }
    pub const fn order_id(&self) -> OrderId {
        self.order_id
    }
    pub const fn expected_revision(&self) -> OrderRevision {
        self.expected_revision
    }
    pub fn lines(&self) -> &[ReplacementOrderLine] {
        &self.lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplacedOrderLineReadModel {
    pub order_line_id: OrderLineId,
    pub line_key: String,
    pub line_number: u32,
    pub item_id: CatalogItemId,
    pub quantity: i64,
    pub requested_uom: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceFulfillmentOrderLinesResult {
    pub amendment_id: OrderLineAmendmentId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub order_status: OrderStatus,
    pub previous_revision: OrderRevision,
    pub revision: OrderRevision,
    pub previous_line_count: i64,
    pub previous_quantity: i64,
    pub resulting_quantity: i64,
    pub released_reservation_count: i64,
    pub released_allocation_count: i64,
    pub released_quantity: i64,
    pub lines: Vec<ReplacedOrderLineReadModel>,
    pub amended_by: UserId,
    pub amended_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_hash_contains_path_identity_revision_and_exact_ordered_lines() {
        let command = ReplaceFulfillmentOrderLinesCommand::new(
            OrderId::new(19).unwrap(),
            OrderRevision::new(4).unwrap(),
            vec![ReplacementOrderLine::new("A", 31, 8, "case").unwrap()],
        );
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "order_id": 19,
                "expected_revision": 4,
                "lines": [{"line_key":"A","item_id":31,"quantity":8,"requested_uom":"case"}]
            })
        );
    }
}
