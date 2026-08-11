use axum::extract::{Path, Query, State};
use axum::Json;
use sha2::{Digest, Sha256};
use wareboxes_api_contract::v1::{
    CreatePurchaseOrderAsnRequest, CreatePurchaseOrderAsnResponse, CreatePurchaseOrderRequest,
    CreatePurchaseOrderResponse, CreatedPurchaseOrderAsnLineResponse,
    CreatedPurchaseOrderLineResponse, InboundAsnStatus as ApiInboundAsnStatus, OpaqueCursor,
    PurchaseOrderDetailResponse, PurchaseOrderLineResponse, PurchaseOrderPage as ApiPage,
    PurchaseOrderPageRequest, PurchaseOrderStatus as ApiStatus, PurchaseOrderSummaryResponse,
    ReleasePurchaseOrderRequest, ReleasePurchaseOrderResponse, Revision,
};
use wareboxes_application::inbound_asn::{
    CreatePurchaseOrderAsnCommand, CreatePurchaseOrderAsnResult,
};
use wareboxes_application::purchase_order::{
    CreatePurchaseOrderCommand, CreatePurchaseOrderResult, PurchaseOrderPageFilter,
    PurchaseOrderReadModel, ReleasePurchaseOrderCommand, ReleasePurchaseOrderResult,
};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InboundAsnNumber, InboundAsnQuantity, InboundAsnRevision,
    InboundAsnStatus, InventoryOwnerId, NewPurchaseOrder, NewPurchaseOrderAsn,
    PurchaseOrderAsnLineDefinition, PurchaseOrderId, PurchaseOrderLineDefinition,
    PurchaseOrderLineId, PurchaseOrderNumber, PurchaseOrderQuantity, PurchaseOrderRevision,
    PurchaseOrderStatus, PurchaseOrderSupplier, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "po1.";
const MAX_SEARCH_LENGTH: usize = 100;

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreatePurchaseOrderRequest>,
) -> V1Result<Json<CreatePurchaseOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let order = NewPurchaseOrder::new(
        InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        FacilityId::new(body.facility_id).map_err(validation)?,
        PurchaseOrderNumber::new(body.number).map_err(validation)?,
        PurchaseOrderSupplier::new(body.supplier).map_err(validation)?,
        body.expected_by
            .map(|value| parse_timestamp(&value, "expected_by"))
            .transpose()?,
        body.lines
            .into_iter()
            .map(|line| {
                Ok(PurchaseOrderLineDefinition::new(
                    CatalogItemId::new(line.item_id).map_err(validation)?,
                    PurchaseOrderQuantity::new(line.ordered_quantity).map_err(validation)?,
                ))
            })
            .collect::<V1Result<Vec<_>>>()?,
    )
    .map_err(validation)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::purchase_order::create(
        &state.db,
        &user.tenant,
        &context,
        &CreatePurchaseOrderCommand { order },
    )
    .await?;
    Ok(Json(map_create(result)?))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(purchase_order_id): Path<i64>,
    Json(body): Json<ReleasePurchaseOrderRequest>,
) -> V1Result<Json<ReleasePurchaseOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ReleasePurchaseOrderCommand {
        purchase_order_id: PurchaseOrderId::new(purchase_order_id).map_err(validation)?,
        expected_revision: PurchaseOrderRevision::new(body.expected_revision.get())
            .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::purchase_order::release(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_release(result)?))
}

pub async fn create_asn(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(purchase_order_id): Path<i64>,
    Json(body): Json<CreatePurchaseOrderAsnRequest>,
) -> V1Result<Json<CreatePurchaseOrderAsnResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let notice = NewPurchaseOrderAsn::new(
        PurchaseOrderId::new(purchase_order_id).map_err(validation)?,
        PurchaseOrderRevision::new(body.expected_purchase_order_revision.get())
            .map_err(validation)?,
        InboundAsnNumber::new(body.number).map_err(validation)?,
        body.expected_at
            .map(|value| parse_timestamp(&value, "expected_at"))
            .transpose()?,
        body.lines
            .into_iter()
            .map(|line| {
                PurchaseOrderAsnLineDefinition::new(
                    PurchaseOrderLineId::new(line.purchase_order_line_id).map_err(validation)?,
                    InboundAsnQuantity::new(line.expected_quantity).map_err(validation)?,
                    line.lot,
                    line.serial,
                    line.expiration
                        .map(|value| parse_timestamp(&value, "expiration"))
                        .transpose()?,
                )
                .map_err(validation)
            })
            .collect::<V1Result<Vec<_>>>()?,
    )
    .map_err(validation)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::purchase_order::create_asn(
        &state.db,
        &user.tenant,
        &context,
        &CreatePurchaseOrderAsnCommand { notice },
    )
    .await?;
    Ok(Json(map_create_asn(result)?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<PurchaseOrderPageRequest>,
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
    let page = repo::purchase_order::page(
        &state.db,
        &user.tenant,
        &PurchaseOrderPageFilter {
            facility_id,
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
    Path(purchase_order_id): Path<i64>,
) -> V1Result<Json<PurchaseOrderDetailResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let detail = repo::purchase_order::detail(
        &state.db,
        &user.tenant,
        PurchaseOrderId::new(purchase_order_id).map_err(validation)?,
    )
    .await?
    .ok_or_else(|| V1Error::from(AppError::not_found("purchase order")))?;
    let summary = map_summary(detail.clone())?;
    Ok(Json(PurchaseOrderDetailResponse {
        summary,
        lines: detail
            .lines
            .into_iter()
            .map(|line| PurchaseOrderLineResponse {
                line_id: line.line_id.get(),
                sequence: line.sequence,
                item_id: line.item_id.get(),
                item_description: line.item_description,
                uom: line.uom,
                ordered_quantity: line.ordered_quantity,
                historical_asn_quantity: line.historical_asn_quantity,
                active_inbound_quantity: line.active_inbound_quantity,
                available_to_notify_quantity: line.available_to_notify_quantity,
                received_quantity: line.received_quantity,
                rejected_quantity: line.rejected_quantity,
                missing_quantity: line.missing_quantity,
                open_receipt_quantity: line.open_receipt_quantity,
            })
            .collect(),
    }))
}

fn map_create_asn(value: CreatePurchaseOrderAsnResult) -> V1Result<CreatePurchaseOrderAsnResponse> {
    Ok(CreatePurchaseOrderAsnResponse {
        source_id: value.source_id.get(),
        purchase_order_id: value.purchase_order_id.get(),
        purchase_order_revision: api_revision(value.purchase_order_revision)?,
        asn_id: value.asn_id.get(),
        number: value.number,
        status: map_inbound_asn_status(value.status),
        revision: api_inbound_asn_revision(value.revision)?,
        lines: value
            .lines
            .into_iter()
            .map(|line| CreatedPurchaseOrderAsnLineResponse {
                source_line_id: line.source_line_id.get(),
                purchase_order_line_id: line.purchase_order_line_id.get(),
                asn_line_id: line.asn_line_id.get(),
                item_id: line.item_id.get(),
                expected_quantity: line.expected_quantity,
            })
            .collect(),
        total_expected_quantity: value.total_expected_quantity,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
    })
}

fn map_create(value: CreatePurchaseOrderResult) -> V1Result<CreatePurchaseOrderResponse> {
    Ok(CreatePurchaseOrderResponse {
        purchase_order_id: value.purchase_order_id.get(),
        number: value.number,
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        lines: value
            .lines
            .into_iter()
            .map(|line| CreatedPurchaseOrderLineResponse {
                line_id: line.line_id.get(),
                item_id: line.item_id.get(),
                ordered_quantity: line.ordered_quantity,
            })
            .collect(),
        total_ordered_quantity: value.total_ordered_quantity,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
    })
}

fn map_inbound_asn_status(value: InboundAsnStatus) -> ApiInboundAsnStatus {
    match value {
        InboundAsnStatus::Open => ApiInboundAsnStatus::Open,
        InboundAsnStatus::Planned => ApiInboundAsnStatus::Planned,
    }
}

fn api_inbound_asn_revision(value: InboundAsnRevision) -> V1Result<Revision> {
    Revision::new(value.get()).map_err(|error| V1Error::internal(error.to_string()))
}

fn map_release(value: ReleasePurchaseOrderResult) -> V1Result<ReleasePurchaseOrderResponse> {
    Ok(ReleasePurchaseOrderResponse {
        release_id: value.release_id.get(),
        purchase_order_id: value.purchase_order_id.get(),
        previous_status: map_status_to_api(value.previous_status),
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        released_by: value.released_by.get(),
        released_at: value.released_at.to_rfc3339(),
    })
}

fn map_summary(value: PurchaseOrderReadModel) -> V1Result<PurchaseOrderSummaryResponse> {
    Ok(PurchaseOrderSummaryResponse {
        purchase_order_id: value.purchase_order_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        number: value.number,
        supplier: value.supplier,
        expected_by: value.expected_by.map(|timestamp| timestamp.to_rfc3339()),
        status: map_status_to_api(value.status),
        revision: api_revision(value.revision)?,
        line_count: value.line_count,
        total_ordered_quantity: value.total_ordered_quantity,
        total_historical_asn_quantity: value.total_historical_asn_quantity,
        total_active_inbound_quantity: value.total_active_inbound_quantity,
        total_available_to_notify_quantity: value.total_available_to_notify_quantity,
        total_received_quantity: value.total_received_quantity,
        total_rejected_quantity: value.total_rejected_quantity,
        total_missing_quantity: value.total_missing_quantity,
        total_open_receipt_quantity: value.total_open_receipt_quantity,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        released_by: value.released_by.map(wareboxes_domain::UserId::get),
        released_at: value.released_at.map(|timestamp| timestamp.to_rfc3339()),
    })
}

fn map_status(value: ApiStatus) -> PurchaseOrderStatus {
    match value {
        ApiStatus::Draft => PurchaseOrderStatus::Draft,
        ApiStatus::Released => PurchaseOrderStatus::Released,
    }
}

fn map_status_to_api(value: PurchaseOrderStatus) -> ApiStatus {
    match value {
        PurchaseOrderStatus::Draft => ApiStatus::Draft,
        PurchaseOrderStatus::Released => ApiStatus::Released,
    }
}

fn api_revision(value: PurchaseOrderRevision) -> V1Result<Revision> {
    Revision::new(value.get()).map_err(|error| V1Error::internal(error.to_string()))
}

fn parse_timestamp(value: &str, field: &str) -> V1Result<Timestamp> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc))
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

fn encode_cursor(offset: u64, request: &PurchaseOrderPageRequest) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{offset:016x}",
        cursor_fingerprint(request)
    ))
    .map_err(|error| V1Error::internal(error.to_string()))
}

fn decode_cursor(cursor: &OpaqueCursor, request: &PurchaseOrderPageRequest) -> V1Result<u64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("purchase order"))?;
    let (fingerprint, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("purchase order"))?;
    if fingerprint != cursor_fingerprint(request) || offset.len() != 16 {
        return Err(V1Error::invalid_cursor_for("purchase order"));
    }
    u64::from_str_radix(offset, 16).map_err(|_| V1Error::invalid_cursor_for("purchase order"))
}

fn cursor_fingerprint(request: &PurchaseOrderPageRequest) -> String {
    let raw = format!(
        "{}|{}|{}|{}|{}",
        request
            .facility_id
            .map_or_else(String::new, |id| id.to_string()),
        request
            .inventory_owner_id
            .map_or_else(String::new, |id| id.to_string()),
        request.status.map_or("", |status| match status {
            ApiStatus::Draft => "draft",
            ApiStatus::Released => "released",
        }),
        request.search.as_deref().unwrap_or_default(),
        request.limit.get()
    );
    let digest = Sha256::digest(raw.as_bytes());
    hex::encode(&digest[..8])
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_is_bound_to_filters() {
        let request = PurchaseOrderPageRequest {
            facility_id: Some(7),
            inventory_owner_id: Some(8),
            status: Some(ApiStatus::Draft),
            search: Some("PO 100".into()),
            cursor: None,
            limit: wareboxes_api_contract::v1::PageLimit::new(20).unwrap(),
        };
        let cursor = encode_cursor(20, &request).unwrap();
        assert_eq!(decode_cursor(&cursor, &request).unwrap(), 20);
        let mut changed = request;
        changed.status = Some(ApiStatus::Released);
        assert!(decode_cursor(&cursor, &changed).is_err());
    }
}
