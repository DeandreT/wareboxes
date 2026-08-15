//! Replay-safe relabeling, refurbishment, kitting, de-kitting, assembly, and VAS work.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    BillableEventId, FacilityId, InventoryBalanceId, InventoryHoldId, InventoryOwnerId,
    ItemBatchId, LicensePlateId, LocationId, Timestamp, UserId, ValueAddedInventoryStatus,
    ValueAddedQuantity, ValueAddedRevision, ValueAddedWorkEventId, ValueAddedWorkId,
    ValueAddedWorkInputId, ValueAddedWorkKind, ValueAddedWorkNote, ValueAddedWorkNumber,
    ValueAddedWorkOutputId, ValueAddedWorkStatus,
};

pub const CREATE_VALUE_ADDED_WORK_OPERATION: &str = "value_added_work.create.v1";
pub const RELEASE_VALUE_ADDED_WORK_OPERATION: &str = "value_added_work.release.v1";
pub const COMPLETE_VALUE_ADDED_WORK_OPERATION: &str = "value_added_work.complete.v1";
pub const CANCEL_VALUE_ADDED_WORK_OPERATION: &str = "value_added_work.cancel.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateValueAddedWorkInput {
    pub inventory_balance_id: InventoryBalanceId,
    pub quantity: ValueAddedQuantity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateValueAddedWorkOutput {
    pub location_id: LocationId,
    pub license_plate_id: Option<LicensePlateId>,
    pub item_batch_id: ItemBatchId,
    pub inventory_status: ValueAddedInventoryStatus,
    pub quantity: ValueAddedQuantity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateValueAddedWorkCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub number: ValueAddedWorkNumber,
    pub kind: ValueAddedWorkKind,
    pub note: Option<ValueAddedWorkNote>,
    pub inputs: Vec<CreateValueAddedWorkInput>,
    pub outputs: Vec<CreateValueAddedWorkOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValueAddedWorkLifecycleCommand {
    pub work_id: ValueAddedWorkId,
    pub expected_revision: ValueAddedRevision,
    pub note: ValueAddedWorkNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueAddedWorkInputReadModel {
    pub input_id: ValueAddedWorkInputId,
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
    pub inventory_status: ValueAddedInventoryStatus,
    pub quantity: ValueAddedQuantity,
    pub hold_id: Option<InventoryHoldId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueAddedWorkOutputReadModel {
    pub output_id: ValueAddedWorkOutputId,
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
    pub inventory_status: ValueAddedInventoryStatus,
    pub quantity: ValueAddedQuantity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueAddedWorkEventReadModel {
    pub event_id: ValueAddedWorkEventId,
    pub from_status: Option<ValueAddedWorkStatus>,
    pub to_status: ValueAddedWorkStatus,
    pub note: Option<String>,
    pub resulting_revision: ValueAddedRevision,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueAddedWorkReadModel {
    pub work_id: ValueAddedWorkId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub number: String,
    pub kind: ValueAddedWorkKind,
    pub status: ValueAddedWorkStatus,
    pub revision: ValueAddedRevision,
    pub note: Option<String>,
    pub inputs: Vec<ValueAddedWorkInputReadModel>,
    pub outputs: Vec<ValueAddedWorkOutputReadModel>,
    pub completion_inventory_transaction_id: Option<i64>,
    pub billable_event_id: Option<BillableEventId>,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub released_by: Option<UserId>,
    pub released_at: Option<Timestamp>,
    pub completed_by: Option<UserId>,
    pub completed_at: Option<Timestamp>,
    pub cancelled_by: Option<UserId>,
    pub cancelled_at: Option<Timestamp>,
    pub events: Vec<ValueAddedWorkEventReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueAddedWorkPage {
    pub items: Vec<ValueAddedWorkReadModel>,
    pub next_before_id: Option<ValueAddedWorkId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueAddedWorkFilter {
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub status: Option<ValueAddedWorkStatus>,
    pub before_id: Option<ValueAddedWorkId>,
    pub limit: u32,
}
