use serde::{Deserialize, Serialize};

use crate::workflow::{
    Activity, CommandOutcome, DurableCommandDraft, PersistedCommand, RfCommand, Transition,
    WorkflowEffect,
};

mod shortage;

pub use shortage::{
    PickControlledEvidence, PickShortageCommand, PickShortageDisposition, PickShortageDraft,
    PickShortageOutcome, PickShortageReason, PickShortageReportResult, PickShortageStatus,
};
use shortage::{expected_shortage_scan, validate_shortage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickContentState {
    Pending,
    Completed,
    Shorted,
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
    pub order_revision: i64,
    pub priority: i64,
    pub ship_by: Option<String>,
    pub lease_expires_at: String,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub destination_location_name: Option<String>,
    pub execution: PickExecutionEvidence,
    pub pick_policy: PickDecisionPolicy,
    pub suggested_destination_license_plate_barcode: Option<String>,
    pub content: PickClaimContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickExecutionMethod {
    Discrete,
    Case,
    Pallet,
    ClusterCart,
    BatchCart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickExecutionEvidence {
    pub method: PickExecutionMethod,
    pub cluster_id: Option<i64>,
    pub cart_barcode: Option<String>,
    pub slot_code: Option<String>,
    pub sequence: Option<i64>,
    pub task_count: Option<i64>,
    pub batch_total_quantity: Option<i64>,
}

impl PickExecutionEvidence {
    pub const fn discrete() -> Self {
        Self {
            method: PickExecutionMethod::Discrete,
            cluster_id: None,
            cart_barcode: None,
            slot_code: None,
            sequence: None,
            task_count: None,
            batch_total_quantity: None,
        }
    }

    pub const fn case() -> Self {
        Self {
            method: PickExecutionMethod::Case,
            cluster_id: None,
            cart_barcode: None,
            slot_code: None,
            sequence: None,
            task_count: None,
            batch_total_quantity: None,
        }
    }

    pub const fn pallet() -> Self {
        Self {
            method: PickExecutionMethod::Pallet,
            cluster_id: None,
            cart_barcode: None,
            slot_code: None,
            sequence: None,
            task_count: None,
            batch_total_quantity: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickDecisionPolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickDecisionPolicyScope {
    Tenant,
    InventoryOwner {
        inventory_owner_id: i64,
    },
    Facility {
        facility_id: i64,
    },
    OwnerFacility {
        inventory_owner_id: i64,
        facility_id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickDecisionPolicy {
    pub source: PickDecisionPolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<PickDecisionPolicyScope>,
    pub require_source_location_scan: bool,
    pub require_item_scan: bool,
    pub require_destination_container_scan: bool,
    pub policy_hash: String,
}

impl PickDecisionPolicy {
    pub fn product_default() -> Self {
        Self {
            source: PickDecisionPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            require_source_location_scan: true,
            require_item_scan: true,
            require_destination_container_scan: true,
            policy_hash: wareboxes_api_contract::v1::PRODUCT_DEFAULT_PICK_DECISION_POLICY_HASH
                .to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PickScanStage {
    SourceLocation,
    Item,
    SourceLicensePlate,
    DestinationLicensePlate,
    ObservedItem,
    ObservedLot,
    ObservedSerial,
    ShortageDestinationLicensePlate,
}

impl PickScanStage {
    pub const fn prompt(self) -> &'static str {
        match self {
            Self::SourceLocation => "Scan source location",
            Self::Item => "Scan item",
            Self::SourceLicensePlate => "Scan source license plate",
            Self::DestinationLicensePlate => "Scan destination license plate",
            Self::ObservedItem => "Scan observed item",
            Self::ObservedLot => "Scan observed lot",
            Self::ObservedSerial => "Scan observed serial",
            Self::ShortageDestinationLicensePlate => "Scan destination license plate",
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
    ClaimCluster {
        cluster_id: i64,
    },
    ClaimById {
        task_id: i64,
    },
    Confirm {
        task_id: i64,
        content_id: i64,
        source_location_barcode: Option<String>,
        item_barcode: Option<String>,
        source_license_plate_barcode: Option<String>,
        destination_license_plate_barcode: Option<String>,
    },
    ReportShortage(Box<PickShortageCommand>),
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
struct BatchScanEvidence {
    cluster_id: i64,
    source_inventory_balance_id: i64,
    item_batch_id: i64,
    source_location_barcode: Option<String>,
    item_barcode: Option<String>,
    source_license_plate_barcode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PickingWorkflow {
    claim: Option<PickClaim>,
    lane: Lane,
    source_location_scan: Option<String>,
    item_scan: Option<String>,
    source_location_was_scanned: bool,
    item_was_scanned: bool,
    source_license_plate_scan: Option<String>,
    destination_license_plate_scan: Option<String>,
    shortage: Option<PickShortageDraft>,
    scan_draft: String,
    cluster_id_draft: String,
    error: Option<String>,
    notice: Option<String>,
    reconcile_reason: Option<String>,
    batch_scan_evidence: Option<BatchScanEvidence>,
}

impl Default for PickingWorkflow {
    fn default() -> Self {
        Self {
            claim: None,
            lane: Lane::Empty,
            source_location_scan: None,
            item_scan: None,
            source_location_was_scanned: false,
            item_was_scanned: false,
            source_license_plate_scan: None,
            destination_license_plate_scan: None,
            shortage: None,
            scan_draft: String::new(),
            cluster_id_draft: String::new(),
            error: None,
            notice: None,
            reconcile_reason: None,
            batch_scan_evidence: None,
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

    pub const fn shortage(&self) -> Option<&PickShortageDraft> {
        self.shortage.as_ref()
    }

    pub fn shortage_mut(&mut self) -> Option<&mut PickShortageDraft> {
        self.shortage.as_mut()
    }

    pub fn begin_shortage(&mut self) {
        if self.activity() != Activity::Active || self.shortage.is_some() {
            return;
        }
        let controlled_evidence = self.claim.as_ref().and_then(|claim| {
            if claim.content.lot.is_some() {
                Some(PickControlledEvidence::Lot)
            } else if claim.content.serial.is_some() {
                Some(PickControlledEvidence::Serial)
            } else {
                None
            }
        });
        self.shortage = Some(PickShortageDraft {
            reason: PickShortageReason::InventoryMissing,
            disposition: PickShortageDisposition::NoPick,
            controlled_evidence,
            picked_quantity: String::new(),
            note: String::new(),
            observed_item_barcode: None,
            observed_lot: None,
            observed_serial: None,
            destination_license_plate_barcode: None,
        });
        self.item_scan = None;
        self.destination_license_plate_scan = None;
        self.scan_draft.clear();
        self.error = None;
    }

    pub fn cancel_shortage(&mut self) {
        if self.activity() == Activity::Active {
            self.shortage = None;
            self.item_scan = None;
            self.destination_license_plate_scan = None;
            self.scan_draft.clear();
            self.error = None;
        }
    }

    pub fn set_shortage_reason(&mut self, reason: PickShortageReason) {
        let controlled_evidence = self.claim.as_ref().and_then(|claim| {
            if claim.content.lot.is_some() {
                Some(PickControlledEvidence::Lot)
            } else if claim.content.serial.is_some() {
                Some(PickControlledEvidence::Serial)
            } else {
                None
            }
        });
        let Some(shortage) = self.shortage.as_mut() else {
            return;
        };
        if shortage.reason == reason {
            return;
        }
        shortage.reason = reason;
        if !reason.supports_partial() {
            shortage.disposition = PickShortageDisposition::NoPick;
            shortage.picked_quantity.clear();
            shortage.destination_license_plate_barcode = None;
        }
        shortage.controlled_evidence = controlled_evidence;
        shortage.observed_item_barcode = None;
        shortage.observed_lot = None;
        shortage.observed_serial = None;
        self.scan_draft.clear();
        self.error = None;
    }

    pub fn set_shortage_disposition(&mut self, disposition: PickShortageDisposition) {
        let Some(shortage) = self.shortage.as_mut() else {
            return;
        };
        if disposition == PickShortageDisposition::Partial && !shortage.reason.supports_partial() {
            self.error = Some("This reason cannot record a partial pick".into());
            return;
        }
        shortage.disposition = disposition;
        if disposition == PickShortageDisposition::NoPick {
            shortage.picked_quantity.clear();
            shortage.destination_license_plate_barcode = None;
            if shortage.reason != PickShortageReason::LotOrSerialMismatch {
                shortage.observed_lot = None;
                shortage.observed_serial = None;
            }
        }
        self.scan_draft.clear();
        self.error = None;
    }

    pub fn set_controlled_evidence(&mut self, evidence: PickControlledEvidence) {
        let Some(shortage) = self.shortage.as_mut() else {
            return;
        };
        shortage.controlled_evidence = Some(evidence);
        shortage.observed_lot = None;
        shortage.observed_serial = None;
        self.scan_draft.clear();
        self.error = None;
    }

    pub fn shortage_validation_message(&self) -> Option<&'static str> {
        let claim = self.claim.as_ref()?;
        let shortage = self.shortage.as_ref()?;
        validate_shortage(
            claim,
            shortage,
            self.source_location_scan.as_deref(),
            self.source_license_plate_scan.as_deref(),
        )
        .err()
    }

    pub fn scan_draft_mut(&mut self) -> &mut String {
        &mut self.scan_draft
    }

    pub fn cluster_id_draft_mut(&mut self) -> &mut String {
        &mut self.cluster_id_draft
    }

    pub fn expected_scan(&self) -> Option<PickScanStage> {
        if self.activity() != Activity::Active {
            return None;
        }
        let content = &self.claim.as_ref()?.content;
        if let Some(shortage) = self.shortage.as_ref() {
            return expected_shortage_scan(
                content,
                shortage,
                self.source_location_scan.is_some(),
                self.source_license_plate_scan.is_some(),
            );
        }
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

    pub fn begin_cluster_claim(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        let cluster_id = match self.cluster_id_draft.trim().parse::<i64>() {
            Ok(value) if value > 0 => value,
            _ => {
                self.error = Some("Scan or enter a positive cluster route ID".into());
                return None;
            }
        };
        self.begin_idle(
            PickingCommand::ClaimCluster { cluster_id },
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
        let claim = self.claim.as_ref()?;
        let pallet_pick = claim.execution.method == PickExecutionMethod::Pallet;
        let content = claim.content.clone();
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
            PickScanStage::ObservedItem => {
                let matches_item = content
                    .item_barcodes
                    .iter()
                    .any(|barcode| barcode == &scanned);
                self.shortage
                    .as_ref()
                    .is_some_and(|shortage| match shortage.reason {
                        PickShortageReason::WrongInventory => !matches_item,
                        _ => matches_item,
                    })
            }
            PickScanStage::ObservedLot => self.shortage.as_ref().is_some_and(|shortage| {
                content.lot.as_deref().is_some_and(|directed| {
                    if shortage.disposition == PickShortageDisposition::Partial {
                        scanned == directed
                    } else {
                        scanned != directed
                    }
                })
            }),
            PickScanStage::ObservedSerial => self.shortage.as_ref().is_some_and(|shortage| {
                content.serial.as_deref().is_some_and(|directed| {
                    if shortage.disposition == PickShortageDisposition::Partial {
                        scanned == directed
                    } else {
                        scanned != directed
                    }
                })
            }),
            PickScanStage::ShortageDestinationLicensePlate => content
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
                PickScanStage::ObservedItem => {
                    match self.shortage.as_ref().map(|draft| draft.reason) {
                        Some(PickShortageReason::WrongInventory) => {
                            "Observed item matches the directed item"
                        }
                        _ => "Observed item does not match the directed item",
                    }
                }
                PickScanStage::ObservedLot => "Observed lot matches the directed lot",
                PickScanStage::ObservedSerial => "Observed serial matches the directed serial",
                PickScanStage::ShortageDestinationLicensePlate => {
                    "Destination license plate must differ from the source"
                }
            });
            return None;
        }

        match stage {
            PickScanStage::SourceLocation => {
                self.source_location_scan = Some(scanned);
                self.source_location_was_scanned = true;
            }
            PickScanStage::Item => {
                self.item_scan = Some(scanned);
                self.item_was_scanned = true;
            }
            PickScanStage::SourceLicensePlate => {
                self.source_license_plate_scan = Some(scanned.clone());
                if pallet_pick {
                    self.destination_license_plate_scan = Some(scanned);
                }
            }
            PickScanStage::DestinationLicensePlate => {
                self.destination_license_plate_scan = Some(scanned)
            }
            PickScanStage::ObservedItem => {
                self.shortage.as_mut()?.observed_item_barcode = Some(scanned)
            }
            PickScanStage::ObservedLot => self.shortage.as_mut()?.observed_lot = Some(scanned),
            PickScanStage::ObservedSerial => {
                self.shortage.as_mut()?.observed_serial = Some(scanned)
            }
            PickScanStage::ShortageDestinationLicensePlate => {
                self.shortage.as_mut()?.destination_license_plate_barcode = Some(scanned)
            }
        }
        self.scan_draft.clear();
        self.error = None;

        if stage != PickScanStage::DestinationLicensePlate
            && !(pallet_pick && stage == PickScanStage::SourceLicensePlate)
        {
            return None;
        }
        self.begin_confirmation(command_id, idempotency_key)
    }

    pub fn begin_confirmation(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        if self.expected_scan().is_some() || self.shortage.is_some() {
            return None;
        }
        let claim = self.claim.as_ref()?;
        let policy = &claim.pick_policy;
        self.begin_active(
            PickingCommand::Confirm {
                task_id: claim.task_id,
                content_id: claim.content.content_id,
                source_location_barcode: policy
                    .require_source_location_scan
                    .then(|| self.source_location_scan.clone())
                    .flatten(),
                item_barcode: policy
                    .require_item_scan
                    .then(|| self.item_scan.clone())
                    .flatten(),
                source_license_plate_barcode: self.source_license_plate_scan.clone(),
                destination_license_plate_barcode: policy
                    .require_destination_container_scan
                    .then(|| self.destination_license_plate_scan.clone())
                    .flatten(),
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

    pub fn begin_shortage_report(
        &mut self,
        command_id: String,
        idempotency_key: String,
    ) -> Option<WorkflowEffect> {
        let claim = self.claim.as_ref()?;
        let shortage = self.shortage.as_ref()?;
        if let Err(message) = validate_shortage(
            claim,
            shortage,
            self.source_location_scan.as_deref(),
            self.source_license_plate_scan.as_deref(),
        ) {
            self.error = Some(message.into());
            return None;
        }

        let note = (!shortage.note.trim().is_empty()).then(|| shortage.note.trim().to_owned());
        let outcome = match shortage.disposition {
            PickShortageDisposition::NoPick => PickShortageOutcome::NoPick,
            PickShortageDisposition::Partial => PickShortageOutcome::Partial {
                picked_quantity: shortage.picked_quantity.trim().parse().ok()?,
                destination_license_plate_barcode: shortage
                    .destination_license_plate_barcode
                    .clone()?,
            },
        };
        let command = PickingCommand::ReportShortage(Box::new(PickShortageCommand {
            task_id: claim.task_id,
            content_id: claim.content.content_id,
            source_location_barcode: self.source_location_scan.clone()?,
            source_license_plate_barcode: self.source_license_plate_scan.clone(),
            observed_item_barcode: shortage.observed_item_barcode.clone(),
            observed_lot: shortage.observed_lot.clone(),
            observed_serial: shortage.observed_serial.clone(),
            reason: shortage.reason,
            note,
            outcome,
        }));
        self.begin_active(command, command_id, idempotency_key)
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
        if let CommandOutcome::PickShortageReported(result) = &outcome
            && !self.claim.as_ref().is_some_and(|claim| {
                result.order_id == claim.order_id
                    && result.order_revision > claim.order_revision
                    && result.planned_quantity == claim.content.planned_quantity
                    && result.short_quantity == result.planned_quantity - result.picked_quantity
            })
        {
            self.require_reconciliation(
                "Short-pick result did not match the active order revision".into(),
            );
            return;
        }

        self.lane = Lane::Empty;
        match outcome {
            CommandOutcome::PickClaimed(claim) => {
                self.claim = claim.map(|claim| *claim);
                if self.claim.is_none()
                    || !self
                        .claim
                        .as_ref()
                        .is_some_and(|claim| self.batch_evidence_matches(claim))
                {
                    self.batch_scan_evidence = None;
                }
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
                self.retain_batch_scan_evidence();
                self.claim = None;
                self.reset_scans();
                self.notice = Some(if order_ready_to_pack {
                    "Pick confirmed; order picking is complete".into()
                } else {
                    "Pick confirmed".into()
                });
            }
            CommandOutcome::PickShortageReported(result) => {
                self.batch_scan_evidence = None;
                self.claim = None;
                self.reset_scans();
                self.notice = Some(format!(
                    "Short pick {} reported for supervisor recovery",
                    result.shortage_id
                ));
            }
            CommandOutcome::PickReleased { .. } => {
                self.batch_scan_evidence = None;
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
            self.batch_scan_evidence = None;
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
        self.batch_scan_evidence = None;
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
        self.source_location_was_scanned = false;
        self.item_was_scanned = false;
        self.source_location_scan = self.claim.as_ref().and_then(|claim| {
            (!claim.pick_policy.require_source_location_scan)
                .then(|| claim.content.source_location_barcode.clone())
        });
        self.item_scan = self.claim.as_ref().and_then(|claim| {
            (!claim.pick_policy.require_item_scan)
                .then(|| claim.content.item_barcodes.first().cloned())
                .flatten()
        });
        self.source_license_plate_scan = None;
        self.destination_license_plate_scan = self.claim.as_ref().and_then(|claim| {
            (!claim.pick_policy.require_destination_container_scan)
                .then(|| claim.suggested_destination_license_plate_barcode.clone())
                .flatten()
        });
        self.shortage = None;
        self.scan_draft.clear();
        let Some(claim) = self.claim.as_ref() else {
            return;
        };
        let Some(evidence) = self
            .batch_scan_evidence
            .as_ref()
            .filter(|evidence| batch_evidence_matches_claim(evidence, claim))
        else {
            return;
        };
        if claim.pick_policy.require_source_location_scan
            && evidence.source_location_barcode.as_deref()
                == Some(claim.content.source_location_barcode.as_str())
        {
            self.source_location_scan = evidence.source_location_barcode.clone();
            self.source_location_was_scanned = true;
        }
        if claim.pick_policy.require_item_scan
            && evidence.item_barcode.as_ref().is_some_and(|barcode| {
                claim
                    .content
                    .item_barcodes
                    .iter()
                    .any(|candidate| candidate == barcode)
            })
        {
            self.item_scan = evidence.item_barcode.clone();
            self.item_was_scanned = true;
        }
        if evidence.source_license_plate_barcode == claim.content.source_license_plate_barcode {
            self.source_license_plate_scan = evidence.source_license_plate_barcode.clone();
        }
    }

    fn retain_batch_scan_evidence(&mut self) {
        let Some(claim) = self.claim.as_ref() else {
            return;
        };
        let Some(cluster_id) = claim
            .execution
            .cluster_id
            .filter(|_| claim.execution.method == PickExecutionMethod::BatchCart)
        else {
            self.batch_scan_evidence = None;
            return;
        };
        self.batch_scan_evidence = Some(BatchScanEvidence {
            cluster_id,
            source_inventory_balance_id: claim.content.source_inventory_balance_id,
            item_batch_id: claim.content.item_batch_id,
            source_location_barcode: self
                .source_location_was_scanned
                .then(|| self.source_location_scan.clone())
                .flatten(),
            item_barcode: self
                .item_was_scanned
                .then(|| self.item_scan.clone())
                .flatten(),
            source_license_plate_barcode: self.source_license_plate_scan.clone(),
        });
    }

    fn batch_evidence_matches(&self, claim: &PickClaim) -> bool {
        self.batch_scan_evidence
            .as_ref()
            .is_some_and(|evidence| batch_evidence_matches_claim(evidence, claim))
    }
}

fn batch_evidence_matches_claim(evidence: &BatchScanEvidence, claim: &PickClaim) -> bool {
    claim.execution.method == PickExecutionMethod::BatchCart
        && claim.execution.cluster_id == Some(evidence.cluster_id)
        && claim.content.source_inventory_balance_id == evidence.source_inventory_balance_id
        && claim.content.item_batch_id == evidence.item_batch_id
}

fn outcome_matches(command: &RfCommand, outcome: &CommandOutcome) -> bool {
    match (command, outcome) {
        (RfCommand::Picking(PickingCommand::ClaimNext), CommandOutcome::PickClaimed(_)) => true,
        (
            RfCommand::Picking(PickingCommand::ClaimCluster { .. }),
            CommandOutcome::PickClaimed(_),
        ) => true,
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
            RfCommand::Picking(PickingCommand::ReportShortage(command)),
            CommandOutcome::PickShortageReported(result),
        ) => {
            let picked_quantity = match &command.outcome {
                PickShortageOutcome::NoPick => 0,
                PickShortageOutcome::Partial {
                    picked_quantity, ..
                } => *picked_quantity,
            };
            command.task_id == result.task_id
                && command.content_id == result.content_id
                && command.reason == result.reason
                && command.note == result.note
                && command.observed_item_barcode == result.observed_item_barcode
                && command.observed_lot == result.observed_lot
                && command.observed_serial == result.observed_serial
                && picked_quantity == result.picked_quantity
        }
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
#[path = "picking/tests.rs"]
mod tests;
