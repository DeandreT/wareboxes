use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{
    CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId, LocationId, Timestamp,
    TransferOrderLineId,
};

pub const MAX_TRANSFER_ORDER_NUMBER_LENGTH: usize = 120;
pub const MAX_TRANSFER_ORDER_CANCELLATION_NOTE_LENGTH: usize = 500;
pub const MAX_TRANSFER_ORDER_SCAN_LENGTH: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferOrderError {
    #[error("transfer order number must be nonblank, trimmed, and control-free")]
    InvalidNumber,
    #[error("transfer order number cannot exceed 120 characters")]
    NumberTooLong,
    #[error("transfer source and destination facilities must differ")]
    SameFacility,
    #[error("transfer quantity must be positive, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("a transfer order requires at least one line")]
    MissingLines,
    #[error("a transfer order cannot contain the same item more than once")]
    DuplicateItem,
    #[error("a transfer dispatch cannot use the same source balance more than once")]
    DuplicateSourceBalance,
    #[error("expected arrival cannot precede expected departure")]
    InvalidSchedule,
    #[error("transfer order revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("transfer order revision cannot advance beyond its supported range")]
    RevisionExhausted,
    #[error("only a draft transfer order can be released")]
    InvalidReleaseStatus,
    #[error("only a draft or released transfer order can be cancelled")]
    InvalidCancellationStatus,
    #[error("only a released transfer order can be dispatched")]
    InvalidDispatchStatus,
    #[error("only an in-transit transfer order can be received")]
    InvalidReceiptStatus,
    #[error("transfer execution scan must be nonblank, trimmed, control-free, and at most 255 characters")]
    InvalidScan,
    #[error(
        "transfer cancellation note must be trimmed, control-free, and at most 500 characters"
    )]
    InvalidCancellationNote,
    #[error("a note is required for the Other cancellation reason")]
    MissingCancellationNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferOrderNumber(String);

impl TransferOrderNumber {
    pub fn new(value: impl Into<String>) -> Result<Self, TransferOrderError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
            return Err(TransferOrderError::InvalidNumber);
        }
        if value.chars().count() > MAX_TRANSFER_ORDER_NUMBER_LENGTH {
            return Err(TransferOrderError::NumberTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferOrderRevision(i64);

impl TransferOrderRevision {
    pub const fn new(value: i64) -> Result<Self, TransferOrderError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(TransferOrderError::InvalidRevision { value })
        }
    }
    pub const fn get(self) -> i64 {
        self.0
    }
    pub const fn next(self) -> Result<Self, TransferOrderError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(TransferOrderError::RevisionExhausted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOrderStatus {
    Draft,
    Released,
    InTransit,
    Received,
    Cancelled,
}

impl TransferOrderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Released => "released",
            Self::InTransit => "in_transit",
            Self::Received => "received",
            Self::Cancelled => "cancelled",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "released" => Some(Self::Released),
            "in_transit" => Some(Self::InTransit),
            "received" => Some(Self::Received),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct TransferOrderScanValue(String);

impl TransferOrderScanValue {
    pub fn new(value: impl Into<String>) -> Result<Self, TransferOrderError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_TRANSFER_ORDER_SCAN_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(TransferOrderError::InvalidScan);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TransferOrderScanValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferDispatchSelection {
    transfer_order_line_id: TransferOrderLineId,
    source_inventory_balance_id: InventoryBalanceId,
    quantity: TransferOrderQuantity,
    observed_source_location_barcode: TransferOrderScanValue,
}

impl TransferDispatchSelection {
    pub const fn new(
        transfer_order_line_id: TransferOrderLineId,
        source_inventory_balance_id: InventoryBalanceId,
        quantity: TransferOrderQuantity,
        observed_source_location_barcode: TransferOrderScanValue,
    ) -> Self {
        Self {
            transfer_order_line_id,
            source_inventory_balance_id,
            quantity,
            observed_source_location_barcode,
        }
    }

    pub const fn transfer_order_line_id(&self) -> TransferOrderLineId {
        self.transfer_order_line_id
    }
    pub const fn source_inventory_balance_id(&self) -> InventoryBalanceId {
        self.source_inventory_balance_id
    }
    pub const fn quantity(&self) -> TransferOrderQuantity {
        self.quantity
    }
    pub const fn observed_source_location_barcode(&self) -> &TransferOrderScanValue {
        &self.observed_source_location_barcode
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferDispatchExecution {
    transit_location_id: LocationId,
    observed_transit_location_barcode: TransferOrderScanValue,
    selections: Vec<TransferDispatchSelection>,
}

impl TransferDispatchExecution {
    pub fn new(
        transit_location_id: LocationId,
        observed_transit_location_barcode: TransferOrderScanValue,
        selections: Vec<TransferDispatchSelection>,
    ) -> Result<Self, TransferOrderError> {
        if selections.is_empty() {
            return Err(TransferOrderError::MissingLines);
        }
        if selections
            .iter()
            .map(TransferDispatchSelection::source_inventory_balance_id)
            .collect::<HashSet<_>>()
            .len()
            != selections.len()
        {
            return Err(TransferOrderError::DuplicateSourceBalance);
        }
        Ok(Self {
            transit_location_id,
            observed_transit_location_barcode,
            selections,
        })
    }

    pub const fn transit_location_id(&self) -> LocationId {
        self.transit_location_id
    }
    pub const fn observed_transit_location_barcode(&self) -> &TransferOrderScanValue {
        &self.observed_transit_location_barcode
    }
    pub fn selections(&self) -> &[TransferDispatchSelection] {
        &self.selections
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOrderCancellationReason {
    DemandCancelled,
    DuplicateOrder,
    RouteCancelled,
    Other,
}

impl TransferOrderCancellationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DemandCancelled => "demand_cancelled",
            Self::DuplicateOrder => "duplicate_order",
            Self::RouteCancelled => "route_cancelled",
            Self::Other => "other",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "demand_cancelled" => Some(Self::DemandCancelled),
            "duplicate_order" => Some(Self::DuplicateOrder),
            "route_cancelled" => Some(Self::RouteCancelled),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct TransferOrderCancellationNote(String);

impl TransferOrderCancellationNote {
    pub fn new(value: impl Into<String>) -> Result<Self, TransferOrderError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_TRANSFER_ORDER_CANCELLATION_NOTE_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(TransferOrderError::InvalidCancellationNote);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TransferOrderCancellationNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferOrderCancellationDetails {
    reason: TransferOrderCancellationReason,
    note: Option<TransferOrderCancellationNote>,
}

impl TransferOrderCancellationDetails {
    pub fn new(
        reason: TransferOrderCancellationReason,
        note: Option<TransferOrderCancellationNote>,
    ) -> Result<Self, TransferOrderError> {
        if reason == TransferOrderCancellationReason::Other && note.is_none() {
            return Err(TransferOrderError::MissingCancellationNote);
        }
        Ok(Self { reason, note })
    }
    pub const fn reason(&self) -> TransferOrderCancellationReason {
        self.reason
    }
    pub fn note(&self) -> Option<&TransferOrderCancellationNote> {
        self.note.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferOrderQuantity(i64);

impl TransferOrderQuantity {
    pub const fn new(value: i64) -> Result<Self, TransferOrderError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(TransferOrderError::InvalidQuantity { value })
        }
    }
    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferOrderLineDefinition {
    item_id: CatalogItemId,
    requested_quantity: TransferOrderQuantity,
}

impl TransferOrderLineDefinition {
    pub const fn new(item_id: CatalogItemId, requested_quantity: TransferOrderQuantity) -> Self {
        Self {
            item_id,
            requested_quantity,
        }
    }
    pub const fn item_id(&self) -> CatalogItemId {
        self.item_id
    }
    pub const fn requested_quantity(&self) -> TransferOrderQuantity {
        self.requested_quantity
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTransferOrder {
    inventory_owner_id: InventoryOwnerId,
    source_facility_id: FacilityId,
    destination_facility_id: FacilityId,
    number: TransferOrderNumber,
    expected_departure_at: Option<Timestamp>,
    expected_arrival_at: Option<Timestamp>,
    lines: Vec<TransferOrderLineDefinition>,
}

impl NewTransferOrder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inventory_owner_id: InventoryOwnerId,
        source_facility_id: FacilityId,
        destination_facility_id: FacilityId,
        number: TransferOrderNumber,
        expected_departure_at: Option<Timestamp>,
        expected_arrival_at: Option<Timestamp>,
        lines: Vec<TransferOrderLineDefinition>,
    ) -> Result<Self, TransferOrderError> {
        if source_facility_id == destination_facility_id {
            return Err(TransferOrderError::SameFacility);
        }
        if expected_departure_at
            .zip(expected_arrival_at)
            .is_some_and(|(departure, arrival)| arrival < departure)
        {
            return Err(TransferOrderError::InvalidSchedule);
        }
        if lines.is_empty() {
            return Err(TransferOrderError::MissingLines);
        }
        if lines
            .iter()
            .map(|line| line.item_id)
            .collect::<HashSet<_>>()
            .len()
            != lines.len()
        {
            return Err(TransferOrderError::DuplicateItem);
        }
        Ok(Self {
            inventory_owner_id,
            source_facility_id,
            destination_facility_id,
            number,
            expected_departure_at,
            expected_arrival_at,
            lines,
        })
    }
    pub const fn inventory_owner_id(&self) -> InventoryOwnerId {
        self.inventory_owner_id
    }
    pub const fn source_facility_id(&self) -> FacilityId {
        self.source_facility_id
    }
    pub const fn destination_facility_id(&self) -> FacilityId {
        self.destination_facility_id
    }
    pub const fn number(&self) -> &TransferOrderNumber {
        &self.number
    }
    pub const fn expected_departure_at(&self) -> Option<&Timestamp> {
        self.expected_departure_at.as_ref()
    }
    pub const fn expected_arrival_at(&self) -> Option<&Timestamp> {
        self.expected_arrival_at.as_ref()
    }
    pub fn lines(&self) -> &[TransferOrderLineDefinition] {
        &self.lines
    }
}

pub fn release_transfer_order(
    status: TransferOrderStatus,
    revision: TransferOrderRevision,
) -> Result<TransferOrderRevision, TransferOrderError> {
    if status != TransferOrderStatus::Draft {
        return Err(TransferOrderError::InvalidReleaseStatus);
    }
    revision.next()
}

pub fn cancel_transfer_order(
    status: TransferOrderStatus,
    revision: TransferOrderRevision,
) -> Result<TransferOrderRevision, TransferOrderError> {
    if !matches!(
        status,
        TransferOrderStatus::Draft | TransferOrderStatus::Released
    ) {
        return Err(TransferOrderError::InvalidCancellationStatus);
    }
    revision.next()
}

pub fn dispatch_transfer_order(
    status: TransferOrderStatus,
    revision: TransferOrderRevision,
) -> Result<TransferOrderRevision, TransferOrderError> {
    if status != TransferOrderStatus::Released {
        return Err(TransferOrderError::InvalidDispatchStatus);
    }
    revision.next()
}

pub fn receive_transfer_order(
    status: TransferOrderStatus,
    revision: TransferOrderRevision,
) -> Result<TransferOrderRevision, TransferOrderError> {
    if status != TransferOrderStatus::InTransit {
        return Err(TransferOrderError::InvalidReceiptStatus);
    }
    revision.next()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line(item_id: i64) -> TransferOrderLineDefinition {
        TransferOrderLineDefinition::new(
            CatalogItemId::new(item_id).unwrap(),
            TransferOrderQuantity::new(4).unwrap(),
        )
    }

    #[test]
    fn transfer_requires_distinct_facilities_and_unique_lines() {
        let build = |destination, lines| {
            NewTransferOrder::new(
                InventoryOwnerId::new(2).unwrap(),
                FacilityId::new(3).unwrap(),
                FacilityId::new(destination).unwrap(),
                TransferOrderNumber::new("TO-100").unwrap(),
                None,
                None,
                lines,
            )
        };
        assert_eq!(
            build(3, vec![line(8)]),
            Err(TransferOrderError::SameFacility)
        );
        assert_eq!(
            build(4, vec![line(8), line(8)]),
            Err(TransferOrderError::DuplicateItem)
        );
        assert!(build(4, vec![line(8), line(9)]).is_ok());
    }

    #[test]
    fn lifecycle_is_revisioned_and_terminal() {
        let first = TransferOrderRevision::new(1).unwrap();
        assert_eq!(
            release_transfer_order(TransferOrderStatus::Draft, first),
            TransferOrderRevision::new(2)
        );
        assert_eq!(
            cancel_transfer_order(
                TransferOrderStatus::Released,
                TransferOrderRevision::new(2).unwrap()
            ),
            TransferOrderRevision::new(3)
        );
        assert!(cancel_transfer_order(
            TransferOrderStatus::Cancelled,
            TransferOrderRevision::new(3).unwrap()
        )
        .is_err());
        let dispatched = dispatch_transfer_order(
            TransferOrderStatus::Released,
            TransferOrderRevision::new(2).unwrap(),
        )
        .unwrap();
        assert_eq!(dispatched.get(), 3);
        assert_eq!(
            receive_transfer_order(TransferOrderStatus::InTransit, dispatched)
                .unwrap()
                .get(),
            4
        );
    }

    #[test]
    fn other_cancellation_requires_a_note() {
        assert_eq!(
            TransferOrderCancellationDetails::new(TransferOrderCancellationReason::Other, None),
            Err(TransferOrderError::MissingCancellationNote)
        );
    }
}
