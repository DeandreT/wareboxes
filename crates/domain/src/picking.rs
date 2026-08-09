use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_PICK_SCAN_VALUE_LENGTH: usize = 200;
pub const MAX_PICK_REVERSAL_NOTE_LENGTH: usize = 500;
pub const MAX_PICK_SHORTAGE_NOTE_LENGTH: usize = 500;
pub const MAX_PICK_SHORT_SHIP_NOTE_LENGTH: usize = 500;

/// Completion state of one immutable piece of directed pick work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickContentState {
    Pending,
    Completed,
    Shorted,
}

/// Supervisor reason for reversing a physical pick before packing begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickReversalReason {
    MisPick,
    WrongQuantity,
    WrongLotOrSerial,
    DamagedDuringPick,
    OrderException,
    Other,
}

impl PickReversalReason {
    pub const ALL: [Self; 6] = [
        Self::MisPick,
        Self::WrongQuantity,
        Self::WrongLotOrSerial,
        Self::DamagedDuringPick,
        Self::OrderException,
        Self::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MisPick => "mis_pick",
            Self::WrongQuantity => "wrong_quantity",
            Self::WrongLotOrSerial => "wrong_lot_or_serial",
            Self::DamagedDuringPick => "damaged_during_pick",
            Self::OrderException => "order_exception",
            Self::Other => "other",
        }
    }

    pub const fn requires_note(self) -> bool {
        matches!(self, Self::Other)
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == value)
    }
}

/// Trimmed, bounded supervisor context retained with a pick reversal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PickReversalNote(String);

impl PickReversalNote {
    pub fn new(value: impl Into<String>) -> Result<Self, PickingError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value {
            return Err(PickingError::InvalidReversalNote);
        }
        if value.chars().count() > MAX_PICK_REVERSAL_NOTE_LENGTH {
            return Err(PickingError::ReversalNoteTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PickReversalNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Validated reason and context for one pick reversal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PickReversalDetails {
    reason: PickReversalReason,
    note: Option<PickReversalNote>,
}

impl PickReversalDetails {
    pub fn new(
        reason: PickReversalReason,
        note: Option<PickReversalNote>,
    ) -> Result<Self, PickingError> {
        if reason.requires_note() && note.is_none() {
            return Err(PickingError::ReversalNoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> PickReversalReason {
        self.reason
    }

    pub fn note(&self) -> Option<&PickReversalNote> {
        self.note.as_ref()
    }
}

/// Returns the workflow state after a safe pre-packing pick reversal.
pub const fn reverse_pick_before_packing(
    order_status: crate::OrderStatus,
    has_downstream_execution: bool,
    shortage_backed: bool,
) -> Result<crate::OrderStatus, PickingError> {
    if has_downstream_execution {
        return Err(PickingError::ReversalAfterPackingStarted);
    }
    if shortage_backed {
        return Err(PickingError::ShortagePickRequiresShortageRecovery);
    }
    match order_status {
        crate::OrderStatus::Processing | crate::OrderStatus::AwaitingPacking => {
            Ok(crate::OrderStatus::Processing)
        }
        _ => Err(PickingError::OrderNotReversible {
            status: order_status,
        }),
    }
}

impl PickContentState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Shorted => "shorted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "completed" => Some(Self::Completed),
            "shorted" => Some(Self::Shorted),
            _ => None,
        }
    }

    pub const fn complete(self) -> Result<Self, PickingError> {
        match self {
            Self::Pending => Ok(Self::Completed),
            Self::Completed => Err(PickingError::ContentAlreadyCompleted),
            Self::Shorted => Err(PickingError::ContentAlreadyShorted),
        }
    }

    pub const fn short(self) -> Result<Self, PickingError> {
        match self {
            Self::Pending => Ok(Self::Shorted),
            Self::Completed => Err(PickingError::ContentAlreadyCompleted),
            Self::Shorted => Err(PickingError::ContentAlreadyShorted),
        }
    }
}

/// Physical reason an operator could not complete the directed quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageReason {
    InventoryMissing,
    InsufficientQuantity,
    DamagedInventory,
    WrongInventory,
    LotOrSerialMismatch,
    Other,
}

impl PickShortageReason {
    pub const ALL: [Self; 6] = [
        Self::InventoryMissing,
        Self::InsufficientQuantity,
        Self::DamagedInventory,
        Self::WrongInventory,
        Self::LotOrSerialMismatch,
        Self::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InventoryMissing => "inventory_missing",
            Self::InsufficientQuantity => "insufficient_quantity",
            Self::DamagedInventory => "damaged_inventory",
            Self::WrongInventory => "wrong_inventory",
            Self::LotOrSerialMismatch => "lot_or_serial_mismatch",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inventory_missing" => Some(Self::InventoryMissing),
            "insufficient_quantity" => Some(Self::InsufficientQuantity),
            "damaged_inventory" => Some(Self::DamagedInventory),
            "wrong_inventory" => Some(Self::WrongInventory),
            "lot_or_serial_mismatch" => Some(Self::LotOrSerialMismatch),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub const fn requires_note(self) -> bool {
        matches!(self, Self::Other)
    }
}

/// Lifecycle state of a reported pick shortage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageStatus {
    AwaitingInventory,
    RecoveryInProgress,
    Resolved,
}

impl PickShortageStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingInventory => "awaiting_inventory",
            Self::RecoveryInProgress => "recovery_in_progress",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "awaiting_inventory" => Some(Self::AwaitingInventory),
            "recovery_in_progress" => Some(Self::RecoveryInProgress),
            "resolved" => Some(Self::Resolved),
            _ => None,
        }
    }
}

/// Durable terminal outcome of a resolved pick-shortage exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageResolution {
    Recovered,
    ShortShip,
}

impl PickShortageResolution {
    pub const ALL: [Self; 2] = [Self::Recovered, Self::ShortShip];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recovered => "recovered",
            Self::ShortShip => "short_ship",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "recovered" => Some(Self::Recovered),
            "short_ship" => Some(Self::ShortShip),
            _ => None,
        }
    }
}

/// Business reason an authorized supervisor accepts unmet demand for shipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortShipReason {
    ClientAuthorized,
    InventoryUnavailable,
    ShipByCommitment,
    Other,
}

impl PickShortShipReason {
    pub const ALL: [Self; 4] = [
        Self::ClientAuthorized,
        Self::InventoryUnavailable,
        Self::ShipByCommitment,
        Self::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientAuthorized => "client_authorized",
            Self::InventoryUnavailable => "inventory_unavailable",
            Self::ShipByCommitment => "ship_by_commitment",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "client_authorized" => Some(Self::ClientAuthorized),
            "inventory_unavailable" => Some(Self::InventoryUnavailable),
            "ship_by_commitment" => Some(Self::ShipByCommitment),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub const fn requires_note(self) -> bool {
        matches!(self, Self::Other)
    }
}

/// Trimmed, nonblank supervisor context for a short-shipment disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PickShortShipNote(String);

impl PickShortShipNote {
    pub fn new(value: impl Into<String>) -> Result<Self, PickingError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value {
            return Err(PickingError::InvalidShortShipNote);
        }
        if value.chars().count() > MAX_PICK_SHORT_SHIP_NOTE_LENGTH {
            return Err(PickingError::ShortShipNoteTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PickShortShipNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Validated reason and context for accepting one shortage as a short shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PickShortShipDetails {
    reason: PickShortShipReason,
    note: Option<PickShortShipNote>,
}

impl PickShortShipDetails {
    pub fn new(
        reason: PickShortShipReason,
        note: Option<PickShortShipNote>,
    ) -> Result<Self, PickingError> {
        if reason.requires_note() && note.is_none() {
            return Err(PickingError::ShortShipNoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> PickShortShipReason {
        self.reason
    }

    pub fn note(&self) -> Option<&PickShortShipNote> {
        self.note.as_ref()
    }

    pub fn into_note(self) -> Option<PickShortShipNote> {
        self.note
    }
}

impl<'de> Deserialize<'de> for PickShortShipDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawDetails {
            reason: PickShortShipReason,
            note: Option<PickShortShipNote>,
        }

        let raw = RawDetails::deserialize(deserializer)?;
        Self::new(raw.reason, raw.note).map_err(D::Error::custom)
    }
}

/// Original demand, cumulative accepted shortage, and remaining executable demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ShortShipDemandQuantities {
    ordered: PickQuantity,
    accepted_short: ActualPickQuantity,
    effective: ActualPickQuantity,
}

impl ShortShipDemandQuantities {
    pub const fn new(
        ordered: PickQuantity,
        accepted_short: ActualPickQuantity,
    ) -> Result<Self, PickingError> {
        if accepted_short.get() > ordered.get() {
            return Err(PickingError::AcceptedShortExceedsDemand {
                ordered: ordered.get(),
                accepted_short: accepted_short.get(),
            });
        }
        Ok(Self {
            ordered,
            accepted_short,
            effective: ActualPickQuantity(ordered.get() - accepted_short.get()),
        })
    }

    pub const fn ordered(self) -> PickQuantity {
        self.ordered
    }

    pub const fn accepted_short(self) -> ActualPickQuantity {
        self.accepted_short
    }

    pub const fn effective(self) -> ActualPickQuantity {
        self.effective
    }
}

impl<'de> Deserialize<'de> for ShortShipDemandQuantities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawQuantities {
            ordered: PickQuantity,
            accepted_short: ActualPickQuantity,
            effective: ActualPickQuantity,
        }

        let raw = RawQuantities::deserialize(deserializer)?;
        let quantities = Self::new(raw.ordered, raw.accepted_short).map_err(D::Error::custom)?;
        if quantities.effective != raw.effective {
            return Err(D::Error::custom("short-shipment demand does not conserve"));
        }
        Ok(quantities)
    }
}

/// Terminal transition returned when unmet, non-executable demand is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickShortShipTransition {
    accepted_quantity: PickQuantity,
    status: PickShortageStatus,
    resolution: PickShortageResolution,
}

impl PickShortShipTransition {
    pub const fn accepted_quantity(self) -> PickQuantity {
        self.accepted_quantity
    }

    pub const fn status(self) -> PickShortageStatus {
        self.status
    }

    pub const fn resolution(self) -> PickShortageResolution {
        self.resolution
    }
}

/// Resolves only shortage work whose replacement attempts are already terminal.
pub fn resolve_pick_shortage_as_short_ship(
    status: PickShortageStatus,
    short_quantity: PickQuantity,
    reallocated_quantity: ActualPickQuantity,
    recovery_terminal_quantity: ActualPickQuantity,
    remaining_to_allocate_quantity: ActualPickQuantity,
) -> Result<PickShortShipTransition, PickingError> {
    if !matches!(status, PickShortageStatus::AwaitingInventory) {
        return Err(PickingError::ShortShipRequiresAwaitingInventory { status });
    }
    if remaining_to_allocate_quantity.is_zero() {
        return Err(PickingError::NoShortageQuantityToAccept);
    }
    if reallocated_quantity != recovery_terminal_quantity
        || reallocated_quantity
            .get()
            .checked_add(remaining_to_allocate_quantity.get())
            != Some(short_quantity.get())
    {
        return Err(PickingError::InconsistentShortShipRecoveryQuantities);
    }
    Ok(PickShortShipTransition {
        accepted_quantity: PickQuantity(remaining_to_allocate_quantity.get()),
        status: PickShortageStatus::Resolved,
        resolution: PickShortageResolution::ShortShip,
    })
}

/// Positive optimistic revision of one pick-shortage exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PickShortageRevision(i64);

impl PickShortageRevision {
    pub const fn new(value: i64) -> Result<Self, PickingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PickingError::InvalidShortageRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl<'de> Deserialize<'de> for PickShortageRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Actual quantity physically found by the picker, including zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ActualPickQuantity(i64);

impl ActualPickQuantity {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: i64) -> Result<Self, PickingError> {
        if value >= 0 {
            Ok(Self(value))
        } else {
            Err(PickingError::InvalidActualQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl From<PickQuantity> for ActualPickQuantity {
    fn from(value: PickQuantity) -> Self {
        Self(value.get())
    }
}

impl<'de> Deserialize<'de> for ActualPickQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Conserved planned, picked, and short quantities for one exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PickShortageQuantities {
    planned: PickQuantity,
    picked: ActualPickQuantity,
    short: PickQuantity,
}

impl PickShortageQuantities {
    pub const fn new(
        planned: PickQuantity,
        picked: ActualPickQuantity,
    ) -> Result<Self, PickingError> {
        if picked.get() >= planned.get() {
            return Err(PickingError::PickIsNotShort {
                planned: planned.get(),
                picked: picked.get(),
            });
        }
        let short_value = planned.get() - picked.get();
        Ok(Self {
            planned,
            picked,
            short: PickQuantity(short_value),
        })
    }

    pub const fn planned(self) -> PickQuantity {
        self.planned
    }

    pub const fn picked(self) -> ActualPickQuantity {
        self.picked
    }

    pub const fn short(self) -> PickQuantity {
        self.short
    }
}

impl<'de> Deserialize<'de> for PickShortageQuantities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawQuantities {
            planned: PickQuantity,
            picked: ActualPickQuantity,
            short: PickQuantity,
        }

        let raw = RawQuantities::deserialize(deserializer)?;
        let quantities = Self::new(raw.planned, raw.picked).map_err(D::Error::custom)?;
        if quantities.short != raw.short {
            return Err(D::Error::custom("pick shortage quantity does not conserve"));
        }
        Ok(quantities)
    }
}

/// Trimmed, nonblank operator context for a pick shortage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PickShortageNote(String);

impl PickShortageNote {
    pub fn new(value: impl Into<String>) -> Result<Self, PickingError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value {
            return Err(PickingError::InvalidShortageNote);
        }
        if value.chars().count() > MAX_PICK_SHORTAGE_NOTE_LENGTH {
            return Err(PickingError::ShortageNoteTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PickShortageNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Validated operator reason and optional context for a pick shortage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PickShortageDetails {
    reason: PickShortageReason,
    note: Option<PickShortageNote>,
}

impl PickShortageDetails {
    pub fn new(
        reason: PickShortageReason,
        note: Option<PickShortageNote>,
    ) -> Result<Self, PickingError> {
        if reason.requires_note() && note.is_none() {
            return Err(PickingError::ShortageNoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> PickShortageReason {
        self.reason
    }

    pub fn note(&self) -> Option<&PickShortageNote> {
        self.note.as_ref()
    }
}

impl<'de> Deserialize<'de> for PickShortageDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawDetails {
            reason: PickShortageReason,
            note: Option<PickShortageNote>,
        }

        let raw = RawDetails::deserialize(deserializer)?;
        Self::new(raw.reason, raw.note).map_err(D::Error::custom)
    }
}

/// Operator reason for returning an active pick claim to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SourceBlocked,
    InventoryDiscrepancy,
    SafetyIssue,
    Other,
}

/// Positive quantity for planned and confirmed pick work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PickQuantity(i64);

impl PickQuantity {
    pub const fn new(value: i64) -> Result<Self, PickingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PickingError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for PickQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Exact scannable value supplied by an RF operator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PickScanValue(String);

impl PickScanValue {
    pub fn new(value: impl Into<String>) -> Result<Self, PickingError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PickingError::EmptyScanValue);
        }
        if value.trim() != value {
            return Err(PickingError::UntrimmedScanValue);
        }
        if value.chars().count() > MAX_PICK_SCAN_VALUE_LENGTH {
            return Err(PickingError::ScanValueTooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(PickingError::InvalidScanCharacter);
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

impl AsRef<str> for PickScanValue {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for PickScanValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PickScanValue {
    type Err = PickingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for PickScanValue {
    type Error = PickingError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for PickScanValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PickingError {
    #[error("pick quantity must be a positive integer, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("pick scan value cannot be empty")]
    EmptyScanValue,
    #[error("pick scan value must be trimmed")]
    UntrimmedScanValue,
    #[error("pick scan value cannot exceed {MAX_PICK_SCAN_VALUE_LENGTH} characters")]
    ScanValueTooLong,
    #[error("pick scan value cannot contain control characters")]
    InvalidScanCharacter,
    #[error("pick content is already completed")]
    ContentAlreadyCompleted,
    #[error("pick content is already shorted")]
    ContentAlreadyShorted,
    #[error("pick reversal note must be trimmed and nonblank")]
    InvalidReversalNote,
    #[error("pick reversal note cannot exceed {MAX_PICK_REVERSAL_NOTE_LENGTH} characters")]
    ReversalNoteTooLong,
    #[error("pick reversal reason other requires a note")]
    ReversalNoteRequired,
    #[error("order {status} cannot reverse a pick")]
    OrderNotReversible { status: crate::OrderStatus },
    #[error("pick cannot be reversed after packing execution begins")]
    ReversalAfterPackingStarted,
    #[error("shortage-backed picks must be resolved through shortage recovery")]
    ShortagePickRequiresShortageRecovery,
    #[error("actual pick quantity cannot be negative, got {value}")]
    InvalidActualQuantity { value: i64 },
    #[error("pick shortage revision must be positive, got {value}")]
    InvalidShortageRevision { value: i64 },
    #[error("actual pick quantity {picked} must be less than planned quantity {planned}")]
    PickIsNotShort { planned: i64, picked: i64 },
    #[error("pick shortage note must be trimmed and nonblank")]
    InvalidShortageNote,
    #[error("pick shortage note cannot exceed {MAX_PICK_SHORTAGE_NOTE_LENGTH} characters")]
    ShortageNoteTooLong,
    #[error("pick shortage reason other requires a note")]
    ShortageNoteRequired,
    #[error("short-shipment note must be trimmed and nonblank")]
    InvalidShortShipNote,
    #[error("short-shipment note cannot exceed {MAX_PICK_SHORT_SHIP_NOTE_LENGTH} characters")]
    ShortShipNoteTooLong,
    #[error("short-shipment reason other requires a note")]
    ShortShipNoteRequired,
    #[error(
        "only an awaiting-inventory shortage can be accepted for short shipment, got {status:?}"
    )]
    ShortShipRequiresAwaitingInventory { status: PickShortageStatus },
    #[error("pick shortage has no remaining quantity to accept")]
    NoShortageQuantityToAccept,
    #[error("pick-shortage recovery quantities are inconsistent for short shipment")]
    InconsistentShortShipRecoveryQuantities,
    #[error("accepted short quantity {accepted_short} exceeds ordered quantity {ordered}")]
    AcceptedShortExceedsDemand { ordered: i64, accepted_short: i64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_are_positive_and_strict_when_deserialized() {
        assert_eq!(PickQuantity::new(4).map(PickQuantity::get), Ok(4));
        assert_eq!(
            PickQuantity::new(0),
            Err(PickingError::InvalidQuantity { value: 0 })
        );
    }

    #[test]
    fn scans_are_exact_printable_values() {
        let scan = PickScanValue::new("A-01-01").unwrap();
        assert_eq!(scan.as_str(), "A-01-01");

        assert_eq!(PickScanValue::new(""), Err(PickingError::EmptyScanValue));
        assert_eq!(
            PickScanValue::new(" A-01"),
            Err(PickingError::UntrimmedScanValue)
        );
        assert_eq!(
            PickScanValue::new("A\n01"),
            Err(PickingError::InvalidScanCharacter)
        );
        assert_eq!(
            PickScanValue::new("x".repeat(MAX_PICK_SCAN_VALUE_LENGTH + 1)),
            Err(PickingError::ScanValueTooLong)
        );
    }

    #[test]
    fn pick_content_can_only_complete_once() {
        assert_eq!(
            PickContentState::Pending.complete(),
            Ok(PickContentState::Completed)
        );
        assert_eq!(
            PickContentState::Completed.complete(),
            Err(PickingError::ContentAlreadyCompleted)
        );
    }

    #[test]
    fn pick_reversal_requires_prepacking_work_and_valid_context() {
        assert_eq!(
            reverse_pick_before_packing(crate::OrderStatus::AwaitingPacking, false, false),
            Ok(crate::OrderStatus::Processing)
        );
        assert_eq!(
            reverse_pick_before_packing(crate::OrderStatus::Processing, true, false),
            Err(PickingError::ReversalAfterPackingStarted)
        );
        assert_eq!(
            reverse_pick_before_packing(crate::OrderStatus::Processing, false, true),
            Err(PickingError::ShortagePickRequiresShortageRecovery)
        );
        assert_eq!(
            PickReversalDetails::new(PickReversalReason::Other, None),
            Err(PickingError::ReversalNoteRequired)
        );
        assert_eq!(
            PickReversalNote::new(" padded "),
            Err(PickingError::InvalidReversalNote)
        );
    }

    #[test]
    fn shorting_is_a_terminal_content_transition() {
        assert_eq!(
            PickContentState::Pending.short(),
            Ok(PickContentState::Shorted)
        );
        assert_eq!(
            PickContentState::Shorted.complete(),
            Err(PickingError::ContentAlreadyShorted)
        );
        assert_eq!(
            PickContentState::parse("shorted"),
            Some(PickContentState::Shorted)
        );
        assert_eq!(PickContentState::Shorted.as_str(), "shorted");
    }

    #[test]
    fn shortage_quantities_are_nonnegative_short_and_conserved() {
        let quantities = PickShortageQuantities::new(
            PickQuantity::new(7).unwrap(),
            ActualPickQuantity::new(2).unwrap(),
        )
        .unwrap();
        assert_eq!(quantities.planned().get(), 7);
        assert_eq!(quantities.picked().get(), 2);
        assert_eq!(quantities.short().get(), 5);
        assert_eq!(
            ActualPickQuantity::new(-1),
            Err(PickingError::InvalidActualQuantity { value: -1 })
        );
        assert_eq!(
            PickShortageQuantities::new(
                PickQuantity::new(7).unwrap(),
                ActualPickQuantity::new(7).unwrap(),
            ),
            Err(PickingError::PickIsNotShort {
                planned: 7,
                picked: 7,
            })
        );
        assert!(serde_json::from_str::<PickShortageQuantities>(
            r#"{"planned":7,"picked":2,"short":4}"#
        )
        .is_err());
    }

    #[test]
    fn shortage_reasons_and_statuses_have_stable_wire_values() {
        for reason in PickShortageReason::ALL {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(json, format!("\"{}\"", reason.as_str()));
            assert_eq!(PickShortageReason::parse(reason.as_str()), Some(reason));
        }
        assert_eq!(
            serde_json::to_string(&PickShortageStatus::RecoveryInProgress).unwrap(),
            r#""recovery_in_progress""#
        );
        assert_eq!(
            PickShortageStatus::parse("awaiting_inventory"),
            Some(PickShortageStatus::AwaitingInventory)
        );
    }

    #[test]
    fn other_shortage_reason_requires_bounded_trimmed_context() {
        assert_eq!(
            PickShortageDetails::new(PickShortageReason::Other, None),
            Err(PickingError::ShortageNoteRequired)
        );
        assert_eq!(
            PickShortageNote::new(" padded "),
            Err(PickingError::InvalidShortageNote)
        );
        assert_eq!(
            PickShortageNote::new("x".repeat(MAX_PICK_SHORTAGE_NOTE_LENGTH + 1)),
            Err(PickingError::ShortageNoteTooLong)
        );

        let details = PickShortageDetails::new(
            PickShortageReason::Other,
            Some(PickShortageNote::new("Cycle count requested").unwrap()),
        )
        .unwrap();
        assert_eq!(details.reason(), PickShortageReason::Other);
        assert_eq!(
            details.note().map(PickShortageNote::as_str),
            Some("Cycle count requested")
        );
    }

    #[test]
    fn short_ship_values_have_stable_wire_names_and_validated_context() {
        for resolution in PickShortageResolution::ALL {
            assert_eq!(
                PickShortageResolution::parse(resolution.as_str()),
                Some(resolution)
            );
        }
        for reason in PickShortShipReason::ALL {
            assert_eq!(PickShortShipReason::parse(reason.as_str()), Some(reason));
        }
        assert_eq!(
            serde_json::to_string(&PickShortageResolution::ShortShip).unwrap(),
            r#""short_ship""#
        );
        assert_eq!(
            PickShortShipDetails::new(PickShortShipReason::Other, None),
            Err(PickingError::ShortShipNoteRequired)
        );
        assert_eq!(
            PickShortShipNote::new(" padded "),
            Err(PickingError::InvalidShortShipNote)
        );
        assert_eq!(
            PickShortShipNote::new("x".repeat(MAX_PICK_SHORT_SHIP_NOTE_LENGTH + 1)),
            Err(PickingError::ShortShipNoteTooLong)
        );
    }

    #[test]
    fn short_ship_transition_accepts_only_terminal_unmet_recovery_quantity() {
        let transition = resolve_pick_shortage_as_short_ship(
            PickShortageStatus::AwaitingInventory,
            PickQuantity::new(5).unwrap(),
            ActualPickQuantity::new(2).unwrap(),
            ActualPickQuantity::new(2).unwrap(),
            ActualPickQuantity::new(3).unwrap(),
        )
        .unwrap();
        assert_eq!(transition.accepted_quantity().get(), 3);
        assert_eq!(transition.status(), PickShortageStatus::Resolved);
        assert_eq!(transition.resolution(), PickShortageResolution::ShortShip);

        assert_eq!(
            resolve_pick_shortage_as_short_ship(
                PickShortageStatus::RecoveryInProgress,
                PickQuantity::new(5).unwrap(),
                ActualPickQuantity::new(3).unwrap(),
                ActualPickQuantity::new(1).unwrap(),
                ActualPickQuantity::new(2).unwrap(),
            ),
            Err(PickingError::ShortShipRequiresAwaitingInventory {
                status: PickShortageStatus::RecoveryInProgress,
            })
        );
        assert_eq!(
            resolve_pick_shortage_as_short_ship(
                PickShortageStatus::AwaitingInventory,
                PickQuantity::new(5).unwrap(),
                ActualPickQuantity::new(2).unwrap(),
                ActualPickQuantity::new(1).unwrap(),
                ActualPickQuantity::new(3).unwrap(),
            ),
            Err(PickingError::InconsistentShortShipRecoveryQuantities)
        );
    }

    #[test]
    fn effective_demand_conserves_ordered_and_accepted_quantities() {
        let quantities = ShortShipDemandQuantities::new(
            PickQuantity::new(12).unwrap(),
            ActualPickQuantity::new(3).unwrap(),
        )
        .unwrap();
        assert_eq!(quantities.ordered().get(), 12);
        assert_eq!(quantities.accepted_short().get(), 3);
        assert_eq!(quantities.effective().get(), 9);
        assert_eq!(
            ShortShipDemandQuantities::new(
                PickQuantity::new(2).unwrap(),
                ActualPickQuantity::new(3).unwrap(),
            ),
            Err(PickingError::AcceptedShortExceedsDemand {
                ordered: 2,
                accepted_short: 3,
            })
        );
        assert!(serde_json::from_str::<ShortShipDemandQuantities>(
            r#"{"ordered":12,"accepted_short":3,"effective":8}"#
        )
        .is_err());
    }
}
