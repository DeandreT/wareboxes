use serde::{Deserialize, Serialize};

use crate::workflow::{
    Activity, CommandOutcome, DurableCommandDraft, PersistedCommand, RfCommand, Transition,
    WorkflowEffect,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossDockLocation {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossDockClaim {
    pub work_id: i64,
    pub plan_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub order_line_id: i64,
    pub order_line_key: String,
    pub reservation_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<String>,
    pub lease_expires_at: String,
    pub source_receipt_inventory_transaction_id: i64,
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
    pub source_receiving_location: CrossDockLocation,
    pub destination_pick_face: CrossDockLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossDockConfirmationResult {
    pub confirmation_id: i64,
    pub work_id: i64,
    pub plan_id: i64,
    pub order_id: i64,
    pub order_line_id: i64,
    pub reservation_id: i64,
    pub inventory_transaction_id: i64,
    pub inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_pick_face_location_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub quantity: i64,
    pub confirmed_by: i64,
    pub confirmed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossDockConfirmationExpectation {
    pub plan_id: i64,
    pub order_id: i64,
    pub order_line_id: i64,
    pub reservation_id: i64,
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
pub enum CrossDockScanStage {
    SourceReceivingLocation,
    Item,
    Lot,
    Serial,
    DestinationPickFace,
}

impl CrossDockScanStage {
    pub const fn prompt(self) -> &'static str {
        match self {
            Self::SourceReceivingLocation => "Scan receiving source",
            Self::Item => "Scan item",
            Self::Lot => "Scan lot",
            Self::Serial => "Scan serial",
            Self::DestinationPickFace => "Scan destination pick face",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossDockReleaseReason {
    WorkInterrupted,
    EndOfShift,
    EquipmentIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum CrossDockCommand {
    ClaimNext,
    ClaimById {
        work_id: i64,
    },
    Confirm {
        work_id: i64,
        expected: Box<CrossDockConfirmationExpectation>,
        source_receiving_location_barcode: String,
        item_barcode: String,
        lot_scan: Option<String>,
        serial_scan: Option<String>,
        destination_pick_face_barcode: String,
    },
    Release {
        work_id: i64,
        reason: CrossDockReleaseReason,
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
pub struct CrossDockWorkflow {
    claim: Option<CrossDockClaim>,
    lane: Lane,
    source_scan: Option<String>,
    item_scan: Option<String>,
    lot_scan: Option<String>,
    serial_scan: Option<String>,
    destination_scan: Option<String>,
    scan_draft: String,
    error: Option<String>,
    notice: Option<String>,
    reconcile_reason: Option<String>,
}

impl Default for CrossDockWorkflow {
    fn default() -> Self {
        Self {
            claim: None,
            lane: Lane::Empty,
            source_scan: None,
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

impl CrossDockWorkflow {
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

    pub const fn claim(&self) -> Option<&CrossDockClaim> {
        self.claim.as_ref()
    }

    pub fn scan_draft_mut(&mut self) -> &mut String {
        &mut self.scan_draft
    }

    pub fn expected_scan(&self) -> Option<CrossDockScanStage> {
        let claim = self.claim.as_ref()?;
        if self.source_scan.is_none() {
            Some(CrossDockScanStage::SourceReceivingLocation)
        } else if self.item_scan.is_none() {
            Some(CrossDockScanStage::Item)
        } else if claim.lot.is_some() && self.lot_scan.is_none() {
            Some(CrossDockScanStage::Lot)
        } else if claim.serial.is_some() && self.serial_scan.is_none() {
            Some(CrossDockScanStage::Serial)
        } else if self.destination_scan.is_none() {
            Some(CrossDockScanStage::DestinationPickFace)
        } else {
            None
        }
    }

    pub fn begin_claim_next(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        self.begin_idle(CrossDockCommand::ClaimNext, command_id, idempotency_key)
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
            CrossDockCommand::ClaimById { work_id },
            command_id,
            idempotency_key,
        )
    }

    fn begin_idle(
        &mut self,
        command: CrossDockCommand,
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
            CrossDockScanStage::SourceReceivingLocation => {
                scanned == claim.source_receiving_location.barcode
            }
            CrossDockScanStage::Item => claim
                .item_barcodes
                .iter()
                .any(|barcode| barcode == &scanned),
            CrossDockScanStage::Lot => claim.lot.as_deref() == Some(scanned.as_str()),
            CrossDockScanStage::Serial => claim.serial.as_deref() == Some(scanned.as_str()),
            CrossDockScanStage::DestinationPickFace => {
                scanned == claim.destination_pick_face.barcode
            }
        };
        if !accepted {
            self.scan_draft.clear();
            self.error = Some(scan_mismatch(stage).into());
            return false;
        }
        match stage {
            CrossDockScanStage::SourceReceivingLocation => self.source_scan = Some(scanned),
            CrossDockScanStage::Item => self.item_scan = Some(scanned),
            CrossDockScanStage::Lot => self.lot_scan = Some(scanned),
            CrossDockScanStage::Serial => self.serial_scan = Some(scanned),
            CrossDockScanStage::DestinationPickFace => self.destination_scan = Some(scanned),
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
        self.begin(
            CrossDockCommand::Confirm {
                work_id: claim.work_id,
                expected: Box::new(CrossDockConfirmationExpectation::from(claim)),
                source_receiving_location_barcode: self.source_scan.clone()?,
                item_barcode: self.item_scan.clone()?,
                lot_scan: self.lot_scan.clone(),
                serial_scan: self.serial_scan.clone(),
                destination_pick_face_barcode: self.destination_scan.clone()?,
            },
            command_id,
            idempotency_key,
        )
    }

    pub fn begin_release(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        let work_id = self.claim.as_ref()?.work_id;
        self.begin(
            CrossDockCommand::Release {
                work_id,
                reason: CrossDockReleaseReason::WorkInterrupted,
                note: None,
            },
            command_id,
            idempotency_key,
        )
    }

    fn begin(
        &mut self,
        command: CrossDockCommand,
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
            command: RfCommand::CrossDock(command),
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
                "The saved cross-dock result does not match its command or task".into(),
            );
            return;
        }
        self.lane = Lane::Empty;
        match outcome {
            CommandOutcome::CrossDockClaimed(claim) => {
                self.claim = claim.map(|claim| *claim);
                self.reset_scans();
                self.notice = self
                    .claim
                    .is_none()
                    .then(|| "No cross-dock work is ready".into());
            }
            CommandOutcome::CrossDockConfirmed(_) => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some("Cross-dock move confirmed".into());
            }
            CommandOutcome::CrossDockReleased { .. } => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some("Cross-dock work returned to queue".into());
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
                outcome_matches(&command.draft.command, outcome)
            }
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
                RfCommand::CrossDock(CrossDockCommand::Confirm { .. })
            ) {
                self.reset_scans();
            }
            self.error = Some(message);
        }
    }

    pub fn restore_current_claim(&mut self, claim: Option<CrossDockClaim>) {
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
        if record_id <= 0 || !matches!(draft.command, RfCommand::CrossDock(_)) {
            self.require_reconciliation("Saved work is not a cross-dock command".into());
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
        if record_id <= 0 || !matches!(draft.command, RfCommand::CrossDock(_)) {
            self.require_reconciliation("Saved work is not a cross-dock command".into());
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
    pub(crate) fn load_debug_claim(&mut self, claim: CrossDockClaim) {
        self.lane = Lane::Empty;
        self.claim = Some(claim);
        self.reset_scans();
        self.error = None;
        self.notice = None;
        self.reconcile_reason = None;
    }

    fn reset_scans(&mut self) {
        self.source_scan = None;
        self.item_scan = None;
        self.lot_scan = None;
        self.serial_scan = None;
        self.destination_scan = None;
        self.scan_draft.clear();
    }
}

fn scan_mismatch(stage: CrossDockScanStage) -> &'static str {
    match stage {
        CrossDockScanStage::SourceReceivingLocation => "Receiving source does not match this task",
        CrossDockScanStage::Item => "Item does not match this task",
        CrossDockScanStage::Lot => "Lot does not match this task",
        CrossDockScanStage::Serial => "Serial does not match this task",
        CrossDockScanStage::DestinationPickFace => "Destination pick face does not match this task",
    }
}

fn outcome_matches(command: &RfCommand, outcome: &CommandOutcome) -> bool {
    match (command, outcome) {
        (
            RfCommand::CrossDock(CrossDockCommand::ClaimNext),
            CommandOutcome::CrossDockClaimed(_),
        ) => true,
        (
            RfCommand::CrossDock(CrossDockCommand::ClaimById { work_id }),
            CommandOutcome::CrossDockClaimed(Some(claim)),
        ) => claim.work_id == *work_id,
        (
            RfCommand::CrossDock(CrossDockCommand::Confirm {
                work_id, expected, ..
            }),
            CommandOutcome::CrossDockConfirmed(result),
        ) => result.work_id == *work_id && confirmation_matches(result, expected),
        (
            RfCommand::CrossDock(CrossDockCommand::Release { work_id, .. }),
            CommandOutcome::CrossDockReleased {
                work_id: released_id,
            },
        ) => work_id == released_id,
        _ => false,
    }
}

fn confirmation_matches(
    result: &CrossDockConfirmationResult,
    expected: &CrossDockConfirmationExpectation,
) -> bool {
    result.plan_id == expected.plan_id
        && result.order_id == expected.order_id
        && result.order_line_id == expected.order_line_id
        && result.reservation_id == expected.reservation_id
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

impl From<&CrossDockClaim> for CrossDockConfirmationExpectation {
    fn from(claim: &CrossDockClaim) -> Self {
        Self {
            plan_id: claim.plan_id,
            order_id: claim.order_id,
            order_line_id: claim.order_line_id,
            reservation_id: claim.reservation_id,
            source_inventory_balance_id: claim.source_inventory_balance_id,
            item_batch_id: claim.item_batch_id,
            item_id: claim.item_id,
            uom: claim.uom.clone(),
            lot: claim.lot.clone(),
            serial: claim.serial.clone(),
            source_location_id: claim.source_receiving_location.location_id,
            destination_pick_face_location_id: claim.destination_pick_face.location_id,
            quantity: claim.quantity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim() -> CrossDockClaim {
        CrossDockClaim {
            work_id: 42,
            plan_id: 20,
            inventory_owner_id: 3,
            facility_id: 4,
            order_id: 5,
            order_key: "ORDER-5".into(),
            order_line_id: 6,
            order_line_key: "LINE-1".into(),
            reservation_id: 7,
            priority: 25,
            instructions: None,
            due_at: None,
            lease_expires_at: "2026-08-11T12:00:00Z".into(),
            source_receipt_inventory_transaction_id: 8,
            source_inventory_balance_id: 9,
            item_batch_id: 10,
            item_id: 11,
            item_description: Some("Cases".into()),
            item_barcodes: vec!["ITEM-1".into()],
            uom: "case".into(),
            lot: Some("LOT-1".into()),
            serial: None,
            expiration: None,
            quantity: 5,
            source_receiving_location: CrossDockLocation {
                location_id: 12,
                barcode: "RECV-1".into(),
                name: Some("Receiving".into()),
            },
            destination_pick_face: CrossDockLocation {
                location_id: 13,
                barcode: "PICK-1".into(),
                name: Some("Pick face".into()),
            },
        }
    }

    #[test]
    fn controlled_work_requires_exact_scans_in_order() {
        let mut workflow = CrossDockWorkflow::default();
        workflow.restore_current_claim(Some(claim()));
        for (scan, stage) in [
            ("RECV-1", CrossDockScanStage::SourceReceivingLocation),
            ("ITEM-1", CrossDockScanStage::Item),
            ("LOT-1", CrossDockScanStage::Lot),
            ("PICK-1", CrossDockScanStage::DestinationPickFace),
        ] {
            assert_eq!(workflow.expected_scan(), Some(stage));
            workflow.scan_draft_mut().push_str(scan);
            assert!(workflow.submit_scan());
        }
        assert_eq!(workflow.expected_scan(), None);
        assert!(
            workflow
                .begin_confirmation("command".into(), "key".into())
                .is_some()
        );
    }

    #[test]
    fn wrong_scan_clears_draft_without_advancing() {
        let mut workflow = CrossDockWorkflow::default();
        workflow.restore_current_claim(Some(claim()));
        workflow.scan_draft_mut().push_str("WRONG");
        assert!(!workflow.submit_scan());
        assert_eq!(
            workflow.expected_scan(),
            Some(CrossDockScanStage::SourceReceivingLocation)
        );
        assert!(workflow.error().is_some());
    }
}
