use axum::extract::{Path, Query, State};
use axum::Json;
use sha2::{Digest, Sha256};
use wareboxes_api_contract::v1::{
    CreateInboundAsnRequest, CreateInboundAsnResponse, CreatedInboundAsnLineResponse,
    InboundAsnDetailResponse, InboundAsnLineResponse, InboundAsnPage as ApiPage,
    InboundAsnPageRequest, InboundAsnStatus as ApiStatus, InboundAsnSummaryResponse, OpaqueCursor,
    PlanInboundAsnLoadRequest, PlanInboundAsnLoadResponse, PlannedInboundAsnLoadLineResponse,
    Revision,
};
use wareboxes_application::inbound_asn::{
    CreateInboundAsnCommand, CreateInboundAsnResult, InboundAsnPageFilter, InboundAsnReadModel,
    PlanInboundAsnLoadCommand, PlanInboundAsnLoadResult,
};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InboundAsnId, InboundAsnLineDefinition, InboundAsnLoadPlanDetails,
    InboundAsnNumber, InboundAsnQuantity, InboundAsnRevision, InboundAsnStatus, InboundAsnSupplier,
    InventoryOwnerId, NewInboundAsn, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "ia1.";
const MAX_SEARCH_LENGTH: usize = 100;

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateInboundAsnRequest>,
) -> V1Result<Json<CreateInboundAsnResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let notice = NewInboundAsn::new(
        InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        FacilityId::new(body.facility_id).map_err(validation)?,
        InboundAsnNumber::new(body.number).map_err(validation)?,
        InboundAsnSupplier::new(body.supplier).map_err(validation)?,
        body.expected_at
            .map(|value| parse_timestamp(&value, "expected_at"))
            .transpose()?,
        body.lines
            .into_iter()
            .map(|line| {
                InboundAsnLineDefinition::new(
                    CatalogItemId::new(line.item_id).map_err(validation)?,
                    InboundAsnQuantity::new(line.expected_quantity).map_err(validation)?,
                    line.lot,
                    line.serial,
                    line.expiration
                        .map(|value| parse_timestamp(&value, "line expiration"))
                        .transpose()?,
                )
                .map_err(validation)
            })
            .collect::<V1Result<Vec<_>>>()?,
    )
    .map_err(validation)?;
    let command = CreateInboundAsnCommand { notice };
    let context = user.command_context(&idempotency_key);
    let result = repo::inbound_asn::create(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_create(result)?))
}

pub async fn plan_load(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(asn_id): Path<i64>,
    Json(body): Json<PlanInboundAsnLoadRequest>,
) -> V1Result<Json<PlanInboundAsnLoadResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = PlanInboundAsnLoadCommand {
        asn_id: InboundAsnId::new(asn_id).map_err(validation)?,
        expected_revision: InboundAsnRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        details: InboundAsnLoadPlanDetails::new(
            wareboxes_domain::LocationId::new(body.receiving_location_id).map_err(validation)?,
            body.carrier,
            body.trailer_number,
            body.seal_number,
        )
        .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::inbound_asn::plan_load(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_plan(result)?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<InboundAsnPageRequest>,
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
    let page = repo::inbound_asn::page(
        &state.db,
        &user.tenant,
        &InboundAsnPageFilter {
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
    Path(asn_id): Path<i64>,
) -> V1Result<Json<InboundAsnDetailResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let detail = repo::inbound_asn::detail(
        &state.db,
        &user.tenant,
        InboundAsnId::new(asn_id).map_err(validation)?,
    )
    .await?
    .ok_or_else(|| V1Error::from(AppError::not_found("advance shipping notice")))?;
    let summary = map_summary(detail.clone())?;
    Ok(Json(InboundAsnDetailResponse {
        summary,
        lines: detail
            .lines
            .into_iter()
            .map(|line| InboundAsnLineResponse {
                line_id: line.line_id.get(),
                sequence: line.sequence,
                item_id: line.item_id.get(),
                item_description: line.item_description,
                uom: line.uom,
                expected_quantity: line.expected_quantity,
                lot: line.lot,
                serial: line.serial,
                expiration: line.expiration.map(|value| value.to_rfc3339()),
            })
            .collect(),
    }))
}

fn map_create(value: CreateInboundAsnResult) -> V1Result<CreateInboundAsnResponse> {
    Ok(CreateInboundAsnResponse {
        asn_id: value.asn_id.get(),
        number: value.number,
        status: map_status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        lines: value
            .lines
            .into_iter()
            .map(|line| CreatedInboundAsnLineResponse {
                line_id: line.line_id.get(),
                item_id: line.item_id.get(),
                expected_quantity: line.expected_quantity,
            })
            .collect(),
        total_expected_quantity: value.total_expected_quantity,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
    })
}

fn map_plan(value: PlanInboundAsnLoadResult) -> V1Result<PlanInboundAsnLoadResponse> {
    Ok(PlanInboundAsnLoadResponse {
        plan_id: value.plan_id.get(),
        asn_id: value.asn_id.get(),
        asn_status: map_status_to_api(value.asn_status),
        asn_revision: Revision::new(value.asn_revision.get()).map_err(invalid_result)?,
        load_id: value.load_id.get(),
        execution_barcode: value.execution_barcode,
        lines: value
            .lines
            .into_iter()
            .map(|line| PlannedInboundAsnLoadLineResponse {
                asn_line_id: line.asn_line_id.get(),
                load_line_id: line.load_line_id.get(),
                item_id: line.item_id.get(),
                expected_quantity: line.expected_quantity,
            })
            .collect(),
        total_expected_quantity: value.total_expected_quantity,
        planned_by: value.planned_by.get(),
        planned_at: value.planned_at.to_rfc3339(),
    })
}

fn map_summary(value: InboundAsnReadModel) -> V1Result<InboundAsnSummaryResponse> {
    Ok(InboundAsnSummaryResponse {
        asn_id: value.asn_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        number: value.number,
        supplier: value.supplier,
        expected_at: value.expected_at.map(|value| value.to_rfc3339()),
        status: map_status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        line_count: value.line_count,
        total_expected_quantity: value.total_expected_quantity,
        load_id: value.load_id.map(|id| id.get()),
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        planned_by: value.planned_by.map(|id| id.get()),
        planned_at: value.planned_at.map(|value| value.to_rfc3339()),
    })
}

const fn map_status(value: ApiStatus) -> InboundAsnStatus {
    match value {
        ApiStatus::Open => InboundAsnStatus::Open,
        ApiStatus::Planned => InboundAsnStatus::Planned,
    }
}

const fn map_status_to_api(value: InboundAsnStatus) -> ApiStatus {
    match value {
        InboundAsnStatus::Open => ApiStatus::Open,
        InboundAsnStatus::Planned => ApiStatus::Planned,
    }
}

fn cursor_filter(request: &InboundAsnPageRequest) -> String {
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
        },
        &search_hash[..16]
    )
}

fn encode_cursor(offset: u64, request: &InboundAsnPageRequest) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{offset:016x}",
        cursor_filter(request)
    ))
    .map_err(|_| V1Error::internal("generated an invalid ASN cursor"))
}

fn decode_cursor(cursor: &OpaqueCursor, request: &InboundAsnPageRequest) -> V1Result<u64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("advance shipping notices"))?;
    let (filter, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("advance shipping notices"))?;
    if filter != cursor_filter(request) || offset.len() != 16 {
        return Err(V1Error::invalid_cursor_for("advance shipping notices"));
    }
    u64::from_str_radix(offset, 16)
        .map_err(|_| V1Error::invalid_cursor_for("advance shipping notices"))
}

fn validate_search(value: &str) -> V1Result<&str> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > MAX_SEARCH_LENGTH
        || value.chars().any(char::is_control)
    {
        Err(AppError::bad_request("ASN search is invalid").into())
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
    fn cursor_is_filter_bound() {
        let request = InboundAsnPageRequest {
            facility_id: Some(4),
            inventory_owner_id: None,
            status: Some(ApiStatus::Open),
            search: Some("ASN-100".into()),
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
