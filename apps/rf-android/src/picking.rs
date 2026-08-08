use serde::{Deserialize, Serialize};

use crate::workflow::{
    Activity, CommandOutcome, DurableCommandDraft, PersistedCommand, RfCommand, Transition,
    WorkflowEffect,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickContentState {
    Pending,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickClaimContent {
    pub content_id: i64,
    pub order_line_id: i64,
    pub inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub source_license_plate_id: Option<i64>,
    pub source_license_plate_barcode: Option<String>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub planned_quantity: i64,
    pub state: PickContentState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickClaim {
    pub task_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub order_key: String,
    pub priority: i64,
    pub ship_by: Option<String>,
    pub lease_expires_at: String,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub destination_location_name: Option<String>,
    pub content: PickClaimContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PickScanStage {
    SourceLocation,
    Item,
    SourceLicensePlate,
    DestinationLicensePlate,
}

impl PickScanStage {
    pub const fn prompt(self) -> &'static str {
        match self {
            Self::SourceLocation => "Scan source location",
            Self::Item => "Scan item",
            Self::SourceLicensePlate => "Scan source license plate",
            Self::DestinationLicensePlate => "Scan destination license plate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SourceBlocked,
    InventoryDiscrepancy,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum PickingCommand {
    ClaimNext,
    ClaimById {
        task_id: i64,
    },
    Confirm {
        task_id: i64,
        content_id: i64,
        source_location_barcode: String,
        item_barcode: String,
        source_license_plate_barcode: Option<String>,
        destination_license_plate_barcode: String,
    },
    Release {
        task_id: i64,
        reason: PickReleaseReason,
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
pub struct PickingWorkflow {
    claim: Option<PickClaim>,
    lane: Lane,
    source_location_scan: Option<String>,
    item_scan: Option<String>,
    source_license_plate_scan: Option<String>,
    destination_license_plate_scan: Option<String>,
    scan_draft: String,
    error: Option<String>,
    notice: Option<String>,
    reconcile_reason: Option<String>,
}

impl Default for PickingWorkflow {
    fn default() -> Self {
        Self {
            claim: None,
            lane: Lane::Empty,
            source_location_scan: None,
            item_scan: None,
            source_license_plate_scan: None,
            destination_license_plate_scan: None,
            scan_draft: String::new(),
            error: None,
            notice: None,
            reconcile_reason: None,
        }
    }
}

impl PickingWorkflow {
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

    pub const fn claim(&self) -> Option<&PickClaim> {
        self.claim.as_ref()
    }

    pub fn scan_draft_mut(&mut self) -> &mut String {
        &mut self.scan_draft
    }

    pub fn expected_scan(&self) -> Option<PickScanStage> {
        if self.activity() != Activity::Active {
            return None;
        }
        let content = &self.claim.as_ref()?.content;
        if self.source_location_scan.is_none() {
            return Some(PickScanStage::SourceLocation);
        }
        if self.item_scan.is_none() {
            return Some(PickScanStage::Item);
        }
        if content.source_license_plate_barcode.is_some()
            && self.source_license_plate_scan.is_none()
        {
            return Some(PickScanStage::SourceLicensePlate);
        }
        if self.destination_license_plate_scan.is_none() {
            return Some(PickScanStage::DestinationLicensePlate);
        }
        None
    }

    pub fn begin_claim_next(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        self.begin_idle(PickingCommand::ClaimNext, command_id, idempotency_key)
    }

    pub fn begin_claim_by_id(
        &mut self,
        task_id: i64,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if task_id <= 0 {
            self.error = Some("Pick work ID must be positive".into());
            return None;
        }
        self.begin_idle(
            PickingCommand::ClaimById { task_id },
            command_id,
            idempotency_key,
        )
    }

    pub fn submit_scan(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        let stage = self.expected_scan()?;
        let scanned = self.scan_draft.trim().to_owned();
        if scanned.is_empty() {
            self.error = Some("A scan is required".into());
            return None;
        }
        let content = &self.claim.as_ref()?.content;
        let accepted = match stage {
            PickScanStage::SourceLocation => scanned == content.source_location_barcode,
            PickScanStage::Item => content
                .item_barcodes
                .iter()
                .any(|barcode| barcode == &scanned),
            PickScanStage::SourceLicensePlate => {
                content.source_license_plate_barcode.as_deref() == Some(scanned.as_str())
            }
            PickScanStage::DestinationLicensePlate => content
                .source_license_plate_barcode
                .as_deref()
                .is_none_or(|source| source != scanned),
        };
        if !accepted {
            self.reject_scan(match stage {
                PickScanStage::SourceLocation => "Source location does not match this pick",
                PickScanStage::Item => "Item does not match this pick",
                PickScanStage::SourceLicensePlate => {
                    "Source license plate does not match this pick"
                }
                PickScanStage::DestinationLicensePlate => {
                    "Destination license plate must differ from the source"
                }
            });
            return None;
        }

        match stage {
            PickScanStage::SourceLocation => self.source_location_scan = Some(scanned),
            PickScanStage::Item => self.item_scan = Some(scanned),
            PickScanStage::SourceLicensePlate => self.source_license_plate_scan = Some(scanned),
            PickScanStage::DestinationLicensePlate => {
                self.destination_license_plate_scan = Some(scanned)
            }
        }
        self.scan_draft.clear();
        self.error = None;

        if stage != PickScanStage::DestinationLicensePlate {
            return None;
        }
        let claim = self.claim.as_ref()?;
        self.begin_active(
            PickingCommand::Confirm {
                task_id: claim.task_id,
                content_id: claim.content.content_id,
                source_location_barcode: self.source_location_scan.clone()?,
                item_barcode: self.item_scan.clone()?,
                source_license_plate_barcode: self.source_license_plate_scan.clone(),
                destination_license_plate_barcode: self.destination_license_plate_scan.clone()?,
            },
            command_id,
            idempotency_key,
        )
    }

    pub fn begin_release(
        &mut self,
        command_id: String,
        idempotency_key: String,
        reason: PickReleaseReason,
        note: Option<String>,
    ) -> Option<WorkflowEffect> {
        let task_id = self.claim.as_ref()?.task_id;
        let note = note
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if reason == PickReleaseReason::Other && note.is_none() {
            self.error = Some("A note is required for Other".into());
            return None;
        }
        if note
            .as_ref()
            .is_some_and(|value| value.chars().count() > 500)
        {
            self.error = Some("Release note cannot exceed 500 characters".into());
            return None;
        }
        self.begin_active(
            PickingCommand::Release {
                task_id,
                reason,
                note,
            },
            command_id,
            idempotency_key,
        )
    }

    fn begin_idle(
        &mut self,
        command: PickingCommand,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Idle {
            return None;
        }
        self.begin(command, command_id, idempotency_key)
    }

    fn begin_active(
        &mut self,
        command: PickingCommand,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Active {
            return None;
        }
        self.begin(command, command_id, idempotency_key)
    }

    fn begin(
        &mut self,
        command: PickingCommand,
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
            command: RfCommand::Picking(command),
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
        let draft = match &self.lane {
            Lane::InFlight(command) | Lane::Ambiguous { command, .. }
                if command.record_id == record_id =>
            {
                command.draft.clone()
            }
            _ => return,
        };
        if !outcome_matches(&draft.command, &outcome) {
            self.require_reconciliation("Pick result did not match the saved command".into());
            return;
        }

        self.lane = Lane::Empty;
        match outcome {
            CommandOutcome::PickClaimed(claim) => {
                self.claim = claim.map(|claim| *claim);
                self.reset_scans();
                self.notice = self
                    .claim
                    .is_none()
                    .then(|| "No pick work is ready".to_owned());
            }
            CommandOutcome::PickConfirmed {
                order_ready_to_pack,
                ..
            } => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some(if order_ready_to_pack {
                    "Pick confirmed; order picking is complete".into()
                } else {
                    "Pick confirmed".into()
                });
            }
            CommandOutcome::PickReleased { .. } => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some("Pick returned to the queue".into());
            }
            _ => unreachable!("pick outcome was validated before application"),
        }
        self.error = None;
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

    pub fn restore_current_claim(&mut self, claim: Option<PickClaim>) {
        if matches!(self.lane, Lane::Empty) {
            self.claim = claim;
            self.reset_scans();
            self.error = None;
            self.reconcile_reason = None;
        }
    }

    pub fn restore_ready_command(
        &mut self,
        record_id: i64,
        draft: DurableCommandDraft,
    ) -> Transition {
        if !matches!(draft.command, RfCommand::Picking(_)) {
            self.require_reconciliation("Saved work is not a picking command".into());
            return Transition::Ignored;
        }
        if record_id <= 0 {
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
        if !matches!(draft.command, RfCommand::Picking(_)) {
            self.require_reconciliation("Saved work is not a picking command".into());
            return;
        }
        if record_id <= 0 {
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
    pub fn load_debug_claim(&mut self, claim: PickClaim) {
        self.claim = Some(claim);
        self.lane = Lane::Empty;
        self.error = None;
        self.notice = None;
        self.reconcile_reason = None;
        self.reset_scans();
    }

    fn reject_scan(&mut self, message: &str) {
        self.scan_draft.clear();
        self.error = Some(message.to_owned());
    }

    fn reset_scans(&mut self) {
        self.source_location_scan = None;
        self.item_scan = None;
        self.source_license_plate_scan = None;
        self.destination_license_plate_scan = None;
        self.scan_draft.clear();
    }
}

fn outcome_matches(command: &RfCommand, outcome: &CommandOutcome) -> bool {
    match (command, outcome) {
        (RfCommand::Picking(PickingCommand::ClaimNext), CommandOutcome::PickClaimed(_)) => true,
        (
            RfCommand::Picking(PickingCommand::ClaimById { task_id }),
            CommandOutcome::PickClaimed(Some(claim)),
        ) => *task_id == claim.task_id,
        (
            RfCommand::Picking(PickingCommand::Confirm {
                task_id,
                content_id,
                ..
            }),
            CommandOutcome::PickConfirmed {
                task_id: outcome_task_id,
                content_id: outcome_content_id,
                ..
            },
        ) => task_id == outcome_task_id && content_id == outcome_content_id,
        (
            RfCommand::Picking(PickingCommand::Release { task_id, .. }),
            CommandOutcome::PickReleased {
                task_id: outcome_task_id,
            },
        ) => task_id == outcome_task_id,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(with_source_plate: bool) -> PickClaim {
        PickClaim {
            task_id: 41,
            order_id: 51,
            inventory_owner_id: 2,
            facility_id: 3,
            order_key: "SO-1051".into(),
            priority: 90,
            ship_by: Some("2026-08-09T20:00:00Z".into()),
            lease_expires_at: "2026-08-08T20:00:00Z".into(),
            destination_location_id: 9,
            destination_location_barcode: "STAGE-01".into(),
            destination_location_name: Some("Outbound stage 1".into()),
            content: PickClaimContent {
                content_id: 61,
                order_line_id: 71,
                inventory_allocation_id: 81,
                source_inventory_balance_id: 91,
                item_batch_id: 101,
                source_location_id: 8,
                source_location_barcode: "A-01-02".into(),
                source_location_name: Some("Forward A-01-02".into()),
                source_license_plate_id: with_source_plate.then_some(12),
                source_license_plate_barcode: with_source_plate.then(|| "LP-SOURCE".into()),
                item_id: 111,
                item_description: Some("Case-picked filters".into()),
                item_barcodes: vec!["CASE-111".into(), "0012345678905".into()],
                uom: "case".into(),
                lot: Some("LOT-8".into()),
                serial: None,
                expiration: Some("2027-03-01T00:00:00Z".into()),
                planned_quantity: 4,
                state: PickContentState::Pending,
            },
        }
    }

    fn activate(workflow: &mut PickingWorkflow, claim: PickClaim) {
        let effect = workflow
            .begin_claim_next("claim-command".into(), "claim-key".into())
            .unwrap();
        assert!(matches!(effect, WorkflowEffect::PersistCommand(_)));
        assert!(matches!(
            workflow.command_persisted("claim-command", 7),
            Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id: 7 })
        ));
        workflow.dispatch_started(7);
        workflow.durable_outcome_recorded(7, CommandOutcome::PickClaimed(Some(Box::new(claim))));
    }

    fn scan(workflow: &mut PickingWorkflow, value: &str) -> Option<WorkflowEffect> {
        *workflow.scan_draft_mut() = value.into();
        workflow.submit_scan("confirm-command".into(), "confirm-key".into())
    }

    #[test]
    fn loose_pick_enforces_source_item_and_destination_plate_sequence() {
        let mut workflow = PickingWorkflow::default();
        activate(&mut workflow, claim(false));

        assert_eq!(
            workflow.expected_scan(),
            Some(PickScanStage::SourceLocation)
        );
        assert_eq!(scan(&mut workflow, "A-01-02"), None);
        assert_eq!(workflow.expected_scan(), Some(PickScanStage::Item));
        assert_eq!(scan(&mut workflow, "CASE-111"), None);
        assert_eq!(
            workflow.expected_scan(),
            Some(PickScanStage::DestinationLicensePlate)
        );

        let WorkflowEffect::PersistCommand(draft) = scan(&mut workflow, "LP-DEST").unwrap() else {
            panic!("destination plate must persist the pick confirmation");
        };
        assert!(matches!(
            draft.command,
            RfCommand::Picking(PickingCommand::Confirm {
                task_id: 41,
                content_id: 61,
                ref source_location_barcode,
                ref item_barcode,
                source_license_plate_barcode: None,
                ref destination_license_plate_barcode,
            }) if source_location_barcode == "A-01-02"
                && item_barcode == "CASE-111"
                && destination_license_plate_barcode == "LP-DEST"
        ));
    }

    #[test]
    fn source_license_plate_is_required_and_cannot_be_reused_as_destination() {
        let mut workflow = PickingWorkflow::default();
        activate(&mut workflow, claim(true));

        scan(&mut workflow, "A-01-02");
        scan(&mut workflow, "0012345678905");
        assert_eq!(
            workflow.expected_scan(),
            Some(PickScanStage::SourceLicensePlate)
        );
        scan(&mut workflow, "LP-SOURCE");
        assert_eq!(
            workflow.expected_scan(),
            Some(PickScanStage::DestinationLicensePlate)
        );
        assert_eq!(scan(&mut workflow, "LP-SOURCE"), None);
        assert_eq!(
            workflow.error(),
            Some("Destination license plate must differ from the source")
        );
        assert_eq!(
            workflow.expected_scan(),
            Some(PickScanStage::DestinationLicensePlate)
        );
    }

    #[test]
    fn ambiguous_confirmation_retries_the_same_durable_record() {
        let mut workflow = PickingWorkflow::default();
        activate(&mut workflow, claim(false));
        scan(&mut workflow, "A-01-02");
        scan(&mut workflow, "CASE-111");
        scan(&mut workflow, "LP-DEST");
        workflow.command_persisted("confirm-command", 17);
        workflow.dispatch_started(17);
        workflow.dispatch_ambiguous(17, "connection ended after send");

        assert_eq!(
            workflow.retry_ambiguous(),
            Some(WorkflowEffect::DispatchPersistedCommand { record_id: 17 })
        );
    }

    #[test]
    fn mismatched_confirmation_result_requires_reconciliation() {
        let mut workflow = PickingWorkflow::default();
        activate(&mut workflow, claim(false));
        scan(&mut workflow, "A-01-02");
        scan(&mut workflow, "CASE-111");
        scan(&mut workflow, "LP-DEST");
        workflow.command_persisted("confirm-command", 17);
        workflow.dispatch_started(17);
        workflow.durable_outcome_recorded(
            17,
            CommandOutcome::PickConfirmed {
                task_id: 999,
                content_id: 61,
                task_completed: true,
                order_ready_to_pack: false,
            },
        );

        assert_eq!(workflow.activity(), Activity::ReconcileRequired);
        assert!(workflow.claim().is_some());
    }
}
