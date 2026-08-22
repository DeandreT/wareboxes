//! Domain identifiers and invariants shared across application boundaries.

mod allocation;
mod automation;
mod backorder;
mod billing;
mod carrier;
mod configuration;
mod cross_dock;
mod customer_return;
mod cycle_count;
mod data_cell;
mod data_cell_move;
mod facility;
mod inbound_asn;
mod inbound_inspection;
mod inbound_load;
mod integration;
mod integration_mapping;
mod inventory_recall;
mod item_storage_policy;
mod item_substitution;
mod item_traceability_policy;
mod labor;
mod license_plate;
mod order;
mod order_amendment;
mod order_cancellation;
mod order_line_amendment;
mod order_release;
mod outbound_load;
mod outbound_qa;
mod packing;
mod pick_cluster;
mod pick_wave;
mod picking;
mod purchase_order;
mod replenishment;
mod service_account;
mod shipping;
mod slotting;
mod storage_zone;
mod support_access;
mod tenant;
mod transfer_order;
mod value_added_work;
mod vendor_return;
mod work_orchestration;
mod workforce;
mod yard;

pub use allocation::{
    assess_order_allocation_readiness, plan_allocation, plan_fefo_allocation, AllocationCandidate,
    AllocationExecutionStage, AllocationOutcome, AllocationPlan, AllocationPlanError,
    AllocationQuantity, AllocationShortageReason, AllocationStrategy, OrderAllocationBlockReason,
    OrderAllocationReadiness, OrderRevision, PlannedAllocation,
};
pub use automation::*;
pub use backorder::{
    split_current_allocation_shortage, BackorderDetails, BackorderError, BackorderLineSnapshot,
    BackorderNote, BackorderPolicyMode, BackorderPolicyRevision, BackorderReason,
    BackorderSplitLineTransition, BackorderSplitTransition, MAX_BACKORDER_NOTE_LENGTH,
};
pub use billing::{
    validate_review_separation, BillingContractNumber, BillingContractStatus,
    BillingEffectiveWindow, BillingError, BillingQuantity, BillingRateDefinition, BillingRunStatus,
    CurrencyCode, MAX_BILLING_CONTRACT_NUMBER_LENGTH, MAX_BILLING_DESCRIPTION_LENGTH,
    MAX_BILLING_QUANTITY, MAX_BILLING_RATE_MINOR, MAX_BILLING_SOURCE_TYPE_LENGTH,
};
pub use carrier::*;
pub use configuration::{
    resolve_effective_rule, rollback_as_draft, BillableEventType, BillingUnit,
    ConfigurationEffectiveWindow, ConfigurationError, ConfigurationScope, ConfigurationStatus,
    DecisionRuleDefinition, DecisionRuleKind, EffectiveDecisionRule, InventoryRotation,
    MAX_CONFIGURATION_CURRENCY_LENGTH, MAX_CONFIGURATION_RATE_MINOR, MAX_PERCENTAGE_BASIS_POINTS,
    MAX_WAVE_ORDERS,
};
pub use cross_dock::{
    plan_cross_dock, CrossDockCancellationDetails, CrossDockCancellationReason,
    CrossDockClaimReleaseReason, CrossDockError, CrossDockNote, CrossDockPlanDecision,
    CrossDockPlanningSnapshot, CrossDockQuantity, CrossDockScanValue, CrossDockUom,
    CrossDockWorkStatus, MAX_CROSS_DOCK_NOTE_LENGTH, MAX_CROSS_DOCK_SCAN_VALUE_LENGTH,
    MAX_CROSS_DOCK_UOM_LENGTH,
};
pub use customer_return::{
    cancel_customer_return, plan_customer_return, CustomerReturnCancellationDetails,
    CustomerReturnCancellationReason, CustomerReturnError, CustomerReturnLineDefinition,
    CustomerReturnLoadPlanDetails, CustomerReturnNumber, CustomerReturnQuantity,
    CustomerReturnReason, CustomerReturnReference, CustomerReturnRevision, CustomerReturnStatus,
    NewCustomerReturn, MAX_CUSTOMER_RETURN_IDENTITY_LENGTH, MAX_CUSTOMER_RETURN_NOTE_LENGTH,
    MAX_CUSTOMER_RETURN_NUMBER_LENGTH, MAX_CUSTOMER_RETURN_REFERENCE_LENGTH,
};
pub use cycle_count::{
    decide_cycle_count_disposition, decide_cycle_count_disposition_with_approval_threshold,
    CycleCountDisposition, CycleCountError, CycleCountPolicyRevision, CycleCountTolerancePolicy,
    CycleCountVarianceDecision, CycleCountVarianceDecisionDetails, CycleCountVarianceNote,
    CycleCountVarianceReason, CycleCountVarianceRevision, CycleCountVarianceStatus,
    MAX_CYCLE_COUNT_PERCENTAGE_BASIS_POINTS, MAX_CYCLE_COUNT_RECOUNTS,
    MAX_CYCLE_COUNT_VARIANCE_NOTE_LENGTH,
};
pub use data_cell::*;
pub use data_cell_move::*;
pub use facility::{
    FacilityRevision, FacilityShippingOrigin, FacilityShippingOriginError,
    FacilityShippingOriginField, MAX_FACILITY_ORIGIN_ADDRESS_LINE_LENGTH,
    MAX_FACILITY_ORIGIN_CITY_LENGTH, MAX_FACILITY_ORIGIN_COMPANY_LENGTH,
    MAX_FACILITY_ORIGIN_COUNTRY_LENGTH, MAX_FACILITY_ORIGIN_EMAIL_LENGTH,
    MAX_FACILITY_ORIGIN_NAME_LENGTH, MAX_FACILITY_ORIGIN_PHONE_LENGTH,
    MAX_FACILITY_ORIGIN_POSTAL_CODE_LENGTH, MAX_FACILITY_ORIGIN_STATE_LENGTH,
};
pub use inbound_asn::{
    cancel_inbound_asn, plan_inbound_asn, InboundAsnCancellationDetails,
    InboundAsnCancellationNote, InboundAsnCancellationReason, InboundAsnError,
    InboundAsnLineDefinition, InboundAsnLoadPlanDetails, InboundAsnNumber, InboundAsnQuantity,
    InboundAsnRevision, InboundAsnStatus, InboundAsnSupplier, NewInboundAsn, NewPurchaseOrderAsn,
    PurchaseOrderAsnLineDefinition, MAX_INBOUND_ASN_CANCELLATION_NOTE_LENGTH,
    MAX_INBOUND_ASN_IDENTITY_LENGTH, MAX_INBOUND_ASN_NUMBER_LENGTH,
    MAX_INBOUND_ASN_SUPPLIER_LENGTH,
};
pub use inbound_inspection::{
    decide_inbound_inspection, InboundInspectionError, InboundInspectionNote,
    InboundInspectionOutcome, InboundInspectionTargetStatus, MAX_INBOUND_INSPECTION_NOTE_LENGTH,
};
pub use inbound_load::{
    validate_inbound_load_appointment, validate_inbound_load_appointment_reschedule,
    validate_inbound_load_arrival, validate_inbound_load_closure,
    validate_inbound_load_unloading_start, InboundExpectedQuantity, InboundLoadAppointmentError,
    InboundLoadAppointmentRescheduleDetails, InboundLoadAppointmentRescheduleError,
    InboundLoadAppointmentRescheduleNote, InboundLoadAppointmentRescheduleReason,
    InboundLoadArrivalError, InboundLoadCancellationDetails, InboundLoadCancellationError,
    InboundLoadCancellationNote, InboundLoadCancellationReason, InboundLoadClosureError,
    InboundLoadField, InboundLoadPlanLine, InboundLoadPlanningError, InboundLoadPreArrivalStatus,
    InboundLoadReference, InboundLoadRejectionDetails, InboundLoadRejectionError,
    InboundLoadRejectionNote, InboundLoadRejectionReason, InboundLoadScanValue,
    InboundLoadUnloadingError, NewInboundLoadPlan,
    MAX_INBOUND_LOAD_APPOINTMENT_RESCHEDULE_NOTE_LENGTH, MAX_INBOUND_LOAD_CANCELLATION_NOTE_LENGTH,
    MAX_INBOUND_LOAD_IDENTITY_LENGTH, MAX_INBOUND_LOAD_REFERENCE_LENGTH,
    MAX_INBOUND_LOAD_REJECTION_NOTE_LENGTH, MAX_INBOUND_LOAD_SCAN_VALUE_LENGTH,
    MAX_INBOUND_LOAD_TEXT_LENGTH,
};
pub use integration::{
    IntegrationInboxCorrectionReason, IntegrationInboxCorrectionReasonError,
    IntegrationInboxProcessingRevision, IntegrationInboxProcessingStatus,
    OutboxDeadLetterDiscardReason, OutboxDeadLetterDiscardReasonError,
    MAX_INTEGRATION_INBOX_CORRECTION_REASON_LENGTH, MAX_INTEGRATION_PROCESSING_ERROR_CODE_LENGTH,
    MAX_INTEGRATION_PROCESSING_ERROR_MESSAGE_LENGTH, MAX_OUTBOX_DEAD_LETTER_DISCARD_REASON_LENGTH,
};
pub use integration_mapping::{
    ExternalInventoryOwnerKey, ExternalItemKey, ExternalItemUom, IntegrationMappedUom,
    IntegrationMappingError, IntegrationOrderItemMappingDefinition,
    IntegrationOrderItemMappingRevision, IntegrationOrderItemMappingStatus,
    IntegrationOrderOwnerMappingDefinition, IntegrationOrderOwnerMappingRevision,
    IntegrationOrderOwnerMappingStatus, IntegrationSourceKey,
    MAX_EXTERNAL_INVENTORY_OWNER_KEY_LENGTH, MAX_EXTERNAL_ITEM_KEY_LENGTH,
    MAX_EXTERNAL_ITEM_UOM_LENGTH, MAX_INTEGRATION_SOURCE_KEY_LENGTH,
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
pub use labor::*;
pub use license_plate::{
    validate_license_plate_attachment, LicensePlateAttachmentError, LicensePlateAttachmentSnapshot,
    MAX_LICENSE_PLATE_HIERARCHY_DEPTH, MAX_LICENSE_PLATE_HIERARCHY_NODES,
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
pub use pick_cluster::{
    derive_pick_batch_evidence, validate_pick_cart_slot_count, validate_pick_cluster_plan,
    PickBatchEvidence, PickBatchPlanLine, PickCartBarcode, PickCartName, PickCartSlotCode,
    PickCartStatus, PickClusterError, PickClusterPlanLine, PickClusterStatus, PickExecutionMethod,
    PickRouteMode, MAX_PICK_CART_BARCODE_LENGTH, MAX_PICK_CART_NAME_LENGTH, MAX_PICK_CART_SLOTS,
    MAX_PICK_CART_SLOT_CODE_LENGTH, MAX_PICK_CLUSTER_CANCEL_NOTE_LENGTH, MAX_PICK_CLUSTER_TASKS,
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
pub use purchase_order::{
    cancel_purchase_order, release_purchase_order, NewPurchaseOrder,
    PurchaseOrderCancellationDetails, PurchaseOrderCancellationNote,
    PurchaseOrderCancellationReason, PurchaseOrderDemandCoverage, PurchaseOrderError,
    PurchaseOrderLineDefinition, PurchaseOrderNumber, PurchaseOrderQuantity, PurchaseOrderRevision,
    PurchaseOrderStatus, PurchaseOrderSupplier, MAX_PURCHASE_ORDER_CANCELLATION_NOTE_LENGTH,
    MAX_PURCHASE_ORDER_NUMBER_LENGTH, MAX_PURCHASE_ORDER_SUPPLIER_LENGTH,
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
pub use service_account::{
    ServiceAccountAccessPolicy, ServiceAccountBearerToken, ServiceAccountCredentialLabel,
    ServiceAccountDescription, ServiceAccountError, ServiceAccountName, ServiceAccountReason,
    ServiceAccountRevision, ServiceAccountStatus, MAX_SERVICE_ACCOUNT_DESCRIPTION_LENGTH,
    MAX_SERVICE_ACCOUNT_LABEL_LENGTH, MAX_SERVICE_ACCOUNT_NAME_LENGTH,
    MAX_SERVICE_ACCOUNT_PERMISSION_LENGTH, MAX_SERVICE_ACCOUNT_REASON_LENGTH,
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
pub use slotting::*;
pub use storage_zone::{
    StorageZoneCode, StorageZoneDefinition, StorageZoneError, StorageZoneLocationIds,
    StorageZoneName, StorageZonePurpose, StorageZoneRevision, StorageZoneStatus,
    StorageZoneTravelSequence, MAX_STORAGE_ZONE_CODE_LENGTH, MAX_STORAGE_ZONE_NAME_LENGTH,
};
pub use support_access::{
    validate_support_access_window, SupportAccessError, SupportAccessPolicy, SupportAccessReason,
    SupportAccessRevision, SupportAccessStatus, MAX_SUPPORT_ACCESS_DURATION_HOURS,
    MAX_SUPPORT_ACCESS_PERMISSION_LENGTH, MAX_SUPPORT_ACCESS_REASON_LENGTH,
};
pub use tenant::{
    TenantLifecycleError, TenantLifecycleReason, TenantName, TenantRevision, TenantSlug,
    TenantStatus, MAX_TENANT_LIFECYCLE_REASON_LENGTH, MAX_TENANT_NAME_LENGTH,
    MAX_TENANT_SLUG_LENGTH,
};
pub use transfer_order::{
    cancel_transfer_order, dispatch_transfer_order, receive_transfer_order, release_transfer_order,
    NewTransferOrder, TransferDispatchExecution, TransferDispatchSelection,
    TransferOrderCancellationDetails, TransferOrderCancellationNote,
    TransferOrderCancellationReason, TransferOrderError, TransferOrderLineDefinition,
    TransferOrderNumber, TransferOrderQuantity, TransferOrderRevision, TransferOrderScanValue,
    TransferOrderStatus, MAX_TRANSFER_ORDER_CANCELLATION_NOTE_LENGTH,
    MAX_TRANSFER_ORDER_NUMBER_LENGTH,
};
pub use value_added_work::{
    validate_value_added_quantities, validate_value_added_shape, ValueAddedInventoryStatus,
    ValueAddedQuantity, ValueAddedRevision, ValueAddedWorkError, ValueAddedWorkKind,
    ValueAddedWorkNote, ValueAddedWorkNumber, ValueAddedWorkStatus, MAX_VALUE_ADDED_WORK_LINES,
    MAX_VALUE_ADDED_WORK_NOTE_LENGTH, MAX_VALUE_ADDED_WORK_NUMBER_LENGTH,
};
pub use vendor_return::*;
pub use work_orchestration::*;
pub use workforce::*;
pub use yard::{
    calculate_yard_detention, YardAppointmentNumber, YardAppointmentStatus, YardAppointmentWindow,
    YardAssetKind, YardAssetNumber, YardDetention, YardDirection, YardError, YardFreeMinutes,
    YardLocationCode, YardLocationKind, YardName, YardNote, YardOperation, YardRevision,
    YardVisitStatus, MAX_YARD_CODE_LENGTH, MAX_YARD_FREE_MINUTES, MAX_YARD_NAME_LENGTH,
    MAX_YARD_NOTE_LENGTH,
};

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
positive_id!(InboundLoadId, "inbound load ID");
positive_id!(InboundLoadLineId, "inbound load line ID");
positive_id!(InboundLoadArrivalId, "inbound load arrival ID");
positive_id!(InboundLoadAppointmentId, "inbound load appointment ID");
positive_id!(
    InboundLoadAppointmentRescheduleId,
    "inbound load appointment reschedule ID"
);
positive_id!(InboundLoadCancellationId, "inbound load cancellation ID");
positive_id!(InboundLoadRejectionId, "inbound load rejection ID");
positive_id!(InboundLoadClosureId, "inbound load closure ID");
positive_id!(
    InboundLoadUnloadingStartId,
    "inbound load unloading start ID"
);
positive_id!(
    FacilityShippingOriginConfigurationId,
    "facility shipping origin configuration ID"
);
positive_id!(UserId, "user ID");
positive_id!(ServiceAccountId, "service account ID");
positive_id!(ServiceAccountCredentialId, "service account credential ID");
positive_id!(SupportAccessGrantId, "support access grant ID");
positive_id!(DataCellId, "data cell ID");
positive_id!(TenantCellMoveId, "tenant cell move ID");
positive_id!(EmployeeId, "employee ID");
positive_id!(EmployeeIdentityChangeId, "employee identity change ID");
positive_id!(LaborSkillId, "labor skill ID");
positive_id!(EmployeeCertificationId, "employee certification ID");
positive_id!(EquipmentClassId, "equipment class ID");
positive_id!(EquipmentAssetId, "equipment asset ID");
positive_id!(LaborStandardId, "labor standard ID");
positive_id!(AttendanceIntervalId, "attendance interval ID");
positive_id!(AttendanceAdjustmentId, "attendance adjustment ID");
positive_id!(LaborActivityId, "labor activity ID");
positive_id!(LaborActivityAdjustmentId, "labor activity adjustment ID");
positive_id!(OrderId, "order ID");
positive_id!(OrderLineId, "order line ID");
positive_id!(OrderAmendmentId, "order amendment ID");
positive_id!(OrderLineAmendmentId, "order line amendment ID");
positive_id!(OrderCancellationId, "order cancellation ID");
positive_id!(BackorderPolicyId, "backorder policy ID");
positive_id!(BackorderSplitId, "backorder split ID");
positive_id!(ConfigurationVersionId, "configuration version ID");
positive_id!(BillingContractId, "billing contract ID");
positive_id!(BillingRateId, "billing rate ID");
positive_id!(BillableEventId, "billable event ID");
positive_id!(BillingStorageSnapshotId, "billing storage snapshot ID");
positive_id!(BillingChargeId, "billing charge ID");
positive_id!(BillingReconciliationRunId, "billing reconciliation run ID");
positive_id!(BillingFinancialExportId, "billing financial export ID");
positive_id!(YardLocationId, "yard location ID");
positive_id!(YardAssetId, "yard asset ID");
positive_id!(YardAppointmentId, "yard appointment ID");
positive_id!(YardAppointmentEventId, "yard appointment event ID");
positive_id!(YardVisitId, "yard visit ID");
positive_id!(YardVisitEventId, "yard visit event ID");
positive_id!(YardDetentionId, "yard detention ID");
positive_id!(CycleCountPolicyId, "cycle count policy ID");
positive_id!(CycleCountVarianceId, "cycle count variance ID");
positive_id!(CustomerReturnId, "customer return ID");
positive_id!(CustomerReturnLineId, "customer return line ID");
positive_id!(CustomerReturnLoadPlanId, "customer return load plan ID");
positive_id!(
    CustomerReturnCancellationId,
    "customer return cancellation ID"
);
positive_id!(CrossDockPlanId, "cross-dock plan ID");
positive_id!(CrossDockWorkId, "cross-dock work ID");
positive_id!(CrossDockConfirmationId, "cross-dock confirmation ID");
positive_id!(CrossDockCancellationId, "cross-dock cancellation ID");
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
positive_id!(DynamicReleaseRunId, "dynamic release run ID");
positive_id!(AutomationDeviceId, "automation device ID");
positive_id!(AutomationCommandId, "automation command ID");
positive_id!(AutomationHeartbeatId, "automation heartbeat ID");
positive_id!(PickCartId, "pick cart ID");
positive_id!(PickCartSlotId, "pick cart slot ID");
positive_id!(PickClusterId, "pick cluster ID");
positive_id!(PickClusterMemberId, "pick cluster member ID");
positive_id!(PickZoneClaimId, "pick zone claim ID");
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
positive_id!(
    InventoryReconciliationRunId,
    "inventory reconciliation run ID"
);
positive_id!(StorageZoneId, "storage zone ID");
positive_id!(SlottingProfileId, "slotting profile ID");
positive_id!(SlottingRunId, "slotting run ID");
positive_id!(SlottingRecommendationId, "slotting recommendation ID");
positive_id!(WorkOrchestrationPolicyId, "work orchestration policy ID");
positive_id!(WorkOrchestrationSignalId, "work orchestration signal ID");
positive_id!(WorkOrchestrationPlanId, "work orchestration plan ID");
positive_id!(
    WorkOrchestrationPlanItemId,
    "work orchestration plan item ID"
);
positive_id!(
    WorkOrchestrationDispatchId,
    "work orchestration dispatch ID"
);
positive_id!(InboundAsnId, "inbound ASN ID");
positive_id!(InboundAsnLineId, "inbound ASN line ID");
positive_id!(InboundAsnLoadPlanId, "inbound ASN load plan ID");
positive_id!(InboundAsnCancellationId, "inbound ASN cancellation ID");
positive_id!(PurchaseOrderId, "purchase order ID");
positive_id!(PurchaseOrderLineId, "purchase order line ID");
positive_id!(PurchaseOrderReleaseId, "purchase order release ID");
positive_id!(
    PurchaseOrderCancellationId,
    "purchase order cancellation ID"
);
positive_id!(PurchaseOrderAsnSourceId, "purchase order ASN source ID");
positive_id!(
    PurchaseOrderAsnSourceLineId,
    "purchase order ASN source line ID"
);
positive_id!(TransferOrderId, "transfer order ID");
positive_id!(TransferOrderLineId, "transfer order line ID");
positive_id!(TransferOrderReleaseId, "transfer order release ID");
positive_id!(TransferOrderDispatchId, "transfer order dispatch ID");
positive_id!(
    TransferOrderDispatchLineId,
    "transfer order dispatch line ID"
);
positive_id!(TransferOrderReceiptId, "transfer order receipt ID");
positive_id!(TransferOrderReceiptLineId, "transfer order receipt line ID");
positive_id!(
    TransferOrderCancellationId,
    "transfer order cancellation ID"
);
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
positive_id!(OutboxDeadLetterDiscardId, "outbox dead-letter discard ID");
positive_id!(
    IntegrationInboxProcessingId,
    "integration inbox processing ID"
);
positive_id!(
    IntegrationInboxProcessingAttemptId,
    "integration inbox processing attempt ID"
);
positive_id!(
    IntegrationInboxCorrectionId,
    "integration inbox correction ID"
);
positive_id!(
    IntegrationOrderItemMappingId,
    "integration order item mapping ID"
);
positive_id!(
    IntegrationOrderOwnerMappingId,
    "integration order owner mapping ID"
);
positive_id!(PackSessionId, "pack session ID");
positive_id!(CartonId, "carton ID");
positive_id!(CartonContentId, "carton content ID");
positive_id!(CartonContentRemovalId, "carton content removal ID");
positive_id!(CartonReopeningId, "carton reopening ID");
positive_id!(CartonWeightEvidenceId, "carton weight evidence ID");
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
positive_id!(CarrierAccountId, "carrier account ID");
positive_id!(CarrierManifestJobId, "carrier manifest job ID");
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
positive_id!(ValueAddedWorkId, "value-added work ID");
positive_id!(ValueAddedWorkInputId, "value-added work input ID");
positive_id!(ValueAddedWorkOutputId, "value-added work output ID");
positive_id!(ValueAddedWorkEventId, "value-added work event ID");
positive_id!(VendorReturnId, "vendor return ID");
positive_id!(VendorReturnLineId, "vendor return line ID");
positive_id!(VendorReturnEventId, "vendor return event ID");

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
