use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use crate::OrderStatus;

pub const MAX_PACK_SCAN_VALUE_LENGTH: usize = 200;
pub const MAX_PACK_CONTENT_REMOVAL_NOTE_LENGTH: usize = 500;
pub const MAX_PACK_SESSION_ABANDONMENT_NOTE_LENGTH: usize = 500;
pub const MAX_CARTON_REOPEN_NOTE_LENGTH: usize = 500;

/// Lifecycle of an order's pack-station session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSessionStatus {
    Open,
    ReadyToManifest,
    Abandoned,
}

/// Operator-selected reason for abandoning an empty pack session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSessionAbandonmentReason {
    OrderCancellation,
    Repack,
    StationIssue,
    Other,
}

/// Optional bounded audit note attached to one pack-session abandonment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackSessionAbandonmentNote(String);

impl PackSessionAbandonmentNote {
    pub fn new(value: impl Into<String>) -> Result<Self, PackingError> {
        let value = value.into();
        if value.trim() != value {
            return Err(PackingError::UntrimmedAbandonmentNote);
        }
        if value.is_empty() {
            return Err(PackingError::EmptyAbandonmentNote);
        }
        if value.chars().count() > MAX_PACK_SESSION_ABANDONMENT_NOTE_LENGTH {
            return Err(PackingError::AbandonmentNoteTooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(PackingError::InvalidAbandonmentNoteCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated reason and note captured for an immutable session abandonment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackSessionAbandonmentDetails {
    reason: PackSessionAbandonmentReason,
    note: Option<PackSessionAbandonmentNote>,
}

impl PackSessionAbandonmentDetails {
    pub fn new(
        reason: PackSessionAbandonmentReason,
        note: Option<PackSessionAbandonmentNote>,
    ) -> Result<Self, PackingError> {
        if reason == PackSessionAbandonmentReason::Other && note.is_none() {
            return Err(PackingError::AbandonmentNoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> PackSessionAbandonmentReason {
        self.reason
    }

    pub fn note(&self) -> Option<&PackSessionAbandonmentNote> {
        self.note.as_ref()
    }
}

/// Lifecycle of one physical shipping carton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CartonStatus {
    Open,
    Closed,
    Voided,
}

/// Audit reason for reopening a closed carton before downstream execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CartonReopenReason {
    PackingCorrection,
    QualityIssue,
    OrderCancellation,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CartonReopenNote(String);

impl CartonReopenNote {
    pub fn new(value: impl Into<String>) -> Result<Self, PackingError> {
        let value = value.into();
        if value.trim() != value {
            return Err(PackingError::UntrimmedCartonReopenNote);
        }
        if value.is_empty() {
            return Err(PackingError::EmptyCartonReopenNote);
        }
        if value.chars().count() > MAX_CARTON_REOPEN_NOTE_LENGTH {
            return Err(PackingError::CartonReopenNoteTooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(PackingError::InvalidCartonReopenNoteCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CartonReopenDetails {
    reason: CartonReopenReason,
    note: Option<CartonReopenNote>,
}

impl CartonReopenDetails {
    pub fn new(
        reason: CartonReopenReason,
        note: Option<CartonReopenNote>,
    ) -> Result<Self, PackingError> {
        if reason == CartonReopenReason::Other && note.is_none() {
            return Err(PackingError::CartonReopenNoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> CartonReopenReason {
        self.reason
    }

    pub fn note(&self) -> Option<&CartonReopenNote> {
        self.note.as_ref()
    }
}

impl CartonStatus {
    pub const fn close(self, content_count: i64) -> Result<Self, PackingError> {
        match self {
            Self::Closed | Self::Voided => Err(PackingError::CartonNotOpen),
            Self::Open if content_count <= 0 => Err(PackingError::EmptyCarton),
            Self::Open => Ok(Self::Closed),
        }
    }

    pub const fn void(self, content_count: i64) -> Result<Self, PackingError> {
        match self {
            Self::Closed | Self::Voided => Err(PackingError::CartonNotOpen),
            Self::Open if content_count != 0 => Err(PackingError::NonemptyCarton),
            Self::Open => Ok(Self::Voided),
        }
    }

    pub const fn reopen(self, content_count: i64) -> Result<Self, PackingError> {
        match self {
            Self::Open | Self::Voided => Err(PackingError::CartonNotClosed),
            Self::Closed if content_count <= 0 => Err(PackingError::EmptyCarton),
            Self::Closed => Ok(Self::Open),
        }
    }
}

/// Operator-selected reason for returning packed stock to its picked tote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackContentRemovalReason {
    WrongCarton,
    WrongItem,
    QualityIssue,
    DamagedCarton,
    Other,
}

/// Optional bounded audit note attached to one pack reversal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackContentRemovalNote(String);

impl PackContentRemovalNote {
    pub fn new(value: impl Into<String>) -> Result<Self, PackingError> {
        let value = value.into();
        if value.trim() != value {
            return Err(PackingError::UntrimmedRemovalNote);
        }
        if value.is_empty() {
            return Err(PackingError::EmptyRemovalNote);
        }
        if value.chars().count() > MAX_PACK_CONTENT_REMOVAL_NOTE_LENGTH {
            return Err(PackingError::RemovalNoteTooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(PackingError::InvalidRemovalNoteCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated reason and note captured for an immutable pack reversal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackContentRemovalDetails {
    reason: PackContentRemovalReason,
    note: Option<PackContentRemovalNote>,
}

impl PackContentRemovalDetails {
    pub fn new(
        reason: PackContentRemovalReason,
        note: Option<PackContentRemovalNote>,
    ) -> Result<Self, PackingError> {
        if reason == PackContentRemovalReason::Other && note.is_none() {
            return Err(PackingError::RemovalNoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> PackContentRemovalReason {
        self.reason
    }

    pub fn note(&self) -> Option<&PackContentRemovalNote> {
        self.note.as_ref()
    }
}

/// Removes one active content row from an open carton.
pub const fn remove_packed_content(
    status: CartonStatus,
    active_content_count: i64,
) -> Result<i64, PackingError> {
    if !matches!(status, CartonStatus::Open) {
        return Err(PackingError::CartonNotOpen);
    }
    if active_content_count <= 0 {
        return Err(PackingError::CartonContentMissing);
    }
    Ok(active_content_count - 1)
}

/// Prevents creating another carton while the station has an unfinished carton.
pub const fn open_carton(open_carton_count: i64) -> Result<CartonStatus, PackingError> {
    if open_carton_count < 0 {
        Err(PackingError::InvalidProgress)
    } else if open_carton_count == 0 {
        Ok(CartonStatus::Open)
    } else {
        Err(PackingError::OpenCartonAlreadyExists)
    }
}

/// Positive quantity copied from one immutable picked allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PackQuantity(i64);

impl PackQuantity {
    pub const fn new(value: i64) -> Result<Self, PackingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PackingError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for PackQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Exact printable identifier supplied by a pack-station scanner.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PackScanValue(String);

impl PackScanValue {
    pub fn new(value: impl Into<String>) -> Result<Self, PackingError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PackingError::EmptyScanValue);
        }
        if value.trim() != value {
            return Err(PackingError::UntrimmedScanValue);
        }
        if value.chars().count() > MAX_PACK_SCAN_VALUE_LENGTH {
            return Err(PackingError::ScanValueTooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(PackingError::InvalidScanCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for PackScanValue {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for PackScanValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PackScanValue {
    type Err = PackingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for PackScanValue {
    type Error = PackingError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for PackScanValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Positive carton weight represented without floating-point ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WeightGrams(i64);

impl WeightGrams {
    pub const fn new(value: i64) -> Result<Self, PackingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PackingError::InvalidWeight { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for WeightGrams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Positive carton dimension represented in whole millimeters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DimensionMillimeters(i64);

impl DimensionMillimeters {
    pub const fn new(value: i64) -> Result<Self, PackingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PackingError::InvalidDimension { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for DimensionMillimeters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// A complete three-dimensional carton measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CartonDimensions {
    length_mm: DimensionMillimeters,
    width_mm: DimensionMillimeters,
    height_mm: DimensionMillimeters,
}

impl CartonDimensions {
    pub const fn new(
        length_mm: DimensionMillimeters,
        width_mm: DimensionMillimeters,
        height_mm: DimensionMillimeters,
    ) -> Self {
        Self {
            length_mm,
            width_mm,
            height_mm,
        }
    }

    pub const fn length_mm(self) -> DimensionMillimeters {
        self.length_mm
    }

    pub const fn width_mm(self) -> DimensionMillimeters {
        self.width_mm
    }

    pub const fn height_mm(self) -> DimensionMillimeters {
        self.height_mm
    }
}

/// Optional measured facts captured when closing a carton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CartonMeasurements {
    weight_grams: Option<WeightGrams>,
    dimensions: Option<CartonDimensions>,
}

impl CartonMeasurements {
    pub const fn new(
        weight_grams: Option<WeightGrams>,
        dimensions: Option<CartonDimensions>,
    ) -> Self {
        Self {
            weight_grams,
            dimensions,
        }
    }

    pub const fn weight_grams(self) -> Option<WeightGrams> {
        self.weight_grams
    }

    pub const fn dimensions(self) -> Option<CartonDimensions> {
        self.dimensions
    }
}

/// Conserved allocation and carton counts for one pack session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedPackingProgress")]
pub struct PackingProgress {
    expected_allocation_count: i64,
    packed_allocation_count: i64,
    expected_quantity: i64,
    packed_quantity: i64,
    open_carton_count: i64,
    closed_carton_count: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedPackingProgress {
    expected_allocation_count: i64,
    packed_allocation_count: i64,
    expected_quantity: i64,
    packed_quantity: i64,
    open_carton_count: i64,
    closed_carton_count: i64,
}

impl PackingProgress {
    pub const fn new(
        expected_allocation_count: i64,
        packed_allocation_count: i64,
        expected_quantity: i64,
        packed_quantity: i64,
        open_carton_count: i64,
        closed_carton_count: i64,
    ) -> Result<Self, PackingError> {
        let carton_count = match open_carton_count.checked_add(closed_carton_count) {
            Some(count) => count,
            None => return Err(PackingError::InvalidProgress),
        };
        if expected_allocation_count <= 0
            || packed_allocation_count < 0
            || packed_allocation_count > expected_allocation_count
            || expected_quantity <= 0
            || packed_quantity < 0
            || packed_quantity > expected_quantity
            || ((packed_allocation_count == 0) != (packed_quantity == 0))
            || ((packed_allocation_count == expected_allocation_count)
                != (packed_quantity == expected_quantity))
            || open_carton_count < 0
            || open_carton_count > 1
            || closed_carton_count < 0
            || (packed_allocation_count > 0 && carton_count == 0)
        {
            return Err(PackingError::InvalidProgress);
        }
        Ok(Self {
            expected_allocation_count,
            packed_allocation_count,
            expected_quantity,
            packed_quantity,
            open_carton_count,
            closed_carton_count,
        })
    }

    pub const fn expected_allocation_count(self) -> i64 {
        self.expected_allocation_count
    }

    pub const fn packed_allocation_count(self) -> i64 {
        self.packed_allocation_count
    }

    pub const fn expected_quantity(self) -> i64 {
        self.expected_quantity
    }

    pub const fn packed_quantity(self) -> i64 {
        self.packed_quantity
    }

    pub const fn open_carton_count(self) -> i64 {
        self.open_carton_count
    }

    pub const fn closed_carton_count(self) -> i64 {
        self.closed_carton_count
    }

    pub const fn status(self) -> PackSessionStatus {
        if self.packed_allocation_count == self.expected_allocation_count
            && self.packed_quantity == self.expected_quantity
            && self.open_carton_count == 0
            && self.closed_carton_count > 0
        {
            PackSessionStatus::ReadyToManifest
        } else {
            PackSessionStatus::Open
        }
    }

    pub const fn ready_to_manifest(self) -> bool {
        matches!(self.status(), PackSessionStatus::ReadyToManifest)
    }
}

impl TryFrom<UncheckedPackingProgress> for PackingProgress {
    type Error = PackingError;

    fn try_from(value: UncheckedPackingProgress) -> Result<Self, Self::Error> {
        Self::new(
            value.expected_allocation_count,
            value.packed_allocation_count,
            value.expected_quantity,
            value.packed_quantity,
            value.open_carton_count,
            value.closed_carton_count,
        )
    }
}

/// Starts a pack session for an order handed off by the completed pick workflow.
pub const fn begin_packing(status: OrderStatus) -> Result<OrderStatus, PackingError> {
    match status {
        OrderStatus::AwaitingPacking => Ok(OrderStatus::Packing),
        _ => Err(PackingError::OrderNotAwaitingPacking { status }),
    }
}

/// Guards carton and content mutations after a session is open.
pub const fn continue_packing(status: OrderStatus) -> Result<OrderStatus, PackingError> {
    match status {
        OrderStatus::Packing => Ok(OrderStatus::Packing),
        _ => Err(PackingError::OrderNotPacking { status }),
    }
}

/// Returns an empty, open pack session to awaiting-packing for recovery work.
pub const fn abandon_empty_packing(
    status: OrderStatus,
    progress: PackingProgress,
) -> Result<OrderStatus, PackingError> {
    if !matches!(status, OrderStatus::Packing) {
        return Err(PackingError::OrderNotPacking { status });
    }
    if progress.packed_allocation_count() != 0
        || progress.packed_quantity() != 0
        || progress.open_carton_count() != 0
        || progress.closed_carton_count() != 0
    {
        return Err(PackingError::SessionNotEmpty);
    }
    Ok(OrderStatus::AwaitingPacking)
}

/// Reopens one closed carton and returns a completed session to active packing.
pub const fn reopen_carton(
    order_status: OrderStatus,
    session_status: PackSessionStatus,
    progress: PackingProgress,
    carton_status: CartonStatus,
    content_count: i64,
) -> Result<(OrderStatus, PackingProgress), PackingError> {
    if !matches!(
        (order_status, session_status),
        (OrderStatus::Packing, PackSessionStatus::Open)
            | (
                OrderStatus::AwaitingShipment,
                PackSessionStatus::ReadyToManifest
            )
    ) {
        return Err(PackingError::CartonReopenStateMismatch);
    }
    if let Err(error) = carton_status.reopen(content_count) {
        return Err(error);
    }
    if progress.open_carton_count() != 0 || progress.closed_carton_count() <= 0 {
        return Err(PackingError::CartonReopenProgressMismatch);
    }
    let next = match PackingProgress::new(
        progress.expected_allocation_count(),
        progress.packed_allocation_count(),
        progress.expected_quantity(),
        progress.packed_quantity(),
        1,
        progress.closed_carton_count() - 1,
    ) {
        Ok(progress) => progress,
        Err(_) => return Err(PackingError::CartonReopenProgressMismatch),
    };
    Ok((OrderStatus::Packing, next))
}

/// Completes packing only after every picked allocation is in a closed carton.
pub const fn complete_packing(
    status: OrderStatus,
    progress: PackingProgress,
) -> Result<OrderStatus, PackingError> {
    if !matches!(status, OrderStatus::Packing) {
        return Err(PackingError::OrderNotPacking { status });
    }
    if !progress.ready_to_manifest() {
        return Err(PackingError::NotReadyToManifest);
    }
    Ok(OrderStatus::AwaitingShipment)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PackingError {
    #[error("pack quantity must be a positive integer, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("pack scan value cannot be empty")]
    EmptyScanValue,
    #[error("pack scan value must be trimmed")]
    UntrimmedScanValue,
    #[error("pack scan value cannot exceed {MAX_PACK_SCAN_VALUE_LENGTH} characters")]
    ScanValueTooLong,
    #[error("pack scan value cannot contain control characters")]
    InvalidScanCharacter,
    #[error("carton weight must be a positive number of grams, got {value}")]
    InvalidWeight { value: i64 },
    #[error("carton dimension must be a positive number of millimeters, got {value}")]
    InvalidDimension { value: i64 },
    #[error("packing progress is inconsistent")]
    InvalidProgress,
    #[error("a pack session can have only one open carton")]
    OpenCartonAlreadyExists,
    #[error("an empty carton cannot be closed")]
    EmptyCarton,
    #[error("carton is not open")]
    CartonNotOpen,
    #[error("a nonempty carton cannot be voided")]
    NonemptyCarton,
    #[error("carton has no active packed content to remove")]
    CartonContentMissing,
    #[error("pack-content removal note cannot be empty")]
    EmptyRemovalNote,
    #[error("pack-content removal note must be trimmed")]
    UntrimmedRemovalNote,
    #[error(
        "pack-content removal note cannot exceed {MAX_PACK_CONTENT_REMOVAL_NOTE_LENGTH} characters"
    )]
    RemovalNoteTooLong,
    #[error("pack-content removal note cannot contain control characters")]
    InvalidRemovalNoteCharacter,
    #[error("pack-content removal reason Other requires a note")]
    RemovalNoteRequired,
    #[error("carton-reopen note cannot be empty")]
    EmptyCartonReopenNote,
    #[error("carton-reopen note must be trimmed")]
    UntrimmedCartonReopenNote,
    #[error("carton-reopen note cannot exceed {MAX_CARTON_REOPEN_NOTE_LENGTH} characters")]
    CartonReopenNoteTooLong,
    #[error("carton-reopen note cannot contain control characters")]
    InvalidCartonReopenNoteCharacter,
    #[error("carton-reopen reason Other requires a note")]
    CartonReopenNoteRequired,
    #[error("only a nonempty closed carton can be reopened")]
    CartonNotClosed,
    #[error("carton reopening does not match the current order and session states")]
    CartonReopenStateMismatch,
    #[error("carton reopening requires no other open carton and at least one closed carton")]
    CartonReopenProgressMismatch,
    #[error("pack-session abandonment note cannot be empty")]
    EmptyAbandonmentNote,
    #[error("pack-session abandonment note must be trimmed")]
    UntrimmedAbandonmentNote,
    #[error(
        "pack-session abandonment note cannot exceed {MAX_PACK_SESSION_ABANDONMENT_NOTE_LENGTH} characters"
    )]
    AbandonmentNoteTooLong,
    #[error("pack-session abandonment note cannot contain control characters")]
    InvalidAbandonmentNoteCharacter,
    #[error("pack-session abandonment reason Other requires a note")]
    AbandonmentNoteRequired,
    #[error("a pack session must have no packed content or active cartons before abandonment")]
    SessionNotEmpty,
    #[error("only an awaiting-packing order can start packing, got {status}")]
    OrderNotAwaitingPacking { status: OrderStatus },
    #[error("only a packing order can be changed at a pack station, got {status}")]
    OrderNotPacking { status: OrderStatus },
    #[error("packing is not ready to manifest")]
    NotReadyToManifest,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_progress() -> PackingProgress {
        PackingProgress::new(2, 2, 8, 8, 0, 1).unwrap()
    }

    #[test]
    fn packing_order_transitions_are_guarded() {
        assert_eq!(
            begin_packing(OrderStatus::AwaitingPacking),
            Ok(OrderStatus::Packing)
        );
        assert_eq!(
            begin_packing(OrderStatus::AwaitingShipment),
            Err(PackingError::OrderNotAwaitingPacking {
                status: OrderStatus::AwaitingShipment
            })
        );
        assert_eq!(
            complete_packing(OrderStatus::Packing, ready_progress()),
            Ok(OrderStatus::AwaitingShipment)
        );
        assert_eq!(
            abandon_empty_packing(
                OrderStatus::Packing,
                PackingProgress::new(2, 0, 8, 0, 0, 0).unwrap()
            ),
            Ok(OrderStatus::AwaitingPacking)
        );
        assert_eq!(
            abandon_empty_packing(OrderStatus::Packing, ready_progress()),
            Err(PackingError::SessionNotEmpty)
        );
        assert_eq!(
            complete_packing(
                OrderStatus::Packing,
                PackingProgress::new(2, 2, 8, 8, 1, 0).unwrap()
            ),
            Err(PackingError::NotReadyToManifest)
        );
    }

    #[test]
    fn progress_requires_conserved_allocations_and_closed_cartons() {
        assert_eq!(
            ready_progress().status(),
            PackSessionStatus::ReadyToManifest
        );
        assert_eq!(
            PackingProgress::new(2, 1, 8, 3, 1, 0).unwrap().status(),
            PackSessionStatus::Open
        );
        assert_eq!(
            PackingProgress::new(2, 3, 8, 8, 0, 1),
            Err(PackingError::InvalidProgress)
        );
        assert_eq!(
            PackingProgress::new(2, 1, 8, 3, 0, 0),
            Err(PackingError::InvalidProgress)
        );
        assert_eq!(
            PackingProgress::new(2, 1, 8, 9, 1, 0),
            Err(PackingError::InvalidProgress)
        );
        assert_eq!(
            PackingProgress::new(2, 2, 8, 7, 0, 1),
            Err(PackingError::InvalidProgress)
        );
    }

    #[test]
    fn only_one_nonempty_carton_can_be_open_or_closed() {
        assert_eq!(open_carton(0), Ok(CartonStatus::Open));
        assert_eq!(open_carton(1), Err(PackingError::OpenCartonAlreadyExists));
        assert_eq!(CartonStatus::Open.close(1), Ok(CartonStatus::Closed));
        assert_eq!(CartonStatus::Open.close(0), Err(PackingError::EmptyCarton));
        assert_eq!(CartonStatus::Open.void(0), Ok(CartonStatus::Voided));
        assert_eq!(
            CartonStatus::Open.void(1),
            Err(PackingError::NonemptyCarton)
        );
        assert_eq!(
            CartonStatus::Closed.void(0),
            Err(PackingError::CartonNotOpen)
        );
    }

    #[test]
    fn content_removal_requires_an_open_nonempty_carton() {
        assert_eq!(remove_packed_content(CartonStatus::Open, 2), Ok(1));
        assert_eq!(
            remove_packed_content(CartonStatus::Open, 0),
            Err(PackingError::CartonContentMissing)
        );
        assert_eq!(
            remove_packed_content(CartonStatus::Closed, 1),
            Err(PackingError::CartonNotOpen)
        );
    }

    #[test]
    fn content_removal_details_require_a_bounded_other_note() {
        assert!(
            PackContentRemovalDetails::new(PackContentRemovalReason::WrongCarton, None,).is_ok()
        );
        assert_eq!(
            PackContentRemovalDetails::new(PackContentRemovalReason::Other, None),
            Err(PackingError::RemovalNoteRequired)
        );
        let note = PackContentRemovalNote::new("operator verified tote").unwrap();
        assert!(
            PackContentRemovalDetails::new(PackContentRemovalReason::Other, Some(note)).is_ok()
        );
    }

    #[test]
    fn session_abandonment_details_require_a_bounded_other_note() {
        assert!(PackSessionAbandonmentDetails::new(
            PackSessionAbandonmentReason::OrderCancellation,
            None,
        )
        .is_ok());
        assert_eq!(
            PackSessionAbandonmentDetails::new(PackSessionAbandonmentReason::Other, None),
            Err(PackingError::AbandonmentNoteRequired)
        );
        let note = PackSessionAbandonmentNote::new("station scanner failed").unwrap();
        assert!(PackSessionAbandonmentDetails::new(
            PackSessionAbandonmentReason::Other,
            Some(note),
        )
        .is_ok());
    }

    #[test]
    fn closed_carton_reopening_reactivates_ready_packing_exactly() {
        let (status, progress) = reopen_carton(
            OrderStatus::AwaitingShipment,
            PackSessionStatus::ReadyToManifest,
            ready_progress(),
            CartonStatus::Closed,
            2,
        )
        .unwrap();
        assert_eq!(status, OrderStatus::Packing);
        assert_eq!(progress.open_carton_count(), 1);
        assert_eq!(progress.closed_carton_count(), 0);
        assert_eq!(progress.status(), PackSessionStatus::Open);
        assert_eq!(
            reopen_carton(
                OrderStatus::Packing,
                PackSessionStatus::Open,
                PackingProgress::new(2, 2, 8, 8, 1, 0).unwrap(),
                CartonStatus::Closed,
                2,
            ),
            Err(PackingError::CartonReopenProgressMismatch)
        );
    }

    #[test]
    fn carton_reopen_other_reason_requires_a_bounded_note() {
        assert_eq!(
            CartonReopenDetails::new(CartonReopenReason::Other, None),
            Err(PackingError::CartonReopenNoteRequired)
        );
        assert!(CartonReopenDetails::new(CartonReopenReason::PackingCorrection, None,).is_ok());
    }

    #[test]
    fn scanned_and_measured_values_are_strictly_positive_and_exact() {
        assert_eq!(PackScanValue::new("TOTE-1").unwrap().as_str(), "TOTE-1");
        assert_eq!(
            PackScanValue::new(" TOTE-1"),
            Err(PackingError::UntrimmedScanValue)
        );
        assert_eq!(
            WeightGrams::new(0),
            Err(PackingError::InvalidWeight { value: 0 })
        );
        assert_eq!(
            DimensionMillimeters::new(-1),
            Err(PackingError::InvalidDimension { value: -1 })
        );
    }

    #[test]
    fn progress_deserialization_rejects_unknown_or_invalid_fields() {
        assert!(serde_json::from_str::<PackingProgress>(
            r#"{"expected_allocation_count":2,"packed_allocation_count":2,"expected_quantity":8,"packed_quantity":8,"open_carton_count":0,"closed_carton_count":1}"#
        )
        .is_ok());
        assert!(serde_json::from_str::<PackingProgress>(
            r#"{"expected_allocation_count":2,"packed_allocation_count":3,"expected_quantity":8,"packed_quantity":8,"open_carton_count":0,"closed_carton_count":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<PackingProgress>(
            r#"{"expected_allocation_count":2,"packed_allocation_count":2,"expected_quantity":8,"packed_quantity":8,"open_carton_count":0,"closed_carton_count":1,"ready":true}"#
        )
        .is_err());
    }
}
