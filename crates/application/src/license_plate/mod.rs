//! Application contracts for nested license-plate/container hierarchy.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{FacilityId, InventoryOwnerId, Timestamp, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LicensePlateHierarchyAction {
    Attached,
    Detached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicensePlateHierarchyNodeReadModel {
    pub license_plate_id: i64,
    pub barcode: Option<String>,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub location_id: Option<i64>,
    pub parent_license_plate_id: Option<i64>,
    pub root_license_plate_id: i64,
    pub depth: u8,
    pub hierarchy_revision: i64,
    pub direct_child_ids: Vec<i64>,
    pub descendant_ids: Vec<i64>,
    pub direct_unit_quantity: i64,
    pub contained_unit_quantity: i64,
    pub hierarchy_updated_at: Option<Timestamp>,
    pub hierarchy_updated_by: Option<UserId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicensePlateHierarchyEventReadModel {
    pub event_id: i64,
    pub child_license_plate_id: i64,
    pub previous_parent_license_plate_id: Option<i64>,
    pub parent_license_plate_id: Option<i64>,
    pub resulting_revision: i64,
    pub action: LicensePlateHierarchyAction,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicensePlateHierarchyReadModel {
    pub node: LicensePlateHierarchyNodeReadModel,
    pub ancestors: Vec<LicensePlateHierarchyNodeReadModel>,
    pub descendants: Vec<LicensePlateHierarchyNodeReadModel>,
    pub events: Vec<LicensePlateHierarchyEventReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangeLicensePlateParentCommand {
    pub license_plate_id: i64,
    pub parent_license_plate_id: Option<i64>,
    pub expected_revision: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeLicensePlateParentResult {
    pub license_plate_id: i64,
    pub previous_parent_license_plate_id: Option<i64>,
    pub parent_license_plate_id: Option<i64>,
    pub root_license_plate_id: i64,
    pub depth: u8,
    pub resulting_revision: i64,
    pub changed_at: Timestamp,
    pub changed_by: UserId,
}
