//! Domain models, ported from the Drizzle schema in `app/utils/types/db/*.ts`.

use serde::{Deserialize, Serialize};
use std::fmt;
use wareboxes_domain::{InventoryOwnerId, OwnerScope, SiteScope, TenantId, TenantStatus, UserId};

pub use wareboxes_domain::{OrderHoldReason, OrderStatus, Timestamp};

macro_rules! impl_status_display {
    ($ty:ty) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

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
    pub site_scope: SiteScope,
    pub owner_scope: OwnerScope,
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
    pub self_user_id: Option<i64>,
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
        self.self_user_id.is_some()
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
    pub revision: i64,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderItem {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub line_key: String,
    pub line_number: i64,
    pub qty: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub order_id: i64,
    pub uom: String,
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
    pub actor_user_id: Option<i64>,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderHold {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub order_id: i64,
    pub created: Timestamp,
    pub created_by_user_id: i64,
    pub reason: OrderHoldReason,
    pub note: Option<String>,
    pub released_at: Option<Timestamp>,
    pub released_by_user_id: Option<i64>,
    pub release_note: Option<String>,
}

impl OrderHold {
    pub const fn is_active(&self) -> bool {
        self.released_at.is_none()
    }
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
    pub revision: i64,
    pub confirmed: Option<Timestamp>,
    pub closed: Option<Timestamp>,
    pub ship_by: Option<Timestamp>,
    pub wave_id: Option<i64>,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: Option<String>,
    pub recipient_name: Option<String>,
    pub destination_company: Option<String>,
    pub destination_phone: Option<String>,
    pub destination_email: Option<String>,
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
    pub holds: Vec<OrderHold>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dim {
    pub id: i64,
    pub tenant_id: TenantId,
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
pub struct InventoryOwnerItem {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub inventory_owner_id: i64,
    pub item_id: i64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_zone_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_zone_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_zone_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_zone_purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_zone_travel_sequence: Option<i64>,
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
    pub qty_held: i64,
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
pub struct InventoryHoldReconciliationIssue {
    pub inventory_balance_id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub location_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub inventory_status: InventoryStatus,
    pub qty_on_hand: i64,
    pub qty_reserved: i64,
    pub allocated_qty: i64,
    pub qty_held: i64,
    pub held_qty: i64,
    pub overcommitted_qty: i64,
    pub issue_codes: Vec<String>,
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
    pub order_item_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub qty: i64,
    pub status: ReservationStatus,
    pub allocated_qty: i64,
    pub allocations: Vec<InventoryAllocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryAllocation {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub modified: Option<Timestamp>,
    pub deleted: Option<Timestamp>,
    pub reservation_id: i64,
    pub inventory_balance_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub inventory_status: InventoryStatus,
    pub qty: i64,
    pub status: AllocationStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryHold {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub created: Timestamp,
    pub modified: Option<Timestamp>,
    pub deleted: Option<Timestamp>,
    pub created_by: i64,
    pub released_by: Option<i64>,
    pub released_at: Option<Timestamp>,
    pub inventory_balance_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub inventory_status: InventoryStatus,
    pub qty: i64,
    pub reason: InventoryHoldReason,
    pub note: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub status: InventoryHoldStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryHoldReason {
    QualityInspection,
    DamageSuspected,
    InventoryDiscrepancy,
    Regulatory,
    CustomerRequest,
    Other,
}

impl InventoryHoldReason {
    pub const ALL: [Self; 6] = [
        Self::QualityInspection,
        Self::DamageSuspected,
        Self::InventoryDiscrepancy,
        Self::Regulatory,
        Self::CustomerRequest,
        Self::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::QualityInspection => "quality_inspection",
            Self::DamageSuspected => "damage_suspected",
            Self::InventoryDiscrepancy => "inventory_discrepancy",
            Self::Regulatory => "regulatory",
            Self::CustomerRequest => "customer_request",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "quality_inspection" => Self::QualityInspection,
            "damage_suspected" => Self::DamageSuspected,
            "inventory_discrepancy" => Self::InventoryDiscrepancy,
            "regulatory" => Self::Regulatory,
            "customer_request" => Self::CustomerRequest,
            "other" => Self::Other,
            _ => return None,
        })
    }
}

impl_status_display!(InventoryHoldReason);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryHoldStatus {
    Active,
    Released,
}

impl InventoryHoldStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "active" => Self::Active,
            "released" => Self::Released,
            _ => return None,
        })
    }
}

impl_status_display!(InventoryHoldStatus);

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryStatusChangeReason {
    QualityInspection,
    DamageSuspected,
    DamageConfirmed,
    InspectionPassed,
    InventoryDiscrepancy,
    DiscrepancyResolved,
    RegulatoryRestriction,
    RegulatoryRelease,
    CustomerRequest,
    CustomerRelease,
    Other,
}

impl InventoryStatusChangeReason {
    pub const ALL: [Self; 11] = [
        Self::QualityInspection,
        Self::DamageSuspected,
        Self::DamageConfirmed,
        Self::InspectionPassed,
        Self::InventoryDiscrepancy,
        Self::DiscrepancyResolved,
        Self::RegulatoryRestriction,
        Self::RegulatoryRelease,
        Self::CustomerRequest,
        Self::CustomerRelease,
        Self::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::QualityInspection => "quality_inspection",
            Self::DamageSuspected => "damage_suspected",
            Self::DamageConfirmed => "damage_confirmed",
            Self::InspectionPassed => "inspection_passed",
            Self::InventoryDiscrepancy => "inventory_discrepancy",
            Self::DiscrepancyResolved => "discrepancy_resolved",
            Self::RegulatoryRestriction => "regulatory_restriction",
            Self::RegulatoryRelease => "regulatory_release",
            Self::CustomerRequest => "customer_request",
            Self::CustomerRelease => "customer_release",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "quality_inspection" => Self::QualityInspection,
            "damage_suspected" => Self::DamageSuspected,
            "damage_confirmed" => Self::DamageConfirmed,
            "inspection_passed" => Self::InspectionPassed,
            "inventory_discrepancy" => Self::InventoryDiscrepancy,
            "discrepancy_resolved" => Self::DiscrepancyResolved,
            "regulatory_restriction" => Self::RegulatoryRestriction,
            "regulatory_release" => Self::RegulatoryRelease,
            "customer_request" => Self::CustomerRequest,
            "customer_release" => Self::CustomerRelease,
            "other" => Self::Other,
            _ => return None,
        })
    }

    pub fn allows_target_status(self, status: InventoryStatus) -> bool {
        match self {
            Self::QualityInspection
            | Self::DamageSuspected
            | Self::InventoryDiscrepancy
            | Self::RegulatoryRestriction
            | Self::CustomerRequest => {
                matches!(status, InventoryStatus::Hold | InventoryStatus::Quarantine)
            }
            Self::DamageConfirmed => status == InventoryStatus::Damaged,
            Self::InspectionPassed
            | Self::DiscrepancyResolved
            | Self::RegulatoryRelease
            | Self::CustomerRelease => status == InventoryStatus::Available,
            Self::Other => true,
        }
    }
}

impl_status_display!(InventoryStatusChangeReason);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventoryTransactionType {
    #[default]
    Receive,
    Move,
    Adjust,
    Ship,
    StatusChange,
}

impl InventoryTransactionType {
    pub const ALL: [InventoryTransactionType; 5] = [
        InventoryTransactionType::Receive,
        InventoryTransactionType::Move,
        InventoryTransactionType::Adjust,
        InventoryTransactionType::Ship,
        InventoryTransactionType::StatusChange,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            InventoryTransactionType::Receive => "receive",
            InventoryTransactionType::Move => "move",
            InventoryTransactionType::Adjust => "adjust",
            InventoryTransactionType::Ship => "ship",
            InventoryTransactionType::StatusChange => "status_change",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "receive" => InventoryTransactionType::Receive,
            "move" => InventoryTransactionType::Move,
            "adjust" => InventoryTransactionType::Adjust,
            "ship" => InventoryTransactionType::Ship,
            "status_change" => InventoryTransactionType::StatusChange,
            _ => return None,
        })
    }
}

impl_status_display!(InventoryTransactionType);

#[cfg(test)]
mod inventory_status_change_model_tests {
    use super::{InventoryStatusChangeReason, InventoryTransactionType};

    #[test]
    fn status_change_transaction_type_uses_snake_case_wire_value() {
        let value = InventoryTransactionType::StatusChange;

        assert_eq!(value.as_str(), "status_change");
        assert_eq!(value.to_string(), "status_change");
        assert_eq!(
            InventoryTransactionType::parse(" STATUS_CHANGE "),
            Some(value)
        );
        assert_eq!(serde_json::to_string(&value).unwrap(), r#""status_change""#);
        assert_eq!(
            serde_json::from_str::<InventoryTransactionType>(r#""status_change""#).unwrap(),
            value
        );
    }

    #[test]
    fn status_change_reasons_round_trip_through_wire_values() {
        for reason in InventoryStatusChangeReason::ALL {
            assert_eq!(
                InventoryStatusChangeReason::parse(reason.as_str()),
                Some(reason)
            );
            assert_eq!(reason.to_string(), reason.as_str());
            assert_eq!(
                serde_json::from_str::<InventoryStatusChangeReason>(
                    &serde_json::to_string(&reason).unwrap()
                )
                .unwrap(),
                reason
            );
        }
    }

    #[test]
    fn status_change_reasons_limit_target_dispositions() {
        assert!(InventoryStatusChangeReason::QualityInspection
            .allows_target_status(super::InventoryStatus::Quarantine));
        assert!(!InventoryStatusChangeReason::QualityInspection
            .allows_target_status(super::InventoryStatus::Damaged));
        assert!(InventoryStatusChangeReason::DamageConfirmed
            .allows_target_status(super::InventoryStatus::Damaged));
        assert!(InventoryStatusChangeReason::InspectionPassed
            .allows_target_status(super::InventoryStatus::Available));
        assert!(
            InventoryStatusChangeReason::Other.allows_target_status(super::InventoryStatus::Hold)
        );
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReservationStatus {
    #[default]
    Active,
    Cancelled,
    Fulfilled,
}

impl ReservationStatus {
    pub const ALL: [ReservationStatus; 3] = [
        ReservationStatus::Active,
        ReservationStatus::Cancelled,
        ReservationStatus::Fulfilled,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ReservationStatus::Active => "active",
            ReservationStatus::Cancelled => "cancelled",
            ReservationStatus::Fulfilled => "fulfilled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "active" => ReservationStatus::Active,
            "cancelled" => ReservationStatus::Cancelled,
            "fulfilled" => ReservationStatus::Fulfilled,
            _ => return None,
        })
    }
}

impl_status_display!(ReservationStatus);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AllocationStatus {
    #[default]
    Allocated,
    Released,
    Fulfilled,
}

impl AllocationStatus {
    pub const ALL: [AllocationStatus; 3] = [
        AllocationStatus::Allocated,
        AllocationStatus::Released,
        AllocationStatus::Fulfilled,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            AllocationStatus::Allocated => "allocated",
            AllocationStatus::Released => "released",
            AllocationStatus::Fulfilled => "fulfilled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "allocated" => AllocationStatus::Allocated,
            "released" => AllocationStatus::Released,
            "fulfilled" => AllocationStatus::Fulfilled,
            _ => return None,
        })
    }
}

impl_status_display!(AllocationStatus);

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
    pub facility_id: i64,
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
    pub qty_held: i64,
}

// ---------------------------------------------------------------------------
// Employees (app/utils/types/db/employees.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Employee {
    pub id: i64,
    pub tenant_id: TenantId,
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
    pub facility_ids: Vec<i64>,
    pub can_manage: bool,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboundReceiptExceptionReason {
    Damaged,
    QualityRejected,
    ShortShipment,
    CountDiscrepancy,
    WrongItem,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboundReceiptQuarantineReason {
    Damaged,
    QualityInspection,
    CountDiscrepancy,
    WrongItem,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnexpectedReceiptReason {
    Excess,
    UnexpectedItem,
    BlindReceipt,
    MisShipped,
    Other,
}

impl UnexpectedReceiptReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Excess => "excess",
            Self::UnexpectedItem => "unexpected_item",
            Self::BlindReceipt => "blind_receipt",
            Self::MisShipped => "mis_shipped",
            Self::Other => "other",
        }
    }

    pub fn hold_reason(self) -> InventoryHoldReason {
        match self {
            Self::MisShipped => InventoryHoldReason::CustomerRequest,
            Self::Excess | Self::UnexpectedItem | Self::BlindReceipt | Self::Other => {
                InventoryHoldReason::InventoryDiscrepancy
            }
        }
    }
}

impl InboundReceiptQuarantineReason {
    pub fn exception_reason(self) -> InboundReceiptExceptionReason {
        match self {
            Self::Damaged => InboundReceiptExceptionReason::Damaged,
            Self::QualityInspection => InboundReceiptExceptionReason::QualityRejected,
            Self::CountDiscrepancy => InboundReceiptExceptionReason::CountDiscrepancy,
            Self::WrongItem => InboundReceiptExceptionReason::WrongItem,
            Self::Other => InboundReceiptExceptionReason::Other,
        }
    }

    pub fn hold_reason(self) -> InventoryHoldReason {
        match self {
            Self::Damaged => InventoryHoldReason::DamageSuspected,
            Self::QualityInspection => InventoryHoldReason::QualityInspection,
            Self::CountDiscrepancy | Self::WrongItem => InventoryHoldReason::InventoryDiscrepancy,
            Self::Other => InventoryHoldReason::Other,
        }
    }
}

impl InboundReceiptExceptionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Damaged => "damaged",
            Self::QualityRejected => "quality_rejected",
            Self::ShortShipment => "short_shipment",
            Self::CountDiscrepancy => "count_discrepancy",
            Self::WrongItem => "wrong_item",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiveExpectedInventoryResult {
    pub load_id: i64,
    pub load_line_id: i64,
    pub inventory_transaction_id: Option<i64>,
    pub inventory_balance_id: Option<i64>,
    pub item_batch_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub inventory_hold_id: Option<i64>,
    pub inventory_status: Option<InventoryStatus>,
    pub load_status: LoadStatus,
    pub line_status: LoadLineStatus,
    pub cumulative_received_qty: i64,
    pub cumulative_rejected_qty: i64,
    pub cumulative_missing_qty: i64,
    pub remaining_quantity: i64,
    pub receive_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfirmUnexpectedReceiptResult {
    pub unexpected_receipt_id: i64,
    pub load_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub quantity: i64,
    pub receiving_location_id: i64,
    pub observed_item_barcode: String,
    pub observed_receiving_location_barcode: String,
    pub inventory_transaction_id: i64,
    pub inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub inventory_hold_id: i64,
    pub inventory_status: InventoryStatus,
    pub reason: UnexpectedReceiptReason,
    pub note: Option<String>,
    pub load_status: LoadStatus,
    pub confirmed_by_user_id: i64,
    pub confirmed_at: Timestamp,
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
    pub execution_barcode: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditWave {
    pub id: i64,
    pub tenant_id: TenantId,
    pub facility_id: i64,
    pub inventory_owner_id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub created_by: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditLocationCount {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub started: Option<Timestamp>,
    pub ended: Option<Timestamp>,
    pub audit_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub on_hand: i64,
    pub count: i64,
    pub revision: i64,
    pub approval_status: AuditApprovalStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuditApprovalStatus {
    #[default]
    Pending,
    Approved,
    Rejected,
}

impl AuditApprovalStatus {
    pub const ALL: [Self; 3] = [Self::Pending, Self::Approved, Self::Rejected];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "pending" => Self::Pending,
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }
}

impl_status_display!(AuditApprovalStatus);

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
    Putaway,
    LicensePlatePutaway,
    InventoryRelocation,
    Replenishment,
}

impl WorkTaskType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkTaskType::CycleCountItemLocation => "cycle_count_item_location",
            WorkTaskType::CycleCountLocation => "cycle_count_location",
            WorkTaskType::BreakMasterPack => "break_master_pack",
            WorkTaskType::UnpackCancelledOrder => "unpack_cancelled_order",
            WorkTaskType::Putaway => "putaway",
            WorkTaskType::LicensePlatePutaway => "license_plate_putaway",
            WorkTaskType::InventoryRelocation => "inventory_relocation",
            WorkTaskType::Replenishment => "replenishment",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "cycle_count_item_location" => WorkTaskType::CycleCountItemLocation,
            "cycle_count_location" => WorkTaskType::CycleCountLocation,
            "break_master_pack" => WorkTaskType::BreakMasterPack,
            "unpack_cancelled_order" => WorkTaskType::UnpackCancelledOrder,
            "putaway" => WorkTaskType::Putaway,
            "license_plate_putaway" => WorkTaskType::LicensePlatePutaway,
            "inventory_relocation" => WorkTaskType::InventoryRelocation,
            "replenishment" => WorkTaskType::Replenishment,
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
    ReplenishmentConfirmed,
    ReplenishmentHeartbeat,
    ReplenishmentReleased,
    ReplenishmentCancelled,
}

impl WorkTaskProgressAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkTaskProgressAction::Progress => "progress",
            WorkTaskProgressAction::Unpacked => "unpacked",
            WorkTaskProgressAction::Missing => "missing",
            WorkTaskProgressAction::Damaged => "damaged",
            WorkTaskProgressAction::ReplenishmentConfirmed => "replenishment_confirmed",
            WorkTaskProgressAction::ReplenishmentHeartbeat => "replenishment_heartbeat",
            WorkTaskProgressAction::ReplenishmentReleased => "replenishment_released",
            WorkTaskProgressAction::ReplenishmentCancelled => "replenishment_cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "progress" => WorkTaskProgressAction::Progress,
            "unpacked" => WorkTaskProgressAction::Unpacked,
            "missing" => WorkTaskProgressAction::Missing,
            "damaged" => WorkTaskProgressAction::Damaged,
            "replenishment_confirmed" => WorkTaskProgressAction::ReplenishmentConfirmed,
            "replenishment_heartbeat" => WorkTaskProgressAction::ReplenishmentHeartbeat,
            "replenishment_released" => WorkTaskProgressAction::ReplenishmentReleased,
            "replenishment_cancelled" => WorkTaskProgressAction::ReplenishmentCancelled,
            _ => return None,
        })
    }
}

impl_status_display!(WorkTaskProgressAction);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkTask {
    pub id: i64,
    pub tenant_id: TenantId,
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
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
pub struct PutawayClaimSourceLocation {
    pub location_id: i64,
    pub barcode: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutawayClaimDestinationLocation {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PutawayClaimWork {
    Loose {
        source_inventory_balance_id: i64,
        item_batch_id: i64,
        item_id: i64,
        item_description: Option<String>,
        uom: String,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<Timestamp>,
        inventory_status: InventoryStatus,
        quantity: i64,
    },
    LicensePlate {
        license_plate_id: i64,
        license_plate_barcode: String,
        planned_balance_count: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutawayClaim {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<Timestamp>,
    pub lease_expires_at: Timestamp,
    pub source_location: PutawayClaimSourceLocation,
    pub destination_location: PutawayClaimDestinationLocation,
    pub work: PutawayClaimWork,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutawayClaimHeartbeat {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub heartbeat_by: i64,
    pub heartbeat_at: Timestamp,
    pub previous_lease_expires_at: Timestamp,
    pub lease_expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PutawayClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    DestinationBlocked,
    SafetyIssue,
    Other,
}

impl PutawayClaimReleaseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            PutawayClaimReleaseReason::WorkInterrupted => "work_interrupted",
            PutawayClaimReleaseReason::EquipmentUnavailable => "equipment_unavailable",
            PutawayClaimReleaseReason::DestinationBlocked => "destination_blocked",
            PutawayClaimReleaseReason::SafetyIssue => "safety_issue",
            PutawayClaimReleaseReason::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutawayClaimRelease {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub released_by: i64,
    pub released_at: Timestamp,
    pub release_count: i64,
    pub reason: PutawayClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutawayConfirmation {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub inventory_status: InventoryStatus,
    pub quantity: i64,
    pub inventory_transaction_id: i64,
    pub confirmed_by: i64,
    pub confirmed_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LicensePlatePutawayConfirmation {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub license_plate_id: i64,
    pub license_plate_barcode: String,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub inventory_transaction_id: i64,
    pub moved_balance_count: i64,
    pub confirmed_by: i64,
    pub confirmed_at: Timestamp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRelocationWorkflow {
    LooseBalance,
    LicensePlate,
}

impl InventoryRelocationWorkflow {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LooseBalance => "loose_balance",
            Self::LicensePlate => "license_plate",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "loose_balance" => Some(Self::LooseBalance),
            "license_plate" => Some(Self::LicensePlate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryRelocationLocation {
    pub location_id: i64,
    pub barcode: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InventoryRelocationClaimWork {
    LooseBalance {
        source_inventory_balance_id: i64,
        item_batch_id: i64,
        item_id: i64,
        item_description: Option<String>,
        uom: String,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<Timestamp>,
        inventory_status: InventoryStatus,
        quantity: i64,
    },
    LicensePlate {
        license_plate_id: i64,
        license_plate_barcode: String,
        planned_balance_count: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryRelocationClaim {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<Timestamp>,
    pub lease_expires_at: Timestamp,
    pub source_location: InventoryRelocationLocation,
    pub destination_location: InventoryRelocationLocation,
    pub work: InventoryRelocationClaimWork,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryRelocationClaimHeartbeat {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub heartbeat_by: i64,
    pub heartbeat_at: Timestamp,
    pub previous_lease_expires_at: Timestamp,
    pub lease_expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRelocationClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    DestinationBlocked,
    SafetyIssue,
    Other,
}

impl InventoryRelocationClaimReleaseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkInterrupted => "work_interrupted",
            Self::EquipmentUnavailable => "equipment_unavailable",
            Self::DestinationBlocked => "destination_blocked",
            Self::SafetyIssue => "safety_issue",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryRelocationClaimRelease {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub released_by: i64,
    pub released_at: Timestamp,
    pub release_count: i64,
    pub reason: InventoryRelocationClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InventoryRelocationConfirmationResult {
    LooseBalance {
        source_inventory_balance_id: i64,
        destination_inventory_balance_id: i64,
        item_batch_id: i64,
        item_id: i64,
        inventory_status: InventoryStatus,
        uom: String,
        quantity: i64,
    },
    LicensePlate {
        license_plate_id: i64,
        license_plate_barcode: String,
        moved_balance_count: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryRelocationConfirmation {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub inventory_transaction_id: i64,
    pub confirmed_by: i64,
    pub confirmed_at: Timestamp,
    pub result: InventoryRelocationConfirmationResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountItemLocationTask {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
    pub item_id: i64,
    pub inventory_balance_id: i64,
    pub order_id: Option<i64>,
    pub order_item_id: Option<i64>,
    pub source: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountClaimLocation {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountClaimItem {
    pub item_id: i64,
    pub description: Option<String>,
    pub barcodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountClaimStock {
    pub inventory_balance_id: i64,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub inventory_status: InventoryStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountClaim {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<Timestamp>,
    pub lease_expires_at: Timestamp,
    pub location: CycleCountClaimLocation,
    pub item: CycleCountClaimItem,
    pub stock: CycleCountClaimStock,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountClaimHeartbeat {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub heartbeat_by: i64,
    pub heartbeat_at: Timestamp,
    pub previous_lease_expires_at: Timestamp,
    pub lease_expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SafetyIssue,
    Other,
}

impl CycleCountClaimReleaseReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkInterrupted => "work_interrupted",
            Self::EquipmentUnavailable => "equipment_unavailable",
            Self::SafetyIssue => "safety_issue",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountClaimRelease {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub released_by: i64,
    pub released_at: Timestamp,
    pub release_count: i64,
    pub reason: CycleCountClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemLocationCycleCountConfirmation {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub location_id: i64,
    pub inventory_balance_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub inventory_status: InventoryStatus,
    pub previous_on_hand_quantity: i64,
    pub reserved_quantity: i64,
    pub held_quantity: i64,
    pub counted_quantity: i64,
    pub variance_quantity: i64,
    pub inventory_transaction_id: Option<i64>,
    pub disposition: wareboxes_domain::CycleCountDisposition,
    pub variance_id: Option<wareboxes_domain::CycleCountVarianceId>,
    pub variance_revision: Option<wareboxes_domain::CycleCountVarianceRevision>,
    pub next_recount_task_id: Option<i64>,
    pub confirmed_by: i64,
    pub confirmed_at: Timestamp,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CycleCountLocationTask {
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BreakMasterPackTask {
    pub tenant_id: TenantId,
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
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub facility_id: i64,
    pub inventory_owner_id: i64,
    pub order_id: i64,
    #[serde(default)]
    pub lines: Vec<UnpackCancelledOrderTaskLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnpackCancelledOrderTaskLine {
    pub id: i64,
    pub tenant_id: TenantId,
    pub task_id: i64,
    pub facility_id: i64,
    pub inventory_owner_id: i64,
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
    pub tenant_id: TenantId,
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
