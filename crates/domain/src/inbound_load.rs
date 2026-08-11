use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CatalogItemId, FacilityId, InventoryOwnerId, LocationId, Timestamp};

pub const MAX_INBOUND_LOAD_REFERENCE_LENGTH: usize = 200;
pub const MAX_INBOUND_LOAD_TEXT_LENGTH: usize = 200;
pub const MAX_INBOUND_LOAD_IDENTITY_LENGTH: usize = 200;
pub const MAX_INBOUND_LOAD_SCAN_VALUE_LENGTH: usize = 200;
pub const MAX_INBOUND_LOAD_CANCELLATION_NOTE_LENGTH: usize = 500;

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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundLoadArrivalError {
    #[error("inbound load scan must be nonblank, trimmed, control-free, and at most {MAX_INBOUND_LOAD_SCAN_VALUE_LENGTH} characters")]
    InvalidScanValue,
    #[error("inbound load must be planned or scheduled before arrival")]
    InvalidStatus,
    #[error("arrival time cannot be in the future")]
    FutureArrival,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundLoadAppointmentError {
    #[error("inbound load must be planned before an appointment can be scheduled")]
    InvalidStatus,
    #[error("inbound load appointment must be in the future")]
    AppointmentNotFuture,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundLoadCancellationError {
    #[error("inbound load cancellation note must be nonblank, trimmed, control-free, and at most {MAX_INBOUND_LOAD_CANCELLATION_NOTE_LENGTH} characters")]
    InvalidNote,
    #[error("a cancellation note is required when the reason is other")]
    MissingOtherNote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadCancellationReason {
    CarrierCancelled,
    SupplierCancelled,
    DuplicatePlan,
    WarehouseCapacity,
    Other,
}

impl InboundLoadCancellationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CarrierCancelled => "carrier_cancelled",
            Self::SupplierCancelled => "supplier_cancelled",
            Self::DuplicatePlan => "duplicate_plan",
            Self::WarehouseCapacity => "warehouse_capacity",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "carrier_cancelled" => Some(Self::CarrierCancelled),
            "supplier_cancelled" => Some(Self::SupplierCancelled),
            "duplicate_plan" => Some(Self::DuplicatePlan),
            "warehouse_capacity" => Some(Self::WarehouseCapacity),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct InboundLoadCancellationNote(String);

impl InboundLoadCancellationNote {
    pub fn new(value: impl Into<String>) -> Result<Self, InboundLoadCancellationError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_INBOUND_LOAD_CANCELLATION_NOTE_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(InboundLoadCancellationError::InvalidNote);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for InboundLoadCancellationNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundLoadCancellationDetails {
    reason: InboundLoadCancellationReason,
    note: Option<InboundLoadCancellationNote>,
}

impl InboundLoadCancellationDetails {
    pub fn new(
        reason: InboundLoadCancellationReason,
        note: Option<InboundLoadCancellationNote>,
    ) -> Result<Self, InboundLoadCancellationError> {
        if reason == InboundLoadCancellationReason::Other && note.is_none() {
            return Err(InboundLoadCancellationError::MissingOtherNote);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> InboundLoadCancellationReason {
        self.reason
    }

    pub fn note(&self) -> Option<&InboundLoadCancellationNote> {
        self.note.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundLoadUnloadingError {
    #[error("inbound load must be arrived before unloading begins")]
    InvalidStatus,
    #[error("unloading start time cannot be in the future")]
    FutureStart,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundLoadClosureError {
    #[error("inbound load must be fully received before it can be closed")]
    InvalidStatus,
    #[error("load closure time cannot be in the future")]
    FutureClosure,
}

pub fn validate_inbound_load_unloading_start(
    started_at: Timestamp,
    current_time: Timestamp,
) -> Result<(), InboundLoadUnloadingError> {
    if started_at > current_time {
        Err(InboundLoadUnloadingError::FutureStart)
    } else {
        Ok(())
    }
}

pub fn validate_inbound_load_appointment(
    scheduled_for: Timestamp,
    current_time: Timestamp,
) -> Result<(), InboundLoadAppointmentError> {
    if scheduled_for <= current_time {
        Err(InboundLoadAppointmentError::AppointmentNotFuture)
    } else {
        Ok(())
    }
}

pub fn validate_inbound_load_closure(
    closed_at: Timestamp,
    current_time: Timestamp,
) -> Result<(), InboundLoadClosureError> {
    if closed_at > current_time {
        Err(InboundLoadClosureError::FutureClosure)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct InboundLoadScanValue(String);

impl InboundLoadScanValue {
    pub fn new(value: impl Into<String>) -> Result<Self, InboundLoadArrivalError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_INBOUND_LOAD_SCAN_VALUE_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(InboundLoadArrivalError::InvalidScanValue);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for InboundLoadScanValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadPreArrivalStatus {
    Planned,
    Scheduled,
}

pub fn validate_inbound_load_arrival(
    status: InboundLoadPreArrivalStatus,
    arrived_at: Timestamp,
    current_time: Timestamp,
) -> Result<InboundLoadPreArrivalStatus, InboundLoadArrivalError> {
    if arrived_at > current_time {
        return Err(InboundLoadArrivalError::FutureArrival);
    }
    Ok(status)
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

    #[test]
    fn arrival_requires_exact_bounded_scans_and_nonfuture_time() {
        assert!(InboundLoadScanValue::new("WB-LOAD-100").is_ok());
        assert!(InboundLoadScanValue::new(" WB-LOAD-100").is_err());
        assert!(InboundLoadScanValue::new("\n").is_err());
        let now = "2027-08-10T12:00:00Z".parse::<Timestamp>().unwrap();
        let arrived = "2027-08-10T11:59:00Z".parse::<Timestamp>().unwrap();
        assert_eq!(
            validate_inbound_load_arrival(InboundLoadPreArrivalStatus::Planned, arrived, now),
            Ok(InboundLoadPreArrivalStatus::Planned)
        );
        let future = "2027-08-10T12:00:01Z".parse::<Timestamp>().unwrap();
        assert_eq!(
            validate_inbound_load_arrival(InboundLoadPreArrivalStatus::Scheduled, future, now),
            Err(InboundLoadArrivalError::FutureArrival)
        );
        assert_eq!(
            validate_inbound_load_unloading_start(future, now),
            Err(InboundLoadUnloadingError::FutureStart)
        );
        assert_eq!(
            validate_inbound_load_closure(future, now),
            Err(InboundLoadClosureError::FutureClosure)
        );
        assert_eq!(
            validate_inbound_load_appointment(now, now),
            Err(InboundLoadAppointmentError::AppointmentNotFuture)
        );
        assert!(validate_inbound_load_appointment(future, now).is_ok());
    }

    #[test]
    fn cancellation_requires_bounded_reason_evidence() {
        let note = InboundLoadCancellationNote::new("supplier withdrew the load").unwrap();
        let details = InboundLoadCancellationDetails::new(
            InboundLoadCancellationReason::SupplierCancelled,
            Some(note),
        )
        .unwrap();
        assert_eq!(
            details.reason(),
            InboundLoadCancellationReason::SupplierCancelled
        );
        assert!(
            InboundLoadCancellationDetails::new(InboundLoadCancellationReason::Other, None,)
                .is_err()
        );
        assert!(InboundLoadCancellationNote::new(" untrimmed").is_err());
        assert!(InboundLoadCancellationNote::new("x".repeat(501)).is_err());
    }
}
