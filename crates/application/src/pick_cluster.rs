//! Cluster-cart planning and sequential RF execution contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, OrderId, PickCartBarcode, PickCartId, PickCartName,
    PickCartSlotCode, PickCartSlotId, PickCartStatus, PickClusterId, PickClusterMemberId,
    PickClusterStatus, PickRouteMode, PickTaskId, Timestamp, UserId,
};

pub const CREATE_PICK_CART_OPERATION: &str = "picking.cart.create.v1";
pub const CHANGE_PICK_CART_STATUS_OPERATION: &str = "picking.cart.status_change.v1";
pub const PLAN_PICK_CLUSTER_OPERATION: &str = "picking.cluster.plan.v1";
pub const CLAIM_NEXT_CLUSTER_PICK_OPERATION: &str = "picking.cluster.claim_next.v1";
pub const CANCEL_PICK_CLUSTER_OPERATION: &str = "picking.cluster.cancel.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreatePickCartCommand {
    pub facility_id: FacilityId,
    pub barcode: PickCartBarcode,
    pub name: PickCartName,
    pub slot_codes: Vec<PickCartSlotCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ChangePickCartStatusCommand {
    pub cart_id: PickCartId,
    pub expected_revision: i64,
    pub status: PickCartStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PickClusterTaskAssignment {
    pub task_id: PickTaskId,
    pub slot_id: PickCartSlotId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanPickClusterCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub cart_id: PickCartId,
    pub assignments: Vec<PickClusterTaskAssignment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClaimNextClusterPickCommand {
    pub cluster_id: PickClusterId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelPickClusterCommand {
    pub cluster_id: PickClusterId,
    pub expected_revision: i64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickCartSlotReadModel {
    pub slot_id: PickCartSlotId,
    pub code: PickCartSlotCode,
    pub sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickCartReadModel {
    pub cart_id: PickCartId,
    pub facility_id: FacilityId,
    pub barcode: PickCartBarcode,
    pub name: PickCartName,
    pub status: PickCartStatus,
    pub revision: i64,
    pub slots: Vec<PickCartSlotReadModel>,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub status_changed_by: Option<UserId>,
    pub status_changed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickClusterCandidateReadModel {
    pub task_id: PickTaskId,
    pub order_id: OrderId,
    pub order_key: String,
    pub source_location_id: i64,
    pub source_inventory_balance_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub source_travel_sequence: i64,
    pub item_id: i64,
    pub item_batch_id: i64,
    pub item_description: String,
    pub uom: String,
    pub inventory_status: String,
    pub planned_quantity: i64,
    pub priority: i64,
    pub ship_by: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickClusterMemberReadModel {
    pub member_id: PickClusterMemberId,
    pub sequence: i64,
    pub task_id: PickTaskId,
    pub task_status: String,
    pub order_id: OrderId,
    pub order_key: String,
    pub slot_id: PickCartSlotId,
    pub slot_code: PickCartSlotCode,
    pub source_location_id: i64,
    pub source_inventory_balance_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub item_id: i64,
    pub item_batch_id: i64,
    pub item_description: String,
    pub uom: String,
    pub inventory_status: String,
    pub planned_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickClusterReadModel {
    pub cluster_id: PickClusterId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub cart_id: PickCartId,
    pub cart_barcode: PickCartBarcode,
    pub cart_name: PickCartName,
    pub mode: PickRouteMode,
    pub batch_source_inventory_balance_id: Option<i64>,
    pub batch_source_location_id: Option<i64>,
    pub batch_source_location_barcode: Option<String>,
    pub batch_item_batch_id: Option<i64>,
    pub batch_uom: Option<String>,
    pub batch_inventory_status: Option<String>,
    pub batch_total_quantity: Option<i64>,
    pub status: PickClusterStatus,
    pub revision: i64,
    pub task_count: i64,
    pub order_count: i64,
    pub completed_task_count: i64,
    pub assigned_user_id: Option<UserId>,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub cancelled_by: Option<UserId>,
    pub cancelled_at: Option<Timestamp>,
    pub cancellation_note: Option<String>,
    pub members: Vec<PickClusterMemberReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickClusterWorkspace {
    pub carts: Vec<PickCartReadModel>,
    pub candidates: Vec<PickClusterCandidateReadModel>,
    pub clusters: Vec<PickClusterReadModel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickClusterWorkspaceQuery {
    pub facility_id: FacilityId,
    pub inventory_owner_id: InventoryOwnerId,
    pub include_history: bool,
}
