use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ClaimNextPutawayRequest, ClaimPutawayByIdRequest, InventoryBalanceStatus,
    PutawayClaimDestinationLocation, PutawayClaimResponse, PutawayClaimSourceLocation,
    PutawayClaimWork as ContractPutawayClaimWork, PutawayWorkflow,
};
use wareboxes_core::models::{
    InventoryStatus, PutawayClaim, PutawayClaimWork as CorePutawayClaimWork, WorkTaskType,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ClaimNextPutawayRequest>,
) -> V1Result<Json<Option<PutawayClaimResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let context = user.command_context(&idempotency_key);
    let task_type = match body.workflow {
        PutawayWorkflow::Loose => WorkTaskType::Putaway,
        PutawayWorkflow::LicensePlate => WorkTaskType::LicensePlatePutaway,
    };
    let claim =
        repo::tasks::claim_next_putaway_in_scope(&state.db, &user.tenant, &context, task_type)
            .await?;

    let response = match claim {
        Some(claim) => Some(map_claim(&state, &user, claim).await?),
        None => None,
    };
    Ok(Json(response))
}

pub async fn claim_by_id(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<ClaimPutawayByIdRequest>,
) -> V1Result<Json<PutawayClaimResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let context = user.command_context(&idempotency_key);
    let claim =
        repo::tasks::claim_putaway_in_scope(&state.db, &user.tenant, &context, task_id).await?;

    Ok(Json(map_claim(&state, &user, claim).await?))
}

pub async fn current(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<Option<PutawayClaimResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let claim = repo::tasks::current_putaway_claim_in_scope(&state.db, &user.tenant).await?;

    let response = match claim {
        Some(claim) => Some(map_claim(&state, &user, claim).await?),
        None => None,
    };
    Ok(Json(response))
}

async fn map_claim(
    state: &AppState,
    user: &CurrentTenant,
    claim: PutawayClaim,
) -> V1Result<PutawayClaimResponse> {
    let putaway_policy =
        repo::putaway_policy::load_task_policy(&state.db, &user.tenant, claim.task_id).await?;
    Ok(PutawayClaimResponse {
        task_id: claim.task_id,
        inventory_owner_id: claim.inventory_owner_id.get(),
        facility_id: claim.facility_id,
        priority: claim.priority,
        instructions: claim.instructions,
        due_at: claim.due_at.map(|timestamp| timestamp.to_rfc3339()),
        lease_expires_at: claim.lease_expires_at.to_rfc3339(),
        source_location: PutawayClaimSourceLocation {
            location_id: claim.source_location.location_id,
            barcode: claim.source_location.barcode,
            name: claim.source_location.name,
        },
        destination_location: PutawayClaimDestinationLocation {
            location_id: claim.destination_location.location_id,
            barcode: claim.destination_location.barcode,
            name: claim.destination_location.name,
        },
        putaway_policy: super::putaway::map_policy(putaway_policy),
        work: match claim.work {
            CorePutawayClaimWork::Loose {
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
            } => ContractPutawayClaimWork::Loose {
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
            CorePutawayClaimWork::LicensePlate {
                license_plate_id,
                license_plate_barcode,
                planned_balance_count,
            } => ContractPutawayClaimWork::LicensePlate {
                license_plate_id,
                license_plate_barcode,
                planned_balance_count,
            },
        },
    })
}

fn map_inventory_status(status: InventoryStatus) -> InventoryBalanceStatus {
    match status {
        InventoryStatus::Available => InventoryBalanceStatus::Available,
        InventoryStatus::Hold => InventoryBalanceStatus::Hold,
        InventoryStatus::Damaged => InventoryBalanceStatus::Damaged,
        InventoryStatus::Quarantine => InventoryBalanceStatus::Quarantine,
    }
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
