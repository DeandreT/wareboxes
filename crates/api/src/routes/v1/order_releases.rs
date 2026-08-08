use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    OrderReleaseStatus, ReleaseOrderRequest, ReleaseOrderResponse, Revision,
};
use wareboxes_application::order_release::{ReleaseOrderCommand, ReleaseOrderResult};
use wareboxes_domain::{FacilityId, LocationId, OrderId, OrderRevision, OrderStatus};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "orders";

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<ReleaseOrderRequest>,
) -> V1Result<Json<ReleaseOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = release_command(order_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::order_release::release_order(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_result(result)?))
}

fn release_command(order_id: i64, body: ReleaseOrderRequest) -> V1Result<ReleaseOrderCommand> {
    Ok(ReleaseOrderCommand {
        order_id: OrderId::new(order_id).map_err(domain_validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(domain_validation)?,
        destination_location_id: LocationId::new(body.destination_location_id)
            .map_err(domain_validation)?,
        expected_revision: OrderRevision::new(body.expected_revision.get())
            .map_err(domain_validation)?,
    })
}

fn map_result(result: ReleaseOrderResult) -> V1Result<ReleaseOrderResponse> {
    if !result.is_consistent() {
        return Err(V1Error::internal(
            "order release produced inconsistent work totals",
        ));
    }
    let status = match result.status {
        OrderStatus::Processing => OrderReleaseStatus::Processing,
        _ => {
            return Err(V1Error::internal(
                "order release produced an invalid status",
            ))
        }
    };
    Ok(ReleaseOrderResponse {
        release_id: result.release_id.get(),
        order_id: result.order_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        facility_id: result.facility_id.get(),
        destination_location_id: result.destination_location_id.get(),
        status,
        revision: Revision::new(result.revision.get())
            .map_err(|error| V1Error::internal(error.to_string()))?,
        allocation_count: result.allocation_count,
        pick_task_count: result.pick_task_count,
        released_quantity: result.released_quantity,
        released_at: result.released_at.to_rfc3339(),
    })
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_mapping_uses_path_order_and_positive_dimensions() {
        let command = release_command(
            7,
            ReleaseOrderRequest {
                facility_id: 8,
                destination_location_id: 9,
                expected_revision: Revision::new(2).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(command.order_id.get(), 7);
        assert_eq!(command.facility_id.get(), 8);
        assert_eq!(command.destination_location_id.get(), 9);
        assert!(release_command(
            0,
            ReleaseOrderRequest {
                facility_id: 8,
                destination_location_id: 9,
                expected_revision: Revision::new(2).unwrap(),
            }
        )
        .is_err());
    }
}
