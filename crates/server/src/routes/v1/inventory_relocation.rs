use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use wareboxes_api_contract::v1::{
    ClaimInventoryRelocationByIdRequest, ClaimNextInventoryRelocationRequest,
    ConfirmInventoryRelocationRequest, CreateInventoryRelocationTaskRequest,
    CreateInventoryRelocationTaskResponse, HeartbeatInventoryRelocationClaimRequest,
    InventoryBalanceStatus, InventoryRelocationClaimHeartbeatResponse,
    InventoryRelocationClaimReleaseReason, InventoryRelocationClaimReleaseResponse,
    InventoryRelocationClaimResponse, InventoryRelocationClaimWork as ContractClaimWork,
    InventoryRelocationConfirmationResponse, InventoryRelocationLocation,
    InventoryRelocationResult, InventoryRelocationWorkRequest,
    InventoryRelocationWorkflow as ContractWorkflow, ReleaseInventoryRelocationClaimRequest,
};
use wareboxes_core::models::{
    InventoryRelocationClaim, InventoryRelocationClaimReleaseReason as CoreReleaseReason,
    InventoryRelocationClaimWork as CoreClaimWork, InventoryRelocationConfirmation,
    InventoryRelocationConfirmationResult as CoreResult,
    InventoryRelocationWorkflow as CoreWorkflow, InventoryStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const MAX_BARCODE_LENGTH: usize = 200;
const MAX_INSTRUCTIONS_LENGTH: usize = 1_000;
const MAX_RELEASE_NOTE_LENGTH: usize = 500;

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateInventoryRelocationTaskRequest>,
) -> V1Result<Json<CreateInventoryRelocationTaskResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(body.destination_location_id, "destination location ID")?;
    if body.priority.is_some_and(|priority| priority < 0) {
        return Err(invalid("priority cannot be negative"));
    }
    if let Some(assigned_user_id) = body.assigned_user_id {
        require_positive(assigned_user_id, "assigned user ID")?;
    }
    let scheduled_for = parse_timestamp(body.scheduled_for.as_deref(), "scheduled_for")?;
    let due_at = parse_timestamp(body.due_at.as_deref(), "due_at")?;
    if scheduled_for
        .as_ref()
        .zip(due_at.as_ref())
        .is_some_and(|(scheduled_for, due_at)| due_at < scheduled_for)
    {
        return Err(invalid("due_at cannot be earlier than scheduled_for"));
    }
    validate_instructions(body.instructions.as_deref())?;
    let context = user.command_context(&idempotency_key);
    let priority = body.priority.unwrap_or(50);
    let task_id = match body.work {
        InventoryRelocationWorkRequest::LooseBalance {
            source_inventory_balance_id,
            quantity,
        } => {
            require_positive(source_inventory_balance_id, "source inventory balance ID")?;
            require_positive(quantity, "quantity")?;
            repo::tasks::create_loose_inventory_relocation_task_in_scope(
                &state.db,
                &user.tenant,
                &context,
                source_inventory_balance_id,
                body.destination_location_id,
                quantity,
                priority,
                body.assigned_user_id,
                scheduled_for,
                due_at,
                body.instructions.as_deref(),
            )
            .await?
        }
        InventoryRelocationWorkRequest::LicensePlate { license_plate_id } => {
            require_positive(license_plate_id, "license plate ID")?;
            repo::tasks::create_license_plate_inventory_relocation_task_in_scope(
                &state.db,
                &user.tenant,
                &context,
                license_plate_id,
                body.destination_location_id,
                priority,
                body.assigned_user_id,
                scheduled_for,
                due_at,
                body.instructions.as_deref(),
            )
            .await?
        }
    };
    Ok(Json(CreateInventoryRelocationTaskResponse { task_id }))
}

pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ConfirmInventoryRelocationRequest>,
) -> V1Result<Json<InventoryRelocationConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    validate_barcode(
        &body.destination_location_barcode,
        "destination_location_barcode",
    )?;
    if let Some(barcode) = body.license_plate_barcode.as_deref() {
        validate_barcode(barcode, "license_plate_barcode")?;
    }
    let context = user.command_context(&idempotency_key);
    let confirmation = repo::tasks::confirm_inventory_relocation_in_scope(
        &state.db,
        &user.tenant,
        &context,
        task_id,
        &body.destination_location_barcode,
        body.license_plate_barcode.as_deref(),
    )
    .await?;
    Ok(Json(map_confirmation(confirmation)))
}

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ClaimNextInventoryRelocationRequest>,
) -> V1Result<Json<Option<InventoryRelocationClaimResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let context = user.command_context(&idempotency_key);
    let claim = repo::tasks::claim_next_inventory_relocation_in_scope(
        &state.db,
        &user.tenant,
        &context,
        map_workflow(body.workflow),
    )
    .await?;
    Ok(Json(claim.map(map_claim)))
}

pub async fn claim_by_id(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<ClaimInventoryRelocationByIdRequest>,
) -> V1Result<Json<InventoryRelocationClaimResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let context = user.command_context(&idempotency_key);
    let claim = repo::tasks::claim_inventory_relocation_in_scope(
        &state.db,
        &user.tenant,
        &context,
        task_id,
    )
    .await?;
    Ok(Json(map_claim(claim)))
}

pub async fn current(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<Option<InventoryRelocationClaimResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let claim =
        repo::tasks::current_inventory_relocation_claim_in_scope(&state.db, &user.tenant).await?;
    Ok(Json(claim.map(map_claim)))
}

pub async fn heartbeat(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<HeartbeatInventoryRelocationClaimRequest>,
) -> V1Result<Json<InventoryRelocationClaimHeartbeatResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let context = user.command_context(&idempotency_key);
    let heartbeat = repo::tasks::heartbeat_inventory_relocation_claim_in_scope(
        &state.db,
        &user.tenant,
        &context,
        task_id,
    )
    .await?;
    Ok(Json(InventoryRelocationClaimHeartbeatResponse {
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
    Json(body): Json<ReleaseInventoryRelocationClaimRequest>,
) -> V1Result<Json<InventoryRelocationClaimReleaseResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    validate_release(&body)?;
    let context = user.command_context(&idempotency_key);
    let release = repo::tasks::release_inventory_relocation_claim_in_scope(
        &state.db,
        &user.tenant,
        &context,
        task_id,
        map_release_reason(body.reason),
        body.note.as_deref(),
    )
    .await?;
    Ok(Json(InventoryRelocationClaimReleaseResponse {
        task_id: release.task_id,
        released_at: release.released_at.to_rfc3339(),
        release_count: release.release_count,
        reason: body.reason,
        note: release.note,
    }))
}

fn map_workflow(workflow: ContractWorkflow) -> CoreWorkflow {
    match workflow {
        ContractWorkflow::LooseBalance => CoreWorkflow::LooseBalance,
        ContractWorkflow::LicensePlate => CoreWorkflow::LicensePlate,
    }
}

fn map_claim(claim: InventoryRelocationClaim) -> InventoryRelocationClaimResponse {
    InventoryRelocationClaimResponse {
        task_id: claim.task_id,
        inventory_owner_id: claim.inventory_owner_id.get(),
        facility_id: claim.facility_id,
        priority: claim.priority,
        instructions: claim.instructions,
        due_at: claim.due_at.map(|timestamp| timestamp.to_rfc3339()),
        lease_expires_at: claim.lease_expires_at.to_rfc3339(),
        source_location: InventoryRelocationLocation {
            location_id: claim.source_location.location_id,
            barcode: claim.source_location.barcode,
            name: claim.source_location.name,
        },
        destination_location: InventoryRelocationLocation {
            location_id: claim.destination_location.location_id,
            barcode: claim.destination_location.barcode,
            name: claim.destination_location.name,
        },
        work: match claim.work {
            CoreClaimWork::LooseBalance {
                source_inventory_balance_id,
                item_batch_id,
                item_id,
                item_description,
                uom,
                lot,
                serial,
                expiration,
                inventory_status,
                quantity,
            } => ContractClaimWork::LooseBalance {
                source_inventory_balance_id,
                item_batch_id,
                item_id,
                item_description,
                uom,
                lot,
                serial,
                expiration: expiration.map(|timestamp| timestamp.to_rfc3339()),
                inventory_status: map_inventory_status(inventory_status),
                quantity,
            },
            CoreClaimWork::LicensePlate {
                license_plate_id,
                license_plate_barcode,
                planned_balance_count,
            } => ContractClaimWork::LicensePlate {
                license_plate_id,
                license_plate_barcode,
                planned_balance_count,
            },
        },
    }
}

fn map_confirmation(
    confirmation: InventoryRelocationConfirmation,
) -> InventoryRelocationConfirmationResponse {
    InventoryRelocationConfirmationResponse {
        task_id: confirmation.task_id,
        inventory_owner_id: confirmation.inventory_owner_id.get(),
        facility_id: confirmation.facility_id,
        source_location_id: confirmation.source_location_id,
        destination_location_id: confirmation.destination_location_id,
        destination_location_barcode: confirmation.destination_location_barcode,
        inventory_transaction_id: confirmation.inventory_transaction_id,
        confirmed_by: confirmation.confirmed_by,
        confirmed_at: confirmation.confirmed_at.to_rfc3339(),
        result: match confirmation.result {
            CoreResult::LooseBalance {
                source_inventory_balance_id,
                destination_inventory_balance_id,
                item_batch_id,
                item_id,
                inventory_status,
                uom,
                quantity,
            } => InventoryRelocationResult::LooseBalance {
                source_inventory_balance_id,
                destination_inventory_balance_id,
                item_batch_id,
                item_id,
                inventory_status: map_inventory_status(inventory_status),
                uom,
                quantity,
            },
            CoreResult::LicensePlate {
                license_plate_id,
                license_plate_barcode,
                moved_balance_count,
            } => InventoryRelocationResult::LicensePlate {
                license_plate_id,
                license_plate_barcode,
                moved_balance_count,
            },
        },
    }
}

fn map_inventory_status(status: InventoryStatus) -> InventoryBalanceStatus {
    match status {
        InventoryStatus::Available => InventoryBalanceStatus::Available,
        InventoryStatus::Hold => InventoryBalanceStatus::Hold,
        InventoryStatus::Damaged => InventoryBalanceStatus::Damaged,
        InventoryStatus::Quarantine => InventoryBalanceStatus::Quarantine,
    }
}

fn map_release_reason(reason: InventoryRelocationClaimReleaseReason) -> CoreReleaseReason {
    match reason {
        InventoryRelocationClaimReleaseReason::WorkInterrupted => {
            CoreReleaseReason::WorkInterrupted
        }
        InventoryRelocationClaimReleaseReason::EquipmentUnavailable => {
            CoreReleaseReason::EquipmentUnavailable
        }
        InventoryRelocationClaimReleaseReason::DestinationBlocked => {
            CoreReleaseReason::DestinationBlocked
        }
        InventoryRelocationClaimReleaseReason::SafetyIssue => CoreReleaseReason::SafetyIssue,
        InventoryRelocationClaimReleaseReason::Other => CoreReleaseReason::Other,
    }
}

fn parse_timestamp(value: Option<&str>, field: &str) -> V1Result<Option<DateTime<Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim() != value || value.is_empty() {
        return Err(invalid(format!(
            "{field} must be a nonempty RFC3339 timestamp"
        )));
    }
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| Some(timestamp.with_timezone(&Utc)))
        .map_err(|_| invalid(format!("{field} must be an RFC3339 timestamp")))
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

fn validate_instructions(instructions: Option<&str>) -> V1Result<()> {
    let Some(instructions) = instructions else {
        return Ok(());
    };
    if instructions.trim() != instructions || instructions.is_empty() {
        return Err(invalid("instructions must be trimmed and nonempty"));
    }
    if instructions.chars().count() > MAX_INSTRUCTIONS_LENGTH {
        return Err(invalid(format!(
            "instructions cannot exceed {MAX_INSTRUCTIONS_LENGTH} characters"
        )));
    }
    Ok(())
}

fn validate_release(body: &ReleaseInventoryRelocationClaimRequest) -> V1Result<()> {
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
    if body.reason == InventoryRelocationClaimReleaseReason::Other && body.note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    Ok(())
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
