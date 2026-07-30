use serde::{Deserialize, Serialize};

use crate::workflow::{
    Activity, CommandOutcome, CycleCountCommand, DurableCommandDraft, PersistedCommand, RfCommand,
    Transition, WorkflowEffect,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountClaim {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub lease_expires_at: String,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<String>,
    pub inventory_balance_id: i64,
    pub license_plate_barcode: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub inventory_status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CountScanStage {
    Location,
    Item,
    LicensePlate,
}

impl CountScanStage {
    pub const fn prompt(self) -> &'static str {
        match self {
            Self::Location => "Scan count location",
            Self::Item => "Scan item",
            Self::LicensePlate => "Scan license plate",
        }
    }
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
pub struct CycleCountWorkflow {
    claim: Option<CycleCountClaim>,
    lane: Lane,
    location_scan: Option<String>,
    item_scan: Option<String>,
    license_plate_scan: Option<String>,
    scan_draft: String,
    quantity_draft: String,
    note_draft: String,
    error: Option<String>,
    notice: Option<String>,
    reconcile_reason: Option<String>,
}

impl Default for CycleCountWorkflow {
    fn default() -> Self {
        Self {
            claim: None,
            lane: Lane::Empty,
            location_scan: None,
            item_scan: None,
            license_plate_scan: None,
            scan_draft: String::new(),
            quantity_draft: String::new(),
            note_draft: String::new(),
            error: None,
            notice: None,
            reconcile_reason: None,
        }
    }
}

impl CycleCountWorkflow {
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

    pub const fn claim(&self) -> Option<&CycleCountClaim> {
        self.claim.as_ref()
    }

    pub fn scan_draft_mut(&mut self) -> &mut String {
        &mut self.scan_draft
    }

    pub fn quantity_draft_mut(&mut self) -> &mut String {
        &mut self.quantity_draft
    }

    pub fn note_draft_mut(&mut self) -> &mut String {
        &mut self.note_draft
    }

    pub fn expected_scan(&self) -> Option<CountScanStage> {
        let claim = self.claim.as_ref()?;
        if self.location_scan.is_none() {
            Some(CountScanStage::Location)
        } else if self.item_scan.is_none() {
            Some(CountScanStage::Item)
        } else if claim.license_plate_barcode.is_some() && self.license_plate_scan.is_none() {
            Some(CountScanStage::LicensePlate)
        } else {
            None
        }
    }

    pub fn begin_claim_next(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        self.begin(CycleCountCommand::ClaimNext, command_id, idempotency_key)
    }

    pub fn begin_claim_by_id(
        &mut self,
        task_id: i64,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if task_id <= 0 {
            self.error = Some("Task ID must be positive".into());
            return None;
        }
        self.begin(
            CycleCountCommand::ClaimById { task_id },
            command_id,
            idempotency_key,
        )
    }

    fn begin(
        &mut self,
        command: CycleCountCommand,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Idle {
            return None;
        }
        self.error = None;
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id,
            idempotency_key,
            command: RfCommand::CycleCount(command),
        };
        self.lane = Lane::Persisting(draft.clone());
        Some(WorkflowEffect::PersistCommand(draft))
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
            CountScanStage::Location => scanned == claim.location_barcode,
            CountScanStage::Item => claim
                .item_barcodes
                .iter()
                .any(|barcode| barcode == &scanned),
            CountScanStage::LicensePlate => {
                claim.license_plate_barcode.as_deref() == Some(scanned.as_str())
            }
        };
        if !accepted {
            self.scan_draft.clear();
            self.error = Some(
                match stage {
                    CountScanStage::Location => "Location does not match this count",
                    CountScanStage::Item => "Item does not match this count",
                    CountScanStage::LicensePlate => "License plate does not match this count",
                }
                .into(),
            );
            return false;
        }
        match stage {
            CountScanStage::Location => self.location_scan = Some(scanned),
            CountScanStage::Item => self.item_scan = Some(scanned),
            CountScanStage::LicensePlate => self.license_plate_scan = Some(scanned),
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
        let counted_quantity = match self.quantity_draft.trim().parse::<i64>() {
            Ok(quantity) if quantity >= 0 => quantity,
            _ => {
                self.error = Some("Enter a whole-number count of zero or more".into());
                return None;
            }
        };
        let note = match self.note_draft.trim() {
            "" => None,
            note if note.chars().count() <= 1_000 => Some(note.to_owned()),
            _ => {
                self.error = Some("Count note cannot exceed 1000 characters".into());
                return None;
            }
        };
        let claim = self.claim.as_ref()?;
        let command = CycleCountCommand::Confirm {
            task_id: claim.task_id,
            location_barcode: self.location_scan.clone()?,
            item_barcode: self.item_scan.clone()?,
            license_plate_barcode: self.license_plate_scan.clone(),
            counted_quantity,
            note,
        };
        self.begin_active(command, command_id, idempotency_key)
    }

    pub fn begin_release(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        let task_id = self.claim.as_ref()?.task_id;
        self.begin_active(
            CycleCountCommand::Release {
                task_id,
                reason: crate::workflow::ReleaseReason::WorkInterrupted,
                note: None,
            },
            command_id,
            idempotency_key,
        )
    }

    fn begin_active(
        &mut self,
        command: CycleCountCommand,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Active {
            return None;
        }
        self.error = None;
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id,
            idempotency_key,
            command: RfCommand::CycleCount(command),
        };
        self.lane = Lane::Persisting(draft.clone());
        Some(WorkflowEffect::PersistCommand(draft))
    }

    pub fn command_persisted(&mut self, command_id: &str, record_id: i64) -> Transition {
        let Lane::Persisting(draft) = &self.lane else {
            return Transition::Ignored;
        };
        if draft.command_id != command_id {
            return Transition::Ignored;
        }
        let command = PersistedCommand {
            record_id,
            draft: draft.clone(),
        };
        self.lane = Lane::Ready(command);
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
        let command_matches = match &self.lane {
            Lane::InFlight(command) | Lane::Ambiguous { command, .. } => {
                command.record_id == record_id
            }
            _ => false,
        };
        if !command_matches {
            return;
        }
        self.lane = Lane::Empty;
        match outcome {
            CommandOutcome::CycleCountClaimed(claim) => {
                self.claim = claim.map(|claim| *claim);
                self.reset_inputs();
                self.notice = self
                    .claim
                    .is_none()
                    .then(|| "No cycle count work is ready".into());
            }
            CommandOutcome::CycleCountConfirmed { task_id } => {
                if self
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.task_id == task_id)
                {
                    self.claim = None;
                    self.reset_inputs();
                    self.notice = Some("Count recorded".into());
                } else {
                    self.require_reconciliation("Count result did not match active work".into());
                }
            }
            CommandOutcome::CycleCountReleased { task_id } => {
                if self
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.task_id == task_id)
                {
                    self.claim = None;
                    self.reset_inputs();
                    self.notice = Some("Count returned to queue".into());
                } else {
                    self.require_reconciliation("Release result did not match active work".into());
                }
            }
            _ => self.require_reconciliation(
                "The saved command returned the wrong workflow result".into(),
            ),
        }
    }

    pub fn durable_rejection_recorded(&mut self, record_id: i64, message: String) {
        let matches = match &self.lane {
            Lane::InFlight(command) | Lane::Ambiguous { command, .. } => {
                command.record_id == record_id
            }
            _ => false,
        };
        if matches {
            self.lane = Lane::Empty;
            self.error = Some(message);
        }
    }

    pub fn restore_current_claim(&mut self, claim: Option<CycleCountClaim>) {
        if matches!(self.lane, Lane::Empty) {
            self.claim = claim;
            self.reset_inputs();
            self.error = None;
        }
    }

    pub fn restore_ready_command(
        &mut self,
        record_id: i64,
        draft: DurableCommandDraft,
    ) -> Transition {
        if !matches!(draft.command, RfCommand::CycleCount(_)) {
            self.require_reconciliation("Saved work is not a cycle count command".into());
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
        if !matches!(draft.command, RfCommand::CycleCount(_)) {
            self.require_reconciliation("Saved work is not a cycle count command".into());
            return;
        }
        self.lane = Lane::Ambiguous {
            command: PersistedCommand { record_id, draft },
            message: message.into(),
        };
    }

    pub fn require_reconciliation(&mut self, reason: String) {
        self.reconcile_reason = Some(reason);
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

    fn reset_inputs(&mut self) {
        self.location_scan = None;
        self.item_scan = None;
        self.license_plate_scan = None;
        self.scan_draft.clear();
        self.quantity_draft.clear();
        self.note_draft.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleCountConfirmationSnapshot {
    pub task_id: i64,
    pub counted_quantity: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(with_plate: bool) -> CycleCountClaim {
        CycleCountClaim {
            task_id: 42,
            inventory_owner_id: 2,
            facility_id: 3,
            priority: 90,
            instructions: None,
            lease_expires_at: "2026-07-30T01:00:00Z".into(),
            location_id: 4,
            location_name: Some("A-01".into()),
            location_barcode: "A-01".into(),
            item_id: 5,
            item_description: Some("Widget".into()),
            item_barcodes: vec!["SKU-1".into(), "UPC-1".into()],
            inventory_balance_id: 6,
            license_plate_barcode: with_plate.then(|| "LP-1".into()),
            uom: "EA".into(),
            lot: None,
            serial: None,
            inventory_status: "available".into(),
        }
    }

    #[test]
    fn scanner_sequence_is_blind_and_emits_typed_confirmation() {
        let mut workflow = CycleCountWorkflow::default();
        workflow.restore_current_claim(Some(claim(false)));
        assert_eq!(workflow.expected_scan(), Some(CountScanStage::Location));
        *workflow.scan_draft_mut() = "A-01".into();
        assert!(workflow.submit_scan());
        *workflow.scan_draft_mut() = "UPC-1".into();
        assert!(workflow.submit_scan());
        assert_eq!(workflow.expected_scan(), None);
        *workflow.quantity_draft_mut() = "0".into();
        let effect = workflow
            .begin_confirmation("count-1".into(), "count-key-1".into())
            .unwrap();
        let WorkflowEffect::PersistCommand(draft) = effect else {
            panic!("confirmation must persist first");
        };
        assert!(matches!(
            draft.command,
            RfCommand::CycleCount(CycleCountCommand::Confirm {
                task_id: 42,
                counted_quantity: 0,
                ..
            })
        ));
    }

    #[test]
    fn license_plate_count_requires_all_three_scans() {
        let mut workflow = CycleCountWorkflow::default();
        workflow.restore_current_claim(Some(claim(true)));
        for (barcode, expected) in [
            ("A-01", CountScanStage::Location),
            ("SKU-1", CountScanStage::Item),
            ("LP-1", CountScanStage::LicensePlate),
        ] {
            assert_eq!(workflow.expected_scan(), Some(expected));
            *workflow.scan_draft_mut() = barcode.into();
            assert!(workflow.submit_scan());
        }
        assert_eq!(workflow.expected_scan(), None);
    }

    #[test]
    fn wrong_item_scan_does_not_advance() {
        let mut workflow = CycleCountWorkflow::default();
        workflow.restore_current_claim(Some(claim(false)));
        *workflow.scan_draft_mut() = "A-01".into();
        assert!(workflow.submit_scan());
        *workflow.scan_draft_mut() = "OTHER".into();
        assert!(!workflow.submit_scan());
        assert_eq!(workflow.expected_scan(), Some(CountScanStage::Item));
    }
}
