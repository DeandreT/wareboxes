//! Application contracts for typed RF picking and claim lifecycle commands.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    ActualPickQuantity, AllocationExecutionStage, AllocationOutcome, AllocationQuantity,
    AllocationStrategy, FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryHoldId,
    InventoryOwnerId, ItemBatchId, LicensePlateId, LocationId, OrderId, OrderLineId, OrderRevision,
    OrderStatus, PickClaimReleaseReason, PickClusterId, PickConfirmationId, PickContentId,
    PickContentState, PickExecutionMethod, PickQuantity, PickReversalDetails, PickReversalId,
    PickReversalNote, PickReversalReason, PickScanValue, PickShortShipDetails, PickShortShipNote,
    PickShortShipReason, PickShortageDetails, PickShortageDispositionId, PickShortageId,
    PickShortageQuantities, PickShortageReallocationRunId, PickShortageResolution,
    PickShortageRevision, PickShortageStatus, PickTaskId, ShortShipDemandQuantities, Timestamp,
    UserId,
};

use crate::order_allocation::AllocationPolicyReadModel;
use crate::picking_decision_policy::PickDecisionPolicyReadModel;

pub const REPORT_PICK_SHORTAGE_OPERATION: &str = "picking.shortage.report.v1";
pub const REVERSE_PICK_CONFIRMATION_OPERATION: &str = "picking.confirmation.reverse.v1";
pub const REALLOCATE_PICK_SHORTAGE_OPERATION: &str = "picking.shortage.reallocate.v1";
pub const ACCEPT_PICK_SHORTAGE_AS_SHORT_SHIP_OPERATION: &str =
    "picking.shortage.accept_short_ship.v1";

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
    pub execution: PickExecutionEvidence,
    pub pick_policy: PickDecisionPolicyReadModel,
    /// Present only when the task's existing outbound container is unambiguous.
    pub suggested_destination_license_plate_barcode: Option<PickScanValue>,
    pub content: PickClaimContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickExecutionEvidence {
    pub method: PickExecutionMethod,
    pub cluster_id: Option<PickClusterId>,
    pub cart_barcode: Option<String>,
    pub slot_code: Option<String>,
    pub sequence: Option<i64>,
    pub task_count: Option<i64>,
}

impl PickExecutionEvidence {
    pub const fn discrete() -> Self {
        Self {
            method: PickExecutionMethod::Discrete,
            cluster_id: None,
            cart_barcode: None,
            slot_code: None,
            sequence: None,
            task_count: None,
        }
    }

    pub const fn case() -> Self {
        Self {
            method: PickExecutionMethod::Case,
            cluster_id: None,
            cart_barcode: None,
            slot_code: None,
            sequence: None,
            task_count: None,
        }
    }

    pub const fn pallet() -> Self {
        Self {
            method: PickExecutionMethod::Pallet,
            cluster_id: None,
            cart_barcode: None,
            slot_code: None,
            sequence: None,
            task_count: None,
        }
    }
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
    pub source_location_barcode: Option<PickScanValue>,
    pub item_barcode: Option<PickScanValue>,
    pub source_license_plate_barcode: Option<PickScanValue>,
    pub destination_license_plate_barcode: Option<PickScanValue>,
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
    pub pick_policy: PickDecisionPolicyReadModel,
    pub source_location_scan_verified: bool,
    pub item_scan_verified: bool,
    pub destination_container_scan_verified: bool,
    pub picked_quantity: PickQuantity,
    pub confirmed_by: UserId,
    pub confirmed_at: Timestamp,
    pub content_state: PickContentState,
    pub task_completed: bool,
    pub order_ready_to_pack: bool,
    pub order_status: wareboxes_domain::OrderStatus,
    pub order_revision: OrderRevision,
}

/// Scan-verified supervisor command that returns one completed pick to RF work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReversePickConfirmationCommand {
    pub confirmation_id: PickConfirmationId,
    pub expected_order_revision: OrderRevision,
    pub staged_location_barcode: PickScanValue,
    pub staged_license_plate_barcode: PickScanValue,
    pub item_barcode: PickScanValue,
    pub lot_scan: Option<PickScanValue>,
    pub serial_scan: Option<PickScanValue>,
    pub return_location_barcode: PickScanValue,
    pub return_license_plate_barcode: Option<PickScanValue>,
    pub reason: PickReversalReason,
    pub note: Option<PickReversalNote>,
}

impl ReversePickConfirmationCommand {
    pub fn validate_details(&self) -> Result<PickReversalDetails, wareboxes_domain::PickingError> {
        PickReversalDetails::new(self.reason, self.note.clone())
    }
}

/// Replay-stable evidence of one equal-and-opposite pick movement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReversePickConfirmationResult {
    pub reversal_id: PickReversalId,
    pub confirmation_id: PickConfirmationId,
    pub task_id: PickTaskId,
    pub content_id: PickContentId,
    pub order_id: OrderId,
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: InventoryAllocationId,
    pub staged_inventory_allocation_id: InventoryAllocationId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub staged_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub staged_location_id: LocationId,
    pub source_license_plate_id: Option<LicensePlateId>,
    pub staged_license_plate_id: LicensePlateId,
    pub reversed_quantity: PickQuantity,
    pub content_state: PickContentState,
    pub order_status: OrderStatus,
    pub order_revision: OrderRevision,
    pub reason: PickReversalReason,
    pub note: Option<PickReversalNote>,
    pub reversed_by: UserId,
    pub reversed_at: Timestamp,
}

/// Stable keyset boundary for one order's pick-confirmation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickConfirmationHistoryCursor {
    pub confirmed_at: Timestamp,
    pub confirmation_id: PickConfirmationId,
}

/// Bounded query for immutable pick confirmations and optional reversals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickConfirmationHistoryQuery {
    pub order_id: OrderId,
    pub cursor: Option<PickConfirmationHistoryCursor>,
    pub limit: u16,
}

/// Reversal evidence paired with its original confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickReversalHistoryReadModel {
    pub reversal_id: PickReversalId,
    pub reason: PickReversalReason,
    pub note: Option<PickReversalNote>,
    pub reversed_by: UserId,
    pub reversed_at: Timestamp,
}

/// Manager-facing execution history for one physical pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickConfirmationHistoryReadModel {
    pub confirmation_id: PickConfirmationId,
    pub task_id: PickTaskId,
    pub content_id: PickContentId,
    pub order_id: OrderId,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub picked_quantity: PickQuantity,
    pub source_location_id: LocationId,
    pub source_location_name: String,
    pub source_license_plate_required: bool,
    pub staged_location_id: LocationId,
    pub staged_location_name: String,
    pub staged_license_plate_id: LicensePlateId,
    pub pick_policy: PickDecisionPolicyReadModel,
    pub source_location_scan_verified: bool,
    pub item_scan_verified: bool,
    pub destination_container_scan_verified: bool,
    pub confirmed_by: UserId,
    pub confirmed_at: Timestamp,
    pub reversal: Option<PickReversalHistoryReadModel>,
}

/// One keyset page of pick-confirmation history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickConfirmationHistoryPage {
    pub items: Vec<PickConfirmationHistoryReadModel>,
    pub next_cursor: Option<PickConfirmationHistoryCursor>,
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

/// Replans an unresolved shortage using the effective warehouse allocation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReallocatePickShortageCommand {
    pub shortage_id: PickShortageId,
    pub expected_shortage_revision: PickShortageRevision,
    pub expected_order_revision: OrderRevision,
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

/// Replay-stable result of one policy-driven shortage-recovery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReallocatePickShortageResult {
    pub reallocation_run_id: PickShortageReallocationRunId,
    pub shortage_id: PickShortageId,
    pub shortage_revision: PickShortageRevision,
    pub shortage_status: PickShortageStatus,
    pub order_id: OrderId,
    pub order_revision: OrderRevision,
    pub policy: AllocationPolicyReadModel,
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

/// Resolves the server-derived unmet quantity as an authorized short shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcceptPickShortageAsShortShipCommand {
    shortage_id: PickShortageId,
    expected_shortage_revision: PickShortageRevision,
    expected_order_revision: OrderRevision,
    reason: PickShortShipReason,
    note: Option<PickShortShipNote>,
}

impl AcceptPickShortageAsShortShipCommand {
    pub fn new(
        shortage_id: PickShortageId,
        expected_shortage_revision: PickShortageRevision,
        expected_order_revision: OrderRevision,
        reason: PickShortShipReason,
        note: Option<PickShortShipNote>,
    ) -> Result<Self, wareboxes_domain::PickingError> {
        let details = PickShortShipDetails::new(reason, note)?;
        Ok(Self {
            shortage_id,
            expected_shortage_revision,
            expected_order_revision,
            reason: details.reason(),
            note: details.into_note(),
        })
    }

    pub const fn shortage_id(&self) -> PickShortageId {
        self.shortage_id
    }

    pub const fn expected_shortage_revision(&self) -> PickShortageRevision {
        self.expected_shortage_revision
    }

    pub const fn expected_order_revision(&self) -> OrderRevision {
        self.expected_order_revision
    }

    pub const fn reason(&self) -> PickShortShipReason {
        self.reason
    }

    pub fn note(&self) -> Option<&PickShortShipNote> {
        self.note.as_ref()
    }
}

/// Replay-stable result of accepting one shortage as a short shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptPickShortageAsShortShipResult {
    pub disposition_id: PickShortageDispositionId,
    pub shortage_id: PickShortageId,
    pub previous_shortage_status: PickShortageStatus,
    pub shortage_status: PickShortageStatus,
    pub shortage_resolution: PickShortageResolution,
    pub shortage_revision: PickShortageRevision,
    pub order_id: OrderId,
    pub order_line_id: OrderLineId,
    pub previous_order_status: OrderStatus,
    pub order_status: OrderStatus,
    pub order_revision: OrderRevision,
    pub order_ready_to_pack: bool,
    pub shortage_quantities: PickShortageQuantities,
    pub reallocated_quantity: ActualPickQuantity,
    pub recovery_terminal_quantity: ActualPickQuantity,
    pub accepted_short_quantity: PickQuantity,
    pub line_demand: ShortShipDemandQuantities,
    pub order_demand: ShortShipDemandQuantities,
    pub inventory_hold_id: InventoryHoldId,
    pub reason: PickShortShipReason,
    pub note: Option<PickShortShipNote>,
    pub resolved_by: UserId,
    pub resolved_at: Timestamp,
}

/// Reads one shortage by its path-derived identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickShortageQuery {
    pub shortage_id: PickShortageId,
}

/// Scoped filters for the shortage work queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickShortagePageQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub order_id: Option<OrderId>,
    pub order_key: Option<wareboxes_domain::OrderKey>,
    /// `None` selects unresolved shortage work rather than historical rows.
    pub status: Option<PickShortageStatus>,
    pub offset: u64,
    pub limit: u16,
    pub sort: PickShortageQueueSort,
    pub direction: PickShortageQueueSortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickShortageQueueSort {
    Reported,
    Order,
    Status,
    ShortQuantity,
    RemainingQuantity,
    InventoryOwner,
    Item,
    Facility,
}

impl PickShortageQueueSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Order => "order",
            Self::Status => "status",
            Self::ShortQuantity => "short_quantity",
            Self::RemainingQuantity => "remaining_quantity",
            Self::InventoryOwner => "inventory_owner",
            Self::Item => "item",
            Self::Facility => "facility",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickShortageQueueSortDirection {
    Ascending,
    Descending,
}

impl PickShortageQueueSortDirection {
    pub const fn is_ascending(self) -> bool {
        matches!(self, Self::Ascending)
    }
}

/// Supervisor-facing shortage state and recovery progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickShortageReadModel {
    pub shortage_id: PickShortageId,
    pub shortage_revision: PickShortageRevision,
    pub status: PickShortageStatus,
    pub resolution: Option<PickShortageResolution>,
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
    pub accepted_short_quantity: ActualPickQuantity,
    pub accepted_substitute_quantity: ActualPickQuantity,
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
        shortage_recovery_quantities_are_consistent(self)
    }
}

fn shortage_recovery_quantities_are_consistent(shortage: &PickShortageReadModel) -> bool {
    let short_quantity = shortage.quantities.short();
    let reallocated_quantity = shortage.reallocated_quantity;
    let recovery_terminal_quantity = shortage.recovery_terminal_quantity;
    let remaining_to_allocate_quantity = shortage.remaining_to_allocate_quantity;
    let accepted_short_quantity = shortage.accepted_short_quantity;
    let accepted_substitute_quantity = shortage.accepted_substitute_quantity;
    recovery_terminal_quantity.get() <= reallocated_quantity.get()
        && reallocated_quantity
            .get()
            .checked_add(remaining_to_allocate_quantity.get())
            == Some(short_quantity.get())
        && match shortage.status {
            PickShortageStatus::AwaitingInventory => {
                shortage.resolution.is_none()
                    && accepted_short_quantity.is_zero()
                    && accepted_substitute_quantity.is_zero()
                    && reallocated_quantity == recovery_terminal_quantity
                    && recovery_terminal_quantity.get() < short_quantity.get()
                    && shortage.resolved_at.is_none()
            }
            PickShortageStatus::RecoveryInProgress => {
                shortage.resolution.is_none()
                    && accepted_short_quantity.is_zero()
                    && accepted_substitute_quantity.is_zero()
                    && recovery_terminal_quantity.get() < reallocated_quantity.get()
                    && shortage.resolved_at.is_none()
            }
            PickShortageStatus::Resolved => match shortage.resolution {
                Some(PickShortageResolution::Recovered) => {
                    accepted_short_quantity.is_zero()
                        && accepted_substitute_quantity.is_zero()
                        && remaining_to_allocate_quantity.is_zero()
                        && recovery_terminal_quantity.get() == short_quantity.get()
                        && shortage.resolved_at.is_some()
                }
                Some(PickShortageResolution::ShortShip) => {
                    !accepted_short_quantity.is_zero()
                        && accepted_substitute_quantity.is_zero()
                        && accepted_short_quantity == remaining_to_allocate_quantity
                        && recovery_terminal_quantity == reallocated_quantity
                        && shortage.resolved_at.is_some()
                }
                Some(PickShortageResolution::Substituted) => {
                    accepted_short_quantity.is_zero()
                        && !accepted_substitute_quantity.is_zero()
                        && accepted_substitute_quantity == remaining_to_allocate_quantity
                        && recovery_terminal_quantity == reallocated_quantity
                        && shortage.resolved_at.is_some()
                }
                None => false,
            },
        }
}

#[cfg(test)]
fn recovery_quantities_are_consistent(
    status: PickShortageStatus,
    short_quantity: PickQuantity,
    reallocated_quantity: ActualPickQuantity,
    recovery_terminal_quantity: ActualPickQuantity,
    remaining_to_allocate_quantity: ActualPickQuantity,
    is_resolved: bool,
) -> bool {
    let resolution =
        matches!(status, PickShortageStatus::Resolved).then_some(PickShortageResolution::Recovered);
    recovery_terminal_quantity.get() <= reallocated_quantity.get()
        && reallocated_quantity
            .get()
            .checked_add(remaining_to_allocate_quantity.get())
            == Some(short_quantity.get())
        && match status {
            PickShortageStatus::AwaitingInventory => {
                resolution.is_none()
                    && reallocated_quantity == recovery_terminal_quantity
                    && recovery_terminal_quantity.get() < short_quantity.get()
                    && !is_resolved
            }
            PickShortageStatus::RecoveryInProgress => {
                resolution.is_none()
                    && recovery_terminal_quantity.get() < reallocated_quantity.get()
                    && !is_resolved
            }
            PickShortageStatus::Resolved => {
                resolution == Some(PickShortageResolution::Recovered)
                    && remaining_to_allocate_quantity.is_zero()
                    && recovery_terminal_quantity.get() == short_quantity.get()
                    && is_resolved
            }
        }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickShortagePage {
    pub items: Vec<PickShortageReadModel>,
    pub next_offset: Option<u64>,
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
            source_location_barcode: Some(PickScanValue::new("A-01").unwrap()),
            item_barcode: Some(PickScanValue::new("SKU-1").unwrap()),
            source_license_plate_barcode: Some(PickScanValue::new("LP-1").unwrap()),
            destination_license_plate_barcode: Some(PickScanValue::new("TOTE-1").unwrap()),
        };

        assert_eq!(command.task_id.get(), 7);
        assert_eq!(command.content_id.get(), 8);
        assert_eq!(
            command.source_location_barcode.as_ref().unwrap().as_str(),
            "A-01"
        );
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
        assert_eq!(
            ACCEPT_PICK_SHORTAGE_AS_SHORT_SHIP_OPERATION,
            "picking.shortage.accept_short_ship.v1"
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

    #[test]
    fn short_ship_command_is_revisioned_and_does_not_accept_a_quantity() {
        let command = AcceptPickShortageAsShortShipCommand::new(
            PickShortageId::new(21).unwrap(),
            PickShortageRevision::new(3).unwrap(),
            OrderRevision::new(8).unwrap(),
            PickShortShipReason::ClientAuthorized,
            Some(PickShortShipNote::new("Client approved the reduced shipment").unwrap()),
        )
        .unwrap();

        assert_eq!(command.shortage_id().get(), 21);
        assert_eq!(command.expected_shortage_revision().get(), 3);
        assert_eq!(command.expected_order_revision().get(), 8);
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "shortage_id": 21,
                "expected_shortage_revision": 3,
                "expected_order_revision": 8,
                "reason": "client_authorized",
                "note": "Client approved the reduced shipment"
            })
        );
        assert_eq!(
            AcceptPickShortageAsShortShipCommand::new(
                PickShortageId::new(21).unwrap(),
                PickShortageRevision::new(3).unwrap(),
                OrderRevision::new(8).unwrap(),
                PickShortShipReason::Other,
                None,
            ),
            Err(wareboxes_domain::PickingError::ShortShipNoteRequired)
        );
    }

    #[test]
    fn short_ship_result_carries_effective_line_and_order_demand() {
        let result = AcceptPickShortageAsShortShipResult {
            disposition_id: PickShortageDispositionId::new(31).unwrap(),
            shortage_id: PickShortageId::new(21).unwrap(),
            previous_shortage_status: PickShortageStatus::AwaitingInventory,
            shortage_status: PickShortageStatus::Resolved,
            shortage_resolution: PickShortageResolution::ShortShip,
            shortage_revision: PickShortageRevision::new(4).unwrap(),
            order_id: OrderId::new(11).unwrap(),
            order_line_id: OrderLineId::new(12).unwrap(),
            previous_order_status: OrderStatus::Processing,
            order_status: OrderStatus::AwaitingPacking,
            order_revision: OrderRevision::new(9).unwrap(),
            order_ready_to_pack: true,
            shortage_quantities: PickShortageQuantities::new(
                PickQuantity::new(5).unwrap(),
                ActualPickQuantity::new(2).unwrap(),
            )
            .unwrap(),
            reallocated_quantity: ActualPickQuantity::ZERO,
            recovery_terminal_quantity: ActualPickQuantity::ZERO,
            accepted_short_quantity: PickQuantity::new(3).unwrap(),
            line_demand: ShortShipDemandQuantities::new(
                PickQuantity::new(5).unwrap(),
                ActualPickQuantity::new(3).unwrap(),
            )
            .unwrap(),
            order_demand: ShortShipDemandQuantities::new(
                PickQuantity::new(12).unwrap(),
                ActualPickQuantity::new(3).unwrap(),
            )
            .unwrap(),
            inventory_hold_id: InventoryHoldId::new(41).unwrap(),
            reason: PickShortShipReason::InventoryUnavailable,
            note: None,
            resolved_by: UserId::new(51).unwrap(),
            resolved_at: "2026-08-08T20:00:00Z".parse().unwrap(),
        };

        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(encoded["accepted_short_quantity"], 3);
        assert_eq!(encoded["line_demand"]["effective"], 2);
        assert_eq!(encoded["order_demand"]["effective"], 9);
        assert_eq!(
            serde_json::from_value::<AcceptPickShortageAsShortShipResult>(encoded).unwrap(),
            result
        );
    }
}
