use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{CatalogItemId, FacilityId, InventoryOwnerId, Timestamp};

pub const MAX_PURCHASE_ORDER_NUMBER_LENGTH: usize = 120;
pub const MAX_PURCHASE_ORDER_SUPPLIER_LENGTH: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PurchaseOrderError {
    #[error("{field} must be nonblank, trimmed, and control-free")]
    InvalidText { field: &'static str },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong { field: &'static str, maximum: usize },
    #[error("ordered quantity must be positive, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("a purchase order requires at least one line")]
    MissingLines,
    #[error("a purchase order cannot contain the same item more than once")]
    DuplicateItem,
    #[error("purchase order revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("purchase order revision cannot advance beyond its supported range")]
    RevisionExhausted,
    #[error("only a draft purchase order can be released")]
    InvalidReleaseStatus,
    #[error(
        "purchase-order demand coverage must satisfy 0 <= received + active inbound <= ordered"
    )]
    InvalidDemandCoverage,
}

fn required_text(
    value: impl Into<String>,
    field: &'static str,
    maximum: usize,
) -> Result<String, PurchaseOrderError> {
    let value = value.into();
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(PurchaseOrderError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(PurchaseOrderError::TextTooLong { field, maximum });
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PurchaseOrderNumber(String);

impl PurchaseOrderNumber {
    pub fn new(value: impl Into<String>) -> Result<Self, PurchaseOrderError> {
        required_text(
            value,
            "purchase order number",
            MAX_PURCHASE_ORDER_NUMBER_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PurchaseOrderSupplier(String);

impl PurchaseOrderSupplier {
    pub fn new(value: impl Into<String>) -> Result<Self, PurchaseOrderError> {
        required_text(value, "supplier", MAX_PURCHASE_ORDER_SUPPLIER_LENGTH).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PurchaseOrderRevision(i64);

impl PurchaseOrderRevision {
    pub const fn new(value: i64) -> Result<Self, PurchaseOrderError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PurchaseOrderError::InvalidRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn next(self) -> Result<Self, PurchaseOrderError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(PurchaseOrderError::RevisionExhausted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PurchaseOrderStatus {
    Draft,
    Released,
}

impl PurchaseOrderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Released => "released",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PurchaseOrderQuantity(i64);

impl PurchaseOrderQuantity {
    pub const fn new(value: i64) -> Result<Self, PurchaseOrderError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PurchaseOrderError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseOrderDemandCoverage {
    ordered_quantity: i64,
    received_quantity: i64,
    active_inbound_quantity: i64,
}

impl PurchaseOrderDemandCoverage {
    pub fn new(
        ordered_quantity: i64,
        received_quantity: i64,
        active_inbound_quantity: i64,
    ) -> Result<Self, PurchaseOrderError> {
        if ordered_quantity <= 0
            || received_quantity < 0
            || active_inbound_quantity < 0
            || received_quantity
                .checked_add(active_inbound_quantity)
                .is_none_or(|covered| covered > ordered_quantity)
        {
            return Err(PurchaseOrderError::InvalidDemandCoverage);
        }
        Ok(Self {
            ordered_quantity,
            received_quantity,
            active_inbound_quantity,
        })
    }

    pub const fn open_receipt_quantity(self) -> i64 {
        self.ordered_quantity - self.received_quantity
    }

    pub const fn available_to_notify_quantity(self) -> i64 {
        self.ordered_quantity - self.received_quantity - self.active_inbound_quantity
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseOrderLineDefinition {
    item_id: CatalogItemId,
    ordered_quantity: PurchaseOrderQuantity,
}

impl PurchaseOrderLineDefinition {
    pub const fn new(item_id: CatalogItemId, ordered_quantity: PurchaseOrderQuantity) -> Self {
        Self {
            item_id,
            ordered_quantity,
        }
    }

    pub const fn item_id(&self) -> CatalogItemId {
        self.item_id
    }

    pub const fn ordered_quantity(&self) -> PurchaseOrderQuantity {
        self.ordered_quantity
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPurchaseOrder {
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    number: PurchaseOrderNumber,
    supplier: PurchaseOrderSupplier,
    expected_by: Option<Timestamp>,
    lines: Vec<PurchaseOrderLineDefinition>,
}

impl NewPurchaseOrder {
    pub fn new(
        inventory_owner_id: InventoryOwnerId,
        facility_id: FacilityId,
        number: PurchaseOrderNumber,
        supplier: PurchaseOrderSupplier,
        expected_by: Option<Timestamp>,
        lines: Vec<PurchaseOrderLineDefinition>,
    ) -> Result<Self, PurchaseOrderError> {
        if lines.is_empty() {
            return Err(PurchaseOrderError::MissingLines);
        }
        let item_ids = lines
            .iter()
            .map(|line| line.item_id)
            .collect::<HashSet<_>>();
        if item_ids.len() != lines.len() {
            return Err(PurchaseOrderError::DuplicateItem);
        }
        Ok(Self {
            inventory_owner_id,
            facility_id,
            number,
            supplier,
            expected_by,
            lines,
        })
    }

    pub const fn inventory_owner_id(&self) -> InventoryOwnerId {
        self.inventory_owner_id
    }

    pub const fn facility_id(&self) -> FacilityId {
        self.facility_id
    }

    pub const fn number(&self) -> &PurchaseOrderNumber {
        &self.number
    }

    pub const fn supplier(&self) -> &PurchaseOrderSupplier {
        &self.supplier
    }

    pub const fn expected_by(&self) -> Option<&Timestamp> {
        self.expected_by.as_ref()
    }

    pub fn lines(&self) -> &[PurchaseOrderLineDefinition] {
        &self.lines
    }
}

pub fn release_purchase_order(
    status: PurchaseOrderStatus,
    revision: PurchaseOrderRevision,
) -> Result<PurchaseOrderRevision, PurchaseOrderError> {
    if status != PurchaseOrderStatus::Draft {
        return Err(PurchaseOrderError::InvalidReleaseStatus);
    }
    revision.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(item_id: i64) -> PurchaseOrderLineDefinition {
        PurchaseOrderLineDefinition::new(
            CatalogItemId::new(item_id).unwrap(),
            PurchaseOrderQuantity::new(4).unwrap(),
        )
    }

    #[test]
    fn purchase_order_requires_unique_nonempty_lines() {
        let build = |lines| {
            NewPurchaseOrder::new(
                InventoryOwnerId::new(2).unwrap(),
                FacilityId::new(3).unwrap(),
                PurchaseOrderNumber::new("PO-100").unwrap(),
                PurchaseOrderSupplier::new("Northstar Foods").unwrap(),
                None,
                lines,
            )
        };
        assert_eq!(build(Vec::new()), Err(PurchaseOrderError::MissingLines));
        assert_eq!(
            build(vec![line(8), line(8)]),
            Err(PurchaseOrderError::DuplicateItem)
        );
        assert!(build(vec![line(8), line(9)]).is_ok());
    }

    #[test]
    fn release_is_draft_only_and_revisioned() {
        let revision = PurchaseOrderRevision::new(1).unwrap();
        assert_eq!(
            release_purchase_order(PurchaseOrderStatus::Draft, revision),
            Ok(PurchaseOrderRevision::new(2).unwrap())
        );
        assert_eq!(
            release_purchase_order(PurchaseOrderStatus::Released, revision),
            Err(PurchaseOrderError::InvalidReleaseStatus)
        );
    }

    #[test]
    fn source_fields_are_canonical() {
        assert!(PurchaseOrderNumber::new(" PO-100").is_err());
        assert!(PurchaseOrderSupplier::new("Northstar\nFoods").is_err());
        assert!(PurchaseOrderQuantity::new(0).is_err());
    }

    #[test]
    fn receipt_exceptions_restore_notification_capacity_without_reducing_open_demand() {
        let progress = PurchaseOrderDemandCoverage::new(22, 2, 19).unwrap();
        assert_eq!(progress.available_to_notify_quantity(), 1);
        assert_eq!(progress.open_receipt_quantity(), 20);
        assert!(PurchaseOrderDemandCoverage::new(22, 2, 21).is_err());
    }
}
