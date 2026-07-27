use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wareboxes_api_contract::v1::{
    API_PREFIX, ClaimNextPutawayRequest, ClaimPutawayByIdRequest,
    ConfirmLicensePlatePutawayRequest, ConfirmPutawayRequest, IdempotencyKey,
    PutawayClaimReleaseReason, PutawayWorkflow, ReleasePutawayClaimRequest,
};

use crate::workflow::{DurableCommandDraft, PutawayCommand, PutawayKind, ReleaseReason};

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
}
