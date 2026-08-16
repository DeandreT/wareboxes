use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ClaimNextZonePickRequest, CurrentPickResponse, PickZoneQueueResponse, PickZoneWorkspaceRequest,
    PickZoneWorkspaceResponse,
};
use wareboxes_application::pick_zone::{ClaimNextZonePickCommand, PickZoneWorkspaceQuery};
use wareboxes_domain::{FacilityId, InventoryOwnerId, StorageZoneId};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<PickZoneWorkspaceRequest>,
) -> V1Result<Json<PickZoneWorkspaceResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let query = PickZoneWorkspaceQuery {
        inventory_owner_id: InventoryOwnerId::new(request.inventory_owner_id)
            .map_err(domain_validation)?,
        facility_id: FacilityId::new(request.facility_id).map_err(domain_validation)?,
    };
    let result = repo::picking::zone_workspace(&state.db, &user.tenant, query).await?;
    Ok(Json(PickZoneWorkspaceResponse {
        queues: result
            .queues
            .into_iter()
            .map(|queue| PickZoneQueueResponse {
                storage_zone_id: queue.storage_zone_id.get(),
                code: queue.code,
                name: queue.name,
                revision: queue.revision.get(),
                travel_sequence: queue.travel_sequence.get(),
                open_task_count: queue.open_task_count,
                active_task_count: queue.active_task_count,
                oldest_open_task_at: queue.oldest_open_task_at.map(|time| time.to_rfc3339()),
            })
            .collect(),
    }))
}

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(storage_zone_id): Path<i64>,
    Json(_body): Json<ClaimNextZonePickRequest>,
) -> V1Result<Json<CurrentPickResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = ClaimNextZonePickCommand {
        storage_zone_id: StorageZoneId::new(storage_zone_id).map_err(domain_validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::claim_next_zone(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(result.map(super::picking::map_claim).transpose()?))
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}
