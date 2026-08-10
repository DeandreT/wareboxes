//! Domain identifiers and invariants shared across application boundaries.

mod allocation;
mod backorder;
mod cycle_count;
mod facility;
mod inbound_inspection;
mod inventory_recall;
mod item_storage_policy;
mod item_substitution;
mod item_traceability_policy;
mod order;
mod order_amendment;
mod order_cancellation;
mod order_line_amendment;
mod order_release;
mod outbound_load;
mod outbound_qa;
mod packing;
mod pick_wave;
mod picking;
mod replenishment;
mod shipping;
mod storage_zone;
mod tenant;

pub use allocation::{
    assess_order_allocation_readiness, plan_fefo_allocation, AllocationCandidate,
    AllocationExecutionStage, AllocationOutcome, AllocationPlan, AllocationPlanError,
    AllocationQuantity, AllocationShortageReason, AllocationStrategy, OrderAllocationBlockReason,
    OrderAllocationReadiness, OrderRevision, PlannedAllocation,
};
pub use backorder::{
    split_current_allocation_shortage, BackorderDetails, BackorderError, BackorderLineSnapshot,
    BackorderNote, BackorderPolicyMode, BackorderPolicyRevision, BackorderReason,
    BackorderSplitLineTransition, BackorderSplitTransition, MAX_BACKORDER_NOTE_LENGTH,
};
pub use cycle_count::{
    decide_cycle_count_disposition, CycleCountDisposition, CycleCountError,
    CycleCountPolicyRevision, CycleCountTolerancePolicy, CycleCountVarianceDecision,
    CycleCountVarianceDecisionDetails, CycleCountVarianceNote, CycleCountVarianceReason,
    CycleCountVarianceRevision, CycleCountVarianceStatus, MAX_CYCLE_COUNT_PERCENTAGE_BASIS_POINTS,
    MAX_CYCLE_COUNT_RECOUNTS, MAX_CYCLE_COUNT_VARIANCE_NOTE_LENGTH,
};
pub use facility::{
    FacilityRevision, FacilityShippingOrigin, FacilityShippingOriginError,
    FacilityShippingOriginField, MAX_FACILITY_ORIGIN_ADDRESS_LINE_LENGTH,
    MAX_FACILITY_ORIGIN_CITY_LENGTH, MAX_FACILITY_ORIGIN_COMPANY_LENGTH,
    MAX_FACILITY_ORIGIN_COUNTRY_LENGTH, MAX_FACILITY_ORIGIN_EMAIL_LENGTH,
    MAX_FACILITY_ORIGIN_NAME_LENGTH, MAX_FACILITY_ORIGIN_PHONE_LENGTH,
    MAX_FACILITY_ORIGIN_POSTAL_CODE_LENGTH, MAX_FACILITY_ORIGIN_STATE_LENGTH,
};
pub use inbound_inspection::{
    decide_inbound_inspection, InboundInspectionError, InboundInspectionNote,
    InboundInspectionOutcome, InboundInspectionTargetStatus, MAX_INBOUND_INSPECTION_NOTE_LENGTH,
};
pub use inventory_recall::{
    release_inventory_recall, InventoryRecallDetails, InventoryRecallError, InventoryRecallNote,
    InventoryRecallReason, InventoryRecallRevision, InventoryRecallStatus,
    MAX_INVENTORY_RECALL_NOTE_LENGTH,
};
pub use item_storage_policy::{
    AllowedStorageZonePurposes, ItemStorageLocationCapacity, ItemStoragePolicyDefinition,
    ItemStoragePolicyError, ItemStoragePolicyRevision, ItemStoragePolicyStatus,
    ItemStoragePolicyUom, MAX_ITEM_STORAGE_POLICY_UOM_LENGTH,
};
pub use item_substitution::{
    substitute_pick_shortage, ItemSubstitutionDefinition, ItemSubstitutionDetails,
    ItemSubstitutionError, ItemSubstitutionNote, ItemSubstitutionPolicyRevision,
    ItemSubstitutionReason, SubstitutePickShortageTransition, SubstitutionQuantity,
    SubstitutionUom, MAX_ITEM_SUBSTITUTION_NOTE_LENGTH,
};
pub use item_traceability_policy::{
    ItemTraceabilityPolicyDefinition, ItemTraceabilityPolicyError, ItemTraceabilityPolicyRevision,
    ItemTraceabilityPolicyStatus, ItemTraceabilityPolicyUom, MinimumShelfLifeDays,
    TraceabilityRequirement, MAX_ITEM_TRACEABILITY_POLICY_UOM_LENGTH, MAX_MINIMUM_SHELF_LIFE_DAYS,
};
pub use order::{
    CatalogItemId, FulfillmentOrderDemandLine, NewFulfillmentOrder, OrderCreationError,
    OrderCreationField, OrderHoldReason, OrderHoldTransitionError, OrderKey, OrderLineKey,
    OrderQuantity, OrderStatus, RequestedUom, ShippingDestination, ShippingRecipient,
    MAX_DESTINATION_ADDRESS_LINE_LENGTH, MAX_DESTINATION_CITY_LENGTH,
    MAX_DESTINATION_COMPANY_LENGTH, MAX_DESTINATION_COUNTRY_LENGTH, MAX_DESTINATION_EMAIL_LENGTH,
    MAX_DESTINATION_PHONE_LENGTH, MAX_DESTINATION_POSTAL_CODE_LENGTH,
    MAX_DESTINATION_RECIPIENT_NAME_LENGTH, MAX_DESTINATION_REGION_LENGTH, MAX_ORDER_KEY_LENGTH,
    MAX_ORDER_LINE_KEY_LENGTH, MAX_REQUESTED_UOM_LENGTH,
};
pub use order_amendment::{
    amend_fulfillment_order, FulfillmentOrderHeader, OrderAmendmentError, OrderAmendmentTransition,
};
pub use order_cancellation::{
    cancel_order_before_physical_execution, CancellationNote, OrderCancellationDetails,
    OrderCancellationError, OrderCancellationExecution, OrderCancellationReason,
    OrderCancellationTransitionError, MAX_CANCELLATION_NOTE_LENGTH,
};
pub use order_line_amendment::{
    replace_fulfillment_order_lines, OrderLineAmendmentError, OrderLineAmendmentTransition,
};
pub use order_release::{release_order, OrderReleaseError};
pub use outbound_load::{
    cancel_outbound_load, complete_outbound_load_loading, depart_outbound_load,
    depart_packed_carton, load_packed_carton, record_outbound_carton_loaded,
    record_outbound_carton_staged, record_outbound_carton_unloaded,
    record_outbound_carton_unstaged, release_outbound_load, stage_packed_carton,
    start_outbound_load_loading, unload_packed_carton, unstage_packed_carton,
    OutboundLoadCancellationDetails, OutboundLoadCancellationNote, OutboundLoadCancellationReason,
    OutboundLoadError, OutboundLoadProgress, OutboundLoadReference, OutboundLoadRevision,
    OutboundLoadScanValue, OutboundLoadStatus, OutboundLoadTransition, PackedCartonMovementKind,
    PackedCartonPositionRevision, PackedCartonPositionState, SealNumber, TrailerNumber,
    MAX_OUTBOUND_LOAD_CANCELLATION_NOTE_LENGTH, MAX_OUTBOUND_LOAD_REFERENCE_LENGTH,
    MAX_OUTBOUND_LOAD_SCAN_VALUE_LENGTH, MAX_OUTBOUND_LOAD_SEAL_NUMBER_LENGTH,
    MAX_OUTBOUND_LOAD_TRAILER_NUMBER_LENGTH,
};
pub use outbound_qa::{
    begin_outbound_qa, cancel_outbound_qa, complete_outbound_qa, record_outbound_qa_carton,
    OutboundQaCancellationDetails, OutboundQaCancellationNote, OutboundQaCancellationReason,
    OutboundQaError, OutboundQaPolicyRevision, OutboundQaProgress, OutboundQaRequirement,
    OutboundQaScanValue, OutboundQaSessionRevision, OutboundQaSessionStatus,
    MAX_OUTBOUND_QA_CANCELLATION_NOTE_LENGTH, MAX_OUTBOUND_QA_SCAN_VALUE_LENGTH,
};
pub use packing::{
    abandon_empty_packing, begin_packing, complete_packing, continue_packing, open_carton,
    remove_packed_content, reopen_carton, CartonDimensions, CartonMeasurements,
    CartonReopenDetails, CartonReopenNote, CartonReopenReason, CartonStatus, DimensionMillimeters,
    PackContentRemovalDetails, PackContentRemovalNote, PackContentRemovalReason, PackQuantity,
    PackScanValue, PackSessionAbandonmentDetails, PackSessionAbandonmentNote,
    PackSessionAbandonmentReason, PackSessionStatus, PackingError, PackingProgress, WeightGrams,
    MAX_CARTON_REOPEN_NOTE_LENGTH, MAX_PACK_CONTENT_REMOVAL_NOTE_LENGTH,
    MAX_PACK_SCAN_VALUE_LENGTH, MAX_PACK_SESSION_ABANDONMENT_NOTE_LENGTH,
};
pub use pick_wave::{
    cancel_pick_wave, release_pick_wave, validate_pick_wave_plan, PickWaveCancellationNote,
    PickWaveCancellationReason, PickWaveError, PickWaveName, PickWaveOrderPrecondition,
    PickWaveRevision, PickWaveStatus, MAX_PICK_WAVE_CANCELLATION_NOTE_LENGTH,
    MAX_PICK_WAVE_NAME_LENGTH,
};
pub use picking::{
    resolve_pick_shortage_as_short_ship, reverse_pick_before_packing, ActualPickQuantity,
    PickClaimReleaseReason, PickContentState, PickQuantity, PickReversalDetails, PickReversalNote,
    PickReversalReason, PickScanValue, PickShortShipDetails, PickShortShipNote,
    PickShortShipReason, PickShortShipTransition, PickShortageDetails, PickShortageNote,
    PickShortageQuantities, PickShortageReason, PickShortageResolution, PickShortageRevision,
    PickShortageStatus, PickingError, ShortShipDemandQuantities, MAX_PICK_REVERSAL_NOTE_LENGTH,
    MAX_PICK_SCAN_VALUE_LENGTH, MAX_PICK_SHORTAGE_NOTE_LENGTH, MAX_PICK_SHORT_SHIP_NOTE_LENGTH,
};
pub use replenishment::{
    assess_replenishment_source, plan_replenishment, select_replenishment_sources,
    validate_unique_active_replenishment_policy_scopes, EligibleReplenishmentSource,
    PlannedReplenishmentSource, ReplenishmentClaimReleaseReason, ReplenishmentError,
    ReplenishmentInventoryStatus, ReplenishmentLevel, ReplenishmentMoveQuantity,
    ReplenishmentPlanDecision, ReplenishmentPlanningOutcome, ReplenishmentPlanningSnapshot,
    ReplenishmentPolicyDefinition, ReplenishmentPolicyRevision, ReplenishmentPolicyScope,
    ReplenishmentPolicyStatus, ReplenishmentPolicyThresholds,
    ReplenishmentReserveSourceLocationIds, ReplenishmentScanValue, ReplenishmentSourceCandidate,
    ReplenishmentSourceEligibility, ReplenishmentSourceIneligibility, ReplenishmentUom,
    ReplenishmentWorkCancellationNote, ReplenishmentWorkCancellationReason,
    ReplenishmentWorkStatus, MAX_REPLENISHMENT_CANCELLATION_NOTE_LENGTH,
    MAX_REPLENISHMENT_SCAN_VALUE_LENGTH, MAX_REPLENISHMENT_UOM_LENGTH,
};
pub use shipping::{
    cancel_shipment, confirm_shipment_departure, create_shipment, record_manual_manifest,
    CarrierCode, CarrierServiceCode, CartonTrackingAssignment, ManifestReference,
    ShipmentCancellationDetails, ShipmentCancellationNote, ShipmentCancellationReason,
    ShipmentCartonIdentity, ShipmentDepartureTransition, ShipmentDocumentType, ShipmentRevision,
    ShipmentScanValue, ShipmentStatus, ShippingError, ShippingTextField, TrackingNumber,
    MAX_CARRIER_CODE_LENGTH, MAX_CARRIER_SERVICE_CODE_LENGTH, MAX_MANIFEST_REFERENCE_LENGTH,
    MAX_SHIPMENT_CANCELLATION_NOTE_LENGTH, MAX_SHIPMENT_SCAN_VALUE_LENGTH,
    MAX_TRACKING_NUMBER_LENGTH,
};
pub use storage_zone::{
    StorageZoneCode, StorageZoneDefinition, StorageZoneError, StorageZoneLocationIds,
    StorageZoneName, StorageZonePurpose, StorageZoneRevision, StorageZoneStatus,
    StorageZoneTravelSequence, MAX_STORAGE_ZONE_CODE_LENGTH, MAX_STORAGE_ZONE_NAME_LENGTH,
};
pub use tenant::TenantStatus;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub type Timestamp = DateTime<Utc>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{kind} must be a positive integer, got {value}")]
pub struct InvalidId {
    kind: &'static str,
    value: i64,
}

macro_rules! positive_id {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(i64);

        impl $name {
            pub fn new(value: i64) -> Result<Self, InvalidId> {
                if value > 0 {
                    Ok(Self(value))
                } else {
                    Err(InvalidId {
                        kind: $label,
                        value,
                    })
                }
            }

            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl TryFrom<i64> for $name {
            type Error = InvalidId;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> Self {
                value.get()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

positive_id!(TenantId, "tenant ID");
positive_id!(InventoryOwnerId, "inventory owner ID");
positive_id!(FacilityId, "facility ID");
positive_id!(AddressId, "address ID");
positive_id!(
    FacilityShippingOriginConfigurationId,
    "facility shipping origin configuration ID"
);
positive_id!(UserId, "user ID");
positive_id!(OrderId, "order ID");
positive_id!(OrderLineId, "order line ID");
positive_id!(OrderAmendmentId, "order amendment ID");
positive_id!(OrderLineAmendmentId, "order line amendment ID");
positive_id!(OrderCancellationId, "order cancellation ID");
positive_id!(BackorderPolicyId, "backorder policy ID");
positive_id!(BackorderSplitId, "backorder split ID");
positive_id!(CycleCountPolicyId, "cycle count policy ID");
positive_id!(CycleCountVarianceId, "cycle count variance ID");
positive_id!(
    CycleCountVarianceDecisionId,
    "cycle count variance decision ID"
);
positive_id!(ItemSubstitutionPolicyId, "item substitution policy ID");
positive_id!(ItemSubstitutionId, "item substitution ID");
positive_id!(ItemStoragePolicyId, "item storage policy ID");
positive_id!(ItemTraceabilityPolicyId, "item traceability policy ID");
positive_id!(OrderReleaseId, "order release ID");
positive_id!(PickWaveId, "pick wave ID");
positive_id!(PickTaskId, "pick task ID");
positive_id!(PickContentId, "pick content ID");
positive_id!(PickConfirmationId, "pick confirmation ID");
positive_id!(PickReversalId, "pick reversal ID");
positive_id!(PickShortageId, "pick shortage ID");
positive_id!(PickShortageDispositionId, "pick shortage disposition ID");
positive_id!(
    PickShortageReallocationRunId,
    "pick shortage reallocation run ID"
);
positive_id!(InventoryHoldId, "inventory hold ID");
positive_id!(InventoryRecallId, "inventory recall ID");
positive_id!(StorageZoneId, "storage zone ID");
positive_id!(
    InboundInspectionDispositionId,
    "inbound inspection disposition ID"
);
positive_id!(ReplenishmentPolicyId, "replenishment policy ID");
positive_id!(ReplenishmentPlanId, "replenishment plan ID");
positive_id!(ReplenishmentWorkId, "replenishment work ID");
positive_id!(ReplenishmentCancellationId, "replenishment cancellation ID");
positive_id!(ReplenishmentConfirmationId, "replenishment confirmation ID");
positive_id!(OutboxDeadLetterReplayId, "outbox dead-letter replay ID");
positive_id!(PackSessionId, "pack session ID");
positive_id!(CartonId, "carton ID");
positive_id!(CartonContentId, "carton content ID");
positive_id!(CartonContentRemovalId, "carton content removal ID");
positive_id!(CartonReopeningId, "carton reopening ID");
positive_id!(OutboundQaPolicyId, "outbound QA policy ID");
positive_id!(OutboundQaSessionId, "outbound QA session ID");
positive_id!(OutboundQaCancellationId, "outbound QA cancellation ID");
positive_id!(
    OutboundQaCartonVerificationId,
    "outbound QA carton verification ID"
);
positive_id!(ShipmentId, "shipment ID");
positive_id!(ShipmentCancellationId, "shipment cancellation ID");
positive_id!(ShipmentDocumentId, "shipment document ID");
positive_id!(OutboundLoadId, "outbound load ID");
positive_id!(OutboundLoadShipmentId, "outbound load shipment ID");
positive_id!(OutboundLoadCartonId, "outbound load carton ID");
positive_id!(PackedCartonPositionId, "packed carton position ID");
positive_id!(PackedCartonMovementId, "packed carton movement ID");
positive_id!(OutboundLoadCancellationId, "outbound load cancellation ID");
positive_id!(CarrierManifestId, "carrier manifest ID");
positive_id!(
    ShipmentTrackingAssignmentId,
    "shipment tracking assignment ID"
);
positive_id!(AllocationRunId, "allocation run ID");
positive_id!(InventoryReservationId, "inventory reservation ID");
positive_id!(InventoryAllocationId, "inventory allocation ID");
positive_id!(InventoryBalanceId, "inventory balance ID");
positive_id!(ItemBatchId, "item batch ID");
positive_id!(LocationId, "location ID");
positive_id!(LicensePlateId, "license plate ID");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SiteScope {
    pub all_facilities: bool,
    pub facility_ids: Vec<FacilityId>,
}

impl SiteScope {
    pub fn includes(&self, facility_id: FacilityId) -> bool {
        self.all_facilities || self.facility_ids.contains(&facility_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerScope {
    pub all_inventory_owners: bool,
    pub inventory_owner_ids: Vec<InventoryOwnerId>,
}

impl OwnerScope {
    pub fn includes(&self, inventory_owner_id: InventoryOwnerId) -> bool {
        self.all_inventory_owners || self.inventory_owner_ids.contains(&inventory_owner_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OwnerFacilityScope {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
}

impl OwnerFacilityScope {
    pub const fn new(inventory_owner_id: InventoryOwnerId, facility_id: FacilityId) -> Self {
        Self {
            inventory_owner_id,
            facility_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_ids_reject_non_positive_values() {
        assert!(TenantId::new(0).is_err());
        assert!(FacilityId::new(-1).is_err());
        assert_eq!(InventoryOwnerId::new(7).map(InventoryOwnerId::get), Ok(7));
    }

    #[test]
    fn scoped_ids_do_not_compare_across_types() {
        let tenant = TenantId::new(4).unwrap();
        let facility = FacilityId::new(4).unwrap();

        assert_eq!(tenant.get(), facility.get());
    }

    #[test]
    fn access_scopes_include_only_explicit_ids_unless_unbounded() {
        let facility = FacilityId::new(7).unwrap();
        let owner = InventoryOwnerId::new(8).unwrap();
        assert!(SiteScope {
            all_facilities: false,
            facility_ids: vec![facility],
        }
        .includes(facility));
        assert!(!OwnerScope {
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        }
        .includes(owner));
        assert!(OwnerScope {
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        }
        .includes(owner));
    }
}
