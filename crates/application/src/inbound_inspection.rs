//! Application contracts for terminal disposition of quarantined inbound receipts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InboundInspectionDispositionId, InboundInspectionNote, InboundInspectionOutcome,
    InboundInspectionTargetStatus, InventoryBalanceId, InventoryHoldId, InventoryOwnerId,
    ItemBatchId, LocationId, Timestamp, UserId,
};

pub const DISPOSE_INBOUND_INSPECTION_OPERATION: &str = "inbound.inspection.dispose.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisposeInboundInspectionCommand {
    pub inventory_hold_id: InventoryHoldId,
    pub outcome: InboundInspectionOutcome,
    pub note: InboundInspectionNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisposeInboundInspectionResult {
    pub disposition_id: InboundInspectionDispositionId,
    pub inventory_hold_id: InventoryHoldId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub target_inventory_balance_id: InventoryBalanceId,
    pub location_id: LocationId,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: ItemBatchId,
    pub item_id: i64,
    pub uom: String,
    pub quantity: i64,
    pub outcome: InboundInspectionOutcome,
    pub target_status: InboundInspectionTargetStatus,
    pub note: InboundInspectionNote,
    pub inventory_transaction_id: i64,
    pub inspected_by: UserId,
    pub inspected_at: Timestamp,
}
