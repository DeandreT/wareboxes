use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CancelOrderRequest, CancelOrderResponse, OrderCancellationReason as ContractReason,
    OrderCancellationStatus, Revision,
};
use wareboxes_application::order_cancellation::{CancelOrderCommand, CancelOrderResult};
use wareboxes_domain::{
    CancellationNote, OrderCancellationReason, OrderId, OrderRevision, OrderStatus,
};

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
    Json(body): Json<CancelOrderRequest>,
) -> V1Result<Json<CancelOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = command(order_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::order_cancellation::cancel_order(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(response(result)?))
}

fn command(order_id: i64, request: CancelOrderRequest) -> V1Result<CancelOrderCommand> {
    let note = request
        .note
        .map(CancellationNote::new)
        .transpose()
        .map_err(invalid)?;
    CancelOrderCommand::new(
        OrderId::new(order_id).map_err(invalid)?,
        OrderRevision::new(request.expected_revision.get()).map_err(invalid)?,
        reason_to_domain(request.reason),
        note,
    )
    .map_err(invalid)
}

fn response(result: CancelOrderResult) -> V1Result<CancelOrderResponse> {
    Ok(CancelOrderResponse {
        cancellation_id: result.cancellation_id.get(),
        order_id: result.order_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        previous_status: status(result.previous_status)?,
        status: status(result.status)?,
        revision: Revision::new(result.revision.get())
            .map_err(|_| V1Error::internal("order cancellation produced an invalid revision"))?,
        reason: reason_to_contract(result.reason),
        note: result.note.map(|note| note.as_str().to_owned()),
        released_hold_count: result.released_hold_count,
        released_reservation_count: result.released_reservation_count,
        released_allocation_count: result.released_allocation_count,
        released_quantity: result.released_quantity,
    })
}

const fn reason_to_domain(reason: ContractReason) -> OrderCancellationReason {
    match reason {
        ContractReason::ClientRequest => OrderCancellationReason::ClientRequest,
        ContractReason::DuplicateOrder => OrderCancellationReason::DuplicateOrder,
        ContractReason::DataCorrection => OrderCancellationReason::DataCorrection,
        ContractReason::InventoryUnavailable => OrderCancellationReason::InventoryUnavailable,
        ContractReason::FulfillmentException => OrderCancellationReason::FulfillmentException,
        ContractReason::Other => OrderCancellationReason::Other,
    }
}

const fn reason_to_contract(reason: OrderCancellationReason) -> ContractReason {
    match reason {
        OrderCancellationReason::ClientRequest => ContractReason::ClientRequest,
        OrderCancellationReason::DuplicateOrder => ContractReason::DuplicateOrder,
        OrderCancellationReason::DataCorrection => ContractReason::DataCorrection,
        OrderCancellationReason::InventoryUnavailable => ContractReason::InventoryUnavailable,
        OrderCancellationReason::FulfillmentException => ContractReason::FulfillmentException,
        OrderCancellationReason::Other => ContractReason::Other,
    }
}

fn status(status: OrderStatus) -> V1Result<OrderCancellationStatus> {
    match status {
        OrderStatus::Cancelled => Ok(OrderCancellationStatus::Cancelled),
        OrderStatus::Held => Ok(OrderCancellationStatus::Held),
        OrderStatus::Open => Ok(OrderCancellationStatus::Open),
        _ => Err(V1Error::internal(
            "order cancellation produced an invalid workflow status",
        )),
    }
}

fn invalid(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_a_validated_domain_command() {
        let command = command(
            17,
            CancelOrderRequest {
                expected_revision: Revision::new(3).unwrap(),
                reason: ContractReason::Other,
                note: Some("Client supplied cancellation context".into()),
            },
        )
        .unwrap();

        assert_eq!(command.order_id().get(), 17);
        assert_eq!(command.expected_revision().get(), 3);
        assert_eq!(command.reason(), OrderCancellationReason::Other);
    }

    #[test]
    fn rejects_other_without_context() {
        let result = command(
            17,
            CancelOrderRequest {
                expected_revision: Revision::new(3).unwrap(),
                reason: ContractReason::Other,
                note: None,
            },
        );
        assert!(result.is_err());
    }
}
