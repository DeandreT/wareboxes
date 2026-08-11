use serde::{Deserialize, Serialize};

use crate::cross_dock::{CrossDockClaim, CrossDockCommand, CrossDockConfirmationResult};
use crate::expected_receiving::{ConfirmationIntent, ReceivingCommandIntent};
use crate::outbound_load::OutboundLoadCommand;
use crate::picking::{PickClaim, PickShortageReportResult, PickingCommand};
use crate::replenishment::{
    ReplenishmentClaim, ReplenishmentCommand, ReplenishmentConfirmationResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementKind {
    Loose,
    LicensePlate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementOperation {
    Putaway,
    InventoryRelocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOperation {
    Putaway,
    InventoryRelocation,
    CycleCount,
    Picking,
    Replenishment,
    CrossDock,
}

impl From<MovementOperation> for ClaimOperation {
    fn from(operation: MovementOperation) -> Self {
        match operation {
            MovementOperation::Putaway => Self::Putaway,
            MovementOperation::InventoryRelocation => Self::InventoryRelocation,
        }
    }
}

impl MovementOperation {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Putaway => "Putaway",
            Self::InventoryRelocation => "Relocate",
        }
    }
}

impl MovementKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loose => "Loose",
            Self::LicensePlate => "License plate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub location_id: i64,
    pub name: Option<String>,
    pub barcode: Option<String>,
}

impl Location {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.barcode.clone())
            .unwrap_or_else(|| format!("Location {}", self.location_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MovementWork {
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

impl MovementWork {
    pub const fn kind(&self) -> MovementKind {
        match self {
            Self::Loose { .. } => MovementKind::Loose,
            Self::LicensePlate { .. } => MovementKind::LicensePlate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovementClaimDetails {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub lease_expires_at: String,
    pub source: Option<Location>,
    pub destination: Location,
    pub work: MovementWork,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutawayClaim {
    details: MovementClaimDetails,
}

impl PutawayClaim {
    pub fn new(details: MovementClaimDetails) -> Self {
        Self { details }
    }

    pub const fn details(&self) -> &MovementClaimDetails {
        &self.details
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryRelocationClaim {
    details: MovementClaimDetails,
}

impl InventoryRelocationClaim {
    pub fn new(details: MovementClaimDetails) -> Self {
        Self { details }
    }

    pub const fn details(&self) -> &MovementClaimDetails {
        &self.details
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActiveMovementClaim {
    Putaway(PutawayClaim),
    InventoryRelocation(InventoryRelocationClaim),
}

impl ActiveMovementClaim {
    const fn operation(&self) -> MovementOperation {
        match self {
            Self::Putaway(_) => MovementOperation::Putaway,
            Self::InventoryRelocation(_) => MovementOperation::InventoryRelocation,
        }
    }

    const fn details(&self) -> &MovementClaimDetails {
        match self {
            Self::Putaway(claim) => &claim.details,
            Self::InventoryRelocation(claim) => &claim.details,
        }
    }
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
        workflow: MovementKind,
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
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum InventoryRelocationCommand {
    ClaimNext {
        workflow: MovementKind,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum CycleCountCommand {
    ClaimNext,
    ClaimById {
        task_id: i64,
    },
    Confirm {
        task_id: i64,
        location_barcode: String,
        item_barcode: String,
        license_plate_barcode: Option<String>,
        counted_quantity: i64,
        note: Option<String>,
    },
    Release {
        task_id: i64,
        reason: ReleaseReason,
        note: Option<String>,
    },
}

#[derive(Debug, Clone)]
enum MovementCommand {
    ClaimNext {
        workflow: MovementKind,
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

impl MovementCommand {
    fn into_putaway(self) -> PutawayCommand {
        match self {
            Self::ClaimNext { workflow } => PutawayCommand::ClaimNext { workflow },
            Self::ClaimById { task_id } => PutawayCommand::ClaimById { task_id },
            Self::ConfirmLoose {
                task_id,
                destination_location_barcode,
            } => PutawayCommand::ConfirmLoose {
                task_id,
                destination_location_barcode,
            },
            Self::ConfirmLicensePlate {
                task_id,
                license_plate_barcode,
                destination_location_barcode,
            } => PutawayCommand::ConfirmLicensePlate {
                task_id,
                license_plate_barcode,
                destination_location_barcode,
            },
            Self::Release {
                task_id,
                reason,
                note,
            } => PutawayCommand::Release {
                task_id,
                reason,
                note,
            },
        }
    }

    fn into_relocation(self) -> InventoryRelocationCommand {
        match self {
            Self::ClaimNext { workflow } => InventoryRelocationCommand::ClaimNext { workflow },
            Self::ClaimById { task_id } => InventoryRelocationCommand::ClaimById { task_id },
            Self::ConfirmLoose {
                task_id,
                destination_location_barcode,
            } => InventoryRelocationCommand::ConfirmLoose {
                task_id,
                destination_location_barcode,
            },
            Self::ConfirmLicensePlate {
                task_id,
                license_plate_barcode,
                destination_location_barcode,
            } => InventoryRelocationCommand::ConfirmLicensePlate {
                task_id,
                license_plate_barcode,
                destination_location_barcode,
            },
            Self::Release {
                task_id,
                reason,
                note,
            } => InventoryRelocationCommand::Release {
                task_id,
                reason,
                note,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "workflow", content = "payload", rename_all = "snake_case")]
pub enum RfCommand {
    Putaway(PutawayCommand),
    InventoryRelocation(InventoryRelocationCommand),
    CycleCount(CycleCountCommand),
    Picking(PickingCommand),
    Replenishment(ReplenishmentCommand),
    CrossDock(CrossDockCommand),
    OutboundLoad(OutboundLoadCommand),
    ExpectedReceipt(Box<ReceivingCommandIntent>),
}

impl RfCommand {
    pub const fn movement_operation(&self) -> Option<MovementOperation> {
        match self {
            Self::Putaway(_) => Some(MovementOperation::Putaway),
            Self::InventoryRelocation(_) => Some(MovementOperation::InventoryRelocation),
            Self::CycleCount(_) => None,
            Self::Picking(_) => None,
            Self::Replenishment(_) => None,
            Self::CrossDock(_) => None,
            Self::OutboundLoad(_) => None,
            Self::ExpectedReceipt(_) => None,
        }
    }
}

impl From<PutawayCommand> for RfCommand {
    fn from(command: PutawayCommand) -> Self {
        Self::Putaway(command)
    }
}

impl From<ConfirmationIntent> for RfCommand {
    fn from(intent: ConfirmationIntent) -> Self {
        Self::ExpectedReceipt(Box::new(ReceivingCommandIntent::Expected(Box::new(intent))))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableCommandDraft {
    pub schema_version: u16,
    pub command_id: String,
    pub idempotency_key: String,
    pub command: RfCommand,
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
    PutawayClaimed(Option<Box<PutawayClaim>>),
    InventoryRelocationClaimed(Option<Box<InventoryRelocationClaim>>),
    PutawayConfirmed {
        task_id: i64,
    },
    InventoryRelocationConfirmed {
        task_id: i64,
    },
    PutawayReleased {
        task_id: i64,
    },
    InventoryRelocationReleased {
        task_id: i64,
    },
    CycleCountClaimed(Option<Box<crate::cycle_count::CycleCountClaim>>),
    CycleCountConfirmed {
        task_id: i64,
    },
    CycleCountReleased {
        task_id: i64,
    },
    PickClaimed(Option<Box<PickClaim>>),
    PickConfirmed {
        task_id: i64,
        content_id: i64,
        task_completed: bool,
        order_ready_to_pack: bool,
    },
    PickShortageReported(Box<PickShortageReportResult>),
    PickReleased {
        task_id: i64,
    },
    ReplenishmentClaimed(Option<Box<ReplenishmentClaim>>),
    ReplenishmentConfirmed(Box<ReplenishmentConfirmationResult>),
    ReplenishmentReleased {
        work_id: i64,
    },
    CrossDockClaimed(Option<Box<CrossDockClaim>>),
    CrossDockConfirmed(Box<CrossDockConfirmationResult>),
    CrossDockReleased {
        work_id: i64,
    },
    OutboundCartonMoved(Box<wareboxes_api_contract::v1::MovePackedCartonResponse>),
    InboundUnloadingStarted(crate::expected_receiving::UnloadingStartResult),
    ExpectedReceipt(crate::expected_receiving::ConfirmationResult),
    UnexpectedReceipt(Box<crate::expected_receiving::UnexpectedReceiptResult>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    Applied,
    Ignored,
    Effect(WorkflowEffect),
}

#[derive(Debug, Clone)]
pub struct MovementWorkflow {
    operation: MovementOperation,
    selected_kind: MovementKind,
    claim: Option<ActiveMovementClaim>,
    lane: CommandLane,
    source_verified: bool,
    license_plate_scan: Option<String>,
    scan_draft: String,
    error: Option<String>,
    notice: Option<String>,
    reconcile_reason: Option<String>,
}

impl Default for MovementWorkflow {
    fn default() -> Self {
        Self {
            operation: MovementOperation::Putaway,
            selected_kind: MovementKind::Loose,
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

impl MovementWorkflow {
    pub const fn operation(&self) -> MovementOperation {
        self.operation
    }

    pub fn select_operation(&mut self, operation: MovementOperation) {
        if self.activity() == Activity::Idle {
            self.operation = operation;
            self.notice = None;
            self.error = None;
        }
    }

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

    pub const fn selected_kind(&self) -> MovementKind {
        self.selected_kind
    }

    pub fn select_kind(&mut self, kind: MovementKind) {
        if self.activity() == Activity::Idle {
            self.selected_kind = kind;
            self.notice = None;
            self.error = None;
        }
    }

    pub fn claim(&self) -> Option<&MovementClaimDetails> {
        self.claim.as_ref().map(ActiveMovementClaim::details)
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
        let claim = self.claim.as_ref()?.details();
        if claim
            .source
            .as_ref()
            .is_some_and(|source| source.barcode.is_some())
            && !self.source_verified
        {
            return Some(ScanStage::Source);
        }
        if matches!(claim.work, MovementWork::LicensePlate { .. })
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
            MovementCommand::ClaimNext {
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
            MovementCommand::ClaimById { task_id },
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
        let claim = self.claim.as_ref()?.details();

        match stage {
            ScanStage::Source => {
                let expected = claim
                    .source
                    .as_ref()
                    .and_then(|location| location.barcode.as_deref());
                if expected != Some(scanned.as_str()) {
                    self.reject_scan("Source location does not match this task");
                    return None;
                }
                self.source_verified = true;
                self.accept_scan();
                None
            }
            ScanStage::LicensePlate => {
                let MovementWork::LicensePlate { barcode, .. } = &claim.work else {
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
                if claim.destination.barcode.as_deref() != Some(scanned.as_str()) {
                    self.reject_scan("Destination location does not match this task");
                    return None;
                }
                let command = match &claim.work {
                    MovementWork::Loose { .. } => MovementCommand::ConfirmLoose {
                        task_id: claim.task_id,
                        destination_location_barcode: scanned,
                    },
                    MovementWork::LicensePlate { .. } => {
                        let license_plate_barcode = self.license_plate_scan.clone()?;
                        MovementCommand::ConfirmLicensePlate {
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
        let task_id = self.claim.as_ref()?.details().task_id;
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
            MovementCommand::Release {
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

    pub fn restore_ready_command(
        &mut self,
        record_id: i64,
        draft: DurableCommandDraft,
    ) -> Transition {
        let Some(operation) = draft.command.movement_operation() else {
            return Transition::Ignored;
        };
        if record_id <= 0 || self.reconcile_reason.is_some() {
            return Transition::Ignored;
        }
        self.operation = operation;
        self.lane = CommandLane::Ready(PersistedCommand { record_id, draft });
        Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id })
    }

    pub fn restore_ambiguous_command(
        &mut self,
        record_id: i64,
        draft: DurableCommandDraft,
        message: impl Into<String>,
    ) -> Transition {
        let Some(operation) = draft.command.movement_operation() else {
            return Transition::Ignored;
        };
        if record_id <= 0 || self.reconcile_reason.is_some() {
            return Transition::Ignored;
        }
        self.operation = operation;
        self.lane = CommandLane::Ambiguous {
            command: PersistedCommand { record_id, draft },
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
        let Some(operation) = command.draft.command.movement_operation() else {
            self.require_reconciliation("Recorded result does not match the workflow".into());
            return Transition::Applied;
        };
        if operation != self.operation || !Self::outcome_matches(&command.draft.command, &outcome) {
            self.require_reconciliation("Recorded result does not match the command".into());
            return Transition::Applied;
        }

        match outcome {
            CommandOutcome::PutawayClaimed(claim) => {
                let claim = claim.map(|claim| ActiveMovementClaim::Putaway(*claim));
                if !self.apply_claim_outcome(claim) {
                    return Transition::Applied;
                }
            }
            CommandOutcome::InventoryRelocationClaimed(claim) => {
                let claim = claim.map(|claim| ActiveMovementClaim::InventoryRelocation(*claim));
                if !self.apply_claim_outcome(claim) {
                    return Transition::Applied;
                }
            }
            CommandOutcome::PutawayConfirmed { .. }
            | CommandOutcome::InventoryRelocationConfirmed { .. } => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some(format!("{} confirmed", self.operation.label()));
            }
            CommandOutcome::PutawayReleased { .. }
            | CommandOutcome::InventoryRelocationReleased { .. } => {
                self.claim = None;
                self.reset_scans();
                self.notice = Some(format!("{} returned to the queue", self.operation.label()));
            }
            CommandOutcome::ExpectedReceipt(_)
            | CommandOutcome::InboundUnloadingStarted(_)
            | CommandOutcome::UnexpectedReceipt(_)
            | CommandOutcome::CycleCountClaimed(_)
            | CommandOutcome::CycleCountConfirmed { .. }
            | CommandOutcome::CycleCountReleased { .. }
            | CommandOutcome::PickClaimed(_)
            | CommandOutcome::PickConfirmed { .. }
            | CommandOutcome::PickShortageReported(_)
            | CommandOutcome::PickReleased { .. }
            | CommandOutcome::ReplenishmentClaimed(_)
            | CommandOutcome::ReplenishmentConfirmed(_)
            | CommandOutcome::ReplenishmentReleased { .. }
            | CommandOutcome::CrossDockClaimed(_)
            | CommandOutcome::CrossDockConfirmed(_)
            | CommandOutcome::CrossDockReleased { .. }
            | CommandOutcome::OutboundCartonMoved(_) => {
                self.require_reconciliation("Recorded result does not match the workflow".into());
                return Transition::Applied;
            }
        }
        self.lane = CommandLane::Empty;
        self.error = None;
        Transition::Applied
    }

    fn apply_claim_outcome(&mut self, claim: Option<ActiveMovementClaim>) -> bool {
        if claim
            .as_ref()
            .is_some_and(|claim| claim.operation() != self.operation)
        {
            self.require_reconciliation("Claimed work does not match the command".into());
            return false;
        }
        if claim
            .as_ref()
            .is_some_and(|claim| claim.details().work.kind() != self.selected_kind)
        {
            self.require_reconciliation("Claimed work does not match the selected workflow".into());
            return false;
        }
        self.claim = claim;
        self.reset_scans();
        self.notice = self.claim.is_none().then(|| {
            format!(
                "No {} work is available",
                self.operation.label().to_lowercase()
            )
        });
        true
    }

    pub fn durable_rejection_recorded(
        &mut self,
        record_id: i64,
        message: impl Into<String>,
    ) -> Transition {
        let command = match &self.lane {
            CommandLane::InFlight(command) => command,
            CommandLane::Ambiguous { command, .. } => command,
            _ => return Transition::Ignored,
        };
        if command.record_id != record_id {
            return Transition::Ignored;
        }
        self.lane = CommandLane::Empty;
        self.error = Some(message.into());
        Transition::Applied
    }

    pub fn restore_current_putaway_claim(&mut self, claim: Option<PutawayClaim>) {
        self.restore_current_claim(
            MovementOperation::Putaway,
            claim.map(ActiveMovementClaim::Putaway),
        );
    }

    pub fn restore_current_inventory_relocation_claim(
        &mut self,
        claim: Option<InventoryRelocationClaim>,
    ) {
        self.restore_current_claim(
            MovementOperation::InventoryRelocation,
            claim.map(ActiveMovementClaim::InventoryRelocation),
        );
    }

    fn restore_current_claim(
        &mut self,
        operation: MovementOperation,
        claim: Option<ActiveMovementClaim>,
    ) {
        self.operation = operation;
        self.selected_kind = claim
            .as_ref()
            .map(|claim| claim.details().work.kind())
            .unwrap_or(self.selected_kind);
        self.claim = claim;
        self.lane = CommandLane::Empty;
        self.reconcile_reason = None;
        self.error = None;
        self.notice = None;
        self.reset_scans();
    }

    pub fn require_reconciliation(&mut self, reason: String) {
        self.reconcile_reason = Some(reason);
        self.error = None;
    }

    #[cfg(debug_assertions)]
    pub fn load_debug_claim(&mut self, claim: PutawayClaim) {
        self.operation = MovementOperation::Putaway;
        self.claim = Some(ActiveMovementClaim::Putaway(claim));
        self.lane = CommandLane::Empty;
        self.reconcile_reason = None;
        self.notice = None;
        self.error = None;
        self.reset_scans();
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub fn load_debug_relocation_claim(&mut self, claim: InventoryRelocationClaim) {
        self.operation = MovementOperation::InventoryRelocation;
        self.selected_kind = claim.details().work.kind();
        self.claim = Some(ActiveMovementClaim::InventoryRelocation(claim));
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
        command: MovementCommand,
    ) -> Option<WorkflowEffect> {
        if command_id.trim().is_empty() || idempotency_key.trim().is_empty() {
            self.error = Some("Command identity is unavailable".into());
            return None;
        }
        let command = match self.operation {
            MovementOperation::Putaway => RfCommand::Putaway(command.into_putaway()),
            MovementOperation::InventoryRelocation => {
                RfCommand::InventoryRelocation(command.into_relocation())
            }
        };
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

    fn outcome_matches(command: &RfCommand, outcome: &CommandOutcome) -> bool {
        match (command, outcome) {
            (
                RfCommand::Putaway(PutawayCommand::ClaimNext { .. }),
                CommandOutcome::PutawayClaimed(_),
            )
            | (
                RfCommand::InventoryRelocation(InventoryRelocationCommand::ClaimNext { .. }),
                CommandOutcome::InventoryRelocationClaimed(_),
            ) => true,
            (
                RfCommand::Putaway(PutawayCommand::ClaimById { task_id }),
                CommandOutcome::PutawayClaimed(Some(claim)),
            ) => *task_id == claim.details.task_id,
            (
                RfCommand::InventoryRelocation(InventoryRelocationCommand::ClaimById { task_id }),
                CommandOutcome::InventoryRelocationClaimed(Some(claim)),
            ) => *task_id == claim.details.task_id,
            (
                RfCommand::Putaway(
                    PutawayCommand::ConfirmLoose { task_id, .. }
                    | PutawayCommand::ConfirmLicensePlate { task_id, .. },
                ),
                CommandOutcome::PutawayConfirmed {
                    task_id: outcome_task_id,
                },
            )
            | (
                RfCommand::InventoryRelocation(
                    InventoryRelocationCommand::ConfirmLoose { task_id, .. }
                    | InventoryRelocationCommand::ConfirmLicensePlate { task_id, .. },
                ),
                CommandOutcome::InventoryRelocationConfirmed {
                    task_id: outcome_task_id,
                },
            )
            | (
                RfCommand::Putaway(PutawayCommand::Release { task_id, .. }),
                CommandOutcome::PutawayReleased {
                    task_id: outcome_task_id,
                },
            )
            | (
                RfCommand::InventoryRelocation(InventoryRelocationCommand::Release {
                    task_id, ..
                }),
                CommandOutcome::InventoryRelocationReleased {
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
        PutawayClaim::new(MovementClaimDetails {
            task_id: 42,
            inventory_owner_id: 3,
            facility_id: 4,
            priority: 80,
            instructions: None,
            lease_expires_at: "2026-07-27T01:00:00Z".into(),
            source: Some(Location {
                location_id: 5,
                name: Some("Receiving 1".into()),
                barcode: Some("RECV-01".into()),
            }),
            destination: Location {
                location_id: 6,
                name: Some("A-01-01".into()),
                barcode: Some("A-01-01".into()),
            },
            work: MovementWork::Loose {
                item_description: Some("Widget".into()),
                item_id: 7,
                quantity: 4,
                uom: "case".into(),
                lot: Some("LOT-1".into()),
                serial: None,
            },
        })
    }

    fn license_plate_claim() -> PutawayClaim {
        PutawayClaim::new(MovementClaimDetails {
            task_id: 91,
            inventory_owner_id: 3,
            facility_id: 4,
            priority: 60,
            instructions: None,
            lease_expires_at: "2026-07-27T01:00:00Z".into(),
            source: None,
            destination: Location {
                location_id: 8,
                name: None,
                barcode: Some("B-02-03".into()),
            },
            work: MovementWork::LicensePlate {
                barcode: "LP-91".into(),
                planned_balance_count: 3,
            },
        })
    }

    fn persist_active_claim(workflow: &mut MovementWorkflow, claim: PutawayClaim) {
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
            workflow.durable_outcome_recorded(
                10,
                CommandOutcome::PutawayClaimed(Some(Box::new(claim)))
            ),
            Transition::Applied
        );
    }

    fn persist_active_relocation_claim(
        workflow: &mut MovementWorkflow,
        claim: InventoryRelocationClaim,
    ) {
        workflow.select_operation(MovementOperation::InventoryRelocation);
        let effect = workflow
            .begin_claim_next("relocation-command-1".into(), "relocation-key-1".into())
            .expect("relocation claim should begin");
        let WorkflowEffect::PersistCommand(draft) = &effect else {
            panic!("relocation claim must persist first");
        };
        assert!(matches!(
            draft.command,
            RfCommand::InventoryRelocation(InventoryRelocationCommand::ClaimNext { .. })
        ));
        assert_eq!(
            workflow.command_persisted("relocation-command-1", 11),
            Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id: 11 })
        );
        assert_eq!(workflow.dispatch_started(11), Transition::Applied);
        assert_eq!(
            workflow.durable_outcome_recorded(
                11,
                CommandOutcome::InventoryRelocationClaimed(Some(Box::new(claim)))
            ),
            Transition::Applied
        );
    }

    #[test]
    fn command_must_be_persisted_before_dispatch() {
        let mut workflow = MovementWorkflow::default();
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
        let mut workflow = MovementWorkflow::default();
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
    fn persisted_command_recovery_dispatches_the_original_draft() {
        let mut workflow = MovementWorkflow::default();
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id: "command-1".into(),
            idempotency_key: "key-1".into(),
            command: PutawayCommand::ClaimNext {
                workflow: MovementKind::Loose,
            }
            .into(),
        };

        assert_eq!(
            workflow.restore_ready_command(7, draft.clone()),
            Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id: 7 })
        );
        assert_eq!(workflow.current_draft(), Some(&draft));
        assert_eq!(workflow.activity(), Activity::ReadyToDispatch);
    }

    #[test]
    fn restored_ambiguous_command_requires_an_explicit_check() {
        let mut workflow = MovementWorkflow::default();
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id: "command-1".into(),
            idempotency_key: "key-1".into(),
            command: PutawayCommand::ClaimNext {
                workflow: MovementKind::Loose,
            }
            .into(),
        };

        assert_eq!(
            workflow.restore_ambiguous_command(7, draft.clone(), "check saved scan"),
            Transition::Applied
        );
        assert_eq!(workflow.activity(), Activity::Ambiguous);
        assert_eq!(workflow.current_draft(), Some(&draft));
        assert_eq!(workflow.ambiguous_message(), Some("check saved scan"));
    }

    #[test]
    fn loose_scan_sequence_has_no_fabricated_item_scan() {
        let mut workflow = MovementWorkflow::default();
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
            RfCommand::Putaway(PutawayCommand::ConfirmLoose { task_id: 42, .. })
        ));
    }

    #[test]
    fn relocation_loose_scan_sequence_emits_only_a_relocation_command() {
        let mut workflow = MovementWorkflow::default();
        let claim = InventoryRelocationClaim::new(loose_claim().details().clone());
        persist_active_relocation_claim(&mut workflow, claim);

        *workflow.scan_draft_mut() = "RECV-01".into();
        assert_eq!(workflow.submit_scan("unused".into(), "unused".into()), None);
        *workflow.scan_draft_mut() = "A-01-01".into();
        let WorkflowEffect::PersistCommand(draft) = workflow
            .submit_scan(
                "relocation-confirm-1".into(),
                "relocation-confirm-key".into(),
            )
            .expect("destination scan should persist a relocation confirmation")
        else {
            panic!("relocation confirmation must persist first");
        };
        assert!(matches!(
            draft.command,
            RfCommand::InventoryRelocation(InventoryRelocationCommand::ConfirmLoose {
                task_id: 42,
                ..
            })
        ));
    }

    #[test]
    fn relocation_license_plate_requires_plate_then_destination_scans() {
        let mut workflow = MovementWorkflow::default();
        workflow.select_operation(MovementOperation::InventoryRelocation);
        workflow.select_kind(MovementKind::LicensePlate);
        let mut details = license_plate_claim().details().clone();
        details.source = Some(Location {
            location_id: 7,
            name: Some("A-01-03".into()),
            barcode: Some("A-01-03".into()),
        });
        let claim = InventoryRelocationClaim::new(details);
        persist_active_relocation_claim(&mut workflow, claim);

        assert_eq!(workflow.expected_scan(), Some(ScanStage::Source));
        *workflow.scan_draft_mut() = "A-01-03".into();
        assert_eq!(workflow.submit_scan("unused".into(), "unused".into()), None);
        assert_eq!(workflow.expected_scan(), Some(ScanStage::LicensePlate));
        *workflow.scan_draft_mut() = "LP-91".into();
        assert_eq!(workflow.submit_scan("unused".into(), "unused".into()), None);
        assert_eq!(workflow.expected_scan(), Some(ScanStage::Destination));
        *workflow.scan_draft_mut() = "B-02-03".into();
        let WorkflowEffect::PersistCommand(draft) = workflow
            .submit_scan(
                "relocation-confirm-2".into(),
                "relocation-confirm-key-2".into(),
            )
            .expect("destination scan should persist an LPN relocation")
        else {
            panic!("LPN relocation confirmation must persist first");
        };
        assert!(matches!(
            draft.command,
            RfCommand::InventoryRelocation(
                InventoryRelocationCommand::ConfirmLicensePlate {
                    task_id: 91,
                    ref license_plate_barcode,
                    ref destination_location_barcode,
                }
            ) if license_plate_barcode == "LP-91"
                && destination_location_barcode == "B-02-03"
        ));
    }

    #[test]
    fn relocation_command_rejects_a_putaway_result_variant() {
        let mut workflow = MovementWorkflow::default();
        workflow.select_operation(MovementOperation::InventoryRelocation);
        workflow.begin_claim_next("relocation-command-1".into(), "relocation-key-1".into());
        workflow.command_persisted("relocation-command-1", 12);
        workflow.dispatch_started(12);

        assert_eq!(
            workflow.durable_outcome_recorded(12, CommandOutcome::PutawayClaimed(None)),
            Transition::Applied
        );
        assert_eq!(workflow.activity(), Activity::ReconcileRequired);
    }

    #[test]
    fn license_plate_scan_sequence_is_enforced() {
        let mut workflow = MovementWorkflow::default();
        workflow.select_kind(MovementKind::LicensePlate);
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
        let mut workflow = MovementWorkflow::default();
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
        let mut workflow = MovementWorkflow::default();
        workflow.begin_claim_by_id(42, "command-1".into(), "key-1".into());
        workflow.command_persisted("command-1", 4);
        workflow.dispatch_started(4);

        assert_eq!(
            workflow.durable_outcome_recorded(
                4,
                CommandOutcome::PutawayClaimed(Some(Box::new(loose_claim().with_task_id(99))))
            ),
            Transition::Applied
        );
        assert_eq!(workflow.activity(), Activity::ReconcileRequired);
    }

    #[test]
    fn selected_task_claim_cannot_record_an_empty_result() {
        let mut workflow = MovementWorkflow::default();
        workflow.begin_claim_by_id(42, "command-1".into(), "key-1".into());
        workflow.command_persisted("command-1", 4);
        workflow.dispatch_started(4);

        assert_eq!(
            workflow.durable_outcome_recorded(4, CommandOutcome::PutawayClaimed(None)),
            Transition::Applied
        );
        assert_eq!(workflow.activity(), Activity::ReconcileRequired);
    }

    #[test]
    fn other_release_requires_a_bounded_note() {
        let mut workflow = MovementWorkflow::default();
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
            self.details.task_id = task_id;
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
                workflow: MovementKind::Loose,
            }
            .into(),
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
                workflow: MovementKind::Loose,
            }
            .is_terminal()
        );
    }
}
