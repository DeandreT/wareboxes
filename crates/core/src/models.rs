//! Domain models, ported from the Drizzle schema in `app/utils/types/db/*.ts`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use wareboxes_domain::{InventoryOwnerId, TenantId, UserId};

pub type Timestamp = DateTime<Utc>;

macro_rules! impl_status_display {
    ($ty:ty) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    #[default]
    Active,
    Suspended,
}

impl TenantStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
}

impl_status_display!(TenantStatus);

/// A tenant available to the authenticated user. This is an access projection,
/// not the persistence model for either a tenant or membership.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TenantAccess {
    pub tenant_id: TenantId,
    pub user_id: UserId,
    pub slug: String,
    pub name: String,
    pub status: TenantStatus,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Address {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: Option<String>,
    pub company: Option<String>,
    pub line1: String,
    pub line2: Option<String>,
    pub postal_code: Option<String>,
    pub country: String,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub state: Option<String>,
    pub county: Option<String>,
    pub city: Option<String>,
    pub territory: Option<String>,
    pub district: Option<String>,
    pub validated: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct User {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: String,
    pub nick_name: Option<String>,
    pub phone: Option<String>,
    #[serde(default)]
    pub user_roles: Vec<Role>,
    #[serde(default)]
    pub user_permissions: Vec<Permission>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Role {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub parent_roles: Vec<Role>,
    #[serde(default)]
    pub child_roles: Vec<Role>,
    #[serde(default)]
    pub role_permissions: Vec<Permission>,
}

impl Role {
    /// The original app marks per-user "self roles" with this description and
    /// forbids editing/deleting them.
    pub const SELF_ROLE_DESCRIPTION: &'static str = "Self role";

    pub fn is_self_role(&self) -> bool {
        self.description.as_deref() == Some(Self::SELF_ROLE_DESCRIPTION)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Permission {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UserRole {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub user_id: i64,
    pub role_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Facility {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: Option<String>,
    pub address_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryOwner {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub inventory_owner_facilities: Vec<Facility>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum OrderStatus {
    #[serde(rename = "awaiting shipment")]
    AwaitingShipment,
    Shipped,
    Cancelled,
    Held,
    Processing,
    #[default]
    Open,
    Void,
}

impl OrderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderStatus::AwaitingShipment => "awaiting shipment",
            OrderStatus::Shipped => "shipped",
            OrderStatus::Cancelled => "cancelled",
            OrderStatus::Held => "held",
            OrderStatus::Processing => "processing",
            OrderStatus::Open => "open",
            OrderStatus::Void => "void",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "awaiting shipment" => OrderStatus::AwaitingShipment,
            "shipped" => OrderStatus::Shipped,
            "cancelled" => OrderStatus::Cancelled,
            "held" => OrderStatus::Held,
            "processing" => OrderStatus::Processing,
            "open" => OrderStatus::Open,
            "void" => OrderStatus::Void,
            _ => return None,
        })
    }

    pub const ALL: [OrderStatus; 7] = [
        OrderStatus::AwaitingShipment,
        OrderStatus::Shipped,
        OrderStatus::Cancelled,
        OrderStatus::Held,
        OrderStatus::Processing,
        OrderStatus::Open,
        OrderStatus::Void,
    ];

    /// Statuses an order may be in to allow update or soft-delete
    /// (mirrors the `inArray` guard in `app/utils/orders.ts`).
    pub fn is_mutable(&self) -> bool {
        matches!(
            self,
            OrderStatus::Cancelled | OrderStatus::Held | OrderStatus::Open | OrderStatus::Void
        )
    }
}

impl_status_display!(OrderStatus);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderItem {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub qty: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub order_id: i64,
    pub item_batch_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderTrackingNumber {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub order_id: i64,
    pub tracking_number: String,
    pub carrier: Option<String>,
    pub service: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderActivity {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub order_id: i64,
    pub action: String,
}

/// Orders join their shipping address, so the address columns are flattened
/// onto the order (matching `SelectOrder` in `app/utils/types/db/orders.ts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Order {
    pub id: i64,
    pub tenant_id: TenantId,
    pub order_key: String,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub rush: bool,
    pub status: OrderStatus,
    pub address_id: i64,
    pub confirmed: Option<Timestamp>,
    pub closed: Option<Timestamp>,
    pub ship_by: Option<Timestamp>,
    pub wave_id: Option<i64>,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: Option<String>,
    pub line1: Option<String>,
    pub line2: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<String>,
    #[serde(default)]
    pub order_items: Vec<OrderItem>,
    #[serde(default)]
    pub tracking_numbers: Vec<OrderTrackingNumber>,
    #[serde(default)]
    pub reservations: Vec<InventoryReservation>,
    #[serde(default)]
    pub activity: Vec<OrderActivity>,
    #[serde(default)]
    pub ordered_qty: i64,
    #[serde(default)]
    pub reserved_qty: i64,
    #[serde(default)]
    pub out_of_stock: bool,
}

// ---------------------------------------------------------------------------
// Items / catalog (app/utils/types/db/items.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Dim {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub length: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub length_uom: Option<String>,
    pub weight: Option<i64>,
    pub weight_uom: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub packaging_unit: String,
    pub dims_id: Option<i64>,
    pub pallet_hi: Option<i64>,
    pub pallet_ti: Option<i64>,
    pub inner_units: Option<i64>,
    #[serde(default)]
    pub skus: Vec<Sku>,
    #[serde(default)]
    pub barcodes: Vec<Barcode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemPackLink {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub master_item_id: i64,
    pub single_item_id: i64,
    pub inner_qty: i64,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sku {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: String,
    pub item_id: i64,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Barcode {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: String,
    pub r#type: String,
    pub item_id: i64,
    pub notes: Option<String>,
}

// ---------------------------------------------------------------------------
// Locations (app/utils/types/db/locations.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Location {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub parent_location_id: Option<i64>,
    pub barcode: Option<String>,
    pub name: Option<String>,
    pub r#type: String,
    pub active: bool,
    pub pickable: bool,
    pub receivable: bool,
}

// ---------------------------------------------------------------------------
// Inventory (app/utils/types/db/inventory.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemBatch {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub load_id: Option<i64>,
    pub order_id: Option<i64>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryBalance {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub modified: Option<Timestamp>,
    pub deleted: Option<Timestamp>,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub status: InventoryStatus,
    pub qty_on_hand: i64,
    pub qty_reserved: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryReconciliationIssue {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub location_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub status: InventoryStatus,
    pub journal_qty: i64,
    pub projected_qty: i64,
    pub variance: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryTransaction {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub actor_user_id: Option<i64>,
    pub transaction_type: InventoryTransactionType,
    pub reason: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub correlation_id: Option<String>,
    pub operation: String,
    pub idempotency_key: Option<String>,
    pub entries: Vec<InventoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryEntry {
    pub id: i64,
    pub transaction_id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub facility_id: i64,
    pub location_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub license_plate_id: Option<i64>,
    pub status: InventoryStatus,
    pub quantity_delta: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryReservation {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub modified: Option<Timestamp>,
    pub deleted: Option<Timestamp>,
    pub order_id: i64,
    pub order_item_id: Option<i64>,
    pub inventory_balance_id: i64,
    pub facility_id: i64,
    pub item_batch_id: i64,
    pub location_id: i64,
    pub qty: i64,
    pub status: ReservationStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum InventoryStatus {
    #[default]
    Available,
    Hold,
    Damaged,
    Quarantine,
}

impl InventoryStatus {
    pub const ALL: [InventoryStatus; 4] = [
        InventoryStatus::Available,
        InventoryStatus::Hold,
        InventoryStatus::Damaged,
        InventoryStatus::Quarantine,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            InventoryStatus::Available => "available",
            InventoryStatus::Hold => "hold",
            InventoryStatus::Damaged => "damaged",
            InventoryStatus::Quarantine => "quarantine",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "available" => InventoryStatus::Available,
            "hold" => InventoryStatus::Hold,
            "damaged" => InventoryStatus::Damaged,
            "quarantine" => InventoryStatus::Quarantine,
            _ => return None,
        })
    }
}

impl_status_display!(InventoryStatus);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum InventoryTransactionType {
    #[default]
    Receive,
    Move,
    Adjust,
    Ship,
}

impl InventoryTransactionType {
    pub const ALL: [InventoryTransactionType; 4] = [
        InventoryTransactionType::Receive,
        InventoryTransactionType::Move,
        InventoryTransactionType::Adjust,
        InventoryTransactionType::Ship,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            InventoryTransactionType::Receive => "receive",
            InventoryTransactionType::Move => "move",
            InventoryTransactionType::Adjust => "adjust",
            InventoryTransactionType::Ship => "ship",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "receive" => InventoryTransactionType::Receive,
            "move" => InventoryTransactionType::Move,
            "adjust" => InventoryTransactionType::Adjust,
            "ship" => InventoryTransactionType::Ship,
            _ => return None,
        })
    }
}

impl_status_display!(InventoryTransactionType);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReservationStatus {
    #[default]
    Reserved,
    Cancelled,
    Fulfilled,
}

impl ReservationStatus {
    pub const ALL: [ReservationStatus; 3] = [
        ReservationStatus::Reserved,
        ReservationStatus::Cancelled,
        ReservationStatus::Fulfilled,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ReservationStatus::Reserved => "reserved",
            ReservationStatus::Cancelled => "cancelled",
            ReservationStatus::Fulfilled => "fulfilled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "reserved" => ReservationStatus::Reserved,
            "cancelled" => ReservationStatus::Cancelled,
            "fulfilled" => ReservationStatus::Fulfilled,
            _ => return None,
        })
    }
}

impl_status_display!(ReservationStatus);

// ---------------------------------------------------------------------------
// License plates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LicensePlate {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub barcode: Option<String>,
    pub location_id: Option<i64>,
    pub dims_id: Option<i64>,
    #[serde(default)]
    pub contents: Vec<LicensePlateContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LicensePlateContent {
    pub inventory_balance_id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub location_id: i64,
    pub item_batch_id: i64,
    pub status: InventoryStatus,
    pub qty_on_hand: i64,
    pub qty_reserved: i64,
}

// ---------------------------------------------------------------------------
// Employees (app/utils/types/db/employees.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Employee {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub user_id: Option<i64>,
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub title: String,
    pub r#type: String,
    pub hired: Timestamp,
    pub terminated: Option<Timestamp>,
}

// ---------------------------------------------------------------------------
// Loads (app/utils/types/db/loads.ts) — extended with arrival / rejected /
// receive_completed per requirements.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadNote {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub load_id: i64,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadFile {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub load_id: i64,
    /// Filename as uploaded (e.g. "BOL-1234.pdf").
    pub original_name: String,
    /// Stored (unique) filename on the server.
    pub name: String,
    pub path: String,
    pub content_type: Option<String>,
    pub category: LoadFileCategory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadLine {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub load_id: i64,
    pub item_id: i64,
    pub sku_id: Option<i64>,
    pub expected_qty: i64,
    pub received_qty: i64,
    pub rejected_qty: i64,
    pub missing_qty: i64,
    pub missing_confirmed_by: Option<i64>,
    pub missing_confirmed_at: Option<Timestamp>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub status: LoadLineStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiveLoadLineResult {
    pub load_line_id: i64,
    pub inventory_transaction_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadActivity {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub load_id: i64,
    pub user_id: Option<i64>,
    pub action: String,
    pub message: Option<String>,
    pub metadata_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Load {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: Option<String>,
    pub status: LoadStatus,
    pub r#type: LoadType,
    pub reference_number: Option<String>,
    pub invoice_number: Option<String>,
    pub carrier: Option<String>,
    pub trailer_number: Option<String>,
    pub seal_number: Option<String>,
    pub dock_door_location_id: Option<i64>,
    pub expected_time: Option<Timestamp>,
    pub appointment_time: Option<Timestamp>,
    pub actual_time: Option<Timestamp>,
    pub arrival: Option<Timestamp>,
    pub departure: Option<Timestamp>,
    pub rejected: Option<Timestamp>,
    pub receive_completed: bool,
    pub closed: Option<Timestamp>,
    pub checked_in_by: Option<i64>,
    pub closed_by: Option<i64>,
    #[serde(default)]
    pub notes: Vec<LoadNote>,
    #[serde(default)]
    pub files: Vec<LoadFile>,
    #[serde(default)]
    pub lines: Vec<LoadLine>,
    #[serde(default)]
    pub orders: Vec<Order>,
    #[serde(default)]
    pub activity: Vec<LoadActivity>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoadStatus {
    #[default]
    Planned,
    Scheduled,
    Arrived,
    Receiving,
    Received,
    Rejected,
    Closed,
    Cancelled,
}

impl LoadStatus {
    pub const ALL: [LoadStatus; 8] = [
        LoadStatus::Planned,
        LoadStatus::Scheduled,
        LoadStatus::Arrived,
        LoadStatus::Receiving,
        LoadStatus::Received,
        LoadStatus::Rejected,
        LoadStatus::Closed,
        LoadStatus::Cancelled,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            LoadStatus::Planned => "planned",
            LoadStatus::Scheduled => "scheduled",
            LoadStatus::Arrived => "arrived",
            LoadStatus::Receiving => "receiving",
            LoadStatus::Received => "received",
            LoadStatus::Rejected => "rejected",
            LoadStatus::Closed => "closed",
            LoadStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(
            match s.trim().to_ascii_lowercase().replace('_', " ").as_str() {
                "planned" => LoadStatus::Planned,
                "scheduled" => LoadStatus::Scheduled,
                "arrived" => LoadStatus::Arrived,
                "receiving" => LoadStatus::Receiving,
                "received" => LoadStatus::Received,
                "rejected" => LoadStatus::Rejected,
                "closed" => LoadStatus::Closed,
                "cancelled" => LoadStatus::Cancelled,
                _ => return None,
            },
        )
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, LoadStatus::Closed | LoadStatus::Cancelled)
    }

    pub fn can_transition_to(&self, to: Self) -> bool {
        if *self == to {
            return true;
        }
        matches!(
            (*self, to),
            (LoadStatus::Planned, LoadStatus::Scheduled)
                | (LoadStatus::Planned, LoadStatus::Arrived)
                | (LoadStatus::Planned, LoadStatus::Cancelled)
                | (LoadStatus::Scheduled, LoadStatus::Arrived)
                | (LoadStatus::Scheduled, LoadStatus::Cancelled)
                | (LoadStatus::Arrived, LoadStatus::Receiving)
                | (LoadStatus::Arrived, LoadStatus::Rejected)
                | (LoadStatus::Arrived, LoadStatus::Cancelled)
                | (LoadStatus::Receiving, LoadStatus::Received)
                | (LoadStatus::Receiving, LoadStatus::Rejected)
                | (LoadStatus::Received, LoadStatus::Closed)
                | (LoadStatus::Rejected, LoadStatus::Closed)
        )
    }
}

impl_status_display!(LoadStatus);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoadType {
    #[default]
    Inbound,
    Outbound,
}

impl LoadType {
    pub const ALL: [LoadType; 2] = [LoadType::Inbound, LoadType::Outbound];

    pub fn as_str(&self) -> &'static str {
        match self {
            LoadType::Inbound => "inbound",
            LoadType::Outbound => "outbound",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "inbound" => LoadType::Inbound,
            "outbound" => LoadType::Outbound,
            _ => return None,
        })
    }
}

impl_status_display!(LoadType);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoadLineStatus {
    #[default]
    Pending,
    Partial,
    Received,
    Rejected,
    Missing,
}

impl LoadLineStatus {
    pub const ALL: [LoadLineStatus; 5] = [
        LoadLineStatus::Pending,
        LoadLineStatus::Partial,
        LoadLineStatus::Received,
        LoadLineStatus::Rejected,
        LoadLineStatus::Missing,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            LoadLineStatus::Pending => "pending",
            LoadLineStatus::Partial => "partial",
            LoadLineStatus::Received => "received",
            LoadLineStatus::Rejected => "rejected",
            LoadLineStatus::Missing => "missing",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "pending" => LoadLineStatus::Pending,
            "partial" => LoadLineStatus::Partial,
            "received" => LoadLineStatus::Received,
            "rejected" => LoadLineStatus::Rejected,
            "missing" => LoadLineStatus::Missing,
            _ => return None,
        })
    }
}

impl_status_display!(LoadLineStatus);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoadFileCategory {
    #[default]
    General,
    Invoice,
}

impl LoadFileCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            LoadFileCategory::General => "general",
            LoadFileCategory::Invoice => "invoice",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "general" => LoadFileCategory::General,
            "invoice" => LoadFileCategory::Invoice,
            _ => return None,
        })
    }
}

impl_status_display!(LoadFileCategory);

// ---------------------------------------------------------------------------
// Audits (app/utils/types/db/audits.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AuditWave {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AuditLocationCount {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub started: Option<Timestamp>,
    pub ended: Option<Timestamp>,
    pub audit_id: i64,
    pub location_id: i64,
    pub item_id: i64,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub on_hand: i64,
    pub count: i64,
    pub approval_status: String,
}

// ---------------------------------------------------------------------------
// Work tasks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkTaskType {
    CycleCountItemLocation,
    CycleCountLocation,
    BreakMasterPack,
    UnpackCancelledOrder,
}

impl WorkTaskType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkTaskType::CycleCountItemLocation => "cycle_count_item_location",
            WorkTaskType::CycleCountLocation => "cycle_count_location",
            WorkTaskType::BreakMasterPack => "break_master_pack",
            WorkTaskType::UnpackCancelledOrder => "unpack_cancelled_order",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "cycle_count_item_location" => WorkTaskType::CycleCountItemLocation,
            "cycle_count_location" => WorkTaskType::CycleCountLocation,
            "break_master_pack" => WorkTaskType::BreakMasterPack,
            "unpack_cancelled_order" => WorkTaskType::UnpackCancelledOrder,
            _ => return None,
        })
    }
}

impl_status_display!(WorkTaskType);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkTaskStatus {
    Open,
    Assigned,
    InProgress,
    Completed,
    Cancelled,
}

impl WorkTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkTaskStatus::Open => "open",
            WorkTaskStatus::Assigned => "assigned",
            WorkTaskStatus::InProgress => "in_progress",
            WorkTaskStatus::Completed => "completed",
            WorkTaskStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "open" => WorkTaskStatus::Open,
            "assigned" => WorkTaskStatus::Assigned,
            "in_progress" => WorkTaskStatus::InProgress,
            "completed" => WorkTaskStatus::Completed,
            "cancelled" => WorkTaskStatus::Cancelled,
            _ => return None,
        })
    }
}

impl_status_display!(WorkTaskStatus);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkTaskProgressAction {
    #[default]
    Progress,
    Unpacked,
    Missing,
    Damaged,
}

impl WorkTaskProgressAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkTaskProgressAction::Progress => "progress",
            WorkTaskProgressAction::Unpacked => "unpacked",
            WorkTaskProgressAction::Missing => "missing",
            WorkTaskProgressAction::Damaged => "damaged",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "progress" => WorkTaskProgressAction::Progress,
            "unpacked" => WorkTaskProgressAction::Unpacked,
            "missing" => WorkTaskProgressAction::Missing,
            "damaged" => WorkTaskProgressAction::Damaged,
            _ => return None,
        })
    }
}

impl_status_display!(WorkTaskProgressAction);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkTask {
    pub id: i64,
    pub created: Timestamp,
    pub modified: Option<Timestamp>,
    pub deleted: Option<Timestamp>,
    pub task_type: WorkTaskType,
    pub status: WorkTaskStatus,
    pub required_permission: String,
    pub priority: i64,
    pub title: String,
    pub instructions: Option<String>,
    pub assigned_user_id: Option<i64>,
    pub created_by: Option<i64>,
    pub completed_by: Option<i64>,
    pub scheduled_for: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    pub lease_expires_at: Option<Timestamp>,
    pub task_timeout_seconds: i64,
    pub last_released_at: Option<Timestamp>,
    pub release_count: i64,
    pub completed_at: Option<Timestamp>,
    pub metadata_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountItemLocationTask {
    pub task_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
    pub item_id: i64,
    pub inventory_balance_id: Option<i64>,
    pub order_id: Option<i64>,
    pub order_item_id: Option<i64>,
    pub source: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountLocationTask {
    pub task_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BreakMasterPackTask {
    pub task_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
    pub master_item_id: i64,
    pub single_item_id: i64,
    pub master_qty: i64,
    pub master_qty_completed: i64,
    pub inner_qty_snapshot: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnpackCancelledOrderTask {
    pub task_id: i64,
    pub order_id: i64,
    #[serde(default)]
    pub lines: Vec<UnpackCancelledOrderTaskLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnpackCancelledOrderTaskLine {
    pub id: i64,
    pub task_id: i64,
    pub order_item_id: Option<i64>,
    pub item_id: i64,
    pub item_batch_id: Option<i64>,
    pub inventory_balance_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub source_location_id: Option<i64>,
    pub destination_location_id: Option<i64>,
    pub expected_qty: i64,
    pub unpacked_qty: i64,
    pub missing_qty: i64,
    pub damaged_qty: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkTaskProgress {
    pub id: i64,
    pub created: Timestamp,
    pub task_id: i64,
    pub task_line_id: Option<i64>,
    pub user_id: Option<i64>,
    pub action: String,
    pub qty_delta: Option<i64>,
    pub from_location_id: Option<i64>,
    pub to_location_id: Option<i64>,
    pub note: Option<String>,
    pub metadata_json: Option<String>,
}
