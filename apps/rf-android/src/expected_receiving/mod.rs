//! Pure scanner workflow for expected receiving.
//!
//! This module owns no networking or storage. It emits typed effects that the
//! application must resolve through its durable command and transport layers.

mod command;
mod recovery;
mod reducer;
mod unexpected;
mod validation;

use std::collections::HashSet;

use chrono::DateTime;
use serde::{Deserialize, Serialize};

pub use command::{
    ConfirmationMode, ContainerCapture, ExpectedReceiptCommand, ReceiptExceptionReason,
    ReceiptQuarantineReason, UnexpectedReceiptReason,
};
pub use recovery::{
    ConfirmationIntent, ConfirmationRecoverySnapshot, ConfirmationRecoverySnapshotInput,
};
pub use unexpected::{
    ReceivingCommandIntent, ReceivingCommandResult, UnexpectedReceiptCommand,
    UnexpectedReceiptIntent, UnexpectedReceiptRecoverySnapshot, UnexpectedReceiptResult,
};
pub use validation::ReceivingValidationError;

pub const CONFIRMATION_INTENT_SCHEMA_VERSION: u16 = 2;
const MAX_BARCODE_LENGTH: usize = 200;
const MAX_DIMENSION_LENGTH: usize = 200;
const MAX_NOTE_LENGTH: usize = 1_000;

macro_rules! positive_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord,
        )]
        #[serde(try_from = "i64", into = "i64")]
        pub struct $name(i64);

        impl $name {
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl TryFrom<i64> for $name {
            type Error = ReceivingValidationError;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                if value > 0 {
                    Ok(Self(value))
                } else {
                    Err(ReceivingValidationError::InvalidPositiveIdentifier)
                }
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

positive_id!(LoadId);
positive_id!(LoadLineId);
positive_id!(InventoryOwnerId);
positive_id!(FacilityId);
positive_id!(LocationId);
positive_id!(ItemId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct PositiveQuantity(i64);

impl PositiveQuantity {
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for PositiveQuantity {
    type Error = ReceivingValidationError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ReceivingValidationError::InvalidPositiveQuantity)
        }
    }
}

impl From<PositiveQuantity> for i64 {
    fn from(value: PositiveQuantity) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct NonNegativeQuantity(i64);

impl NonNegativeQuantity {
    pub fn new(value: i64) -> Result<Self, ReceivingValidationError> {
        if value >= 0 {
            Ok(Self(value))
        } else {
            Err(ReceivingValidationError::InvalidNonNegativeQuantity)
        }
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for NonNegativeQuantity {
    type Error = ReceivingValidationError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<NonNegativeQuantity> for i64 {
    fn from(value: NonNegativeQuantity) -> Self {
        value.0
    }
}

macro_rules! scanned_code {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ReceivingValidationError> {
                Ok(Self(validated_text(
                    value.into(),
                    MAX_BARCODE_LENGTH,
                    ReceivingValidationError::InvalidBarcode,
                )?))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = ReceivingValidationError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

scanned_code!(ItemBarcode);
scanned_code!(DockBarcode);
scanned_code!(LicensePlateBarcode);

/// Canonical server-side execution code scanned from an inbound load label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LoadBarcode(String);

impl LoadBarcode {
    pub fn new(value: impl Into<String>) -> Result<Self, ReceivingValidationError> {
        let canonical = value.into().trim().to_ascii_uppercase();
        let mut characters = canonical.chars();
        let Some(first) = characters.next() else {
            return Err(ReceivingValidationError::InvalidLoadBarcode);
        };
        if canonical.chars().count() > MAX_BARCODE_LENGTH
            || !first.is_ascii_uppercase() && !first.is_ascii_digit()
            || characters.any(|character| {
                !character.is_ascii_uppercase()
                    && !character.is_ascii_digit()
                    && !matches!(character, '.' | '_' | ':' | '-')
            })
        {
            return Err(ReceivingValidationError::InvalidLoadBarcode);
        }
        Ok(Self(canonical))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LoadBarcode {
    type Error = ReceivingValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<LoadBarcode> for String {
    fn from(value: LoadBarcode) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StockDimension(String);

impl StockDimension {
    pub fn new(value: impl Into<String>) -> Result<Self, ReceivingValidationError> {
        Ok(Self(validated_text(
            value.into(),
            MAX_DIMENSION_LENGTH,
            ReceivingValidationError::InvalidStockDimension,
        )?))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StockDimension {
    type Error = ReceivingValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<StockDimension> for String {
    fn from(value: StockDimension) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Expiration(String);

impl Expiration {
    pub fn new(value: impl Into<String>) -> Result<Self, ReceivingValidationError> {
        let value = validated_text(
            value.into(),
            MAX_DIMENSION_LENGTH,
            ReceivingValidationError::InvalidExpiration,
        )?;
        DateTime::parse_from_rfc3339(&value)
            .map_err(|_| ReceivingValidationError::InvalidExpiration)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Expiration {
    type Error = ReceivingValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Expiration> for String {
    fn from(value: Expiration) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExceptionNote(String);

impl ExceptionNote {
    pub fn new(value: impl Into<String>) -> Result<Self, ReceivingValidationError> {
        Ok(Self(validated_text(
            value.into(),
            MAX_NOTE_LENGTH,
            ReceivingValidationError::InvalidExceptionNote,
        )?))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExceptionNote {
    type Error = ReceivingValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ExceptionNote> for String {
    fn from(value: ExceptionNote) -> Self {
        value.0
    }
}

fn validated_text(
    value: String,
    maximum: usize,
    error: ReceivingValidationError,
) -> Result<String, ReceivingValidationError> {
    if value.trim() != value
        || value.is_empty()
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        Err(error)
    } else {
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceivingLoadStatus {
    Arrived,
    Receiving,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceivingDock {
    location_id: LocationId,
    barcode: DockBarcode,
    name: Option<String>,
}

impl ReceivingDock {
    #[must_use]
    pub fn new(location_id: LocationId, barcode: DockBarcode, name: Option<String>) -> Self {
        Self {
            location_id,
            barcode,
            name,
        }
    }

    #[must_use]
    pub const fn location_id(&self) -> LocationId {
        self.location_id
    }

    #[must_use]
    pub const fn barcode(&self) -> &DockBarcode {
        &self.barcode
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedReceiptLineInput {
    pub load_line_id: LoadLineId,
    pub item_id: ItemId,
    pub item_description: Option<String>,
    pub uom: StockDimension,
    pub item_barcodes: Vec<ItemBarcode>,
    pub expected: PositiveQuantity,
    pub received: NonNegativeQuantity,
    pub rejected: NonNegativeQuantity,
    pub missing: NonNegativeQuantity,
    pub remaining: NonNegativeQuantity,
    pub lot: Option<StockDimension>,
    pub serial: Option<StockDimension>,
    pub expiration: Option<Expiration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "ExpectedReceiptLineInput",
    into = "ExpectedReceiptLineInput"
)]
pub struct ExpectedReceiptLine {
    input: ExpectedReceiptLineInput,
}

impl TryFrom<ExpectedReceiptLineInput> for ExpectedReceiptLine {
    type Error = ReceivingValidationError;

    fn try_from(input: ExpectedReceiptLineInput) -> Result<Self, Self::Error> {
        Self::try_new(input)
    }
}

impl From<ExpectedReceiptLine> for ExpectedReceiptLineInput {
    fn from(line: ExpectedReceiptLine) -> Self {
        line.input
    }
}

impl ExpectedReceiptLine {
    pub fn try_new(input: ExpectedReceiptLineInput) -> Result<Self, ReceivingValidationError> {
        if input.item_barcodes.is_empty() {
            return Err(ReceivingValidationError::MissingItemBarcode);
        }
        let mut barcodes = HashSet::new();
        if input
            .item_barcodes
            .iter()
            .any(|barcode| !barcodes.insert(barcode.as_str().to_ascii_lowercase()))
        {
            return Err(ReceivingValidationError::DuplicateItemBarcode);
        }
        let resolved = input
            .received
            .get()
            .checked_add(input.rejected.get())
            .and_then(|quantity| quantity.checked_add(input.missing.get()))
            .and_then(|quantity| quantity.checked_add(input.remaining.get()))
            .ok_or(ReceivingValidationError::QuantityOverflow)?;
        if resolved != input.expected.get() {
            return Err(ReceivingValidationError::InvalidLineQuantities);
        }
        Ok(Self { input })
    }

    #[must_use]
    pub const fn load_line_id(&self) -> LoadLineId {
        self.input.load_line_id
    }

    #[must_use]
    pub const fn item_id(&self) -> ItemId {
        self.input.item_id
    }

    #[must_use]
    pub fn item_description(&self) -> Option<&str> {
        self.input.item_description.as_deref()
    }

    #[must_use]
    pub const fn uom(&self) -> &StockDimension {
        &self.input.uom
    }

    #[must_use]
    pub fn item_barcodes(&self) -> &[ItemBarcode] {
        &self.input.item_barcodes
    }

    #[must_use]
    pub const fn expected(&self) -> PositiveQuantity {
        self.input.expected
    }

    #[must_use]
    pub const fn received(&self) -> NonNegativeQuantity {
        self.input.received
    }

    #[must_use]
    pub const fn rejected(&self) -> NonNegativeQuantity {
        self.input.rejected
    }

    #[must_use]
    pub const fn missing(&self) -> NonNegativeQuantity {
        self.input.missing
    }

    #[must_use]
    pub const fn remaining(&self) -> NonNegativeQuantity {
        self.input.remaining
    }

    #[must_use]
    pub const fn lot(&self) -> Option<&StockDimension> {
        self.input.lot.as_ref()
    }

    #[must_use]
    pub const fn serial(&self) -> Option<&StockDimension> {
        self.input.serial.as_ref()
    }

    #[must_use]
    pub const fn expiration(&self) -> Option<&Expiration> {
        self.input.expiration.as_ref()
    }

    fn accepts(&self, barcode: &ItemBarcode) -> bool {
        self.input
            .item_barcodes
            .iter()
            .any(|expected| expected.as_str().eq_ignore_ascii_case(barcode.as_str()))
    }

    fn apply_confirmation(
        &mut self,
        result: &ConfirmationResult,
    ) -> Result<(), ReconciliationReason> {
        let prior = [
            self.received().get(),
            self.rejected().get(),
            self.missing().get(),
        ];
        let next = [
            result.cumulative_received.get(),
            result.cumulative_rejected.get(),
            result.cumulative_missing.get(),
        ];
        if next.iter().zip(prior).any(|(next, prior)| *next < prior) {
            return Err(ReconciliationReason::CumulativeQuantityRegressed);
        }

        self.input.received = result.cumulative_received;
        self.input.rejected = result.cumulative_rejected;
        self.input.missing = result.cumulative_missing;
        self.input.remaining = result.remaining;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivingSessionInput {
    pub load_id: LoadId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub reference_number: Option<String>,
    pub status: ReceivingLoadStatus,
    pub dock: ReceivingDock,
    pub lines: Vec<ExpectedReceiptLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivingSession {
    input: ReceivingSessionInput,
}

impl ReceivingSession {
    pub fn try_new(input: ReceivingSessionInput) -> Result<Self, ReceivingValidationError> {
        if input.lines.is_empty() && input.status != ReceivingLoadStatus::Received {
            return Err(ReceivingValidationError::MissingOpenLines);
        }
        let mut line_ids = HashSet::new();
        for line in &input.lines {
            if line.remaining().get() == 0 {
                return Err(ReceivingValidationError::ClosedLineInSession);
            }
            if !line_ids.insert(line.load_line_id()) {
                return Err(ReceivingValidationError::DuplicateLoadLine);
            }
        }
        Ok(Self { input })
    }

    #[must_use]
    pub const fn load_id(&self) -> LoadId {
        self.input.load_id
    }

    #[must_use]
    pub const fn inventory_owner_id(&self) -> InventoryOwnerId {
        self.input.inventory_owner_id
    }

    #[must_use]
    pub const fn facility_id(&self) -> FacilityId {
        self.input.facility_id
    }

    #[must_use]
    pub fn reference_number(&self) -> Option<&str> {
        self.input.reference_number.as_deref()
    }

    #[must_use]
    pub const fn status(&self) -> ReceivingLoadStatus {
        self.input.status
    }

    #[must_use]
    pub const fn dock(&self) -> &ReceivingDock {
        &self.input.dock
    }

    #[must_use]
    pub fn lines(&self) -> &[ExpectedReceiptLine] {
        &self.input.lines
    }

    fn line(&self, line_id: LoadLineId) -> Option<&ExpectedReceiptLine> {
        self.input
            .lines
            .iter()
            .find(|line| line.load_line_id() == line_id)
    }

    fn line_mut(&mut self, line_id: LoadLineId) -> Option<&mut ExpectedReceiptLine> {
        self.input
            .lines
            .iter_mut()
            .find(|line| line.load_line_id() == line_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmationDraftView<'a> {
    pub mode: ConfirmationMode,
    pub selected_line_id: Option<LoadLineId>,
    pub item_barcode: Option<&'a ItemBarcode>,
    pub dock_barcode: Option<&'a DockBarcode>,
    pub quantity: Option<PositiveQuantity>,
    pub container_capture: ContainerCapture,
    pub license_plate_barcode: Option<&'a LicensePlateBarcode>,
    pub exception_reason: Option<ReceiptExceptionReason>,
    pub unexpected_reason: Option<UnexpectedReceiptReason>,
    pub exception_note: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfirmationDraft {
    mode: ConfirmationMode,
    selected_line_id: Option<LoadLineId>,
    item_barcode: Option<ItemBarcode>,
    dock_barcode: Option<DockBarcode>,
    quantity: Option<PositiveQuantity>,
    container_capture: ContainerCapture,
    license_plate_barcode: Option<LicensePlateBarcode>,
    lot: Option<StockDimension>,
    serial: Option<StockDimension>,
    expiration: Option<Expiration>,
    exception_reason: Option<ReceiptExceptionReason>,
    exception_note: Option<ExceptionNote>,
    unexpected_reason: Option<UnexpectedReceiptReason>,
}

impl Default for ConfirmationDraft {
    fn default() -> Self {
        Self {
            mode: ConfirmationMode::Received,
            selected_line_id: None,
            item_barcode: None,
            dock_barcode: None,
            quantity: None,
            container_capture: ContainerCapture::Loose,
            license_plate_barcode: None,
            lot: None,
            serial: None,
            expiration: None,
            exception_reason: None,
            exception_note: None,
            unexpected_reason: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveSession {
    load_barcode: LoadBarcode,
    session: ReceivingSession,
    draft: ConfirmationDraft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoadResolutionId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConfirmationId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RefreshId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivingEffect {
    ResolveLoad {
        resolution_id: LoadResolutionId,
        barcode: LoadBarcode,
    },
    PersistConfirmation {
        confirmation_id: ConfirmationId,
        intent: Box<ReceivingCommandIntent>,
    },
    RefreshSession {
        refresh_id: RefreshId,
        load_id: LoadId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceivingActivity {
    AwaitingLoad,
    ResolvingLoad,
    LoadResolutionFailed,
    Active,
    ConfirmationPending,
    Refreshing,
    RefreshFailed,
    LoadComplete,
    ReconcileRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScannerTarget {
    LoadBarcode,
    ItemBarcode,
    DockBarcode,
    LicensePlateBarcode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    Scanner(ScannerTarget),
    Quantity,
    ExceptionReason,
    ExceptionNote,
    ConfirmAction,
    Blocked(InteractionBlock),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionBlock {
    ResolvingLoad,
    ConfirmationPending,
    Refreshing,
    RefreshFailed,
    LoadComplete,
    ReconciliationRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAccess {
    Allowed,
    Blocked(CommandAccessBlock),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAccessBlock {
    SignedOut,
    Offline,
    SavedCommandPending,
    ServerStateUnverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionBlockReason {
    Device(CommandAccessBlock),
    NoActiveSession,
    NoSelectedLine,
    ItemScanRequired,
    DockScanRequired,
    QuantityRequired,
    LicensePlateScanRequired,
    ExceptionReasonRequired,
    ExceptionNoteRequired,
    QuantityExceedsRemaining,
    WorkflowBusy,
    LoadComplete,
    ReconciliationRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionGuard {
    Allowed,
    Blocked(ActionBlockReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivingOperatorError {
    InvalidScan,
    ItemNotExpected,
    ItemMatchesMultipleLines { line_ids: Vec<LoadLineId> },
    LineNotOpen,
    ItemDoesNotMatchLine,
    WrongReceivingDock,
    InvalidQuantity,
    QuantityExceedsRemaining,
    DimensionDoesNotMatchExpected,
    LoadNotFound,
    LoadNotReady,
    ConnectionUnavailable,
    ConfirmationRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationReason {
    CommandIntegrityFailure,
    ConfirmationIdentityMismatch,
    ConfirmationDispositionMismatch,
    ConfirmationQuantityMismatch,
    CumulativeQuantityRegressed,
    CumulativeQuantityInvalid,
    RefreshAggregateMismatch,
    RefreshQuantityRegressed,
    InvalidServerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadResolutionFailure {
    NotFound,
    NotReady,
    Retryable,
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationFailure {
    Rejected,
    CommandStillPending,
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshFailure {
    Retryable,
    NotFoundOrConflict,
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmationResult {
    pub load_id: LoadId,
    pub load_line_id: LoadLineId,
    pub disposition: ConfirmationMode,
    pub quantity: PositiveQuantity,
    pub cumulative_received: NonNegativeQuantity,
    pub cumulative_rejected: NonNegativeQuantity,
    pub cumulative_missing: NonNegativeQuantity,
    pub remaining: NonNegativeQuantity,
    pub receive_completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmationSummary {
    pub load_id: LoadId,
    pub load_line_id: LoadLineId,
    pub disposition: ConfirmationMode,
    pub quantity: PositiveQuantity,
    pub cumulative_received: NonNegativeQuantity,
    pub cumulative_rejected: NonNegativeQuantity,
    pub cumulative_missing: NonNegativeQuantity,
    pub remaining: NonNegativeQuantity,
    pub receive_completed: bool,
}

impl From<ConfirmationResult> for ConfirmationSummary {
    fn from(result: ConfirmationResult) -> Self {
        Self {
            load_id: result.load_id,
            load_line_id: result.load_line_id,
            disposition: result.disposition,
            quantity: result.quantity,
            cumulative_received: result.cumulative_received,
            cumulative_rejected: result.cumulative_rejected,
            cumulative_missing: result.cumulative_missing,
            remaining: result.remaining,
            receive_completed: result.receive_completed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    AwaitingLoad,
    ResolvingLoad {
        resolution_id: LoadResolutionId,
        barcode: LoadBarcode,
    },
    LoadResolutionFailed {
        barcode: LoadBarcode,
    },
    Active(ActiveSession),
    ConfirmationPending {
        active: ActiveSession,
        confirmation_id: ConfirmationId,
        intent: ReceivingCommandIntent,
    },
    Refreshing {
        active: ActiveSession,
        refresh_id: RefreshId,
        summary: ConfirmationSummary,
    },
    RefreshFailed {
        active: ActiveSession,
        summary: ConfirmationSummary,
    },
    LoadComplete {
        summary: ConfirmationSummary,
    },
    ReconcileRequired {
        reason: ReconciliationReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivingTransition {
    Applied,
    Ignored,
    Effect(ReceivingEffect),
    Blocked(ActionBlockReason),
    ReconciliationRequired(ReconciliationReason),
}

#[derive(Debug, Clone)]
pub struct ExpectedReceivingReducer {
    state: State,
    next_correlation_id: u64,
    operator_error: Option<ReceivingOperatorError>,
    last_confirmation: Option<ConfirmationSummary>,
}

impl Default for ExpectedReceivingReducer {
    fn default() -> Self {
        Self {
            state: State::AwaitingLoad,
            next_correlation_id: 1,
            operator_error: None,
            last_confirmation: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DimensionField {
    Lot,
    Serial,
}

fn select_line(active: &mut ActiveSession, line_id: LoadLineId, barcode: Option<ItemBarcode>) {
    let Some(line) = active.session.line(line_id) else {
        return;
    };
    active.draft = ConfirmationDraft {
        selected_line_id: Some(line_id),
        item_barcode: barcode,
        lot: line.lot().cloned(),
        serial: line.serial().cloned(),
        expiration: line.expiration().cloned(),
        ..ConfirmationDraft::default()
    };
}

fn focus_for_draft(active: &ActiveSession) -> FocusTarget {
    let draft = &active.draft;
    if draft.mode != ConfirmationMode::Unexpected && draft.selected_line_id.is_none() {
        return FocusTarget::Scanner(ScannerTarget::ItemBarcode);
    }
    if draft.mode != ConfirmationMode::Missing && draft.item_barcode.is_none() {
        return FocusTarget::Scanner(ScannerTarget::ItemBarcode);
    }
    match draft.mode {
        ConfirmationMode::Received
        | ConfirmationMode::Quarantined
        | ConfirmationMode::Unexpected => {
            if draft.dock_barcode.is_none() {
                return FocusTarget::Scanner(ScannerTarget::DockBarcode);
            }
            if draft.container_capture == ContainerCapture::LicensePlate
                && draft.license_plate_barcode.is_none()
            {
                return FocusTarget::Scanner(ScannerTarget::LicensePlateBarcode);
            }
        }
        ConfirmationMode::Rejected | ConfirmationMode::Missing => {}
    }
    if draft.quantity.is_none() {
        return FocusTarget::Quantity;
    }
    if matches!(
        draft.mode,
        ConfirmationMode::Quarantined | ConfirmationMode::Rejected | ConfirmationMode::Missing
    ) {
        let Some(reason) = draft.exception_reason else {
            return FocusTarget::ExceptionReason;
        };
        if reason == ReceiptExceptionReason::Other && draft.exception_note.is_none() {
            return FocusTarget::ExceptionNote;
        }
    }
    if draft.mode == ConfirmationMode::Unexpected {
        let Some(reason) = draft.unexpected_reason else {
            return FocusTarget::ExceptionReason;
        };
        if reason == UnexpectedReceiptReason::Other && draft.exception_note.is_none() {
            return FocusTarget::ExceptionNote;
        }
    }
    FocusTarget::ConfirmAction
}

fn guard_for_draft(active: &ActiveSession) -> ActionGuard {
    let draft = &active.draft;
    if draft.mode != ConfirmationMode::Unexpected {
        let Some(line_id) = draft.selected_line_id else {
            return ActionGuard::Blocked(ActionBlockReason::NoSelectedLine);
        };
        let Some(line) = active.session.line(line_id) else {
            return ActionGuard::Blocked(ActionBlockReason::NoSelectedLine);
        };
        let Some(quantity) = draft.quantity else {
            return ActionGuard::Blocked(ActionBlockReason::QuantityRequired);
        };
        if quantity.get() > line.remaining().get() {
            return ActionGuard::Blocked(ActionBlockReason::QuantityExceedsRemaining);
        }
    } else if draft.quantity.is_none() {
        return ActionGuard::Blocked(ActionBlockReason::QuantityRequired);
    }
    match draft.mode {
        ConfirmationMode::Received
        | ConfirmationMode::Quarantined
        | ConfirmationMode::Unexpected => {
            if draft.item_barcode.is_none() {
                return ActionGuard::Blocked(ActionBlockReason::ItemScanRequired);
            }
            if draft.dock_barcode.is_none() {
                return ActionGuard::Blocked(ActionBlockReason::DockScanRequired);
            }
            if draft.container_capture == ContainerCapture::LicensePlate
                && draft.license_plate_barcode.is_none()
            {
                return ActionGuard::Blocked(ActionBlockReason::LicensePlateScanRequired);
            }
        }
        ConfirmationMode::Rejected => {
            if draft.item_barcode.is_none() {
                return ActionGuard::Blocked(ActionBlockReason::ItemScanRequired);
            }
            if draft.exception_reason.is_none() {
                return ActionGuard::Blocked(ActionBlockReason::ExceptionReasonRequired);
            }
        }
        ConfirmationMode::Missing => {
            if draft.exception_reason.is_none() {
                return ActionGuard::Blocked(ActionBlockReason::ExceptionReasonRequired);
            }
        }
    }
    if draft.mode == ConfirmationMode::Quarantined
        && draft
            .exception_reason
            .and_then(ReceiptQuarantineReason::from_exception)
            .is_none()
    {
        return ActionGuard::Blocked(ActionBlockReason::ExceptionReasonRequired);
    }
    if draft.exception_reason == Some(ReceiptExceptionReason::Other)
        && draft.exception_note.is_none()
    {
        return ActionGuard::Blocked(ActionBlockReason::ExceptionNoteRequired);
    }
    if draft.mode == ConfirmationMode::Unexpected {
        let Some(reason) = draft.unexpected_reason else {
            return ActionGuard::Blocked(ActionBlockReason::ExceptionReasonRequired);
        };
        if reason == UnexpectedReceiptReason::Other && draft.exception_note.is_none() {
            return ActionGuard::Blocked(ActionBlockReason::ExceptionNoteRequired);
        }
    }
    ActionGuard::Allowed
}

fn intent_for_draft(active: &ActiveSession) -> Option<ReceivingCommandIntent> {
    if guard_for_draft(active) != ActionGuard::Allowed {
        return None;
    }
    if active.draft.mode == ConfirmationMode::Unexpected {
        return UnexpectedReceiptIntent::capture(active)
            .map(Box::new)
            .map(ReceivingCommandIntent::Unexpected);
    }
    let draft = &active.draft;
    let load_line_id = draft.selected_line_id?;
    let quantity = draft.quantity?;
    let command = match draft.mode {
        ConfirmationMode::Received => ExpectedReceiptCommand::Received {
            item_barcode: draft.item_barcode.clone()?,
            receiving_location_barcode: draft.dock_barcode.clone()?,
            quantity,
            license_plate_barcode: draft.license_plate_barcode.clone(),
            lot: draft.lot.clone(),
            serial: draft.serial.clone(),
            expiration: draft.expiration.clone(),
        },
        ConfirmationMode::Quarantined => ExpectedReceiptCommand::Quarantined {
            item_barcode: draft.item_barcode.clone()?,
            receiving_location_barcode: draft.dock_barcode.clone()?,
            quantity,
            license_plate_barcode: draft.license_plate_barcode.clone(),
            lot: draft.lot.clone(),
            serial: draft.serial.clone(),
            expiration: draft.expiration.clone(),
            reason: ReceiptQuarantineReason::from_exception(draft.exception_reason?)?,
            note: draft.exception_note.clone(),
        },
        ConfirmationMode::Rejected => ExpectedReceiptCommand::Rejected {
            item_barcode: draft.item_barcode.clone()?,
            quantity,
            reason: draft.exception_reason?,
            note: draft.exception_note.clone(),
        },
        ConfirmationMode::Missing => ExpectedReceiptCommand::Missing {
            quantity,
            reason: draft.exception_reason?,
            note: draft.exception_note.clone(),
        },
        ConfirmationMode::Unexpected => return None,
    };
    let recovery = ConfirmationRecoverySnapshot::capture(active, load_line_id)?;
    ConfirmationIntent::try_new(recovery, command)
        .ok()
        .map(Box::new)
        .map(ReceivingCommandIntent::Expected)
}

fn validate_result_quantities(
    line: &ExpectedReceiptLine,
    result: &ConfirmationResult,
) -> Result<(), ReconciliationReason> {
    let cumulative = result
        .cumulative_received
        .get()
        .checked_add(result.cumulative_rejected.get())
        .and_then(|quantity| quantity.checked_add(result.cumulative_missing.get()))
        .and_then(|quantity| quantity.checked_add(result.remaining.get()))
        .ok_or(ReconciliationReason::CumulativeQuantityInvalid)?;
    if cumulative != line.expected().get() {
        return Err(ReconciliationReason::CumulativeQuantityInvalid);
    }

    let received_delta =
        i64::from(result.disposition == ConfirmationMode::Received) * result.quantity.get();
    let rejected_delta =
        i64::from(result.disposition == ConfirmationMode::Rejected) * result.quantity.get();
    let missing_delta =
        i64::from(result.disposition == ConfirmationMode::Missing) * result.quantity.get();
    let expected_received = line
        .received()
        .get()
        .checked_add(received_delta)
        .ok_or(ReconciliationReason::CumulativeQuantityInvalid)?;
    let expected_rejected = line
        .rejected()
        .get()
        .checked_add(rejected_delta)
        .ok_or(ReconciliationReason::CumulativeQuantityInvalid)?;
    let expected_missing = line
        .missing()
        .get()
        .checked_add(missing_delta)
        .ok_or(ReconciliationReason::CumulativeQuantityInvalid)?;
    if result.cumulative_received.get() != expected_received
        || result.cumulative_rejected.get() != expected_rejected
        || result.cumulative_missing.get() != expected_missing
    {
        return Err(ReconciliationReason::CumulativeQuantityInvalid);
    }
    if result.receive_completed && result.remaining.get() != 0 {
        return Err(ReconciliationReason::CumulativeQuantityInvalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
