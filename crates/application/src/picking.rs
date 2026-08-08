//! Application contracts for typed RF picking and claim lifecycle commands.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    ActualPickQuantity, AllocationExecutionStage, AllocationOutcome, AllocationQuantity,
    AllocationStrategy, FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryHoldId,
    InventoryOwnerId, ItemBatchId, LicensePlateId, LocationId, OrderId, OrderLineId, OrderRevision,
    PickClaimReleaseReason, PickContentId, PickContentState, PickQuantity, PickScanValue,
    PickShortageDetails, PickShortageId, PickShortageQuantities, PickShortageReallocationRunId,
    PickShortageRevision, PickShortageStatus, PickTaskId, Timestamp, UserId,
};

pub const REPORT_PICK_SHORTAGE_OPERATION: &str = "picking.shortage.report.v1";
pub const REALLOCATE_PICK_SHORTAGE_OPERATION: &str = "picking.shortage.reallocate.v1";

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
    pub order_revision: OrderRevision,
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

/// Physical outcome reported by the operator for a short pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportPickShortageOutcome {
    NoPick,
    Partial {
        picked_quantity: PickQuantity,
        destination_license_plate_barcode: PickScanValue,
    },
}

impl ReportPickShortageOutcome {
    pub fn actual_quantity(&self) -> ActualPickQuantity {
        match self {
            Self::NoPick => ActualPickQuantity::ZERO,
            Self::Partial {
                picked_quantity, ..
            } => ActualPickQuantity::from(*picked_quantity),
        }
    }
}

/// Reports a shortage against the current allocation-backed claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportPickShortageCommand {
    pub task_id: PickTaskId,
    pub content_id: PickContentId,
    pub source_location_barcode: PickScanValue,
    pub source_license_plate_barcode: Option<PickScanValue>,
    pub observed_item_barcode: Option<PickScanValue>,
    pub observed_lot: Option<PickScanValue>,
    pub observed_serial: Option<PickScanValue>,
    pub details: PickShortageDetails,
    pub outcome: ReportPickShortageOutcome,
}

/// Inventory movement committed for a nonzero partial pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickShortageMovementResult {
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
    pub destination_stage: AllocationExecutionStage,
}

/// Quantity hold created for the physically short source stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickShortageHoldResult {
    pub hold_id: InventoryHoldId,
    pub inventory_balance_id: InventoryBalanceId,
    pub held_quantity: PickQuantity,
}

/// Replay-stable result of reporting one pick shortage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportPickShortageResult {
    pub shortage_id: PickShortageId,
    pub shortage_revision: PickShortageRevision,
    pub shortage_status: PickShortageStatus,
    pub task_id: PickTaskId,
    pub content_id: PickContentId,
    pub order_id: OrderId,
    pub order_revision: OrderRevision,
    pub quantities: PickShortageQuantities,
    pub details: PickShortageDetails,
    pub reallocated_quantity: ActualPickQuantity,
    pub recovery_terminal_quantity: ActualPickQuantity,
    pub remaining_to_allocate_quantity: ActualPickQuantity,
    pub observed_item_barcode: Option<PickScanValue>,
    pub observed_lot: Option<PickScanValue>,
    pub observed_serial: Option<PickScanValue>,
    pub hold: PickShortageHoldResult,
    pub movement: Option<PickShortageMovementResult>,
    pub reported_by: UserId,
    pub reported_at: Timestamp,
}

/// Replans an unresolved shortage using the warehouse's FEFO policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReallocatePickShortageCommand {
    pub shortage_id: PickShortageId,
    pub expected_shortage_revision: PickShortageRevision,
    pub expected_order_revision: OrderRevision,
    pub strategy: AllocationStrategy,
}

/// One replacement allocation created under the existing order release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickShortageAllocationReadModel {
    pub allocation_id: InventoryAllocationId,
    pub inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub location_id: LocationId,
    pub location_name: Option<String>,
    pub location_barcode: PickScanValue,
    pub license_plate_id: Option<LicensePlateId>,
    pub license_plate_barcode: Option<PickScanValue>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantity: AllocationQuantity,
    pub execution_stage: AllocationExecutionStage,
}

/// Replacement RF task created for one recovery allocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickShortageTaskReadModel {
    pub task_id: PickTaskId,
    pub content_id: PickContentId,
    pub source_allocation_id: InventoryAllocationId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub source_location_barcode: PickScanValue,
    pub source_license_plate_id: Option<LicensePlateId>,
    pub source_license_plate_barcode: Option<PickScanValue>,
    pub planned_quantity: PickQuantity,
}

/// Replay-stable result of one FEFO shortage-recovery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReallocatePickShortageResult {
    pub reallocation_run_id: PickShortageReallocationRunId,
    pub shortage_id: PickShortageId,
    pub shortage_revision: PickShortageRevision,
    pub shortage_status: PickShortageStatus,
    pub order_id: OrderId,
    pub order_revision: OrderRevision,
    pub strategy: AllocationStrategy,
    pub outcome: AllocationOutcome,
    pub newly_allocated_quantity: ActualPickQuantity,
    pub reallocated_quantity: ActualPickQuantity,
    pub recovery_terminal_quantity: ActualPickQuantity,
    pub remaining_to_allocate_quantity: ActualPickQuantity,
    pub new_allocations: Vec<PickShortageAllocationReadModel>,
    pub new_tasks: Vec<PickShortageTaskReadModel>,
    pub executed_by: UserId,
    pub executed_at: Timestamp,
}

/// Reads one shortage by its path-derived identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickShortageQuery {
    pub shortage_id: PickShortageId,
}

/// Stable keyset boundary for the shortage work queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickShortageCursor {
    pub reported_at: Timestamp,
    pub shortage_id: PickShortageId,
}

/// Scoped filters for the shortage work queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickShortagePageQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub order_id: Option<OrderId>,
    /// `None` selects unresolved shortage work rather than historical rows.
    pub status: Option<PickShortageStatus>,
    pub cursor: Option<PickShortageCursor>,
    pub limit: u16,
}

/// Supervisor-facing shortage state and recovery progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickShortageReadModel {
    pub shortage_id: PickShortageId,
    pub shortage_revision: PickShortageRevision,
    pub status: PickShortageStatus,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub order_id: OrderId,
    pub order_key: String,
    pub order_revision: OrderRevision,
    pub order_line_id: OrderLineId,
    pub task_id: PickTaskId,
    pub content_id: PickContentId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub source_location_barcode: PickScanValue,
    pub source_location_name: Option<String>,
    pub source_license_plate_id: Option<LicensePlateId>,
    pub source_license_plate_barcode: Option<PickScanValue>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantities: PickShortageQuantities,
    pub reallocated_quantity: ActualPickQuantity,
    pub recovery_terminal_quantity: ActualPickQuantity,
    pub remaining_to_allocate_quantity: ActualPickQuantity,
    pub observed_item_barcode: Option<PickScanValue>,
    pub observed_lot: Option<PickScanValue>,
    pub observed_serial: Option<PickScanValue>,
    pub details: PickShortageDetails,
    pub hold: PickShortageHoldResult,
    pub reported_by: UserId,
    pub reported_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
}

impl PickShortageReadModel {
    pub fn recovery_quantities_are_consistent(&self) -> bool {
        recovery_quantities_are_consistent(
            self.status,
            self.quantities.short(),
            self.reallocated_quantity,
            self.recovery_terminal_quantity,
            self.remaining_to_allocate_quantity,
            self.resolved_at.is_some(),
        )
    }
}

fn recovery_quantities_are_consistent(
    status: PickShortageStatus,
    short_quantity: PickQuantity,
    reallocated_quantity: ActualPickQuantity,
    recovery_terminal_quantity: ActualPickQuantity,
    remaining_to_allocate_quantity: ActualPickQuantity,
    is_resolved: bool,
) -> bool {
    recovery_terminal_quantity.get() <= reallocated_quantity.get()
        && reallocated_quantity
            .get()
            .checked_add(remaining_to_allocate_quantity.get())
            == Some(short_quantity.get())
        && match status {
            PickShortageStatus::AwaitingInventory => {
                reallocated_quantity == recovery_terminal_quantity
                    && recovery_terminal_quantity.get() < short_quantity.get()
                    && !is_resolved
            }
            PickShortageStatus::RecoveryInProgress => {
                recovery_terminal_quantity.get() < reallocated_quantity.get() && !is_resolved
            }
            PickShortageStatus::Resolved => {
                remaining_to_allocate_quantity.is_zero()
                    && recovery_terminal_quantity.get() == short_quantity.get()
                    && is_resolved
            }
        }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickShortagePage {
    pub items: Vec<PickShortageReadModel>,
    pub next_cursor: Option<PickShortageCursor>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wareboxes_domain::PickShortageReason;

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

    #[test]
    fn shortage_operations_and_report_shape_are_replay_stable() {
        assert_eq!(REPORT_PICK_SHORTAGE_OPERATION, "picking.shortage.report.v1");
        assert_eq!(
            REALLOCATE_PICK_SHORTAGE_OPERATION,
            "picking.shortage.reallocate.v1"
        );

        let command = ReportPickShortageCommand {
            task_id: PickTaskId::new(7).unwrap(),
            content_id: PickContentId::new(8).unwrap(),
            source_location_barcode: PickScanValue::new("A-01").unwrap(),
            source_license_plate_barcode: Some(PickScanValue::new("LP-1").unwrap()),
            observed_item_barcode: Some(PickScanValue::new("SKU-1").unwrap()),
            observed_lot: Some(PickScanValue::new("LOT-OBSERVED").unwrap()),
            observed_serial: None,
            details: PickShortageDetails::new(PickShortageReason::LotOrSerialMismatch, None)
                .unwrap(),
            outcome: ReportPickShortageOutcome::Partial {
                picked_quantity: PickQuantity::new(2).unwrap(),
                destination_license_plate_barcode: PickScanValue::new("TOTE-1").unwrap(),
            },
        };

        assert_eq!(command.outcome.actual_quantity().get(), 2);
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "task_id": 7,
                "content_id": 8,
                "source_location_barcode": "A-01",
                "source_license_plate_barcode": "LP-1",
                "observed_item_barcode": "SKU-1",
                "observed_lot": "LOT-OBSERVED",
                "observed_serial": null,
                "details": {
                    "reason": "lot_or_serial_mismatch",
                    "note": null
                },
                "outcome": {
                    "kind": "partial",
                    "picked_quantity": 2,
                    "destination_license_plate_barcode": "TOTE-1"
                }
            })
        );
    }

    #[test]
    fn shortage_recovery_status_matches_conserved_cumulative_counters() {
        let short = PickQuantity::new(5).unwrap();
        let quantity = |value| ActualPickQuantity::new(value).unwrap();

        assert!(recovery_quantities_are_consistent(
            PickShortageStatus::AwaitingInventory,
            short,
            quantity(0),
            quantity(0),
            quantity(5),
            false,
        ));
        assert!(recovery_quantities_are_consistent(
            PickShortageStatus::AwaitingInventory,
            short,
            quantity(2),
            quantity(2),
            quantity(3),
            false,
        ));
        assert!(recovery_quantities_are_consistent(
            PickShortageStatus::RecoveryInProgress,
            short,
            quantity(3),
            quantity(1),
            quantity(2),
            false,
        ));
        assert!(recovery_quantities_are_consistent(
            PickShortageStatus::Resolved,
            short,
            quantity(5),
            quantity(5),
            quantity(0),
            true,
        ));
        assert!(!recovery_quantities_are_consistent(
            PickShortageStatus::Resolved,
            short,
            quantity(4),
            quantity(4),
            quantity(1),
            true,
        ));
        assert!(!recovery_quantities_are_consistent(
            PickShortageStatus::AwaitingInventory,
            short,
            quantity(2),
            quantity(1),
            quantity(3),
            false,
        ));
        assert!(!recovery_quantities_are_consistent(
            PickShortageStatus::RecoveryInProgress,
            short,
            quantity(2),
            quantity(3),
            quantity(3),
            false,
        ));
    }

    #[test]
    fn shortage_reallocation_uses_a_distinct_run_identity() {
        let run_id = PickShortageReallocationRunId::new(41).unwrap();
        assert_eq!(run_id.get(), 41);
    }
}
