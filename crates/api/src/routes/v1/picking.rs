use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ClaimNextPickRequest, ClaimPickByIdRequest, ConfirmPickContentRequest, CurrentPickResponse,
    HeartbeatPickClaimRequest, PickClaimContent as ApiPickClaimContent, PickClaimHeartbeatResponse,
    PickClaimReleaseReason as ApiReleaseReason, PickClaimReleaseResponse, PickClaimResponse,
    PickContentConfirmationResponse, PickContentState as ApiContentState, PickOrderStatus,
    ReleasePickClaimRequest, Revision,
};
use wareboxes_application::picking::{
    ClaimNextPickCommand, ClaimPickByIdCommand, ConfirmPickContentCommand,
    ConfirmPickContentResult, HeartbeatPickClaimCommand, PickClaim, PickClaimContent,
    PickClaimHeartbeatResult, PickClaimReleaseResult, ReleasePickClaimCommand,
};
use wareboxes_domain::{
    OrderStatus, PickClaimReleaseReason, PickContentId, PickContentState, PickScanValue,
    PickTaskId, MAX_PICK_SCAN_VALUE_LENGTH,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const MAX_RELEASE_NOTE_LENGTH: usize = 500;

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(_body): Json<ClaimNextPickRequest>,
) -> V1Result<Json<CurrentPickResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let context = user.command_context(&idempotency_key);
    let claim =
        repo::picking::claim_next(&state.db, &user.tenant, &context, ClaimNextPickCommand).await?;
    Ok(Json(claim.map(map_claim).transpose()?))
}

pub async fn claim_by_id(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<ClaimPickByIdRequest>,
) -> V1Result<Json<PickClaimResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ClaimPickByIdCommand {
        task_id: pick_task_id(task_id)?,
    };
    let context = user.command_context(&idempotency_key);
    let claim = repo::picking::claim_by_id(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_claim(claim)?))
}

pub async fn current(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<CurrentPickResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let claim = repo::picking::current(&state.db, &user.tenant).await?;
    Ok(Json(claim.map(map_claim).transpose()?))
}

pub async fn heartbeat(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<HeartbeatPickClaimRequest>,
) -> V1Result<Json<PickClaimHeartbeatResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = HeartbeatPickClaimCommand {
        task_id: pick_task_id(task_id)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::heartbeat(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_heartbeat(result)))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ReleasePickClaimRequest>,
) -> V1Result<Json<PickClaimReleaseResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    validate_release(&body)?;
    let command = ReleasePickClaimCommand {
        task_id: pick_task_id(task_id)?,
        reason: map_release_reason(body.reason),
        note: body.note,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::release_claim(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_release(result)))
}

pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((task_id, content_id)): Path<(i64, i64)>,
    Json(body): Json<ConfirmPickContentRequest>,
) -> V1Result<Json<PickContentConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ConfirmPickContentCommand {
        task_id: pick_task_id(task_id)?,
        content_id: PickContentId::new(content_id).map_err(domain_validation)?,
        source_location_barcode: scan(body.source_location_barcode, "source location barcode")?,
        item_barcode: scan(body.item_barcode, "item barcode")?,
        source_license_plate_barcode: body
            .source_license_plate_barcode
            .map(|value| scan(value, "source license plate barcode"))
            .transpose()?,
        destination_license_plate_barcode: scan(
            body.destination_license_plate_barcode,
            "destination license plate barcode",
        )?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::confirm_content(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_confirmation(result)?))
}

fn map_claim(claim: PickClaim) -> V1Result<PickClaimResponse> {
    Ok(PickClaimResponse {
        task_id: claim.task_id.get(),
        order_id: claim.order_id.get(),
        inventory_owner_id: claim.inventory_owner_id.get(),
        facility_id: claim.facility_id.get(),
        order_key: claim.order_key,
        order_revision: Revision::new(claim.order_revision.get())
            .map_err(|error| V1Error::internal(error.to_string()))?,
        priority: claim.priority,
        ship_by: claim.ship_by.map(|value| value.to_rfc3339()),
        lease_expires_at: claim.lease_expires_at.to_rfc3339(),
        destination_location_id: claim.destination_location_id.get(),
        destination_location_barcode: claim.destination_location_barcode.into_inner(),
        destination_location_name: claim.destination_location_name,
        content: map_content(claim.content),
    })
}

fn map_content(content: PickClaimContent) -> ApiPickClaimContent {
    ApiPickClaimContent {
        content_id: content.content_id.get(),
        order_line_id: content.order_line_id.get(),
        inventory_allocation_id: content.inventory_allocation_id.get(),
        source_inventory_balance_id: content.source_inventory_balance_id.get(),
        item_batch_id: content.item_batch_id.get(),
        source_location_id: content.source_location_id.get(),
        source_location_barcode: content.source_location_barcode.into_inner(),
        source_location_name: content.source_location_name,
        source_license_plate_id: content.source_license_plate_id.map(|id| id.get()),
        source_license_plate_barcode: content
            .source_license_plate_barcode
            .map(PickScanValue::into_inner),
        item_id: content.item_id,
        item_description: content.item_description,
        item_barcodes: content
            .item_barcodes
            .into_iter()
            .map(PickScanValue::into_inner)
            .collect(),
        uom: content.uom,
        lot: content.lot,
        serial: content.serial,
        expiration: content.expiration.map(|value| value.to_rfc3339()),
        planned_quantity: content.planned_quantity.get(),
        state: map_content_state(content.state),
    }
}

fn map_heartbeat(result: PickClaimHeartbeatResult) -> PickClaimHeartbeatResponse {
    PickClaimHeartbeatResponse {
        task_id: result.task_id.get(),
        heartbeat_at: result.heartbeat_at.to_rfc3339(),
        lease_expires_at: result.lease_expires_at.to_rfc3339(),
    }
}

fn map_release(result: PickClaimReleaseResult) -> PickClaimReleaseResponse {
    PickClaimReleaseResponse {
        task_id: result.task_id.get(),
        released_at: result.released_at.to_rfc3339(),
        release_count: result.release_count,
        reason: map_release_reason_to_api(result.reason),
        note: result.note,
    }
}

fn map_confirmation(result: ConfirmPickContentResult) -> V1Result<PickContentConfirmationResponse> {
    let order_status = match result.order_status {
        OrderStatus::Processing => PickOrderStatus::Processing,
        OrderStatus::AwaitingPacking => PickOrderStatus::AwaitingPacking,
        _ => {
            return Err(V1Error::internal(
                "pick confirmation produced an invalid order status",
            ))
        }
    };
    Ok(PickContentConfirmationResponse {
        result_id: result.result_id,
        content_id: result.content_id.get(),
        task_id: result.task_id.get(),
        order_id: result.order_id.get(),
        inventory_transaction_id: result.inventory_transaction_id,
        source_inventory_allocation_id: result.source_inventory_allocation_id.get(),
        destination_inventory_allocation_id: result.destination_inventory_allocation_id.get(),
        source_inventory_balance_id: result.source_inventory_balance_id.get(),
        destination_inventory_balance_id: result.destination_inventory_balance_id.get(),
        source_location_id: result.source_location_id.get(),
        destination_location_id: result.destination_location_id.get(),
        source_license_plate_id: result.source_license_plate_id.map(|id| id.get()),
        destination_license_plate_id: result.destination_license_plate_id.get(),
        picked_quantity: result.picked_quantity.get(),
        confirmed_by: result.confirmed_by.get(),
        confirmed_at: result.confirmed_at.to_rfc3339(),
        content_state: map_content_state(result.content_state),
        task_completed: result.task_completed,
        order_ready_to_pack: result.order_ready_to_pack,
        order_status,
        order_revision: Revision::new(result.order_revision.get())
            .map_err(|error| V1Error::internal(error.to_string()))?,
    })
}

fn validate_release(body: &ReleasePickClaimRequest) -> V1Result<()> {
    if let Some(note) = body.note.as_deref() {
        if note.trim() != note || note.is_empty() {
            return Err(invalid("note must be trimmed and nonempty when provided"));
        }
        if note.chars().count() > MAX_RELEASE_NOTE_LENGTH {
            return Err(invalid(format!(
                "note cannot exceed {MAX_RELEASE_NOTE_LENGTH} characters"
            )));
        }
    }
    if body.reason == ApiReleaseReason::Other && body.note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    Ok(())
}

fn scan(value: String, label: &str) -> V1Result<PickScanValue> {
    PickScanValue::new(value).map_err(|error| {
        invalid(format!(
            "invalid {label}: {error}; maximum length is {MAX_PICK_SCAN_VALUE_LENGTH}"
        ))
    })
}

fn pick_task_id(value: i64) -> V1Result<PickTaskId> {
    PickTaskId::new(value).map_err(domain_validation)
}

fn map_content_state(state: PickContentState) -> ApiContentState {
    match state {
        PickContentState::Pending => ApiContentState::Pending,
        PickContentState::Completed => ApiContentState::Completed,
        PickContentState::Shorted => ApiContentState::Shorted,
    }
}

fn map_release_reason(reason: ApiReleaseReason) -> PickClaimReleaseReason {
    match reason {
        ApiReleaseReason::WorkInterrupted => PickClaimReleaseReason::WorkInterrupted,
        ApiReleaseReason::EquipmentUnavailable => PickClaimReleaseReason::EquipmentUnavailable,
        ApiReleaseReason::SourceBlocked => PickClaimReleaseReason::SourceBlocked,
        ApiReleaseReason::InventoryDiscrepancy => PickClaimReleaseReason::InventoryDiscrepancy,
        ApiReleaseReason::SafetyIssue => PickClaimReleaseReason::SafetyIssue,
        ApiReleaseReason::Other => PickClaimReleaseReason::Other,
    }
}

fn map_release_reason_to_api(reason: PickClaimReleaseReason) -> ApiReleaseReason {
    match reason {
        PickClaimReleaseReason::WorkInterrupted => ApiReleaseReason::WorkInterrupted,
        PickClaimReleaseReason::EquipmentUnavailable => ApiReleaseReason::EquipmentUnavailable,
        PickClaimReleaseReason::SourceBlocked => ApiReleaseReason::SourceBlocked,
        PickClaimReleaseReason::InventoryDiscrepancy => ApiReleaseReason::InventoryDiscrepancy,
        PickClaimReleaseReason::SafetyIssue => ApiReleaseReason::SafetyIssue,
        PickClaimReleaseReason::Other => ApiReleaseReason::Other,
    }
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    invalid(error.to_string())
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
