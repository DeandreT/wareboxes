use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ClaimCycleCountByIdRequest, ClaimNextCycleCountRequest, ConfirmCycleCountRequest,
    CycleCountClaimHeartbeatResponse, CycleCountClaimReleaseReason, CycleCountClaimReleaseResponse,
    CycleCountClaimResponse, CycleCountConfirmationResponse, CycleCountItem, CycleCountLocation,
    CycleCountStock, HeartbeatCycleCountClaimRequest, InventoryBalanceStatus,
    ReleaseCycleCountClaimRequest,
};
use wareboxes_core::models::{
    CycleCountClaim, CycleCountClaimReleaseReason as CoreReleaseReason, InventoryStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const MAX_BARCODE_LENGTH: usize = 200;
const MAX_NOTE_LENGTH: usize = 1_000;

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(_body): Json<ClaimNextCycleCountRequest>,
) -> V1Result<Json<Option<CycleCountClaimResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = user.command_context(&idempotency_key);
    let claim =
        repo::tasks::claim_next_cycle_count_in_scope(&state.db, &user.tenant, &command).await?;
    Ok(Json(claim.map(map_claim)))
}

pub async fn claim_by_id(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<ClaimCycleCountByIdRequest>,
) -> V1Result<Json<CycleCountClaimResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let command = user.command_context(&idempotency_key);
    let claim =
        repo::tasks::claim_cycle_count_by_id_in_scope(&state.db, &user.tenant, &command, task_id)
            .await?;
    Ok(Json(map_claim(claim)))
}

pub async fn current(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<Option<CycleCountClaimResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let claim = repo::tasks::get_current_cycle_count_claim_in_scope(
        &state.db,
        &user.tenant,
        user.tenant.user_id.get(),
    )
    .await?;
    Ok(Json(claim.map(map_claim)))
}

pub async fn heartbeat(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<HeartbeatCycleCountClaimRequest>,
) -> V1Result<Json<CycleCountClaimHeartbeatResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let command = user.command_context(&idempotency_key);
    let heartbeat = repo::tasks::heartbeat_cycle_count_claim_in_scope(
        &state.db,
        &user.tenant,
        &command,
        task_id,
    )
    .await?;
    Ok(Json(CycleCountClaimHeartbeatResponse {
        task_id: heartbeat.task_id,
        heartbeat_at: heartbeat.heartbeat_at.to_rfc3339(),
        lease_expires_at: heartbeat.lease_expires_at.to_rfc3339(),
    }))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ReleaseCycleCountClaimRequest>,
) -> V1Result<Json<CycleCountClaimReleaseResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let command = user.command_context(&idempotency_key);
    let reason = map_release_reason(body.reason);
    let release = repo::tasks::release_cycle_count_claim_in_scope(
        &state.db,
        &user.tenant,
        &command,
        task_id,
        reason,
        body.note.as_deref(),
    )
    .await?;
    Ok(Json(CycleCountClaimReleaseResponse {
        task_id: release.task_id,
        released_at: release.released_at.to_rfc3339(),
        release_count: release.release_count,
        reason: body.reason,
        note: release.note,
    }))
}

pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ConfirmCycleCountRequest>,
) -> V1Result<Json<CycleCountConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    validate_barcode(&body.location_barcode, "location_barcode")?;
    validate_barcode(&body.item_barcode, "item_barcode")?;
    if let Some(barcode) = body.license_plate_barcode.as_deref() {
        validate_barcode(barcode, "license_plate_barcode")?;
    }
    if body.counted_quantity < 0 {
        return Err(invalid("counted_quantity cannot be negative"));
    }
    validate_note(body.note.as_deref())?;
    let command = user.command_context(&idempotency_key);
    let confirmation = repo::tasks::confirm_scanned_item_location_cycle_count_in_scope(
        &state.db,
        &user.tenant,
        &command,
        task_id,
        &body.location_barcode,
        &body.item_barcode,
        body.license_plate_barcode.as_deref(),
        body.counted_quantity,
        body.note.as_deref(),
    )
    .await?;
    Ok(Json(CycleCountConfirmationResponse {
        task_id: confirmation.task_id,
        inventory_owner_id: confirmation.inventory_owner_id.get(),
        facility_id: confirmation.facility_id,
        location_id: confirmation.location_id,
        inventory_balance_id: confirmation.inventory_balance_id,
        counted_quantity: confirmation.counted_quantity,
        variance_quantity: confirmation.variance_quantity,
        inventory_transaction_id: confirmation.inventory_transaction_id,
        confirmed_by: confirmation.confirmed_by,
        confirmed_at: confirmation.confirmed_at.to_rfc3339(),
    }))
}

fn map_claim(claim: CycleCountClaim) -> CycleCountClaimResponse {
    CycleCountClaimResponse {
        task_id: claim.task_id,
        inventory_owner_id: claim.inventory_owner_id.get(),
        facility_id: claim.facility_id,
        priority: claim.priority,
        instructions: claim.instructions,
        due_at: claim.due_at.map(|timestamp| timestamp.to_rfc3339()),
        lease_expires_at: claim.lease_expires_at.to_rfc3339(),
        location: CycleCountLocation {
            location_id: claim.location.location_id,
            barcode: claim.location.barcode,
            name: claim.location.name,
        },
        item: CycleCountItem {
            item_id: claim.item.item_id,
            description: claim.item.description,
            barcodes: claim.item.barcodes,
        },
        stock: CycleCountStock {
            inventory_balance_id: claim.stock.inventory_balance_id,
            license_plate_barcode: claim.stock.license_plate_barcode,
            uom: claim.stock.uom,
            lot: claim.stock.lot,
            expiration: claim
                .stock
                .expiration
                .map(|timestamp| timestamp.to_rfc3339()),
            serial: claim.stock.serial,
            inventory_status: map_inventory_status(claim.stock.inventory_status),
        },
    }
}

const fn map_release_reason(reason: CycleCountClaimReleaseReason) -> CoreReleaseReason {
    match reason {
        CycleCountClaimReleaseReason::WorkInterrupted => CoreReleaseReason::WorkInterrupted,
        CycleCountClaimReleaseReason::EquipmentUnavailable => {
            CoreReleaseReason::EquipmentUnavailable
        }
        CycleCountClaimReleaseReason::SafetyIssue => CoreReleaseReason::SafetyIssue,
        CycleCountClaimReleaseReason::Other => CoreReleaseReason::Other,
    }
}

const fn map_inventory_status(status: InventoryStatus) -> InventoryBalanceStatus {
    match status {
        InventoryStatus::Available => InventoryBalanceStatus::Available,
        InventoryStatus::Hold => InventoryBalanceStatus::Hold,
        InventoryStatus::Quarantine => InventoryBalanceStatus::Quarantine,
        InventoryStatus::Damaged => InventoryBalanceStatus::Damaged,
    }
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn validate_barcode(value: &str, field: &str) -> V1Result<()> {
    if value.trim() != value || value.is_empty() {
        return Err(invalid(format!("{field} must be trimmed and nonempty")));
    }
    if value.chars().count() > MAX_BARCODE_LENGTH {
        return Err(invalid(format!(
            "{field} cannot exceed {MAX_BARCODE_LENGTH} characters"
        )));
    }
    Ok(())
}

fn validate_note(note: Option<&str>) -> V1Result<()> {
    let Some(note) = note else {
        return Ok(());
    };
    if note.trim() != note || note.is_empty() {
        return Err(invalid("note must be trimmed and nonempty"));
    }
    if note.chars().count() > MAX_NOTE_LENGTH {
        return Err(invalid(format!(
            "note cannot exceed {MAX_NOTE_LENGTH} characters"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
