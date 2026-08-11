use axum::extract::{Path, Query, State};
use axum::Json;
use sha2::{Digest, Sha256};
use wareboxes_api_contract::v1::{
    CancelTransferOrderRequest, CancelTransferOrderResponse, CreateTransferOrderRequest,
    CreateTransferOrderResponse, CreatedTransferOrderLineResponse, DispatchTransferOrderRequest,
    DispatchTransferOrderResponse, OpaqueCursor, ReceiveTransferOrderRequest,
    ReceiveTransferOrderResponse, ReleaseTransferOrderRequest, ReleaseTransferOrderResponse,
    Revision, TransferDispatchCandidateResponse, TransferDispatchLineResponse,
    TransferExecutionLocationResponse, TransferExecutionReadinessResponse,
    TransferOrderCancellationReason as ApiReason, TransferOrderDetailResponse,
    TransferOrderLineResponse, TransferOrderPage as ApiPage, TransferOrderPageRequest,
    TransferOrderStatus as ApiStatus, TransferOrderSummaryResponse, TransferReceiptLineResponse,
};
use wareboxes_application::transfer_order::{
    CancelTransferOrderCommand, CancelTransferOrderResult, CreateTransferOrderCommand,
    CreateTransferOrderResult, DispatchTransferOrderCommand, DispatchTransferOrderResult,
    ReceiveTransferOrderCommand, ReceiveTransferOrderResult, ReleaseTransferOrderCommand,
    ReleaseTransferOrderResult, TransferExecutionLocationReadModel, TransferExecutionReadiness,
    TransferOrderPageFilter, TransferOrderReadModel,
};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId, LocationId, NewTransferOrder,
    TransferDispatchExecution, TransferDispatchSelection, TransferOrderCancellationDetails,
    TransferOrderCancellationNote, TransferOrderCancellationReason, TransferOrderId,
    TransferOrderLineDefinition, TransferOrderNumber, TransferOrderQuantity, TransferOrderRevision,
    TransferOrderScanValue, TransferOrderStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "to1.";
const MAX_SEARCH_LENGTH: usize = 100;

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateTransferOrderRequest>,
) -> V1Result<Json<CreateTransferOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let order = NewTransferOrder::new(
        InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        FacilityId::new(body.source_facility_id).map_err(validation)?,
        FacilityId::new(body.destination_facility_id).map_err(validation)?,
        TransferOrderNumber::new(body.number).map_err(validation)?,
        body.expected_departure_at
            .map(|value| parse_timestamp(&value, "expected_departure_at"))
            .transpose()?,
        body.expected_arrival_at
            .map(|value| parse_timestamp(&value, "expected_arrival_at"))
            .transpose()?,
        body.lines
            .into_iter()
            .map(|line| {
                Ok(TransferOrderLineDefinition::new(
                    CatalogItemId::new(line.item_id).map_err(validation)?,
                    TransferOrderQuantity::new(line.requested_quantity).map_err(validation)?,
                ))
            })
            .collect::<V1Result<Vec<_>>>()?,
    )
    .map_err(validation)?;
    let result = repo::transfer_order::create(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &CreateTransferOrderCommand { order },
    )
    .await?;
    Ok(Json(map_create(result)?))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(id): Path<i64>,
    Json(body): Json<ReleaseTransferOrderRequest>,
) -> V1Result<Json<ReleaseTransferOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let result = repo::transfer_order::release(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &ReleaseTransferOrderCommand {
            transfer_order_id: TransferOrderId::new(id).map_err(validation)?,
            expected_revision: domain_revision(body.expected_revision)?,
        },
    )
    .await?;
    Ok(Json(map_release(result)?))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(id): Path<i64>,
    Json(body): Json<CancelTransferOrderRequest>,
) -> V1Result<Json<CancelTransferOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let details = TransferOrderCancellationDetails::new(
        map_reason(body.reason),
        body.note
            .map(TransferOrderCancellationNote::new)
            .transpose()
            .map_err(validation)?,
    )
    .map_err(validation)?;
    let command = CancelTransferOrderCommand::new(
        TransferOrderId::new(id).map_err(validation)?,
        domain_revision(body.expected_revision)?,
        details,
    );
    let result = repo::transfer_order::cancel(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_cancel(result)?))
}

pub async fn dispatch(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(id): Path<i64>,
    Json(body): Json<DispatchTransferOrderRequest>,
) -> V1Result<Json<DispatchTransferOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let execution = TransferDispatchExecution::new(
        LocationId::new(body.transit_location_id).map_err(validation)?,
        TransferOrderScanValue::new(body.transit_location_barcode).map_err(validation)?,
        body.lines
            .into_iter()
            .map(|line| {
                Ok(TransferDispatchSelection::new(
                    wareboxes_domain::TransferOrderLineId::new(line.transfer_order_line_id)
                        .map_err(validation)?,
                    InventoryBalanceId::new(line.source_inventory_balance_id)
                        .map_err(validation)?,
                    TransferOrderQuantity::new(line.quantity).map_err(validation)?,
                    TransferOrderScanValue::new(line.source_location_barcode)
                        .map_err(validation)?,
                ))
            })
            .collect::<V1Result<Vec<_>>>()?,
    )
    .map_err(validation)?;
    let result = repo::transfer_order::dispatch(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &DispatchTransferOrderCommand {
            transfer_order_id: TransferOrderId::new(id).map_err(validation)?,
            expected_revision: domain_revision(body.expected_revision)?,
            execution,
        },
    )
    .await?;
    Ok(Json(map_dispatch(result)?))
}

pub async fn receive(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(id): Path<i64>,
    Json(body): Json<ReceiveTransferOrderRequest>,
) -> V1Result<Json<ReceiveTransferOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let result = repo::transfer_order::receive(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &ReceiveTransferOrderCommand {
            transfer_order_id: TransferOrderId::new(id).map_err(validation)?,
            expected_revision: domain_revision(body.expected_revision)?,
            destination_location_id: LocationId::new(body.destination_location_id)
                .map_err(validation)?,
            observed_destination_location_barcode: TransferOrderScanValue::new(
                body.destination_location_barcode,
            )
            .map_err(validation)?,
        },
    )
    .await?;
    Ok(Json(map_receive(result)?))
}

pub async fn execution_readiness(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(id): Path<i64>,
) -> V1Result<Json<TransferExecutionReadinessResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let readiness = repo::transfer_order::execution_readiness(
        &state.db,
        &user.tenant,
        TransferOrderId::new(id).map_err(validation)?,
    )
    .await?
    .ok_or_else(|| V1Error::from(AppError::not_found("transfer order")))?;
    Ok(Json(map_readiness(readiness)?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<TransferOrderPageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let source_facility_id = request
        .source_facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let destination_facility_id = request
        .destination_facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let search = request
        .search
        .as_deref()
        .map(validate_search)
        .transpose()?
        .map(str::to_owned);
    let offset = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?
        .unwrap_or(0);
    let page = repo::transfer_order::page(
        &state.db,
        &user.tenant,
        &TransferOrderPageFilter {
            source_facility_id,
            destination_facility_id,
            inventory_owner_id,
            status: request.status.map(map_status),
            search,
            offset,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_offset
        .map(|next| encode_cursor(next, &request))
        .transpose()?;
    Ok(Json(ApiPage::new(
        page.entries
            .into_iter()
            .map(map_summary)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(id): Path<i64>,
) -> V1Result<Json<TransferOrderDetailResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let detail = repo::transfer_order::detail(
        &state.db,
        &user.tenant,
        TransferOrderId::new(id).map_err(validation)?,
    )
    .await?
    .ok_or_else(|| V1Error::from(AppError::not_found("transfer order")))?;
    let summary = map_summary(detail.clone())?;
    Ok(Json(TransferOrderDetailResponse {
        summary,
        lines: detail
            .lines
            .into_iter()
            .map(|line| TransferOrderLineResponse {
                line_id: line.line_id.get(),
                sequence: line.sequence,
                item_id: line.item_id.get(),
                item_description: line.item_description,
                uom: line.uom,
                requested_quantity: line.requested_quantity,
                dispatched_quantity: line.dispatched_quantity,
                received_quantity: line.received_quantity,
            })
            .collect(),
    }))
}

fn map_create(value: CreateTransferOrderResult) -> V1Result<CreateTransferOrderResponse> {
    Ok(CreateTransferOrderResponse {
        transfer_order_id: value.transfer_order_id.get(),
        number: value.number,
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        lines: value
            .lines
            .into_iter()
            .map(|line| CreatedTransferOrderLineResponse {
                line_id: line.line_id.get(),
                item_id: line.item_id.get(),
                requested_quantity: line.requested_quantity,
            })
            .collect(),
        total_requested_quantity: value.total_requested_quantity,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
    })
}

fn map_release(value: ReleaseTransferOrderResult) -> V1Result<ReleaseTransferOrderResponse> {
    Ok(ReleaseTransferOrderResponse {
        release_id: value.release_id.get(),
        transfer_order_id: value.transfer_order_id.get(),
        previous_status: map_status_to_api(value.previous_status),
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        released_by: value.released_by.get(),
        released_at: value.released_at.to_rfc3339(),
    })
}

fn map_cancel(value: CancelTransferOrderResult) -> V1Result<CancelTransferOrderResponse> {
    Ok(CancelTransferOrderResponse {
        cancellation_id: value.cancellation_id.get(),
        transfer_order_id: value.transfer_order_id.get(),
        previous_status: map_status_to_api(value.previous_status),
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        reason: map_reason_to_api(value.reason),
        note: value.note,
        cancelled_by: value.cancelled_by.get(),
        cancelled_at: value.cancelled_at.to_rfc3339(),
    })
}

fn map_dispatch(value: DispatchTransferOrderResult) -> V1Result<DispatchTransferOrderResponse> {
    Ok(DispatchTransferOrderResponse {
        dispatch_id: value.dispatch_id.get(),
        transfer_order_id: value.transfer_order_id.get(),
        previous_status: map_status_to_api(value.previous_status),
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        transit_location_id: value.transit_location_id.get(),
        transit_location_barcode: value.transit_location_barcode,
        inventory_transaction_id: value.inventory_transaction_id,
        lines: value
            .lines
            .into_iter()
            .map(|line| TransferDispatchLineResponse {
                dispatch_line_id: line.dispatch_line_id.get(),
                transfer_order_line_id: line.transfer_order_line_id.get(),
                source_inventory_balance_id: line.source_inventory_balance_id.get(),
                source_location_id: line.source_location_id.get(),
                transit_inventory_balance_id: line.transit_inventory_balance_id.get(),
                item_batch_id: line.item_batch_id.get(),
                item_id: line.item_id.get(),
                uom: line.uom,
                lot: line.lot,
                expiration: line.expiration.map(|value| value.to_rfc3339()),
                serial: line.serial,
                inventory_status: line.inventory_status,
                quantity: line.quantity,
            })
            .collect(),
        total_dispatched_quantity: value.total_dispatched_quantity,
        dispatched_by: value.dispatched_by.get(),
        dispatched_at: value.dispatched_at.to_rfc3339(),
    })
}

fn map_receive(value: ReceiveTransferOrderResult) -> V1Result<ReceiveTransferOrderResponse> {
    Ok(ReceiveTransferOrderResponse {
        receipt_id: value.receipt_id.get(),
        transfer_order_id: value.transfer_order_id.get(),
        previous_status: map_status_to_api(value.previous_status),
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        destination_location_id: value.destination_location_id.get(),
        destination_location_barcode: value.destination_location_barcode,
        inventory_transaction_id: value.inventory_transaction_id,
        lines: value
            .lines
            .into_iter()
            .map(|line| TransferReceiptLineResponse {
                receipt_line_id: line.receipt_line_id.get(),
                dispatch_line_id: line.dispatch_line_id.get(),
                transfer_order_line_id: line.transfer_order_line_id.get(),
                transit_inventory_balance_id: line.transit_inventory_balance_id.get(),
                destination_inventory_balance_id: line.destination_inventory_balance_id.get(),
                item_batch_id: line.item_batch_id.get(),
                item_id: line.item_id.get(),
                uom: line.uom,
                lot: line.lot,
                expiration: line.expiration.map(|value| value.to_rfc3339()),
                serial: line.serial,
                inventory_status: line.inventory_status,
                quantity: line.quantity,
            })
            .collect(),
        total_received_quantity: value.total_received_quantity,
        received_by: value.received_by.get(),
        received_at: value.received_at.to_rfc3339(),
    })
}

fn map_readiness(
    value: TransferExecutionReadiness,
) -> V1Result<TransferExecutionReadinessResponse> {
    Ok(TransferExecutionReadinessResponse {
        transfer_order_id: value.transfer_order_id.get(),
        revision: api_revision(value.revision)?,
        status: map_status_to_api(value.status),
        dispatch_candidates: value
            .dispatch_candidates
            .into_iter()
            .map(|candidate| TransferDispatchCandidateResponse {
                transfer_order_line_id: candidate.transfer_order_line_id.get(),
                source_inventory_balance_id: candidate.source_inventory_balance_id.get(),
                source_location_id: candidate.source_location_id.get(),
                source_location_barcode: candidate.source_location_barcode,
                source_location_name: candidate.source_location_name,
                item_batch_id: candidate.item_batch_id.get(),
                item_id: candidate.item_id.get(),
                item_description: candidate.item_description,
                uom: candidate.uom,
                lot: candidate.lot,
                expiration: candidate.expiration.map(|value| value.to_rfc3339()),
                serial: candidate.serial,
                free_quantity: candidate.free_quantity,
            })
            .collect(),
        transit_locations: value
            .transit_locations
            .into_iter()
            .map(map_execution_location)
            .collect(),
        receiving_locations: value
            .receiving_locations
            .into_iter()
            .map(map_execution_location)
            .collect(),
    })
}

fn map_execution_location(
    value: TransferExecutionLocationReadModel,
) -> TransferExecutionLocationResponse {
    TransferExecutionLocationResponse {
        location_id: value.location_id.get(),
        barcode: value.barcode,
        name: value.name,
    }
}

fn map_summary(value: TransferOrderReadModel) -> V1Result<TransferOrderSummaryResponse> {
    Ok(TransferOrderSummaryResponse {
        transfer_order_id: value.transfer_order_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        source_facility_id: value.source_facility_id.get(),
        source_facility_name: value.source_facility_name,
        destination_facility_id: value.destination_facility_id.get(),
        destination_facility_name: value.destination_facility_name,
        number: value.number,
        expected_departure_at: value.expected_departure_at.map(|value| value.to_rfc3339()),
        expected_arrival_at: value.expected_arrival_at.map(|value| value.to_rfc3339()),
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        line_count: value.line_count,
        total_requested_quantity: value.total_requested_quantity,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        released_by: value.released_by.map(wareboxes_domain::UserId::get),
        released_at: value.released_at.map(|value| value.to_rfc3339()),
        cancellation_id: value
            .cancellation_id
            .map(wareboxes_domain::TransferOrderCancellationId::get),
        cancellation_reason: value.cancellation_reason.map(map_reason_to_api),
        cancellation_note: value.cancellation_note,
        cancelled_by: value.cancelled_by.map(wareboxes_domain::UserId::get),
        cancelled_at: value.cancelled_at.map(|value| value.to_rfc3339()),
        dispatch_id: value
            .dispatch_id
            .map(wareboxes_domain::TransferOrderDispatchId::get),
        dispatch_inventory_transaction_id: value.dispatch_inventory_transaction_id,
        transit_location_id: value.transit_location_id.map(LocationId::get),
        transit_location_barcode: value.transit_location_barcode,
        dispatched_by: value.dispatched_by.map(wareboxes_domain::UserId::get),
        dispatched_at: value.dispatched_at.map(|value| value.to_rfc3339()),
        receipt_id: value
            .receipt_id
            .map(wareboxes_domain::TransferOrderReceiptId::get),
        receipt_inventory_transaction_id: value.receipt_inventory_transaction_id,
        destination_receiving_location_id: value
            .destination_receiving_location_id
            .map(LocationId::get),
        destination_receiving_location_barcode: value.destination_receiving_location_barcode,
        received_by: value.received_by.map(wareboxes_domain::UserId::get),
        received_at: value.received_at.map(|value| value.to_rfc3339()),
    })
}

fn map_status(value: ApiStatus) -> TransferOrderStatus {
    match value {
        ApiStatus::Draft => TransferOrderStatus::Draft,
        ApiStatus::Released => TransferOrderStatus::Released,
        ApiStatus::InTransit => TransferOrderStatus::InTransit,
        ApiStatus::Received => TransferOrderStatus::Received,
        ApiStatus::Cancelled => TransferOrderStatus::Cancelled,
    }
}
fn map_status_to_api(value: TransferOrderStatus) -> ApiStatus {
    match value {
        TransferOrderStatus::Draft => ApiStatus::Draft,
        TransferOrderStatus::Released => ApiStatus::Released,
        TransferOrderStatus::InTransit => ApiStatus::InTransit,
        TransferOrderStatus::Received => ApiStatus::Received,
        TransferOrderStatus::Cancelled => ApiStatus::Cancelled,
    }
}
fn map_reason(value: ApiReason) -> TransferOrderCancellationReason {
    match value {
        ApiReason::DemandCancelled => TransferOrderCancellationReason::DemandCancelled,
        ApiReason::DuplicateOrder => TransferOrderCancellationReason::DuplicateOrder,
        ApiReason::RouteCancelled => TransferOrderCancellationReason::RouteCancelled,
        ApiReason::Other => TransferOrderCancellationReason::Other,
    }
}
fn map_reason_to_api(value: TransferOrderCancellationReason) -> ApiReason {
    match value {
        TransferOrderCancellationReason::DemandCancelled => ApiReason::DemandCancelled,
        TransferOrderCancellationReason::DuplicateOrder => ApiReason::DuplicateOrder,
        TransferOrderCancellationReason::RouteCancelled => ApiReason::RouteCancelled,
        TransferOrderCancellationReason::Other => ApiReason::Other,
    }
}
fn api_revision(value: TransferOrderRevision) -> V1Result<Revision> {
    Revision::new(value.get()).map_err(|error| V1Error::internal(error.to_string()))
}
fn domain_revision(value: Revision) -> V1Result<TransferOrderRevision> {
    TransferOrderRevision::new(value.get()).map_err(validation)
}
fn parse_timestamp(value: &str, field: &str) -> V1Result<wareboxes_domain::Timestamp> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&chrono::Utc))
        .map_err(|_| AppError::bad_request(format!("{field} must be an RFC 3339 timestamp")).into())
}
fn validate_search(value: &str) -> V1Result<&str> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > MAX_SEARCH_LENGTH
        || value.chars().any(char::is_control)
    {
        Err(AppError::bad_request(
            "search must be trimmed, control-free, and at most 100 characters",
        )
        .into())
    } else {
        Ok(value)
    }
}
fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn encode_cursor(offset: u64, request: &TransferOrderPageRequest) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{offset:016x}",
        cursor_fingerprint(request)
    ))
    .map_err(|error| V1Error::internal(error.to_string()))
}
fn decode_cursor(cursor: &OpaqueCursor, request: &TransferOrderPageRequest) -> V1Result<u64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("transfer order"))?;
    let (fingerprint, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("transfer order"))?;
    if fingerprint != cursor_fingerprint(request) || offset.len() != 16 {
        return Err(V1Error::invalid_cursor_for("transfer order"));
    }
    u64::from_str_radix(offset, 16).map_err(|_| V1Error::invalid_cursor_for("transfer order"))
}
fn cursor_fingerprint(request: &TransferOrderPageRequest) -> String {
    let raw = format!(
        "{}|{}|{}|{}|{}|{}",
        request
            .source_facility_id
            .map_or_else(String::new, |id| id.to_string()),
        request
            .destination_facility_id
            .map_or_else(String::new, |id| id.to_string()),
        request
            .inventory_owner_id
            .map_or_else(String::new, |id| id.to_string()),
        request.status.map_or("", |status| match status {
            ApiStatus::Draft => "draft",
            ApiStatus::Released => "released",
            ApiStatus::InTransit => "in_transit",
            ApiStatus::Received => "received",
            ApiStatus::Cancelled => "cancelled",
        }),
        request.search.as_deref().unwrap_or_default(),
        request.limit.get()
    );
    hex::encode(&Sha256::digest(raw.as_bytes())[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_is_bound_to_both_facilities() {
        let request = TransferOrderPageRequest {
            source_facility_id: Some(2),
            destination_facility_id: Some(3),
            inventory_owner_id: Some(4),
            status: Some(ApiStatus::Released),
            search: None,
            cursor: None,
            limit: wareboxes_api_contract::v1::PageLimit::new(20).unwrap(),
        };
        let cursor = encode_cursor(20, &request).unwrap();
        assert_eq!(decode_cursor(&cursor, &request).unwrap(), 20);
        let mut changed = request;
        changed.destination_facility_id = Some(5);
        assert!(decode_cursor(&cursor, &changed).is_err());
    }
}
