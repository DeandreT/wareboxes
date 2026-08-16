use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    DynamicReleaseCandidateResponse, DynamicReleaseReadinessRequest,
    DynamicReleaseReadinessResponse, DynamicReleaseRunResponse, Revision, RunDynamicReleaseRequest,
};
use wareboxes_application::dynamic_release::{
    DynamicReleaseCandidateReadModel, DynamicReleaseCommand, DynamicReleaseReadinessQuery,
    DynamicReleaseReadinessReadModel, DynamicReleaseRunReadModel,
};
use wareboxes_domain::LocationId;

use super::error::V1Result;
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const READ_PERMISSION: &str = "orders";
const MUTATE_PERMISSION: &str = "wms_supervisor";

pub async fn readiness(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<DynamicReleaseReadinessRequest>,
) -> V1Result<Json<DynamicReleaseReadinessResponse>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let query = DynamicReleaseReadinessQuery {
        facility_id: user.require_facility(query.facility_id)?,
        inventory_owner_id: user.require_inventory_owner(query.inventory_owner_id)?,
    };
    let result = repo::dynamic_release::readiness(&state.db, &user.tenant, &query).await?;
    Ok(Json(map_readiness(result)?))
}

pub async fn run(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RunDynamicReleaseRequest>,
) -> V1Result<Json<DynamicReleaseRunResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let command = DynamicReleaseCommand {
        facility_id: user.require_facility(body.facility_id)?,
        inventory_owner_id: user.require_inventory_owner(body.inventory_owner_id)?,
        destination_location_id: LocationId::new(body.destination_location_id)
            .map_err(domain_validation)?,
        expected_policy: super::pick_waves::map_policy_expectation(body.expected_policy)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::dynamic_release::run(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_run(result)?))
}

fn map_readiness(
    result: DynamicReleaseReadinessReadModel,
) -> V1Result<DynamicReleaseReadinessResponse> {
    if !result.is_consistent() {
        return Err(AppError::internal("dynamic release readiness is inconsistent").into());
    }
    Ok(DynamicReleaseReadinessResponse {
        facility_id: result.facility_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        input_snapshot_at: result.input_snapshot_at.to_rfc3339(),
        policy: super::pick_waves::map_wave_policy(result.policy),
        eligible_order_count: result.eligible_order_count,
        selected_order_count: result.selected_order_count,
        deferred_order_count: result.deferred_order_count,
        selected_orders: result
            .selected_orders
            .into_iter()
            .map(map_candidate)
            .collect::<V1Result<Vec<_>>>()?,
    })
}

fn map_run(result: DynamicReleaseRunReadModel) -> V1Result<DynamicReleaseRunResponse> {
    if !result.is_consistent() {
        return Err(AppError::internal("dynamic release result is inconsistent").into());
    }
    Ok(DynamicReleaseRunResponse {
        run_id: result.run_id.get(),
        facility_id: result.facility_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        destination_location_id: result.destination_location_id.get(),
        input_snapshot_at: result.input_snapshot_at.to_rfc3339(),
        policy: super::pick_waves::map_wave_policy(result.policy),
        eligible_order_count: result.eligible_order_count,
        selected_order_count: result.selected_order_count,
        deferred_order_count: result.deferred_order_count,
        selected_orders: result
            .selected_orders
            .into_iter()
            .map(map_candidate)
            .collect::<V1Result<Vec<_>>>()?,
        wave: result.wave.map(super::pick_waves::map_wave).transpose()?,
        released_by: result.released_by.get(),
        released_at: result.released_at.to_rfc3339(),
    })
}

fn map_candidate(
    candidate: DynamicReleaseCandidateReadModel,
) -> V1Result<DynamicReleaseCandidateResponse> {
    Ok(DynamicReleaseCandidateResponse {
        order_id: candidate.order_id.get(),
        order_key: candidate.order_key,
        revision: Revision::new(candidate.revision.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        rank: candidate.rank,
        rush: candidate.rush,
        ship_by: candidate.ship_by.map(|value| value.to_rfc3339()),
        order_created_at: candidate.order_created_at.to_rfc3339(),
        demand_quantity: candidate.demand_quantity,
        allocated_quantity: candidate.allocated_quantity,
    })
}

fn domain_validation(error: impl std::fmt::Display) -> AppError {
    AppError::bad_request(error.to_string())
}
