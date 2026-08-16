use serde::{Deserialize, Serialize};

use super::*;

const UNEXPECTED_RECEIPT_INTENT_SCHEMA_VERSION: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnexpectedReceiptCommand {
    pub item_barcode: ItemBarcode,
    pub receiving_location_barcode: DockBarcode,
    pub quantity: PositiveQuantity,
    pub license_plate_barcode: Option<LicensePlateBarcode>,
    pub lot: Option<StockDimension>,
    pub serial: Option<StockDimension>,
    pub expiration: Option<Expiration>,
    pub reason: UnexpectedReceiptReason,
    pub note: Option<ExceptionNote>,
    pub receipt_policy: ReceiptPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnexpectedReceiptRecoverySnapshot {
    pub load_barcode: LoadBarcode,
    pub load_id: LoadId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub reference_number: Option<String>,
    pub status: ReceivingLoadStatus,
    pub expected_seal: Option<SealBarcode>,
    pub dock: ReceivingDock,
    pub receipt_policy: ReceiptPolicy,
    pub lines: Vec<ExpectedReceiptLine>,
}

impl UnexpectedReceiptRecoverySnapshot {
    fn capture(active: &ActiveSession) -> Self {
        Self {
            load_barcode: active.load_barcode.clone(),
            load_id: active.session.load_id(),
            inventory_owner_id: active.session.inventory_owner_id(),
            facility_id: active.session.facility_id(),
            reference_number: active.session.reference_number().map(str::to_owned),
            status: active.session.status(),
            expected_seal: active.session.expected_seal().cloned(),
            dock: active.session.dock().clone(),
            receipt_policy: active.session.receipt_policy().clone(),
            lines: active.session.lines().to_vec(),
        }
    }

    fn restore_active(&self, command: &UnexpectedReceiptCommand) -> Option<ActiveSession> {
        let session = ReceivingSession::try_new(ReceivingSessionInput {
            load_id: self.load_id,
            inventory_owner_id: self.inventory_owner_id,
            facility_id: self.facility_id,
            reference_number: self.reference_number.clone(),
            status: self.status,
            expected_seal: self.expected_seal.clone(),
            dock: self.dock.clone(),
            receipt_policy: self.receipt_policy.clone(),
            lines: self.lines.clone(),
        })
        .ok()?;
        Some(ActiveSession {
            load_barcode: self.load_barcode.clone(),
            session,
            draft: ConfirmationDraft {
                mode: ConfirmationMode::Unexpected,
                selected_line_id: None,
                item_barcode: Some(command.item_barcode.clone()),
                dock_barcode: Some(command.receiving_location_barcode.clone()),
                quantity: Some(command.quantity),
                container_capture: if command.license_plate_barcode.is_some() {
                    ContainerCapture::LicensePlate
                } else {
                    ContainerCapture::Loose
                },
                license_plate_barcode: command.license_plate_barcode.clone(),
                lot: command.lot.clone(),
                serial: command.serial.clone(),
                expiration: command.expiration.clone(),
                exception_reason: None,
                exception_note: command.note.clone(),
                unexpected_reason: Some(command.reason),
            },
            unloading: UnloadingDraft::default(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnexpectedReceiptIntent {
    pub schema_version: u16,
    pub load_id: LoadId,
    pub command: UnexpectedReceiptCommand,
    pub recovery: Box<UnexpectedReceiptRecoverySnapshot>,
}

impl UnexpectedReceiptIntent {
    pub(super) fn capture(active: &ActiveSession) -> Option<Self> {
        let draft = &active.draft;
        let command = UnexpectedReceiptCommand {
            item_barcode: draft.item_barcode.clone()?,
            receiving_location_barcode: draft.dock_barcode.clone()?,
            quantity: draft.quantity?,
            license_plate_barcode: draft.license_plate_barcode.clone(),
            lot: draft.lot.clone(),
            serial: draft.serial.clone(),
            expiration: draft.expiration.clone(),
            reason: draft.unexpected_reason?,
            note: draft.exception_note.clone(),
            receipt_policy: active.session.receipt_policy().clone(),
        };
        let intent = Self {
            schema_version: UNEXPECTED_RECEIPT_INTENT_SCHEMA_VERSION,
            load_id: active.session.load_id(),
            command,
            recovery: Box::new(UnexpectedReceiptRecoverySnapshot::capture(active)),
        };
        intent.is_current_and_valid().then_some(intent)
    }

    #[must_use]
    pub fn is_current_and_valid(&self) -> bool {
        self.schema_version == UNEXPECTED_RECEIPT_INTENT_SCHEMA_VERSION
            && self.load_id == self.recovery.load_id
            && self.command.receiving_location_barcode == *self.recovery.dock.barcode()
            && self.command.receipt_policy == self.recovery.receipt_policy
            && (self.command.reason != UnexpectedReceiptReason::Other
                || self.command.note.is_some())
            && ReceivingSession::try_new(ReceivingSessionInput {
                load_id: self.recovery.load_id,
                inventory_owner_id: self.recovery.inventory_owner_id,
                facility_id: self.recovery.facility_id,
                reference_number: self.recovery.reference_number.clone(),
                status: self.recovery.status,
                expected_seal: self.recovery.expected_seal.clone(),
                dock: self.recovery.dock.clone(),
                receipt_policy: self.recovery.receipt_policy.clone(),
                lines: self.recovery.lines.clone(),
            })
            .is_ok()
    }

    pub(super) fn restore_active(&self) -> Option<ActiveSession> {
        self.is_current_and_valid()
            .then(|| self.recovery.restore_active(&self.command))
            .flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "intent", rename_all = "snake_case")]
pub enum ReceivingCommandIntent {
    Unloading(Box<UnloadingStartIntent>),
    Expected(Box<ConfirmationIntent>),
    Unexpected(Box<UnexpectedReceiptIntent>),
}

impl ReceivingCommandIntent {
    #[must_use]
    pub fn is_current_and_valid(&self) -> bool {
        match self {
            Self::Unloading(intent) => intent.is_current_and_valid(),
            Self::Expected(intent) => intent.is_current_and_valid(),
            Self::Unexpected(intent) => intent.is_current_and_valid(),
        }
    }

    pub(super) fn restore_active(&self) -> Option<ActiveSession> {
        match self {
            Self::Unloading(intent) => intent.restore_active(),
            Self::Expected(intent) => intent.restore_active(),
            Self::Unexpected(intent) => intent.restore_active(),
        }
    }

    pub fn canonical_payload(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::Unloading(intent) => serde_json::to_vec(intent),
            Self::Expected(intent) => intent.canonical_payload(),
            Self::Unexpected(intent) => serde_json::to_vec(intent),
        }
    }

    #[must_use]
    pub const fn as_expected(&self) -> Option<&ConfirmationIntent> {
        match self {
            Self::Expected(intent) => Some(intent),
            Self::Unloading(_) | Self::Unexpected(_) => None,
        }
    }

    #[must_use]
    pub const fn is_unloading(&self) -> bool {
        matches!(self, Self::Unloading(_))
    }
}

impl From<ConfirmationIntent> for ReceivingCommandIntent {
    fn from(intent: ConfirmationIntent) -> Self {
        Self::Expected(Box::new(intent))
    }
}

impl PartialEq<ConfirmationIntent> for ReceivingCommandIntent {
    fn eq(&self, other: &ConfirmationIntent) -> bool {
        matches!(self, Self::Expected(intent) if intent.as_ref() == other)
    }
}

impl PartialEq<ReceivingCommandIntent> for ConfirmationIntent {
    fn eq(&self, other: &ReceivingCommandIntent) -> bool {
        other == self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnexpectedReceiptResult {
    pub unexpected_receipt_id: i64,
    pub load_id: LoadId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub item_id: ItemId,
    pub uom: StockDimension,
    pub quantity: PositiveQuantity,
    pub receiving_location_id: LocationId,
    pub observed_item_barcode: ItemBarcode,
    pub observed_receiving_location_barcode: DockBarcode,
    pub inventory_transaction_id: i64,
    pub inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<LicensePlateBarcode>,
    pub lot: Option<StockDimension>,
    pub serial: Option<StockDimension>,
    pub expiration: Option<Expiration>,
    pub inventory_hold_id: i64,
    pub reason: UnexpectedReceiptReason,
    pub note: Option<ExceptionNote>,
    pub load_status: ReceivingLoadStatus,
    pub confirmed_by_user_id: i64,
    pub confirmed_at: String,
    pub receipt_policy: ReceiptPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivingCommandResult {
    Unloading(UnloadingStartResult),
    Expected(ConfirmationResult),
    Unexpected(Box<UnexpectedReceiptResult>),
}

impl From<ConfirmationResult> for ReceivingCommandResult {
    fn from(result: ConfirmationResult) -> Self {
        Self::Expected(result)
    }
}
