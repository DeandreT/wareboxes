//! Application contracts for demand-backed inbound flow-through work.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, CrossDockCancellationDetails, CrossDockCancellationId,
    CrossDockClaimReleaseReason, CrossDockConfirmationId, CrossDockPlanId, CrossDockQuantity,
    CrossDockScanValue, CrossDockUom, CrossDockWorkId, CrossDockWorkStatus, FacilityId,
    InboundLoadId, InventoryBalanceId, InventoryOwnerId, ItemBatchId, LocationId, OrderId,
    OrderLineId, OrderRevision, Timestamp, UserId,
};

pub const PLAN_CROSS_DOCK_WORK_OPERATION: &str = "inbound.cross_dock.plan.v1";
pub const CLAIM_NEXT_CROSS_DOCK_WORK_OPERATION: &str = "inbound.cross_dock.claim_next.v1";
pub const CLAIM_CROSS_DOCK_WORK_BY_ID_OPERATION: &str = "inbound.cross_dock.claim_by_id.v1";
pub const HEARTBEAT_CROSS_DOCK_CLAIM_OPERATION: &str = "inbound.cross_dock.heartbeat.v1";
pub const RELEASE_CROSS_DOCK_CLAIM_OPERATION: &str = "inbound.cross_dock.release.v1";
pub const CONFIRM_CROSS_DOCK_WORK_OPERATION: &str = "inbound.cross_dock.confirm.v1";
pub const CANCEL_CROSS_DOCK_WORK_OPERATION: &str = "inbound.cross_dock.cancel.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanCrossDockWorkCommand {
    pub order_id: OrderId,
    pub order_line_id: OrderLineId,
    pub expected_order_revision: OrderRevision,
    pub source_receipt_inventory_transaction_id: i64,
    pub destination_pick_face_location_id: LocationId,
    pub quantity: CrossDockQuantity,
    pub priority: i64,
    pub assigned_user_id: Option<UserId>,
    pub due_at: Option<Timestamp>,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanCrossDockWorkResult {
    pub plan_id: CrossDockPlanId,
    pub work_id: CrossDockWorkId,
    pub order_id: OrderId,
    pub order_line_id: OrderLineId,
    pub reservation_id: i64,
    pub previous_order_revision: OrderRevision,
    pub order_revision: OrderRevision,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub inbound_load_id: InboundLoadId,
    pub source_receipt_inventory_transaction_id: i64,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub destination_pick_face_location_id: LocationId,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub uom: CrossDockUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantity: CrossDockQuantity,
    pub remaining_unallocated_quantity: i64,
    pub status: CrossDockWorkStatus,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClaimNextCrossDockWorkCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimCrossDockWorkByIdCommand {
    pub work_id: CrossDockWorkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatCrossDockClaimCommand {
    pub work_id: CrossDockWorkId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseCrossDockClaimCommand {
    pub work_id: CrossDockWorkId,
    pub reason: CrossDockClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelCrossDockWorkCommand {
    pub work_id: CrossDockWorkId,
    pub expected_order_revision: OrderRevision,
    pub details: CrossDockCancellationDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockClaimHeartbeatResult {
    pub work_id: CrossDockWorkId,
    pub heartbeat_at: Timestamp,
    pub lease_expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockClaimReleaseResult {
    pub work_id: CrossDockWorkId,
    pub status: CrossDockWorkStatus,
    pub released_at: Timestamp,
    pub release_count: i64,
    pub reason: CrossDockClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockLocationReadModel {
    pub location_id: LocationId,
    pub barcode: CrossDockScanValue,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockClaim {
    pub work_id: CrossDockWorkId,
    pub plan_id: CrossDockPlanId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub order_id: OrderId,
    pub order_key: String,
    pub order_line_id: OrderLineId,
    pub order_line_key: String,
    pub reservation_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<Timestamp>,
    pub lease_expires_at: Timestamp,
    pub source_receipt_inventory_transaction_id: i64,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<CrossDockScanValue>,
    pub uom: CrossDockUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantity: CrossDockQuantity,
    pub source_receiving_location: CrossDockLocationReadModel,
    pub destination_pick_face: CrossDockLocationReadModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfirmCrossDockWorkCommand {
    pub work_id: CrossDockWorkId,
    pub source_receiving_location_barcode: CrossDockScanValue,
    pub item_barcode: CrossDockScanValue,
    pub lot_scan: Option<CrossDockScanValue>,
    pub serial_scan: Option<CrossDockScanValue>,
    pub destination_pick_face_barcode: CrossDockScanValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmCrossDockWorkResult {
    pub confirmation_id: CrossDockConfirmationId,
    pub work_id: CrossDockWorkId,
    pub plan_id: CrossDockPlanId,
    pub order_id: OrderId,
    pub order_line_id: OrderLineId,
    pub reservation_id: i64,
    pub inventory_transaction_id: i64,
    pub inventory_allocation_id: i64,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub destination_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub destination_pick_face_location_id: LocationId,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub uom: CrossDockUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub quantity: CrossDockQuantity,
    pub work_status: CrossDockWorkStatus,
    pub confirmed_by: UserId,
    pub confirmed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelCrossDockWorkResult {
    pub cancellation_id: CrossDockCancellationId,
    pub work_id: CrossDockWorkId,
    pub plan_id: CrossDockPlanId,
    pub order_id: OrderId,
    pub order_line_id: OrderLineId,
    pub previous_order_revision: OrderRevision,
    pub order_revision: OrderRevision,
    pub quantity: CrossDockQuantity,
    pub previous_status: CrossDockWorkStatus,
    pub status: CrossDockWorkStatus,
    pub details: CrossDockCancellationDetails,
    pub cancelled_by: UserId,
    pub cancelled_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossDockWorkPageFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub order_id: Option<OrderId>,
    pub status: Option<CrossDockWorkStatus>,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossDockPlanningOptionPageFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockPlanningOptionReadModel {
    pub order_id: OrderId,
    pub order_key: String,
    pub order_revision: OrderRevision,
    pub order_line_id: OrderLineId,
    pub order_line_key: String,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub reservation_id: i64,
    pub item_id: CatalogItemId,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: CrossDockUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub unallocated_quantity: i64,
    pub source_receipt_inventory_transaction_id: i64,
    pub inbound_load_id: InboundLoadId,
    pub inbound_load_reference: Option<String>,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_receiving_location: CrossDockLocationReadModel,
    pub source_free_quantity: i64,
    pub receipt_remaining_quantity: i64,
    pub maximum_plan_quantity: i64,
    pub destination_pick_faces: Vec<CrossDockLocationReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockPlanningOptionPage {
    pub items: Vec<CrossDockPlanningOptionReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockWorkReadModel {
    pub work_id: CrossDockWorkId,
    pub plan_id: CrossDockPlanId,
    pub status: CrossDockWorkStatus,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub inbound_load_id: InboundLoadId,
    pub order_id: OrderId,
    pub order_key: String,
    pub order_revision: OrderRevision,
    pub order_line_id: OrderLineId,
    pub order_line_key: String,
    pub reservation_id: i64,
    pub priority: i64,
    pub item_id: CatalogItemId,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: CrossDockUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantity: CrossDockQuantity,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_receiving_location: CrossDockLocationReadModel,
    pub destination_pick_face: CrossDockLocationReadModel,
    pub claimed_by: Option<UserId>,
    pub lease_expires_at: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub completed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossDockWorkPage {
    pub items: Vec<CrossDockWorkReadModel>,
    pub next_offset: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_command_contains_scans_but_no_client_quantity() {
        let command = ConfirmCrossDockWorkCommand {
            work_id: CrossDockWorkId::new(1).unwrap(),
            source_receiving_location_barcode: CrossDockScanValue::new("DOCK-01").unwrap(),
            item_barcode: CrossDockScanValue::new("SKU-01").unwrap(),
            lot_scan: Some(CrossDockScanValue::new("LOT-01").unwrap()),
            serial_scan: None,
            destination_pick_face_barcode: CrossDockScanValue::new("PICK-01").unwrap(),
        };
        assert_eq!(command.work_id.get(), 1);
        assert_eq!(command.item_barcode.as_str(), "SKU-01");
    }
}
