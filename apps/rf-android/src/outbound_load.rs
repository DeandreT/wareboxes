use serde::{Deserialize, Serialize};
use wareboxes_api_contract::v1::{
    MovePackedCartonResponse, OutboundLoadResponse, OutboundLoadStatus, PackedCartonMovementKind,
    PackedCartonPositionStateResponse,
};

use crate::workflow::{
    Activity, CommandOutcome, DurableCommandDraft, PersistedCommand, RfCommand, Transition,
    WorkflowEffect,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundCartonOperation {
    Stage,
    Load,
    Unload,
    Unstage,
}

impl OutboundCartonOperation {
    pub const ALL: [Self; 4] = [Self::Stage, Self::Load, Self::Unload, Self::Unstage];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Load => "Load",
            Self::Unload => "Unload",
            Self::Unstage => "Unstage",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboundLoadScanStage {
    Load,
    Source,
    Carton,
    Destination,
}

impl OutboundLoadScanStage {
    pub const fn prompt(self, operation: OutboundCartonOperation) -> &'static str {
        match (self, operation) {
            (Self::Load, _) => "Scan outbound load",
            (Self::Source, OutboundCartonOperation::Stage) => "Scan packed source",
            (Self::Source, OutboundCartonOperation::Load) => "Scan staging lane",
            (Self::Source, OutboundCartonOperation::Unload) => "Scan trailer",
            (Self::Source, OutboundCartonOperation::Unstage) => "Scan staging lane",
            (Self::Carton, _) => "Scan carton",
            (Self::Destination, OutboundCartonOperation::Stage) => "Scan staging lane",
            (Self::Destination, OutboundCartonOperation::Load) => "Scan trailer",
            (Self::Destination, OutboundCartonOperation::Unload) => "Scan staging lane",
            (Self::Destination, OutboundCartonOperation::Unstage) => "Scan return location",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundCartonMovementExpectation {
    pub load: Box<OutboundLoadResponse>,
    pub carton_id: i64,
    pub operation: OutboundCartonOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum OutboundLoadCommand {
    Stage {
        expected: Box<OutboundCartonMovementExpectation>,
        source_location_barcode: String,
        carton_barcode: String,
        staging_location_barcode: String,
    },
    Load {
        expected: Box<OutboundCartonMovementExpectation>,
        staging_location_barcode: String,
        carton_barcode: String,
        trailer_number: String,
    },
    Unload {
        expected: Box<OutboundCartonMovementExpectation>,
        trailer_number: String,
        carton_barcode: String,
        staging_location_barcode: String,
    },
    Unstage {
        expected: Box<OutboundCartonMovementExpectation>,
        staging_location_barcode: String,
        carton_barcode: String,
        return_location_barcode: String,
    },
}

impl OutboundLoadCommand {
    pub const fn expectation(&self) -> &OutboundCartonMovementExpectation {
        match self {
            Self::Stage { expected, .. }
            | Self::Load { expected, .. }
            | Self::Unload { expected, .. }
            | Self::Unstage { expected, .. } => expected,
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
pub struct OutboundLoadWorkflow {
    load: Option<OutboundLoadResponse>,
    operation: OutboundCartonOperation,
    lane: Lane,
    source_scan: Option<String>,
    carton_scan: Option<String>,
    destination_scan: Option<String>,
    scan_draft: String,
    error: Option<String>,
    notice: Option<String>,
    reconcile_reason: Option<String>,
}

impl Default for OutboundLoadWorkflow {
    fn default() -> Self {
        Self {
            load: None,
            operation: OutboundCartonOperation::Stage,
            lane: Lane::Empty,
            source_scan: None,
            carton_scan: None,
            destination_scan: None,
            scan_draft: String::new(),
            error: None,
            notice: None,
            reconcile_reason: None,
        }
    }
}

impl OutboundLoadWorkflow {
    pub fn activity(&self) -> Activity {
        if self.reconcile_reason.is_some() {
            return Activity::ReconcileRequired;
        }
        match self.lane {
            Lane::Persisting(_) => Activity::Persisting,
            Lane::Ready(_) => Activity::ReadyToDispatch,
            Lane::InFlight(_) => Activity::InFlight,
            Lane::Ambiguous { .. } => Activity::Ambiguous,
            Lane::Empty if self.load.is_some() => Activity::Active,
            Lane::Empty => Activity::Idle,
        }
    }

    pub const fn load(&self) -> Option<&OutboundLoadResponse> {
        self.load.as_ref()
    }

    pub const fn operation(&self) -> OutboundCartonOperation {
        self.operation
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

    pub fn resolve_load(&mut self, load: OutboundLoadResponse) {
        self.load = Some(load);
        self.lane = Lane::Empty;
        self.error = None;
        self.notice = None;
        self.clear_scans();
        self.select_first_available_operation();
    }

    pub fn load_lookup_failed(&mut self, message: impl Into<String>) {
        if self.load.is_none() && matches!(self.lane, Lane::Empty) {
            self.error = Some(message.into());
        }
    }

    pub fn clear_load(&mut self) {
        if self.activity() != Activity::Active {
            return;
        }
        self.load = None;
        self.notice = None;
        self.error = None;
        self.clear_scans();
    }

    pub fn select_operation(&mut self, operation: OutboundCartonOperation) {
        if self.activity() == Activity::Active && self.operation_allowed(operation) {
            self.operation = operation;
            self.error = None;
            self.notice = None;
            self.clear_scans();
        }
    }

    pub fn operation_allowed(&self, operation: OutboundCartonOperation) -> bool {
        let Some(load) = self.load.as_ref() else {
            return false;
        };
        match operation {
            OutboundCartonOperation::Stage => {
                matches!(
                    load.status,
                    OutboundLoadStatus::Staging | OutboundLoadStatus::Loading
                ) && load.cartons.iter().any(|carton| {
                    matches!(
                        carton.state,
                        PackedCartonPositionStateResponse::Packed { .. }
                    )
                })
            }
            OutboundCartonOperation::Load => {
                load.status == OutboundLoadStatus::Loading
                    && load.cartons.iter().any(|carton| {
                        matches!(
                            carton.state,
                            PackedCartonPositionStateResponse::Staged { .. }
                        )
                    })
            }
            OutboundCartonOperation::Unload => {
                matches!(
                    load.status,
                    OutboundLoadStatus::Loading | OutboundLoadStatus::ReadyToDepart
                ) && load.cartons.iter().any(|carton| {
                    matches!(
                        carton.state,
                        PackedCartonPositionStateResponse::Loaded { .. }
                    )
                })
            }
            OutboundCartonOperation::Unstage => {
                matches!(
                    load.status,
                    OutboundLoadStatus::Staging | OutboundLoadStatus::Loading
                ) && load.cartons.iter().any(|carton| {
                    matches!(
                        carton.state,
                        PackedCartonPositionStateResponse::Staged { .. }
                    )
                })
            }
        }
    }

    pub fn expected_scan(&self) -> Option<OutboundLoadScanStage> {
        self.load.as_ref()?;
        if self.source_scan.is_none() {
            Some(OutboundLoadScanStage::Source)
        } else if self.carton_scan.is_none() {
            Some(OutboundLoadScanStage::Carton)
        } else if self.destination_scan.is_none() {
            Some(OutboundLoadScanStage::Destination)
        } else {
            None
        }
    }

    pub fn submit_scan(&mut self) {
        if self.activity() != Activity::Active {
            return;
        }
        let Some(stage) = self.expected_scan() else {
            return;
        };
        let scanned = self.scan_draft.trim().to_owned();
        self.scan_draft.clear();
        if scanned.is_empty() {
            self.error = Some("Scan value is required".into());
            return;
        }
        if let Err(message) = self.validate_scan(stage, &scanned) {
            self.error = Some(message.into());
            return;
        }
        match stage {
            OutboundLoadScanStage::Source => self.source_scan = Some(scanned),
            OutboundLoadScanStage::Carton => self.carton_scan = Some(scanned),
            OutboundLoadScanStage::Destination => self.destination_scan = Some(scanned),
            OutboundLoadScanStage::Load => return,
        }
        self.error = None;
    }

    pub fn begin_movement(&mut self, command_id: String, idempotency_key: String) -> Transition {
        if self.activity() != Activity::Active || self.expected_scan().is_some() {
            return Transition::Ignored;
        }
        let Some(load) = self.load.clone() else {
            return Transition::Ignored;
        };
        let Some(source) = self.source_scan.clone() else {
            return Transition::Ignored;
        };
        let Some(carton_scan) = self.carton_scan.clone() else {
            return Transition::Ignored;
        };
        let Some(destination) = self.destination_scan.clone() else {
            return Transition::Ignored;
        };
        let Some(carton) = matching_carton(&load, self.operation, &carton_scan) else {
            self.error = Some("Carton is not eligible for this operation".into());
            return Transition::Ignored;
        };
        let carton_id = carton.carton_id;
        let expected = Box::new(OutboundCartonMovementExpectation {
            load: Box::new(load),
            carton_id,
            operation: self.operation,
        });
        let command = match self.operation {
            OutboundCartonOperation::Stage => OutboundLoadCommand::Stage {
                expected,
                source_location_barcode: source,
                carton_barcode: carton_scan,
                staging_location_barcode: destination,
            },
            OutboundCartonOperation::Load => OutboundLoadCommand::Load {
                expected,
                staging_location_barcode: source,
                carton_barcode: carton_scan,
                trailer_number: destination,
            },
            OutboundCartonOperation::Unload => OutboundLoadCommand::Unload {
                expected,
                trailer_number: source,
                carton_barcode: carton_scan,
                staging_location_barcode: destination,
            },
            OutboundCartonOperation::Unstage => OutboundLoadCommand::Unstage {
                expected,
                staging_location_barcode: source,
                carton_barcode: carton_scan,
                return_location_barcode: destination,
            },
        };
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id,
            idempotency_key,
            command: RfCommand::OutboundLoad(command),
        };
        self.lane = Lane::Persisting(draft.clone());
        Transition::Effect(WorkflowEffect::PersistCommand(draft))
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
        if let Lane::InFlight(command) | Lane::Ready(command) = &self.lane
            && command.record_id == record_id
        {
            self.lane = Lane::Ambiguous {
                command: command.clone(),
                message: message.into(),
            };
        }
    }

    pub fn restore_ready_command(
        &mut self,
        record_id: i64,
        draft: DurableCommandDraft,
    ) -> Transition {
        let Some(command) = outbound_command(&draft) else {
            self.require_reconciliation("Saved work is not an outbound-load command".into());
            return Transition::Ignored;
        };
        self.load = Some((*command.expectation().load).clone());
        self.operation = command.expectation().operation;
        let persisted = PersistedCommand { record_id, draft };
        self.lane = Lane::Ready(persisted);
        Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id })
    }

    pub fn restore_ambiguous_command(
        &mut self,
        record_id: i64,
        draft: DurableCommandDraft,
        message: impl Into<String>,
    ) {
        let Some(command) = outbound_command(&draft) else {
            self.require_reconciliation("Saved work is not an outbound-load command".into());
            return;
        };
        self.load = Some((*command.expectation().load).clone());
        self.operation = command.expectation().operation;
        self.lane = Lane::Ambiguous {
            command: PersistedCommand { record_id, draft },
            message: message.into(),
        };
    }

    pub fn retry_ambiguous(&mut self) -> Transition {
        let Lane::Ambiguous { command, .. } = &self.lane else {
            return Transition::Ignored;
        };
        let record_id = command.record_id;
        self.lane = Lane::Ready(command.clone());
        Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id })
    }

    pub fn owns_record(&self, record_id: i64) -> bool {
        match &self.lane {
            Lane::Ready(command) | Lane::InFlight(command) => command.record_id == record_id,
            Lane::Ambiguous { command, .. } => command.record_id == record_id,
            Lane::Empty | Lane::Persisting(_) => false,
        }
    }

    pub fn ambiguous_message(&self) -> Option<&str> {
        match &self.lane {
            Lane::Ambiguous { message, .. } => Some(message),
            _ => None,
        }
    }

    pub fn accepts_outcome(&self, record_id: i64, outcome: &CommandOutcome) -> bool {
        let Some(command) = self.persisted_command(record_id) else {
            return false;
        };
        let CommandOutcome::OutboundCartonMoved(result) = outcome else {
            return false;
        };
        movement_matches(command.expectation(), result)
    }

    pub fn durable_outcome_recorded(&mut self, record_id: i64, outcome: CommandOutcome) {
        if !self.accepts_outcome(record_id, &outcome) {
            self.require_reconciliation("Outbound carton result conflicts with saved scans".into());
            return;
        }
        let CommandOutcome::OutboundCartonMoved(result) = outcome else {
            return;
        };
        let Some(mut load) = self.load.take() else {
            self.require_reconciliation("Outbound load snapshot is missing".into());
            return;
        };
        if let Some(carton) = load
            .cartons
            .iter_mut()
            .find(|carton| carton.carton_id == result.movement.carton_id)
        {
            carton.state = result.position.state.clone();
            carton.position_revision = result.position.revision;
            carton.last_movement_id = Some(result.movement.movement_id);
        }
        load.status = result.load_status;
        load.revision = result.load_revision;
        load.progress = result.progress;
        self.load = Some(load);
        self.lane = Lane::Empty;
        self.notice = Some(format!(
            "Carton {} {} complete",
            result.position.carton_barcode,
            self.operation.label().to_lowercase()
        ));
        self.error = None;
        self.clear_scans();
        self.select_first_available_operation();
    }

    pub fn durable_rejection_recorded(&mut self, record_id: i64, message: String) {
        if self.owns_record(record_id) {
            self.lane = Lane::Empty;
            self.error = Some(message);
            self.clear_scans();
        }
    }

    pub fn require_reconciliation(&mut self, message: String) {
        self.reconcile_reason = Some(message);
    }

    fn persisted_command(&self, record_id: i64) -> Option<&OutboundLoadCommand> {
        let command = match &self.lane {
            Lane::Ready(command) | Lane::InFlight(command) if command.record_id == record_id => {
                command
            }
            Lane::Ambiguous { command, .. } if command.record_id == record_id => command,
            _ => return None,
        };
        outbound_command(&command.draft)
    }

    fn validate_scan(
        &self,
        stage: OutboundLoadScanStage,
        scanned: &str,
    ) -> Result<(), &'static str> {
        let load = self.load.as_ref().ok_or("Scan an outbound load first")?;
        match stage {
            OutboundLoadScanStage::Source => match self.operation {
                OutboundCartonOperation::Load | OutboundCartonOperation::Unstage
                    if !load.staging_location_barcode.eq_ignore_ascii_case(scanned) =>
                {
                    Err("Staging lane does not match this load")
                }
                OutboundCartonOperation::Unload
                    if load
                        .trailer_number
                        .as_deref()
                        .is_none_or(|value| !value.eq_ignore_ascii_case(scanned)) =>
                {
                    Err("Trailer does not match this load")
                }
                _ => Ok(()),
            },
            OutboundLoadScanStage::Carton => matching_carton(load, self.operation, scanned)
                .map(|_| ())
                .ok_or("Carton is not eligible for this operation"),
            OutboundLoadScanStage::Destination => match self.operation {
                OutboundCartonOperation::Stage | OutboundCartonOperation::Unload
                    if !load.staging_location_barcode.eq_ignore_ascii_case(scanned) =>
                {
                    Err("Staging lane does not match this load")
                }
                OutboundCartonOperation::Load
                    if load
                        .trailer_number
                        .as_deref()
                        .is_none_or(|value| !value.eq_ignore_ascii_case(scanned)) =>
                {
                    Err("Trailer does not match this load")
                }
                _ => Ok(()),
            },
            OutboundLoadScanStage::Load => Ok(()),
        }
    }

    fn clear_scans(&mut self) {
        self.source_scan = None;
        self.carton_scan = None;
        self.destination_scan = None;
        self.scan_draft.clear();
    }

    fn select_first_available_operation(&mut self) {
        if !self.operation_allowed(self.operation)
            && let Some(operation) = OutboundCartonOperation::ALL
                .into_iter()
                .find(|operation| self.operation_allowed(*operation))
        {
            self.operation = operation;
        }
    }
}

fn outbound_command(draft: &DurableCommandDraft) -> Option<&OutboundLoadCommand> {
    match &draft.command {
        RfCommand::OutboundLoad(command) => Some(command),
        _ => None,
    }
}

fn matching_carton<'a>(
    load: &'a OutboundLoadResponse,
    operation: OutboundCartonOperation,
    barcode: &str,
) -> Option<&'a wareboxes_api_contract::v1::OutboundLoadCartonResponse> {
    load.cartons.iter().find(|carton| {
        carton.carton_barcode.eq_ignore_ascii_case(barcode)
            && match operation {
                OutboundCartonOperation::Stage => {
                    matches!(
                        carton.state,
                        PackedCartonPositionStateResponse::Packed { .. }
                    )
                }
                OutboundCartonOperation::Load => {
                    matches!(
                        carton.state,
                        PackedCartonPositionStateResponse::Staged { .. }
                    )
                }
                OutboundCartonOperation::Unload => {
                    matches!(
                        carton.state,
                        PackedCartonPositionStateResponse::Loaded { .. }
                    )
                }
                OutboundCartonOperation::Unstage => {
                    matches!(
                        carton.state,
                        PackedCartonPositionStateResponse::Staged { .. }
                    )
                }
            }
    })
}

fn movement_matches(
    expected: &OutboundCartonMovementExpectation,
    result: &MovePackedCartonResponse,
) -> bool {
    let Some(carton) = expected
        .load
        .cartons
        .iter()
        .find(|carton| carton.carton_id == expected.carton_id)
    else {
        return false;
    };
    let Some(owner_id) = expected
        .load
        .shipments
        .iter()
        .find(|shipment| shipment.shipment_id == carton.shipment_id)
        .map(|shipment| shipment.inventory_owner_id)
    else {
        return false;
    };
    let Some(resulting_position_revision) = carton.position_revision.checked_next() else {
        return false;
    };
    let expected_kind = match expected.operation {
        OutboundCartonOperation::Stage => PackedCartonMovementKind::Stage,
        OutboundCartonOperation::Load => PackedCartonMovementKind::Load,
        OutboundCartonOperation::Unload => PackedCartonMovementKind::Unload,
        OutboundCartonOperation::Unstage => PackedCartonMovementKind::Unstage,
    };
    let expected_load_revision = if expected.operation == OutboundCartonOperation::Unload
        && expected.load.status == OutboundLoadStatus::ReadyToDepart
    {
        expected.load.revision.checked_next()
    } else {
        Some(expected.load.revision)
    };
    let expected_status = if expected.operation == OutboundCartonOperation::Unload
        && expected.load.status == OutboundLoadStatus::ReadyToDepart
    {
        OutboundLoadStatus::Loading
    } else {
        expected.load.status
    };
    let state_matches = match (&expected.operation, &result.position.state) {
        (
            OutboundCartonOperation::Stage | OutboundCartonOperation::Unload,
            PackedCartonPositionStateResponse::Staged {
                outbound_load_id,
                staging_location_id,
            },
        ) => {
            *outbound_load_id == expected.load.outbound_load_id
                && *staging_location_id == expected.load.staging_location_id
        }
        (
            OutboundCartonOperation::Load,
            PackedCartonPositionStateResponse::Loaded {
                outbound_load_id,
                load_sequence,
            },
        ) => {
            *outbound_load_id == expected.load.outbound_load_id
                && *load_sequence == carton.load_sequence
        }
        (OutboundCartonOperation::Unstage, PackedCartonPositionStateResponse::Packed { .. }) => {
            true
        }
        _ => false,
    };
    let detail_quantity = result
        .movement
        .details
        .iter()
        .try_fold(0_i64, |total, detail| total.checked_add(detail.quantity));
    let position_quantity = result
        .position
        .contents
        .iter()
        .try_fold(0_i64, |total, content| {
            total.checked_add(content.packed_quantity)
        });
    let (expected_staged, expected_loaded) = expected
        .load
        .cartons
        .iter()
        .map(|candidate| {
            if candidate.carton_id == carton.carton_id {
                &result.position.state
            } else {
                &candidate.state
            }
        })
        .fold((0_u32, 0_u32), |(staged, loaded), state| match state {
            PackedCartonPositionStateResponse::Staged { .. } => (staged + 1, loaded),
            PackedCartonPositionStateResponse::Loaded { .. } => (staged, loaded + 1),
            PackedCartonPositionStateResponse::Packed { .. }
            | PackedCartonPositionStateResponse::Departed { .. } => (staged, loaded),
        });

    result.outbound_load_id == expected.load.outbound_load_id
        && result.movement.outbound_load_id == expected.load.outbound_load_id
        && result.movement.outbound_load_carton_id == carton.outbound_load_carton_id
        && result.movement.carton_id == expected.carton_id
        && result.position.carton_id == expected.carton_id
        && result.position.carton_barcode == carton.carton_barcode
        && result.position.inventory_owner_id == owner_id
        && result.position.facility_id == expected.load.facility_id
        && result.position.revision == resulting_position_revision
        && result.movement.kind == expected_kind
        && result.movement.quantity == carton.packed_quantity
        && result.movement.quantity > 0
        && result.movement.source_location_id != result.movement.destination_location_id
        && !result.movement.details.is_empty()
        && detail_quantity == Some(carton.packed_quantity)
        && !result.position.contents.is_empty()
        && position_quantity == Some(carton.packed_quantity)
        && state_matches
        && expected_load_revision == Some(result.load_revision)
        && result.load_status == expected_status
        && result.progress.planned_shipment_count == expected.load.progress.planned_shipment_count
        && result.progress.planned_carton_count == expected.load.progress.planned_carton_count
        && result.progress.staged_carton_count == expected_staged
        && result.progress.loaded_carton_count == expected_loaded
}

#[cfg(any(test, all(debug_assertions, not(target_os = "android"))))]
pub(crate) fn example_outbound_load() -> OutboundLoadResponse {
    serde_json::from_value(serde_json::json!({
        "outbound_load_id": 44,
        "load_reference": "LOAD-2026-0044",
        "load_barcode": "OL-00000044",
        "carrier_code": "UPSN",
        "facility_id": 4,
        "status": "loading",
        "revision": 3,
        "progress": {
            "planned_shipment_count": 1,
            "planned_carton_count": 3,
            "staged_carton_count": 1,
            "loaded_carton_count": 1
        },
        "staging_location_id": 101,
        "staging_location_barcode": "STAGE-04",
        "staging_location_name": "Outbound staging 04",
        "dock_location_id": 102,
        "dock_location_barcode": "DOCK-07",
        "dock_location_name": "Dock door 07",
        "virtual_trailer_location_id": 103,
        "trailer_number": "TRL-8801",
        "seal_number": null,
        "scheduled_departure_at": "2026-08-08T21:30:00Z",
        "shipments": [{
            "outbound_load_shipment_id": 701,
            "shipment_id": 501,
            "order_id": 401,
            "order_key": "ORDER-00401",
            "inventory_owner_id": 12,
            "inventory_owner_name": "Northwind Retail",
            "shipment_sequence": 1,
            "shipment_status": "manifested",
            "shipment_revision": 2,
            "order_status": "awaiting_shipment",
            "order_revision": 9,
            "demand": {
                "ordered_quantity": 24,
                "shipped_quantity": 24,
                "accepted_short_quantity": 0,
                "accepted_substitute_quantity": 0
            }
        }],
        "cartons": [
            {
                "outbound_load_carton_id": 801,
                "shipment_id": 501,
                "carton_id": 601,
                "carton_barcode": "CTN-00601",
                "license_plate_id": 901,
                "load_sequence": 1,
                "state": { "state": "packed", "location_id": 201 },
                "position_revision": 1,
                "content_count": 1,
                "packed_quantity": 8,
                "last_movement_id": null
            },
            {
                "outbound_load_carton_id": 802,
                "shipment_id": 501,
                "carton_id": 602,
                "carton_barcode": "CTN-00602",
                "license_plate_id": 902,
                "load_sequence": 2,
                "state": {
                    "state": "staged",
                    "outbound_load_id": 44,
                    "staging_location_id": 101
                },
                "position_revision": 2,
                "content_count": 1,
                "packed_quantity": 8,
                "last_movement_id": 1001
            },
            {
                "outbound_load_carton_id": 803,
                "shipment_id": 501,
                "carton_id": 603,
                "carton_barcode": "CTN-00603",
                "license_plate_id": 903,
                "load_sequence": 3,
                "state": {
                    "state": "loaded",
                    "outbound_load_id": 44,
                    "load_sequence": 3
                },
                "position_revision": 3,
                "content_count": 1,
                "packed_quantity": 8,
                "last_movement_id": 1002
            }
        ],
        "planned_by": 1,
        "planned_at": "2026-08-08T20:00:00Z",
        "released_by": 1,
        "released_at": "2026-08-08T20:05:00Z",
        "loading_started_by": 1,
        "loading_started_at": "2026-08-08T20:15:00Z",
        "ready_to_depart_by": null,
        "ready_to_depart_at": null,
        "departed_by": null,
        "departed_at": null,
        "cancelled_by": null,
        "cancelled_at": null
    }))
    .unwrap_or_else(|error| panic!("outbound-load example must deserialize: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submit(workflow: &mut OutboundLoadWorkflow, scan: &str) {
        *workflow.scan_draft_mut() = scan.into();
        workflow.submit_scan();
    }

    fn staged_result() -> MovePackedCartonResponse {
        serde_json::from_value(serde_json::json!({
            "movement": {
                "movement_id": 1101,
                "outbound_load_id": 44,
                "outbound_load_carton_id": 801,
                "carton_id": 601,
                "kind": "stage",
                "inventory_transaction_id": 1201,
                "source_location_id": 201,
                "destination_location_id": 101,
                "quantity": 8,
                "details": [{
                    "carton_content_id": 1301,
                    "source_inventory_allocation_id": 1401,
                    "destination_inventory_allocation_id": 1402,
                    "source_inventory_balance_id": 1501,
                    "destination_inventory_balance_id": 1502,
                    "quantity": 8
                }],
                "moved_by": 1,
                "moved_at": "2026-08-08T20:20:00Z"
            },
            "position": {
                "carton_id": 601,
                "carton_barcode": "CTN-00601",
                "inventory_owner_id": 12,
                "facility_id": 4,
                "state": {
                    "state": "staged",
                    "outbound_load_id": 44,
                    "staging_location_id": 101
                },
                "revision": 2,
                "contents": [{
                    "position_id": 1601,
                    "carton_content_id": 1301,
                    "current_inventory_allocation_id": 1402,
                    "current_inventory_balance_id": 1502,
                    "current_location_id": 101,
                    "current_license_plate_id": 901,
                    "packed_quantity": 8
                }],
                "positioned_at": "2026-08-08T20:20:00Z",
                "departed_at": null
            },
            "outbound_load_id": 44,
            "load_status": "loading",
            "load_revision": 3,
            "progress": {
                "planned_shipment_count": 1,
                "planned_carton_count": 3,
                "staged_carton_count": 2,
                "loaded_carton_count": 1
            }
        }))
        .unwrap_or_else(|error| panic!("movement example must deserialize: {error}"))
    }

    #[test]
    fn exact_scans_persist_before_dispatch_and_apply_authoritative_result() {
        let mut workflow = OutboundLoadWorkflow::default();
        workflow.resolve_load(example_outbound_load());
        for (scan, stage) in [
            ("PACK-01", OutboundLoadScanStage::Source),
            ("CTN-00601", OutboundLoadScanStage::Carton),
            ("STAGE-04", OutboundLoadScanStage::Destination),
        ] {
            assert_eq!(workflow.expected_scan(), Some(stage));
            submit(&mut workflow, scan);
        }
        let Transition::Effect(WorkflowEffect::PersistCommand(draft)) =
            workflow.begin_movement("command-1".into(), "key-1".into())
        else {
            panic!("carton move must persist before dispatch");
        };
        assert!(matches!(
            workflow.command_persisted("command-1", 9),
            Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id: 9 })
        ));
        workflow.dispatch_started(9);
        let outcome = CommandOutcome::OutboundCartonMoved(Box::new(staged_result()));
        assert!(workflow.accepts_outcome(9, &outcome));
        workflow.durable_outcome_recorded(9, outcome);
        assert_eq!(workflow.activity(), Activity::Active);
        assert!(matches!(
            workflow
                .load()
                .and_then(|load| load.cartons.first())
                .map(|carton| &carton.state),
            Some(PackedCartonPositionStateResponse::Staged { .. })
        ));
        assert!(matches!(draft.command, RfCommand::OutboundLoad(_)));
    }

    #[test]
    fn wrong_scan_does_not_advance_and_loading_allows_recovery_unstage() {
        let mut workflow = OutboundLoadWorkflow::default();
        workflow.resolve_load(example_outbound_load());
        submit(&mut workflow, "PACK-01");
        submit(&mut workflow, "OTHER-CARTON");
        assert_eq!(
            workflow.expected_scan(),
            Some(OutboundLoadScanStage::Carton)
        );
        assert_eq!(
            workflow.error(),
            Some("Carton is not eligible for this operation")
        );
        assert!(workflow.operation_allowed(OutboundCartonOperation::Unstage));
    }

    #[test]
    fn mismatched_or_stale_result_requires_reconciliation() {
        let mut workflow = OutboundLoadWorkflow::default();
        workflow.resolve_load(example_outbound_load());
        for scan in ["PACK-01", "CTN-00601", "STAGE-04"] {
            submit(&mut workflow, scan);
        }
        let Transition::Effect(WorkflowEffect::PersistCommand(_)) =
            workflow.begin_movement("command-1".into(), "key-1".into())
        else {
            panic!("carton move must persist before dispatch");
        };
        assert!(matches!(
            workflow.command_persisted("command-1", 9),
            Transition::Effect(_)
        ));
        workflow.dispatch_started(9);
        let mut result = staged_result();
        result.position.revision = wareboxes_api_contract::v1::Revision::new(3)
            .unwrap_or_else(|error| panic!("valid revision: {error}"));
        workflow.durable_outcome_recorded(9, CommandOutcome::OutboundCartonMoved(Box::new(result)));
        assert_eq!(workflow.activity(), Activity::ReconcileRequired);
    }

    #[test]
    fn ambiguous_restart_retains_exact_authoritative_snapshot() {
        let mut workflow = OutboundLoadWorkflow::default();
        workflow.resolve_load(example_outbound_load());
        for scan in ["PACK-01", "CTN-00601", "STAGE-04"] {
            submit(&mut workflow, scan);
        }
        let Transition::Effect(WorkflowEffect::PersistCommand(draft)) =
            workflow.begin_movement("command-1".into(), "key-1".into())
        else {
            panic!("carton move must persist before dispatch");
        };
        let mut restored = OutboundLoadWorkflow::default();
        restored.restore_ambiguous_command(9, draft, "Check saved move");
        let outcome = CommandOutcome::OutboundCartonMoved(Box::new(staged_result()));
        assert!(restored.accepts_outcome(9, &outcome));
    }
}
