use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CatalogItemId, FacilityId, InventoryOwnerId, LocationId, Timestamp};

pub const MAX_INBOUND_LOAD_REFERENCE_LENGTH: usize = 200;
pub const MAX_INBOUND_LOAD_TEXT_LENGTH: usize = 200;
pub const MAX_INBOUND_LOAD_IDENTITY_LENGTH: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundLoadField {
    Reference,
    Invoice,
    Carrier,
    Trailer,
    Seal,
    Lot,
    Serial,
}

impl fmt::Display for InboundLoadField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Reference => "load reference",
            Self::Invoice => "invoice number",
            Self::Carrier => "carrier",
            Self::Trailer => "trailer number",
            Self::Seal => "seal number",
            Self::Lot => "lot",
            Self::Serial => "serial",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundLoadPlanningError {
    #[error("{field} must be trimmed and nonblank")]
    InvalidText { field: InboundLoadField },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong {
        field: InboundLoadField,
        maximum: usize,
    },
    #[error("expected quantity must be positive, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("an inbound load requires at least one expected line")]
    MissingLines,
    #[error("an inbound load cannot contain duplicate expected item identities")]
    DuplicateLineIdentity,
}

fn required_text(
    value: impl Into<String>,
    field: InboundLoadField,
    maximum: usize,
) -> Result<String, InboundLoadPlanningError> {
    let value = value.into();
    if value.is_empty() || value.trim() != value {
        return Err(InboundLoadPlanningError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(InboundLoadPlanningError::TextTooLong { field, maximum });
    }
    Ok(value)
}

fn optional_text(
    value: Option<String>,
    field: InboundLoadField,
    maximum: usize,
) -> Result<Option<String>, InboundLoadPlanningError> {
    value
        .map(|value| required_text(value, field, maximum))
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboundLoadReference(String);

impl InboundLoadReference {
    pub fn new(value: impl Into<String>) -> Result<Self, InboundLoadPlanningError> {
        required_text(
            value,
            InboundLoadField::Reference,
            MAX_INBOUND_LOAD_REFERENCE_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InboundLoadReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboundExpectedQuantity(i64);

impl InboundExpectedQuantity {
    pub const fn new(value: i64) -> Result<Self, InboundLoadPlanningError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(InboundLoadPlanningError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundLoadPlanLine {
    item_id: CatalogItemId,
    expected_quantity: InboundExpectedQuantity,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
}

impl InboundLoadPlanLine {
    pub fn new(
        item_id: CatalogItemId,
        expected_quantity: InboundExpectedQuantity,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<Timestamp>,
    ) -> Result<Self, InboundLoadPlanningError> {
        Ok(Self {
            item_id,
            expected_quantity,
            lot: optional_text(lot, InboundLoadField::Lot, MAX_INBOUND_LOAD_IDENTITY_LENGTH)?,
            serial: optional_text(
                serial,
                InboundLoadField::Serial,
                MAX_INBOUND_LOAD_IDENTITY_LENGTH,
            )?,
            expiration,
        })
    }

    pub const fn item_id(&self) -> CatalogItemId {
        self.item_id
    }

    pub const fn expected_quantity(&self) -> InboundExpectedQuantity {
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
pub struct NewInboundLoadPlan {
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    receiving_location_id: LocationId,
    reference: InboundLoadReference,
    invoice_number: Option<String>,
    carrier: Option<String>,
    trailer_number: Option<String>,
    seal_number: Option<String>,
    expected_at: Option<Timestamp>,
    appointment_at: Option<Timestamp>,
    lines: Vec<InboundLoadPlanLine>,
}

impl NewInboundLoadPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inventory_owner_id: InventoryOwnerId,
        facility_id: FacilityId,
        receiving_location_id: LocationId,
        reference: InboundLoadReference,
        invoice_number: Option<String>,
        carrier: Option<String>,
        trailer_number: Option<String>,
        seal_number: Option<String>,
        expected_at: Option<Timestamp>,
        appointment_at: Option<Timestamp>,
        lines: Vec<InboundLoadPlanLine>,
    ) -> Result<Self, InboundLoadPlanningError> {
        if lines.is_empty() {
            return Err(InboundLoadPlanningError::MissingLines);
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
            return Err(InboundLoadPlanningError::DuplicateLineIdentity);
        }
        Ok(Self {
            inventory_owner_id,
            facility_id,
            receiving_location_id,
            reference,
            invoice_number: optional_text(
                invoice_number,
                InboundLoadField::Invoice,
                MAX_INBOUND_LOAD_TEXT_LENGTH,
            )?,
            carrier: optional_text(
                carrier,
                InboundLoadField::Carrier,
                MAX_INBOUND_LOAD_TEXT_LENGTH,
            )?,
            trailer_number: optional_text(
                trailer_number,
                InboundLoadField::Trailer,
                MAX_INBOUND_LOAD_TEXT_LENGTH,
            )?,
            seal_number: optional_text(
                seal_number,
                InboundLoadField::Seal,
                MAX_INBOUND_LOAD_TEXT_LENGTH,
            )?,
            expected_at,
            appointment_at,
            lines,
        })
    }

    pub const fn inventory_owner_id(&self) -> InventoryOwnerId {
        self.inventory_owner_id
    }
    pub const fn facility_id(&self) -> FacilityId {
        self.facility_id
    }
    pub const fn receiving_location_id(&self) -> LocationId {
        self.receiving_location_id
    }
    pub const fn reference(&self) -> &InboundLoadReference {
        &self.reference
    }
    pub fn invoice_number(&self) -> Option<&str> {
        self.invoice_number.as_deref()
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
    pub const fn expected_at(&self) -> Option<&Timestamp> {
        self.expected_at.as_ref()
    }
    pub const fn appointment_at(&self) -> Option<&Timestamp> {
        self.appointment_at.as_ref()
    }
    pub fn lines(&self) -> &[InboundLoadPlanLine] {
        &self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(item_id: i64, lot: Option<&str>) -> InboundLoadPlanLine {
        InboundLoadPlanLine::new(
            CatalogItemId::new(item_id).unwrap(),
            InboundExpectedQuantity::new(4).unwrap(),
            lot.map(str::to_owned),
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn plan_requires_nonempty_unique_expected_identities() {
        let build = |lines| {
            NewInboundLoadPlan::new(
                InventoryOwnerId::new(2).unwrap(),
                FacilityId::new(3).unwrap(),
                LocationId::new(4).unwrap(),
                InboundLoadReference::new("ASN-100").unwrap(),
                None,
                None,
                None,
                None,
                None,
                None,
                lines,
            )
        };
        assert!(matches!(
            build(vec![]),
            Err(InboundLoadPlanningError::MissingLines)
        ));
        assert!(matches!(
            build(vec![line(9, Some("LOT-A")), line(9, Some("LOT-A"))]),
            Err(InboundLoadPlanningError::DuplicateLineIdentity)
        ));
        assert!(build(vec![line(9, Some("LOT-A")), line(9, Some("LOT-B"))]).is_ok());
    }

    #[test]
    fn plan_rejects_untrimmed_text_and_nonpositive_quantity() {
        assert!(InboundLoadReference::new(" ASN-100").is_err());
        assert!(InboundExpectedQuantity::new(0).is_err());
        assert!(InboundLoadPlanLine::new(
            CatalogItemId::new(9).unwrap(),
            InboundExpectedQuantity::new(1).unwrap(),
            Some("LOT-A ".into()),
            None,
            None,
        )
        .is_err());
    }
}
