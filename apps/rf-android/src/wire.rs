use chrono::DateTime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wareboxes_api_contract::v1::{
    API_PREFIX, ClaimNextPutawayRequest, ClaimPutawayByIdRequest,
    ConfirmLicensePlatePutawayRequest, ConfirmPutawayRequest, HeartbeatPutawayClaimRequest,
    IdempotencyKey, LicensePlatePutawayConfirmationResponse, PutawayClaimHeartbeatResponse,
    PutawayClaimReleaseReason, PutawayClaimResponse, PutawayClaimSourceLocation, PutawayClaimWork,
    PutawayConfirmationResponse, PutawayWorkflow, ReleasePutawayClaimRequest,
};

use crate::workflow::{
    CommandOutcome, DurableCommandDraft, Location, PutawayClaim, PutawayCommand, PutawayKind,
    PutawayWork, ReleaseReason,
};

pub const JSON_CONTENT_TYPE: &str = "application/json";

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
}

pub fn build_heartbeat_request_parts(task_id: i64) -> Result<(String, Vec<u8>), WireRequestError> {
    validate_task_id(task_id)?;
    Ok((
        format!("{API_PREFIX}/putaway-claims/{task_id}/heartbeats"),
        serde_json::to_vec(&HeartbeatPutawayClaimRequest::default())?,
    ))
}

pub fn build_durable_request(
    draft: &DurableCommandDraft,
) -> Result<DurableHttpRequest, WireRequestError> {
    validate_draft(draft)?;
    let (path, body, response_kind) = match &draft.command {
        PutawayCommand::ClaimNext { workflow } => (
            format!("{API_PREFIX}/putaway-claims/next"),
            serde_json::to_vec(&ClaimNextPutawayRequest {
                workflow: map_workflow(*workflow),
            })?,
            ResponseKind::OptionalClaim,
        ),
        PutawayCommand::ClaimById { task_id } => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/putaway-claims/{task_id}"),
                serde_json::to_vec(&ClaimPutawayByIdRequest::default())?,
                ResponseKind::Claim,
            )
        }
        PutawayCommand::ConfirmLoose {
            task_id,
            destination_location_barcode,
        } => {
            validate_task_id(*task_id)?;
            (
                format!("{API_PREFIX}/putaway-tasks/{task_id}/confirmations"),
                serde_json::to_vec(&ConfirmPutawayRequest {
                    destination_location_barcode: destination_location_barcode.clone(),
                })?,
                ResponseKind::LooseConfirmation,
            )
        }
        PutawayCommand::ConfirmLicensePlate {
            task_id,
            license_plate_barcode,
            destination_location_barcode,
        } => {
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
        PutawayCommand::Release {
            task_id,
            reason,
            note,
        } => {
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
            command,
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
}
