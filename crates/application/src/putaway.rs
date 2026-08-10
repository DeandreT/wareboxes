//! Application read contracts for directed putaway planning and execution monitoring.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{FacilityId, InventoryOwnerId, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PutawayWorkflow {
    Loose,
    LicensePlate,
}

impl PutawayWorkflow {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Loose => "loose",
            Self::LicensePlate => "license_plate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PutawayWorkStatus {
    Pending,
    Claimed,
    Completed,
    Cancelled,
}

impl PutawayWorkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PutawayCandidateSort {
    #[default]
    ReceivedAt,
    Client,
    Facility,
    Source,
    Item,
    Quantity,
    Workflow,
}

impl PutawayCandidateSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReceivedAt => "received_at",
            Self::Client => "client",
            Self::Facility => "facility",
            Self::Source => "source",
            Self::Item => "item",
            Self::Quantity => "quantity",
            Self::Workflow => "workflow",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PutawayWorkSort {
    Priority,
    #[default]
    CreatedAt,
    Client,
    Facility,
    Source,
    Destination,
    Quantity,
    Status,
    Workflow,
}

impl PutawayWorkSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::CreatedAt => "created_at",
            Self::Client => "client",
            Self::Facility => "facility",
            Self::Source => "source",
            Self::Destination => "destination",
            Self::Quantity => "quantity",
            Self::Status => "status",
            Self::Workflow => "workflow",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PutawaySortDirection {
    #[default]
    Asc,
    Desc,
}

impl PutawaySortDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutawayCursor {
    pub offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutawayCandidateQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub workflow: Option<PutawayWorkflow>,
    pub sort: PutawayCandidateSort,
    pub direction: PutawaySortDirection,
    pub cursor: Option<PutawayCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutawayWorkQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub workflow: Option<PutawayWorkflow>,
    pub status: Option<PutawayWorkStatus>,
    pub sort: PutawayWorkSort,
    pub direction: PutawaySortDirection,
    pub cursor: Option<PutawayCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutawayLocationReadModel {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutawayCandidateReadModel {
    pub workflow: PutawayWorkflow,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub source_inventory_balance_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub source_location: PutawayLocationReadModel,
    pub item_count: i64,
    pub balance_count: i64,
    pub item_id: Option<i64>,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub available_quantity: i64,
    pub received_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutawayCandidatePage {
    pub items: Vec<PutawayCandidateReadModel>,
    pub next_cursor: Option<PutawayCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutawayWorkReadModel {
    pub task_id: i64,
    pub workflow: PutawayWorkflow,
    pub status: PutawayWorkStatus,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub source_inventory_balance_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub source_location: PutawayLocationReadModel,
    pub destination_location: PutawayLocationReadModel,
    pub item_count: i64,
    pub balance_count: i64,
    pub item_id: Option<i64>,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: Option<String>,
    pub planned_quantity: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub assigned_user_id: Option<i64>,
    pub lease_expires_at: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub completed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutawayWorkPage {
    pub items: Vec<PutawayWorkReadModel>,
    pub next_cursor: Option<PutawayCursor>,
}
