use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    PlanOrderAllocationRequest, StreamOrderRequest, StreamOrderResponse,
};
use wareboxes_application::order_stream::StreamOrderCommand;
use wareboxes_domain::LocationId;

use super::error::V1Result;
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<StreamOrderRequest>,
) -> V1Result<Json<StreamOrderResponse>> {
    user.require_permission(&state.db, "orders").await?;
    let allocation = super::order_allocations::plan_command(
        order_id,
        PlanOrderAllocationRequest {
            facility_id: body.facility_id,
            expected_revision: body.expected_revision,
            expected_policy: body.expected_allocation_policy,
        },
    )?;
    let command = StreamOrderCommand {
        order_id: allocation.order_id,
        facility_id: allocation.facility_id,
        destination_location_id: LocationId::new(body.destination_location_id)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        expected_revision: allocation.expected_revision,
        expected_allocation_policy: allocation.expected_policy,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::order_stream::stream_order(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(StreamOrderResponse {
        allocation: super::order_allocations::map_plan_result(result.allocation)?,
        release: super::order_releases::map_result(result.release)?,
    }))
}
