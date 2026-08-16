use serde::{Deserialize, Serialize};

use super::*;

const UNLOADING_START_INTENT_SCHEMA_VERSION: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnloadingStartCommand {
    pub load_scan: LoadBarcode,
    pub receiving_location_scan: DockBarcode,
    pub seal_scan: Option<SealBarcode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnloadingStartRecoverySnapshot {
    load_barcode: LoadBarcode,
    load_id: LoadId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    reference_number: Option<String>,
    expected_seal: Option<SealBarcode>,
    dock: ReceivingDock,
    receipt_policy: ReceiptPolicy,
    lines: Vec<ExpectedReceiptLine>,
}

impl UnloadingStartRecoverySnapshot {
    fn capture(active: &ActiveSession) -> Self {
        Self {
            load_barcode: active.load_barcode.clone(),
            load_id: active.session.load_id(),
            inventory_owner_id: active.session.inventory_owner_id(),
            facility_id: active.session.facility_id(),
            reference_number: active.session.reference_number().map(str::to_owned),
            expected_seal: active.session.expected_seal().cloned(),
            dock: active.session.dock().clone(),
            receipt_policy: active.session.receipt_policy().clone(),
            lines: active.session.lines().to_vec(),
        }
    }

    fn restore_active(&self, command: &UnloadingStartCommand) -> Option<ActiveSession> {
        let session = ReceivingSession::try_new(ReceivingSessionInput {
            load_id: self.load_id,
            inventory_owner_id: self.inventory_owner_id,
            facility_id: self.facility_id,
            reference_number: self.reference_number.clone(),
            status: ReceivingLoadStatus::Arrived,
            expected_seal: self.expected_seal.clone(),
            dock: self.dock.clone(),
            receipt_policy: self.receipt_policy.clone(),
            lines: self.lines.clone(),
        })
        .ok()?;
        Some(ActiveSession {
            load_barcode: self.load_barcode.clone(),
            session,
            draft: ConfirmationDraft::default(),
            unloading: UnloadingDraft {
                dock_scan: Some(command.receiving_location_scan.clone()),
                seal_scan: command.seal_scan.clone(),
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnloadingStartIntent {
    pub schema_version: u16,
    pub load_id: LoadId,
    pub command: UnloadingStartCommand,
    recovery: Box<UnloadingStartRecoverySnapshot>,
}

impl UnloadingStartIntent {
    pub fn try_new(
        load_barcode: LoadBarcode,
        session: ReceivingSession,
        command: UnloadingStartCommand,
    ) -> Result<Self, ReceivingValidationError> {
        if session.status() != ReceivingLoadStatus::Arrived {
            return Err(ReceivingValidationError::InvalidConfirmationIntent);
        }
        let active = ActiveSession {
            load_barcode,
            session,
            draft: ConfirmationDraft::default(),
            unloading: UnloadingDraft {
                dock_scan: Some(command.receiving_location_scan.clone()),
                seal_scan: command.seal_scan.clone(),
            },
        };
        let intent = Self {
            schema_version: UNLOADING_START_INTENT_SCHEMA_VERSION,
            load_id: active.session.load_id(),
            command,
            recovery: Box::new(UnloadingStartRecoverySnapshot::capture(&active)),
        };
        if intent.is_current_and_valid() {
            Ok(intent)
        } else {
            Err(ReceivingValidationError::InvalidConfirmationIntent)
        }
    }

    pub(super) fn capture(active: &ActiveSession) -> Option<Self> {
        let command = UnloadingStartCommand {
            load_scan: active.load_barcode.clone(),
            receiving_location_scan: active.unloading.dock_scan.clone()?,
            seal_scan: active.unloading.seal_scan.clone(),
        };
        Self::try_new(active.load_barcode.clone(), active.session.clone(), command).ok()
    }

    #[must_use]
    pub fn is_current_and_valid(&self) -> bool {
        self.schema_version == UNLOADING_START_INTENT_SCHEMA_VERSION
            && self.load_id == self.recovery.load_id
            && self.command.load_scan == self.recovery.load_barcode
            && self.command.receiving_location_scan == *self.recovery.dock.barcode()
            && self.command.seal_scan == self.recovery.expected_seal
            && ReceivingSession::try_new(ReceivingSessionInput {
                load_id: self.recovery.load_id,
                inventory_owner_id: self.recovery.inventory_owner_id,
                facility_id: self.recovery.facility_id,
                reference_number: self.recovery.reference_number.clone(),
                status: ReceivingLoadStatus::Arrived,
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

    #[must_use]
    pub(super) const fn receiving_location_id(&self) -> LocationId {
        self.recovery.dock.location_id()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnloadingStartResult {
    pub unloading_start_id: i64,
    pub load_id: LoadId,
    pub receiving_location_id: LocationId,
    pub started_by: i64,
    pub started_at: String,
}
