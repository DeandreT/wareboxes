use wareboxes_domain::{FacilityId, InventoryOwnerId, Timestamp};

pub const MAX_INVENTORY_BALANCE_PAGE_SIZE: u16 = 1_000;
pub const MAX_INVENTORY_BALANCE_QUERY_LENGTH: usize = 200;
pub const MAX_INVENTORY_HOLD_PAGE_SIZE: u16 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryBalanceStatus {
    Available,
    Hold,
    Damaged,
    Quarantine,
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
    pub next_after_id: Option<i64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryHoldPageFilter {
    pub before_id: Option<i64>,
    pub limit: u16,
    pub status: Option<InventoryHoldStatus>,
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
    pub next_before_id: Option<i64>,
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
}
