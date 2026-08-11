use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{CatalogItemId, FacilityId, InventoryOwnerId, LocationId, Timestamp};

pub const MAX_CUSTOMER_RETURN_NUMBER_LENGTH: usize = 120;
pub const MAX_CUSTOMER_RETURN_REFERENCE_LENGTH: usize = 200;
pub const MAX_CUSTOMER_RETURN_IDENTITY_LENGTH: usize = 200;
pub const MAX_CUSTOMER_RETURN_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CustomerReturnError {
    #[error("{field} must be nonblank, trimmed, and control-free")]
    InvalidText { field: &'static str },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong { field: &'static str, maximum: usize },
    #[error("authorized quantity must be positive, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("customer return revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("customer return revision cannot advance beyond its supported range")]
    RevisionExhausted,
    #[error("a customer return requires at least one line")]
    MissingLines,
    #[error("a customer return cannot repeat the same item identity")]
    DuplicateLineIdentity,
    #[error("a note is required when the reason is other")]
    MissingOtherNote,
    #[error("only an open customer return can be planned into a load")]
    InvalidPlanStatus,
    #[error("only an open customer return can be cancelled")]
    InvalidCancellationStatus,
}

fn required_text(
    value: impl Into<String>,
    field: &'static str,
    maximum: usize,
) -> Result<String, CustomerReturnError> {
    let value = value.into();
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(CustomerReturnError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(CustomerReturnError::TextTooLong { field, maximum });
    }
    Ok(value)
}

fn optional_text(
    value: Option<String>,
    field: &'static str,
    maximum: usize,
) -> Result<Option<String>, CustomerReturnError> {
    value
        .map(|value| required_text(value, field, maximum))
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CustomerReturnNumber(String);

impl CustomerReturnNumber {
    pub fn new(value: impl Into<String>) -> Result<Self, CustomerReturnError> {
        required_text(value, "return number", MAX_CUSTOMER_RETURN_NUMBER_LENGTH).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CustomerReturnReference(String);

impl CustomerReturnReference {
    pub fn new(value: impl Into<String>) -> Result<Self, CustomerReturnError> {
        required_text(
            value,
            "customer reference",
            MAX_CUSTOMER_RETURN_REFERENCE_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CustomerReturnQuantity(i64);

impl CustomerReturnQuantity {
    pub const fn new(value: i64) -> Result<Self, CustomerReturnError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(CustomerReturnError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CustomerReturnRevision(i64);

impl CustomerReturnRevision {
    pub const fn new(value: i64) -> Result<Self, CustomerReturnError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(CustomerReturnError::InvalidRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn next(self) -> Result<Self, CustomerReturnError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(CustomerReturnError::RevisionExhausted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerReturnStatus {
    Open,
    Planned,
    Cancelled,
}

impl CustomerReturnStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Planned => "planned",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "planned" => Some(Self::Planned),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerReturnReason {
    CustomerRequest,
    Damaged,
    RefusedDelivery,
    Recall,
    Warranty,
    Other,
}

impl CustomerReturnReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CustomerRequest => "customer_request",
            Self::Damaged => "damaged",
            Self::RefusedDelivery => "refused_delivery",
            Self::Recall => "recall",
            Self::Warranty => "warranty",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "customer_request" => Some(Self::CustomerRequest),
            "damaged" => Some(Self::Damaged),
            "refused_delivery" => Some(Self::RefusedDelivery),
            "recall" => Some(Self::Recall),
            "warranty" => Some(Self::Warranty),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerReturnCancellationReason {
    CustomerCancelled,
    DuplicateAuthorization,
    ReturnWindowExpired,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerReturnCancellationDetails {
    reason: CustomerReturnCancellationReason,
    note: Option<String>,
}

impl CustomerReturnCancellationDetails {
    pub fn new(
        reason: CustomerReturnCancellationReason,
        note: Option<String>,
    ) -> Result<Self, CustomerReturnError> {
        let note = optional_text(note, "cancellation note", MAX_CUSTOMER_RETURN_NOTE_LENGTH)?;
        if reason == CustomerReturnCancellationReason::Other && note.is_none() {
            return Err(CustomerReturnError::MissingOtherNote);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> CustomerReturnCancellationReason {
        self.reason
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

impl CustomerReturnCancellationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CustomerCancelled => "customer_cancelled",
            Self::DuplicateAuthorization => "duplicate_authorization",
            Self::ReturnWindowExpired => "return_window_expired",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "customer_cancelled" => Some(Self::CustomerCancelled),
            "duplicate_authorization" => Some(Self::DuplicateAuthorization),
            "return_window_expired" => Some(Self::ReturnWindowExpired),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerReturnLineDefinition {
    item_id: CatalogItemId,
    authorized_quantity: CustomerReturnQuantity,
    reason: CustomerReturnReason,
    note: Option<String>,
    lot: Option<String>,
    serial: Option<String>,
}

impl CustomerReturnLineDefinition {
    pub fn new(
        item_id: CatalogItemId,
        authorized_quantity: CustomerReturnQuantity,
        reason: CustomerReturnReason,
        note: Option<String>,
        lot: Option<String>,
        serial: Option<String>,
    ) -> Result<Self, CustomerReturnError> {
        let note = optional_text(note, "return note", MAX_CUSTOMER_RETURN_NOTE_LENGTH)?;
        if reason == CustomerReturnReason::Other && note.is_none() {
            return Err(CustomerReturnError::MissingOtherNote);
        }
        Ok(Self {
            item_id,
            authorized_quantity,
            reason,
            note,
            lot: optional_text(lot, "lot", MAX_CUSTOMER_RETURN_IDENTITY_LENGTH)?,
            serial: optional_text(serial, "serial", MAX_CUSTOMER_RETURN_IDENTITY_LENGTH)?,
        })
    }

    pub const fn item_id(&self) -> CatalogItemId {
        self.item_id
    }

    pub const fn authorized_quantity(&self) -> CustomerReturnQuantity {
        self.authorized_quantity
    }

    pub const fn reason(&self) -> CustomerReturnReason {
        self.reason
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub fn lot(&self) -> Option<&str> {
        self.lot.as_deref()
    }

    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewCustomerReturn {
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    number: CustomerReturnNumber,
    customer_reference: CustomerReturnReference,
    expected_at: Option<Timestamp>,
    lines: Vec<CustomerReturnLineDefinition>,
}

impl NewCustomerReturn {
    pub fn new(
        inventory_owner_id: InventoryOwnerId,
        facility_id: FacilityId,
        number: CustomerReturnNumber,
        customer_reference: CustomerReturnReference,
        expected_at: Option<Timestamp>,
        lines: Vec<CustomerReturnLineDefinition>,
    ) -> Result<Self, CustomerReturnError> {
        if lines.is_empty() {
            return Err(CustomerReturnError::MissingLines);
        }
        let identities = lines
            .iter()
            .map(|line| (line.item_id(), line.lot(), line.serial()))
            .collect::<HashSet<_>>();
        if identities.len() != lines.len() {
            return Err(CustomerReturnError::DuplicateLineIdentity);
        }
        Ok(Self {
            inventory_owner_id,
            facility_id,
            number,
            customer_reference,
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

    pub const fn number(&self) -> &CustomerReturnNumber {
        &self.number
    }

    pub const fn customer_reference(&self) -> &CustomerReturnReference {
        &self.customer_reference
    }

    pub const fn expected_at(&self) -> Option<Timestamp> {
        self.expected_at
    }

    pub fn lines(&self) -> &[CustomerReturnLineDefinition] {
        &self.lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerReturnLoadPlanDetails {
    receiving_location_id: LocationId,
    carrier: Option<String>,
    trailer_number: Option<String>,
    seal_number: Option<String>,
}

impl CustomerReturnLoadPlanDetails {
    pub fn new(
        receiving_location_id: LocationId,
        carrier: Option<String>,
        trailer_number: Option<String>,
        seal_number: Option<String>,
    ) -> Result<Self, CustomerReturnError> {
        Ok(Self {
            receiving_location_id,
            carrier: optional_text(carrier, "carrier", MAX_CUSTOMER_RETURN_REFERENCE_LENGTH)?,
            trailer_number: optional_text(
                trailer_number,
                "trailer number",
                MAX_CUSTOMER_RETURN_IDENTITY_LENGTH,
            )?,
            seal_number: optional_text(
                seal_number,
                "seal number",
                MAX_CUSTOMER_RETURN_IDENTITY_LENGTH,
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

pub fn plan_customer_return(
    status: CustomerReturnStatus,
    revision: CustomerReturnRevision,
) -> Result<CustomerReturnRevision, CustomerReturnError> {
    if status != CustomerReturnStatus::Open {
        return Err(CustomerReturnError::InvalidPlanStatus);
    }
    revision.next()
}

pub fn cancel_customer_return(
    status: CustomerReturnStatus,
    revision: CustomerReturnRevision,
) -> Result<CustomerReturnRevision, CustomerReturnError> {
    if status != CustomerReturnStatus::Open {
        return Err(CustomerReturnError::InvalidCancellationStatus);
    }
    revision.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(value: i64) -> CatalogItemId {
        CatalogItemId::new(value).unwrap()
    }

    #[test]
    fn return_lines_require_positive_unique_identities() {
        let line = CustomerReturnLineDefinition::new(
            item(1),
            CustomerReturnQuantity::new(2).unwrap(),
            CustomerReturnReason::Damaged,
            None,
            Some("LOT-A".into()),
            None,
        )
        .unwrap();
        let duplicate = line.clone();
        assert!(matches!(
            NewCustomerReturn::new(
                InventoryOwnerId::new(1).unwrap(),
                FacilityId::new(1).unwrap(),
                CustomerReturnNumber::new("RMA-1").unwrap(),
                CustomerReturnReference::new("ORDER-1").unwrap(),
                None,
                vec![line, duplicate],
            ),
            Err(CustomerReturnError::DuplicateLineIdentity)
        ));
    }

    #[test]
    fn other_reasons_require_a_bounded_note() {
        assert_eq!(
            CustomerReturnLineDefinition::new(
                item(1),
                CustomerReturnQuantity::new(1).unwrap(),
                CustomerReturnReason::Other,
                None,
                None,
                None,
            ),
            Err(CustomerReturnError::MissingOtherNote)
        );
    }

    #[test]
    fn only_open_returns_can_plan_or_cancel() {
        let revision = CustomerReturnRevision::new(3).unwrap();
        assert_eq!(
            plan_customer_return(CustomerReturnStatus::Open, revision)
                .unwrap()
                .get(),
            4
        );
        assert!(plan_customer_return(CustomerReturnStatus::Planned, revision).is_err());
        assert!(cancel_customer_return(CustomerReturnStatus::Cancelled, revision).is_err());
    }
}
