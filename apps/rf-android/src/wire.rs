use chrono::DateTime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wareboxes_api_contract::v1::{
    API_PREFIX, ClaimCycleCountByIdRequest, ClaimInventoryRelocationByIdRequest,
    ClaimNextCycleCountRequest, ClaimNextInventoryRelocationRequest, ClaimNextPickRequest,
    ClaimNextPutawayRequest, ClaimPickByIdRequest, ClaimPutawayByIdRequest,
    ConfigurationScope as ApiConfigurationScope, ConfirmCycleCountRequest,
    ConfirmExpectedReceiptRequest, ConfirmInventoryRelocationRequest,
    ConfirmLicensePlatePutawayRequest, ConfirmPickContentRequest, ConfirmPutawayRequest,
    ConfirmUnexpectedReceiptRequest, CycleCountClaimHeartbeatResponse,
    CycleCountClaimReleaseReason, CycleCountClaimResponse, CycleCountConfirmationResponse,
    ExpectedReceiptConfirmationResponse, ExpectedReceiptDisposition,
    ExpectedReceiptExceptionReason, ExpectedReceiptQuarantineReason, ExpectedReceivingLoadStatus,
    ExpectedReceivingSessionResponse, HeartbeatInventoryRelocationClaimRequest,
    HeartbeatPutawayClaimRequest, IdempotencyKey, InventoryRelocationClaimHeartbeatResponse,
    InventoryRelocationClaimReleaseReason, InventoryRelocationClaimResponse,
    InventoryRelocationClaimWork, InventoryRelocationConfirmationResponse,
    InventoryRelocationWorkflow, LicensePlatePutawayConfirmationResponse,
    PickClaimHeartbeatResponse, PickClaimReleaseReason, PickClaimResponse,
    PickContentConfirmationResponse, PickDecisionPolicySource as ApiPickDecisionPolicySource,
    PickShortageDetails as ApiPickShortageDetails, PickShortageReason as ApiPickShortageReason,
    PickShortageStatus as ApiPickShortageStatus, PutawayClaimHeartbeatResponse,
    PutawayClaimReleaseReason, PutawayClaimResponse, PutawayClaimSourceLocation, PutawayClaimWork,
    PutawayConfirmationResponse, PutawayPolicyExpectation as ApiPutawayPolicyExpectation,
    PutawayPolicyResponse as ApiPutawayPolicyResponse,
    PutawayPolicySource as ApiPutawayPolicySource, PutawayWorkflow as ApiPutawayWorkflow,
    ReceiptPolicyExpectation as ApiReceiptPolicyExpectation,
    ReceiptPolicyResponse as ApiReceiptPolicyResponse,
    ReceiptPolicySource as ApiReceiptPolicySource, ReleaseCycleCountClaimRequest,
    ReleaseInventoryRelocationClaimRequest, ReleasePickClaimRequest, ReleasePutawayClaimRequest,
    ReportPickShortageOutcome as ApiReportPickShortageOutcome, ReportPickShortageRequest,
    ReportPickShortageResponse, StartInboundLoadUnloadingRequest,
    StartInboundLoadUnloadingResponse, UnexpectedReceiptConfirmationResponse,
    UnexpectedReceiptReason as ApiUnexpectedReceiptReason,
};

use crate::cycle_count::CycleCountClaim;
use crate::expected_receiving::{
    ConfirmationMode, ConfirmationResult, DockBarcode, ExceptionNote, ExpectedReceiptCommand,
    ExpectedReceiptLine as DomainExpectedReceiptLine, ExpectedReceiptLineInput, Expiration,
    FacilityId, InventoryOwnerId, ItemBarcode, ItemId, LicensePlateBarcode, LoadId, LoadLineId,
    LocationId, NonNegativeQuantity, PositiveQuantity, ReceiptExceptionReason, ReceiptPolicy,
    ReceiptPolicyInput, ReceiptPolicyScope, ReceiptPolicySource, ReceiptQuarantineReason,
    ReceivingCommandIntent, ReceivingDock, ReceivingLoadStatus, ReceivingSession,
    ReceivingSessionInput, SealBarcode, StockDimension, UnexpectedReceiptCommand,
    UnexpectedReceiptReason, UnexpectedReceiptResult,
};
#[cfg(test)]
use crate::expected_receiving::{
    UnloadingStartCommand, UnloadingStartIntent, UnloadingStartResult,
};
use crate::picking::{
    PickClaim, PickClaimContent, PickContentState, PickDecisionPolicy, PickDecisionPolicyScope,
    PickDecisionPolicySource, PickReleaseReason, PickShortageOutcome, PickShortageReason,
    PickShortageReportResult, PickShortageStatus, PickingCommand,
};
use crate::workflow::{
    CommandOutcome, CycleCountCommand, DurableCommandDraft, InventoryRelocationClaim,
    InventoryRelocationCommand, Location, MovementClaimDetails, MovementKind, MovementOperation,
    MovementWork, PutawayClaim, PutawayCommand, PutawayPolicy, PutawayPolicyExpectation,
    PutawayPolicySource, ReleaseReason, RfCommand,
};

mod cross_dock;
mod outbound_load;
mod replenishment;

pub use cross_dock::{
    build_heartbeat_request_parts as build_cross_dock_heartbeat_request_parts,
    decode_claim_response as decode_cross_dock_claim_response,
    decode_heartbeat_response as decode_cross_dock_heartbeat_response,
};
pub use replenishment::{
    build_heartbeat_request_parts as build_replenishment_heartbeat_request_parts,
    decode_claim_response as decode_replenishment_claim_response,
    decode_heartbeat_response as decode_replenishment_heartbeat_response,
};

pub const JSON_CONTENT_TYPE: &str = "application/json";
pub const EXPECTED_RECEIVING_BARCODE_LOOKUP_PATH: &str =
    "/api/v1/expected-receiving/loads/by-barcode";

const MAX_EXPECTED_RECEIVING_BARCODE_LENGTH: usize = 200;
const MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH: usize = 200;
const MAX_EXPECTED_RECEIVING_NOTE_LENGTH: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Post,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseKind {
    OptionalClaim,
    Claim,
    LooseConfirmation,
    LicensePlateConfirmation,
    Release,
    RelocationOptionalClaim,
    RelocationClaim,
    RelocationConfirmation,
    RelocationRelease,
    CycleCountOptionalClaim,
    CycleCountClaim,
    CycleCountConfirmation,
    CycleCountRelease,
    PickOptionalClaim,
    PickClaim,
    PickConfirmation,
    PickShortageReport,
    PickRelease,
    ReplenishmentOptionalClaim,
    ReplenishmentClaim,
    ReplenishmentConfirmation,
    ReplenishmentRelease,
    CrossDockOptionalClaim,
    CrossDockClaim,
    CrossDockConfirmation,
    CrossDockRelease,
    OutboundCartonMovement,
    InboundUnloadingStart,
    ExpectedReceiptConfirmation,
    UnexpectedReceiptConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableHttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub content_type: String,
    pub body: Vec<u8>,
    pub body_sha256: [u8; 32],
    pub response_kind: ResponseKind,
}

impl DurableHttpRequest {
    pub fn verify_body(&self) -> bool {
        Sha256::digest(&self.body).as_slice() == self.body_sha256
    }
}

#[derive(Debug, Error)]
pub enum WireRequestError {
    #[error("command schema version {0} is unsupported")]
    UnsupportedSchema(u16),
    #[error("command ID must be a non-empty visible ASCII value")]
    InvalidCommandId,
    #[error("task ID must be positive")]
    InvalidTaskId,
    #[error("pick content ID must be positive")]
    InvalidPickContentId,
    #[error("pick shortage contains invalid {field}")]
    InvalidPickShortageField { field: &'static str },
    #[error("replenishment command contains invalid persisted state")]
    InvalidReplenishmentCommand,
    #[error("cross-dock command contains invalid persisted state")]
    InvalidCrossDockCommand,
    #[error("outbound-load command contains invalid persisted state")]
    InvalidOutboundLoadCommand,
    #[error("load ID must be positive")]
    InvalidLoadId,
    #[error("load line ID must be positive")]
    InvalidLoadLineId,
    #[error(
        "expected receiving barcode must start with an ASCII letter or digit and contain at most \
         200 ASCII letters, digits, periods, underscores, colons, or hyphens"
    )]
    InvalidExpectedReceivingBarcode,
    #[error("expected receipt confirmation contains invalid {field}")]
    InvalidExpectedReceiptField { field: &'static str },
    #[error("an expected receipt exception with reason other requires a note")]
    ExpectedReceiptNoteRequired,
    #[error("invalid idempotency key: {0}")]
    InvalidIdempotencyKey(#[from] wareboxes_api_contract::v1::IdempotencyKeyError),
    #[error("could not encode the versioned API request: {0}")]
    Encode(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum WireResponseError {
    #[error("HTTP status {0} is not a successful command response")]
    UnsuccessfulStatus(u16),
    #[error("the warehouse service returned an invalid command response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("the warehouse service returned an invalid putaway claim")]
    InvalidClaim,
    #[error("the heartbeat response contains an invalid task ID")]
    InvalidHeartbeatTaskId,
    #[error("the heartbeat response task ID {actual} does not match requested task {expected}")]
    HeartbeatTaskMismatch { expected: i64, actual: i64 },
    #[error("the heartbeat response contains an invalid RFC 3339 {field}")]
    InvalidHeartbeatTimestamp { field: &'static str },
    #[error("the warehouse service returned an invalid expected receiving session")]
    InvalidExpectedReceivingSession,
    #[error("the warehouse service returned an invalid inbound unloading result")]
    InvalidInboundUnloadingStart,
    #[error(
        "the expected receiving response load ID {actual} does not match requested load {expected}"
    )]
    ExpectedReceivingLoadMismatch { expected: i64, actual: i64 },
    #[error("the warehouse service returned an invalid expected receipt confirmation")]
    InvalidExpectedReceiptConfirmation,
    #[error("the warehouse service returned an invalid unexpected receipt confirmation")]
    InvalidUnexpectedReceiptConfirmation,
    #[error("the warehouse service returned an invalid pick-shortage result")]
    InvalidPickShortageResponse,
    #[error("the warehouse service returned an invalid replenishment result")]
    InvalidReplenishmentResponse,
    #[error("the warehouse service returned an invalid cross-dock result")]
    InvalidCrossDockResponse,
    #[error(
        "the expected receipt response line ID {actual} does not match requested line {expected}"
    )]
    ExpectedReceiptLineMismatch { expected: i64, actual: i64 },
}

pub fn build_heartbeat_request_parts(task_id: i64) -> Result<(String, Vec<u8>), WireRequestError> {
    build_movement_heartbeat_request_parts(MovementOperation::Putaway, task_id)
}

pub fn build_movement_heartbeat_request_parts(
    operation: MovementOperation,
    task_id: i64,
) -> Result<(String, Vec<u8>), WireRequestError> {
    validate_task_id(task_id)?;
    match operation {
        MovementOperation::Putaway => Ok((
            format!("{API_PREFIX}/putaway-claims/{task_id}/heartbeats"),
            serde_json::to_vec(&HeartbeatPutawayClaimRequest::default())?,
        )),
        MovementOperation::InventoryRelocation => Ok((
            format!("{API_PREFIX}/inventory-relocation-claims/{task_id}/heartbeats"),
            serde_json::to_vec(&HeartbeatInventoryRelocationClaimRequest::default())?,
        )),
    }
}

pub fn build_pick_heartbeat_request_parts(
    task_id: i64,
) -> Result<(String, Vec<u8>), WireRequestError> {
    validate_task_id(task_id)?;
    Ok((
        format!("{API_PREFIX}/picking-claims/{task_id}/heartbeats"),
        serde_json::to_vec(&wareboxes_api_contract::v1::HeartbeatPickClaimRequest::default())?,
    ))
}

pub fn build_expected_receiving_session_path(load_id: i64) -> Result<String, WireRequestError> {
    if load_id <= 0 {
        return Err(WireRequestError::InvalidLoadId);
    }
    Ok(format!("{API_PREFIX}/expected-receiving/loads/{load_id}"))
}

pub fn validate_expected_receiving_load_barcode(barcode: &str) -> Result<(), WireRequestError> {
    normalize_expected_receiving_load_barcode(barcode).map(drop)
}

pub fn normalize_expected_receiving_load_barcode(
    barcode: &str,
) -> Result<String, WireRequestError> {
    let normalized = barcode.trim();
    let mut characters = normalized.bytes();
    let valid_first = characters
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric());
    let valid_rest = characters
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
    if normalized.is_empty()
        || normalized.len() > MAX_EXPECTED_RECEIVING_BARCODE_LENGTH
        || !valid_first
        || !valid_rest
    {
        return Err(WireRequestError::InvalidExpectedReceivingBarcode);
    }
    Ok(normalized.to_ascii_uppercase())
}

fn build_expected_receipt_confirmation_parts(
    load_line_id: i64,
    confirmation: &ConfirmExpectedReceiptRequest,
) -> Result<(String, Vec<u8>), WireRequestError> {
    if load_line_id <= 0 {
        return Err(WireRequestError::InvalidLoadLineId);
    }
    validate_expected_receipt_confirmation(confirmation)?;
    Ok((
        format!("{API_PREFIX}/expected-receiving/lines/{load_line_id}/confirmations"),
        serde_json::to_vec(confirmation)?,
    ))
}

pub fn build_durable_request(
    draft: &DurableCommandDraft,
) -> Result<DurableHttpRequest, WireRequestError> {
    validate_draft(draft)?;
    let (path, body, response_kind) = match &draft.command {
        RfCommand::Putaway(PutawayCommand::ClaimNext { workflow }) => (
            format!("{API_PREFIX}/putaway-claims/next"),
            serde_json::to_vec(&ClaimNextPutawayRequest {
                workflow: map_workflow(*workflow),
            })?,
            ResponseKind::OptionalClaim,
        ),
        RfCommand::Putaway(PutawayCommand::ClaimById { task_id }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/putaway-claims/{task_id}"),
                serde_json::to_vec(&ClaimPutawayByIdRequest::default())?,
                ResponseKind::Claim,
            )
        }
        RfCommand::Putaway(PutawayCommand::ConfirmLoose {
            task_id,
            destination_location_barcode,
            expected_policy,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/putaway-tasks/{task_id}/confirmations"),
                serde_json::to_vec(&ConfirmPutawayRequest {
                    destination_location_barcode: destination_location_barcode.clone(),
                    expected_policy: map_putaway_expectation(expected_policy),
                })?,
                ResponseKind::LooseConfirmation,
            )
        }
        RfCommand::Putaway(PutawayCommand::ConfirmLicensePlate {
            task_id,
            license_plate_barcode,
            destination_location_barcode,
            expected_policy,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/license-plate-putaway-tasks/{task_id}/confirmations"),
                serde_json::to_vec(&ConfirmLicensePlatePutawayRequest {
                    license_plate_barcode: license_plate_barcode.clone(),
                    destination_location_barcode: destination_location_barcode.clone(),
                    expected_policy: map_putaway_expectation(expected_policy),
                })?,
                ResponseKind::LicensePlateConfirmation,
            )
        }
        RfCommand::Putaway(PutawayCommand::Release {
            task_id,
            reason,
            note,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/putaway-claims/{task_id}/releases"),
                serde_json::to_vec(&ReleasePutawayClaimRequest {
                    reason: map_release_reason(*reason),
                    note: note.clone(),
                })?,
                ResponseKind::Release,
            )
        }
        RfCommand::InventoryRelocation(InventoryRelocationCommand::ClaimNext { workflow }) => (
            format!("{API_PREFIX}/inventory-relocation-claims/next"),
            serde_json::to_vec(&ClaimNextInventoryRelocationRequest {
                workflow: map_relocation_workflow(*workflow),
            })?,
            ResponseKind::RelocationOptionalClaim,
        ),
        RfCommand::InventoryRelocation(InventoryRelocationCommand::ClaimById { task_id }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/inventory-relocation-claims/{task_id}"),
                serde_json::to_vec(&ClaimInventoryRelocationByIdRequest::default())?,
                ResponseKind::RelocationClaim,
            )
        }
        RfCommand::InventoryRelocation(InventoryRelocationCommand::ConfirmLoose {
            task_id,
            destination_location_barcode,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/inventory-relocation-tasks/{task_id}/confirmations"),
                serde_json::to_vec(&ConfirmInventoryRelocationRequest {
                    destination_location_barcode: destination_location_barcode.clone(),
                    license_plate_barcode: None,
                })?,
                ResponseKind::RelocationConfirmation,
            )
        }
        RfCommand::InventoryRelocation(InventoryRelocationCommand::ConfirmLicensePlate {
            task_id,
            license_plate_barcode,
            destination_location_barcode,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/inventory-relocation-tasks/{task_id}/confirmations"),
                serde_json::to_vec(&ConfirmInventoryRelocationRequest {
                    destination_location_barcode: destination_location_barcode.clone(),
                    license_plate_barcode: Some(license_plate_barcode.clone()),
                })?,
                ResponseKind::RelocationConfirmation,
            )
        }
        RfCommand::InventoryRelocation(InventoryRelocationCommand::Release {
            task_id,
            reason,
            note,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/inventory-relocation-claims/{task_id}/releases"),
                serde_json::to_vec(&ReleaseInventoryRelocationClaimRequest {
                    reason: map_relocation_release_reason(*reason),
                    note: note.clone(),
                })?,
                ResponseKind::RelocationRelease,
            )
        }
        RfCommand::CycleCount(CycleCountCommand::ClaimNext) => (
            format!("{API_PREFIX}/cycle-count-claims/next"),
            serde_json::to_vec(&ClaimNextCycleCountRequest::default())?,
            ResponseKind::CycleCountOptionalClaim,
        ),
        RfCommand::CycleCount(CycleCountCommand::ClaimById { task_id }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/cycle-count-claims/{task_id}"),
                serde_json::to_vec(&ClaimCycleCountByIdRequest::default())?,
                ResponseKind::CycleCountClaim,
            )
        }
        RfCommand::CycleCount(CycleCountCommand::Confirm {
            task_id,
            location_barcode,
            item_barcode,
            license_plate_barcode,
            counted_quantity,
            note,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/cycle-count-tasks/{task_id}/confirmations"),
                serde_json::to_vec(&ConfirmCycleCountRequest {
                    location_barcode: location_barcode.clone(),
                    item_barcode: item_barcode.clone(),
                    license_plate_barcode: license_plate_barcode.clone(),
                    counted_quantity: *counted_quantity,
                    note: note.clone(),
                })?,
                ResponseKind::CycleCountConfirmation,
            )
        }
        RfCommand::CycleCount(CycleCountCommand::Release {
            task_id,
            reason,
            note,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/cycle-count-claims/{task_id}/releases"),
                serde_json::to_vec(&ReleaseCycleCountClaimRequest {
                    reason: map_cycle_count_release_reason(*reason),
                    note: note.clone(),
                })?,
                ResponseKind::CycleCountRelease,
            )
        }
        RfCommand::Picking(PickingCommand::ClaimNext) => (
            format!("{API_PREFIX}/picking-claims/next"),
            serde_json::to_vec(&ClaimNextPickRequest::default())?,
            ResponseKind::PickOptionalClaim,
        ),
        RfCommand::Picking(PickingCommand::ClaimCluster { cluster_id }) => {
            validate_task_id(*cluster_id)?;
            (
                format!("{API_PREFIX}/pick-clusters/{cluster_id}/claims/next"),
                serde_json::to_vec(
                    &wareboxes_api_contract::v1::ClaimNextClusterPickRequest::default(),
                )?,
                ResponseKind::PickOptionalClaim,
            )
        }
        RfCommand::Picking(PickingCommand::ClaimById { task_id }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/picking-claims/{task_id}"),
                serde_json::to_vec(&ClaimPickByIdRequest::default())?,
                ResponseKind::PickClaim,
            )
        }
        RfCommand::Picking(PickingCommand::Confirm {
            task_id,
            content_id,
            source_location_barcode,
            item_barcode,
            source_license_plate_barcode,
            destination_license_plate_barcode,
        }) => {
            validate_task_id(*task_id)?;
            if *content_id <= 0 {
                return Err(WireRequestError::InvalidPickContentId);
            }
            (
                format!("{API_PREFIX}/picking-tasks/{task_id}/contents/{content_id}/confirmations"),
                serde_json::to_vec(&ConfirmPickContentRequest {
                    source_location_barcode: source_location_barcode.clone(),
                    item_barcode: item_barcode.clone(),
                    source_license_plate_barcode: source_license_plate_barcode.clone(),
                    destination_license_plate_barcode: destination_license_plate_barcode.clone(),
                })?,
                ResponseKind::PickConfirmation,
            )
        }
        RfCommand::Picking(PickingCommand::ReportShortage(command)) => {
            validate_task_id(command.task_id)?;
            if command.content_id <= 0 {
                return Err(WireRequestError::InvalidPickContentId);
            }
            validate_pick_shortage_command(
                &command.source_location_barcode,
                command.source_license_plate_barcode.as_deref(),
                command.observed_item_barcode.as_deref(),
                command.observed_lot.as_deref(),
                command.observed_serial.as_deref(),
                command.reason,
                command.note.as_deref(),
                &command.outcome,
            )?;
            let outcome = match &command.outcome {
                PickShortageOutcome::NoPick => ApiReportPickShortageOutcome::NoPick {},
                PickShortageOutcome::Partial {
                    picked_quantity,
                    destination_license_plate_barcode,
                } => ApiReportPickShortageOutcome::Partial {
                    picked_quantity: *picked_quantity,
                    destination_license_plate_barcode: destination_license_plate_barcode.clone(),
                },
            };
            (
                format!(
                    "{API_PREFIX}/picking-tasks/{}/contents/{}/short-picks",
                    command.task_id, command.content_id
                ),
                serde_json::to_vec(&ReportPickShortageRequest {
                    source_location_barcode: command.source_location_barcode.clone(),
                    source_license_plate_barcode: command.source_license_plate_barcode.clone(),
                    observed_item_barcode: command.observed_item_barcode.clone(),
                    observed_lot: command.observed_lot.clone(),
                    observed_serial: command.observed_serial.clone(),
                    details: ApiPickShortageDetails {
                        reason: map_pick_shortage_reason(command.reason),
                        note: command.note.clone(),
                    },
                    outcome,
                })?,
                ResponseKind::PickShortageReport,
            )
        }
        RfCommand::Picking(PickingCommand::Release {
            task_id,
            reason,
            note,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/picking-claims/{task_id}/releases"),
                serde_json::to_vec(&ReleasePickClaimRequest {
                    reason: map_pick_release_reason(*reason),
                    note: note.clone(),
                })?,
                ResponseKind::PickRelease,
            )
        }
        RfCommand::Replenishment(command) => replenishment::build_command_parts(command)?,
        RfCommand::CrossDock(command) => cross_dock::build_command_parts(command)?,
        RfCommand::OutboundLoad(command) => outbound_load::build_command_parts(command)?,
        RfCommand::ExpectedReceipt(intent) => {
            if !intent.is_current_and_valid() {
                return Err(WireRequestError::InvalidExpectedReceiptField {
                    field: "confirmation intent",
                });
            }
            match intent.as_ref() {
                ReceivingCommandIntent::Unloading(intent) => (
                    format!(
                        "{API_PREFIX}/inbound-loads/{}/unloading-starts",
                        intent.load_id.get()
                    ),
                    serde_json::to_vec(&StartInboundLoadUnloadingRequest {
                        load_scan: intent.command.load_scan.as_str().to_owned(),
                        receiving_location_scan: intent
                            .command
                            .receiving_location_scan
                            .as_str()
                            .to_owned(),
                        seal_scan: intent
                            .command
                            .seal_scan
                            .as_ref()
                            .map(|value| value.as_str().to_owned()),
                        started_at: None,
                    })?,
                    ResponseKind::InboundUnloadingStart,
                ),
                ReceivingCommandIntent::Expected(intent) => {
                    let confirmation = map_expected_receipt_command(&intent.command);
                    let (path, body) = build_expected_receipt_confirmation_parts(
                        intent.load_line_id.get(),
                        &confirmation,
                    )?;
                    (path, body, ResponseKind::ExpectedReceiptConfirmation)
                }
                ReceivingCommandIntent::Unexpected(intent) => (
                    format!(
                        "{API_PREFIX}/expected-receiving/loads/{}/unexpected-receipts",
                        intent.load_id.get()
                    ),
                    serde_json::to_vec(&map_unexpected_receipt_command(&intent.command))?,
                    ResponseKind::UnexpectedReceiptConfirmation,
                ),
            }
        }
    };
    let body_sha256 = Sha256::digest(&body).into();

    Ok(DurableHttpRequest {
        method: HttpMethod::Post,
        path,
        content_type: JSON_CONTENT_TYPE.to_owned(),
        body,
        body_sha256,
        response_kind,
    })
}

pub fn decode_command_response(
    response_kind: ResponseKind,
    status: u16,
    body: &[u8],
) -> Result<CommandOutcome, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    match response_kind {
        ResponseKind::OptionalClaim => {
            let claim = serde_json::from_slice::<Option<PutawayClaimResponse>>(body)?;
            Ok(CommandOutcome::PutawayClaimed(
                claim.map(map_claim).transpose()?.map(Box::new),
            ))
        }
        ResponseKind::Claim => Ok(CommandOutcome::PutawayClaimed(Some(Box::new(map_claim(
            serde_json::from_slice::<PutawayClaimResponse>(body)?,
        )?)))),
        ResponseKind::LooseConfirmation => {
            let response = serde_json::from_slice::<PutawayConfirmationResponse>(body)?;
            Ok(CommandOutcome::PutawayConfirmed {
                task_id: response.task_id,
            })
        }
        ResponseKind::LicensePlateConfirmation => {
            let response = serde_json::from_slice::<LicensePlatePutawayConfirmationResponse>(body)?;
            Ok(CommandOutcome::PutawayConfirmed {
                task_id: response.task_id,
            })
        }
        ResponseKind::Release => {
            let response = serde_json::from_slice::<
                wareboxes_api_contract::v1::PutawayClaimReleaseResponse,
            >(body)?;
            Ok(CommandOutcome::PutawayReleased {
                task_id: response.task_id,
            })
        }
        ResponseKind::RelocationOptionalClaim => {
            let claim = serde_json::from_slice::<Option<InventoryRelocationClaimResponse>>(body)?;
            Ok(CommandOutcome::InventoryRelocationClaimed(
                claim.map(map_relocation_claim).transpose()?.map(Box::new),
            ))
        }
        ResponseKind::RelocationClaim => Ok(CommandOutcome::InventoryRelocationClaimed(Some(
            Box::new(map_relocation_claim(serde_json::from_slice::<
                InventoryRelocationClaimResponse,
            >(body)?)?),
        ))),
        ResponseKind::RelocationConfirmation => {
            let response = serde_json::from_slice::<InventoryRelocationConfirmationResponse>(body)?;
            Ok(CommandOutcome::InventoryRelocationConfirmed {
                task_id: response.task_id,
            })
        }
        ResponseKind::RelocationRelease => {
            let response = serde_json::from_slice::<
                wareboxes_api_contract::v1::InventoryRelocationClaimReleaseResponse,
            >(body)?;
            Ok(CommandOutcome::InventoryRelocationReleased {
                task_id: response.task_id,
            })
        }
        ResponseKind::CycleCountOptionalClaim => {
            let claim = serde_json::from_slice::<Option<CycleCountClaimResponse>>(body)?;
            Ok(CommandOutcome::CycleCountClaimed(
                claim.map(map_cycle_count_claim).transpose()?.map(Box::new),
            ))
        }
        ResponseKind::CycleCountClaim => Ok(CommandOutcome::CycleCountClaimed(Some(Box::new(
            map_cycle_count_claim(serde_json::from_slice::<CycleCountClaimResponse>(body)?)?,
        )))),
        ResponseKind::CycleCountConfirmation => {
            let response = serde_json::from_slice::<CycleCountConfirmationResponse>(body)?;
            Ok(CommandOutcome::CycleCountConfirmed {
                task_id: response.task_id,
            })
        }
        ResponseKind::CycleCountRelease => {
            let response = serde_json::from_slice::<
                wareboxes_api_contract::v1::CycleCountClaimReleaseResponse,
            >(body)?;
            Ok(CommandOutcome::CycleCountReleased {
                task_id: response.task_id,
            })
        }
        ResponseKind::PickOptionalClaim => {
            let claim = serde_json::from_slice::<Option<PickClaimResponse>>(body)?;
            Ok(CommandOutcome::PickClaimed(
                claim.map(map_pick_claim).transpose()?.map(Box::new),
            ))
        }
        ResponseKind::PickClaim => Ok(CommandOutcome::PickClaimed(Some(Box::new(map_pick_claim(
            serde_json::from_slice::<PickClaimResponse>(body)?,
        )?)))),
        ResponseKind::PickConfirmation => {
            let response = serde_json::from_slice::<PickContentConfirmationResponse>(body)?;
            Ok(CommandOutcome::PickConfirmed {
                task_id: response.task_id,
                content_id: response.content_id,
                task_completed: response.task_completed,
                order_ready_to_pack: response.order_ready_to_pack,
            })
        }
        ResponseKind::PickShortageReport => {
            let response = serde_json::from_slice::<ReportPickShortageResponse>(body)?;
            Ok(CommandOutcome::PickShortageReported(Box::new(
                map_pick_shortage_response(response)?,
            )))
        }
        ResponseKind::PickRelease => {
            let response = serde_json::from_slice::<
                wareboxes_api_contract::v1::PickClaimReleaseResponse,
            >(body)?;
            Ok(CommandOutcome::PickReleased {
                task_id: response.task_id,
            })
        }
        ResponseKind::ReplenishmentOptionalClaim
        | ResponseKind::ReplenishmentClaim
        | ResponseKind::ReplenishmentConfirmation
        | ResponseKind::ReplenishmentRelease => replenishment::decode_outcome(response_kind, body),
        ResponseKind::CrossDockOptionalClaim
        | ResponseKind::CrossDockClaim
        | ResponseKind::CrossDockConfirmation
        | ResponseKind::CrossDockRelease => cross_dock::decode_outcome(response_kind, body),
        ResponseKind::OutboundCartonMovement => outbound_load::decode_response(body),
        ResponseKind::InboundUnloadingStart => {
            let response = serde_json::from_slice::<StartInboundLoadUnloadingResponse>(body)?;
            if response.unloading_start_id <= 0
                || response.load_id <= 0
                || response.receiving_location_id <= 0
                || response.started_by <= 0
                || response.status
                    != wareboxes_api_contract::v1::InboundLoadReceivingStatus::Receiving
                || DateTime::parse_from_rfc3339(&response.started_at).is_err()
            {
                return Err(WireResponseError::InvalidInboundUnloadingStart);
            }
            Ok(CommandOutcome::InboundUnloadingStarted(
                crate::expected_receiving::UnloadingStartResult {
                    unloading_start_id: response.unloading_start_id,
                    load_id: response
                        .load_id
                        .try_into()
                        .map_err(|_| WireResponseError::InvalidInboundUnloadingStart)?,
                    receiving_location_id: response
                        .receiving_location_id
                        .try_into()
                        .map_err(|_| WireResponseError::InvalidInboundUnloadingStart)?,
                    started_by: response.started_by,
                    started_at: response.started_at,
                },
            ))
        }
        ResponseKind::ExpectedReceiptConfirmation => {
            let response = decode_expected_receipt_confirmation_response_from_body(status, body)?;
            Ok(CommandOutcome::ExpectedReceipt(response))
        }
        ResponseKind::UnexpectedReceiptConfirmation => Ok(CommandOutcome::UnexpectedReceipt(
            Box::new(decode_unexpected_receipt_confirmation(status, body)?),
        )),
    }
}

fn map_unexpected_receipt_command(
    command: &UnexpectedReceiptCommand,
) -> ConfirmUnexpectedReceiptRequest {
    ConfirmUnexpectedReceiptRequest {
        item_barcode: command.item_barcode.as_str().to_owned(),
        receiving_location_barcode: command.receiving_location_barcode.as_str().to_owned(),
        quantity: command.quantity.get(),
        license_plate_barcode: command
            .license_plate_barcode
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        lot: command.lot.as_ref().map(|value| value.as_str().to_owned()),
        serial: command
            .serial
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        expiration: command
            .expiration
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        reason: map_unexpected_receipt_reason(command.reason),
        note: command.note.as_ref().map(|value| value.as_str().to_owned()),
        expected_policy: ApiReceiptPolicyExpectation {
            source: map_receipt_policy_source(command.receipt_policy.source()),
            configuration_id: command.receipt_policy.configuration_id(),
            configuration_revision: command.receipt_policy.configuration_revision(),
            policy_hash: command.receipt_policy.policy_hash().to_owned(),
        },
    }
}

const fn map_receipt_policy_source(source: ReceiptPolicySource) -> ApiReceiptPolicySource {
    match source {
        ReceiptPolicySource::ProductDefault => ApiReceiptPolicySource::ProductDefault,
        ReceiptPolicySource::Configuration => ApiReceiptPolicySource::Configuration,
    }
}

fn map_receipt_policy(
    response: ApiReceiptPolicyResponse,
) -> Result<ReceiptPolicy, WireResponseError> {
    let scope = response
        .configuration_scope
        .map(map_receipt_policy_scope)
        .transpose()?;
    ReceiptPolicy::try_new(ReceiptPolicyInput {
        source: match response.source {
            ApiReceiptPolicySource::ProductDefault => ReceiptPolicySource::ProductDefault,
            ApiReceiptPolicySource::Configuration => ReceiptPolicySource::Configuration,
        },
        configuration_id: response.configuration_id,
        configuration_revision: response.configuration_revision,
        configuration_scope: scope,
        allow_unexpected: response.allow_unexpected,
        quarantine_unmapped_items: response.quarantine_unmapped_items,
        over_receipt_tolerance_basis_points: response.over_receipt_tolerance_basis_points,
        policy_hash: response.policy_hash,
    })
    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)
}

fn map_receipt_policy_scope(
    scope: ApiConfigurationScope,
) -> Result<ReceiptPolicyScope, WireResponseError> {
    match scope {
        ApiConfigurationScope::Tenant => Ok(ReceiptPolicyScope::Tenant),
        ApiConfigurationScope::InventoryOwner { inventory_owner_id } => {
            Ok(ReceiptPolicyScope::InventoryOwner {
                inventory_owner_id: inventory_owner_id
                    .try_into()
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
            })
        }
        ApiConfigurationScope::Facility { facility_id } => Ok(ReceiptPolicyScope::Facility {
            facility_id: facility_id
                .try_into()
                .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        }),
        ApiConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => Ok(ReceiptPolicyScope::OwnerFacility {
            inventory_owner_id: inventory_owner_id
                .try_into()
                .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
            facility_id: facility_id
                .try_into()
                .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        }),
    }
}

const fn map_unexpected_receipt_reason(
    reason: UnexpectedReceiptReason,
) -> ApiUnexpectedReceiptReason {
    match reason {
        UnexpectedReceiptReason::Excess => ApiUnexpectedReceiptReason::Excess,
        UnexpectedReceiptReason::UnexpectedItem => ApiUnexpectedReceiptReason::UnexpectedItem,
        UnexpectedReceiptReason::BlindReceipt => ApiUnexpectedReceiptReason::BlindReceipt,
        UnexpectedReceiptReason::MisShipped => ApiUnexpectedReceiptReason::MisShipped,
        UnexpectedReceiptReason::Other => ApiUnexpectedReceiptReason::Other,
    }
}

const fn map_api_unexpected_receipt_reason(
    reason: ApiUnexpectedReceiptReason,
) -> UnexpectedReceiptReason {
    match reason {
        ApiUnexpectedReceiptReason::Excess => UnexpectedReceiptReason::Excess,
        ApiUnexpectedReceiptReason::UnexpectedItem => UnexpectedReceiptReason::UnexpectedItem,
        ApiUnexpectedReceiptReason::BlindReceipt => UnexpectedReceiptReason::BlindReceipt,
        ApiUnexpectedReceiptReason::MisShipped => UnexpectedReceiptReason::MisShipped,
        ApiUnexpectedReceiptReason::Other => UnexpectedReceiptReason::Other,
    }
}

fn decode_unexpected_receipt_confirmation(
    status: u16,
    body: &[u8],
) -> Result<UnexpectedReceiptResult, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    let response = serde_json::from_slice::<UnexpectedReceiptConfirmationResponse>(body)?;
    let text_is_valid = valid_response_text(&response.uom, MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH)
        && valid_response_text(
            &response.observed_item_barcode,
            MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
        )
        && valid_response_text(
            &response.observed_receiving_location_barcode,
            MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
        )
        && response
            .license_plate_barcode
            .as_deref()
            .is_none_or(|value| valid_response_text(value, MAX_EXPECTED_RECEIVING_BARCODE_LENGTH))
        && response.lot.as_deref().is_none_or(|value| {
            valid_response_text(value, MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH)
        })
        && response.serial.as_deref().is_none_or(|value| {
            valid_response_text(value, MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH)
        })
        && response
            .note
            .as_deref()
            .is_none_or(|value| valid_response_text(value, MAX_EXPECTED_RECEIVING_NOTE_LENGTH));
    if response.unexpected_receipt_id <= 0
        || response.inventory_transaction_id <= 0
        || response.inventory_balance_id <= 0
        || response.item_batch_id <= 0
        || response.inventory_hold_id <= 0
        || response.confirmed_by_user_id <= 0
        || response.license_plate_id.is_some_and(|id| id <= 0)
        || response.license_plate_id.is_some() != response.license_plate_barcode.is_some()
        || response.inventory_status
            != wareboxes_api_contract::v1::InventoryBalanceStatus::Quarantine
        || !matches!(
            response.load_status,
            ExpectedReceivingLoadStatus::Arrived
                | ExpectedReceivingLoadStatus::Receiving
                | ExpectedReceivingLoadStatus::Received
        )
        || DateTime::parse_from_rfc3339(&response.confirmed_at).is_err()
        || response
            .expiration
            .as_deref()
            .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
        || response.reason == ApiUnexpectedReceiptReason::Other && response.note.is_none()
        || !text_is_valid
    {
        return Err(WireResponseError::InvalidUnexpectedReceiptConfirmation);
    }
    let receipt_policy = map_receipt_policy(response.receipt_policy)
        .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?;
    Ok(UnexpectedReceiptResult {
        unexpected_receipt_id: response.unexpected_receipt_id,
        load_id: response
            .load_id
            .try_into()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        inventory_owner_id: response
            .inventory_owner_id
            .try_into()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        facility_id: response
            .facility_id
            .try_into()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        item_id: response
            .item_id
            .try_into()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        uom: StockDimension::new(response.uom)
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        quantity: response
            .quantity
            .try_into()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        receiving_location_id: response
            .receiving_location_id
            .try_into()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        observed_item_barcode: ItemBarcode::new(response.observed_item_barcode)
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        observed_receiving_location_barcode: DockBarcode::new(
            response.observed_receiving_location_barcode,
        )
        .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        inventory_transaction_id: response.inventory_transaction_id,
        inventory_balance_id: response.inventory_balance_id,
        item_batch_id: response.item_batch_id,
        license_plate_id: response.license_plate_id,
        license_plate_barcode: response
            .license_plate_barcode
            .map(LicensePlateBarcode::new)
            .transpose()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        lot: response
            .lot
            .map(StockDimension::new)
            .transpose()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        serial: response
            .serial
            .map(StockDimension::new)
            .transpose()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        expiration: response
            .expiration
            .map(Expiration::new)
            .transpose()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        inventory_hold_id: response.inventory_hold_id,
        reason: map_api_unexpected_receipt_reason(response.reason),
        note: response
            .note
            .map(ExceptionNote::new)
            .transpose()
            .map_err(|_| WireResponseError::InvalidUnexpectedReceiptConfirmation)?,
        load_status: match response.load_status {
            ExpectedReceivingLoadStatus::Arrived => ReceivingLoadStatus::Arrived,
            ExpectedReceivingLoadStatus::Receiving => ReceivingLoadStatus::Receiving,
            ExpectedReceivingLoadStatus::Received => ReceivingLoadStatus::Received,
        },
        confirmed_by_user_id: response.confirmed_by_user_id,
        confirmed_at: response.confirmed_at,
        receipt_policy,
    })
}

pub fn decode_claim_response(body: &[u8]) -> Result<Option<PutawayClaim>, WireResponseError> {
    serde_json::from_slice::<Option<PutawayClaimResponse>>(body)?
        .map(map_claim)
        .transpose()
}

pub fn decode_relocation_claim_response(
    body: &[u8],
) -> Result<Option<InventoryRelocationClaim>, WireResponseError> {
    serde_json::from_slice::<Option<InventoryRelocationClaimResponse>>(body)?
        .map(map_relocation_claim)
        .transpose()
}

pub fn decode_cycle_count_claim_response(
    body: &[u8],
) -> Result<Option<CycleCountClaim>, WireResponseError> {
    serde_json::from_slice::<Option<CycleCountClaimResponse>>(body)?
        .map(map_cycle_count_claim)
        .transpose()
}

pub fn decode_pick_claim_response(body: &[u8]) -> Result<Option<PickClaim>, WireResponseError> {
    serde_json::from_slice::<Option<PickClaimResponse>>(body)?
        .map(map_pick_claim)
        .transpose()
}

pub fn decode_pick_heartbeat_response(
    expected_task_id: i64,
    status: u16,
    body: &[u8],
) -> Result<PickClaimHeartbeatResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    if expected_task_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    let response = serde_json::from_slice::<PickClaimHeartbeatResponse>(body)?;
    validate_heartbeat(
        expected_task_id,
        response.task_id,
        &response.heartbeat_at,
        &response.lease_expires_at,
    )?;
    Ok(response)
}

fn validate_heartbeat(
    expected_task_id: i64,
    actual_task_id: i64,
    heartbeat_at: &str,
    lease_expires_at: &str,
) -> Result<(), WireResponseError> {
    if actual_task_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    if actual_task_id != expected_task_id {
        return Err(WireResponseError::HeartbeatTaskMismatch {
            expected: expected_task_id,
            actual: actual_task_id,
        });
    }
    if DateTime::parse_from_rfc3339(heartbeat_at).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp {
            field: "heartbeat_at",
        });
    }
    if DateTime::parse_from_rfc3339(lease_expires_at).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp {
            field: "lease_expires_at",
        });
    }
    Ok(())
}

pub fn decode_heartbeat_response(
    expected_task_id: i64,
    status: u16,
    body: &[u8],
) -> Result<PutawayClaimHeartbeatResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    if expected_task_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }

    let response = serde_json::from_slice::<PutawayClaimHeartbeatResponse>(body)?;
    if response.task_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    if response.task_id != expected_task_id {
        return Err(WireResponseError::HeartbeatTaskMismatch {
            expected: expected_task_id,
            actual: response.task_id,
        });
    }
    if DateTime::parse_from_rfc3339(&response.heartbeat_at).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp {
            field: "heartbeat_at",
        });
    }
    if DateTime::parse_from_rfc3339(&response.lease_expires_at).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp {
            field: "lease_expires_at",
        });
    }
    Ok(response)
}

pub fn decode_relocation_heartbeat_response(
    expected_task_id: i64,
    status: u16,
    body: &[u8],
) -> Result<InventoryRelocationClaimHeartbeatResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    if expected_task_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    let response = serde_json::from_slice::<InventoryRelocationClaimHeartbeatResponse>(body)?;
    if response.task_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    if response.task_id != expected_task_id {
        return Err(WireResponseError::HeartbeatTaskMismatch {
            expected: expected_task_id,
            actual: response.task_id,
        });
    }
    if DateTime::parse_from_rfc3339(&response.heartbeat_at).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp {
            field: "heartbeat_at",
        });
    }
    if DateTime::parse_from_rfc3339(&response.lease_expires_at).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp {
            field: "lease_expires_at",
        });
    }
    Ok(response)
}

pub fn decode_cycle_count_heartbeat_response(
    expected_task_id: i64,
    status: u16,
    body: &[u8],
) -> Result<CycleCountClaimHeartbeatResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    if expected_task_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    let response = serde_json::from_slice::<CycleCountClaimHeartbeatResponse>(body)?;
    if response.task_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    if response.task_id != expected_task_id {
        return Err(WireResponseError::HeartbeatTaskMismatch {
            expected: expected_task_id,
            actual: response.task_id,
        });
    }
    if DateTime::parse_from_rfc3339(&response.heartbeat_at).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp {
            field: "heartbeat_at",
        });
    }
    if DateTime::parse_from_rfc3339(&response.lease_expires_at).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp {
            field: "lease_expires_at",
        });
    }
    Ok(response)
}

pub fn decode_expected_receiving_session_response(
    expected_load_id: Option<i64>,
    status: u16,
    body: &[u8],
) -> Result<ExpectedReceivingSessionResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    if expected_load_id.is_some_and(|load_id| load_id <= 0) {
        return Err(WireResponseError::InvalidExpectedReceivingSession);
    }

    let response = serde_json::from_slice::<ExpectedReceivingSessionResponse>(body)?;
    validate_expected_receiving_session(&response)?;
    if let Some(expected_load_id) = expected_load_id
        && response.load_id != expected_load_id
    {
        return Err(WireResponseError::ExpectedReceivingLoadMismatch {
            expected: expected_load_id,
            actual: response.load_id,
        });
    }
    Ok(response)
}

pub fn decode_receiving_session(
    expected_load_id: Option<i64>,
    status: u16,
    body: &[u8],
) -> Result<ReceivingSession, WireResponseError> {
    let response = decode_expected_receiving_session_response(expected_load_id, status, body)?;
    map_receiving_session(response)
}

pub fn decode_expected_receipt_confirmation_response(
    expected_load_line_id: i64,
    status: u16,
    body: &[u8],
) -> Result<ExpectedReceiptConfirmationResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    if expected_load_line_id <= 0 {
        return Err(WireResponseError::InvalidExpectedReceiptConfirmation);
    }

    let response = decode_expected_receipt_confirmation_contract(status, body)?;
    if response.load_line_id != expected_load_line_id {
        return Err(WireResponseError::ExpectedReceiptLineMismatch {
            expected: expected_load_line_id,
            actual: response.load_line_id,
        });
    }
    Ok(response)
}

fn decode_expected_receipt_confirmation_response_from_body(
    status: u16,
    body: &[u8],
) -> Result<ConfirmationResult, WireResponseError> {
    let response = decode_expected_receipt_confirmation_contract(status, body)?;
    Ok(ConfirmationResult {
        load_id: response
            .load_id
            .try_into()
            .map_err(|_| WireResponseError::InvalidExpectedReceiptConfirmation)?,
        load_line_id: response
            .load_line_id
            .try_into()
            .map_err(|_| WireResponseError::InvalidExpectedReceiptConfirmation)?,
        disposition: map_expected_receipt_disposition(response.disposition),
        quantity: PositiveQuantity::try_from(response.quantity)
            .map_err(|_| WireResponseError::InvalidExpectedReceiptConfirmation)?,
        cumulative_received: NonNegativeQuantity::new(response.cumulative_received_quantity)
            .map_err(|_| WireResponseError::InvalidExpectedReceiptConfirmation)?,
        cumulative_rejected: NonNegativeQuantity::new(response.cumulative_rejected_quantity)
            .map_err(|_| WireResponseError::InvalidExpectedReceiptConfirmation)?,
        cumulative_missing: NonNegativeQuantity::new(response.cumulative_missing_quantity)
            .map_err(|_| WireResponseError::InvalidExpectedReceiptConfirmation)?,
        remaining: NonNegativeQuantity::new(response.remaining_quantity)
            .map_err(|_| WireResponseError::InvalidExpectedReceiptConfirmation)?,
        receive_completed: response.receive_completed,
    })
}

fn decode_expected_receipt_confirmation_contract(
    status: u16,
    body: &[u8],
) -> Result<ExpectedReceiptConfirmationResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    let response = serde_json::from_slice::<ExpectedReceiptConfirmationResponse>(body)?;
    validate_expected_receipt_confirmation_response(&response)?;
    Ok(response)
}

fn map_claim(response: PutawayClaimResponse) -> Result<PutawayClaim, WireResponseError> {
    if response.task_id <= 0
        || response.inventory_owner_id <= 0
        || response.facility_id <= 0
        || response.destination_location.location_id <= 0
        || response.destination_location.barcode.trim().is_empty()
    {
        return Err(WireResponseError::InvalidClaim);
    }
    let source = map_source(response.source_location)?;
    let policy = map_putaway_policy(response.putaway_policy)?;
    let work = match response.work {
        PutawayClaimWork::Loose {
            item_id,
            item_description,
            uom,
            lot,
            serial,
            quantity,
            ..
        } => MovementWork::Loose {
            item_description,
            item_id,
            quantity,
            uom,
            lot,
            serial,
        },
        PutawayClaimWork::LicensePlate {
            license_plate_barcode,
            planned_balance_count,
            ..
        } => MovementWork::LicensePlate {
            barcode: license_plate_barcode,
            planned_balance_count,
        },
    };
    Ok(PutawayClaim::with_policy(
        MovementClaimDetails {
            task_id: response.task_id,
            inventory_owner_id: response.inventory_owner_id,
            facility_id: response.facility_id,
            priority: response.priority,
            instructions: response.instructions,
            lease_expires_at: response.lease_expires_at,
            source: Some(source),
            destination: Location {
                location_id: response.destination_location.location_id,
                name: response.destination_location.name,
                barcode: Some(response.destination_location.barcode),
            },
            work,
        },
        policy,
    ))
}

fn map_putaway_expectation(value: &PutawayPolicyExpectation) -> ApiPutawayPolicyExpectation {
    ApiPutawayPolicyExpectation {
        source: match value.source {
            PutawayPolicySource::ProductDefault => ApiPutawayPolicySource::ProductDefault,
            PutawayPolicySource::Configuration => ApiPutawayPolicySource::Configuration,
        },
        configuration_id: value.configuration_id,
        configuration_revision: value.configuration_revision,
        policy_hash: value.policy_hash.clone(),
    }
}

fn map_putaway_policy(value: ApiPutawayPolicyResponse) -> Result<PutawayPolicy, WireResponseError> {
    if value.policy_hash.len() != 64
        || !value
            .policy_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || matches!(value.source, ApiPutawayPolicySource::ProductDefault)
            && (value.configuration_id.is_some() || value.configuration_revision.is_some())
        || matches!(value.source, ApiPutawayPolicySource::Configuration)
            && (value.configuration_id.is_none() || value.configuration_revision.is_none())
    {
        return Err(WireResponseError::InvalidClaim);
    }
    Ok(PutawayPolicy {
        source: match value.source {
            ApiPutawayPolicySource::ProductDefault => PutawayPolicySource::ProductDefault,
            ApiPutawayPolicySource::Configuration => PutawayPolicySource::Configuration,
        },
        configuration_id: value.configuration_id,
        configuration_revision: value.configuration_revision,
        require_zone_compatibility: value.require_zone_compatibility,
        enforce_location_capacity: value.enforce_location_capacity,
        allow_mixed_lots: value.allow_mixed_lots,
        policy_hash: value.policy_hash,
    })
}

fn map_relocation_claim(
    response: InventoryRelocationClaimResponse,
) -> Result<InventoryRelocationClaim, WireResponseError> {
    let destination_barcode = response
        .destination_location
        .barcode
        .filter(|barcode| !barcode.trim().is_empty())
        .ok_or(WireResponseError::InvalidClaim)?;
    if response.task_id <= 0
        || response.inventory_owner_id <= 0
        || response.facility_id <= 0
        || response.source_location.location_id <= 0
        || response.destination_location.location_id <= 0
    {
        return Err(WireResponseError::InvalidClaim);
    }
    let work = match response.work {
        InventoryRelocationClaimWork::LooseBalance {
            item_id,
            item_description,
            uom,
            lot,
            serial,
            quantity,
            ..
        } => MovementWork::Loose {
            item_description,
            item_id,
            quantity,
            uom,
            lot,
            serial,
        },
        InventoryRelocationClaimWork::LicensePlate {
            license_plate_barcode,
            planned_balance_count,
            ..
        } => MovementWork::LicensePlate {
            barcode: license_plate_barcode,
            planned_balance_count,
        },
    };
    Ok(InventoryRelocationClaim::new(MovementClaimDetails {
        task_id: response.task_id,
        inventory_owner_id: response.inventory_owner_id,
        facility_id: response.facility_id,
        priority: response.priority,
        instructions: response.instructions,
        lease_expires_at: response.lease_expires_at,
        source: Some(Location {
            location_id: response.source_location.location_id,
            name: response.source_location.name,
            barcode: response
                .source_location
                .barcode
                .filter(|barcode| !barcode.trim().is_empty()),
        }),
        destination: Location {
            location_id: response.destination_location.location_id,
            name: response.destination_location.name,
            barcode: Some(destination_barcode),
        },
        work,
    }))
}

fn map_cycle_count_claim(
    response: CycleCountClaimResponse,
) -> Result<CycleCountClaim, WireResponseError> {
    if response.task_id <= 0
        || response.inventory_owner_id <= 0
        || response.facility_id <= 0
        || response.location.location_id <= 0
        || response.location.barcode.trim().is_empty()
        || response.item.item_id <= 0
        || response.item.barcodes.is_empty()
        || response
            .item
            .barcodes
            .iter()
            .any(|barcode| barcode.trim().is_empty())
        || response.stock.inventory_balance_id <= 0
        || response.stock.uom.trim().is_empty()
        || response
            .stock
            .license_plate_barcode
            .as_ref()
            .is_some_and(|barcode| barcode.trim().is_empty())
        || DateTime::parse_from_rfc3339(&response.lease_expires_at).is_err()
    {
        return Err(WireResponseError::InvalidClaim);
    }
    let inventory_status = match response.stock.inventory_status {
        wareboxes_api_contract::v1::InventoryBalanceStatus::Available => "available",
        wareboxes_api_contract::v1::InventoryBalanceStatus::Hold => "hold",
        wareboxes_api_contract::v1::InventoryBalanceStatus::Damaged => "damaged",
        wareboxes_api_contract::v1::InventoryBalanceStatus::Quarantine => "quarantine",
    };
    Ok(CycleCountClaim {
        task_id: response.task_id,
        inventory_owner_id: response.inventory_owner_id,
        facility_id: response.facility_id,
        priority: response.priority,
        instructions: response.instructions,
        lease_expires_at: response.lease_expires_at,
        location_id: response.location.location_id,
        location_name: response.location.name,
        location_barcode: response.location.barcode,
        item_id: response.item.item_id,
        item_description: response.item.description,
        item_barcodes: response.item.barcodes,
        inventory_balance_id: response.stock.inventory_balance_id,
        license_plate_barcode: response.stock.license_plate_barcode,
        uom: response.stock.uom,
        lot: response.stock.lot,
        serial: response.stock.serial,
        inventory_status: inventory_status.into(),
    })
}

fn map_pick_claim(response: PickClaimResponse) -> Result<PickClaim, WireResponseError> {
    let content = response.content;
    let policy = response.pick_policy;
    let policy_identity_valid = match policy.source {
        ApiPickDecisionPolicySource::ProductDefault => {
            policy.configuration_id.is_none()
                && policy.configuration_revision.is_none()
                && policy.configuration_scope.is_none()
                && policy.require_source_location_scan
                && policy.require_item_scan
                && policy.require_destination_container_scan
        }
        ApiPickDecisionPolicySource::Configuration => {
            policy.configuration_id.is_some_and(|id| id > 0)
                && policy
                    .configuration_revision
                    .is_some_and(|revision| revision > 0)
                && policy.configuration_scope.is_some()
        }
    };
    if response.task_id <= 0
        || response.order_id <= 0
        || response.inventory_owner_id <= 0
        || response.facility_id <= 0
        || response.order_key.trim().is_empty()
        || response.destination_location_id <= 0
        || response.destination_location_barcode.trim().is_empty()
        || content.content_id <= 0
        || content.order_line_id <= 0
        || content.inventory_allocation_id <= 0
        || content.source_inventory_balance_id <= 0
        || content.item_batch_id <= 0
        || content.source_location_id <= 0
        || content.source_location_barcode.trim().is_empty()
        || content.item_id <= 0
        || content.item_barcodes.is_empty()
        || content
            .item_barcodes
            .iter()
            .any(|barcode| barcode.trim().is_empty())
        || content.uom.trim().is_empty()
        || content.planned_quantity <= 0
        || content.state != wareboxes_api_contract::v1::PickContentState::Pending
        || content
            .source_license_plate_barcode
            .as_ref()
            .is_some_and(|barcode| barcode.trim().is_empty())
        || DateTime::parse_from_rfc3339(&response.lease_expires_at).is_err()
        || !policy_identity_valid
        || policy.policy_hash.len() != 64
        || !policy
            .policy_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || response
            .suggested_destination_license_plate_barcode
            .as_ref()
            .is_some_and(|barcode| barcode.trim().is_empty())
        || (policy.require_destination_container_scan
            && response
                .suggested_destination_license_plate_barcode
                .is_some())
    {
        return Err(WireResponseError::InvalidClaim);
    }
    if content.source_license_plate_id.is_some() != content.source_license_plate_barcode.is_some() {
        return Err(WireResponseError::InvalidClaim);
    }
    Ok(PickClaim {
        task_id: response.task_id,
        order_id: response.order_id,
        inventory_owner_id: response.inventory_owner_id,
        facility_id: response.facility_id,
        order_key: response.order_key,
        order_revision: response.order_revision.get(),
        priority: response.priority,
        ship_by: response.ship_by,
        lease_expires_at: response.lease_expires_at,
        destination_location_id: response.destination_location_id,
        destination_location_barcode: response.destination_location_barcode,
        destination_location_name: response.destination_location_name,
        execution: match response.execution.method {
            wareboxes_api_contract::v1::PickExecutionMethod::Discrete => {
                if response.execution.cluster_id.is_some()
                    || response.execution.cart_barcode.is_some()
                    || response.execution.slot_code.is_some()
                    || response.execution.sequence.is_some()
                    || response.execution.task_count.is_some()
                    || content.uom.eq_ignore_ascii_case("case")
                {
                    return Err(WireResponseError::InvalidClaim);
                }
                crate::picking::PickExecutionEvidence::discrete()
            }
            wareboxes_api_contract::v1::PickExecutionMethod::Case => {
                if response.execution.cluster_id.is_some()
                    || response.execution.cart_barcode.is_some()
                    || response.execution.slot_code.is_some()
                    || response.execution.sequence.is_some()
                    || response.execution.task_count.is_some()
                    || !content.uom.eq_ignore_ascii_case("case")
                {
                    return Err(WireResponseError::InvalidClaim);
                }
                crate::picking::PickExecutionEvidence::case()
            }
            wareboxes_api_contract::v1::PickExecutionMethod::ClusterCart => {
                let execution = response.execution;
                if execution.cluster_id.is_none_or(|value| value <= 0)
                    || execution
                        .cart_barcode
                        .as_ref()
                        .is_none_or(|value| value.trim().is_empty())
                    || execution
                        .slot_code
                        .as_ref()
                        .is_none_or(|value| value.trim().is_empty())
                    || execution.sequence.is_none_or(|value| value <= 0)
                    || execution.task_count.is_none_or(|value| value < 2)
                {
                    return Err(WireResponseError::InvalidClaim);
                }
                crate::picking::PickExecutionEvidence {
                    method: crate::picking::PickExecutionMethod::ClusterCart,
                    cluster_id: execution.cluster_id,
                    cart_barcode: execution.cart_barcode,
                    slot_code: execution.slot_code,
                    sequence: execution.sequence,
                    task_count: execution.task_count,
                }
            }
        },
        pick_policy: PickDecisionPolicy {
            source: match policy.source {
                ApiPickDecisionPolicySource::ProductDefault => {
                    PickDecisionPolicySource::ProductDefault
                }
                ApiPickDecisionPolicySource::Configuration => {
                    PickDecisionPolicySource::Configuration
                }
            },
            configuration_id: policy.configuration_id,
            configuration_revision: policy.configuration_revision,
            configuration_scope: policy.configuration_scope.map(|scope| match scope {
                ApiConfigurationScope::Tenant => PickDecisionPolicyScope::Tenant,
                ApiConfigurationScope::InventoryOwner { inventory_owner_id } => {
                    PickDecisionPolicyScope::InventoryOwner { inventory_owner_id }
                }
                ApiConfigurationScope::Facility { facility_id } => {
                    PickDecisionPolicyScope::Facility { facility_id }
                }
                ApiConfigurationScope::OwnerFacility {
                    inventory_owner_id,
                    facility_id,
                } => PickDecisionPolicyScope::OwnerFacility {
                    inventory_owner_id,
                    facility_id,
                },
            }),
            require_source_location_scan: policy.require_source_location_scan,
            require_item_scan: policy.require_item_scan,
            require_destination_container_scan: policy.require_destination_container_scan,
            policy_hash: policy.policy_hash,
        },
        suggested_destination_license_plate_barcode: response
            .suggested_destination_license_plate_barcode,
        content: PickClaimContent {
            content_id: content.content_id,
            order_line_id: content.order_line_id,
            inventory_allocation_id: content.inventory_allocation_id,
            source_inventory_balance_id: content.source_inventory_balance_id,
            item_batch_id: content.item_batch_id,
            source_location_id: content.source_location_id,
            source_location_barcode: content.source_location_barcode,
            source_location_name: content.source_location_name,
            source_license_plate_id: content.source_license_plate_id,
            source_license_plate_barcode: content.source_license_plate_barcode,
            item_id: content.item_id,
            item_description: content.item_description,
            item_barcodes: content.item_barcodes,
            uom: content.uom,
            lot: content.lot,
            serial: content.serial,
            expiration: content.expiration,
            planned_quantity: content.planned_quantity,
            state: PickContentState::Pending,
        },
    })
}

fn map_pick_shortage_response(
    response: ReportPickShortageResponse,
) -> Result<PickShortageReportResult, WireResponseError> {
    let quantities = response.quantities;
    let details = response.details;
    let scans_are_valid = [
        response.observed_item_barcode.as_deref(),
        response.observed_lot.as_deref(),
        response.observed_serial.as_deref(),
    ]
    .into_iter()
    .flatten()
    .all(valid_pick_scan);
    let note_is_valid = details.note.as_deref().is_none_or(|note| {
        !note.is_empty()
            && note.trim() == note
            && note.chars().count() <= 500
            && !note.chars().any(char::is_control)
    });
    let movement_is_valid = match (quantities.picked, response.movement.as_ref()) {
        (0, None) => true,
        (picked, Some(movement)) if picked > 0 => {
            movement.inventory_transaction_id > 0
                && movement.source_inventory_allocation_id > 0
                && movement.destination_inventory_allocation_id > 0
                && movement.source_inventory_balance_id > 0
                && movement.destination_inventory_balance_id > 0
                && movement.source_location_id > 0
                && movement.destination_location_id > 0
                && movement
                    .source_license_plate_id
                    .is_none_or(|license_plate_id| license_plate_id > 0)
                && movement.destination_license_plate_id > 0
                && movement.picked_quantity == picked
        }
        _ => false,
    };
    if response.shortage_id <= 0
        || response.task_id <= 0
        || response.content_id <= 0
        || response.order_id <= 0
        || quantities.planned <= 0
        || quantities.picked < 0
        || quantities.picked >= quantities.planned
        || quantities.short != quantities.planned - quantities.picked
        || response.shortage_status != ApiPickShortageStatus::AwaitingInventory
        || response.reallocated_quantity != 0
        || response.recovery_terminal_quantity != 0
        || response.remaining_to_allocate_quantity != quantities.short
        || response.hold.hold_id <= 0
        || response.hold.inventory_balance_id <= 0
        || response.hold.held_quantity <= 0
        || response.reported_by <= 0
        || DateTime::parse_from_rfc3339(&response.reported_at).is_err()
        || !scans_are_valid
        || !note_is_valid
        || (details.reason == ApiPickShortageReason::Other && details.note.is_none())
        || !movement_is_valid
    {
        return Err(WireResponseError::InvalidPickShortageResponse);
    }

    Ok(PickShortageReportResult {
        shortage_id: response.shortage_id,
        shortage_revision: response.shortage_revision.get(),
        task_id: response.task_id,
        content_id: response.content_id,
        order_id: response.order_id,
        order_revision: response.order_revision.get(),
        planned_quantity: quantities.planned,
        picked_quantity: quantities.picked,
        short_quantity: quantities.short,
        reason: map_api_pick_shortage_reason(details.reason),
        note: details.note,
        observed_item_barcode: response.observed_item_barcode,
        observed_lot: response.observed_lot,
        observed_serial: response.observed_serial,
        status: PickShortageStatus::AwaitingInventory,
    })
}

fn map_receiving_session(
    response: ExpectedReceivingSessionResponse,
) -> Result<ReceivingSession, WireResponseError> {
    let status = match response.status {
        ExpectedReceivingLoadStatus::Arrived => ReceivingLoadStatus::Arrived,
        ExpectedReceivingLoadStatus::Receiving => ReceivingLoadStatus::Receiving,
        ExpectedReceivingLoadStatus::Received => ReceivingLoadStatus::Received,
    };
    let lines = response
        .lines
        .into_iter()
        .map(|line| {
            DomainExpectedReceiptLine::try_new(ExpectedReceiptLineInput {
                load_line_id: LoadLineId::try_from(line.load_line_id)
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                item_id: ItemId::try_from(line.item_id)
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                item_description: line.item_description,
                uom: StockDimension::new(line.uom)
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                item_barcodes: line
                    .item_barcodes
                    .into_iter()
                    .map(ItemBarcode::new)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                expected: PositiveQuantity::try_from(line.expected_quantity)
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                received: NonNegativeQuantity::new(line.received_quantity)
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                rejected: NonNegativeQuantity::new(line.rejected_quantity)
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                missing: NonNegativeQuantity::new(line.missing_quantity)
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                remaining: NonNegativeQuantity::new(line.remaining_quantity)
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                lot: line
                    .lot
                    .map(StockDimension::new)
                    .transpose()
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                serial: line
                    .serial
                    .map(StockDimension::new)
                    .transpose()
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
                expiration: line
                    .expiration
                    .map(Expiration::new)
                    .transpose()
                    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
            })
            .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let receipt_policy = map_receipt_policy(response.receipt_policy)?;
    ReceivingSession::try_new(ReceivingSessionInput {
        load_id: LoadId::try_from(response.load_id)
            .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        inventory_owner_id: InventoryOwnerId::try_from(response.inventory_owner_id)
            .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        facility_id: FacilityId::try_from(response.facility_id)
            .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        reference_number: response.reference_number,
        status,
        expected_seal: response
            .expected_seal
            .map(SealBarcode::new)
            .transpose()
            .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        dock: ReceivingDock::new(
            LocationId::try_from(response.receiving_location.location_id)
                .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
            DockBarcode::new(response.receiving_location.barcode)
                .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
            response.receiving_location.name,
        ),
        receipt_policy,
        lines,
    })
    .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)
}

fn map_source(source: PutawayClaimSourceLocation) -> Result<Location, WireResponseError> {
    if source.location_id <= 0 {
        return Err(WireResponseError::InvalidClaim);
    }
    Ok(Location {
        location_id: source.location_id,
        name: source.name,
        barcode: source.barcode.filter(|barcode| !barcode.trim().is_empty()),
    })
}

fn validate_expected_receipt_confirmation(
    confirmation: &ConfirmExpectedReceiptRequest,
) -> Result<(), WireRequestError> {
    let quantity = match confirmation {
        ConfirmExpectedReceiptRequest::Received {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            expiration,
        } => {
            validate_expected_receiving_text(
                item_barcode,
                "item barcode",
                MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
            )?;
            validate_expected_receiving_text(
                receiving_location_barcode,
                "receiving location barcode",
                MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
            )?;
            validate_optional_expected_receiving_text(
                license_plate_barcode.as_deref(),
                "license plate barcode",
                MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
            )?;
            validate_optional_expected_receiving_text(
                lot.as_deref(),
                "lot",
                MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH,
            )?;
            validate_optional_expected_receiving_text(
                serial.as_deref(),
                "serial",
                MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH,
            )?;
            if expiration
                .as_deref()
                .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
            {
                return Err(WireRequestError::InvalidExpectedReceiptField {
                    field: "expiration",
                });
            }
            *quantity
        }
        ConfirmExpectedReceiptRequest::Quarantined {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            expiration,
            reason,
            note,
        } => {
            validate_expected_receiving_text(
                item_barcode,
                "item barcode",
                MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
            )?;
            validate_expected_receiving_text(
                receiving_location_barcode,
                "receiving location barcode",
                MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
            )?;
            validate_optional_expected_receiving_text(
                license_plate_barcode.as_deref(),
                "license plate barcode",
                MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
            )?;
            validate_optional_expected_receiving_text(
                lot.as_deref(),
                "lot",
                MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH,
            )?;
            validate_optional_expected_receiving_text(
                serial.as_deref(),
                "serial",
                MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH,
            )?;
            validate_optional_expected_receiving_text(
                note.as_deref(),
                "note",
                MAX_EXPECTED_RECEIVING_NOTE_LENGTH,
            )?;
            if expiration
                .as_deref()
                .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
            {
                return Err(WireRequestError::InvalidExpectedReceiptField {
                    field: "expiration",
                });
            }
            if *reason == ExpectedReceiptQuarantineReason::Other && note.is_none() {
                return Err(WireRequestError::ExpectedReceiptNoteRequired);
            }
            *quantity
        }
        ConfirmExpectedReceiptRequest::Rejected {
            item_barcode,
            quantity,
            reason,
            note,
        } => {
            validate_expected_receiving_text(
                item_barcode,
                "item barcode",
                MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
            )?;
            validate_optional_expected_receiving_text(
                note.as_deref(),
                "note",
                MAX_EXPECTED_RECEIVING_NOTE_LENGTH,
            )?;
            if *reason == wareboxes_api_contract::v1::ExpectedReceiptExceptionReason::Other
                && note.is_none()
            {
                return Err(WireRequestError::ExpectedReceiptNoteRequired);
            }
            *quantity
        }
        ConfirmExpectedReceiptRequest::Missing {
            quantity,
            reason,
            note,
        } => {
            validate_optional_expected_receiving_text(
                note.as_deref(),
                "note",
                MAX_EXPECTED_RECEIVING_NOTE_LENGTH,
            )?;
            if *reason == wareboxes_api_contract::v1::ExpectedReceiptExceptionReason::Other
                && note.is_none()
            {
                return Err(WireRequestError::ExpectedReceiptNoteRequired);
            }
            *quantity
        }
    };
    if quantity <= 0 {
        return Err(WireRequestError::InvalidExpectedReceiptField { field: "quantity" });
    }
    Ok(())
}

fn map_expected_receipt_command(command: &ExpectedReceiptCommand) -> ConfirmExpectedReceiptRequest {
    match command {
        ExpectedReceiptCommand::Received {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            expiration,
        } => ConfirmExpectedReceiptRequest::Received {
            item_barcode: item_barcode.as_str().to_owned(),
            receiving_location_barcode: receiving_location_barcode.as_str().to_owned(),
            quantity: quantity.get(),
            license_plate_barcode: license_plate_barcode
                .as_ref()
                .map(|barcode| barcode.as_str().to_owned()),
            lot: lot.as_ref().map(|value| value.as_str().to_owned()),
            serial: serial.as_ref().map(|value| value.as_str().to_owned()),
            expiration: expiration.as_ref().map(|value| value.as_str().to_owned()),
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
        } => ConfirmExpectedReceiptRequest::Quarantined {
            item_barcode: item_barcode.as_str().to_owned(),
            receiving_location_barcode: receiving_location_barcode.as_str().to_owned(),
            quantity: quantity.get(),
            license_plate_barcode: license_plate_barcode
                .as_ref()
                .map(|barcode| barcode.as_str().to_owned()),
            lot: lot.as_ref().map(|value| value.as_str().to_owned()),
            serial: serial.as_ref().map(|value| value.as_str().to_owned()),
            expiration: expiration.as_ref().map(|value| value.as_str().to_owned()),
            reason: map_expected_receipt_quarantine_reason(*reason),
            note: note.as_ref().map(|value| value.as_str().to_owned()),
        },
        ExpectedReceiptCommand::Rejected {
            item_barcode,
            quantity,
            reason,
            note,
        } => ConfirmExpectedReceiptRequest::Rejected {
            item_barcode: item_barcode.as_str().to_owned(),
            quantity: quantity.get(),
            reason: map_expected_receipt_exception_reason(*reason),
            note: note.as_ref().map(|value| value.as_str().to_owned()),
        },
        ExpectedReceiptCommand::Missing {
            quantity,
            reason,
            note,
        } => ConfirmExpectedReceiptRequest::Missing {
            quantity: quantity.get(),
            reason: map_expected_receipt_exception_reason(*reason),
            note: note.as_ref().map(|value| value.as_str().to_owned()),
        },
    }
}

const fn map_expected_receipt_quarantine_reason(
    reason: ReceiptQuarantineReason,
) -> ExpectedReceiptQuarantineReason {
    match reason {
        ReceiptQuarantineReason::Damaged => ExpectedReceiptQuarantineReason::Damaged,
        ReceiptQuarantineReason::QualityInspection => {
            ExpectedReceiptQuarantineReason::QualityInspection
        }
        ReceiptQuarantineReason::CountDiscrepancy => {
            ExpectedReceiptQuarantineReason::CountDiscrepancy
        }
        ReceiptQuarantineReason::WrongItem => ExpectedReceiptQuarantineReason::WrongItem,
        ReceiptQuarantineReason::Other => ExpectedReceiptQuarantineReason::Other,
    }
}

const fn map_expected_receipt_exception_reason(
    reason: ReceiptExceptionReason,
) -> ExpectedReceiptExceptionReason {
    match reason {
        ReceiptExceptionReason::Damaged => ExpectedReceiptExceptionReason::Damaged,
        ReceiptExceptionReason::QualityRejected => ExpectedReceiptExceptionReason::QualityRejected,
        ReceiptExceptionReason::ShortShipment => ExpectedReceiptExceptionReason::ShortShipment,
        ReceiptExceptionReason::CountDiscrepancy => {
            ExpectedReceiptExceptionReason::CountDiscrepancy
        }
        ReceiptExceptionReason::WrongItem => ExpectedReceiptExceptionReason::WrongItem,
        ReceiptExceptionReason::Other => ExpectedReceiptExceptionReason::Other,
    }
}

const fn map_expected_receipt_disposition(
    disposition: ExpectedReceiptDisposition,
) -> ConfirmationMode {
    match disposition {
        ExpectedReceiptDisposition::Received => ConfirmationMode::Received,
        ExpectedReceiptDisposition::Quarantined => ConfirmationMode::Quarantined,
        ExpectedReceiptDisposition::Rejected => ConfirmationMode::Rejected,
        ExpectedReceiptDisposition::Missing => ConfirmationMode::Missing,
    }
}

fn validate_expected_receiving_text(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), WireRequestError> {
    if value.trim() != value
        || value.is_empty()
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        return Err(WireRequestError::InvalidExpectedReceiptField { field });
    }
    Ok(())
}

fn validate_optional_expected_receiving_text(
    value: Option<&str>,
    field: &'static str,
    maximum: usize,
) -> Result<(), WireRequestError> {
    if let Some(value) = value {
        validate_expected_receiving_text(value, field, maximum)?;
    }
    Ok(())
}

fn validate_expected_receiving_session(
    response: &ExpectedReceivingSessionResponse,
) -> Result<(), WireResponseError> {
    if response.load_id <= 0
        || response.inventory_owner_id <= 0
        || response.facility_id <= 0
        || response.receiving_location.location_id <= 0
        || (response.status != ExpectedReceivingLoadStatus::Received && response.lines.is_empty())
        || !valid_response_text(
            &response.receiving_location.barcode,
            MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
        )
        || response
            .expected_seal
            .as_deref()
            .is_some_and(|seal| !valid_response_text(seal, MAX_EXPECTED_RECEIVING_BARCODE_LENGTH))
    {
        return Err(WireResponseError::InvalidExpectedReceivingSession);
    }

    for line in &response.lines {
        let resolved = line
            .received_quantity
            .checked_add(line.rejected_quantity)
            .and_then(|quantity| quantity.checked_add(line.missing_quantity));
        let total = resolved.and_then(|quantity| quantity.checked_add(line.remaining_quantity));
        if line.load_line_id <= 0
            || line.item_id <= 0
            || !valid_response_text(&line.uom, MAX_EXPECTED_RECEIVING_DIMENSION_LENGTH)
            || line.item_barcodes.is_empty()
            || line
                .item_barcodes
                .iter()
                .any(|barcode| !valid_response_text(barcode, MAX_EXPECTED_RECEIVING_BARCODE_LENGTH))
            || line.expected_quantity <= 0
            || line.received_quantity < 0
            || line.rejected_quantity < 0
            || line.missing_quantity < 0
            || line.remaining_quantity <= 0
            || total != Some(line.expected_quantity)
            || line
                .expiration
                .as_deref()
                .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
        {
            return Err(WireResponseError::InvalidExpectedReceivingSession);
        }
    }
    Ok(())
}

fn validate_expected_receipt_confirmation_response(
    response: &ExpectedReceiptConfirmationResponse,
) -> Result<(), WireResponseError> {
    let optional_ids = [
        response.inventory_transaction_id,
        response.inventory_balance_id,
        response.item_batch_id,
        response.license_plate_id,
        response.inventory_hold_id,
    ];
    let inventory_shape_is_valid = match response.disposition {
        ExpectedReceiptDisposition::Received => {
            response.inventory_transaction_id.is_some()
                && response.inventory_balance_id.is_some()
                && response.item_batch_id.is_some()
                && response.inventory_hold_id.is_none()
                && response.inventory_status
                    == Some(wareboxes_api_contract::v1::InventoryBalanceStatus::Available)
        }
        ExpectedReceiptDisposition::Quarantined => {
            response.inventory_transaction_id.is_some()
                && response.inventory_balance_id.is_some()
                && response.item_batch_id.is_some()
                && response.inventory_hold_id.is_some()
                && response.inventory_status
                    == Some(wareboxes_api_contract::v1::InventoryBalanceStatus::Quarantine)
        }
        ExpectedReceiptDisposition::Rejected | ExpectedReceiptDisposition::Missing => {
            optional_ids.iter().all(Option::is_none) && response.inventory_status.is_none()
        }
    };
    let completion_is_valid = if response.receive_completed {
        response.remaining_quantity == 0
            && response.load_status == ExpectedReceivingLoadStatus::Received
    } else {
        response.load_status == ExpectedReceivingLoadStatus::Receiving
    };
    if response.load_id <= 0
        || response.load_line_id <= 0
        || response.quantity <= 0
        || optional_ids.into_iter().flatten().any(|id| id <= 0)
        || response.cumulative_received_quantity < 0
        || response.cumulative_rejected_quantity < 0
        || response.cumulative_missing_quantity < 0
        || response.remaining_quantity < 0
        || !inventory_shape_is_valid
        || !completion_is_valid
    {
        return Err(WireResponseError::InvalidExpectedReceiptConfirmation);
    }
    Ok(())
}

fn valid_response_text(value: &str, maximum: usize) -> bool {
    value.trim() == value
        && !value.is_empty()
        && value.chars().count() <= maximum
        && !value.chars().any(char::is_control)
}

fn validate_draft(draft: &DurableCommandDraft) -> Result<(), WireRequestError> {
    if draft.schema_version != 1 {
        return Err(WireRequestError::UnsupportedSchema(draft.schema_version));
    }
    if draft.command_id.is_empty()
        || draft.command_id.len() > 128
        || !draft.command_id.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(WireRequestError::InvalidCommandId);
    }
    IdempotencyKey::new(draft.idempotency_key.clone())?;
    Ok(())
}

fn validate_task_id(task_id: i64) -> Result<(), WireRequestError> {
    if task_id <= 0 {
        return Err(WireRequestError::InvalidTaskId);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_pick_shortage_command(
    source_location_barcode: &str,
    source_license_plate_barcode: Option<&str>,
    observed_item_barcode: Option<&str>,
    observed_lot: Option<&str>,
    observed_serial: Option<&str>,
    reason: PickShortageReason,
    note: Option<&str>,
    outcome: &PickShortageOutcome,
) -> Result<(), WireRequestError> {
    if !valid_pick_scan(source_location_barcode) {
        return Err(WireRequestError::InvalidPickShortageField {
            field: "source location barcode",
        });
    }
    for (field, value) in [
        ("source license plate barcode", source_license_plate_barcode),
        ("observed item barcode", observed_item_barcode),
        ("observed lot", observed_lot),
        ("observed serial", observed_serial),
    ] {
        if value.is_some_and(|value| !valid_pick_scan(value)) {
            return Err(WireRequestError::InvalidPickShortageField { field });
        }
    }
    if note.is_some_and(|note| {
        note.is_empty()
            || note.trim() != note
            || note.chars().count() > 500
            || note.chars().any(char::is_control)
    }) {
        return Err(WireRequestError::InvalidPickShortageField { field: "note" });
    }
    if reason == PickShortageReason::Other && note.is_none() {
        return Err(WireRequestError::InvalidPickShortageField { field: "note" });
    }

    let item_required = matches!(
        reason,
        PickShortageReason::InsufficientQuantity
            | PickShortageReason::DamagedInventory
            | PickShortageReason::WrongInventory
            | PickShortageReason::LotOrSerialMismatch
    ) || matches!(outcome, PickShortageOutcome::Partial { .. });
    if item_required && observed_item_barcode.is_none() {
        return Err(WireRequestError::InvalidPickShortageField {
            field: "observed item barcode",
        });
    }
    if reason == PickShortageReason::LotOrSerialMismatch
        && observed_lot.is_none()
        && observed_serial.is_none()
    {
        return Err(WireRequestError::InvalidPickShortageField {
            field: "observed lot or serial",
        });
    }
    if let PickShortageOutcome::Partial {
        picked_quantity,
        destination_license_plate_barcode,
    } = outcome
    {
        if !reason.supports_partial() || *picked_quantity <= 0 {
            return Err(WireRequestError::InvalidPickShortageField {
                field: "picked quantity",
            });
        }
        if !valid_pick_scan(destination_license_plate_barcode)
            || source_license_plate_barcode == Some(destination_license_plate_barcode.as_str())
        {
            return Err(WireRequestError::InvalidPickShortageField {
                field: "destination license plate barcode",
            });
        }
    }
    Ok(())
}

fn valid_pick_scan(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= 200
        && !value.chars().any(char::is_control)
}

const fn map_workflow(workflow: MovementKind) -> ApiPutawayWorkflow {
    match workflow {
        MovementKind::Loose => ApiPutawayWorkflow::Loose,
        MovementKind::LicensePlate => ApiPutawayWorkflow::LicensePlate,
    }
}

const fn map_relocation_workflow(workflow: MovementKind) -> InventoryRelocationWorkflow {
    match workflow {
        MovementKind::Loose => InventoryRelocationWorkflow::LooseBalance,
        MovementKind::LicensePlate => InventoryRelocationWorkflow::LicensePlate,
    }
}

const fn map_release_reason(reason: ReleaseReason) -> PutawayClaimReleaseReason {
    match reason {
        ReleaseReason::WorkInterrupted => PutawayClaimReleaseReason::WorkInterrupted,
        ReleaseReason::EquipmentUnavailable => PutawayClaimReleaseReason::EquipmentUnavailable,
        ReleaseReason::DestinationBlocked => PutawayClaimReleaseReason::DestinationBlocked,
        ReleaseReason::SafetyIssue => PutawayClaimReleaseReason::SafetyIssue,
        ReleaseReason::Other => PutawayClaimReleaseReason::Other,
    }
}

const fn map_relocation_release_reason(
    reason: ReleaseReason,
) -> InventoryRelocationClaimReleaseReason {
    match reason {
        ReleaseReason::WorkInterrupted => InventoryRelocationClaimReleaseReason::WorkInterrupted,
        ReleaseReason::EquipmentUnavailable => {
            InventoryRelocationClaimReleaseReason::EquipmentUnavailable
        }
        ReleaseReason::DestinationBlocked => {
            InventoryRelocationClaimReleaseReason::DestinationBlocked
        }
        ReleaseReason::SafetyIssue => InventoryRelocationClaimReleaseReason::SafetyIssue,
        ReleaseReason::Other => InventoryRelocationClaimReleaseReason::Other,
    }
}

const fn map_cycle_count_release_reason(reason: ReleaseReason) -> CycleCountClaimReleaseReason {
    match reason {
        ReleaseReason::WorkInterrupted => CycleCountClaimReleaseReason::WorkInterrupted,
        ReleaseReason::EquipmentUnavailable => CycleCountClaimReleaseReason::EquipmentUnavailable,
        ReleaseReason::SafetyIssue => CycleCountClaimReleaseReason::SafetyIssue,
        ReleaseReason::Other | ReleaseReason::DestinationBlocked => {
            CycleCountClaimReleaseReason::Other
        }
    }
}

const fn map_pick_release_reason(reason: PickReleaseReason) -> PickClaimReleaseReason {
    match reason {
        PickReleaseReason::WorkInterrupted => PickClaimReleaseReason::WorkInterrupted,
        PickReleaseReason::EquipmentUnavailable => PickClaimReleaseReason::EquipmentUnavailable,
        PickReleaseReason::SourceBlocked => PickClaimReleaseReason::SourceBlocked,
        PickReleaseReason::InventoryDiscrepancy => PickClaimReleaseReason::InventoryDiscrepancy,
        PickReleaseReason::SafetyIssue => PickClaimReleaseReason::SafetyIssue,
        PickReleaseReason::Other => PickClaimReleaseReason::Other,
    }
}

const fn map_pick_shortage_reason(reason: PickShortageReason) -> ApiPickShortageReason {
    match reason {
        PickShortageReason::InventoryMissing => ApiPickShortageReason::InventoryMissing,
        PickShortageReason::InsufficientQuantity => ApiPickShortageReason::InsufficientQuantity,
        PickShortageReason::DamagedInventory => ApiPickShortageReason::DamagedInventory,
        PickShortageReason::WrongInventory => ApiPickShortageReason::WrongInventory,
        PickShortageReason::LotOrSerialMismatch => ApiPickShortageReason::LotOrSerialMismatch,
        PickShortageReason::Other => ApiPickShortageReason::Other,
    }
}

const fn map_api_pick_shortage_reason(reason: ApiPickShortageReason) -> PickShortageReason {
    match reason {
        ApiPickShortageReason::InventoryMissing => PickShortageReason::InventoryMissing,
        ApiPickShortageReason::InsufficientQuantity => PickShortageReason::InsufficientQuantity,
        ApiPickShortageReason::DamagedInventory => PickShortageReason::DamagedInventory,
        ApiPickShortageReason::WrongInventory => PickShortageReason::WrongInventory,
        ApiPickShortageReason::LotOrSerialMismatch => PickShortageReason::LotOrSerialMismatch,
        ApiPickShortageReason::Other => PickShortageReason::Other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::picking::PickShortageCommand;

    fn draft(command: PutawayCommand) -> DurableCommandDraft {
        DurableCommandDraft {
            schema_version: 1,
            command_id: "command-1".into(),
            idempotency_key: "putaway:key-1".into(),
            command: command.into(),
        }
    }

    fn relocation_draft(command: InventoryRelocationCommand) -> DurableCommandDraft {
        DurableCommandDraft {
            schema_version: 1,
            command_id: "relocation-command-1".into(),
            idempotency_key: "relocation:key-1".into(),
            command: RfCommand::InventoryRelocation(command),
        }
    }

    fn cycle_count_draft(command: CycleCountCommand) -> DurableCommandDraft {
        DurableCommandDraft {
            schema_version: 1,
            command_id: "cycle-count-command-1".into(),
            idempotency_key: "cycle-count:key-1".into(),
            command: RfCommand::CycleCount(command),
        }
    }

    fn pick_draft(command: PickingCommand) -> DurableCommandDraft {
        DurableCommandDraft {
            schema_version: 1,
            command_id: "pick-command-1".into(),
            idempotency_key: "pick:key-1".into(),
            command: RfCommand::Picking(command),
        }
    }

    fn body(request: &DurableHttpRequest) -> serde_json::Value {
        serde_json::from_slice(&request.body).expect("request body should be valid JSON")
    }

    #[test]
    fn claim_next_uses_the_public_v1_contract() {
        let request = build_durable_request(&draft(PutawayCommand::ClaimNext {
            workflow: MovementKind::LicensePlate,
        }))
        .expect("claim request should build");

        assert_eq!(request.path, "/api/v1/putaway-claims/next");
        assert_eq!(body(&request), json!({"workflow": "license_plate"}));
        assert_eq!(request.response_kind, ResponseKind::OptionalClaim);
        assert!(request.verify_body());
    }

    #[test]
    fn selected_claim_has_an_exact_empty_body() {
        let request = build_durable_request(&draft(PutawayCommand::ClaimById { task_id: 42 }))
            .expect("selected claim should build");

        assert_eq!(request.path, "/api/v1/putaway-claims/42");
        assert_eq!(body(&request), json!({}));
        assert_eq!(request.response_kind, ResponseKind::Claim);
    }

    #[test]
    fn heartbeat_request_uses_the_public_v1_contract() {
        let (path, body) =
            build_heartbeat_request_parts(42).expect("heartbeat request should build");

        assert_eq!(path, "/api/v1/putaway-claims/42/heartbeats");
        assert_eq!(body, b"{}");
        assert!(matches!(
            build_heartbeat_request_parts(0),
            Err(WireRequestError::InvalidTaskId)
        ));
    }

    #[test]
    fn loose_confirmation_persists_the_scanned_destination() {
        let request = build_durable_request(&draft(PutawayCommand::ConfirmLoose {
            task_id: 42,
            destination_location_barcode: "A-01-02".into(),
            expected_policy: PutawayPolicy::product_default().expectation(),
        }))
        .expect("loose confirmation should build");

        assert_eq!(request.path, "/api/v1/putaway-tasks/42/confirmations");
        assert_eq!(
            body(&request),
            json!({
                "destination_location_barcode": "A-01-02",
                "expected_policy": {
                    "source": "product_default",
                    "configuration_id": null,
                    "configuration_revision": null,
                    "policy_hash": wareboxes_api_contract::v1::PRODUCT_DEFAULT_PUTAWAY_POLICY_HASH
                }
            })
        );
        assert_eq!(request.response_kind, ResponseKind::LooseConfirmation);
    }

    #[test]
    fn license_plate_confirmation_persists_both_scans() {
        let request = build_durable_request(&draft(PutawayCommand::ConfirmLicensePlate {
            task_id: 42,
            license_plate_barcode: "LP-42".into(),
            destination_location_barcode: "A-01-02".into(),
            expected_policy: PutawayPolicy::product_default().expectation(),
        }))
        .expect("license plate confirmation should build");

        assert_eq!(
            request.path,
            "/api/v1/license-plate-putaway-tasks/42/confirmations"
        );
        assert_eq!(
            body(&request),
            json!({
                "license_plate_barcode": "LP-42",
                "destination_location_barcode": "A-01-02",
                "expected_policy": {
                    "source": "product_default",
                    "configuration_id": null,
                    "configuration_revision": null,
                    "policy_hash": wareboxes_api_contract::v1::PRODUCT_DEFAULT_PUTAWAY_POLICY_HASH
                }
            })
        );
        assert_eq!(
            request.response_kind,
            ResponseKind::LicensePlateConfirmation
        );
    }

    #[test]
    fn release_uses_the_typed_reason_contract() {
        let request = build_durable_request(&draft(PutawayCommand::Release {
            task_id: 42,
            reason: ReleaseReason::DestinationBlocked,
            note: Some("Lane obstructed".into()),
        }))
        .expect("release should build");

        assert_eq!(request.path, "/api/v1/putaway-claims/42/releases");
        assert_eq!(
            body(&request),
            json!({
                "reason": "destination_blocked",
                "note": "Lane obstructed"
            })
        );
        assert_eq!(request.response_kind, ResponseKind::Release);
    }

    #[test]
    fn relocation_commands_use_the_typed_relocation_contract() {
        let claim =
            build_durable_request(&relocation_draft(InventoryRelocationCommand::ClaimNext {
                workflow: MovementKind::LicensePlate,
            }))
            .unwrap();
        assert_eq!(claim.path, "/api/v1/inventory-relocation-claims/next");
        assert_eq!(body(&claim), json!({"workflow": "license_plate"}));
        assert_eq!(claim.response_kind, ResponseKind::RelocationOptionalClaim);

        let loose = build_durable_request(&relocation_draft(
            InventoryRelocationCommand::ConfirmLoose {
                task_id: 42,
                destination_location_barcode: "A-01-02".into(),
            },
        ))
        .unwrap();
        assert_eq!(
            loose.path,
            "/api/v1/inventory-relocation-tasks/42/confirmations"
        );
        assert_eq!(
            body(&loose),
            json!({"destination_location_barcode": "A-01-02"})
        );

        let plate = build_durable_request(&relocation_draft(
            InventoryRelocationCommand::ConfirmLicensePlate {
                task_id: 43,
                license_plate_barcode: "LP-43".into(),
                destination_location_barcode: "B-02-03".into(),
            },
        ))
        .unwrap();
        assert_eq!(
            body(&plate),
            json!({
                "destination_location_barcode": "B-02-03",
                "license_plate_barcode": "LP-43"
            })
        );
        assert_eq!(plate.response_kind, ResponseKind::RelocationConfirmation);

        let release =
            build_durable_request(&relocation_draft(InventoryRelocationCommand::Release {
                task_id: 43,
                reason: ReleaseReason::SafetyIssue,
                note: None,
            }))
            .unwrap();
        assert_eq!(
            release.path,
            "/api/v1/inventory-relocation-claims/43/releases"
        );
        assert_eq!(body(&release), json!({"reason": "safety_issue"}));
        assert_eq!(release.response_kind, ResponseKind::RelocationRelease);
    }

    #[test]
    fn relocation_claim_and_results_decode_into_the_durable_workflow() {
        let claim = serde_json::to_vec(&json!({
            "task_id": 52,
            "inventory_owner_id": 7,
            "facility_id": 9,
            "priority": 60,
            "instructions": "Use aisle crossing 2",
            "due_at": null,
            "lease_expires_at": "2026-07-27T02:00:00Z",
            "source_location": {
                "location_id": 11,
                "barcode": "A-01-01",
                "name": "A-01-01"
            },
            "destination_location": {
                "location_id": 12,
                "barcode": "B-02-03",
                "name": "B-02-03"
            },
            "work": {
                "workflow": "license_plate",
                "license_plate_id": 13,
                "license_plate_barcode": "LP-52",
                "planned_balance_count": 3
            }
        }))
        .unwrap();
        let CommandOutcome::InventoryRelocationClaimed(Some(claim)) =
            decode_command_response(ResponseKind::RelocationOptionalClaim, 200, &claim).unwrap()
        else {
            panic!("expected relocation claim");
        };
        assert_eq!(claim.details().task_id, 52);
        assert_eq!(
            claim.details().source.as_ref().unwrap().barcode.as_deref(),
            Some("A-01-01")
        );
        assert_eq!(
            claim.details().work,
            MovementWork::LicensePlate {
                barcode: "LP-52".into(),
                planned_balance_count: 3
            }
        );

        let confirmation = serde_json::to_vec(&json!({
            "task_id": 52,
            "inventory_owner_id": 7,
            "facility_id": 9,
            "source_location_id": 11,
            "destination_location_id": 12,
            "destination_location_barcode": "B-02-03",
            "inventory_transaction_id": 19,
            "confirmed_by": 4,
            "confirmed_at": "2026-07-27T01:30:00Z",
            "result": {
                "workflow": "license_plate",
                "license_plate_id": 13,
                "license_plate_barcode": "LP-52",
                "moved_balance_count": 3
            }
        }))
        .unwrap();
        assert_eq!(
            decode_command_response(ResponseKind::RelocationConfirmation, 200, &confirmation)
                .unwrap(),
            CommandOutcome::InventoryRelocationConfirmed { task_id: 52 }
        );

        let release = serde_json::to_vec(&json!({
            "task_id": 52,
            "released_at": "2026-07-27T01:40:00Z",
            "release_count": 1,
            "reason": "work_interrupted"
        }))
        .unwrap();
        assert_eq!(
            decode_command_response(ResponseKind::RelocationRelease, 200, &release).unwrap(),
            CommandOutcome::InventoryRelocationReleased { task_id: 52 }
        );
    }

    #[test]
    fn cycle_count_confirmation_persists_every_scanned_identity() {
        let request = build_durable_request(&cycle_count_draft(CycleCountCommand::Confirm {
            task_id: 71,
            location_barcode: "A-01-02".into(),
            item_barcode: "ITEM-71".into(),
            license_plate_barcode: Some("LP-71".into()),
            counted_quantity: 0,
            note: Some("Location empty".into()),
        }))
        .unwrap();

        assert_eq!(request.path, "/api/v1/cycle-count-tasks/71/confirmations");
        assert_eq!(
            body(&request),
            json!({
                "location_barcode": "A-01-02",
                "item_barcode": "ITEM-71",
                "license_plate_barcode": "LP-71",
                "counted_quantity": 0,
                "note": "Location empty"
            })
        );
        assert_eq!(request.response_kind, ResponseKind::CycleCountConfirmation);
    }

    #[test]
    fn cycle_count_claim_is_blind_and_requires_a_scannable_item() {
        let response = json!({
            "task_id": 71,
            "inventory_owner_id": 7,
            "facility_id": 9,
            "priority": 80,
            "instructions": null,
            "due_at": null,
            "lease_expires_at": "2026-07-27T02:00:00Z",
            "location": {
                "location_id": 11,
                "barcode": "A-01-02",
                "name": "A-01-02"
            },
            "item": {
                "item_id": 15,
                "description": "Widget",
                "barcodes": ["ITEM-71"]
            },
            "stock": {
                "inventory_balance_id": 13,
                "license_plate_barcode": null,
                "uom": "each",
                "lot": "LOT-1",
                "expiration": null,
                "serial": null,
                "inventory_status": "available"
            }
        });
        assert!(response["stock"].get("quantity").is_none());
        let encoded = serde_json::to_vec(&response).unwrap();
        let CommandOutcome::CycleCountClaimed(Some(claim)) =
            decode_command_response(ResponseKind::CycleCountOptionalClaim, 200, &encoded).unwrap()
        else {
            panic!("expected cycle count claim");
        };
        assert_eq!(claim.task_id, 71);
        assert_eq!(claim.item_barcodes, vec!["ITEM-71"]);

        let mut invalid = response;
        invalid["item"]["barcodes"] = json!([]);
        assert!(matches!(
            decode_command_response(
                ResponseKind::CycleCountOptionalClaim,
                200,
                &serde_json::to_vec(&invalid).unwrap()
            ),
            Err(WireResponseError::InvalidClaim)
        ));
    }

    #[test]
    fn pick_commands_use_allocation_content_routes_and_exact_scans() {
        let next = build_durable_request(&pick_draft(PickingCommand::ClaimNext)).unwrap();
        assert_eq!(next.path, "/api/v1/picking-claims/next");
        assert_eq!(body(&next), json!({}));
        assert_eq!(next.response_kind, ResponseKind::PickOptionalClaim);

        let cluster =
            build_durable_request(&pick_draft(PickingCommand::ClaimCluster { cluster_id: 44 }))
                .unwrap();
        assert_eq!(cluster.path, "/api/v1/pick-clusters/44/claims/next");
        assert_eq!(body(&cluster), json!({}));
        assert_eq!(cluster.response_kind, ResponseKind::PickOptionalClaim);

        let confirmation = build_durable_request(&pick_draft(PickingCommand::Confirm {
            task_id: 71,
            content_id: 81,
            source_location_barcode: Some("A-01-02".into()),
            item_barcode: Some("ITEM-71".into()),
            source_license_plate_barcode: Some("LP-71".into()),
            destination_license_plate_barcode: Some("TOTE-9".into()),
        }))
        .unwrap();
        assert_eq!(
            confirmation.path,
            "/api/v1/picking-tasks/71/contents/81/confirmations"
        );
        assert_eq!(
            body(&confirmation),
            json!({
                "source_location_barcode": "A-01-02",
                "item_barcode": "ITEM-71",
                "source_license_plate_barcode": "LP-71",
                "destination_license_plate_barcode": "TOTE-9"
            })
        );
        assert_eq!(confirmation.response_kind, ResponseKind::PickConfirmation);

        let shortage = build_durable_request(&pick_draft(PickingCommand::ReportShortage(
            Box::new(PickShortageCommand {
                task_id: 71,
                content_id: 81,
                source_location_barcode: "A-01-02".into(),
                source_license_plate_barcode: Some("LP-71".into()),
                observed_item_barcode: Some("ITEM-71".into()),
                observed_lot: Some("LOT-1".into()),
                observed_serial: None,
                reason: PickShortageReason::InsufficientQuantity,
                note: Some("Two cases found".into()),
                outcome: PickShortageOutcome::Partial {
                    picked_quantity: 2,
                    destination_license_plate_barcode: "TOTE-9".into(),
                },
            }),
        )))
        .unwrap();
        assert_eq!(
            shortage.path,
            "/api/v1/picking-tasks/71/contents/81/short-picks"
        );
        assert_eq!(
            body(&shortage),
            json!({
                "source_location_barcode": "A-01-02",
                "source_license_plate_barcode": "LP-71",
                "observed_item_barcode": "ITEM-71",
                "observed_lot": "LOT-1",
                "details": {
                    "reason": "insufficient_quantity",
                    "note": "Two cases found"
                },
                "outcome": {
                    "kind": "partial",
                    "picked_quantity": 2,
                    "destination_license_plate_barcode": "TOTE-9"
                }
            })
        );
        assert_eq!(shortage.response_kind, ResponseKind::PickShortageReport);

        let release = build_durable_request(&pick_draft(PickingCommand::Release {
            task_id: 71,
            reason: PickReleaseReason::InventoryDiscrepancy,
            note: Some("Source is short".into()),
        }))
        .unwrap();
        assert_eq!(release.path, "/api/v1/picking-claims/71/releases");
        assert_eq!(
            body(&release),
            json!({"reason": "inventory_discrepancy", "note": "Source is short"})
        );
        assert_eq!(release.response_kind, ResponseKind::PickRelease);
    }

    #[test]
    fn pick_claim_decoder_preserves_allocation_and_scan_identity() {
        let response = json!({
            "task_id": 71,
            "order_id": 72,
            "inventory_owner_id": 7,
            "facility_id": 9,
            "order_key": "SO-72",
            "order_revision": 3,
            "priority": 80,
            "ship_by": "2026-08-09T20:00:00Z",
            "lease_expires_at": "2026-08-08T20:30:00Z",
            "destination_location_id": 10,
            "destination_location_barcode": "PACK-01",
            "destination_location_name": "Pack lane 1",
            "execution": {
                "method": "case",
                "cluster_id": null,
                "cart_barcode": null,
                "slot_code": null,
                "sequence": null,
                "task_count": null
            },
            "pick_policy": {
                "source": "product_default",
                "configuration_id": null,
                "configuration_revision": null,
                "configuration_scope": null,
                "require_source_location_scan": true,
                "require_item_scan": true,
                "require_destination_container_scan": true,
                "policy_hash": wareboxes_api_contract::v1::PRODUCT_DEFAULT_PICK_DECISION_POLICY_HASH
            },
            "suggested_destination_license_plate_barcode": null,
            "content": {
                "content_id": 81,
                "order_line_id": 82,
                "inventory_allocation_id": 83,
                "source_inventory_balance_id": 84,
                "item_batch_id": 85,
                "source_location_id": 11,
                "source_location_barcode": "A-01-02",
                "source_location_name": "Forward A-01-02",
                "source_license_plate_id": 12,
                "source_license_plate_barcode": "LP-71",
                "item_id": 13,
                "item_description": "Widget",
                "item_barcodes": ["ITEM-71", "000071"],
                "uom": "case",
                "lot": "LOT-1",
                "serial": null,
                "expiration": "2027-01-01T00:00:00Z",
                "planned_quantity": 4,
                "state": "pending"
            }
        });
        let encoded = serde_json::to_vec(&response).unwrap();

        let CommandOutcome::PickClaimed(Some(claim)) =
            decode_command_response(ResponseKind::PickOptionalClaim, 200, &encoded).unwrap()
        else {
            panic!("expected pick claim");
        };
        assert_eq!(claim.task_id, 71);
        assert_eq!(claim.order_revision, 3);
        assert_eq!(claim.content.content_id, 81);
        assert_eq!(claim.content.inventory_allocation_id, 83);
        assert_eq!(
            claim.execution.method,
            crate::picking::PickExecutionMethod::Case
        );
        assert_eq!(claim.content.item_barcodes, vec!["ITEM-71", "000071"]);
        assert_eq!(
            claim.content.source_license_plate_barcode.as_deref(),
            Some("LP-71")
        );

        let mut invalid = response;
        invalid["execution"]["method"] = json!("discrete");
        assert!(matches!(
            decode_command_response(
                ResponseKind::PickOptionalClaim,
                200,
                &serde_json::to_vec(&invalid).unwrap(),
            ),
            Err(WireResponseError::InvalidClaim)
        ));
    }

    #[test]
    fn pick_shortage_decoder_validates_initial_recovery_counters() {
        let response = json!({
            "shortage_id": 91,
            "shortage_revision": 1,
            "shortage_status": "awaiting_inventory",
            "task_id": 71,
            "content_id": 81,
            "order_id": 72,
            "order_revision": 4,
            "quantities": {"planned": 4, "picked": 0, "short": 4},
            "details": {"reason": "inventory_missing"},
            "reallocated_quantity": 0,
            "recovery_terminal_quantity": 0,
            "remaining_to_allocate_quantity": 4,
            "observed_item_barcode": null,
            "observed_lot": null,
            "observed_serial": null,
            "hold": {
                "hold_id": 101,
                "inventory_balance_id": 84,
                "held_quantity": 4
            },
            "movement": null,
            "reported_by": 9,
            "reported_at": "2026-08-08T20:00:00Z"
        });
        let CommandOutcome::PickShortageReported(result) = decode_command_response(
            ResponseKind::PickShortageReport,
            201,
            &serde_json::to_vec(&response).unwrap(),
        )
        .unwrap() else {
            panic!("expected a shortage result");
        };
        assert_eq!(result.shortage_id, 91);
        assert_eq!(result.short_quantity, 4);
        assert_eq!(result.reason, PickShortageReason::InventoryMissing);

        let mut invalid = response;
        invalid["remaining_to_allocate_quantity"] = json!(3);
        assert!(matches!(
            decode_command_response(
                ResponseKind::PickShortageReport,
                201,
                &serde_json::to_vec(&invalid).unwrap(),
            ),
            Err(WireResponseError::InvalidPickShortageResponse)
        ));
    }

    #[test]
    fn request_hash_detects_body_drift() {
        let mut request = build_durable_request(&draft(PutawayCommand::ClaimNext {
            workflow: MovementKind::Loose,
        }))
        .expect("claim request should build");
        request.body.push(b' ');

        assert!(!request.verify_body());
    }

    #[test]
    fn unsupported_command_schema_never_reaches_storage() {
        let mut draft = draft(PutawayCommand::ClaimNext {
            workflow: MovementKind::Loose,
        });
        draft.schema_version = 2;

        assert!(matches!(
            build_durable_request(&draft),
            Err(WireRequestError::UnsupportedSchema(2))
        ));
    }

    #[test]
    fn optional_claim_response_maps_scope_and_scannable_locations() {
        let response = serde_json::to_vec(&json!({
            "task_id": 42,
            "inventory_owner_id": 7,
            "facility_id": 9,
            "priority": 80,
            "instructions": "Keep upright",
            "due_at": null,
            "lease_expires_at": "2026-07-27T02:00:00Z",
            "putaway_policy": {
                "source": "product_default",
                "configuration_id": null,
                "configuration_revision": null,
                "configuration_scope": null,
                "require_zone_compatibility": false,
                "enforce_location_capacity": false,
                "allow_mixed_lots": false,
                "policy_hash": "9ebb7234209756a6ff122d74733521612cd2dd38dbb8ed8490e732c9b1625971"
            },
            "source_location": {
                "location_id": 11,
                "barcode": null,
                "name": "Receiving"
            },
            "destination_location": {
                "location_id": 12,
                "barcode": "A-01-02",
                "name": "A-01-02"
            },
            "work": {
                "workflow": "loose",
                "source_inventory_balance_id": 13,
                "item_batch_id": 14,
                "item_id": 15,
                "item_description": "Widget",
                "uom": "case",
                "lot": "LOT-1",
                "serial": null,
                "expiration": null,
                "inventory_status": "available",
                "quantity": 4
            }
        }))
        .unwrap();

        let CommandOutcome::PutawayClaimed(Some(claim)) =
            decode_command_response(ResponseKind::OptionalClaim, 200, &response).unwrap()
        else {
            panic!("expected a claim");
        };
        assert_eq!(claim.details().inventory_owner_id, 7);
        assert_eq!(claim.details().facility_id, 9);
        assert_eq!(claim.details().source.as_ref().unwrap().barcode, None);
        assert_eq!(
            claim.details().destination.barcode.as_deref(),
            Some("A-01-02")
        );
    }

    #[test]
    fn optional_claim_accepts_an_exact_json_null() {
        assert_eq!(
            decode_command_response(ResponseKind::OptionalClaim, 200, b"null").unwrap(),
            CommandOutcome::PutawayClaimed(None)
        );
    }

    #[test]
    fn malformed_success_never_becomes_a_workflow_outcome() {
        assert!(matches!(
            decode_command_response(ResponseKind::LooseConfirmation, 200, b"{}"),
            Err(WireResponseError::Decode(_))
        ));
        assert!(matches!(
            decode_command_response(ResponseKind::Release, 503, b"{}"),
            Err(WireResponseError::UnsuccessfulStatus(503))
        ));
    }

    #[test]
    fn heartbeat_response_validates_task_and_rfc3339_timestamps() {
        let body = serde_json::to_vec(&json!({
            "task_id": 42,
            "heartbeat_at": "2026-07-27T00:05:00.123456+00:00",
            "lease_expires_at": "2026-07-27T00:07:00Z"
        }))
        .unwrap();

        let response = decode_heartbeat_response(42, 200, &body).unwrap();

        assert_eq!(response.task_id, 42);
        assert_eq!(response.heartbeat_at, "2026-07-27T00:05:00.123456+00:00");
    }

    #[test]
    fn heartbeat_response_rejects_invalid_ids_and_timestamps() {
        let invalid_task = br#"{
            "task_id": 0,
            "heartbeat_at": "2026-07-27T00:05:00Z",
            "lease_expires_at": "2026-07-27T00:07:00Z"
        }"#;
        assert!(matches!(
            decode_heartbeat_response(42, 200, invalid_task),
            Err(WireResponseError::InvalidHeartbeatTaskId)
        ));

        let mismatched = br#"{
            "task_id": 43,
            "heartbeat_at": "2026-07-27T00:05:00Z",
            "lease_expires_at": "2026-07-27T00:07:00Z"
        }"#;
        assert!(matches!(
            decode_heartbeat_response(42, 200, mismatched),
            Err(WireResponseError::HeartbeatTaskMismatch {
                expected: 42,
                actual: 43
            })
        ));

        let invalid_heartbeat = br#"{
            "task_id": 42,
            "heartbeat_at": "2026-02-29T00:05:00Z",
            "lease_expires_at": "2026-07-27T00:07:00Z"
        }"#;
        assert!(matches!(
            decode_heartbeat_response(42, 200, invalid_heartbeat),
            Err(WireResponseError::InvalidHeartbeatTimestamp {
                field: "heartbeat_at"
            })
        ));

        let invalid_lease = br#"{
            "task_id": 42,
            "heartbeat_at": "2026-07-27T00:05:00Z",
            "lease_expires_at": "2026-07-27T24:00:00Z"
        }"#;
        assert!(matches!(
            decode_heartbeat_response(42, 200, invalid_lease),
            Err(WireResponseError::InvalidHeartbeatTimestamp {
                field: "lease_expires_at"
            })
        ));
        assert!(matches!(
            decode_heartbeat_response(0, 200, invalid_lease),
            Err(WireResponseError::InvalidHeartbeatTaskId)
        ));
        assert!(matches!(
            decode_heartbeat_response(42, 503, b"{}"),
            Err(WireResponseError::UnsuccessfulStatus(503))
        ));
    }

    fn expected_receiving_session_body() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "load_id": 11,
            "inventory_owner_id": 22,
            "facility_id": 33,
            "reference_number": "ASN-1001",
            "status": "receiving",
            "expected_seal": "SEAL-1001",
            "receiving_location": {
                "location_id": 44,
                "barcode": "DOCK-04",
                "name": "Inbound Dock 4"
            },
            "receipt_policy": {
                "source": "product_default",
                "configuration_id": null,
                "configuration_revision": null,
                "configuration_scope": null,
                "allow_unexpected": true,
                "quarantine_unmapped_items": true,
                "over_receipt_tolerance_basis_points": 10000,
                "policy_hash": "d52ecae3b5747640fb1bcdf91c7fb3a8800fa4ccce0c220267b44da3a8808326"
            },
            "lines": [{
                "load_line_id": 55,
                "item_id": 66,
                "item_description": "Case-picked item",
                "uom": "case",
                "item_barcodes": ["0012345678905", "CASE-66"],
                "expected_quantity": 12,
                "received_quantity": 4,
                "rejected_quantity": 1,
                "missing_quantity": 0,
                "remaining_quantity": 7,
                "lot": "LOT-07",
                "serial": null,
                "expiration": "2027-07-26T00:00:00+00:00"
            }]
        }))
        .unwrap()
    }

    #[test]
    fn expected_receiving_paths_and_confirmation_match_the_public_contract() {
        assert_eq!(
            build_expected_receiving_session_path(11).unwrap(),
            "/api/v1/expected-receiving/loads/11"
        );
        assert!(matches!(
            build_expected_receiving_session_path(0),
            Err(WireRequestError::InvalidLoadId)
        ));
        validate_expected_receiving_load_barcode("LOAD:ASN_1001.2").unwrap();
        assert_eq!(
            normalize_expected_receiving_load_barcode(" load:asn_1001.2 ").unwrap(),
            "LOAD:ASN_1001.2"
        );

        let confirmation = ConfirmExpectedReceiptRequest::Received {
            item_barcode: "0012345678905".into(),
            receiving_location_barcode: "DOCK-04".into(),
            quantity: 4,
            license_plate_barcode: Some("LP-1004".into()),
            lot: Some("LOT-07".into()),
            serial: None,
            expiration: Some("2027-07-26T00:00:00+00:00".into()),
        };
        let (path, body) = build_expected_receipt_confirmation_parts(55, &confirmation).unwrap();

        assert_eq!(path, "/api/v1/expected-receiving/lines/55/confirmations");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            json!({
                "disposition": "received",
                "item_barcode": "0012345678905",
                "receiving_location_barcode": "DOCK-04",
                "quantity": 4,
                "license_plate_barcode": "LP-1004",
                "lot": "LOT-07",
                "serial": null,
                "expiration": "2027-07-26T00:00:00+00:00"
            })
        );
    }

    #[test]
    fn unloading_start_request_and_response_match_the_public_contract() {
        let mut response = serde_json::from_slice::<ExpectedReceivingSessionResponse>(
            &expected_receiving_session_body(),
        )
        .unwrap();
        response.status = ExpectedReceivingLoadStatus::Arrived;
        let session = map_receiving_session(response).unwrap();
        let intent = UnloadingStartIntent::try_new(
            crate::expected_receiving::LoadBarcode::new("WB-LOAD-11").unwrap(),
            session,
            UnloadingStartCommand {
                load_scan: crate::expected_receiving::LoadBarcode::new("WB-LOAD-11").unwrap(),
                receiving_location_scan: DockBarcode::new("DOCK-04").unwrap(),
                seal_scan: Some(SealBarcode::new("SEAL-1001").unwrap()),
            },
        )
        .unwrap();
        let request = build_durable_request(&DurableCommandDraft {
            schema_version: 1,
            command_id: "unloading-11".into(),
            idempotency_key: "unloading:11:1".into(),
            command: RfCommand::ExpectedReceipt(Box::new(ReceivingCommandIntent::Unloading(
                Box::new(intent),
            ))),
        })
        .unwrap();
        assert_eq!(request.path, "/api/v1/inbound-loads/11/unloading-starts");
        assert_eq!(request.response_kind, ResponseKind::InboundUnloadingStart);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
            json!({
                "load_scan": "WB-LOAD-11",
                "receiving_location_scan": "DOCK-04",
                "seal_scan": "SEAL-1001",
                "started_at": null
            })
        );

        let body = serde_json::to_vec(&json!({
            "unloading_start_id": 71,
            "load_id": 11,
            "status": "receiving",
            "receiving_location_id": 44,
            "started_by": 12,
            "started_at": "2026-08-11T04:00:00Z"
        }))
        .unwrap();
        let outcome =
            decode_command_response(ResponseKind::InboundUnloadingStart, 200, &body).unwrap();
        assert!(matches!(
            outcome,
            CommandOutcome::InboundUnloadingStarted(UnloadingStartResult {
                unloading_start_id: 71,
                ..
            })
        ));
        assert!(matches!(
            decode_command_response(ResponseKind::InboundUnloadingStart, 200, b"{}"),
            Err(WireResponseError::Decode(_))
        ));
    }

    #[test]
    fn expected_receipt_confirmation_is_only_built_from_a_durable_intent() {
        let line = DomainExpectedReceiptLine::try_new(ExpectedReceiptLineInput {
            load_line_id: LoadLineId::try_from(55).unwrap(),
            item_id: ItemId::try_from(66).unwrap(),
            item_description: Some("Case-picked item".into()),
            uom: StockDimension::new("case").unwrap(),
            item_barcodes: vec![ItemBarcode::new("CASE-66").unwrap()],
            expected: PositiveQuantity::try_from(10).unwrap(),
            received: NonNegativeQuantity::new(2).unwrap(),
            rejected: NonNegativeQuantity::new(0).unwrap(),
            missing: NonNegativeQuantity::new(0).unwrap(),
            remaining: NonNegativeQuantity::new(8).unwrap(),
            lot: None,
            serial: None,
            expiration: None,
        })
        .unwrap();
        let recovery = crate::expected_receiving::ConfirmationRecoverySnapshot::try_new(
            crate::expected_receiving::ConfirmationRecoverySnapshotInput {
                load_barcode: crate::expected_receiving::LoadBarcode::new("LOAD-11").unwrap(),
                load_id: LoadId::try_from(11).unwrap(),
                inventory_owner_id: InventoryOwnerId::try_from(22).unwrap(),
                facility_id: FacilityId::try_from(33).unwrap(),
                reference_number: Some("ASN-11".into()),
                status: ReceivingLoadStatus::Receiving,
                expected_seal: None,
                dock: ReceivingDock::new(
                    LocationId::try_from(44).unwrap(),
                    DockBarcode::new("DOCK-04").unwrap(),
                    Some("Inbound dock 4".into()),
                ),
                receipt_policy: ReceiptPolicy::product_default(),
                selected_line: line,
            },
        )
        .unwrap();
        let intent = crate::expected_receiving::ConfirmationIntent::try_new(
            recovery,
            ExpectedReceiptCommand::Rejected {
                item_barcode: crate::expected_receiving::ItemBarcode::new("CASE-66").unwrap(),
                quantity: PositiveQuantity::try_from(2).unwrap(),
                reason: ReceiptExceptionReason::QualityRejected,
                note: Some(
                    crate::expected_receiving::ExceptionNote::new("Seal was broken").unwrap(),
                ),
            },
        )
        .unwrap();
        let request = build_durable_request(&DurableCommandDraft {
            schema_version: 1,
            command_id: "receipt-confirmation-1".into(),
            idempotency_key: "expected-receiving:55:1".into(),
            command: RfCommand::ExpectedReceipt(Box::new(intent.into())),
        })
        .unwrap();

        assert_eq!(
            request.path,
            "/api/v1/expected-receiving/lines/55/confirmations"
        );
        assert_eq!(
            request.response_kind,
            ResponseKind::ExpectedReceiptConfirmation
        );
        assert!(request.verify_body());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
            json!({
                "disposition": "rejected",
                "item_barcode": "CASE-66",
                "quantity": 2,
                "reason": "quality_rejected",
                "note": "Seal was broken"
            })
        );
    }

    #[test]
    fn expected_receiving_request_validation_matches_server_limits() {
        for barcode in [
            "", "-LOAD-1", "LOAD/1", "LOAD 1", "LOAD%1", "LOAD?1", "LOAD#1", "東京", "LOAD-\n1",
        ] {
            assert!(matches!(
                validate_expected_receiving_load_barcode(barcode),
                Err(WireRequestError::InvalidExpectedReceivingBarcode)
            ));
        }
        assert!(matches!(
            validate_expected_receiving_load_barcode(&"X".repeat(201)),
            Err(WireRequestError::InvalidExpectedReceivingBarcode)
        ));

        let invalid_quantity = ConfirmExpectedReceiptRequest::Missing {
            quantity: 0,
            reason: wareboxes_api_contract::v1::ExpectedReceiptExceptionReason::ShortShipment,
            note: None,
        };
        assert!(matches!(
            build_expected_receipt_confirmation_parts(55, &invalid_quantity),
            Err(WireRequestError::InvalidExpectedReceiptField { field: "quantity" })
        ));

        let missing_note = ConfirmExpectedReceiptRequest::Rejected {
            item_barcode: "CASE-66".into(),
            quantity: 1,
            reason: wareboxes_api_contract::v1::ExpectedReceiptExceptionReason::Other,
            note: None,
        };
        assert!(matches!(
            build_expected_receipt_confirmation_parts(55, &missing_note),
            Err(WireRequestError::ExpectedReceiptNoteRequired)
        ));
        assert!(matches!(
            build_expected_receipt_confirmation_parts(0, &missing_note),
            Err(WireRequestError::InvalidLoadLineId)
        ));
    }

    #[test]
    fn expected_receiving_session_decoder_validates_scope_and_projection_shape() {
        let body = expected_receiving_session_body();
        let response = decode_expected_receiving_session_response(Some(11), 200, &body).unwrap();
        assert_eq!(response.load_id, 11);
        assert_eq!(response.lines[0].remaining_quantity, 7);
        assert_eq!(
            decode_expected_receiving_session_response(None, 200, &body).unwrap(),
            response
        );
        assert!(matches!(
            decode_expected_receiving_session_response(Some(12), 200, &body),
            Err(WireResponseError::ExpectedReceivingLoadMismatch {
                expected: 12,
                actual: 11
            })
        ));
        let session = decode_receiving_session(Some(11), 200, &body).unwrap();
        assert_eq!(session.load_id().get(), 11);
        assert_eq!(session.lines()[0].load_line_id().get(), 55);
        assert_eq!(session.lines()[0].remaining().get(), 7);

        let mut invalid = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        invalid["lines"][0]["remaining_quantity"] = json!(8);
        assert!(matches!(
            decode_expected_receiving_session_response(
                Some(11),
                200,
                &serde_json::to_vec(&invalid).unwrap()
            ),
            Err(WireResponseError::InvalidExpectedReceivingSession)
        ));
        assert!(matches!(
            decode_expected_receiving_session_response(Some(11), 503, b"{}"),
            Err(WireResponseError::UnsuccessfulStatus(503))
        ));
    }

    #[test]
    fn expected_receipt_confirmation_decoder_validates_line_and_inventory_shape() {
        let body = serde_json::to_vec(&json!({
            "load_id": 11,
            "load_line_id": 55,
            "disposition": "received",
            "quantity": 4,
            "inventory_transaction_id": 77,
            "inventory_balance_id": 88,
            "item_batch_id": 99,
            "license_plate_id": 111,
            "inventory_hold_id": null,
            "inventory_status": "available",
            "line_status": "partial",
            "load_status": "receiving",
            "cumulative_received_quantity": 4,
            "cumulative_rejected_quantity": 1,
            "cumulative_missing_quantity": 0,
            "remaining_quantity": 7,
            "receive_completed": false
        }))
        .unwrap();

        let response = decode_expected_receipt_confirmation_response(55, 200, &body).unwrap();
        assert_eq!(response.load_id, 11);
        assert_eq!(response.disposition, ExpectedReceiptDisposition::Received);
        assert!(matches!(
            decode_expected_receipt_confirmation_response(56, 200, &body),
            Err(WireResponseError::ExpectedReceiptLineMismatch {
                expected: 56,
                actual: 55
            })
        ));

        let mut quarantined = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        quarantined["disposition"] = json!("quarantined");
        quarantined["inventory_status"] = json!("quarantine");
        quarantined["inventory_hold_id"] = json!(112);
        quarantined["cumulative_received_quantity"] = json!(0);
        quarantined["cumulative_rejected_quantity"] = json!(5);
        let quarantined = serde_json::to_vec(&quarantined).unwrap();
        assert_eq!(
            decode_expected_receipt_confirmation_response(55, 200, &quarantined)
                .unwrap()
                .disposition,
            ExpectedReceiptDisposition::Quarantined
        );

        let mut invalid_quarantine =
            serde_json::from_slice::<serde_json::Value>(&quarantined).unwrap();
        invalid_quarantine["inventory_hold_id"] = json!(null);
        assert!(matches!(
            decode_expected_receipt_confirmation_response(
                55,
                200,
                &serde_json::to_vec(&invalid_quarantine).unwrap()
            ),
            Err(WireResponseError::InvalidExpectedReceiptConfirmation)
        ));

        let mut invalid = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        invalid["inventory_transaction_id"] = json!(null);
        assert!(matches!(
            decode_expected_receipt_confirmation_response(
                55,
                200,
                &serde_json::to_vec(&invalid).unwrap()
            ),
            Err(WireResponseError::InvalidExpectedReceiptConfirmation)
        ));

        let mut invalid = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        invalid["load_status"] = json!("arrived");
        assert!(matches!(
            decode_expected_receipt_confirmation_response(
                55,
                200,
                &serde_json::to_vec(&invalid).unwrap()
            ),
            Err(WireResponseError::InvalidExpectedReceiptConfirmation)
        ));
    }

    #[test]
    fn unexpected_receipt_durable_wire_contract_is_exact_and_strict() {
        let intent = crate::expected_receiving::UnexpectedReceiptIntent {
            schema_version: 2,
            load_id: LoadId::try_from(11).unwrap(),
            command: UnexpectedReceiptCommand {
                item_barcode: ItemBarcode::new("CASE-NEW").unwrap(),
                receiving_location_barcode: DockBarcode::new("DOCK-04").unwrap(),
                quantity: PositiveQuantity::try_from(3).unwrap(),
                license_plate_barcode: Some(LicensePlateBarcode::new("LP-NEW-1").unwrap()),
                lot: Some(StockDimension::new("LOT-NEW").unwrap()),
                serial: None,
                expiration: Some(Expiration::new("2027-08-26T00:00:00Z").unwrap()),
                reason: UnexpectedReceiptReason::UnexpectedItem,
                note: None,
                receipt_policy: ReceiptPolicy::product_default(),
            },
            recovery: Box::new(
                crate::expected_receiving::UnexpectedReceiptRecoverySnapshot {
                    load_barcode: crate::expected_receiving::LoadBarcode::new("LOAD-11").unwrap(),
                    load_id: LoadId::try_from(11).unwrap(),
                    inventory_owner_id: InventoryOwnerId::try_from(22).unwrap(),
                    facility_id: FacilityId::try_from(33).unwrap(),
                    reference_number: Some("ASN-11".into()),
                    status: ReceivingLoadStatus::Received,
                    expected_seal: None,
                    dock: ReceivingDock::new(
                        LocationId::try_from(44).unwrap(),
                        DockBarcode::new("DOCK-04").unwrap(),
                        Some("Inbound dock 4".into()),
                    ),
                    receipt_policy: ReceiptPolicy::product_default(),
                    lines: Vec::new(),
                },
            ),
        };
        let request = build_durable_request(&DurableCommandDraft {
            schema_version: 1,
            command_id: "unexpected-receipt-1".into(),
            idempotency_key: "unexpected-receipt:11:1".into(),
            command: RfCommand::ExpectedReceipt(Box::new(ReceivingCommandIntent::Unexpected(
                Box::new(intent),
            ))),
        })
        .unwrap();

        assert_eq!(
            request.path,
            "/api/v1/expected-receiving/loads/11/unexpected-receipts"
        );
        assert_eq!(
            request.response_kind,
            ResponseKind::UnexpectedReceiptConfirmation
        );
        assert!(request.verify_body());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
            json!({
                "item_barcode": "CASE-NEW",
                "receiving_location_barcode": "DOCK-04",
                "quantity": 3,
                "license_plate_barcode": "LP-NEW-1",
                "lot": "LOT-NEW",
                "serial": null,
                "expiration": "2027-08-26T00:00:00Z",
                "reason": "unexpected_item",
                "note": null,
                "expected_policy": {
                    "source": "product_default",
                    "configuration_id": null,
                    "configuration_revision": null,
                    "policy_hash": "d52ecae3b5747640fb1bcdf91c7fb3a8800fa4ccce0c220267b44da3a8808326"
                }
            })
        );

        let body = serde_json::to_vec(&json!({
            "unexpected_receipt_id": 71,
            "load_id": 11,
            "inventory_owner_id": 22,
            "facility_id": 33,
            "item_id": 66,
            "uom": "case",
            "quantity": 3,
            "receiving_location_id": 44,
            "observed_item_barcode": "CASE-NEW",
            "observed_receiving_location_barcode": "DOCK-04",
            "inventory_transaction_id": 72,
            "inventory_balance_id": 73,
            "item_batch_id": 74,
            "license_plate_id": 75,
            "license_plate_barcode": "LP-NEW-1",
            "lot": "LOT-NEW",
            "serial": null,
            "expiration": "2027-08-26T00:00:00Z",
            "inventory_hold_id": 76,
            "inventory_status": "quarantine",
            "reason": "unexpected_item",
            "note": null,
            "load_status": "received",
            "confirmed_by_user_id": 77,
            "confirmed_at": "2026-08-09T18:00:00Z",
            "receipt_policy": {
                "source": "product_default",
                "configuration_id": null,
                "configuration_revision": null,
                "configuration_scope": null,
                "allow_unexpected": true,
                "quarantine_unmapped_items": true,
                "over_receipt_tolerance_basis_points": 10000,
                "policy_hash": "d52ecae3b5747640fb1bcdf91c7fb3a8800fa4ccce0c220267b44da3a8808326"
            }
        }))
        .unwrap();
        let CommandOutcome::UnexpectedReceipt(result) =
            decode_command_response(ResponseKind::UnexpectedReceiptConfirmation, 200, &body)
                .unwrap()
        else {
            panic!("unexpected receipt response must remain distinct");
        };
        assert_eq!(result.load_id.get(), 11);
        assert_eq!(result.quantity.get(), 3);
        assert_eq!(result.reason, UnexpectedReceiptReason::UnexpectedItem);

        let mut invalid = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        invalid["inventory_status"] = json!("available");
        assert!(matches!(
            decode_command_response(
                ResponseKind::UnexpectedReceiptConfirmation,
                200,
                &serde_json::to_vec(&invalid).unwrap(),
            ),
            Err(WireResponseError::InvalidUnexpectedReceiptConfirmation)
        ));
    }
}
