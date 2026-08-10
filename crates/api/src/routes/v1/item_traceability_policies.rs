use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ConfigureItemTraceabilityPolicyRequest, ItemTraceabilityPolicyPage as ApiPolicyPage,
    ItemTraceabilityPolicyPageRequest, ItemTraceabilityPolicyResponse,
    ItemTraceabilityPolicyStatus as ApiStatus, OpaqueCursor, RetireItemTraceabilityPolicyRequest,
    Revision, TraceabilityRequirement as ApiRequirement,
};
use wareboxes_application::item_traceability_policy::{
    ConfigureItemTraceabilityPolicyCommand, ItemTraceabilityPolicyCursor,
    ItemTraceabilityPolicyPageQuery, ItemTraceabilityPolicyReadModel,
    RetireItemTraceabilityPolicyCommand,
};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, ItemTraceabilityPolicyDefinition,
    ItemTraceabilityPolicyId, ItemTraceabilityPolicyRevision, ItemTraceabilityPolicyStatus,
    ItemTraceabilityPolicyUom, MinimumShelfLifeDays, TraceabilityRequirement,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const READ_PERMISSION: &str = "wms";
const MUTATE_PERMISSION: &str = "wms_supervisor";
const CURSOR_PREFIX: &str = "itp1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<ItemTraceabilityPolicyPageRequest>,
) -> V1Result<Json<ApiPolicyPage>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| InventoryOwnerId::new(id).map_err(validation))
        .transpose()?;
    let facility_id = request
        .facility_id
        .map(|id| FacilityId::new(id).map_err(validation))
        .transpose()?;
    let item_id = request
        .item_id
        .map(|id| CatalogItemId::new(id).map_err(validation))
        .transpose()?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let page = repo::item_traceability_policy::item_traceability_policy_page(
        &state.db,
        &user.tenant,
        ItemTraceabilityPolicyPageQuery {
            inventory_owner_id,
            facility_id,
            item_id,
            lot: request.lot.map(map_requirement),
            serial: request.serial.map(map_requirement),
            expiration: request.expiration.map(map_requirement),
            status: request.status.map(map_status),
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_cursor(cursor, &request))
        .transpose()?;
    Ok(Json(ApiPolicyPage::new(
        page.items
            .into_iter()
            .map(map_response)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn configure(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureItemTraceabilityPolicyRequest>,
) -> V1Result<Json<ItemTraceabilityPolicyResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let inventory_owner_id = InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?;
    let facility_id = FacilityId::new(body.facility_id).map_err(validation)?;
    let command = ConfigureItemTraceabilityPolicyCommand {
        definition: ItemTraceabilityPolicyDefinition::new(
            user.tenant.tenant_id,
            inventory_owner_id,
            facility_id,
            CatalogItemId::new(body.item_id).map_err(validation)?,
            ItemTraceabilityPolicyUom::new(body.uom).map_err(validation)?,
            map_requirement(body.lot),
            map_requirement(body.serial),
            map_requirement(body.expiration),
            body.minimum_shelf_life_days
                .map(MinimumShelfLifeDays::new)
                .transpose()
                .map_err(validation)?,
        )
        .map_err(validation)?,
        expected_revision: body
            .expected_revision
            .map(|revision| ItemTraceabilityPolicyRevision::new(revision.get()).map_err(validation))
            .transpose()?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::item_traceability_policy::configure_item_traceability_policy(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn retire(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(policy_id): Path<i64>,
    Json(body): Json<RetireItemTraceabilityPolicyRequest>,
) -> V1Result<Json<ItemTraceabilityPolicyResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let command = RetireItemTraceabilityPolicyCommand {
        item_traceability_policy_id: ItemTraceabilityPolicyId::new(policy_id)
            .map_err(validation)?,
        expected_revision: ItemTraceabilityPolicyRevision::new(body.expected_revision.get())
            .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::item_traceability_policy::retire_item_traceability_policy(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

fn map_response(
    value: ItemTraceabilityPolicyReadModel,
) -> V1Result<ItemTraceabilityPolicyResponse> {
    Ok(ItemTraceabilityPolicyResponse {
        item_traceability_policy_id: value.item_traceability_policy_id.get(),
        inventory_owner_id: value.definition.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.definition.facility_id.get(),
        facility_name: value.facility_name,
        item_id: value.definition.item_id.get(),
        item_description: value.item_description,
        uom: value.definition.uom.as_str().to_owned(),
        lot: map_requirement_to_api(value.definition.lot),
        serial: map_requirement_to_api(value.definition.serial),
        expiration: map_requirement_to_api(value.definition.expiration),
        minimum_shelf_life_days: value
            .definition
            .minimum_shelf_life_days
            .map(MinimumShelfLifeDays::get),
        status: map_status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        retired_by: value.retired_by.map(|user| user.get()),
        retired_at: value.retired_at.map(|time| time.to_rfc3339()),
    })
}

const fn map_requirement(value: ApiRequirement) -> TraceabilityRequirement {
    match value {
        ApiRequirement::NotTracked => TraceabilityRequirement::NotTracked,
        ApiRequirement::Required => TraceabilityRequirement::Required,
    }
}

const fn map_requirement_to_api(value: TraceabilityRequirement) -> ApiRequirement {
    match value {
        TraceabilityRequirement::NotTracked => ApiRequirement::NotTracked,
        TraceabilityRequirement::Required => ApiRequirement::Required,
    }
}

const fn map_status(value: ApiStatus) -> ItemTraceabilityPolicyStatus {
    match value {
        ApiStatus::Active => ItemTraceabilityPolicyStatus::Active,
        ApiStatus::Retired => ItemTraceabilityPolicyStatus::Retired,
    }
}

const fn map_status_to_api(value: ItemTraceabilityPolicyStatus) -> ApiStatus {
    match value {
        ItemTraceabilityPolicyStatus::Active => ApiStatus::Active,
        ItemTraceabilityPolicyStatus::Retired => ApiStatus::Retired,
    }
}

fn cursor_filter(request: &ItemTraceabilityPolicyPageRequest) -> String {
    format!(
        "{}.{}.{}.{}.{}.{}.{}",
        encoded_id(request.inventory_owner_id),
        encoded_id(request.facility_id),
        encoded_id(request.item_id),
        request.lot.map_or("all", requirement_name),
        request.serial.map_or("all", requirement_name),
        request.expiration.map_or("all", requirement_name),
        request.status.map_or("active", status_name),
    )
}

fn encoded_id(value: Option<i64>) -> String {
    value.map_or_else(|| "-".to_owned(), |id| format!("{id:016x}"))
}

const fn requirement_name(value: ApiRequirement) -> &'static str {
    match value {
        ApiRequirement::NotTracked => "not_tracked",
        ApiRequirement::Required => "required",
    }
}

const fn status_name(value: ApiStatus) -> &'static str {
    match value {
        ApiStatus::Active => "active",
        ApiStatus::Retired => "retired",
    }
}

fn encode_cursor(
    cursor: ItemTraceabilityPolicyCursor,
    request: &ItemTraceabilityPolicyPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        cursor.after_item_traceability_policy_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid item traceability policy cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &ItemTraceabilityPolicyPageRequest,
) -> V1Result<ItemTraceabilityPolicyCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("item traceability policy"))?;
    let (filter, id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("item traceability policy"))?;
    if filter != cursor_filter(request) || id.len() != 16 {
        return Err(V1Error::invalid_cursor_for("item traceability policy"));
    }
    let id = i64::from_str_radix(id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("item traceability policy"))?;
    Ok(ItemTraceabilityPolicyCursor {
        after_item_traceability_policy_id: ItemTraceabilityPolicyId::new(id)
            .map_err(|_| V1Error::invalid_cursor_for("item traceability policy"))?,
    })
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
    fn cursor_round_trips_and_is_filter_bound() {
        let request = ItemTraceabilityPolicyPageRequest {
            inventory_owner_id: Some(2),
            facility_id: Some(3),
            item_id: Some(4),
            lot: Some(ApiRequirement::Required),
            serial: None,
            expiration: Some(ApiRequirement::Required),
            status: None,
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = ItemTraceabilityPolicyCursor {
            after_item_traceability_policy_id: ItemTraceabilityPolicyId::new(9).unwrap(),
        };
        let encoded = encode_cursor(cursor, &request).unwrap();
        assert_eq!(decode_cursor(&encoded, &request).unwrap(), cursor);
        let mut changed = request;
        changed.serial = Some(ApiRequirement::Required);
        assert!(decode_cursor(&encoded, &changed).is_err());
    }
}
