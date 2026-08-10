//! Facility storage-zone configuration and scoped read contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, LocationId, StorageZoneDefinition, StorageZoneId, StorageZonePurpose,
    StorageZoneRevision, StorageZoneStatus, StorageZoneTravelSequence, Timestamp, UserId,
};

pub const CONFIGURE_STORAGE_ZONE_OPERATION: &str = "topology.storage_zone.configure.v1";
pub const RETIRE_STORAGE_ZONE_OPERATION: &str = "topology.storage_zone.retire.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureStorageZoneCommand {
    pub definition: StorageZoneDefinition,
    pub expected_revision: Option<StorageZoneRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RetireStorageZoneCommand {
    pub storage_zone_id: StorageZoneId,
    pub expected_revision: StorageZoneRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageZoneLocationReadModel {
    pub location_id: LocationId,
    pub barcode: String,
    pub name: Option<String>,
    pub location_type: String,
    pub pickable: bool,
    pub receivable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageZoneReadModel {
    pub storage_zone_id: StorageZoneId,
    pub facility_name: String,
    pub definition: StorageZoneDefinition,
    pub status: StorageZoneStatus,
    pub revision: StorageZoneRevision,
    pub locations: Vec<StorageZoneLocationReadModel>,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub retired_by: Option<UserId>,
    pub retired_at: Option<Timestamp>,
}

pub type ConfigureStorageZoneResult = StorageZoneReadModel;
pub type RetireStorageZoneResult = StorageZoneReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageZoneCursor {
    pub after_travel_sequence: StorageZoneTravelSequence,
    pub after_storage_zone_id: StorageZoneId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageZonePageQuery {
    pub facility_id: Option<FacilityId>,
    pub purpose: Option<StorageZonePurpose>,
    pub status: Option<StorageZoneStatus>,
    pub cursor: Option<StorageZoneCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageZonePage {
    pub items: Vec<StorageZoneReadModel>,
    pub next_cursor: Option<StorageZoneCursor>,
}
