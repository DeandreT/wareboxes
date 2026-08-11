use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmationRecoverySnapshotInput {
    pub load_barcode: LoadBarcode,
    pub load_id: LoadId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub reference_number: Option<String>,
    pub status: ReceivingLoadStatus,
    pub expected_seal: Option<SealBarcode>,
    pub dock: ReceivingDock,
    pub selected_line: ExpectedReceiptLine,
}

/// Durable pre-command state required to reconcile an expected receipt after restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "ConfirmationRecoverySnapshotInput",
    into = "ConfirmationRecoverySnapshotInput"
)]
pub struct ConfirmationRecoverySnapshot {
    input: ConfirmationRecoverySnapshotInput,
}

impl ConfirmationRecoverySnapshot {
    pub fn try_new(
        input: ConfirmationRecoverySnapshotInput,
    ) -> Result<Self, ReceivingValidationError> {
        if input.selected_line.remaining().get() == 0 {
            return Err(ReceivingValidationError::InvalidRecoveryLine);
        }
        Ok(Self { input })
    }

    #[must_use]
    pub const fn load_barcode(&self) -> &LoadBarcode {
        &self.input.load_barcode
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
    pub const fn expected_seal(&self) -> Option<&SealBarcode> {
        self.input.expected_seal.as_ref()
    }

    #[must_use]
    pub const fn selected_line(&self) -> &ExpectedReceiptLine {
        &self.input.selected_line
    }

    pub(super) fn capture(active: &ActiveSession, line_id: LoadLineId) -> Option<Self> {
        let selected_line = active.session.line(line_id)?.clone();
        Self::try_new(ConfirmationRecoverySnapshotInput {
            load_barcode: active.load_barcode.clone(),
            load_id: active.session.load_id(),
            inventory_owner_id: active.session.inventory_owner_id(),
            facility_id: active.session.facility_id(),
            reference_number: active.session.reference_number().map(str::to_owned),
            status: active.session.status(),
            expected_seal: active.session.expected_seal().cloned(),
            dock: active.session.dock().clone(),
            selected_line,
        })
        .ok()
    }

    fn restore_active(&self, command: &ExpectedReceiptCommand) -> Option<ActiveSession> {
        let line = self.selected_line();
        let draft = match command {
            ExpectedReceiptCommand::Received {
                item_barcode,
                receiving_location_barcode,
                quantity,
                license_plate_barcode,
                lot,
                serial,
                expiration,
            } => ConfirmationDraft {
                mode: ConfirmationMode::Received,
                selected_line_id: Some(line.load_line_id()),
                item_barcode: Some(item_barcode.clone()),
                dock_barcode: Some(receiving_location_barcode.clone()),
                quantity: Some(*quantity),
                container_capture: if license_plate_barcode.is_some() {
                    ContainerCapture::LicensePlate
                } else {
                    ContainerCapture::Loose
                },
                license_plate_barcode: license_plate_barcode.clone(),
                lot: lot.clone(),
                serial: serial.clone(),
                expiration: expiration.clone(),
                exception_reason: None,
                exception_note: None,
                unexpected_reason: None,
            },
            ExpectedReceiptCommand::Quarantined {
                item_barcode,
                receiving_location_barcode,
                quantity,
                license_plate_barcode,
                lot,
                serial,
                expiration,
                reason,
                note,
            } => ConfirmationDraft {
                mode: ConfirmationMode::Quarantined,
                selected_line_id: Some(line.load_line_id()),
                item_barcode: Some(item_barcode.clone()),
                dock_barcode: Some(receiving_location_barcode.clone()),
                quantity: Some(*quantity),
                container_capture: if license_plate_barcode.is_some() {
                    ContainerCapture::LicensePlate
                } else {
                    ContainerCapture::Loose
                },
                license_plate_barcode: license_plate_barcode.clone(),
                lot: lot.clone(),
                serial: serial.clone(),
                expiration: expiration.clone(),
                exception_reason: Some(reason.as_exception()),
                exception_note: note.clone(),
                unexpected_reason: None,
            },
            ExpectedReceiptCommand::Rejected {
                item_barcode,
                quantity,
                reason,
                note,
            } => ConfirmationDraft {
                mode: ConfirmationMode::Rejected,
                selected_line_id: Some(line.load_line_id()),
                item_barcode: Some(item_barcode.clone()),
                quantity: Some(*quantity),
                exception_reason: Some(*reason),
                exception_note: note.clone(),
                ..ConfirmationDraft::default()
            },
            ExpectedReceiptCommand::Missing {
                quantity,
                reason,
                note,
            } => ConfirmationDraft {
                mode: ConfirmationMode::Missing,
                selected_line_id: Some(line.load_line_id()),
                quantity: Some(*quantity),
                exception_reason: Some(*reason),
                exception_note: note.clone(),
                ..ConfirmationDraft::default()
            },
        };
        let session = ReceivingSession::try_new(ReceivingSessionInput {
            load_id: self.load_id(),
            inventory_owner_id: self.inventory_owner_id(),
            facility_id: self.facility_id(),
            reference_number: self.reference_number().map(str::to_owned),
            status: self.status(),
            expected_seal: self.expected_seal().cloned(),
            dock: self.dock().clone(),
            lines: vec![line.clone()],
        })
        .ok()?;
        Some(ActiveSession {
            load_barcode: self.load_barcode().clone(),
            session,
            draft,
            unloading: UnloadingDraft::default(),
        })
    }
}

impl TryFrom<ConfirmationRecoverySnapshotInput> for ConfirmationRecoverySnapshot {
    type Error = ReceivingValidationError;

    fn try_from(input: ConfirmationRecoverySnapshotInput) -> Result<Self, Self::Error> {
        Self::try_new(input)
    }
}

impl From<ConfirmationRecoverySnapshot> for ConfirmationRecoverySnapshotInput {
    fn from(snapshot: ConfirmationRecoverySnapshot) -> Self {
        snapshot.input
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmationIntent {
    pub schema_version: u16,
    pub load_id: LoadId,
    pub load_line_id: LoadLineId,
    pub command: ExpectedReceiptCommand,
    pub recovery: Box<ConfirmationRecoverySnapshot>,
}

impl ConfirmationIntent {
    pub fn try_new(
        recovery: ConfirmationRecoverySnapshot,
        command: ExpectedReceiptCommand,
    ) -> Result<Self, ReceivingValidationError> {
        let intent = Self {
            schema_version: CONFIRMATION_INTENT_SCHEMA_VERSION,
            load_id: recovery.load_id(),
            load_line_id: recovery.selected_line().load_line_id(),
            command,
            recovery: Box::new(recovery),
        };
        if intent.is_current_and_valid() {
            Ok(intent)
        } else {
            Err(ReceivingValidationError::InvalidConfirmationIntent)
        }
    }

    pub fn canonical_payload(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    #[must_use]
    pub fn is_current_and_valid(&self) -> bool {
        self.validation_failure().is_none()
    }

    pub(super) fn restore_active(&self) -> Option<ActiveSession> {
        self.recovery.restore_active(&self.command)
    }

    fn validation_failure(&self) -> Option<ReconciliationReason> {
        let line = self.recovery.selected_line();
        if self.schema_version != CONFIRMATION_INTENT_SCHEMA_VERSION
            || self.load_id != self.recovery.load_id()
            || self.load_line_id != line.load_line_id()
            || self.command.quantity().get() > line.remaining().get()
        {
            return Some(ReconciliationReason::CommandIntegrityFailure);
        }

        match &self.command {
            ExpectedReceiptCommand::Received {
                item_barcode,
                receiving_location_barcode,
                lot,
                serial,
                expiration,
                ..
            } => {
                if !line.accepts(item_barcode)
                    || receiving_location_barcode != self.recovery.dock().barcode()
                    || !matches_expected(line.lot(), lot.as_ref())
                    || !matches_expected(line.serial(), serial.as_ref())
                    || !matches_expected(line.expiration(), expiration.as_ref())
                {
                    return Some(ReconciliationReason::CommandIntegrityFailure);
                }
            }
            ExpectedReceiptCommand::Quarantined {
                item_barcode,
                receiving_location_barcode,
                lot,
                serial,
                expiration,
                reason,
                note,
                ..
            } => {
                if !line.accepts(item_barcode)
                    || receiving_location_barcode != self.recovery.dock().barcode()
                    || !matches_expected(line.lot(), lot.as_ref())
                    || !matches_expected(line.serial(), serial.as_ref())
                    || !matches_expected(line.expiration(), expiration.as_ref())
                    || (*reason == ReceiptQuarantineReason::Other && note.is_none())
                {
                    return Some(ReconciliationReason::CommandIntegrityFailure);
                }
            }
            ExpectedReceiptCommand::Rejected {
                item_barcode,
                reason,
                note,
                ..
            } => {
                if !line.accepts(item_barcode)
                    || (*reason == ReceiptExceptionReason::Other && note.is_none())
                {
                    return Some(ReconciliationReason::CommandIntegrityFailure);
                }
            }
            ExpectedReceiptCommand::Missing { reason, note, .. } => {
                if *reason == ReceiptExceptionReason::Other && note.is_none() {
                    return Some(ReconciliationReason::CommandIntegrityFailure);
                }
            }
        }
        None
    }
}

fn matches_expected<T: PartialEq>(expected: Option<&T>, actual: Option<&T>) -> bool {
    expected.is_none_or(|expected| actual == Some(expected))
}
