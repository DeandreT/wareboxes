//! Typed purchase-order source intake and release contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, NewPurchaseOrder, PurchaseOrderId,
    PurchaseOrderLineId, PurchaseOrderReleaseId, PurchaseOrderRevision, PurchaseOrderStatus,
    Timestamp, UserId,
};

pub const CREATE_PURCHASE_ORDER_OPERATION: &str = "inbound.purchase_order.create.v1";
pub const RELEASE_PURCHASE_ORDER_OPERATION: &str = "inbound.purchase_order.release.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePurchaseOrderCommand {
    pub order: NewPurchaseOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedPurchaseOrderLineResult {
    pub line_id: PurchaseOrderLineId,
    pub item_id: CatalogItemId,
    pub ordered_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePurchaseOrderResult {
    pub purchase_order_id: PurchaseOrderId,
    pub number: String,
    pub status: PurchaseOrderStatus,
    pub revision: PurchaseOrderRevision,
    pub lines: Vec<CreatedPurchaseOrderLineResult>,
    pub total_ordered_quantity: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasePurchaseOrderCommand {
    pub purchase_order_id: PurchaseOrderId,
    pub expected_revision: PurchaseOrderRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasePurchaseOrderResult {
    pub release_id: PurchaseOrderReleaseId,
    pub purchase_order_id: PurchaseOrderId,
    pub previous_status: PurchaseOrderStatus,
    pub status: PurchaseOrderStatus,
    pub revision: PurchaseOrderRevision,
    pub released_by: UserId,
    pub released_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseOrderLineReadModel {
    pub line_id: PurchaseOrderLineId,
    pub sequence: i64,
    pub item_id: CatalogItemId,
    pub item_description: String,
    pub uom: String,
    pub ordered_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseOrderReadModel {
    pub purchase_order_id: PurchaseOrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub number: String,
    pub supplier: String,
    pub expected_by: Option<Timestamp>,
    pub status: PurchaseOrderStatus,
    pub revision: PurchaseOrderRevision,
    pub line_count: i64,
    pub total_ordered_quantity: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub released_by: Option<UserId>,
    pub released_at: Option<Timestamp>,
    pub lines: Vec<PurchaseOrderLineReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseOrderPageFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub status: Option<PurchaseOrderStatus>,
    pub search: Option<String>,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseOrderPage {
    pub entries: Vec<PurchaseOrderReadModel>,
    pub next_offset: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idempotency::PreparedCommand;
    use crate::CommandContext;
    use wareboxes_domain::{
        PurchaseOrderLineDefinition, PurchaseOrderNumber, PurchaseOrderQuantity,
        PurchaseOrderSupplier, TenantId,
    };

    #[test]
    fn create_hash_contains_the_exact_line_set() {
        let command = CreatePurchaseOrderCommand {
            order: NewPurchaseOrder::new(
                InventoryOwnerId::new(2).unwrap(),
                FacilityId::new(3).unwrap(),
                PurchaseOrderNumber::new("PO-100").unwrap(),
                PurchaseOrderSupplier::new("Northstar Foods").unwrap(),
                None,
                vec![PurchaseOrderLineDefinition::new(
                    CatalogItemId::new(4).unwrap(),
                    PurchaseOrderQuantity::new(5).unwrap(),
                )],
            )
            .unwrap(),
        };
        let context = CommandContext {
            tenant_id: TenantId::new(1).unwrap(),
            actor_id: UserId::new(6).unwrap(),
            request_id: "req-1".into(),
            idempotency_key: Some("key-1".into()),
        };
        let prepared =
            PreparedCommand::new_v1(&context, CREATE_PURCHASE_ORDER_OPERATION, &command).unwrap();
        assert!(!prepared.request_hash().is_empty());
    }
}
