//! Application contracts for typed RF picking and claim lifecycle commands.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryOwnerId, ItemBatchId,
    LicensePlateId, LocationId, OrderId, OrderLineId, OrderRevision, PickClaimReleaseReason,
    PickContentId, PickContentState, PickQuantity, PickScanValue, PickTaskId, Timestamp, UserId,
};

/// Claims the next available waveless pick task for the current RF identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClaimNextPickCommand;

/// Claims one visible waveless pick task by its route identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimPickByIdCommand {
    pub task_id: PickTaskId,
}

/// Reads the active pick claim for the current RF identity, when one exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurrentPickQuery;

/// One ordered, allocation-backed unit of work in an active pick claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickClaimContent {
    pub content_id: PickContentId,
    pub order_line_id: OrderLineId,
    pub inventory_allocation_id: InventoryAllocationId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub source_location_id: LocationId,
    pub source_location_barcode: PickScanValue,
    pub source_location_name: Option<String>,
    pub source_license_plate_id: Option<LicensePlateId>,
    pub source_license_plate_barcode: Option<PickScanValue>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<PickScanValue>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub planned_quantity: PickQuantity,
    pub state: PickContentState,
}

/// Active scanner-ready claim without persistence or tenant metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickClaim {
    pub task_id: PickTaskId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub order_key: String,
    pub priority: i64,
    pub ship_by: Option<Timestamp>,
    pub lease_expires_at: Timestamp,
    pub destination_location_id: LocationId,
    pub destination_location_barcode: PickScanValue,
    pub destination_location_name: Option<String>,
    pub content: PickClaimContent,
}

/// Current-claim queries deliberately return absence instead of a synthetic task.
pub type CurrentPickResult = Option<PickClaim>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatPickClaimCommand {
    pub task_id: PickTaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickClaimHeartbeatResult {
    pub task_id: PickTaskId,
    pub heartbeat_at: Timestamp,
    pub lease_expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePickClaimCommand {
    pub task_id: PickTaskId,
    pub reason: PickClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickClaimReleaseResult {
    pub task_id: PickTaskId,
    pub released_at: Timestamp,
    pub release_count: i64,
    pub reason: PickClaimReleaseReason,
    pub note: Option<String>,
}

/// Confirms one allocation-backed content line using only scanned identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPickContentCommand {
    pub task_id: PickTaskId,
    pub content_id: PickContentId,
    pub source_location_barcode: PickScanValue,
    pub item_barcode: PickScanValue,
    pub source_license_plate_barcode: Option<PickScanValue>,
    pub destination_license_plate_barcode: PickScanValue,
}

/// Atomic inventory and workflow result of confirming one pick content line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmPickContentResult {
    pub result_id: i64,
    pub content_id: PickContentId,
    pub task_id: PickTaskId,
    pub order_id: OrderId,
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: InventoryAllocationId,
    pub destination_inventory_allocation_id: InventoryAllocationId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub destination_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub destination_location_id: LocationId,
    pub source_license_plate_id: Option<LicensePlateId>,
    pub destination_license_plate_id: LicensePlateId,
    pub picked_quantity: PickQuantity,
    pub confirmed_by: UserId,
    pub confirmed_at: Timestamp,
    pub content_state: PickContentState,
    pub task_completed: bool,
    pub order_ready_to_pack: bool,
    pub order_status: wareboxes_domain::OrderStatus,
    pub order_revision: OrderRevision,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_command_contains_scans_not_client_selected_dimensions() {
        let command = ConfirmPickContentCommand {
            task_id: PickTaskId::new(7).unwrap(),
            content_id: PickContentId::new(8).unwrap(),
            source_location_barcode: PickScanValue::new("A-01").unwrap(),
            item_barcode: PickScanValue::new("SKU-1").unwrap(),
            source_license_plate_barcode: Some(PickScanValue::new("LP-1").unwrap()),
            destination_license_plate_barcode: PickScanValue::new("TOTE-1").unwrap(),
        };

        assert_eq!(command.task_id.get(), 7);
        assert_eq!(command.content_id.get(), 8);
        assert_eq!(command.source_location_barcode.as_str(), "A-01");
    }

    #[test]
    fn current_pick_result_can_represent_no_active_claim() {
        let result: CurrentPickResult = None;
        assert!(result.is_none());
    }
}
