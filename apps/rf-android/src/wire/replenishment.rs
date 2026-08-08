use chrono::DateTime;
use wareboxes_api_contract::v1::{
    ClaimNextReplenishmentWorkRequest, ClaimReplenishmentWorkByIdRequest,
    ConfirmReplenishmentWorkRequest, HeartbeatReplenishmentClaimRequest,
    ReleaseReplenishmentClaimRequest, ReplenishmentClaimHeartbeatResponse,
    ReplenishmentClaimReleaseReason as ApiReleaseReason, ReplenishmentClaimReleaseResponse,
    ReplenishmentClaimResponse, ReplenishmentConfirmationResponse,
    ReplenishmentWorkStatus as ApiWorkStatus,
};

use crate::replenishment::{
    ReplenishmentClaim, ReplenishmentCommand, ReplenishmentConfirmationExpectation,
    ReplenishmentConfirmationResult, ReplenishmentLocation, ReplenishmentReleaseReason,
};
use crate::workflow::CommandOutcome;

use super::{API_PREFIX, ResponseKind, WireRequestError, WireResponseError};

pub(super) fn build_command_parts(
    command: &ReplenishmentCommand,
) -> Result<(String, Vec<u8>, ResponseKind), WireRequestError> {
    match command {
        ReplenishmentCommand::ClaimNext => Ok((
            format!("{API_PREFIX}/replenishment-claims/next"),
            serde_json::to_vec(&ClaimNextReplenishmentWorkRequest::default())?,
            ResponseKind::ReplenishmentOptionalClaim,
        )),
        ReplenishmentCommand::ClaimById { work_id } => {
            super::validate_task_id(*work_id)?;
            Ok((
                format!("{API_PREFIX}/replenishment-claims/{work_id}"),
                serde_json::to_vec(&ClaimReplenishmentWorkByIdRequest::default())?,
                ResponseKind::ReplenishmentClaim,
            ))
        }
        ReplenishmentCommand::Confirm {
            work_id,
            expected,
            source_location_barcode,
            item_barcode,
            lot_scan,
            serial_scan,
            destination_pick_face_barcode,
        } => {
            super::validate_task_id(*work_id)?;
            if !valid_confirmation(
                expected,
                source_location_barcode,
                item_barcode,
                lot_scan.as_deref(),
                serial_scan.as_deref(),
                destination_pick_face_barcode,
            ) {
                return Err(WireRequestError::InvalidReplenishmentCommand);
            }
            Ok((
                format!("{API_PREFIX}/replenishment-tasks/{work_id}/confirmations"),
                serde_json::to_vec(&ConfirmReplenishmentWorkRequest {
                    source_location_barcode: source_location_barcode.clone(),
                    item_barcode: item_barcode.clone(),
                    lot_scan: lot_scan.clone(),
                    serial_scan: serial_scan.clone(),
                    destination_pick_face_barcode: destination_pick_face_barcode.clone(),
                })?,
                ResponseKind::ReplenishmentConfirmation,
            ))
        }
        ReplenishmentCommand::Release {
            work_id,
            reason,
            note,
        } => {
            super::validate_task_id(*work_id)?;
            Ok((
                format!("{API_PREFIX}/replenishment-claims/{work_id}/releases"),
                serde_json::to_vec(&ReleaseReplenishmentClaimRequest {
                    reason: map_release_reason(*reason),
                    note: note.clone(),
                })?,
                ResponseKind::ReplenishmentRelease,
            ))
        }
    }
}

pub fn build_heartbeat_request_parts(work_id: i64) -> Result<(String, Vec<u8>), WireRequestError> {
    super::validate_task_id(work_id)?;
    Ok((
        format!("{API_PREFIX}/replenishment-claims/{work_id}/heartbeats"),
        serde_json::to_vec(&HeartbeatReplenishmentClaimRequest::default())?,
    ))
}

pub fn decode_claim_response(body: &[u8]) -> Result<Option<ReplenishmentClaim>, WireResponseError> {
    serde_json::from_slice::<Option<ReplenishmentClaimResponse>>(body)?
        .map(map_claim)
        .transpose()
}

pub fn decode_heartbeat_response(
    expected_work_id: i64,
    status: u16,
    body: &[u8],
) -> Result<ReplenishmentClaimHeartbeatResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    if expected_work_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    let response = serde_json::from_slice::<ReplenishmentClaimHeartbeatResponse>(body)?;
    if response.work_id != expected_work_id {
        return Err(WireResponseError::HeartbeatTaskMismatch {
            expected: expected_work_id,
            actual: response.work_id,
        });
    }
    validate_timestamp(&response.heartbeat_at, "heartbeat_at")?;
    validate_timestamp(&response.lease_expires_at, "lease_expires_at")?;
    Ok(response)
}

pub(super) fn decode_outcome(
    response_kind: ResponseKind,
    body: &[u8],
) -> Result<CommandOutcome, WireResponseError> {
    match response_kind {
        ResponseKind::ReplenishmentOptionalClaim => Ok(CommandOutcome::ReplenishmentClaimed(
            serde_json::from_slice::<Option<ReplenishmentClaimResponse>>(body)?
                .map(map_claim)
                .transpose()?
                .map(Box::new),
        )),
        ResponseKind::ReplenishmentClaim => {
            Ok(CommandOutcome::ReplenishmentClaimed(Some(Box::new(
                map_claim(serde_json::from_slice::<ReplenishmentClaimResponse>(body)?)?,
            ))))
        }
        ResponseKind::ReplenishmentConfirmation => {
            let response = serde_json::from_slice::<ReplenishmentConfirmationResponse>(body)?;
            Ok(CommandOutcome::ReplenishmentConfirmed(Box::new(
                map_confirmation(response)?,
            )))
        }
        ResponseKind::ReplenishmentRelease => {
            let response = serde_json::from_slice::<ReplenishmentClaimReleaseResponse>(body)?;
            if response.work_id <= 0 || response.status != ApiWorkStatus::Pending {
                return Err(WireResponseError::InvalidReplenishmentResponse);
            }
            validate_timestamp(&response.released_at, "released_at")?;
            Ok(CommandOutcome::ReplenishmentReleased {
                work_id: response.work_id,
            })
        }
        _ => Err(WireResponseError::InvalidReplenishmentResponse),
    }
}

fn map_claim(
    response: ReplenishmentClaimResponse,
) -> Result<ReplenishmentClaim, WireResponseError> {
    if [
        response.work_id,
        response.plan_id,
        response.policy_id,
        response.policy_revision.get(),
        response.inventory_owner_id,
        response.facility_id,
        response.source_inventory_balance_id,
        response.item_batch_id,
        response.item_id,
        response.quantity,
        response.source_location.location_id,
        response.destination_pick_face.location_id,
    ]
    .into_iter()
    .any(|value| value <= 0)
        || response.sequence == 0
        || response.priority < 0
        || response.source_location.location_id == response.destination_pick_face.location_id
        || !valid_scan(&response.source_location.barcode)
        || !valid_scan(&response.destination_pick_face.barcode)
        || response.item_barcodes.is_empty()
        || response
            .item_barcodes
            .iter()
            .any(|barcode| !valid_scan(barcode))
        || !valid_uom(&response.uom)
        || response.lot.as_ref().is_some_and(|lot| !valid_scan(lot))
        || response
            .serial
            .as_ref()
            .is_some_and(|serial| !valid_scan(serial))
        || DateTime::parse_from_rfc3339(&response.lease_expires_at).is_err()
        || response
            .due_at
            .as_deref()
            .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
        || response
            .expiration
            .as_deref()
            .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
    {
        return Err(WireResponseError::InvalidReplenishmentResponse);
    }
    Ok(ReplenishmentClaim {
        work_id: response.work_id,
        plan_id: response.plan_id,
        policy_id: response.policy_id,
        policy_revision: response.policy_revision.get(),
        inventory_owner_id: response.inventory_owner_id,
        facility_id: response.facility_id,
        sequence: response.sequence,
        priority: response.priority,
        instructions: response.instructions,
        due_at: response.due_at,
        lease_expires_at: response.lease_expires_at,
        source_inventory_balance_id: response.source_inventory_balance_id,
        item_batch_id: response.item_batch_id,
        item_id: response.item_id,
        item_description: response.item_description,
        item_barcodes: response.item_barcodes,
        uom: response.uom,
        lot: response.lot,
        serial: response.serial,
        expiration: response.expiration,
        quantity: response.quantity,
        source_location: ReplenishmentLocation {
            location_id: response.source_location.location_id,
            barcode: response.source_location.barcode,
            name: response.source_location.name,
        },
        destination_pick_face: ReplenishmentLocation {
            location_id: response.destination_pick_face.location_id,
            barcode: response.destination_pick_face.barcode,
            name: response.destination_pick_face.name,
        },
    })
}

fn map_confirmation(
    response: ReplenishmentConfirmationResponse,
) -> Result<ReplenishmentConfirmationResult, WireResponseError> {
    if [
        response.confirmation_id,
        response.work_id,
        response.plan_id,
        response.policy_id,
        response.inventory_transaction_id,
        response.source_inventory_balance_id,
        response.destination_inventory_balance_id,
        response.item_batch_id,
        response.item_id,
        response.source_location_id,
        response.destination_pick_face_location_id,
        response.quantity,
        response.confirmed_by,
    ]
    .into_iter()
    .any(|value| value <= 0)
        || response.source_inventory_balance_id == response.destination_inventory_balance_id
        || response.source_location_id == response.destination_pick_face_location_id
        || response.work_status != ApiWorkStatus::Completed
        || !valid_uom(&response.uom)
        || response.lot.as_ref().is_some_and(|lot| !valid_scan(lot))
        || response
            .serial
            .as_ref()
            .is_some_and(|serial| !valid_scan(serial))
        || DateTime::parse_from_rfc3339(&response.confirmed_at).is_err()
    {
        return Err(WireResponseError::InvalidReplenishmentResponse);
    }
    Ok(ReplenishmentConfirmationResult {
        confirmation_id: response.confirmation_id,
        work_id: response.work_id,
        plan_id: response.plan_id,
        policy_id: response.policy_id,
        inventory_transaction_id: response.inventory_transaction_id,
        source_inventory_balance_id: response.source_inventory_balance_id,
        destination_inventory_balance_id: response.destination_inventory_balance_id,
        item_batch_id: response.item_batch_id,
        item_id: response.item_id,
        uom: response.uom,
        lot: response.lot,
        serial: response.serial,
        source_location_id: response.source_location_id,
        destination_pick_face_location_id: response.destination_pick_face_location_id,
        quantity: response.quantity,
        confirmed_by: response.confirmed_by,
        confirmed_at: response.confirmed_at,
    })
}

const fn map_release_reason(reason: ReplenishmentReleaseReason) -> ApiReleaseReason {
    match reason {
        ReplenishmentReleaseReason::WorkInterrupted => ApiReleaseReason::WorkInterrupted,
        ReplenishmentReleaseReason::EquipmentUnavailable => ApiReleaseReason::EquipmentUnavailable,
        ReplenishmentReleaseReason::SourceBlocked => ApiReleaseReason::SourceBlocked,
        ReplenishmentReleaseReason::DestinationBlocked => ApiReleaseReason::DestinationBlocked,
        ReplenishmentReleaseReason::InventoryMismatch => ApiReleaseReason::InventoryMismatch,
        ReplenishmentReleaseReason::SafetyIssue => ApiReleaseReason::SafetyIssue,
        ReplenishmentReleaseReason::Other => ApiReleaseReason::Other,
    }
}

fn validate_timestamp(value: &str, field: &'static str) -> Result<(), WireResponseError> {
    if DateTime::parse_from_rfc3339(value).is_err() {
        return Err(WireResponseError::InvalidHeartbeatTimestamp { field });
    }
    Ok(())
}

fn valid_scan(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= 200
        && !value.chars().any(char::is_control)
}

fn valid_uom(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= 32
        && !value.chars().any(char::is_control)
}

fn valid_confirmation(
    expected: &ReplenishmentConfirmationExpectation,
    source_location_barcode: &str,
    item_barcode: &str,
    lot_scan: Option<&str>,
    serial_scan: Option<&str>,
    destination_pick_face_barcode: &str,
) -> bool {
    [
        expected.plan_id,
        expected.policy_id,
        expected.source_inventory_balance_id,
        expected.item_batch_id,
        expected.item_id,
        expected.source_location_id,
        expected.destination_pick_face_location_id,
        expected.quantity,
    ]
    .into_iter()
    .all(|value| value > 0)
        && expected.source_location_id != expected.destination_pick_face_location_id
        && valid_uom(&expected.uom)
        && expected.lot.as_deref() == lot_scan
        && expected.serial.as_deref() == serial_scan
        && valid_scan(source_location_barcode)
        && valid_scan(item_barcode)
        && valid_scan(destination_pick_face_barcode)
        && lot_scan.is_none_or(valid_scan)
        && serial_scan.is_none_or(valid_scan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{DurableCommandDraft, RfCommand};
    use wareboxes_api_contract::v1::{ReplenishmentLocationResponse, Revision};

    #[test]
    fn confirmation_request_contains_scans_but_no_quantity() {
        let command = ReplenishmentCommand::Confirm {
            work_id: 42,
            expected: Box::new(ReplenishmentConfirmationExpectation {
                plan_id: 2,
                policy_id: 3,
                source_inventory_balance_id: 4,
                item_batch_id: 5,
                item_id: 6,
                uom: "each".into(),
                lot: Some("LOT-1".into()),
                serial: None,
                source_location_id: 7,
                destination_pick_face_location_id: 8,
                quantity: 9,
            }),
            source_location_barcode: "RES-01".into(),
            item_barcode: "SKU-1".into(),
            lot_scan: Some("LOT-1".into()),
            serial_scan: None,
            destination_pick_face_barcode: "PICK-01".into(),
        };
        let (_, body, kind) = build_command_parts(&command).unwrap();
        assert_eq!(kind, ResponseKind::ReplenishmentConfirmation);
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["source_location_barcode"], "RES-01");
        assert_eq!(value["destination_pick_face_barcode"], "PICK-01");
        assert!(value.get("quantity").is_none());

        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id: "command-confirm".into(),
            idempotency_key: "key-confirm".into(),
            command: RfCommand::Replenishment(command),
        };
        let durable = super::super::build_durable_request(&draft).unwrap();
        let restored: DurableCommandDraft =
            serde_json::from_slice(&serde_json::to_vec(&draft).unwrap()).unwrap();
        let rebuilt = super::super::build_durable_request(&restored).unwrap();
        assert_eq!(durable, rebuilt);
        assert!(rebuilt.verify_body());
        assert_eq!(rebuilt.body, body);
    }

    #[test]
    fn durable_request_rebuilds_exact_replenishment_bytes() {
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id: "command-1".into(),
            idempotency_key: "key-1".into(),
            command: RfCommand::Replenishment(ReplenishmentCommand::ClaimNext),
        };
        let request = super::super::build_durable_request(&draft).unwrap();
        assert_eq!(request.path, "/api/v1/replenishment-claims/next");
        assert!(request.verify_body());
        assert_eq!(request.body, b"{}");
    }

    #[test]
    fn negative_claim_priority_is_rejected() {
        let response = ReplenishmentClaimResponse {
            work_id: 42,
            plan_id: 20,
            policy_id: 10,
            policy_revision: Revision::new(3).unwrap(),
            inventory_owner_id: 2,
            facility_id: 3,
            sequence: 1,
            priority: -1,
            instructions: None,
            due_at: None,
            lease_expires_at: "2026-08-08T20:00:00Z".into(),
            source_inventory_balance_id: 100,
            item_batch_id: 101,
            item_id: 5,
            item_description: Some("Nitrile gloves".into()),
            item_barcodes: vec!["SKU-1".into()],
            uom: "each".into(),
            lot: None,
            serial: None,
            expiration: None,
            quantity: 8,
            source_location: ReplenishmentLocationResponse {
                location_id: 7,
                barcode: "RES-01".into(),
                name: None,
            },
            destination_pick_face: ReplenishmentLocationResponse {
                location_id: 8,
                barcode: "PICK-01".into(),
                name: None,
            },
        };
        assert!(matches!(
            map_claim(response),
            Err(WireResponseError::InvalidReplenishmentResponse)
        ));
    }
}
