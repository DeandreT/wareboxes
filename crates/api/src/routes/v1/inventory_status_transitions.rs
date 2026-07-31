use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CreateInventoryStatusTransitionRequest, InventoryBalanceStatus,
    InventoryStatusTransitionReason, InventoryStatusTransitionResponse,
};
use wareboxes_core::models::{
    InventoryStatus as CoreInventoryStatus,
    InventoryStatusChangeReason as CoreInventoryStatusChangeReason,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const MAX_NOTE_LENGTH: usize = 1_000;
const MAX_REFERENCE_TYPE_LENGTH: usize = 100;

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(balance_id): Path<i64>,
    Json(body): Json<CreateInventoryStatusTransitionRequest>,
) -> V1Result<Json<InventoryStatusTransitionResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    validate_request(balance_id, &body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::inventory::change_inventory_status(
        &state.db,
        &user.tenant,
        &context,
        &repo::inventory::ChangeInventoryStatusCommand {
            inventory_balance_id: balance_id,
            qty: body.quantity,
            to_status: map_status_to_core(body.to_status),
            reason: map_reason_to_core(body.reason),
            note: body.note.as_deref(),
            reference_type: body.reference_type.as_deref(),
            reference_id: body.reference_id,
        },
    )
    .await?;

    Ok(Json(InventoryStatusTransitionResponse {
        inventory_transaction_id: result.inventory_transaction_id,
        source_inventory_balance_id: result.source_inventory_balance_id,
        target_inventory_balance_id: result.target_inventory_balance_id,
        quantity: result.qty,
        from_status: map_status_from_core(result.from_status),
        to_status: map_status_from_core(result.to_status),
    }))
}

fn validate_request(
    balance_id: i64,
    body: &CreateInventoryStatusTransitionRequest,
) -> V1Result<()> {
    require_positive(balance_id, "inventory balance ID")?;
    require_positive(body.quantity, "quantity")?;
    validate_optional_text(body.note.as_deref(), "note", MAX_NOTE_LENGTH)?;
    validate_optional_text(
        body.reference_type.as_deref(),
        "reference_type",
        MAX_REFERENCE_TYPE_LENGTH,
    )?;
    match (&body.reference_type, body.reference_id) {
        (None, None) | (Some(_), Some(1..)) => {}
        _ => {
            return Err(invalid(
                "reference_type and positive reference_id must be provided together",
            ));
        }
    }
    if body.reason == InventoryStatusTransitionReason::Other && body.note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    if !map_reason_to_core(body.reason).allows_target_status(map_status_to_core(body.to_status)) {
        return Err(invalid(
            "reason does not permit the requested target status",
        ));
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, field: &str, maximum: usize) -> V1Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.trim() != value || value.is_empty() {
        return Err(invalid(format!(
            "{field} must be trimmed and nonempty when provided"
        )));
    }
    if value.chars().count() > maximum {
        return Err(invalid(format!(
            "{field} cannot exceed {maximum} characters"
        )));
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

fn map_status_to_core(status: InventoryBalanceStatus) -> CoreInventoryStatus {
    match status {
        InventoryBalanceStatus::Available => CoreInventoryStatus::Available,
        InventoryBalanceStatus::Hold => CoreInventoryStatus::Hold,
        InventoryBalanceStatus::Damaged => CoreInventoryStatus::Damaged,
        InventoryBalanceStatus::Quarantine => CoreInventoryStatus::Quarantine,
    }
}

fn map_status_from_core(status: CoreInventoryStatus) -> InventoryBalanceStatus {
    match status {
        CoreInventoryStatus::Available => InventoryBalanceStatus::Available,
        CoreInventoryStatus::Hold => InventoryBalanceStatus::Hold,
        CoreInventoryStatus::Damaged => InventoryBalanceStatus::Damaged,
        CoreInventoryStatus::Quarantine => InventoryBalanceStatus::Quarantine,
    }
}

fn map_reason_to_core(reason: InventoryStatusTransitionReason) -> CoreInventoryStatusChangeReason {
    match reason {
        InventoryStatusTransitionReason::QualityInspection => {
            CoreInventoryStatusChangeReason::QualityInspection
        }
        InventoryStatusTransitionReason::DamageSuspected => {
            CoreInventoryStatusChangeReason::DamageSuspected
        }
        InventoryStatusTransitionReason::DamageConfirmed => {
            CoreInventoryStatusChangeReason::DamageConfirmed
        }
        InventoryStatusTransitionReason::InspectionPassed => {
            CoreInventoryStatusChangeReason::InspectionPassed
        }
        InventoryStatusTransitionReason::InventoryDiscrepancy => {
            CoreInventoryStatusChangeReason::InventoryDiscrepancy
        }
        InventoryStatusTransitionReason::DiscrepancyResolved => {
            CoreInventoryStatusChangeReason::DiscrepancyResolved
        }
        InventoryStatusTransitionReason::RegulatoryRestriction => {
            CoreInventoryStatusChangeReason::RegulatoryRestriction
        }
        InventoryStatusTransitionReason::RegulatoryRelease => {
            CoreInventoryStatusChangeReason::RegulatoryRelease
        }
        InventoryStatusTransitionReason::CustomerRequest => {
            CoreInventoryStatusChangeReason::CustomerRequest
        }
        InventoryStatusTransitionReason::CustomerRelease => {
            CoreInventoryStatusChangeReason::CustomerRelease
        }
        InventoryStatusTransitionReason::Other => CoreInventoryStatusChangeReason::Other,
    }
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
