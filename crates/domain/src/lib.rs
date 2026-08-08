//! Domain identifiers and invariants shared across application boundaries.

mod allocation;
mod facility;
mod order;
mod order_cancellation;
mod order_release;
mod packing;
mod picking;
mod shipping;
mod tenant;

pub use allocation::{
    assess_order_allocation_readiness, plan_fefo_allocation, AllocationCandidate,
    AllocationExecutionStage, AllocationOutcome, AllocationPlan, AllocationPlanError,
    AllocationQuantity, AllocationShortageReason, AllocationStrategy, OrderAllocationBlockReason,
    OrderAllocationReadiness, OrderRevision, PlannedAllocation,
};
pub use facility::{
    FacilityRevision, FacilityShippingOrigin, FacilityShippingOriginError,
    FacilityShippingOriginField, MAX_FACILITY_ORIGIN_ADDRESS_LINE_LENGTH,
    MAX_FACILITY_ORIGIN_CITY_LENGTH, MAX_FACILITY_ORIGIN_COMPANY_LENGTH,
    MAX_FACILITY_ORIGIN_COUNTRY_LENGTH, MAX_FACILITY_ORIGIN_EMAIL_LENGTH,
    MAX_FACILITY_ORIGIN_NAME_LENGTH, MAX_FACILITY_ORIGIN_PHONE_LENGTH,
    MAX_FACILITY_ORIGIN_POSTAL_CODE_LENGTH, MAX_FACILITY_ORIGIN_STATE_LENGTH,
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
pub use order_cancellation::{
    CancellationNote, OrderCancellationDetails, OrderCancellationError, OrderCancellationReason,
    MAX_CANCELLATION_NOTE_LENGTH,
};
pub use order_release::{release_order, OrderReleaseError};
pub use packing::{
    begin_packing, complete_packing, continue_packing, open_carton, CartonDimensions,
    CartonMeasurements, CartonStatus, DimensionMillimeters, PackQuantity, PackScanValue,
    PackSessionStatus, PackingError, PackingProgress, WeightGrams, MAX_PACK_SCAN_VALUE_LENGTH,
};
pub use picking::{
    ActualPickQuantity, PickClaimReleaseReason, PickContentState, PickQuantity, PickScanValue,
    PickShortageDetails, PickShortageNote, PickShortageQuantities, PickShortageReason,
    PickShortageRevision, PickShortageStatus, PickingError, MAX_PICK_SCAN_VALUE_LENGTH,
    MAX_PICK_SHORTAGE_NOTE_LENGTH,
};
pub use shipping::{
    confirm_shipment_departure, create_shipment, record_manual_manifest, CarrierCode,
    CarrierServiceCode, CartonTrackingAssignment, ManifestReference, ShipmentCartonIdentity,
    ShipmentDepartureTransition, ShipmentRevision, ShipmentScanValue, ShipmentStatus,
    ShippingError, ShippingTextField, TrackingNumber, MAX_CARRIER_CODE_LENGTH,
    MAX_CARRIER_SERVICE_CODE_LENGTH, MAX_MANIFEST_REFERENCE_LENGTH, MAX_SHIPMENT_SCAN_VALUE_LENGTH,
    MAX_TRACKING_NUMBER_LENGTH,
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
positive_id!(OrderCancellationId, "order cancellation ID");
positive_id!(OrderReleaseId, "order release ID");
positive_id!(PickTaskId, "pick task ID");
positive_id!(PickContentId, "pick content ID");
positive_id!(PickShortageId, "pick shortage ID");
positive_id!(
    PickShortageReallocationRunId,
    "pick shortage reallocation run ID"
);
positive_id!(InventoryHoldId, "inventory hold ID");
positive_id!(PackSessionId, "pack session ID");
positive_id!(CartonId, "carton ID");
positive_id!(CartonContentId, "carton content ID");
positive_id!(ShipmentId, "shipment ID");
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
