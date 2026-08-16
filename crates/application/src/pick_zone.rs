//! Zone-directed pick queue commands and read models.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, PickZoneClaimId, StorageZoneId, StorageZoneRevision,
    StorageZoneTravelSequence, Timestamp,
};

pub const CLAIM_NEXT_ZONE_PICK_OPERATION: &str = "picking.zone.claim_next.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClaimNextZonePickCommand {
    pub storage_zone_id: StorageZoneId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickZoneWorkspaceQuery {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickZoneQueueReadModel {
    pub storage_zone_id: StorageZoneId,
    pub code: String,
    pub name: String,
    pub revision: StorageZoneRevision,
    pub travel_sequence: StorageZoneTravelSequence,
    pub open_task_count: i64,
    pub active_task_count: i64,
    pub oldest_open_task_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickZoneWorkspace {
    pub queues: Vec<PickZoneQueueReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickZoneClaimEvidence {
    pub zone_claim_id: PickZoneClaimId,
    pub storage_zone_id: StorageZoneId,
    pub storage_zone_code: String,
    pub storage_zone_revision: i64,
    pub storage_zone_travel_sequence: StorageZoneTravelSequence,
}
