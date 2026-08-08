use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    OrderHoldOrderStatus, OrderHoldReason, PlaceOrderHoldRequest, PlaceOrderHoldResponse,
    ReleaseOrderHoldRequest, ReleaseOrderHoldResponse,
};
use wareboxes_core::models::{OrderHoldReason as DomainOrderHoldReason, OrderStatus};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "orders";
const MAX_NOTE_LENGTH: usize = 1_000;

pub async fn place(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<PlaceOrderHoldRequest>,
) -> V1Result<Json<PlaceOrderHoldResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(order_id, "order ID")?;
    validate_note(body.note.as_deref(), body.reason == OrderHoldReason::Other)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::orders::place_order_hold(
        &state.db,
        &user.tenant,
        &context,
        order_id,
        map_reason(body.reason),
        body.note.as_deref(),
    )
    .await?;
    Ok(Json(PlaceOrderHoldResponse {
        order_id: result.order_id,
        hold_id: result.hold_id,
        order_status: map_status(result.order_status)?,
        active_hold_count: result.active_hold_count,
    }))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((order_id, hold_id)): Path<(i64, i64)>,
    Json(body): Json<ReleaseOrderHoldRequest>,
) -> V1Result<Json<ReleaseOrderHoldResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(order_id, "order ID")?;
    require_positive(hold_id, "order hold ID")?;
    validate_note(body.note.as_deref(), false)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::orders::release_order_hold(
        &state.db,
        &user.tenant,
        &context,
        order_id,
        hold_id,
        body.note.as_deref(),
    )
    .await?;
    Ok(Json(ReleaseOrderHoldResponse {
        order_id: result.order_id,
        hold_id: result.hold_id,
        order_status: map_status(result.order_status)?,
        active_hold_count: result.active_hold_count,
    }))
}

fn validate_note(note: Option<&str>, required: bool) -> V1Result<()> {
    if required && note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    let Some(note) = note else {
        return Ok(());
    };
    if note.trim() != note || note.is_empty() {
        return Err(invalid("note must be trimmed and nonempty when provided"));
    }
    if note.chars().count() > MAX_NOTE_LENGTH {
        return Err(invalid(format!(
            "note cannot exceed {MAX_NOTE_LENGTH} characters"
        )));
    }
    Ok(())
}

fn map_reason(reason: OrderHoldReason) -> DomainOrderHoldReason {
    match reason {
        OrderHoldReason::AddressReview => DomainOrderHoldReason::AddressReview,
        OrderHoldReason::ComplianceReview => DomainOrderHoldReason::ComplianceReview,
        OrderHoldReason::CustomerRequest => DomainOrderHoldReason::CustomerRequest,
        OrderHoldReason::InventoryShortage => DomainOrderHoldReason::InventoryShortage,
        OrderHoldReason::PaymentReview => DomainOrderHoldReason::PaymentReview,
        OrderHoldReason::Other => DomainOrderHoldReason::Other,
    }
}

fn map_status(status: OrderStatus) -> V1Result<OrderHoldOrderStatus> {
    match status {
        OrderStatus::Held => Ok(OrderHoldOrderStatus::Held),
        OrderStatus::Open => Ok(OrderHoldOrderStatus::Open),
        _ => Err(V1Error::internal(
            "order hold command produced an invalid order status",
        )),
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
