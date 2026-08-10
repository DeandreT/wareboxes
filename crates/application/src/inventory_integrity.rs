use wareboxes_domain::{FacilityId, InventoryOwnerId, Timestamp};

use crate::inventory::InventoryBalanceStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventorySortDirection {
    Ascending,
    Descending,
}

impl InventorySortDirection {
    pub const fn is_ascending(self) -> bool {
        matches!(self, Self::Ascending)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryJournalSort {
    OccurredAt,
    Transaction,
    Type,
    Client,
    NetQuantity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryIntegritySort {
    Severity,
    Facility,
    Client,
    Item,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryIntegrityIssueKind {
    JournalProjection,
    Commitments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryAgingBucket {
    Expired,
    DueWithin7Days,
    DueWithin30Days,
    DueWithin90Days,
    Beyond90Days,
    NoExpiration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryAgingSort {
    Age,
    Expiration,
    Quantity,
    Facility,
    Client,
    Item,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryAgingQuery {
    pub search: Option<String>,
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub item_id: Option<i64>,
    pub bucket: Option<InventoryAgingBucket>,
    pub sort: InventoryAgingSort,
    pub direction: InventorySortDirection,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryAgingReadModel {
    pub inventory_balance_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub primary_sku: Option<String>,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub received_at: Timestamp,
    pub age_days: i64,
    pub expiration: Option<Timestamp>,
    pub days_to_expiration: Option<i64>,
    pub bucket: InventoryAgingBucket,
    pub status: InventoryBalanceStatus,
    pub on_hand_quantity: i64,
    pub reserved_quantity: i64,
    pub held_quantity: i64,
    pub available_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryAgingPage {
    pub items: Vec<InventoryAgingReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryJournalQuery {
    pub search: Option<String>,
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub item_id: Option<i64>,
    pub item_batch_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub transaction_id: Option<i64>,
    pub sort: InventoryJournalSort,
    pub direction: InventorySortDirection,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryJournalTransactionReadModel {
    pub id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub occurred_at: Timestamp,
    pub actor_user_id: Option<i64>,
    pub transaction_type: String,
    pub reason: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub correlation_id: Option<String>,
    pub operation: String,
    pub entry_count: u32,
    pub net_quantity: i64,
    pub entries: Vec<InventoryJournalEntryReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryJournalEntryReadModel {
    pub id: i64,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub primary_sku: Option<String>,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub status: InventoryBalanceStatus,
    pub quantity_delta: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryJournalPage {
    pub items: Vec<InventoryJournalTransactionReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryIntegrityQuery {
    pub kind: Option<InventoryIntegrityIssueKind>,
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub item_id: Option<i64>,
    pub sort: InventoryIntegritySort,
    pub direction: InventorySortDirection,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryIntegrityIssueReadModel {
    pub issue_key: String,
    pub kind: InventoryIntegrityIssueKind,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub primary_sku: Option<String>,
    pub item_description: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub uom: String,
    pub status: InventoryBalanceStatus,
    pub journal_quantity: Option<i64>,
    pub projected_quantity: Option<i64>,
    pub variance_quantity: Option<i64>,
    pub on_hand_quantity: Option<i64>,
    pub reserved_quantity: Option<i64>,
    pub allocated_quantity: Option<i64>,
    pub held_quantity: Option<i64>,
    pub hold_ledger_quantity: Option<i64>,
    pub overcommitted_quantity: Option<i64>,
    pub severity_quantity: i64,
    pub issue_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryIntegrityPage {
    pub items: Vec<InventoryIntegrityIssueReadModel>,
    pub next_offset: Option<u64>,
}
