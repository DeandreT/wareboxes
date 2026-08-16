use serde::{Deserialize, Serialize};

use super::{
    AllocationPolicyResponse, CursorPage, OpaqueCursor, OrderAllocationOutcome,
    OrderAllocationStrategy, PageLimit, Revision,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextPickRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimPickByIdRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatPickClaimRequest {}

/// Operator reason for returning active pick work to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SourceBlocked,
    InventoryDiscrepancy,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePickClaimRequest {
    pub reason: PickClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClaimHeartbeatResponse {
    pub task_id: i64,
    pub heartbeat_at: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClaimReleaseResponse {
    pub task_id: i64,
    pub released_at: String,
    pub release_count: i64,
    pub reason: PickClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Completion state of one allocation-backed pick content record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickContentState {
    Pending,
    Completed,
    Shorted,
}

/// Order states observable after an individual pick confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickOrderStatus {
    Processing,
    AwaitingPacking,
}

/// One scanner-ready allocation in a claimed pick task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClaimContent {
    pub content_id: i64,
    pub order_line_id: i64,
    pub inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub source_license_plate_id: Option<i64>,
    pub source_license_plate_barcode: Option<String>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub planned_quantity: i64,
    pub state: PickContentState,
}

/// Active typed pick claim without persistence or tenant metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClaimResponse {
    pub task_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub order_key: String,
    pub order_revision: Revision,
    pub priority: i64,
    pub ship_by: Option<String>,
    pub lease_expires_at: String,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub destination_location_name: Option<String>,
    pub content: PickClaimContent,
}

/// The current claim is absent when the RF identity owns no active pick work.
pub type CurrentPickResponse = Option<PickClaimResponse>;

/// Confirms the immutable planned quantity using scanned source and tote identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmPickContentRequest {
    pub source_location_barcode: String,
    pub item_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_license_plate_barcode: Option<String>,
    pub destination_license_plate_barcode: String,
}

/// Result of atomically moving one pick and advancing its workflow state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickContentConfirmationResponse {
    pub result_id: i64,
    pub content_id: i64,
    pub task_id: i64,
    pub order_id: i64,
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: i64,
    pub destination_inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub source_license_plate_id: Option<i64>,
    pub destination_license_plate_id: i64,
    pub picked_quantity: i64,
    pub confirmed_by: i64,
    pub confirmed_at: String,
    pub content_state: PickContentState,
    pub task_completed: bool,
    pub order_ready_to_pack: bool,
    pub order_status: PickOrderStatus,
    pub order_revision: Revision,
}

/// Supervisor reason retained with an immutable pick-reversal record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickReversalReason {
    MisPick,
    WrongQuantity,
    WrongLotOrSerial,
    DamagedDuringPick,
    OrderException,
    Other,
}

/// Exact scans required to reverse one completed pick before packing begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReversePickConfirmationRequest {
    pub expected_order_revision: Revision,
    pub staged_location_barcode: String,
    pub staged_license_plate_barcode: String,
    pub item_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lot_scan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_scan: Option<String>,
    pub return_location_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_license_plate_barcode: Option<String>,
    pub reason: PickReversalReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Equal-and-opposite movement and reopened RF work produced by a reversal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReversePickConfirmationResponse {
    pub reversal_id: i64,
    pub confirmation_id: i64,
    pub task_id: i64,
    pub content_id: i64,
    pub order_id: i64,
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: i64,
    pub staged_inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub staged_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub staged_location_id: i64,
    pub source_license_plate_id: Option<i64>,
    pub staged_license_plate_id: i64,
    pub reversed_quantity: i64,
    pub content_state: PickContentState,
    pub order_status: PickOrderStatus,
    pub order_revision: Revision,
    pub reason: PickReversalReason,
    pub note: Option<String>,
    pub reversed_by: i64,
    pub reversed_at: String,
}

/// Immutable reversal evidence attached to one confirmation-history row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickReversalHistoryResponse {
    pub reversal_id: i64,
    pub reason: PickReversalReason,
    pub note: Option<String>,
    pub reversed_by: i64,
    pub reversed_at: String,
}

/// One physical pick confirmation shown in an order's fulfillment history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickConfirmationHistoryResponse {
    pub confirmation_id: i64,
    pub task_id: i64,
    pub content_id: i64,
    pub order_id: i64,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub picked_quantity: i64,
    pub source_location_id: i64,
    pub source_location_name: String,
    pub source_license_plate_required: bool,
    pub staged_location_id: i64,
    pub staged_location_name: String,
    pub staged_license_plate_id: i64,
    pub confirmed_by: i64,
    pub confirmed_at: String,
    pub reversal: Option<PickReversalHistoryResponse>,
}

pub type PickConfirmationHistoryPageRequest = super::CursorPageRequest;
pub type PickConfirmationHistoryPage = super::CursorPage<PickConfirmationHistoryResponse>;

/// Physical reason an operator could not complete the directed quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageReason {
    InventoryMissing,
    InsufficientQuantity,
    DamagedInventory,
    WrongInventory,
    LotOrSerialMismatch,
    Other,
}

/// Lifecycle state of a pick-shortage exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageStatus {
    AwaitingInventory,
    RecoveryInProgress,
    Resolved,
}

/// Durable terminal outcome of a resolved pick-shortage exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageResolution {
    Recovered,
    ShortShip,
    Substituted,
}

/// Business reason for accepting unmet demand as a short shipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortShipReason {
    ClientAuthorized,
    InventoryUnavailable,
    ShipByCommitment,
    Other,
}

/// Physical execution stage of a replacement allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationExecutionStage {
    PickSource,
    Staged,
    Packed,
}

/// Operator-supplied shortage reason and bounded context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickShortageDetails {
    pub reason: PickShortageReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Physical outcome reported by the operator for a short pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReportPickShortageOutcome {
    NoPick {},
    Partial {
        picked_quantity: i64,
        destination_license_plate_barcode: String,
    },
}

/// Scanner evidence and physical outcome for one active claimed pick line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportPickShortageRequest {
    pub source_location_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_license_plate_barcode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_item_barcode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_lot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_serial: Option<String>,
    pub details: PickShortageDetails,
    pub outcome: ReportPickShortageOutcome,
}

/// Conserved quantities recorded for one shortage exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickShortageQuantitiesResponse {
    pub planned: i64,
    pub picked: i64,
    pub short: i64,
}

/// Quantity hold created for the physically short source stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickShortageHoldResponse {
    pub hold_id: i64,
    pub inventory_balance_id: i64,
    pub held_quantity: i64,
}

/// Inventory movement committed for a nonzero partial pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickShortageMovementResponse {
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: i64,
    pub destination_inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub source_license_plate_id: Option<i64>,
    pub destination_license_plate_id: i64,
    pub picked_quantity: i64,
    pub destination_stage: AllocationExecutionStage,
}

/// Replay-stable result of reporting one pick shortage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportPickShortageResponse {
    pub shortage_id: i64,
    pub shortage_revision: Revision,
    pub shortage_status: PickShortageStatus,
    pub task_id: i64,
    pub content_id: i64,
    pub order_id: i64,
    pub order_revision: Revision,
    pub quantities: PickShortageQuantitiesResponse,
    pub details: PickShortageDetails,
    pub reallocated_quantity: i64,
    pub recovery_terminal_quantity: i64,
    pub remaining_to_allocate_quantity: i64,
    pub observed_item_barcode: Option<String>,
    pub observed_lot: Option<String>,
    pub observed_serial: Option<String>,
    pub hold: PickShortageHoldResponse,
    pub movement: Option<PickShortageMovementResponse>,
    pub reported_by: i64,
    pub reported_at: String,
}

/// Optimistic policy-driven recovery command for one unresolved shortage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReallocatePickShortageRequest {
    pub expected_shortage_revision: Revision,
    pub expected_order_revision: Revision,
}

/// One replacement allocation created under the existing order release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickShortageAllocationResponse {
    pub allocation_id: i64,
    pub inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: String,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub quantity: i64,
    pub execution_stage: AllocationExecutionStage,
}

/// Replacement RF task created for one recovery allocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickShortageTaskResponse {
    pub task_id: i64,
    pub content_id: i64,
    pub source_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_license_plate_id: Option<i64>,
    pub source_license_plate_barcode: Option<String>,
    pub planned_quantity: i64,
}

/// Replay-stable result of one policy-driven shortage-recovery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReallocatePickShortageResponse {
    pub reallocation_run_id: i64,
    pub shortage_id: i64,
    pub shortage_revision: Revision,
    pub shortage_status: PickShortageStatus,
    pub order_id: i64,
    pub order_revision: Revision,
    pub policy: AllocationPolicyResponse,
    pub strategy: OrderAllocationStrategy,
    pub outcome: OrderAllocationOutcome,
    pub newly_allocated_quantity: i64,
    pub reallocated_quantity: i64,
    pub recovery_terminal_quantity: i64,
    pub remaining_to_allocate_quantity: i64,
    pub new_allocations: Vec<PickShortageAllocationResponse>,
    pub new_tasks: Vec<PickShortageTaskResponse>,
    pub executed_by: i64,
    pub executed_at: String,
}

/// Optimistic request to accept the server-derived unmet quantity for shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptPickShortageAsShortShipRequest {
    pub expected_shortage_revision: Revision,
    pub expected_order_revision: Revision,
    pub reason: PickShortShipReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Conserved original, accepted-short, and effective executable demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShortShipDemandResponse {
    pub ordered: i64,
    pub accepted_short: i64,
    pub accepted_substitute: i64,
    pub effective: i64,
}

/// Replay-stable result of accepting one shortage as a short shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptPickShortageAsShortShipResponse {
    pub disposition_id: i64,
    pub shortage_id: i64,
    pub previous_shortage_status: PickShortageStatus,
    pub shortage_status: PickShortageStatus,
    pub shortage_resolution: PickShortageResolution,
    pub shortage_revision: Revision,
    pub order_id: i64,
    pub order_line_id: i64,
    pub previous_order_status: PickOrderStatus,
    pub order_status: PickOrderStatus,
    pub order_revision: Revision,
    pub order_ready_to_pack: bool,
    pub shortage_quantities: PickShortageQuantitiesResponse,
    pub reallocated_quantity: i64,
    pub recovery_terminal_quantity: i64,
    pub accepted_short_quantity: i64,
    pub line_demand: ShortShipDemandResponse,
    pub order_demand: ShortShipDemandResponse,
    pub inventory_hold_id: i64,
    pub reason: PickShortShipReason,
    pub note: Option<String>,
    pub resolved_by: i64,
    pub resolved_at: String,
}

/// Scoped filters for the supervisor shortage work queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageQueueSort {
    #[default]
    Reported,
    Order,
    Status,
    ShortQuantity,
    RemainingQuantity,
    InventoryOwner,
    Item,
    Facility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageQueueSortDirection {
    Ascending,
    #[default]
    Descending,
}

/// Scoped filters for the supervisor shortage work queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PickShortagePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<i64>,
    /// Exact business-facing order key. Requests must not combine this with `order_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_key: Option<String>,
    /// When omitted, the queue returns unresolved shortage work only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PickShortageStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default)]
    pub sort: PickShortageQueueSort,
    #[serde(default)]
    pub direction: PickShortageQueueSortDirection,
}

/// Supervisor-facing shortage state and recovery progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickShortageResponse {
    pub shortage_id: i64,
    pub shortage_revision: Revision,
    pub status: PickShortageStatus,
    pub resolution: Option<PickShortageResolution>,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub order_id: i64,
    pub order_key: String,
    pub order_revision: Revision,
    pub order_line_id: i64,
    pub task_id: i64,
    pub content_id: i64,
    pub source_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub source_license_plate_id: Option<i64>,
    pub source_license_plate_barcode: Option<String>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub quantities: PickShortageQuantitiesResponse,
    pub reallocated_quantity: i64,
    pub recovery_terminal_quantity: i64,
    pub remaining_to_allocate_quantity: i64,
    pub accepted_short_quantity: i64,
    pub accepted_substitute_quantity: i64,
    pub observed_item_barcode: Option<String>,
    pub observed_lot: Option<String>,
    pub observed_serial: Option<String>,
    pub details: PickShortageDetails,
    pub hold: PickShortageHoldResponse,
    pub reported_by: i64,
    pub reported_at: String,
    pub resolved_at: Option<String>,
}

pub type PickShortagePage = CursorPage<PickShortageResponse>;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn claim_lifecycle_requests_are_empty_or_typed_and_strict() {
        assert_eq!(
            serde_json::from_value::<ClaimNextPickRequest>(json!({})).unwrap(),
            ClaimNextPickRequest {}
        );
        assert_eq!(
            serde_json::from_value::<ClaimPickByIdRequest>(json!({})).unwrap(),
            ClaimPickByIdRequest {}
        );
        assert!(serde_json::from_value::<ClaimPickByIdRequest>(json!({
            "task_id": 3
        }))
        .is_err());
        assert!(serde_json::from_value::<HeartbeatPickClaimRequest>(json!({
            "lease_seconds": 600
        }))
        .is_err());
        assert_eq!(
            serde_json::from_value::<ReleasePickClaimRequest>(json!({
                "reason": "inventory_discrepancy",
                "note": "Stock is missing"
            }))
            .unwrap(),
            ReleasePickClaimRequest {
                reason: PickClaimReleaseReason::InventoryDiscrepancy,
                note: Some("Stock is missing".into()),
            }
        );
    }

    #[test]
    fn confirmation_accepts_scans_but_not_a_client_selected_quantity() {
        let request = serde_json::from_value::<ConfirmPickContentRequest>(json!({
            "source_location_barcode": "A-01",
            "item_barcode": "SKU-1",
            "source_license_plate_barcode": "LP-1",
            "destination_license_plate_barcode": "TOTE-1"
        }))
        .unwrap();
        assert_eq!(request.source_location_barcode, "A-01");

        assert!(serde_json::from_value::<ConfirmPickContentRequest>(json!({
            "source_location_barcode": "A-01",
            "item_barcode": "SKU-1",
            "source_license_plate_barcode": "LP-1",
            "destination_license_plate_barcode": "TOTE-1",
            "picked_quantity": 4
        }))
        .is_err());
    }

    #[test]
    fn reversal_is_revisioned_scan_only_and_strict() {
        let request = serde_json::from_value::<ReversePickConfirmationRequest>(json!({
            "expected_order_revision": 4,
            "staged_location_barcode": "STAGE-01",
            "staged_license_plate_barcode": "TOTE-1",
            "item_barcode": "SKU-1",
            "lot_scan": "LOT-1",
            "return_location_barcode": "A-01",
            "return_license_plate_barcode": "LP-1",
            "reason": "mis_pick"
        }))
        .unwrap();
        assert_eq!(request.reason, PickReversalReason::MisPick);
        assert!(
            serde_json::from_value::<ReversePickConfirmationRequest>(json!({
                "expected_order_revision": 4,
                "staged_location_barcode": "STAGE-01",
                "staged_license_plate_barcode": "TOTE-1",
                "item_barcode": "SKU-1",
                "return_location_barcode": "A-01",
                "reason": "mis_pick",
                "quantity": 1
            }))
            .is_err()
        );
    }

    #[test]
    fn claim_is_one_allocation_backed_work_item() {
        let claim = PickClaimResponse {
            task_id: 1,
            order_id: 2,
            inventory_owner_id: 3,
            facility_id: 4,
            order_key: "ORDER-2".into(),
            order_revision: Revision::new(3).unwrap(),
            priority: 80,
            ship_by: Some("2026-08-09T20:00:00Z".into()),
            lease_expires_at: "2026-08-08T20:30:00Z".into(),
            destination_location_id: 5,
            destination_location_barcode: "PACK-01".into(),
            destination_location_name: Some("Pack lane 1".into()),
            content: PickClaimContent {
                content_id: 6,
                order_line_id: 7,
                inventory_allocation_id: 8,
                source_inventory_balance_id: 9,
                item_batch_id: 10,
                source_location_id: 11,
                source_location_barcode: "A-01".into(),
                source_location_name: Some("Forward A-01".into()),
                source_license_plate_id: Some(12),
                source_license_plate_barcode: Some("LP-12".into()),
                item_id: 13,
                item_description: Some("Widget".into()),
                item_barcodes: vec!["SKU-1".into(), "000123".into()],
                uom: "each".into(),
                lot: Some("LOT-1".into()),
                serial: None,
                expiration: Some("2027-01-01T00:00:00Z".into()),
                planned_quantity: 4,
                state: PickContentState::Pending,
            },
        };

        let value = serde_json::to_value(claim).unwrap();
        assert_eq!(value["content"]["inventory_allocation_id"], 8);
        assert_eq!(value["content"]["item_barcodes"][1], "000123");
        assert!(value.get("tenant_id").is_none());
        assert!(value.get("metadata_json").is_none());
        assert!(value.get("contents").is_none());
    }

    #[test]
    fn short_pick_report_is_strict_and_preserves_observed_evidence() {
        let request = serde_json::from_value::<ReportPickShortageRequest>(json!({
            "source_location_barcode": "A-01",
            "source_license_plate_barcode": "LP-12",
            "observed_item_barcode": "SKU-OBSERVED",
            "observed_lot": "LOT-OBSERVED",
            "observed_serial": null,
            "details": {
                "reason": "lot_or_serial_mismatch",
                "note": "Directed lot was not present"
            },
            "outcome": {
                "kind": "partial",
                "picked_quantity": 2,
                "destination_license_plate_barcode": "TOTE-1"
            }
        }))
        .unwrap();
        assert_eq!(
            request.observed_item_barcode.as_deref(),
            Some("SKU-OBSERVED")
        );
        assert_eq!(request.observed_lot.as_deref(), Some("LOT-OBSERVED"));

        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("expected_order_revision").is_none());
        assert!(value.get("idempotency_key").is_none());
        assert!(serde_json::from_value::<ReportPickShortageRequest>(json!({
            "source_location_barcode": "A-01",
            "details": {"reason": "inventory_missing"},
            "outcome": {"kind": "no_pick", "picked_quantity": 0}
        }))
        .is_err());
        assert!(serde_json::from_value::<ReportPickShortageRequest>(json!({
            "source_location_barcode": "A-01",
            "details": {"reason": "inventory_missing"},
            "outcome": {"kind": "no_pick"},
            "expected_order_revision": 3
        }))
        .is_err());
    }

    #[test]
    fn report_response_is_replay_complete() {
        let response = ReportPickShortageResponse {
            shortage_id: 21,
            shortage_revision: Revision::new(1).unwrap(),
            shortage_status: PickShortageStatus::AwaitingInventory,
            task_id: 7,
            content_id: 8,
            order_id: 9,
            order_revision: Revision::new(4).unwrap(),
            quantities: PickShortageQuantitiesResponse {
                planned: 4,
                picked: 0,
                short: 4,
            },
            details: PickShortageDetails {
                reason: PickShortageReason::InventoryMissing,
                note: None,
            },
            reallocated_quantity: 0,
            recovery_terminal_quantity: 0,
            remaining_to_allocate_quantity: 4,
            observed_item_barcode: None,
            observed_lot: None,
            observed_serial: None,
            hold: PickShortageHoldResponse {
                hold_id: 31,
                inventory_balance_id: 41,
                held_quantity: 4,
            },
            movement: None,
            reported_by: 51,
            reported_at: "2026-08-08T20:00:00Z".into(),
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "shortage_id": 21,
                "shortage_revision": 1,
                "shortage_status": "awaiting_inventory",
                "task_id": 7,
                "content_id": 8,
                "order_id": 9,
                "order_revision": 4,
                "quantities": {"planned": 4, "picked": 0, "short": 4},
                "details": {"reason": "inventory_missing"},
                "reallocated_quantity": 0,
                "recovery_terminal_quantity": 0,
                "remaining_to_allocate_quantity": 4,
                "observed_item_barcode": null,
                "observed_lot": null,
                "observed_serial": null,
                "hold": {
                    "hold_id": 31,
                    "inventory_balance_id": 41,
                    "held_quantity": 4
                },
                "movement": null,
                "reported_by": 51,
                "reported_at": "2026-08-08T20:00:00Z"
            })
        );
    }

    #[test]
    fn reallocation_and_queue_contracts_are_optimistic_bounded_and_strict() {
        let request = serde_json::from_value::<ReallocatePickShortageRequest>(json!({
            "expected_shortage_revision": 2,
            "expected_order_revision": 7
        }))
        .unwrap();
        assert_eq!(request.expected_shortage_revision.get(), 2);
        assert_eq!(request.expected_order_revision.get(), 7);
        assert!(
            serde_json::from_value::<ReallocatePickShortageRequest>(json!({
                "expected_shortage_revision": 0,
                "expected_order_revision": 7
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ReallocatePickShortageRequest>(json!({
                "expected_shortage_revision": 2,
                "expected_order_revision": 7,
                "strategy": "fifo"
            }))
            .is_err()
        );

        let page = serde_json::from_value::<PickShortagePageRequest>(json!({
            "facility_id": 3,
            "inventory_owner_id": 4,
            "order_key": "ORDER-005",
            "status": "recovery_in_progress",
            "limit": 25
        }))
        .unwrap();
        assert_eq!(page.order_key.as_deref(), Some("ORDER-005"));
        assert_eq!(page.limit.get(), 25);
        assert_eq!(page.sort, PickShortageQueueSort::Reported);
        assert_eq!(page.direction, PickShortageQueueSortDirection::Descending);
        assert!(serde_json::from_value::<PickShortagePageRequest>(json!({
            "limit": 1001
        }))
        .is_err());
        assert!(serde_json::from_value::<PickShortagePageRequest>(json!({
            "limit": 25,
            "offset": 10
        }))
        .is_err());
    }

    #[test]
    fn short_ship_request_is_strict_revisioned_and_has_no_client_quantity() {
        let request = serde_json::from_value::<AcceptPickShortageAsShortShipRequest>(json!({
            "expected_shortage_revision": 3,
            "expected_order_revision": 8,
            "reason": "client_authorized",
            "note": "Client approved reduced fulfillment"
        }))
        .unwrap();
        assert_eq!(request.expected_shortage_revision.get(), 3);
        assert_eq!(request.expected_order_revision.get(), 8);
        assert_eq!(request.reason, PickShortShipReason::ClientAuthorized);

        assert!(
            serde_json::from_value::<AcceptPickShortageAsShortShipRequest>(json!({
                "expected_shortage_revision": 3,
                "expected_order_revision": 8,
                "reason": "inventory_unavailable",
                "note": null,
                "accepted_short_quantity": 4
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<AcceptPickShortageAsShortShipRequest>(json!({
                "expected_shortage_revision": 0,
                "expected_order_revision": 8,
                "reason": "inventory_unavailable",
                "note": null
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<AcceptPickShortageAsShortShipRequest>(json!({
                "expected_shortage_revision": 3,
                "expected_order_revision": 8,
                "reason": "supervisor_override",
                "note": null
            }))
            .is_err()
        );
    }

    #[test]
    fn short_ship_response_exposes_conserved_effective_demand() {
        let response = AcceptPickShortageAsShortShipResponse {
            disposition_id: 31,
            shortage_id: 21,
            previous_shortage_status: PickShortageStatus::AwaitingInventory,
            shortage_status: PickShortageStatus::Resolved,
            shortage_resolution: PickShortageResolution::ShortShip,
            shortage_revision: Revision::new(4).unwrap(),
            order_id: 11,
            order_line_id: 12,
            previous_order_status: PickOrderStatus::Processing,
            order_status: PickOrderStatus::AwaitingPacking,
            order_revision: Revision::new(9).unwrap(),
            order_ready_to_pack: true,
            shortage_quantities: PickShortageQuantitiesResponse {
                planned: 5,
                picked: 2,
                short: 3,
            },
            reallocated_quantity: 0,
            recovery_terminal_quantity: 0,
            accepted_short_quantity: 3,
            line_demand: ShortShipDemandResponse {
                ordered: 5,
                accepted_short: 3,
                accepted_substitute: 0,
                effective: 2,
            },
            order_demand: ShortShipDemandResponse {
                ordered: 12,
                accepted_short: 3,
                accepted_substitute: 0,
                effective: 9,
            },
            inventory_hold_id: 41,
            reason: PickShortShipReason::InventoryUnavailable,
            note: None,
            resolved_by: 51,
            resolved_at: "2026-08-08T20:00:00Z".into(),
        };

        let encoded = serde_json::to_value(&response).unwrap();
        assert_eq!(encoded["shortage_resolution"], "short_ship");
        assert_eq!(encoded["line_demand"]["effective"], 2);
        assert_eq!(encoded["order_demand"]["accepted_short"], 3);
        assert_eq!(
            serde_json::from_value::<AcceptPickShortageAsShortShipResponse>(encoded).unwrap(),
            response
        );
    }
}
