use serde::{Deserialize, Serialize};

use crate::workflow::{
    Activity, CommandOutcome, DurableCommandDraft, PersistedCommand, RfCommand, Transition,
    WorkflowEffect,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplenishmentLocation {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplenishmentClaim {
    pub work_id: i64,
    pub plan_id: i64,
    pub policy_id: i64,
    pub policy_revision: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub sequence: u32,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<String>,
    pub lease_expires_at: String,
    pub source_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub quantity: i64,
    pub source_location: ReplenishmentLocation,
    pub destination_pick_face: ReplenishmentLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplenishmentConfirmationResult {
    pub confirmation_id: i64,
    pub work_id: i64,
    pub plan_id: i64,
    pub policy_id: i64,
    pub inventory_transaction_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub source_location_id: i64,
    pub destination_pick_face_location_id: i64,
    pub quantity: i64,
    pub confirmed_by: i64,
    pub confirmed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentConfirmationExpectation {
    pub plan_id: i64,
    pub policy_id: i64,
    pub source_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub source_location_id: i64,
    pub destination_pick_face_location_id: i64,
    pub quantity: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReplenishmentScanStage {
    SourceLocation,
    Item,
    Lot,
    Serial,
    DestinationPickFace,
}

impl ReplenishmentScanStage {
    pub const fn prompt(self) -> &'static str {
        match self {
            Self::SourceLocation => "Scan reserve source",
            Self::Item => "Scan item",
            Self::Lot => "Scan lot",
            Self::Serial => "Scan serial",
            Self::DestinationPickFace => "Scan destination pick face",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SourceBlocked,
    DestinationBlocked,
    InventoryMismatch,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ReplenishmentCommand {
    ClaimNext,
    ClaimById {
        work_id: i64,
    },
    Confirm {
        work_id: i64,
        expected: Box<ReplenishmentConfirmationExpectation>,
        source_location_barcode: String,
        item_barcode: String,
        lot_scan: Option<String>,
        serial_scan: Option<String>,
        destination_pick_face_barcode: String,
    },
    Release {
        work_id: i64,
        reason: ReplenishmentReleaseReason,
        note: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lane {
    Empty,
    Persisting(DurableCommandDraft),
    Ready(PersistedCommand),
    InFlight(PersistedCommand),
    Ambiguous {
        command: PersistedCommand,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct ReplenishmentWorkflow {
    claim: Option<ReplenishmentClaim>,
    lane: Lane,
    source_location_scan: Option<String>,
    item_scan: Option<String>,
    lot_scan: Option<String>,
    serial_scan: Option<String>,
    destination_scan: Option<String>,
    scan_draft: String,
    error: Option<String>,
    notice: Option<String>,
    reconcile_reason: Option<String>,
}

impl Default for ReplenishmentWorkflow {
    fn default() -> Self {
        Self {
            claim: None,
            lane: Lane::Empty,
            source_location_scan: None,
            item_scan: None,
            lot_scan: None,
            serial_scan: None,
            destination_scan: None,
            scan_draft: String::new(),
            error: None,
            notice: None,
            reconcile_reason: None,
        }
    }
}

impl ReplenishmentWorkflow {
    pub fn activity(&self) -> Activity {
        if self.reconcile_reason.is_some() {
            return Activity::ReconcileRequired;
        }
        match self.lane {
            Lane::Persisting(_) => Activity::Persisting,
            Lane::Ready(_) => Activity::ReadyToDispatch,
            Lane::InFlight(_) => Activity::InFlight,
            Lane::Ambiguous { .. } => Activity::Ambiguous,
            Lane::Empty if self.claim.is_some() => Activity::Active,
            Lane::Empty => Activity::Idle,
        }
    }

    pub const fn claim(&self) -> Option<&ReplenishmentClaim> {
        self.claim.as_ref()
    }

    pub fn scan_draft_mut(&mut self) -> &mut String {
        &mut self.scan_draft
    }

    pub fn expected_scan(&self) -> Option<ReplenishmentScanStage> {
        let claim = self.claim.as_ref()?;
        if self.source_location_scan.is_none() {
            Some(ReplenishmentScanStage::SourceLocation)
        } else if self.item_scan.is_none() {
            Some(ReplenishmentScanStage::Item)
        } else if claim.lot.is_some() && self.lot_scan.is_none() {
            Some(ReplenishmentScanStage::Lot)
        } else if claim.serial.is_some() && self.serial_scan.is_none() {
            Some(ReplenishmentScanStage::Serial)
        } else if self.destination_scan.is_none() {
            Some(ReplenishmentScanStage::DestinationPickFace)
        } else {
            None
        }
    }

    pub fn begin_claim_next(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        self.begin_idle(ReplenishmentCommand::ClaimNext, command_id, idempotency_key)
    }

    pub fn begin_claim_by_id(
        &mut self,
        work_id: i64,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if work_id <= 0 {
            self.error = Some("Task ID must be positive".into());
            return None;
        }
        self.begin_idle(
            ReplenishmentCommand::ClaimById { work_id },
            command_id,
            idempotency_key,
        )
    }

    fn begin_idle(
        &mut self,
        command: ReplenishmentCommand,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Idle {
            return None;
        }
        self.begin(command, command_id, idempotency_key)
    }

    pub fn submit_scan(&mut self) -> bool {
        if self.activity() != Activity::Active {
            return false;
        }
        let Some(stage) = self.expected_scan() else {
            return false;
        };
        let scanned = self.scan_draft.trim().to_owned();
        if scanned.is_empty() {
            self.error = Some("A scan is required".into());
            return false;
        }
        let Some(claim) = self.claim.as_ref() else {
            return false;
        };
        let accepted = match stage {
            ReplenishmentScanStage::SourceLocation => scanned == claim.source_location.barcode,
            ReplenishmentScanStage::Item => claim
                .item_barcodes
                .iter()
                .any(|barcode| barcode == &scanned),
            ReplenishmentScanStage::Lot => claim.lot.as_deref() == Some(scanned.as_str()),
            ReplenishmentScanStage::Serial => claim.serial.as_deref() == Some(scanned.as_str()),
            ReplenishmentScanStage::DestinationPickFace => {
                scanned == claim.destination_pick_face.barcode
            }
        };
        if !accepted {
            self.scan_draft.clear();
            self.error = Some(scan_mismatch(stage).into());
            return false;
        }
        match stage {
            ReplenishmentScanStage::SourceLocation => self.source_location_scan = Some(scanned),
            ReplenishmentScanStage::Item => self.item_scan = Some(scanned),
            ReplenishmentScanStage::Lot => self.lot_scan = Some(scanned),
            ReplenishmentScanStage::Serial => self.serial_scan = Some(scanned),
            ReplenishmentScanStage::DestinationPickFace => self.destination_scan = Some(scanned),
        }
        self.scan_draft.clear();
        self.error = None;
        true
    }

    pub fn begin_confirmation(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Active || self.expected_scan().is_some() {
            return None;
        }
        let claim = self.claim.as_ref()?;
        let command = ReplenishmentCommand::Confirm {
            work_id: claim.work_id,
            expected: Box::new(ReplenishmentConfirmationExpectation::from(claim)),
            source_location_barcode: self.source_location_scan.clone()?,
            item_barcode: self.item_scan.clone()?,
            lot_scan: self.lot_scan.clone(),
            serial_scan: self.serial_scan.clone(),
            destination_pick_face_barcode: self.destination_scan.clone()?,
        };
        self.begin(command, command_id, idempotency_key)
    }

    pub fn begin_release(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        let work_id = self.claim.as_ref()?.work_id;
        self.begin(
            ReplenishmentCommand::Release {
                work_id,
                reason: ReplenishmentReleaseReason::WorkInterrupted,
                note: None,
            },
            command_id,
            idempotency_key,
        )
    }

    fn begin(
        &mut self,
        command: ReplenishmentCommand,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if command_id.trim().is_empty() || idempotency_key.trim().is_empty() {
            self.error = Some("Command identity is unavailable".into());
            return None;
        }
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id,
            idempotency_key,
            command: RfCommand::Replenishment(command),
        };
        self.lane = Lane::Persisting(draft.clone());
        self.error = None;
        self.notice = None;
        Some(WorkflowEffect::PersistCommand(draft))
    }

    pub fn command_persisted(&mut self, command_id: &str, record_id: i64) -> Transition {
        let Lane::Persisting(draft) = &self.lane else {
            return Transition::Ignored;
        };
        if draft.command_id != command_id || record_id <= 0 {
            return Transition::Ignored;
        }
        self.lane = Lane::Ready(PersistedCommand {
            record_id,
            draft: draft.clone(),
        });
        Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id })
    }

    pub fn dispatch_started(&mut self, record_id: i64) {
        if let Lane::Ready(command) = &self.lane
            && command.record_id == record_id
        {
            self.lane = Lane::InFlight(command.clone());
        }
    }

    pub fn dispatch_ambiguous(&mut self, record_id: i64, message: impl Into<String>) {
        if let Lane::InFlight(command) = &self.lane
            && command.record_id == record_id
        {
            self.lane = Lane::Ambiguous {
                command: command.clone(),
                message: message.into(),
            };
        }
    }

    pub fn retry_ambiguous(&mut self) -> Option<WorkflowEffect> {
        let Lane::Ambiguous { command, .. } = &self.lane else {
            return None;
        };
        let record_id = command.record_id;
        self.lane = Lane::Ready(command.clone());
        Some(WorkflowEffect::DispatchPersistedCommand { record_id })
    }

    pub fn ambiguous_message(&self) -> Option<&str> {
        match &self.lane {
            Lane::Ambiguous { message, .. } => Some(message),
            _ => None,
        }
    }

    pub fn durable_outcome_recorded(&mut self, record_id: i64, outcome: CommandOutcome) {
        if !self.accepts_outcome(record_id, &outcome) {
            self.require_reconciliation(
                "The saved replenishment result does not match its command or task".into(),
            );
            return;
        }
        self.lane = Lane::Empty;
        match outcome {
            CommandOutcome::ReplenishmentClaimed(claim) => {
                self.claim = claim.map(|claim| *claim);
                self.reset_scans();
                self.notice = self
                    .claim
                    .is_none()
                    .then(|| "No replenishment work is ready".into());
            }
            CommandOutcome::ReplenishmentConfirmed(_) => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some("Pick face replenished".into());
            }
            CommandOutcome::ReplenishmentReleased { .. } => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some("Replenishment returned to queue".into());
            }
            _ => self.require_reconciliation(
                "The saved command returned the wrong workflow result".into(),
            ),
        }
    }

    pub fn accepts_outcome(&self, record_id: i64, outcome: &CommandOutcome) -> bool {
        match &self.lane {
            Lane::InFlight(command) | Lane::Ambiguous { command, .. }
                if command.record_id == record_id =>
            {
                self.outcome_matches(&command.draft.command, outcome)
            }
            Lane::Empty
            | Lane::Persisting(_)
            | Lane::Ready(_)
            | Lane::InFlight(_)
            | Lane::Ambiguous { .. } => false,
        }
    }

    fn outcome_matches(&self, command: &RfCommand, outcome: &CommandOutcome) -> bool {
        match (command, outcome) {
            (
                RfCommand::Replenishment(ReplenishmentCommand::ClaimNext),
                CommandOutcome::ReplenishmentClaimed(_),
            ) => true,
            (
                RfCommand::Replenishment(ReplenishmentCommand::ClaimById { work_id }),
                CommandOutcome::ReplenishmentClaimed(Some(claim)),
            ) => claim.work_id == *work_id,
            (
                RfCommand::Replenishment(ReplenishmentCommand::Confirm {
                    work_id, expected, ..
                }),
                CommandOutcome::ReplenishmentConfirmed(result),
            ) => result.work_id == *work_id && confirmation_matches_expectation(result, expected),
            (
                RfCommand::Replenishment(ReplenishmentCommand::Release { work_id, .. }),
                CommandOutcome::ReplenishmentReleased {
                    work_id: released_id,
                },
            ) => work_id == released_id,
            _ => false,
        }
    }

    pub fn durable_rejection_recorded(&mut self, record_id: i64, message: String) {
        let command = match &self.lane {
            Lane::InFlight(command) | Lane::Ambiguous { command, .. } => {
                (command.record_id == record_id).then(|| command.draft.command.clone())
            }
            _ => None,
        };
        if let Some(command) = command {
            self.lane = Lane::Empty;
            if matches!(
                command,
                RfCommand::Replenishment(ReplenishmentCommand::Confirm { .. })
            ) {
                self.reset_scans();
            }
            self.error = Some(message);
        }
    }

    pub fn restore_current_claim(&mut self, claim: Option<ReplenishmentClaim>) {
        if matches!(self.lane, Lane::Empty) {
            self.claim = claim;
            self.reset_scans();
            self.error = None;
            self.notice = None;
        }
    }

    pub fn restore_ready_command(
        &mut self,
        record_id: i64,
        draft: DurableCommandDraft,
    ) -> Transition {
        if record_id <= 0 || !matches!(draft.command, RfCommand::Replenishment(_)) {
            self.require_reconciliation("Saved work is not a replenishment command".into());
            return Transition::Ignored;
        }
        self.lane = Lane::Ready(PersistedCommand { record_id, draft });
        Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id })
    }

    pub fn restore_ambiguous_command(
        &mut self,
        record_id: i64,
        draft: DurableCommandDraft,
        message: impl Into<String>,
    ) {
        if record_id <= 0 || !matches!(draft.command, RfCommand::Replenishment(_)) {
            self.require_reconciliation("Saved work is not a replenishment command".into());
            return;
        }
        self.lane = Lane::Ambiguous {
            command: PersistedCommand { record_id, draft },
            message: message.into(),
        };
    }

    pub fn require_reconciliation(&mut self, reason: String) {
        self.reconcile_reason = Some(reason);
        self.error = None;
    }

    pub fn reconcile_reason(&self) -> Option<&str> {
        self.reconcile_reason.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn owns_record(&self, record_id: i64) -> bool {
        match &self.lane {
            Lane::Ready(command) | Lane::InFlight(command) | Lane::Ambiguous { command, .. } => {
                command.record_id == record_id
            }
            Lane::Empty | Lane::Persisting(_) => false,
        }
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub fn load_debug_claim(&mut self, claim: ReplenishmentClaim) {
        self.claim = Some(claim);
        self.lane = Lane::Empty;
        self.reconcile_reason = None;
        self.error = None;
        self.notice = None;
        self.reset_scans();
    }

    fn reset_scans(&mut self) {
        self.source_location_scan = None;
        self.item_scan = None;
        self.lot_scan = None;
        self.serial_scan = None;
        self.destination_scan = None;
        self.scan_draft.clear();
    }
}

fn scan_mismatch(stage: ReplenishmentScanStage) -> &'static str {
    match stage {
        ReplenishmentScanStage::SourceLocation => "Source location does not match this task",
        ReplenishmentScanStage::Item => "Item does not match this task",
        ReplenishmentScanStage::Lot => "Lot does not match this task",
        ReplenishmentScanStage::Serial => "Serial does not match this task",
        ReplenishmentScanStage::DestinationPickFace => {
            "Destination pick face does not match this task"
        }
    }
}

fn confirmation_matches_expectation(
    result: &ReplenishmentConfirmationResult,
    expected: &ReplenishmentConfirmationExpectation,
) -> bool {
    result.plan_id == expected.plan_id
        && result.policy_id == expected.policy_id
        && result.source_inventory_balance_id == expected.source_inventory_balance_id
        && result.item_batch_id == expected.item_batch_id
        && result.item_id == expected.item_id
        && result.uom == expected.uom
        && result.lot == expected.lot
        && result.serial == expected.serial
        && result.source_location_id == expected.source_location_id
        && result.destination_pick_face_location_id == expected.destination_pick_face_location_id
        && result.quantity == expected.quantity
}

impl From<&ReplenishmentClaim> for ReplenishmentConfirmationExpectation {
    fn from(claim: &ReplenishmentClaim) -> Self {
        Self {
            plan_id: claim.plan_id,
            policy_id: claim.policy_id,
            source_inventory_balance_id: claim.source_inventory_balance_id,
            item_batch_id: claim.item_batch_id,
            item_id: claim.item_id,
            uom: claim.uom.clone(),
            lot: claim.lot.clone(),
            serial: claim.serial.clone(),
            source_location_id: claim.source_location.location_id,
            destination_pick_face_location_id: claim.destination_pick_face.location_id,
            quantity: claim.quantity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(controlled: bool) -> ReplenishmentClaim {
        ReplenishmentClaim {
            work_id: 42,
            plan_id: 20,
            policy_id: 10,
            policy_revision: 3,
            inventory_owner_id: 2,
            facility_id: 3,
            sequence: 1,
            priority: 90,
            instructions: None,
            due_at: None,
            lease_expires_at: "2026-08-08T20:00:00Z".into(),
            source_inventory_balance_id: 100,
            item_batch_id: 101,
            item_id: 5,
            item_description: Some("Nitrile gloves".into()),
            item_barcodes: vec!["SKU-1".into(), "UPC-1".into()],
            uom: "each".into(),
            lot: controlled.then(|| "LOT-1".into()),
            serial: controlled.then(|| "SERIAL-1".into()),
            expiration: None,
            quantity: 8,
            source_location: ReplenishmentLocation {
                location_id: 7,
                barcode: "RES-01".into(),
                name: Some("Reserve 01".into()),
            },
            destination_pick_face: ReplenishmentLocation {
                location_id: 8,
                barcode: "PICK-01".into(),
                name: Some("Pick 01".into()),
            },
        }
    }

    #[test]
    fn exact_scanner_sequence_emits_confirmation_without_quantity() {
        let mut workflow = ReplenishmentWorkflow::default();
        workflow.restore_current_claim(Some(claim(true)));
        for (scan, stage) in [
            ("RES-01", ReplenishmentScanStage::SourceLocation),
            ("UPC-1", ReplenishmentScanStage::Item),
            ("LOT-1", ReplenishmentScanStage::Lot),
            ("SERIAL-1", ReplenishmentScanStage::Serial),
            ("PICK-01", ReplenishmentScanStage::DestinationPickFace),
        ] {
            assert_eq!(workflow.expected_scan(), Some(stage));
            *workflow.scan_draft_mut() = scan.into();
            assert!(workflow.submit_scan());
        }
        let effect = workflow
            .begin_confirmation("command-1".into(), "key-1".into())
            .unwrap();
        let WorkflowEffect::PersistCommand(draft) = effect else {
            panic!("confirmation must persist first");
        };
        let RfCommand::Replenishment(ReplenishmentCommand::Confirm {
            work_id,
            lot_scan,
            serial_scan,
            ..
        }) = draft.command
        else {
            panic!("expected replenishment confirmation");
        };
        assert_eq!(work_id, 42);
        assert_eq!(lot_scan.as_deref(), Some("LOT-1"));
        assert_eq!(serial_scan.as_deref(), Some("SERIAL-1"));
    }

    #[test]
    fn uncontrolled_stock_skips_lot_and_serial() {
        let mut workflow = ReplenishmentWorkflow::default();
        workflow.restore_current_claim(Some(claim(false)));
        for scan in ["RES-01", "SKU-1"] {
            *workflow.scan_draft_mut() = scan.into();
            assert!(workflow.submit_scan());
        }
        assert_eq!(
            workflow.expected_scan(),
            Some(ReplenishmentScanStage::DestinationPickFace)
        );
    }

    #[test]
    fn mismatch_does_not_advance_or_retain_bad_scan() {
        let mut workflow = ReplenishmentWorkflow::default();
        workflow.restore_current_claim(Some(claim(false)));
        *workflow.scan_draft_mut() = "OTHER".into();
        assert!(!workflow.submit_scan());
        assert_eq!(
            workflow.expected_scan(),
            Some(ReplenishmentScanStage::SourceLocation)
        );
        assert!(workflow.scan_draft_mut().is_empty());
        assert_eq!(
            workflow.error(),
            Some("Source location does not match this task")
        );
    }

    #[test]
    fn confirmation_result_must_match_the_full_claim_snapshot() {
        let mut workflow = ReplenishmentWorkflow::default();
        workflow.restore_current_claim(Some(claim(false)));
        for scan in ["RES-01", "SKU-1", "PICK-01"] {
            *workflow.scan_draft_mut() = scan.into();
            assert!(workflow.submit_scan());
        }
        let effect = workflow
            .begin_confirmation("command-1".into(), "key-1".into())
            .unwrap();
        let WorkflowEffect::PersistCommand(draft) = effect else {
            panic!("confirmation must persist first");
        };
        assert!(matches!(
            workflow.command_persisted("command-1", 9),
            Transition::Effect(_)
        ));
        workflow.dispatch_started(9);
        let mut result = confirmation_for(&claim(false));
        result.quantity = 7;
        assert!(!workflow.accepts_outcome(
            9,
            &CommandOutcome::ReplenishmentConfirmed(Box::new(result.clone()))
        ));
        workflow
            .durable_outcome_recorded(9, CommandOutcome::ReplenishmentConfirmed(Box::new(result)));
        assert_eq!(workflow.activity(), Activity::ReconcileRequired);
        assert!(matches!(draft.command, RfCommand::Replenishment(_)));
    }

    #[test]
    fn restarted_ambiguous_confirmation_retains_authoritative_expectation() {
        let claim = claim(false);
        let mut before_restart = ReplenishmentWorkflow::default();
        before_restart.restore_current_claim(Some(claim.clone()));
        for scan in ["RES-01", "SKU-1", "PICK-01"] {
            *before_restart.scan_draft_mut() = scan.into();
            assert!(before_restart.submit_scan());
        }
        let Some(WorkflowEffect::PersistCommand(draft)) =
            before_restart.begin_confirmation("command-1".into(), "key-1".into())
        else {
            panic!("confirmation must persist first");
        };

        let mut restored = ReplenishmentWorkflow::default();
        restored.restore_ambiguous_command(9, draft, "Check saved result");
        let outcome = CommandOutcome::ReplenishmentConfirmed(Box::new(confirmation_for(&claim)));
        assert!(restored.accepts_outcome(9, &outcome));
        restored.durable_outcome_recorded(9, outcome);
        assert_eq!(restored.activity(), Activity::Idle);
        assert_eq!(restored.notice(), Some("Pick face replenished"));
    }

    #[test]
    fn definitive_validation_rejection_requires_fresh_scans() {
        let mut workflow = ReplenishmentWorkflow::default();
        workflow.restore_current_claim(Some(claim(false)));
        for scan in ["RES-01", "SKU-1", "PICK-01"] {
            *workflow.scan_draft_mut() = scan.into();
            assert!(workflow.submit_scan());
        }
        let Some(WorkflowEffect::PersistCommand(_)) =
            workflow.begin_confirmation("command-1".into(), "key-1".into())
        else {
            panic!("confirmation must persist first");
        };
        assert!(matches!(
            workflow.command_persisted("command-1", 9),
            Transition::Effect(_)
        ));
        workflow.dispatch_started(9);
        workflow.durable_rejection_recorded(9, "Scan rejected".into());
        assert_eq!(workflow.activity(), Activity::Active);
        assert_eq!(
            workflow.expected_scan(),
            Some(ReplenishmentScanStage::SourceLocation)
        );
    }

    fn confirmation_for(claim: &ReplenishmentClaim) -> ReplenishmentConfirmationResult {
        ReplenishmentConfirmationResult {
            confirmation_id: 1,
            work_id: claim.work_id,
            plan_id: claim.plan_id,
            policy_id: claim.policy_id,
            inventory_transaction_id: 2,
            source_inventory_balance_id: claim.source_inventory_balance_id,
            destination_inventory_balance_id: 200,
            item_batch_id: claim.item_batch_id,
            item_id: claim.item_id,
            uom: claim.uom.clone(),
            lot: claim.lot.clone(),
            serial: claim.serial.clone(),
            source_location_id: claim.source_location.location_id,
            destination_pick_face_location_id: claim.destination_pick_face.location_id,
            quantity: claim.quantity,
            confirmed_by: 3,
            confirmed_at: "2026-08-08T20:00:00Z".into(),
        }
    }
}
