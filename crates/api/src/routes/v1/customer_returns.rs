use axum::extract::{Path, Query, State};
use axum::Json;
use sha2::{Digest, Sha256};
use wareboxes_api_contract::v1::{
    CancelCustomerReturnRequest, CancelCustomerReturnResponse, CreateCustomerReturnRequest,
    CreateCustomerReturnResponse, CreatedCustomerReturnLineResponse,
    CustomerReturnCancellationReason as ApiCancellationReason, CustomerReturnDetailResponse,
    CustomerReturnExecutionStatus as ApiExecutionStatus, CustomerReturnLineResponse,
    CustomerReturnPage as ApiPage, CustomerReturnPageRequest, CustomerReturnReason as ApiReason,
    CustomerReturnStatus as ApiStatus, CustomerReturnSummaryResponse, OpaqueCursor,
    PlanCustomerReturnLoadRequest, PlanCustomerReturnLoadResponse,
    PlannedCustomerReturnLoadLineResponse, Revision,
};
use wareboxes_application::customer_return::{
    CancelCustomerReturnCommand, CancelCustomerReturnResult, CreateCustomerReturnCommand,
    CreateCustomerReturnResult, CustomerReturnExecutionStatus, CustomerReturnPageFilter,
    CustomerReturnReadModel, PlanCustomerReturnLoadCommand, PlanCustomerReturnLoadResult,
};
use wareboxes_domain::{
    CatalogItemId, CustomerReturnCancellationDetails, CustomerReturnCancellationReason,
    CustomerReturnId, CustomerReturnLineDefinition, CustomerReturnLoadPlanDetails,
    CustomerReturnNumber, CustomerReturnQuantity, CustomerReturnReason, CustomerReturnReference,
    CustomerReturnRevision, CustomerReturnStatus, FacilityId, InventoryOwnerId, NewCustomerReturn,
    Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "cr1.";
const MAX_SEARCH_LENGTH: usize = 100;

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateCustomerReturnRequest>,
) -> V1Result<Json<CreateCustomerReturnResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let authorization = NewCustomerReturn::new(
        InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        FacilityId::new(body.facility_id).map_err(validation)?,
        CustomerReturnNumber::new(body.number).map_err(validation)?,
        CustomerReturnReference::new(body.customer_reference).map_err(validation)?,
        body.expected_at
            .map(|value| parse_timestamp(&value, "expected_at"))
            .transpose()?,
        body.lines
            .into_iter()
            .map(|line| {
                CustomerReturnLineDefinition::new(
                    CatalogItemId::new(line.item_id).map_err(validation)?,
                    CustomerReturnQuantity::new(line.authorized_quantity).map_err(validation)?,
                    reason_to_domain(line.reason),
                    line.note,
                    line.lot,
                    line.serial,
                )
                .map_err(validation)
            })
            .collect::<V1Result<Vec<_>>>()?,
    )
    .map_err(validation)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::customer_return::create(
        &state.db,
        &user.tenant,
        &context,
        &CreateCustomerReturnCommand { authorization },
    )
    .await?;
    Ok(Json(map_create(result)?))
}

pub async fn plan_load(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(customer_return_id): Path<i64>,
    Json(body): Json<PlanCustomerReturnLoadRequest>,
) -> V1Result<Json<PlanCustomerReturnLoadResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = PlanCustomerReturnLoadCommand {
        customer_return_id: CustomerReturnId::new(customer_return_id).map_err(validation)?,
        expected_revision: CustomerReturnRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        details: CustomerReturnLoadPlanDetails::new(
            wareboxes_domain::LocationId::new(body.receiving_location_id).map_err(validation)?,
            body.carrier,
            body.trailer_number,
            body.seal_number,
        )
        .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::customer_return::plan_load(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_plan(result)?))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(customer_return_id): Path<i64>,
    Json(body): Json<CancelCustomerReturnRequest>,
) -> V1Result<Json<CancelCustomerReturnResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let details = CustomerReturnCancellationDetails::new(
        cancellation_reason_to_domain(body.reason),
        body.note,
    )
    .map_err(validation)?;
    let command = CancelCustomerReturnCommand::new(
        CustomerReturnId::new(customer_return_id).map_err(validation)?,
        CustomerReturnRevision::new(body.expected_revision.get()).map_err(validation)?,
        details,
    );
    let context = user.command_context(&idempotency_key);
    let result = repo::customer_return::cancel(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_cancel(result)?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<CustomerReturnPageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let facility_id = request
        .facility_id
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
    let page = repo::customer_return::page(
        &state.db,
        &user.tenant,
        &CustomerReturnPageFilter {
            facility_id,
            inventory_owner_id,
            status: request.status.map(status_to_domain),
            search,
            offset,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(offset, &request))
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
    Path(customer_return_id): Path<i64>,
) -> V1Result<Json<CustomerReturnDetailResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let detail = repo::customer_return::detail(
        &state.db,
        &user.tenant,
        CustomerReturnId::new(customer_return_id).map_err(validation)?,
    )
    .await?
    .ok_or_else(|| V1Error::from(AppError::not_found("customer return")))?;
    let summary = map_summary(detail.clone())?;
    Ok(Json(CustomerReturnDetailResponse {
        summary,
        lines: detail
            .lines
            .into_iter()
            .map(|line| CustomerReturnLineResponse {
                line_id: line.line_id.get(),
                sequence: line.sequence,
                item_id: line.item_id.get(),
                item_description: line.item_description,
                uom: line.uom,
                authorized_quantity: line.authorized_quantity,
                received_quantity: line.received_quantity,
                rejected_quantity: line.rejected_quantity,
                missing_quantity: line.missing_quantity,
                remaining_quantity: line.remaining_quantity,
                reason: reason_to_api(line.reason),
                note: line.note,
                lot: line.lot,
                serial: line.serial,
                inspection_hold_ids: line
                    .inspection_hold_ids
                    .into_iter()
                    .map(|id| id.get())
                    .collect(),
            })
            .collect(),
    }))
}

fn map_create(value: CreateCustomerReturnResult) -> V1Result<CreateCustomerReturnResponse> {
    Ok(CreateCustomerReturnResponse {
        customer_return_id: value.customer_return_id.get(),
        number: value.number,
        status: status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        lines: value
            .lines
            .into_iter()
            .map(|line| CreatedCustomerReturnLineResponse {
                line_id: line.line_id.get(),
                item_id: line.item_id.get(),
                authorized_quantity: line.authorized_quantity,
                reason: reason_to_api(line.reason),
            })
            .collect(),
        total_authorized_quantity: value.total_authorized_quantity,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
    })
}

fn map_plan(value: PlanCustomerReturnLoadResult) -> V1Result<PlanCustomerReturnLoadResponse> {
    Ok(PlanCustomerReturnLoadResponse {
        plan_id: value.plan_id.get(),
        customer_return_id: value.customer_return_id.get(),
        status: status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        load_id: value.load_id.get(),
        execution_barcode: value.execution_barcode,
        lines: value
            .lines
            .into_iter()
            .map(|line| PlannedCustomerReturnLoadLineResponse {
                customer_return_line_id: line.customer_return_line_id.get(),
                load_line_id: line.load_line_id.get(),
                item_id: line.item_id.get(),
                authorized_quantity: line.authorized_quantity,
            })
            .collect(),
        total_authorized_quantity: value.total_authorized_quantity,
        planned_by: value.planned_by.get(),
        planned_at: value.planned_at.to_rfc3339(),
    })
}

fn map_cancel(value: CancelCustomerReturnResult) -> V1Result<CancelCustomerReturnResponse> {
    Ok(CancelCustomerReturnResponse {
        cancellation_id: value.cancellation_id.get(),
        customer_return_id: value.customer_return_id.get(),
        previous_status: status_to_api(value.previous_status),
        status: status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        reason: cancellation_reason_to_api(value.reason),
        note: value.note,
        cancelled_by: value.cancelled_by.get(),
        cancelled_at: value.cancelled_at.to_rfc3339(),
    })
}

fn map_summary(value: CustomerReturnReadModel) -> V1Result<CustomerReturnSummaryResponse> {
    Ok(CustomerReturnSummaryResponse {
        customer_return_id: value.customer_return_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        number: value.number,
        customer_reference: value.customer_reference,
        expected_at: value.expected_at.map(|value| value.to_rfc3339()),
        status: status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        line_count: value.line_count,
        total_authorized_quantity: value.total_authorized_quantity,
        total_received_quantity: value.total_received_quantity,
        total_rejected_quantity: value.total_rejected_quantity,
        total_missing_quantity: value.total_missing_quantity,
        total_remaining_quantity: value.total_remaining_quantity,
        load_id: value.load_id.map(|id| id.get()),
        execution_status: value.execution_status.map(execution_status_to_api),
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        planned_by: value.planned_by.map(|id| id.get()),
        planned_at: value.planned_at.map(|value| value.to_rfc3339()),
        cancellation_id: value.cancellation_id.map(|id| id.get()),
        cancellation_reason: value.cancellation_reason.map(cancellation_reason_to_api),
        cancellation_note: value.cancellation_note,
        cancelled_by: value.cancelled_by.map(|id| id.get()),
        cancelled_at: value.cancelled_at.map(|value| value.to_rfc3339()),
    })
}

const fn status_to_domain(value: ApiStatus) -> CustomerReturnStatus {
    match value {
        ApiStatus::Open => CustomerReturnStatus::Open,
        ApiStatus::Planned => CustomerReturnStatus::Planned,
        ApiStatus::Cancelled => CustomerReturnStatus::Cancelled,
    }
}

const fn status_to_api(value: CustomerReturnStatus) -> ApiStatus {
    match value {
        CustomerReturnStatus::Open => ApiStatus::Open,
        CustomerReturnStatus::Planned => ApiStatus::Planned,
        CustomerReturnStatus::Cancelled => ApiStatus::Cancelled,
    }
}

const fn reason_to_domain(value: ApiReason) -> CustomerReturnReason {
    match value {
        ApiReason::CustomerRequest => CustomerReturnReason::CustomerRequest,
        ApiReason::Damaged => CustomerReturnReason::Damaged,
        ApiReason::RefusedDelivery => CustomerReturnReason::RefusedDelivery,
        ApiReason::Recall => CustomerReturnReason::Recall,
        ApiReason::Warranty => CustomerReturnReason::Warranty,
        ApiReason::Other => CustomerReturnReason::Other,
    }
}

const fn reason_to_api(value: CustomerReturnReason) -> ApiReason {
    match value {
        CustomerReturnReason::CustomerRequest => ApiReason::CustomerRequest,
        CustomerReturnReason::Damaged => ApiReason::Damaged,
        CustomerReturnReason::RefusedDelivery => ApiReason::RefusedDelivery,
        CustomerReturnReason::Recall => ApiReason::Recall,
        CustomerReturnReason::Warranty => ApiReason::Warranty,
        CustomerReturnReason::Other => ApiReason::Other,
    }
}

const fn cancellation_reason_to_domain(
    value: ApiCancellationReason,
) -> CustomerReturnCancellationReason {
    match value {
        ApiCancellationReason::CustomerCancelled => {
            CustomerReturnCancellationReason::CustomerCancelled
        }
        ApiCancellationReason::DuplicateAuthorization => {
            CustomerReturnCancellationReason::DuplicateAuthorization
        }
        ApiCancellationReason::ReturnWindowExpired => {
            CustomerReturnCancellationReason::ReturnWindowExpired
        }
        ApiCancellationReason::Other => CustomerReturnCancellationReason::Other,
    }
}

const fn cancellation_reason_to_api(
    value: CustomerReturnCancellationReason,
) -> ApiCancellationReason {
    match value {
        CustomerReturnCancellationReason::CustomerCancelled => {
            ApiCancellationReason::CustomerCancelled
        }
        CustomerReturnCancellationReason::DuplicateAuthorization => {
            ApiCancellationReason::DuplicateAuthorization
        }
        CustomerReturnCancellationReason::ReturnWindowExpired => {
            ApiCancellationReason::ReturnWindowExpired
        }
        CustomerReturnCancellationReason::Other => ApiCancellationReason::Other,
    }
}

const fn execution_status_to_api(value: CustomerReturnExecutionStatus) -> ApiExecutionStatus {
    match value {
        CustomerReturnExecutionStatus::Planned => ApiExecutionStatus::Planned,
        CustomerReturnExecutionStatus::Scheduled => ApiExecutionStatus::Scheduled,
        CustomerReturnExecutionStatus::Arrived => ApiExecutionStatus::Arrived,
        CustomerReturnExecutionStatus::Receiving => ApiExecutionStatus::Receiving,
        CustomerReturnExecutionStatus::Received => ApiExecutionStatus::Received,
        CustomerReturnExecutionStatus::Rejected => ApiExecutionStatus::Rejected,
        CustomerReturnExecutionStatus::Closed => ApiExecutionStatus::Closed,
        CustomerReturnExecutionStatus::Cancelled => ApiExecutionStatus::Cancelled,
    }
}

fn cursor_filter(request: &CustomerReturnPageRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.search.as_deref().unwrap_or_default().as_bytes());
    let search_hash = hex::encode(hasher.finalize());
    format!(
        "{}.{}.{}.{}",
        request
            .facility_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        match request.status {
            None => "all",
            Some(ApiStatus::Open) => "open",
            Some(ApiStatus::Planned) => "planned",
            Some(ApiStatus::Cancelled) => "cancelled",
        },
        &search_hash[..16]
    )
}

fn encode_cursor(offset: u64, request: &CustomerReturnPageRequest) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{offset:016x}",
        cursor_filter(request)
    ))
    .map_err(|_| V1Error::internal("generated an invalid customer return cursor"))
}

fn decode_cursor(cursor: &OpaqueCursor, request: &CustomerReturnPageRequest) -> V1Result<u64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("customer returns"))?;
    let (filter, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("customer returns"))?;
    if filter != cursor_filter(request) || offset.len() != 16 {
        return Err(V1Error::invalid_cursor_for("customer returns"));
    }
    u64::from_str_radix(offset, 16).map_err(|_| V1Error::invalid_cursor_for("customer returns"))
}

fn validate_search(value: &str) -> V1Result<&str> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > MAX_SEARCH_LENGTH
        || value.chars().any(char::is_control)
    {
        Err(AppError::bad_request("customer return search is invalid").into())
    } else {
        Ok(value)
    }
}

fn parse_timestamp(value: &str, field: &str) -> V1Result<Timestamp> {
    value
        .parse::<Timestamp>()
        .map_err(|error| AppError::bad_request(format!("{field} is invalid: {error}")).into())
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::PageLimit;

    #[test]
    fn cursor_is_bound_to_return_filters() {
        let request = CustomerReturnPageRequest {
            facility_id: Some(4),
            inventory_owner_id: None,
            status: Some(ApiStatus::Open),
            search: Some("RMA-100".into()),
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = encode_cursor(100, &request).unwrap();
        assert_eq!(decode_cursor(&cursor, &request).unwrap(), 100);
        let mut changed = request;
        changed.status = Some(ApiStatus::Planned);
        assert!(decode_cursor(&cursor, &changed).is_err());
    }
}
