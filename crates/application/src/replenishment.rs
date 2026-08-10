//! Application contracts for supervisor planning and loose-stock replenishment execution.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId, ItemBatchId, LocationId,
    PlannedReplenishmentSource, ReplenishmentCancellationId, ReplenishmentClaimReleaseReason,
    ReplenishmentConfirmationId, ReplenishmentLevel, ReplenishmentMoveQuantity,
    ReplenishmentPlanId, ReplenishmentPlanningOutcome, ReplenishmentPlanningSnapshot,
    ReplenishmentPolicyDefinition, ReplenishmentPolicyId, ReplenishmentPolicyRevision,
    ReplenishmentPolicyScope, ReplenishmentPolicyStatus, ReplenishmentScanValue, ReplenishmentUom,
    ReplenishmentWorkCancellationNote, ReplenishmentWorkCancellationReason, ReplenishmentWorkId,
    ReplenishmentWorkStatus, Timestamp, UserId,
};

pub const CONFIGURE_REPLENISHMENT_POLICY_OPERATION: &str = "replenishment.policy.configure.v1";
pub const RETIRE_REPLENISHMENT_POLICY_OPERATION: &str = "replenishment.policy.retire.v1";
pub const PLAN_REPLENISHMENT_OPERATION: &str = "replenishment.plan.v1";
pub const CLAIM_NEXT_REPLENISHMENT_WORK_OPERATION: &str = "replenishment.claim_next.v1";
pub const CLAIM_REPLENISHMENT_WORK_BY_ID_OPERATION: &str = "replenishment.claim_by_id.v1";
pub const HEARTBEAT_REPLENISHMENT_CLAIM_OPERATION: &str = "replenishment.heartbeat.v1";
pub const RELEASE_REPLENISHMENT_CLAIM_OPERATION: &str = "replenishment.release.v1";
pub const CONFIRM_REPLENISHMENT_WORK_OPERATION: &str = "task.confirm_replenishment.v1";
pub const CANCEL_REPLENISHMENT_WORK_OPERATION: &str = "replenishment.work.cancel.v1";

/// Creates or replaces the active policy at one natural key.
///
/// An absent expected revision is create-only. A present revision must match the
/// exact active version replaced by this command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureReplenishmentPolicyCommand {
    pub definition: ReplenishmentPolicyDefinition,
    pub expected_revision: Option<ReplenishmentPolicyRevision>,
}

/// Replay-stable active policy version produced by configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureReplenishmentPolicyResult {
    pub policy_id: ReplenishmentPolicyId,
    pub definition: ReplenishmentPolicyDefinition,
    pub status: ReplenishmentPolicyStatus,
    pub previous_revision: Option<ReplenishmentPolicyRevision>,
    pub revision: ReplenishmentPolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetireReplenishmentPolicyCommand {
    pub policy_id: ReplenishmentPolicyId,
    pub expected_revision: ReplenishmentPolicyRevision,
}

/// Replay-stable result of removing one policy from active consideration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetireReplenishmentPolicyResult {
    pub policy_id: ReplenishmentPolicyId,
    pub scope: ReplenishmentPolicyScope,
    pub revision: ReplenishmentPolicyRevision,
    pub status: ReplenishmentPolicyStatus,
    pub retired_by: UserId,
    pub retired_at: Timestamp,
}

/// Supervisor command. All quantity facts are derived inside the locked transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReplenishmentCommand {
    pub policy_id: ReplenishmentPolicyId,
    pub expected_policy_revision: ReplenishmentPolicyRevision,
}

/// One exact loose-stock movement emitted by a planning run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedReplenishmentWork {
    pub work_id: ReplenishmentWorkId,
    pub sequence: u32,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub source_location_id: LocationId,
    pub source_location_barcode: ReplenishmentScanValue,
    pub source_location_name: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub source_received_at: Timestamp,
    pub quantity: ReplenishmentMoveQuantity,
}

impl PlannedReplenishmentWork {
    pub fn from_domain(
        work_id: ReplenishmentWorkId,
        source: PlannedReplenishmentSource,
        source_location_barcode: ReplenishmentScanValue,
        source_location_name: Option<String>,
    ) -> Self {
        Self {
            work_id,
            sequence: source.sequence,
            source_inventory_balance_id: source.source_inventory_balance_id,
            item_batch_id: source.item_batch_id,
            source_location_id: source.source_location_id,
            source_location_barcode,
            source_location_name,
            lot: source.lot,
            serial: source.serial,
            expiration: source.expiration,
            source_received_at: source.received_at,
            quantity: source.quantity,
        }
    }
}

/// Replay-stable decision and exact work identities from one planning transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReplenishmentResult {
    pub plan_id: ReplenishmentPlanId,
    pub policy_id: ReplenishmentPolicyId,
    pub policy_revision: ReplenishmentPolicyRevision,
    pub scope: ReplenishmentPolicyScope,
    pub snapshot: ReplenishmentPlanningSnapshot,
    pub required_level: ReplenishmentLevel,
    pub target_gap: ReplenishmentLevel,
    pub planned: ReplenishmentLevel,
    pub remaining: ReplenishmentLevel,
    pub outcome: ReplenishmentPlanningOutcome,
    pub work: Vec<PlannedReplenishmentWork>,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
}

impl PlanReplenishmentResult {
    pub fn quantities_and_sequence_are_consistent(&self) -> bool {
        let total = self
            .work
            .iter()
            .try_fold(0_i64, |sum, work| sum.checked_add(work.quantity.get()));
        let sequence_is_contiguous = self.work.iter().enumerate().all(|(index, work)| {
            usize::try_from(work.sequence)
                .ok()
                .and_then(|sequence| sequence.checked_sub(1))
                == Some(index)
        });

        total == Some(self.planned.get())
            && self.target_gap.get() == self.planned.get() + self.remaining.get()
            && sequence_is_contiguous
            && (self.planned == ReplenishmentLevel::ZERO) == self.work.is_empty()
    }
}

/// Stable filters decoded from a cursor-bound manager policy query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplenishmentPolicyPageFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub item_id: Option<CatalogItemId>,
    pub pick_face_location_id: Option<LocationId>,
    pub offset: u64,
    pub limit: u16,
    pub sort: ReplenishmentPolicySort,
    pub direction: ReplenishmentPolicySortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplenishmentPolicySort {
    InventoryOwner,
    Facility,
    Item,
    PickFace,
    Projected,
    Demand,
    Reserve,
    TargetGap,
    Outcome,
    ActiveWork,
}

impl ReplenishmentPolicySort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InventoryOwner => "inventory_owner",
            Self::Facility => "facility",
            Self::Item => "item",
            Self::PickFace => "pick_face",
            Self::Projected => "projected",
            Self::Demand => "demand",
            Self::Reserve => "reserve",
            Self::TargetGap => "target_gap",
            Self::Outcome => "outcome",
            Self::ActiveWork => "active_work",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplenishmentPolicySortDirection {
    Ascending,
    Descending,
}

impl ReplenishmentPolicySortDirection {
    pub const fn is_ascending(self) -> bool {
        matches!(self, Self::Ascending)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentLatestPlanReadModel {
    pub plan_id: ReplenishmentPlanId,
    pub outcome: ReplenishmentPlanningOutcome,
    pub planned: ReplenishmentLevel,
    pub remaining: ReplenishmentLevel,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
}

/// Active policy readiness visible even when no execution work exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentPolicyReadinessReadModel {
    pub policy_id: ReplenishmentPolicyId,
    pub revision: ReplenishmentPolicyRevision,
    pub definition: ReplenishmentPolicyDefinition,
    pub inventory_owner_name: String,
    pub facility_name: String,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub pick_face: ReplenishmentLocationReadModel,
    pub snapshot: ReplenishmentPlanningSnapshot,
    pub required_level: ReplenishmentLevel,
    pub target_gap: ReplenishmentLevel,
    pub suggested_outcome: ReplenishmentPlanningOutcome,
    pub suggested_quantity: ReplenishmentLevel,
    pub suggested_remaining: ReplenishmentLevel,
    pub active_work_count: i64,
    pub active_work_quantity: ReplenishmentLevel,
    pub latest_plan: Option<ReplenishmentLatestPlanReadModel>,
}

impl ReplenishmentPolicyReadinessReadModel {
    /// Checks repository projections against the same pure decision used by planning.
    pub fn quantities_are_consistent(&self) -> bool {
        let decision =
            wareboxes_domain::plan_replenishment(self.definition.thresholds(), self.snapshot);
        self.active_work_count >= 0
            && self.active_work_quantity == self.snapshot.active_inbound()
            && self.required_level == decision.required_level
            && self.target_gap == decision.target_gap
            && self.suggested_outcome == decision.outcome
            && self.suggested_quantity == decision.planned
            && self.suggested_remaining == decision.remaining
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplenishmentPolicyPage {
    pub items: Vec<ReplenishmentPolicyReadinessReadModel>,
    pub next_offset: Option<u64>,
}

/// Stable filters decoded from a cursor-bound execution-monitor query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplenishmentWorkPageFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub item_id: Option<CatalogItemId>,
    pub pick_face_location_id: Option<LocationId>,
    pub status: Option<ReplenishmentWorkStatus>,
    pub offset: u64,
    pub limit: u16,
    pub sort: ReplenishmentWorkSort,
    pub direction: ReplenishmentWorkSortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplenishmentWorkSort {
    Created,
    Priority,
    InventoryOwner,
    Facility,
    Item,
    Source,
    Destination,
    Quantity,
    Status,
    Lease,
}

impl ReplenishmentWorkSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Priority => "priority",
            Self::InventoryOwner => "inventory_owner",
            Self::Facility => "facility",
            Self::Item => "item",
            Self::Source => "source",
            Self::Destination => "destination",
            Self::Quantity => "quantity",
            Self::Status => "status",
            Self::Lease => "lease",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplenishmentWorkSortDirection {
    Ascending,
    Descending,
}

impl ReplenishmentWorkSortDirection {
    pub const fn is_ascending(self) -> bool {
        matches!(self, Self::Ascending)
    }
}

/// Supervisor execution-monitor row independent of RF claim mutation contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentWorkReadModel {
    pub work_id: ReplenishmentWorkId,
    pub plan_id: ReplenishmentPlanId,
    pub policy_id: ReplenishmentPolicyId,
    pub policy_revision: ReplenishmentPolicyRevision,
    pub status: ReplenishmentWorkStatus,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub sequence: u32,
    pub priority: i64,
    pub item_id: CatalogItemId,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: ReplenishmentUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantity: ReplenishmentMoveQuantity,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub source_location: ReplenishmentLocationReadModel,
    pub destination_pick_face: ReplenishmentLocationReadModel,
    pub claimed_by: Option<UserId>,
    pub lease_expires_at: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub completed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplenishmentWorkPage {
    pub items: Vec<ReplenishmentWorkReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClaimNextReplenishmentWorkCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimReplenishmentWorkByIdCommand {
    pub work_id: ReplenishmentWorkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatReplenishmentClaimCommand {
    pub work_id: ReplenishmentWorkId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseReplenishmentClaimCommand {
    pub work_id: ReplenishmentWorkId,
    pub reason: ReplenishmentClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelReplenishmentWorkCommand {
    work_id: ReplenishmentWorkId,
    reason: ReplenishmentWorkCancellationReason,
    note: Option<ReplenishmentWorkCancellationNote>,
}

impl CancelReplenishmentWorkCommand {
    pub fn new(
        work_id: ReplenishmentWorkId,
        reason: ReplenishmentWorkCancellationReason,
        note: Option<ReplenishmentWorkCancellationNote>,
    ) -> Result<Self, wareboxes_domain::ReplenishmentError> {
        if reason == ReplenishmentWorkCancellationReason::Other && note.is_none() {
            return Err(wareboxes_domain::ReplenishmentError::CancellationNoteRequired);
        }
        Ok(Self {
            work_id,
            reason,
            note,
        })
    }

    pub const fn work_id(&self) -> ReplenishmentWorkId {
        self.work_id
    }

    pub const fn reason(&self) -> ReplenishmentWorkCancellationReason {
        self.reason
    }

    pub fn note(&self) -> Option<&ReplenishmentWorkCancellationNote> {
        self.note.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelReplenishmentWorkResult {
    pub cancellation_id: ReplenishmentCancellationId,
    pub work_id: ReplenishmentWorkId,
    pub plan_id: ReplenishmentPlanId,
    pub policy_id: ReplenishmentPolicyId,
    pub policy_revision: ReplenishmentPolicyRevision,
    pub scope: ReplenishmentPolicyScope,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub quantity: ReplenishmentMoveQuantity,
    pub previous_status: ReplenishmentWorkStatus,
    pub previous_assigned_user_id: Option<UserId>,
    pub status: ReplenishmentWorkStatus,
    pub reason: ReplenishmentWorkCancellationReason,
    pub note: Option<ReplenishmentWorkCancellationNote>,
    pub cancelled_by: UserId,
    pub cancelled_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentClaimHeartbeatResult {
    pub work_id: ReplenishmentWorkId,
    pub heartbeat_at: Timestamp,
    pub lease_expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentClaimReleaseResult {
    pub work_id: ReplenishmentWorkId,
    pub status: ReplenishmentWorkStatus,
    pub released_at: Timestamp,
    pub release_count: i64,
    pub reason: ReplenishmentClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentLocationReadModel {
    pub location_id: LocationId,
    pub barcode: ReplenishmentScanValue,
    pub name: Option<String>,
}

/// Scanner-ready claim preserving the planned batch and destination identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentClaim {
    pub work_id: ReplenishmentWorkId,
    pub plan_id: ReplenishmentPlanId,
    pub policy_id: ReplenishmentPolicyId,
    pub policy_revision: ReplenishmentPolicyRevision,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub sequence: u32,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<Timestamp>,
    pub lease_expires_at: Timestamp,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<ReplenishmentScanValue>,
    pub uom: ReplenishmentUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantity: ReplenishmentMoveQuantity,
    pub source_location: ReplenishmentLocationReadModel,
    pub destination_pick_face: ReplenishmentLocationReadModel,
}

/// Confirms the server-planned quantity using physical evidence only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmReplenishmentWorkCommand {
    pub work_id: ReplenishmentWorkId,
    pub source_location_barcode: ReplenishmentScanValue,
    pub item_barcode: ReplenishmentScanValue,
    pub lot_scan: Option<ReplenishmentScanValue>,
    pub serial_scan: Option<ReplenishmentScanValue>,
    pub destination_pick_face_barcode: ReplenishmentScanValue,
}

/// Replay-stable inventory and work result of one confirmed movement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmReplenishmentWorkResult {
    pub confirmation_id: ReplenishmentConfirmationId,
    pub work_id: ReplenishmentWorkId,
    pub plan_id: ReplenishmentPlanId,
    pub policy_id: ReplenishmentPolicyId,
    pub inventory_transaction_id: i64,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub destination_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub uom: ReplenishmentUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub source_location_id: LocationId,
    pub destination_pick_face_location_id: LocationId,
    pub quantity: ReplenishmentMoveQuantity,
    pub work_status: ReplenishmentWorkStatus,
    pub confirmed_by: UserId,
    pub confirmed_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{
        ReplenishmentPolicyThresholds, ReplenishmentReserveSourceLocationIds, TenantId,
    };

    fn domain_id<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>) -> T {
        constructor(value).ok().unwrap()
    }

    fn result() -> PlanReplenishmentResult {
        let timestamp = "2026-08-08T12:00:00Z".parse::<Timestamp>().unwrap();
        PlanReplenishmentResult {
            plan_id: domain_id(1, ReplenishmentPlanId::new),
            policy_id: domain_id(2, ReplenishmentPolicyId::new),
            policy_revision: ReplenishmentPolicyRevision::new(3).unwrap(),
            scope: ReplenishmentPolicyScope {
                tenant_id: domain_id(4, TenantId::new),
                inventory_owner_id: domain_id(5, InventoryOwnerId::new),
                facility_id: domain_id(6, FacilityId::new),
                item_id: CatalogItemId::new(7).unwrap(),
                uom: ReplenishmentUom::new("each").unwrap(),
                pick_face_location_id: domain_id(8, LocationId::new),
            },
            snapshot: ReplenishmentPlanningSnapshot::new(
                ReplenishmentLevel::new(1).unwrap(),
                ReplenishmentLevel::new(1).unwrap(),
                ReplenishmentLevel::new(4).unwrap(),
                ReplenishmentLevel::new(18).unwrap(),
            )
            .unwrap(),
            required_level: ReplenishmentLevel::new(20).unwrap(),
            target_gap: ReplenishmentLevel::new(18).unwrap(),
            planned: ReplenishmentLevel::new(18).unwrap(),
            remaining: ReplenishmentLevel::ZERO,
            outcome: ReplenishmentPlanningOutcome::FullyPlanned,
            work: vec![PlannedReplenishmentWork {
                work_id: domain_id(9, ReplenishmentWorkId::new),
                sequence: 1,
                source_inventory_balance_id: domain_id(10, InventoryBalanceId::new),
                item_batch_id: domain_id(11, ItemBatchId::new),
                source_location_id: domain_id(12, LocationId::new),
                source_location_barcode: ReplenishmentScanValue::new("RES-01").unwrap(),
                source_location_name: Some("Reserve 01".into()),
                lot: None,
                serial: None,
                expiration: None,
                source_received_at: timestamp,
                quantity: ReplenishmentMoveQuantity::new(18).unwrap(),
            }],
            planned_by: domain_id(13, UserId::new),
            planned_at: timestamp,
        }
    }

    #[test]
    fn operation_names_are_stable_and_match_the_execution_lifecycle() {
        assert_eq!(
            CONFIGURE_REPLENISHMENT_POLICY_OPERATION,
            "replenishment.policy.configure.v1"
        );
        assert_eq!(
            RETIRE_REPLENISHMENT_POLICY_OPERATION,
            "replenishment.policy.retire.v1"
        );
        assert_eq!(PLAN_REPLENISHMENT_OPERATION, "replenishment.plan.v1");
        assert_eq!(
            CONFIRM_REPLENISHMENT_WORK_OPERATION,
            "task.confirm_replenishment.v1"
        );
        assert_eq!(
            CANCEL_REPLENISHMENT_WORK_OPERATION,
            "replenishment.work.cancel.v1"
        );
    }

    #[test]
    fn configuration_distinguishes_create_from_exact_revision_replace() {
        let definition = ReplenishmentPolicyDefinition::new(
            result().scope,
            ReplenishmentPolicyThresholds::new(
                ReplenishmentLevel::new(5).unwrap(),
                ReplenishmentLevel::new(20).unwrap(),
            )
            .unwrap(),
            ReplenishmentReserveSourceLocationIds::new(vec![domain_id(12, LocationId::new)])
                .unwrap(),
        )
        .unwrap();
        let create = ConfigureReplenishmentPolicyCommand {
            definition: definition.clone(),
            expected_revision: None,
        };
        let replace = ConfigureReplenishmentPolicyCommand {
            definition,
            expected_revision: Some(ReplenishmentPolicyRevision::new(4).unwrap()),
        };

        assert!(create.expected_revision.is_none());
        assert_eq!(replace.expected_revision.map(|value| value.get()), Some(4));
    }

    #[test]
    fn planning_results_conserve_quantity_and_stable_work_sequence() {
        let result = result();
        assert!(result.quantities_and_sequence_are_consistent());

        let mut broken = result;
        broken.work[0].sequence = 2;
        assert!(!broken.quantities_and_sequence_are_consistent());
    }

    #[test]
    fn confirmation_command_has_scans_but_no_client_quantity() {
        let command = ConfirmReplenishmentWorkCommand {
            work_id: domain_id(1, ReplenishmentWorkId::new),
            source_location_barcode: ReplenishmentScanValue::new("RES-01").unwrap(),
            item_barcode: ReplenishmentScanValue::new("SKU-1").unwrap(),
            lot_scan: Some(ReplenishmentScanValue::new("LOT-1").unwrap()),
            serial_scan: None,
            destination_pick_face_barcode: ReplenishmentScanValue::new("PICK-01").unwrap(),
        };

        assert_eq!(command.item_barcode.as_str(), "SKU-1");
        assert_eq!(command.destination_pick_face_barcode.as_str(), "PICK-01");
    }

    #[test]
    fn policy_readiness_is_visible_without_created_work() {
        let planning = result();
        let definition = ReplenishmentPolicyDefinition::new(
            planning.scope.clone(),
            ReplenishmentPolicyThresholds::new(
                ReplenishmentLevel::new(5).unwrap(),
                ReplenishmentLevel::new(20).unwrap(),
            )
            .unwrap(),
            ReplenishmentReserveSourceLocationIds::new(vec![domain_id(12, LocationId::new)])
                .unwrap(),
        )
        .unwrap();
        let readiness = ReplenishmentPolicyReadinessReadModel {
            policy_id: planning.policy_id,
            revision: planning.policy_revision,
            definition,
            inventory_owner_name: "Alpine".into(),
            facility_name: "Reno DC".into(),
            item_description: Some("Widget".into()),
            primary_sku: Some("WIDGET-EA".into()),
            pick_face: ReplenishmentLocationReadModel {
                location_id: domain_id(8, LocationId::new),
                barcode: ReplenishmentScanValue::new("PICK-01").unwrap(),
                name: Some("Forward pick 01".into()),
            },
            snapshot: planning.snapshot,
            required_level: planning.required_level,
            target_gap: planning.target_gap,
            suggested_outcome: planning.outcome,
            suggested_quantity: planning.planned,
            suggested_remaining: planning.remaining,
            active_work_count: 1,
            active_work_quantity: ReplenishmentLevel::new(1).unwrap(),
            latest_plan: None,
        };

        assert!(readiness.quantities_are_consistent());
        assert!(readiness.latest_plan.is_none());
    }
}
