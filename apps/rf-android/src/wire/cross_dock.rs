use chrono::DateTime;
use wareboxes_api_contract::v1::{
    ClaimCrossDockWorkByIdRequest, ClaimNextCrossDockWorkRequest, ConfirmCrossDockWorkRequest,
    ConfirmCrossDockWorkResponse, CrossDockClaimHeartbeatResponse,
    CrossDockClaimReleaseReason as ApiReleaseReason, CrossDockClaimReleaseResponse,
    CrossDockClaimResponse, CrossDockWorkStatus, HeartbeatCrossDockClaimRequest,
    ReleaseCrossDockClaimRequest,
};

use crate::cross_dock::{
    CrossDockClaim, CrossDockCommand, CrossDockConfirmationExpectation,
    CrossDockConfirmationResult, CrossDockLocation, CrossDockReleaseReason,
};
use crate::workflow::CommandOutcome;

use super::{API_PREFIX, ResponseKind, WireRequestError, WireResponseError};

pub(super) fn build_command_parts(
    command: &CrossDockCommand,
) -> Result<(String, Vec<u8>, ResponseKind), WireRequestError> {
    match command {
        CrossDockCommand::ClaimNext => Ok((
            format!("{API_PREFIX}/cross-dock-claims/next"),
            serde_json::to_vec(&ClaimNextCrossDockWorkRequest::default())?,
            ResponseKind::CrossDockOptionalClaim,
        )),
        CrossDockCommand::ClaimById { work_id } => {
            super::validate_task_id(*work_id)?;
            Ok((
                format!("{API_PREFIX}/cross-dock-claims/{work_id}"),
                serde_json::to_vec(&ClaimCrossDockWorkByIdRequest::default())?,
                ResponseKind::CrossDockClaim,
            ))
        }
        CrossDockCommand::Confirm {
            work_id,
            expected,
            source_receiving_location_barcode,
            item_barcode,
            lot_scan,
            serial_scan,
            destination_pick_face_barcode,
        } => {
            super::validate_task_id(*work_id)?;
            if !valid_confirmation(
                expected,
                source_receiving_location_barcode,
                item_barcode,
                lot_scan.as_deref(),
                serial_scan.as_deref(),
                destination_pick_face_barcode,
            ) {
                return Err(WireRequestError::InvalidCrossDockCommand);
            }
            Ok((
                format!("{API_PREFIX}/cross-dock-tasks/{work_id}/confirmations"),
                serde_json::to_vec(&ConfirmCrossDockWorkRequest {
                    source_receiving_location_barcode: source_receiving_location_barcode.clone(),
                    item_barcode: item_barcode.clone(),
                    lot_scan: lot_scan.clone(),
                    serial_scan: serial_scan.clone(),
                    destination_pick_face_barcode: destination_pick_face_barcode.clone(),
                })?,
                ResponseKind::CrossDockConfirmation,
            ))
        }
        CrossDockCommand::Release {
            work_id,
            reason,
            note,
        } => {
            super::validate_task_id(*work_id)?;
            Ok((
                format!("{API_PREFIX}/cross-dock-claims/{work_id}/releases"),
                serde_json::to_vec(&ReleaseCrossDockClaimRequest {
                    reason: map_release_reason(*reason),
                    note: note.clone(),
                })?,
                ResponseKind::CrossDockRelease,
            ))
        }
    }
}

pub fn build_heartbeat_request_parts(work_id: i64) -> Result<(String, Vec<u8>), WireRequestError> {
    super::validate_task_id(work_id)?;
    Ok((
        format!("{API_PREFIX}/cross-dock-claims/{work_id}/heartbeats"),
        serde_json::to_vec(&HeartbeatCrossDockClaimRequest::default())?,
    ))
}

pub fn decode_claim_response(body: &[u8]) -> Result<Option<CrossDockClaim>, WireResponseError> {
    serde_json::from_slice::<Option<CrossDockClaimResponse>>(body)?
        .map(map_claim)
        .transpose()
}

pub fn decode_heartbeat_response(
    expected_work_id: i64,
    status: u16,
    body: &[u8],
) -> Result<CrossDockClaimHeartbeatResponse, WireResponseError> {
    if !(200..300).contains(&status) {
        return Err(WireResponseError::UnsuccessfulStatus(status));
    }
    if expected_work_id <= 0 {
        return Err(WireResponseError::InvalidHeartbeatTaskId);
    }
    let response = serde_json::from_slice::<CrossDockClaimHeartbeatResponse>(body)?;
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
        ResponseKind::CrossDockOptionalClaim => Ok(CommandOutcome::CrossDockClaimed(
            serde_json::from_slice::<Option<CrossDockClaimResponse>>(body)?
                .map(map_claim)
                .transpose()?
                .map(Box::new),
        )),
        ResponseKind::CrossDockClaim => Ok(CommandOutcome::CrossDockClaimed(Some(Box::new(
            map_claim(serde_json::from_slice::<CrossDockClaimResponse>(body)?)?,
        )))),
        ResponseKind::CrossDockConfirmation => {
            let response = serde_json::from_slice::<ConfirmCrossDockWorkResponse>(body)?;
            Ok(CommandOutcome::CrossDockConfirmed(Box::new(
                map_confirmation(response)?,
            )))
        }
        ResponseKind::CrossDockRelease => {
            let response = serde_json::from_slice::<CrossDockClaimReleaseResponse>(body)?;
            if response.work_id <= 0 || response.status != CrossDockWorkStatus::Pending {
                return Err(WireResponseError::InvalidCrossDockResponse);
            }
            validate_timestamp(&response.released_at, "released_at")?;
            Ok(CommandOutcome::CrossDockReleased {
                work_id: response.work_id,
            })
        }
        _ => Err(WireResponseError::InvalidCrossDockResponse),
    }
}

fn map_claim(response: CrossDockClaimResponse) -> Result<CrossDockClaim, WireResponseError> {
    if [
        response.work_id,
        response.plan_id,
        response.inventory_owner_id,
        response.facility_id,
        response.order_id,
        response.order_line_id,
        response.reservation_id,
        response.source_receipt_inventory_transaction_id,
        response.source_inventory_balance_id,
        response.item_batch_id,
        response.item_id,
        response.quantity,
        response.source_receiving_location.location_id,
        response.destination_pick_face.location_id,
    ]
    .into_iter()
    .any(|value| value <= 0)
        || response.priority < 0
        || response.order_key.trim().is_empty()
        || response.order_line_key.trim().is_empty()
        || response.source_receiving_location.location_id
            == response.destination_pick_face.location_id
        || !valid_scan(&response.source_receiving_location.barcode)
        || !valid_scan(&response.destination_pick_face.barcode)
        || response.item_barcodes.is_empty()
        || response
            .item_barcodes
            .iter()
            .any(|value| !valid_scan(value))
        || !valid_uom(&response.uom)
        || response
            .lot
            .as_ref()
            .is_some_and(|value| !valid_scan(value))
        || response
            .serial
            .as_ref()
            .is_some_and(|value| !valid_scan(value))
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
        return Err(WireResponseError::InvalidCrossDockResponse);
    }
    Ok(CrossDockClaim {
        work_id: response.work_id,
        plan_id: response.plan_id,
        inventory_owner_id: response.inventory_owner_id,
        facility_id: response.facility_id,
        order_id: response.order_id,
        order_key: response.order_key,
        order_line_id: response.order_line_id,
        order_line_key: response.order_line_key,
        reservation_id: response.reservation_id,
        priority: response.priority,
        instructions: response.instructions,
        due_at: response.due_at,
        lease_expires_at: response.lease_expires_at,
        source_receipt_inventory_transaction_id: response.source_receipt_inventory_transaction_id,
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
        source_receiving_location: CrossDockLocation {
            location_id: response.source_receiving_location.location_id,
            barcode: response.source_receiving_location.barcode,
            name: response.source_receiving_location.name,
        },
        destination_pick_face: CrossDockLocation {
            location_id: response.destination_pick_face.location_id,
            barcode: response.destination_pick_face.barcode,
            name: response.destination_pick_face.name,
        },
    })
}

fn map_confirmation(
    response: ConfirmCrossDockWorkResponse,
) -> Result<CrossDockConfirmationResult, WireResponseError> {
    if [
        response.confirmation_id,
        response.work_id,
        response.plan_id,
        response.order_id,
        response.order_line_id,
        response.reservation_id,
        response.inventory_transaction_id,
        response.inventory_allocation_id,
        response.source_inventory_balance_id,
        response.destination_inventory_balance_id,
        response.source_location_id,
        response.destination_pick_face_location_id,
        response.item_batch_id,
        response.item_id,
        response.quantity,
        response.confirmed_by,
    ]
    .into_iter()
    .any(|value| value <= 0)
        || response.source_inventory_balance_id == response.destination_inventory_balance_id
        || response.source_location_id == response.destination_pick_face_location_id
        || response.status != CrossDockWorkStatus::Completed
        || !valid_uom(&response.uom)
        || response
            .lot
            .as_ref()
            .is_some_and(|value| !valid_scan(value))
        || response
            .serial
            .as_ref()
            .is_some_and(|value| !valid_scan(value))
        || DateTime::parse_from_rfc3339(&response.confirmed_at).is_err()
    {
        return Err(WireResponseError::InvalidCrossDockResponse);
    }
    Ok(CrossDockConfirmationResult {
        confirmation_id: response.confirmation_id,
        work_id: response.work_id,
        plan_id: response.plan_id,
        order_id: response.order_id,
        order_line_id: response.order_line_id,
        reservation_id: response.reservation_id,
        inventory_transaction_id: response.inventory_transaction_id,
        inventory_allocation_id: response.inventory_allocation_id,
        source_inventory_balance_id: response.source_inventory_balance_id,
        destination_inventory_balance_id: response.destination_inventory_balance_id,
        source_location_id: response.source_location_id,
        destination_pick_face_location_id: response.destination_pick_face_location_id,
        item_batch_id: response.item_batch_id,
        item_id: response.item_id,
        uom: response.uom,
        lot: response.lot,
        serial: response.serial,
        quantity: response.quantity,
        confirmed_by: response.confirmed_by,
        confirmed_at: response.confirmed_at,
    })
}

const fn map_release_reason(reason: CrossDockReleaseReason) -> ApiReleaseReason {
    match reason {
        CrossDockReleaseReason::WorkInterrupted => ApiReleaseReason::WorkInterrupted,
        CrossDockReleaseReason::EndOfShift => ApiReleaseReason::EndOfShift,
        CrossDockReleaseReason::EquipmentIssue => ApiReleaseReason::EquipmentIssue,
        CrossDockReleaseReason::Other => ApiReleaseReason::Other,
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
    expected: &CrossDockConfirmationExpectation,
    source: &str,
    item: &str,
    lot: Option<&str>,
    serial: Option<&str>,
    destination: &str,
) -> bool {
    [
        expected.plan_id,
        expected.order_id,
        expected.order_line_id,
        expected.reservation_id,
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
        && expected.lot.as_deref() == lot
        && expected.serial.as_deref() == serial
        && valid_scan(source)
        && valid_scan(item)
        && valid_scan(destination)
        && lot.is_none_or(valid_scan)
        && serial.is_none_or(valid_scan)
}
