use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{
    CatalogItemId, FacilityId, InventoryOwnerId, LocationId, PurchaseOrderId, PurchaseOrderLineId,
    PurchaseOrderRevision, Timestamp,
};

pub const MAX_INBOUND_ASN_NUMBER_LENGTH: usize = 120;
pub const MAX_INBOUND_ASN_SUPPLIER_LENGTH: usize = 200;
pub const MAX_INBOUND_ASN_IDENTITY_LENGTH: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundAsnError {
    #[error("{field} must be nonblank, trimmed, and control-free")]
    InvalidText { field: &'static str },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong { field: &'static str, maximum: usize },
    #[error("expected quantity must be positive, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("an advance shipping notice requires at least one line")]
    MissingLines,
    #[error("an advance shipping notice cannot contain duplicate expected identities")]
    DuplicateLineIdentity,
    #[error("advance shipping notice revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("advance shipping notice revision cannot advance beyond its supported range")]
    RevisionExhausted,
    #[error("a purchase-order ASN requires at least one source line")]
    MissingPurchaseOrderLines,
    #[error("a purchase-order ASN cannot repeat the same source line identity")]
    DuplicatePurchaseOrderLineIdentity,
    #[error("only an open advance shipping notice can be planned into a load")]
    InvalidStatus,
}

fn required_text(
    value: impl Into<String>,
    field: &'static str,
    maximum: usize,
) -> Result<String, InboundAsnError> {
    let value = value.into();
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(InboundAsnError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(InboundAsnError::TextTooLong { field, maximum });
    }
    Ok(value)
}

fn optional_text(
    value: Option<String>,
    field: &'static str,
    maximum: usize,
) -> Result<Option<String>, InboundAsnError> {
    value
        .map(|value| required_text(value, field, maximum))
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboundAsnNumber(String);

impl InboundAsnNumber {
    pub fn new(value: impl Into<String>) -> Result<Self, InboundAsnError> {
        required_text(value, "ASN number", MAX_INBOUND_ASN_NUMBER_LENGTH).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboundAsnSupplier(String);

impl InboundAsnSupplier {
    pub fn new(value: impl Into<String>) -> Result<Self, InboundAsnError> {
        required_text(value, "supplier", MAX_INBOUND_ASN_SUPPLIER_LENGTH).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboundAsnRevision(i64);

impl InboundAsnRevision {
    pub const fn new(value: i64) -> Result<Self, InboundAsnError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(InboundAsnError::InvalidRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn next(self) -> Result<Self, InboundAsnError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(InboundAsnError::RevisionExhausted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundAsnStatus {
    Open,
    Planned,
}

impl InboundAsnStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Planned => "planned",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "planned" => Some(Self::Planned),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboundAsnQuantity(i64);

impl InboundAsnQuantity {
    pub const fn new(value: i64) -> Result<Self, InboundAsnError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(InboundAsnError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundAsnLineDefinition {
    item_id: CatalogItemId,
    expected_quantity: InboundAsnQuantity,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
}

impl InboundAsnLineDefinition {
    pub fn new(
        item_id: CatalogItemId,
        expected_quantity: InboundAsnQuantity,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<Timestamp>,
    ) -> Result<Self, InboundAsnError> {
        Ok(Self {
            item_id,
            expected_quantity,
            lot: optional_text(lot, "lot", MAX_INBOUND_ASN_IDENTITY_LENGTH)?,
            serial: optional_text(serial, "serial", MAX_INBOUND_ASN_IDENTITY_LENGTH)?,
            expiration,
        })
    }

    pub const fn item_id(&self) -> CatalogItemId {
        self.item_id
    }

    pub const fn expected_quantity(&self) -> InboundAsnQuantity {
        self.expected_quantity
    }

    pub fn lot(&self) -> Option<&str> {
        self.lot.as_deref()
    }

    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    pub const fn expiration(&self) -> Option<&Timestamp> {
        self.expiration.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewInboundAsn {
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    number: InboundAsnNumber,
    supplier: InboundAsnSupplier,
    expected_at: Option<Timestamp>,
    lines: Vec<InboundAsnLineDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseOrderAsnLineDefinition {
    purchase_order_line_id: PurchaseOrderLineId,
    expected_quantity: InboundAsnQuantity,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
}

impl PurchaseOrderAsnLineDefinition {
    pub fn new(
        purchase_order_line_id: PurchaseOrderLineId,
        expected_quantity: InboundAsnQuantity,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<Timestamp>,
    ) -> Result<Self, InboundAsnError> {
        Ok(Self {
            purchase_order_line_id,
            expected_quantity,
            lot: optional_text(lot, "lot", MAX_INBOUND_ASN_IDENTITY_LENGTH)?,
            serial: optional_text(serial, "serial", MAX_INBOUND_ASN_IDENTITY_LENGTH)?,
            expiration,
        })
    }

    pub const fn purchase_order_line_id(&self) -> PurchaseOrderLineId {
        self.purchase_order_line_id
    }

    pub const fn expected_quantity(&self) -> InboundAsnQuantity {
        self.expected_quantity
    }

    pub fn lot(&self) -> Option<&str> {
        self.lot.as_deref()
    }

    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    pub const fn expiration(&self) -> Option<&Timestamp> {
        self.expiration.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPurchaseOrderAsn {
    purchase_order_id: PurchaseOrderId,
    expected_purchase_order_revision: PurchaseOrderRevision,
    number: InboundAsnNumber,
    expected_at: Option<Timestamp>,
    lines: Vec<PurchaseOrderAsnLineDefinition>,
}

impl NewPurchaseOrderAsn {
    pub fn new(
        purchase_order_id: PurchaseOrderId,
        expected_purchase_order_revision: PurchaseOrderRevision,
        number: InboundAsnNumber,
        expected_at: Option<Timestamp>,
        lines: Vec<PurchaseOrderAsnLineDefinition>,
    ) -> Result<Self, InboundAsnError> {
        if lines.is_empty() {
            return Err(InboundAsnError::MissingPurchaseOrderLines);
        }
        let unique = lines
            .iter()
            .map(|line| {
                (
                    line.purchase_order_line_id,
                    line.lot.clone(),
                    line.serial.clone(),
                    line.expiration,
                )
            })
            .collect::<HashSet<_>>();
        if unique.len() != lines.len() {
            return Err(InboundAsnError::DuplicatePurchaseOrderLineIdentity);
        }
        Ok(Self {
            purchase_order_id,
            expected_purchase_order_revision,
            number,
            expected_at,
            lines,
        })
    }

    pub const fn purchase_order_id(&self) -> PurchaseOrderId {
        self.purchase_order_id
    }

    pub const fn expected_purchase_order_revision(&self) -> PurchaseOrderRevision {
        self.expected_purchase_order_revision
    }

    pub const fn number(&self) -> &InboundAsnNumber {
        &self.number
    }

    pub const fn expected_at(&self) -> Option<&Timestamp> {
        self.expected_at.as_ref()
    }

    pub fn lines(&self) -> &[PurchaseOrderAsnLineDefinition] {
        &self.lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundAsnLoadPlanDetails {
    receiving_location_id: LocationId,
    carrier: Option<String>,
    trailer_number: Option<String>,
    seal_number: Option<String>,
}

impl InboundAsnLoadPlanDetails {
    pub fn new(
        receiving_location_id: LocationId,
        carrier: Option<String>,
        trailer_number: Option<String>,
        seal_number: Option<String>,
    ) -> Result<Self, InboundAsnError> {
        Ok(Self {
            receiving_location_id,
            carrier: optional_text(carrier, "carrier", MAX_INBOUND_ASN_SUPPLIER_LENGTH)?,
            trailer_number: optional_text(
                trailer_number,
                "trailer number",
                MAX_INBOUND_ASN_IDENTITY_LENGTH,
            )?,
            seal_number: optional_text(
                seal_number,
                "seal number",
                MAX_INBOUND_ASN_IDENTITY_LENGTH,
            )?,
        })
    }

    pub const fn receiving_location_id(&self) -> LocationId {
        self.receiving_location_id
    }

    pub fn carrier(&self) -> Option<&str> {
        self.carrier.as_deref()
    }

    pub fn trailer_number(&self) -> Option<&str> {
        self.trailer_number.as_deref()
    }

    pub fn seal_number(&self) -> Option<&str> {
        self.seal_number.as_deref()
    }
}

impl NewInboundAsn {
    pub fn new(
        inventory_owner_id: InventoryOwnerId,
        facility_id: FacilityId,
        number: InboundAsnNumber,
        supplier: InboundAsnSupplier,
        expected_at: Option<Timestamp>,
        lines: Vec<InboundAsnLineDefinition>,
    ) -> Result<Self, InboundAsnError> {
        if lines.is_empty() {
            return Err(InboundAsnError::MissingLines);
        }
        let unique = lines
            .iter()
            .map(|line| {
                (
                    line.item_id,
                    line.lot.clone(),
                    line.serial.clone(),
                    line.expiration,
                )
            })
            .collect::<HashSet<_>>();
        if unique.len() != lines.len() {
            return Err(InboundAsnError::DuplicateLineIdentity);
        }
        Ok(Self {
            inventory_owner_id,
            facility_id,
            number,
            supplier,
            expected_at,
            lines,
        })
    }

    pub const fn inventory_owner_id(&self) -> InventoryOwnerId {
        self.inventory_owner_id
    }

    pub const fn facility_id(&self) -> FacilityId {
        self.facility_id
    }

    pub const fn number(&self) -> &InboundAsnNumber {
        &self.number
    }

    pub const fn supplier(&self) -> &InboundAsnSupplier {
        &self.supplier
    }

    pub const fn expected_at(&self) -> Option<&Timestamp> {
        self.expected_at.as_ref()
    }

    pub fn lines(&self) -> &[InboundAsnLineDefinition] {
        &self.lines
    }
}

pub fn plan_inbound_asn(
    status: InboundAsnStatus,
    revision: InboundAsnRevision,
) -> Result<InboundAsnRevision, InboundAsnError> {
    if status != InboundAsnStatus::Open {
        return Err(InboundAsnError::InvalidStatus);
    }
    revision.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(lot: Option<&str>) -> InboundAsnLineDefinition {
        InboundAsnLineDefinition::new(
            CatalogItemId::new(9).unwrap(),
            InboundAsnQuantity::new(4).unwrap(),
            lot.map(str::to_owned),
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn notice_requires_a_unique_nonempty_line_set() {
        let build = |lines| {
            NewInboundAsn::new(
                InventoryOwnerId::new(2).unwrap(),
                FacilityId::new(3).unwrap(),
                InboundAsnNumber::new("ASN-100").unwrap(),
                InboundAsnSupplier::new("Northstar Foods").unwrap(),
                None,
                lines,
            )
        };
        assert_eq!(build(vec![]), Err(InboundAsnError::MissingLines));
        assert_eq!(
            build(vec![line(Some("LOT-A")), line(Some("LOT-A"))]),
            Err(InboundAsnError::DuplicateLineIdentity)
        );
        assert!(build(vec![line(Some("LOT-A")), line(Some("LOT-B"))]).is_ok());
    }

    #[test]
    fn planning_is_open_only_and_advances_revision() {
        let revision = InboundAsnRevision::new(1).unwrap();
        assert_eq!(
            plan_inbound_asn(InboundAsnStatus::Open, revision),
            Ok(InboundAsnRevision::new(2).unwrap())
        );
        assert_eq!(
            plan_inbound_asn(InboundAsnStatus::Planned, revision),
            Err(InboundAsnError::InvalidStatus)
        );
        assert_eq!(
            plan_inbound_asn(
                InboundAsnStatus::Open,
                InboundAsnRevision::new(i64::MAX).unwrap()
            ),
            Err(InboundAsnError::RevisionExhausted)
        );
    }

    #[test]
    fn source_text_and_quantities_are_canonical() {
        assert!(InboundAsnNumber::new(" ASN-100").is_err());
        assert!(InboundAsnSupplier::new("Northstar\nFoods").is_err());
        assert!(InboundAsnQuantity::new(0).is_err());
        assert!(InboundAsnLineDefinition::new(
            CatalogItemId::new(9).unwrap(),
            InboundAsnQuantity::new(1).unwrap(),
            Some("LOT-A ".into()),
            None,
            None,
        )
        .is_err());
    }

    #[test]
    fn purchase_order_notice_requires_unique_source_identities() {
        let line = || {
            PurchaseOrderAsnLineDefinition::new(
                PurchaseOrderLineId::new(7).unwrap(),
                InboundAsnQuantity::new(2).unwrap(),
                Some("LOT-A".into()),
                None,
                None,
            )
            .unwrap()
        };
        let build = |lines| {
            NewPurchaseOrderAsn::new(
                PurchaseOrderId::new(6).unwrap(),
                PurchaseOrderRevision::new(2).unwrap(),
                InboundAsnNumber::new("ASN-PO-100").unwrap(),
                None,
                lines,
            )
        };
        assert_eq!(
            build(Vec::new()),
            Err(InboundAsnError::MissingPurchaseOrderLines)
        );
        assert_eq!(
            build(vec![line(), line()]),
            Err(InboundAsnError::DuplicatePurchaseOrderLineIdentity)
        );
    }
}
