//! Replay-safe facility batch recall commands and read contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, InventoryRecallDetails, InventoryRecallId,
    InventoryRecallRevision, InventoryRecallStatus, ItemBatchId, Timestamp, UserId,
};

pub const CREATE_INVENTORY_RECALL_OPERATION: &str = "inventory.recall.create.v1";
pub const RELEASE_INVENTORY_RECALL_OPERATION: &str = "inventory.recall.release.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateInventoryRecallCommand {
    pub facility_id: FacilityId,
    pub item_batch_id: ItemBatchId,
    pub details: InventoryRecallDetails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReleaseInventoryRecallCommand {
    pub recall_id: InventoryRecallId,
    pub expected_revision: InventoryRecallRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryRecallCursor {
    pub before_id: InventoryRecallId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryRecallPageQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub status: Option<InventoryRecallStatus>,
    pub cursor: Option<InventoryRecallCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryRecallReadModel {
    pub recall_id: InventoryRecallId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub item_batch_id: ItemBatchId,
    pub item_id: i64,
    pub primary_sku: Option<String>,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub status: InventoryRecallStatus,
    pub revision: InventoryRecallRevision,
    pub details: InventoryRecallDetails,
    pub affected_position_count: u32,
    pub held_quantity: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub released_by: Option<UserId>,
    pub released_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryRecallPage {
    pub items: Vec<InventoryRecallReadModel>,
    pub next_cursor: Option<InventoryRecallCursor>,
}

pub type CreateInventoryRecallResult = InventoryRecallReadModel;
pub type ReleaseInventoryRecallResult = InventoryRecallReadModel;

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{InventoryRecallNote, InventoryRecallReason};

    #[test]
    fn create_hash_shape_contains_facility_batch_and_validated_details() {
        let command = CreateInventoryRecallCommand {
            facility_id: FacilityId::new(3).unwrap(),
            item_batch_id: ItemBatchId::new(7).unwrap(),
            details: InventoryRecallDetails::new(
                InventoryRecallReason::SupplierNotice,
                Some(InventoryRecallNote::new("Recall bulletin 42").unwrap()),
            )
            .unwrap(),
        };
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            serde_json::json!({
                "facility_id": 3,
                "item_batch_id": 7,
                "details": {"reason": "supplier_notice", "note": "Recall bulletin 42"}
            })
        );
    }
}
