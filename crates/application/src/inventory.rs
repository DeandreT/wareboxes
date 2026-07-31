use wareboxes_domain::{FacilityId, InventoryOwnerId};

pub const MAX_INVENTORY_BALANCE_PAGE_SIZE: u16 = 1_000;
pub const MAX_INVENTORY_BALANCE_QUERY_LENGTH: usize = 200;

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
}
