use chrono::DateTime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wareboxes_api_contract::v1::{
    API_PREFIX, ClaimNextPutawayRequest, ClaimPutawayByIdRequest, ConfirmExpectedReceiptRequest,
    ConfirmLicensePlatePutawayRequest, ConfirmPutawayRequest, ExpectedReceiptConfirmationResponse,
    ExpectedReceiptDisposition, ExpectedReceiptExceptionReason, ExpectedReceivingLoadStatus,
    ExpectedReceivingSessionResponse, HeartbeatPutawayClaimRequest, IdempotencyKey,
    LicensePlatePutawayConfirmationResponse, PutawayClaimHeartbeatResponse,
    PutawayClaimReleaseReason, PutawayClaimResponse, PutawayClaimSourceLocation, PutawayClaimWork,
    PutawayConfirmationResponse, PutawayWorkflow, ReleasePutawayClaimRequest,
};

use crate::expected_receiving::{
    ConfirmationMode, ConfirmationResult, DockBarcode, ExpectedReceiptCommand,
    ExpectedReceiptLine as DomainExpectedReceiptLine, ExpectedReceiptLineInput, Expiration,
    FacilityId, InventoryOwnerId, ItemBarcode, ItemId, LoadId, LoadLineId, LocationId,
    NonNegativeQuantity, PositiveQuantity, ReceiptExceptionReason, ReceivingDock,
    ReceivingLoadStatus, ReceivingSession, ReceivingSessionInput, StockDimension,
};
use crate::workflow::{
    CommandOutcome, DurableCommandDraft, Location, PutawayClaim, PutawayCommand, PutawayKind,
    PutawayWork, ReleaseReason, RfCommand,
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
    ExpectedReceiptConfirmation,
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
    #[error(
        "the expected receiving response load ID {actual} does not match requested load {expected}"
    )]
    ExpectedReceivingLoadMismatch { expected: i64, actual: i64 },
    #[error("the warehouse service returned an invalid expected receipt confirmation")]
    InvalidExpectedReceiptConfirmation,
    #[error(
        "the expected receipt response line ID {actual} does not match requested line {expected}"
    )]
    ExpectedReceiptLineMismatch { expected: i64, actual: i64 },
}

pub fn build_heartbeat_request_parts(task_id: i64) -> Result<(String, Vec<u8>), WireRequestError> {
    validate_task_id(task_id)?;
    Ok((
        format!("{API_PREFIX}/putaway-claims/{task_id}/heartbeats"),
        serde_json::to_vec(&HeartbeatPutawayClaimRequest::default())?,
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
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/putaway-tasks/{task_id}/confirmations"),
                serde_json::to_vec(&ConfirmPutawayRequest {
                    destination_location_barcode: destination_location_barcode.clone(),
                })?,
                ResponseKind::LooseConfirmation,
            )
        }
        RfCommand::Putaway(PutawayCommand::ConfirmLicensePlate {
            task_id,
            license_plate_barcode,
            destination_location_barcode,
        }) => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/license-plate-putaway-tasks/{task_id}/confirmations"),
                serde_json::to_vec(&ConfirmLicensePlatePutawayRequest {
                    license_plate_barcode: license_plate_barcode.clone(),
                    destination_location_barcode: destination_location_barcode.clone(),
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
        RfCommand::ExpectedReceipt(intent) => {
            if !intent.is_current_and_valid() {
                return Err(WireRequestError::InvalidExpectedReceiptField {
                    field: "confirmation intent",
                });
            }
            let confirmation = map_expected_receipt_command(&intent.command);
            let (path, body) = build_expected_receipt_confirmation_parts(
                intent.load_line_id.get(),
                &confirmation,
            )?;
            (path, body, ResponseKind::ExpectedReceiptConfirmation)
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
            Ok(CommandOutcome::Claimed(
                claim.map(map_claim).transpose()?.map(Box::new),
            ))
        }
        ResponseKind::Claim => Ok(CommandOutcome::Claimed(Some(Box::new(map_claim(
            serde_json::from_slice::<PutawayClaimResponse>(body)?,
        )?)))),
        ResponseKind::LooseConfirmation => {
            let response = serde_json::from_slice::<PutawayConfirmationResponse>(body)?;
            Ok(CommandOutcome::Confirmed {
                task_id: response.task_id,
            })
        }
        ResponseKind::LicensePlateConfirmation => {
            let response = serde_json::from_slice::<LicensePlatePutawayConfirmationResponse>(body)?;
            Ok(CommandOutcome::Confirmed {
                task_id: response.task_id,
            })
        }
        ResponseKind::Release => {
            let response = serde_json::from_slice::<
                wareboxes_api_contract::v1::PutawayClaimReleaseResponse,
            >(body)?;
            Ok(CommandOutcome::Released {
                task_id: response.task_id,
            })
        }
        ResponseKind::ExpectedReceiptConfirmation => {
            let response = decode_expected_receipt_confirmation_response_from_body(status, body)?;
            Ok(CommandOutcome::ExpectedReceipt(response))
        }
    }
}

pub fn decode_claim_response(body: &[u8]) -> Result<Option<PutawayClaim>, WireResponseError> {
    serde_json::from_slice::<Option<PutawayClaimResponse>>(body)?
        .map(map_claim)
        .transpose()
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
    let work = match response.work {
        PutawayClaimWork::Loose {
            item_id,
            item_description,
            uom,
            lot,
            serial,
            quantity,
            ..
        } => PutawayWork::Loose {
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
        } => PutawayWork::LicensePlate {
            barcode: license_plate_barcode,
            planned_balance_count,
        },
    };
    Ok(PutawayClaim {
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
    })
}

fn map_receiving_session(
    response: ExpectedReceivingSessionResponse,
) -> Result<ReceivingSession, WireResponseError> {
    let status = match response.status {
        ExpectedReceivingLoadStatus::Arrived => ReceivingLoadStatus::Arrived,
        ExpectedReceivingLoadStatus::Receiving => ReceivingLoadStatus::Receiving,
        ExpectedReceivingLoadStatus::Received => {
            return Err(WireResponseError::InvalidExpectedReceivingSession);
        }
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
    ReceivingSession::try_new(ReceivingSessionInput {
        load_id: LoadId::try_from(response.load_id)
            .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        inventory_owner_id: InventoryOwnerId::try_from(response.inventory_owner_id)
            .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        facility_id: FacilityId::try_from(response.facility_id)
            .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
        reference_number: response.reference_number,
        status,
        dock: ReceivingDock::new(
            LocationId::try_from(response.receiving_location.location_id)
                .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
            DockBarcode::new(response.receiving_location.barcode)
                .map_err(|_| WireResponseError::InvalidExpectedReceivingSession)?,
            response.receiving_location.name,
        ),
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
        || !matches!(
            response.status,
            ExpectedReceivingLoadStatus::Arrived | ExpectedReceivingLoadStatus::Receiving
        )
        || response.lines.is_empty()
        || !valid_response_text(
            &response.receiving_location.barcode,
            MAX_EXPECTED_RECEIVING_BARCODE_LENGTH,
        )
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
    ];
    let inventory_shape_is_valid = match response.disposition {
        ExpectedReceiptDisposition::Received => {
            response.inventory_transaction_id.is_some()
                && response.inventory_balance_id.is_some()
                && response.item_batch_id.is_some()
        }
        ExpectedReceiptDisposition::Rejected | ExpectedReceiptDisposition::Missing => {
            optional_ids.iter().all(Option::is_none)
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

const fn map_workflow(workflow: PutawayKind) -> PutawayWorkflow {
    match workflow {
        PutawayKind::Loose => PutawayWorkflow::Loose,
        PutawayKind::LicensePlate => PutawayWorkflow::LicensePlate,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn draft(command: PutawayCommand) -> DurableCommandDraft {
        DurableCommandDraft {
            schema_version: 1,
            command_id: "command-1".into(),
            idempotency_key: "putaway:key-1".into(),
            command: command.into(),
        }
    }

    fn body(request: &DurableHttpRequest) -> serde_json::Value {
        serde_json::from_slice(&request.body).expect("request body should be valid JSON")
    }

    #[test]
    fn claim_next_uses_the_public_v1_contract() {
        let request = build_durable_request(&draft(PutawayCommand::ClaimNext {
            workflow: PutawayKind::LicensePlate,
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
        }))
        .expect("loose confirmation should build");

        assert_eq!(request.path, "/api/v1/putaway-tasks/42/confirmations");
        assert_eq!(
            body(&request),
            json!({"destination_location_barcode": "A-01-02"})
        );
        assert_eq!(request.response_kind, ResponseKind::LooseConfirmation);
    }

    #[test]
    fn license_plate_confirmation_persists_both_scans() {
        let request = build_durable_request(&draft(PutawayCommand::ConfirmLicensePlate {
            task_id: 42,
            license_plate_barcode: "LP-42".into(),
            destination_location_barcode: "A-01-02".into(),
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
                "destination_location_barcode": "A-01-02"
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
    fn request_hash_detects_body_drift() {
        let mut request = build_durable_request(&draft(PutawayCommand::ClaimNext {
            workflow: PutawayKind::Loose,
        }))
        .expect("claim request should build");
        request.body.push(b' ');

        assert!(!request.verify_body());
    }

    #[test]
    fn unsupported_command_schema_never_reaches_storage() {
        let mut draft = draft(PutawayCommand::ClaimNext {
            workflow: PutawayKind::Loose,
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

        let CommandOutcome::Claimed(Some(claim)) =
            decode_command_response(ResponseKind::OptionalClaim, 200, &response).unwrap()
        else {
            panic!("expected a claim");
        };
        assert_eq!(claim.inventory_owner_id, 7);
        assert_eq!(claim.facility_id, 9);
        assert_eq!(claim.source.as_ref().unwrap().barcode, None);
        assert_eq!(claim.destination.barcode.as_deref(), Some("A-01-02"));
    }

    #[test]
    fn optional_claim_accepts_an_exact_json_null() {
        assert_eq!(
            decode_command_response(ResponseKind::OptionalClaim, 200, b"null").unwrap(),
            CommandOutcome::Claimed(None)
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
            "receiving_location": {
                "location_id": 44,
                "barcode": "DOCK-04",
                "name": "Inbound Dock 4"
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
                dock: ReceivingDock::new(
                    LocationId::try_from(44).unwrap(),
                    DockBarcode::new("DOCK-04").unwrap(),
                    Some("Inbound dock 4".into()),
                ),
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
            command: RfCommand::ExpectedReceipt(Box::new(intent)),
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
}
