use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use wareboxes_api_contract::v1::{
    ConfirmPutawayRequest, CreatePutawayTaskRequest, CreatePutawayTaskResponse,
    PutawayConfirmationResponse,
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

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreatePutawayTaskRequest>,
) -> V1Result<Json<CreatePutawayTaskResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(
        body.source_inventory_balance_id,
        "source inventory balance ID",
    )?;
    require_positive(body.destination_location_id, "destination location ID")?;
    require_positive(body.quantity, "quantity")?;
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
    let task_id = repo::tasks::create_putaway_task_in_scope(
        &state.db,
        &user.tenant,
        &context,
        body.source_inventory_balance_id,
        body.destination_location_id,
        body.quantity,
        body.priority.unwrap_or(50),
        body.assigned_user_id,
        scheduled_for,
        due_at,
        body.instructions.as_deref(),
    )
    .await?;

    Ok(Json(CreatePutawayTaskResponse { task_id }))
}

pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ConfirmPutawayRequest>,
) -> V1Result<Json<PutawayConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    validate_barcode(
        &body.destination_location_barcode,
        "destination_location_barcode",
    )?;
    let context = user.command_context(&idempotency_key);
    let confirmation = repo::tasks::confirm_putaway_in_scope(
        &state.db,
        &user.tenant,
        &context,
        task_id,
        &body.destination_location_barcode,
    )
    .await?;

    Ok(Json(PutawayConfirmationResponse {
        task_id: confirmation.task_id,
        inventory_owner_id: confirmation.inventory_owner_id.get(),
        facility_id: confirmation.facility_id,
        inventory_transaction_id: confirmation.inventory_transaction_id,
        source_inventory_balance_id: confirmation.source_inventory_balance_id,
        destination_inventory_balance_id: confirmation.destination_inventory_balance_id,
        source_location_id: confirmation.source_location_id,
        destination_location_id: confirmation.destination_location_id,
        destination_location_barcode: confirmation.destination_location_barcode,
        item_batch_id: confirmation.item_batch_id,
        item_id: confirmation.item_id,
        quantity: confirmation.quantity,
        inventory_status: confirmation.inventory_status.as_str().to_owned(),
        confirmed_by: confirmation.confirmed_by,
        confirmed_at: confirmation.confirmed_at.to_rfc3339(),
    }))
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
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

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
