use wareboxes_domain::{FacilityId, InventoryOwnerId, Timestamp};

pub const MAX_INVENTORY_BALANCE_PAGE_SIZE: u16 = 1_000;
pub const MAX_INVENTORY_BALANCE_QUERY_LENGTH: usize = 200;
pub const MAX_INVENTORY_HOLD_PAGE_SIZE: u16 = 1_000;
pub const MAX_INVENTORY_ROLLUP_PAGE_SIZE: u16 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryBalanceStatus {
    Available,
    Hold,
    Damaged,
    Quarantine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryBalanceSort {
    Position,
    Facility,
    Client,
    Location,
    Item,
    Tracking,
    LicensePlate,
    Status,
    OnHand,
    Reserved,
    Held,
    Available,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryBalanceSortDirection {
    Ascending,
    Descending,
}

impl InventoryBalanceSortDirection {
    pub const fn is_ascending(self) -> bool {
        matches!(self, Self::Ascending)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryBalancePageQuery {
    pub offset: u64,
    pub limit: u16,
    pub query: Option<String>,
    pub sort: InventoryBalanceSort,
    pub direction: InventoryBalanceSortDirection,
    pub movable_only: bool,
}

impl InventoryBalanceStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "available" => Some(Self::Available),
            "hold" => Some(Self::Hold),
            "damaged" => Some(Self::Damaged),
            "quarantine" => Some(Self::Quarantine),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryQuantityProjectionError {
    NegativeQuantity,
    CommittedQuantityExceedsOnHand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryQuantityProjection {
    on_hand: i64,
    reserved: i64,
    held: i64,
    available: i64,
}

impl InventoryQuantityProjection {
    pub fn new(
        status: InventoryBalanceStatus,
        on_hand: i64,
        reserved: i64,
        held: i64,
    ) -> Result<Self, InventoryQuantityProjectionError> {
        if on_hand < 0 || reserved < 0 || held < 0 {
            return Err(InventoryQuantityProjectionError::NegativeQuantity);
        }
        let uncommitted = on_hand
            .checked_sub(reserved)
            .and_then(|quantity| quantity.checked_sub(held))
            .ok_or(InventoryQuantityProjectionError::CommittedQuantityExceedsOnHand)?;
        if uncommitted < 0 {
            return Err(InventoryQuantityProjectionError::CommittedQuantityExceedsOnHand);
        }
        let available = if status == InventoryBalanceStatus::Available {
            uncommitted
        } else {
            0
        };

        Ok(Self {
            on_hand,
            reserved,
            held,
            available,
        })
    }

    pub const fn on_hand(self) -> i64 {
        self.on_hand
    }

    pub const fn reserved(self) -> i64 {
        self.reserved
    }

    pub const fn held(self) -> i64 {
        self.held
    }

    pub const fn available(self) -> i64 {
        self.available
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryBalanceReadModel {
    pub id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub uom: String,
    pub status: InventoryBalanceStatus,
    pub quantity: InventoryQuantityProjection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryBalancePage {
    pub items: Vec<InventoryBalanceReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryHoldReason {
    QualityInspection,
    DamageSuspected,
    InventoryDiscrepancy,
    Regulatory,
    CustomerRequest,
    Other,
}

impl InventoryHoldReason {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "quality_inspection" => Some(Self::QualityInspection),
            "damage_suspected" => Some(Self::DamageSuspected),
            "inventory_discrepancy" => Some(Self::InventoryDiscrepancy),
            "regulatory" => Some(Self::Regulatory),
            "customer_request" => Some(Self::CustomerRequest),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryHoldStatus {
    Active,
    Released,
}

impl InventoryHoldStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "released" => Some(Self::Released),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryHoldQuantity(i64);

impl InventoryHoldQuantity {
    pub const fn new(value: i64) -> Option<Self> {
        if value > 0 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryHoldPageFilter {
    pub offset: u64,
    pub limit: u16,
    pub status: Option<InventoryHoldStatus>,
    pub query: Option<String>,
    pub sort: InventoryHoldSort,
    pub direction: InventoryBalanceSortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryHoldSort {
    Id,
    Item,
    Client,
    Position,
    Reason,
    Created,
    Quantity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryHoldReadModel {
    pub id: i64,
    pub created_at: Timestamp,
    pub created_by_user_id: i64,
    pub released_at: Option<Timestamp>,
    pub released_by_user_id: Option<i64>,
    pub inventory_balance_id: i64,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_barcode: Option<String>,
    pub location_name: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub inventory_status: InventoryBalanceStatus,
    pub quantity: InventoryHoldQuantity,
    pub reason: InventoryHoldReason,
    pub note: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub status: InventoryHoldStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryHoldPage {
    pub items: Vec<InventoryHoldReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryRollupSort {
    Client,
    Item,
    Scope,
    Balances,
    Batches,
    Locations,
}

impl InventoryRollupSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Item => "item",
            Self::Scope => "scope",
            Self::Balances => "balances",
            Self::Batches => "batches",
            Self::Locations => "locations",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryRollupSortDirection {
    Ascending,
    Descending,
}

impl InventoryRollupSortDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryRollupPageQuery {
    pub offset: u64,
    pub limit: u16,
    pub query: Option<String>,
    pub sort: InventoryRollupSort,
    pub direction: InventoryRollupSortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryRollupQuantityError {
    EmptyUom,
    NegativeQuantity,
    CommittedQuantityExceedsOnHand,
    AvailableQuantityExceedsUncommitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryRollupQuantity {
    uom: String,
    on_hand: i64,
    reserved: i64,
    held: i64,
    available: i64,
}

impl InventoryRollupQuantity {
    pub fn new(
        uom: String,
        on_hand: i64,
        reserved: i64,
        held: i64,
        available: i64,
    ) -> Result<Self, InventoryRollupQuantityError> {
        if uom.trim().is_empty() {
            return Err(InventoryRollupQuantityError::EmptyUom);
        }
        if on_hand < 0 || reserved < 0 || held < 0 || available < 0 {
            return Err(InventoryRollupQuantityError::NegativeQuantity);
        }
        let uncommitted = on_hand
            .checked_sub(reserved)
            .and_then(|quantity| quantity.checked_sub(held))
            .ok_or(InventoryRollupQuantityError::CommittedQuantityExceedsOnHand)?;
        if uncommitted < 0 {
            return Err(InventoryRollupQuantityError::CommittedQuantityExceedsOnHand);
        }
        if available > uncommitted {
            return Err(InventoryRollupQuantityError::AvailableQuantityExceedsUncommitted);
        }
        Ok(Self {
            uom,
            on_hand,
            reserved,
            held,
            available,
        })
    }

    pub fn into_parts(self) -> (String, i64, i64, i64, i64) {
        (
            self.uom,
            self.on_hand,
            self.reserved,
            self.held,
            self.available,
        )
    }

    pub const fn on_hand(&self) -> i64 {
        self.on_hand
    }

    pub const fn reserved(&self) -> i64 {
        self.reserved
    }

    pub const fn held(&self) -> i64 {
        self.held
    }

    pub const fn available(&self) -> i64 {
        self.available
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryRollupCount(i64);

impl InventoryRollupCount {
    pub const fn new(value: i64) -> Option<Self> {
        if value > 0 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryLocationRollupReadModel {
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub facility_id: FacilityId,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub quantities: Vec<InventoryRollupQuantity>,
    pub balance_count: InventoryRollupCount,
    pub batch_count: InventoryRollupCount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryLocationRollupPage {
    pub items: Vec<InventoryLocationRollupReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryFacilityRollupReadModel {
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub facility_id: FacilityId,
    pub facility_name: Option<String>,
    pub quantities: Vec<InventoryRollupQuantity>,
    pub balance_count: InventoryRollupCount,
    pub batch_count: InventoryRollupCount,
    pub location_count: InventoryRollupCount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryFacilityRollupPage {
    pub items: Vec<InventoryFacilityRollupReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryItemRollupReadModel {
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub quantities: Vec<InventoryRollupQuantity>,
    pub balance_count: InventoryRollupCount,
    pub batch_count: InventoryRollupCount,
    pub location_count: InventoryRollupCount,
    pub facility_count: InventoryRollupCount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryItemRollupPage {
    pub items: Vec<InventoryItemRollupReadModel>,
    pub next_offset: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parsing_rejects_unknown_persisted_values() {
        assert_eq!(
            InventoryBalanceStatus::parse("quarantine"),
            Some(InventoryBalanceStatus::Quarantine)
        );
        assert_eq!(InventoryBalanceStatus::parse("QUARANTINE"), None);
        assert_eq!(InventoryBalanceStatus::parse("unknown"), None);
    }

    #[test]
    fn available_quantity_requires_nonnegative_uncommitted_stock() {
        let available =
            InventoryQuantityProjection::new(InventoryBalanceStatus::Available, 12, 3, 2).unwrap();
        assert_eq!(available.available(), 7);

        let held =
            InventoryQuantityProjection::new(InventoryBalanceStatus::Hold, 12, 3, 2).unwrap();
        assert_eq!(held.available(), 0);
        assert_eq!(
            InventoryQuantityProjection::new(InventoryBalanceStatus::Available, 4, 3, 2),
            Err(InventoryQuantityProjectionError::CommittedQuantityExceedsOnHand)
        );
        assert_eq!(
            InventoryQuantityProjection::new(InventoryBalanceStatus::Available, 4, -1, 0),
            Err(InventoryQuantityProjectionError::NegativeQuantity)
        );
    }

    #[test]
    fn hold_classifications_reject_unknown_persisted_values() {
        assert_eq!(
            InventoryHoldReason::parse("quality_inspection"),
            Some(InventoryHoldReason::QualityInspection)
        );
        assert_eq!(InventoryHoldReason::parse("unknown"), None);
        assert_eq!(
            InventoryHoldStatus::parse("released"),
            Some(InventoryHoldStatus::Released)
        );
        assert_eq!(InventoryHoldStatus::parse("RELEASED"), None);
    }

    #[test]
    fn hold_quantity_must_be_positive() {
        assert_eq!(
            InventoryHoldQuantity::new(4).map(InventoryHoldQuantity::get),
            Some(4)
        );
        assert_eq!(InventoryHoldQuantity::new(0), None);
        assert_eq!(InventoryHoldQuantity::new(-1), None);
    }

    #[test]
    fn rollup_quantities_reject_invalid_aggregates() {
        let quantity = InventoryRollupQuantity::new("each".to_owned(), 12, 3, 2, 7).unwrap();
        assert_eq!(quantity.into_parts(), ("each".to_owned(), 12, 3, 2, 7));
        assert_eq!(
            InventoryRollupQuantity::new(" ".to_owned(), 1, 0, 0, 1),
            Err(InventoryRollupQuantityError::EmptyUom)
        );
        assert_eq!(
            InventoryRollupQuantity::new("each".to_owned(), 4, 3, 2, 0),
            Err(InventoryRollupQuantityError::CommittedQuantityExceedsOnHand)
        );
        assert_eq!(
            InventoryRollupQuantity::new("each".to_owned(), 5, 1, 1, 4),
            Err(InventoryRollupQuantityError::AvailableQuantityExceedsUncommitted)
        );
    }

    #[test]
    fn rollup_counts_must_be_positive() {
        assert_eq!(
            InventoryRollupCount::new(2).map(InventoryRollupCount::get),
            Some(2)
        );
        assert_eq!(InventoryRollupCount::new(0), None);
    }
}
