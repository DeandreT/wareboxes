use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    DisposeInboundInspectionRequest, DisposeInboundInspectionResponse,
    InboundInspectionOutcome as ApiOutcome, InventoryBalanceStatus,
};
use wareboxes_application::inbound_inspection::DisposeInboundInspectionCommand;
use wareboxes_domain::{InboundInspectionNote, InboundInspectionOutcome, InventoryHoldId};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms_supervisor";

pub async fn dispose(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(hold_id): Path<i64>,
    Json(body): Json<DisposeInboundInspectionRequest>,
) -> V1Result<Json<DisposeInboundInspectionResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let hold_id =
        InventoryHoldId::new(hold_id).map_err(|_| invalid("inventory hold ID must be positive"))?;
    let note = InboundInspectionNote::new(body.note).map_err(|error| invalid(error.to_string()))?;
    let outcome = match body.outcome {
        ApiOutcome::Approved => InboundInspectionOutcome::Approved,
        ApiOutcome::Damaged => InboundInspectionOutcome::Damaged,
    };
    let result = repo::inbound_inspection::dispose(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &DisposeInboundInspectionCommand {
            inventory_hold_id: hold_id,
            outcome,
            note,
        },
    )
    .await?;
    Ok(Json(DisposeInboundInspectionResponse {
        disposition_id: result.disposition_id.get(),
        inventory_hold_id: result.inventory_hold_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        facility_id: result.facility_id.get(),
        source_inventory_balance_id: result.source_inventory_balance_id.get(),
        target_inventory_balance_id: result.target_inventory_balance_id.get(),
        location_id: result.location_id.get(),
        license_plate_id: result.license_plate_id,
        item_batch_id: result.item_batch_id.get(),
        item_id: result.item_id,
        uom: result.uom,
        quantity: result.quantity,
        outcome: match result.outcome {
            InboundInspectionOutcome::Approved => ApiOutcome::Approved,
            InboundInspectionOutcome::Damaged => ApiOutcome::Damaged,
        },
        target_status: match result.target_status {
            wareboxes_domain::InboundInspectionTargetStatus::Available => {
                InventoryBalanceStatus::Available
            }
            wareboxes_domain::InboundInspectionTargetStatus::Damaged => {
                InventoryBalanceStatus::Damaged
            }
        },
        note: result.note.as_str().to_owned(),
        inventory_transaction_id: result.inventory_transaction_id,
        inspected_by_user_id: result.inspected_by.get(),
        inspected_at: result.inspected_at.to_rfc3339(),
    }))
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
