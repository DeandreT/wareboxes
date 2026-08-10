//! Application read contracts for cycle-count planning and execution monitoring.

use crate::inventory::InventoryBalanceStatus;
use wareboxes_domain::{FacilityId, InventoryOwnerId, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleCountWorkStatus {
    Pending,
    Claimed,
    Completed,
    Cancelled,
}

impl CycleCountWorkStatus {
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
pub enum CycleCountCandidateSort {
    #[default]
    LastCounted,
    Client,
    Facility,
    Location,
    Item,
    Quantity,
    InventoryStatus,
}

impl CycleCountCandidateSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LastCounted => "last_counted",
            Self::Client => "client",
            Self::Facility => "facility",
            Self::Location => "location",
            Self::Item => "item",
            Self::Quantity => "quantity",
            Self::InventoryStatus => "inventory_status",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CycleCountWorkSort {
    Priority,
    #[default]
    CreatedAt,
    Client,
    Facility,
    Location,
    Item,
    Quantity,
    Variance,
    Status,
}

impl CycleCountWorkSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::CreatedAt => "created_at",
            Self::Client => "client",
            Self::Facility => "facility",
            Self::Location => "location",
            Self::Item => "item",
            Self::Quantity => "quantity",
            Self::Variance => "variance",
            Self::Status => "status",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CycleCountSortDirection {
    #[default]
    Asc,
    Desc,
}

impl CycleCountSortDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleCountCursor {
    pub offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleCountCandidateQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub inventory_status: Option<InventoryBalanceStatus>,
    pub sort: CycleCountCandidateSort,
    pub direction: CycleCountSortDirection,
    pub cursor: Option<CycleCountCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleCountWorkQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub status: Option<CycleCountWorkStatus>,
    pub sort: CycleCountWorkSort,
    pub direction: CycleCountSortDirection,
    pub cursor: Option<CycleCountCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountLocationReadModel {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountStockReadModel {
    pub inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub inventory_status: InventoryBalanceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountCandidateReadModel {
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub location: CycleCountLocationReadModel,
    pub stock: CycleCountStockReadModel,
    pub quantity_on_hand: i64,
    pub quantity_reserved: i64,
    pub quantity_held: i64,
    pub last_counted_at: Option<Timestamp>,
    pub last_variance_quantity: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountCandidatePage {
    pub items: Vec<CycleCountCandidateReadModel>,
    pub next_cursor: Option<CycleCountCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountWorkReadModel {
    pub task_id: i64,
    pub status: CycleCountWorkStatus,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub location: CycleCountLocationReadModel,
    pub stock: CycleCountStockReadModel,
    pub current_quantity_on_hand: Option<i64>,
    pub current_quantity_reserved: Option<i64>,
    pub current_quantity_held: Option<i64>,
    pub system_quantity_on_hand: Option<i64>,
    pub system_quantity_reserved: Option<i64>,
    pub system_quantity_held: Option<i64>,
    pub counted_quantity: Option<i64>,
    pub variance_quantity: Option<i64>,
    pub inventory_transaction_id: Option<i64>,
    pub priority: i64,
    pub note: Option<String>,
    pub assigned_user_id: Option<i64>,
    pub lease_expires_at: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub confirmed_by: Option<i64>,
    pub confirmed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountWorkPage {
    pub items: Vec<CycleCountWorkReadModel>,
    pub next_cursor: Option<CycleCountCursor>,
}
