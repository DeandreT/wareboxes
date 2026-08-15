//! Replay-safe vendor-return planning, reservation, shipping, and cancellation.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    BillableEventId, FacilityId, InventoryBalanceId, InventoryHoldId, InventoryOwnerId,
    ItemBatchId, LicensePlateId, LocationId, Timestamp, UserId, VendorName, VendorReference,
    VendorReturnEventId, VendorReturnId, VendorReturnLineId, VendorReturnNote, VendorReturnNumber,
    VendorReturnQuantity, VendorReturnReason, VendorReturnRevision, VendorReturnStatus,
};

pub const CREATE_VENDOR_RETURN_OPERATION: &str = "vendor_return.create.v1";
pub const RELEASE_VENDOR_RETURN_OPERATION: &str = "vendor_return.release.v1";
pub const SHIP_VENDOR_RETURN_OPERATION: &str = "vendor_return.ship.v1";
pub const CANCEL_VENDOR_RETURN_OPERATION: &str = "vendor_return.cancel.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateVendorReturnLine {
    pub inventory_balance_id: InventoryBalanceId,
    pub quantity: VendorReturnQuantity,
    pub reason: VendorReturnReason,
    pub note: Option<VendorReturnNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateVendorReturnCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub number: VendorReturnNumber,
    pub vendor_name: VendorName,
    pub vendor_reference: Option<VendorReference>,
    pub note: Option<VendorReturnNote>,
    pub lines: Vec<CreateVendorReturnLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorReturnLifecycleCommand {
    pub vendor_return_id: VendorReturnId,
    pub expected_revision: VendorReturnRevision,
    pub note: VendorReturnNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorReturnLineReadModel {
    pub line_id: VendorReturnLineId,
    pub inventory_balance_id: InventoryBalanceId,
    pub location_id: LocationId,
    pub location_code: String,
    pub license_plate_id: Option<LicensePlateId>,
    pub license_plate_number: Option<String>,
    pub item_batch_id: ItemBatchId,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub inventory_status: String,
    pub quantity: VendorReturnQuantity,
    pub reason: VendorReturnReason,
    pub note: Option<String>,
    pub hold_id: Option<InventoryHoldId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorReturnEventReadModel {
    pub event_id: VendorReturnEventId,
    pub from_status: Option<VendorReturnStatus>,
    pub to_status: VendorReturnStatus,
    pub note: Option<String>,
    pub resulting_revision: VendorReturnRevision,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorReturnReadModel {
    pub vendor_return_id: VendorReturnId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub number: String,
    pub vendor_name: String,
    pub vendor_reference: Option<String>,
    pub status: VendorReturnStatus,
    pub revision: VendorReturnRevision,
    pub note: Option<String>,
    pub lines: Vec<VendorReturnLineReadModel>,
    pub shipment_inventory_transaction_id: Option<i64>,
    pub billable_event_id: Option<BillableEventId>,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub released_by: Option<UserId>,
    pub released_at: Option<Timestamp>,
    pub shipped_by: Option<UserId>,
    pub shipped_at: Option<Timestamp>,
    pub cancelled_by: Option<UserId>,
    pub cancelled_at: Option<Timestamp>,
    pub events: Vec<VendorReturnEventReadModel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorReturnFilter {
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub status: Option<VendorReturnStatus>,
    pub before_id: Option<VendorReturnId>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorReturnPage {
    pub items: Vec<VendorReturnReadModel>,
    pub next_before_id: Option<VendorReturnId>,
}
