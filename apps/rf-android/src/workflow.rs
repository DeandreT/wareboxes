use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayKind {
    Loose,
    LicensePlate,
}

impl PutawayKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loose => "Loose",
            Self::LicensePlate => "License plate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub name: Option<String>,
    pub barcode: String,
}

impl Location {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.barcode)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutawayWork {
    Loose {
        item_description: Option<String>,
        item_id: i64,
        quantity: i64,
        uom: String,
        lot: Option<String>,
        serial: Option<String>,
    },
    LicensePlate {
        barcode: String,
        planned_balance_count: i64,
    },
}

impl PutawayWork {
    pub const fn kind(&self) -> PutawayKind {
        match self {
            Self::Loose { .. } => PutawayKind::Loose,
            Self::LicensePlate { .. } => PutawayKind::LicensePlate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutawayClaim {
    pub task_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub lease_expires_at: String,
    pub source: Option<Location>,
    pub destination: Location,
    pub work: PutawayWork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScanStage {
    Source,
    LicensePlate,
    Destination,
}

impl ScanStage {
    pub const fn prompt(self) -> &'static str {
        match self {
            Self::Source => "Scan source location",
            Self::LicensePlate => "Scan license plate",
            Self::Destination => "Scan destination location",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    DestinationBlocked,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum PutawayCommand {
    ClaimNext {
        workflow: PutawayKind,
    },
    ClaimById {
        task_id: i64,
    },
    ConfirmLoose {
        task_id: i64,
        destination_location_barcode: String,
    },
    ConfirmLicensePlate {
        task_id: i64,
        license_plate_barcode: String,
        destination_location_barcode: String,
    },
    Release {
        task_id: i64,
        reason: ReleaseReason,
        note: Option<String>,
    },
}

impl PutawayCommand {
    #[cfg(test)]
    const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::ConfirmLoose { .. } | Self::ConfirmLicensePlate { .. } | Self::Release { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableCommandDraft {
    pub schema_version: u16,
    pub command_id: String,
    pub idempotency_key: String,
    pub command: PutawayCommand,
}

impl DurableCommandDraft {
    pub fn canonical_payload(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedCommand {
    pub record_id: i64,
    pub draft: DurableCommandDraft,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowEffect {
    PersistCommand(DurableCommandDraft),
    DispatchPersistedCommand { record_id: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Active,
    Persisting,
    ReadyToDispatch,
    InFlight,
    Ambiguous,
    ReconcileRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandLane {
    Empty,
    Persisting(DurableCommandDraft),
    Ready(PersistedCommand),
    InFlight(PersistedCommand),
    Ambiguous {
        command: PersistedCommand,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Claimed(Option<Box<PutawayClaim>>),
    Confirmed { task_id: i64 },
    Released { task_id: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    Applied,
    Ignored,
    Effect(WorkflowEffect),
}

#[derive(Debug, Clone)]
pub struct PutawayWorkflow {
    selected_kind: PutawayKind,
    claim: Option<PutawayClaim>,
    lane: CommandLane,
    source_verified: bool,
    license_plate_scan: Option<String>,
    scan_draft: String,
    error: Option<String>,
    notice: Option<String>,
    reconcile_reason: Option<String>,
}

impl Default for PutawayWorkflow {
    fn default() -> Self {
        Self {
            selected_kind: PutawayKind::Loose,
            claim: None,
            lane: CommandLane::Empty,
            source_verified: false,
            license_plate_scan: None,
            scan_draft: String::new(),
            error: None,
            notice: None,
            reconcile_reason: None,
        }
    }
}

impl PutawayWorkflow {
    pub fn activity(&self) -> Activity {
        if self.reconcile_reason.is_some() {
            return Activity::ReconcileRequired;
        }
        match self.lane {
            CommandLane::Persisting(_) => Activity::Persisting,
            CommandLane::Ready(_) => Activity::ReadyToDispatch,
            CommandLane::InFlight(_) => Activity::InFlight,
            CommandLane::Ambiguous { .. } => Activity::Ambiguous,
            CommandLane::Empty if self.claim.is_some() => Activity::Active,
            CommandLane::Empty => Activity::Idle,
        }
    }

    pub const fn selected_kind(&self) -> PutawayKind {
        self.selected_kind
    }

    pub fn select_kind(&mut self, kind: PutawayKind) {
        if self.activity() == Activity::Idle {
            self.selected_kind = kind;
            self.notice = None;
            self.error = None;
        }
    }

    pub fn claim(&self) -> Option<&PutawayClaim> {
        self.claim.as_ref()
    }

    pub fn scan_draft_mut(&mut self) -> &mut String {
        &mut self.scan_draft
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn reconcile_reason(&self) -> Option<&str> {
        self.reconcile_reason.as_deref()
    }

    pub fn ambiguous_message(&self) -> Option<&str> {
        match &self.lane {
            CommandLane::Ambiguous { message, .. } => Some(message),
            _ => None,
        }
    }

    pub fn expected_scan(&self) -> Option<ScanStage> {
        if self.activity() != Activity::Active {
            return None;
        }
        let claim = self.claim.as_ref()?;
        if claim.source.is_some() && !self.source_verified {
            return Some(ScanStage::Source);
        }
        if matches!(claim.work, PutawayWork::LicensePlate { .. })
            && self.license_plate_scan.is_none()
        {
            return Some(ScanStage::LicensePlate);
        }
        Some(ScanStage::Destination)
    }

    pub fn begin_claim_next(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Idle {
            return None;
        }
        self.begin_command(
            command_id,
            idempotency_key,
            PutawayCommand::ClaimNext {
                workflow: self.selected_kind,
            },
        )
    }

    pub fn begin_claim_by_id(
        &mut self,
        task_id: i64,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Idle {
            return None;
        }
        if task_id <= 0 {
            self.error = Some("Task ID must be positive".into());
            return None;
        }
        self.begin_command(
            command_id,
            idempotency_key,
            PutawayCommand::ClaimById { task_id },
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
            self.error = Some("Scan cannot be empty".into());
            return None;
        }
        let claim = self.claim.as_ref()?;

        match stage {
            ScanStage::Source => {
                let expected = claim
                    .source
                    .as_ref()
                    .map(|location| location.barcode.as_str());
                if expected != Some(scanned.as_str()) {
                    self.reject_scan("Source location does not match this task");
                    return None;
                }
                self.source_verified = true;
                self.accept_scan();
                None
            }
            ScanStage::LicensePlate => {
                let PutawayWork::LicensePlate { barcode, .. } = &claim.work else {
                    self.require_reconciliation(
                        "Active work type changed while scanning".to_owned(),
                    );
                    return None;
                };
                if barcode != &scanned {
                    self.reject_scan("License plate does not match this task");
                    return None;
                }
                self.license_plate_scan = Some(scanned);
                self.accept_scan();
                None
            }
            ScanStage::Destination => {
                if claim.destination.barcode != scanned {
                    self.reject_scan("Destination location does not match this task");
                    return None;
                }
                let command = match &claim.work {
                    PutawayWork::Loose { .. } => PutawayCommand::ConfirmLoose {
                        task_id: claim.task_id,
                        destination_location_barcode: scanned,
                    },
                    PutawayWork::LicensePlate { .. } => {
                        let license_plate_barcode = self.license_plate_scan.clone()?;
                        PutawayCommand::ConfirmLicensePlate {
                            task_id: claim.task_id,
                            license_plate_barcode,
                            destination_location_barcode: scanned,
                        }
                    }
                };
                self.accept_scan();
                self.begin_command(command_id, idempotency_key, command)
            }
        }
    }

    pub fn begin_release(
        &mut self,
        command_id: String,
        idempotency_key: String,
        reason: ReleaseReason,
        note: Option<String>,
    ) -> Option<WorkflowEffect> {
        if self.activity() != Activity::Active {
            return None;
        }
        let task_id = self.claim.as_ref()?.task_id;
        let note = note
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if reason == ReleaseReason::Other && note.is_none() {
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
        self.begin_command(
            command_id,
            idempotency_key,
            PutawayCommand::Release {
                task_id,
                reason,
                note,
            },
        )
    }

    pub fn command_persisted(&mut self, command_id: &str, record_id: i64) -> Transition {
        let CommandLane::Persisting(draft) = &self.lane else {
            return Transition::Ignored;
        };
        if draft.command_id != command_id || record_id <= 0 {
            return Transition::Ignored;
        }
        let command = PersistedCommand {
            record_id,
            draft: draft.clone(),
        };
        self.lane = CommandLane::Ready(command);
        Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id })
    }

    pub fn dispatch_started(&mut self, record_id: i64) -> Transition {
        let CommandLane::Ready(command) = &self.lane else {
            return Transition::Ignored;
        };
        if command.record_id != record_id {
            return Transition::Ignored;
        }
        self.lane = CommandLane::InFlight(command.clone());
        Transition::Applied
    }

    pub fn dispatch_ambiguous(&mut self, record_id: i64, message: impl Into<String>) -> Transition {
        let CommandLane::InFlight(command) = &self.lane else {
            return Transition::Ignored;
        };
        if command.record_id != record_id {
            return Transition::Ignored;
        }
        self.lane = CommandLane::Ambiguous {
            command: command.clone(),
            message: message.into(),
        };
        Transition::Applied
    }

    pub fn retry_ambiguous(&mut self) -> Option<WorkflowEffect> {
        let CommandLane::Ambiguous { command, .. } = &self.lane else {
            return None;
        };
        let record_id = command.record_id;
        self.lane = CommandLane::Ready(command.clone());
        Some(WorkflowEffect::DispatchPersistedCommand { record_id })
    }

    pub fn durable_outcome_recorded(
        &mut self,
        record_id: i64,
        outcome: CommandOutcome,
    ) -> Transition {
        let command = match &self.lane {
            CommandLane::InFlight(command) => command.clone(),
            CommandLane::Ambiguous { command, .. } => command.clone(),
            _ => return Transition::Ignored,
        };
        if command.record_id != record_id {
            return Transition::Ignored;
        }
        if !Self::outcome_matches(&command.draft.command, &outcome) {
            self.require_reconciliation("Recorded result does not match the command".into());
            return Transition::Applied;
        }

        match outcome {
            CommandOutcome::Claimed(claim) => {
                if claim
                    .as_ref()
                    .is_some_and(|claim| claim.work.kind() != self.selected_kind)
                {
                    self.require_reconciliation(
                        "Claimed work does not match the selected workflow".into(),
                    );
                    return Transition::Applied;
                }
                self.claim = claim.map(|claim| *claim);
                self.reset_scans();
                self.notice = self
                    .claim
                    .is_none()
                    .then(|| "No putaway work is available".to_owned());
            }
            CommandOutcome::Confirmed { .. } => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some("Putaway confirmed".into());
            }
            CommandOutcome::Released { .. } => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some("Putaway returned to the queue".into());
            }
        }
        self.lane = CommandLane::Empty;
        self.error = None;
        Transition::Applied
    }

    pub fn require_reconciliation(&mut self, reason: String) {
        self.reconcile_reason = Some(reason);
        self.error = None;
    }

    #[cfg(debug_assertions)]
    pub fn load_debug_claim(&mut self, claim: PutawayClaim) {
        self.claim = Some(claim);
        self.lane = CommandLane::Empty;
        self.reconcile_reason = None;
        self.notice = None;
        self.error = None;
        self.reset_scans();
    }

    fn begin_command(
        &mut self,
        command_id: String,
        idempotency_key: String,
        command: PutawayCommand,
    ) -> Option<WorkflowEffect> {
        if command_id.trim().is_empty() || idempotency_key.trim().is_empty() {
            self.error = Some("Command identity is unavailable".into());
            return None;
        }
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id,
            idempotency_key,
            command,
        };
        self.lane = CommandLane::Persisting(draft.clone());
        self.error = None;
        self.notice = None;
        Some(WorkflowEffect::PersistCommand(draft))
    }

    fn outcome_matches(command: &PutawayCommand, outcome: &CommandOutcome) -> bool {
        match (command, outcome) {
            (PutawayCommand::ClaimNext { .. }, CommandOutcome::Claimed(_)) => true,
            (PutawayCommand::ClaimById { task_id }, CommandOutcome::Claimed(Some(claim))) => {
                *task_id == claim.task_id
            }
            (
                PutawayCommand::ConfirmLoose { task_id, .. }
                | PutawayCommand::ConfirmLicensePlate { task_id, .. },
                CommandOutcome::Confirmed {
                    task_id: outcome_task_id,
                },
            )
            | (
                PutawayCommand::Release { task_id, .. },
                CommandOutcome::Released {
                    task_id: outcome_task_id,
                },
            ) => task_id == outcome_task_id,
            _ => false,
        }
    }

    fn accept_scan(&mut self) {
        self.scan_draft.clear();
        self.error = None;
    }

    fn reject_scan(&mut self, message: &str) {
        self.scan_draft.clear();
        self.error = Some(message.to_owned());
    }

    fn reset_scans(&mut self) {
        self.source_verified = false;
        self.license_plate_scan = None;
        self.scan_draft.clear();
    }

    #[cfg(test)]
    fn current_draft(&self) -> Option<&DurableCommandDraft> {
        match &self.lane {
            CommandLane::Persisting(draft) => Some(draft),
            CommandLane::Ready(command)
            | CommandLane::InFlight(command)
            | CommandLane::Ambiguous { command, .. } => Some(&command.draft),
            CommandLane::Empty => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loose_claim() -> PutawayClaim {
        PutawayClaim {
            task_id: 42,
            priority: 80,
            instructions: None,
            lease_expires_at: "2026-07-27T01:00:00Z".into(),
            source: Some(Location {
                name: Some("Receiving 1".into()),
                barcode: "RECV-01".into(),
            }),
            destination: Location {
                name: Some("A-01-01".into()),
                barcode: "A-01-01".into(),
            },
            work: PutawayWork::Loose {
                item_description: Some("Widget".into()),
                item_id: 7,
                quantity: 4,
                uom: "case".into(),
                lot: Some("LOT-1".into()),
                serial: None,
            },
        }
    }

    fn license_plate_claim() -> PutawayClaim {
        PutawayClaim {
            task_id: 91,
            priority: 60,
            instructions: None,
            lease_expires_at: "2026-07-27T01:00:00Z".into(),
            source: None,
            destination: Location {
                name: None,
                barcode: "B-02-03".into(),
            },
            work: PutawayWork::LicensePlate {
                barcode: "LP-91".into(),
                planned_balance_count: 3,
            },
        }
    }

    fn persist_active_claim(workflow: &mut PutawayWorkflow, claim: PutawayClaim) {
        let effect = workflow
            .begin_claim_next("command-1".into(), "key-1".into())
            .expect("claim should begin");
        assert!(matches!(effect, WorkflowEffect::PersistCommand(_)));
        let dispatch = workflow.command_persisted("command-1", 10);
        assert_eq!(
            dispatch,
            Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id: 10 })
        );
        assert_eq!(workflow.dispatch_started(10), Transition::Applied);
        assert_eq!(
            workflow.durable_outcome_recorded(10, CommandOutcome::Claimed(Some(Box::new(claim)))),
            Transition::Applied
        );
    }

    #[test]
    fn command_must_be_persisted_before_dispatch() {
        let mut workflow = PutawayWorkflow::default();
        let effect = workflow
            .begin_claim_next("command-1".into(), "key-1".into())
            .expect("claim should begin");

        assert!(matches!(effect, WorkflowEffect::PersistCommand(_)));
        assert_eq!(workflow.activity(), Activity::Persisting);
        assert_eq!(workflow.dispatch_started(1), Transition::Ignored);
        assert_eq!(
            workflow.command_persisted("command-1", 1),
            Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id: 1 })
        );
    }

    #[test]
    fn ambiguous_retry_reuses_the_exact_durable_command() {
        let mut workflow = PutawayWorkflow::default();
        workflow.begin_claim_next("command-1".into(), "key-1".into());
        workflow.command_persisted("command-1", 7);
        workflow.dispatch_started(7);
        let before = workflow.current_draft().cloned();

        workflow.dispatch_ambiguous(7, "connection closed");
        assert_eq!(workflow.activity(), Activity::Ambiguous);
        assert_eq!(
            workflow.retry_ambiguous(),
            Some(WorkflowEffect::DispatchPersistedCommand { record_id: 7 })
        );
        assert_eq!(workflow.current_draft(), before.as_ref());
    }

    #[test]
    fn loose_scan_sequence_has_no_fabricated_item_scan() {
        let mut workflow = PutawayWorkflow::default();
        persist_active_claim(&mut workflow, loose_claim());
        assert_eq!(workflow.expected_scan(), Some(ScanStage::Source));

        *workflow.scan_draft_mut() = "RECV-01".into();
        assert_eq!(workflow.submit_scan("unused".into(), "unused".into()), None);
        assert_eq!(workflow.expected_scan(), Some(ScanStage::Destination));

        *workflow.scan_draft_mut() = "A-01-01".into();
        let effect = workflow
            .submit_scan("confirm-1".into(), "confirm-key".into())
            .expect("destination should produce a command");
        let WorkflowEffect::PersistCommand(draft) = effect else {
            panic!("confirmation must persist first");
        };
        assert!(matches!(
            draft.command,
            PutawayCommand::ConfirmLoose { task_id: 42, .. }
        ));
    }

    #[test]
    fn license_plate_scan_sequence_is_enforced() {
        let mut workflow = PutawayWorkflow::default();
        workflow.select_kind(PutawayKind::LicensePlate);
        persist_active_claim(&mut workflow, license_plate_claim());
        assert_eq!(workflow.expected_scan(), Some(ScanStage::LicensePlate));

        *workflow.scan_draft_mut() = "WRONG".into();
        assert!(
            workflow
                .submit_scan("unused".into(), "unused".into())
                .is_none()
        );
        assert_eq!(workflow.expected_scan(), Some(ScanStage::LicensePlate));

        *workflow.scan_draft_mut() = "LP-91".into();
        workflow.submit_scan("unused".into(), "unused".into());
        assert_eq!(workflow.expected_scan(), Some(ScanStage::Destination));
    }

    #[test]
    fn ambiguous_terminal_command_keeps_the_claim() {
        let mut workflow = PutawayWorkflow::default();
        persist_active_claim(&mut workflow, loose_claim());
        *workflow.scan_draft_mut() = "RECV-01".into();
        workflow.submit_scan("unused".into(), "unused".into());
        *workflow.scan_draft_mut() = "A-01-01".into();
        workflow.submit_scan("confirm-1".into(), "confirm-key".into());
        workflow.command_persisted("confirm-1", 22);
        workflow.dispatch_started(22);

        workflow.dispatch_ambiguous(22, "timeout after send");

        assert_eq!(workflow.activity(), Activity::Ambiguous);
        assert_eq!(workflow.claim().map(|claim| claim.task_id), Some(42));
    }

    #[test]
    fn mismatched_durable_result_requires_reconciliation() {
        let mut workflow = PutawayWorkflow::default();
        workflow.begin_claim_by_id(42, "command-1".into(), "key-1".into());
        workflow.command_persisted("command-1", 4);
        workflow.dispatch_started(4);

        assert_eq!(
            workflow.durable_outcome_recorded(
                4,
                CommandOutcome::Claimed(Some(Box::new(loose_claim().with_task_id(99))))
            ),
            Transition::Applied
        );
        assert_eq!(workflow.activity(), Activity::ReconcileRequired);
    }

    #[test]
    fn selected_task_claim_cannot_record_an_empty_result() {
        let mut workflow = PutawayWorkflow::default();
        workflow.begin_claim_by_id(42, "command-1".into(), "key-1".into());
        workflow.command_persisted("command-1", 4);
        workflow.dispatch_started(4);

        assert_eq!(
            workflow.durable_outcome_recorded(4, CommandOutcome::Claimed(None)),
            Transition::Applied
        );
        assert_eq!(workflow.activity(), Activity::ReconcileRequired);
    }

    #[test]
    fn other_release_requires_a_bounded_note() {
        let mut workflow = PutawayWorkflow::default();
        persist_active_claim(&mut workflow, loose_claim());

        assert!(
            workflow
                .begin_release(
                    "release-1".into(),
                    "key-1".into(),
                    ReleaseReason::Other,
                    None,
                )
                .is_none()
        );
        assert_eq!(workflow.activity(), Activity::Active);
    }

    trait ClaimTestExt {
        fn with_task_id(self, task_id: i64) -> Self;
    }

    impl ClaimTestExt for PutawayClaim {
        fn with_task_id(mut self, task_id: i64) -> Self {
            self.task_id = task_id;
            self
        }
    }

    #[test]
    fn canonical_payload_retains_command_and_idempotency_identity() {
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id: "command-7".into(),
            idempotency_key: "key-7".into(),
            command: PutawayCommand::ClaimNext {
                workflow: PutawayKind::Loose,
            },
        };

        let payload = draft.canonical_payload().expect("payload should encode");
        let decoded: DurableCommandDraft =
            serde_json::from_slice(&payload).expect("payload should decode");
        assert_eq!(decoded, draft);
    }

    #[test]
    fn command_terminal_classification_is_explicit() {
        assert!(
            PutawayCommand::Release {
                task_id: 1,
                reason: ReleaseReason::SafetyIssue,
                note: None,
            }
            .is_terminal()
        );
        assert!(
            !PutawayCommand::ClaimNext {
                workflow: PutawayKind::Loose,
            }
            .is_terminal()
        );
    }
}
