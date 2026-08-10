use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ConfigureItemStoragePolicyRequest, ItemStoragePolicyPage as ApiPolicyPage,
    ItemStoragePolicyPageRequest, ItemStoragePolicyResponse, ItemStoragePolicyStatus as ApiStatus,
    OpaqueCursor, RetireItemStoragePolicyRequest, Revision, StorageZonePurpose as ApiPurpose,
};
use wareboxes_application::item_storage_policy::{
    ConfigureItemStoragePolicyCommand, ItemStoragePolicyCursor, ItemStoragePolicyPageQuery,
    ItemStoragePolicyReadModel, RetireItemStoragePolicyCommand,
};
use wareboxes_domain::{
    AllowedStorageZonePurposes, CatalogItemId, ItemStorageLocationCapacity,
    ItemStoragePolicyDefinition, ItemStoragePolicyId, ItemStoragePolicyRevision,
    ItemStoragePolicyStatus, ItemStoragePolicyUom, StorageZonePurpose,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const READ_PERMISSION: &str = "wms";
const MUTATE_PERMISSION: &str = "wms_supervisor";
const CURSOR_PREFIX: &str = "isp1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<ItemStoragePolicyPageRequest>,
) -> V1Result<Json<ApiPolicyPage>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
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
    let page = repo::item_storage_policy::item_storage_policy_page(
        &state.db,
        &user.tenant,
        ItemStoragePolicyPageQuery {
            inventory_owner_id,
            facility_id,
            item_id,
            purpose: request.purpose.map(map_purpose),
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
    Json(body): Json<ConfigureItemStoragePolicyRequest>,
) -> V1Result<Json<ItemStoragePolicyResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let inventory_owner_id = user.require_inventory_owner(body.inventory_owner_id)?;
    let facility_id = user.require_facility(body.facility_id)?;
    let command = ConfigureItemStoragePolicyCommand {
        definition: ItemStoragePolicyDefinition {
            tenant_id: user.tenant.tenant_id,
            inventory_owner_id,
            facility_id,
            item_id: CatalogItemId::new(body.item_id).map_err(validation)?,
            uom: ItemStoragePolicyUom::new(body.uom).map_err(validation)?,
            allowed_zone_purposes: AllowedStorageZonePurposes::new(
                body.allowed_zone_purposes
                    .into_iter()
                    .map(map_purpose)
                    .collect(),
            )
            .map_err(validation)?,
            max_quantity_per_location: body
                .max_quantity_per_location
                .map(|value| ItemStorageLocationCapacity::new(value).map_err(validation))
                .transpose()?,
        },
        expected_revision: body
            .expected_revision
            .map(|revision| ItemStoragePolicyRevision::new(revision.get()).map_err(validation))
            .transpose()?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::item_storage_policy::configure_item_storage_policy(
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
    Json(body): Json<RetireItemStoragePolicyRequest>,
) -> V1Result<Json<ItemStoragePolicyResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let command = RetireItemStoragePolicyCommand {
        item_storage_policy_id: ItemStoragePolicyId::new(policy_id).map_err(validation)?,
        expected_revision: ItemStoragePolicyRevision::new(body.expected_revision.get())
            .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::item_storage_policy::retire_item_storage_policy(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

fn map_response(value: ItemStoragePolicyReadModel) -> V1Result<ItemStoragePolicyResponse> {
    Ok(ItemStoragePolicyResponse {
        item_storage_policy_id: value.item_storage_policy_id.get(),
        inventory_owner_id: value.definition.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.definition.facility_id.get(),
        facility_name: value.facility_name,
        item_id: value.definition.item_id.get(),
        item_description: value.item_description,
        uom: value.definition.uom.as_str().to_owned(),
        allowed_zone_purposes: value
            .definition
            .allowed_zone_purposes
            .as_slice()
            .iter()
            .copied()
            .map(map_purpose_to_api)
            .collect(),
        max_quantity_per_location: value
            .definition
            .max_quantity_per_location
            .map(ItemStorageLocationCapacity::get),
        status: map_status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        retired_by: value.retired_by.map(|user| user.get()),
        retired_at: value.retired_at.map(|time| time.to_rfc3339()),
    })
}

const fn map_purpose(value: ApiPurpose) -> StorageZonePurpose {
    match value {
        ApiPurpose::Receiving => StorageZonePurpose::Receiving,
        ApiPurpose::Reserve => StorageZonePurpose::Reserve,
        ApiPurpose::Pick => StorageZonePurpose::Pick,
        ApiPurpose::Staging => StorageZonePurpose::Staging,
        ApiPurpose::Packing => StorageZonePurpose::Packing,
        ApiPurpose::Shipping => StorageZonePurpose::Shipping,
        ApiPurpose::Quarantine => StorageZonePurpose::Quarantine,
        ApiPurpose::Damage => StorageZonePurpose::Damage,
    }
}

const fn map_purpose_to_api(value: StorageZonePurpose) -> ApiPurpose {
    match value {
        StorageZonePurpose::Receiving => ApiPurpose::Receiving,
        StorageZonePurpose::Reserve => ApiPurpose::Reserve,
        StorageZonePurpose::Pick => ApiPurpose::Pick,
        StorageZonePurpose::Staging => ApiPurpose::Staging,
        StorageZonePurpose::Packing => ApiPurpose::Packing,
        StorageZonePurpose::Shipping => ApiPurpose::Shipping,
        StorageZonePurpose::Quarantine => ApiPurpose::Quarantine,
        StorageZonePurpose::Damage => ApiPurpose::Damage,
    }
}

const fn map_status(value: ApiStatus) -> ItemStoragePolicyStatus {
    match value {
        ApiStatus::Active => ItemStoragePolicyStatus::Active,
        ApiStatus::Retired => ItemStoragePolicyStatus::Retired,
    }
}

const fn map_status_to_api(value: ItemStoragePolicyStatus) -> ApiStatus {
    match value {
        ItemStoragePolicyStatus::Active => ApiStatus::Active,
        ItemStoragePolicyStatus::Retired => ApiStatus::Retired,
    }
}

fn cursor_filter(request: &ItemStoragePolicyPageRequest) -> String {
    format!(
        "{}.{}.{}.{}.{}",
        encoded_id(request.inventory_owner_id),
        encoded_id(request.facility_id),
        encoded_id(request.item_id),
        request.purpose.map_or("all", purpose_name),
        request.status.map_or("active", status_name),
    )
}

fn encoded_id(value: Option<i64>) -> String {
    value.map_or_else(|| "-".to_owned(), |id| format!("{id:016x}"))
}

const fn purpose_name(value: ApiPurpose) -> &'static str {
    match value {
        ApiPurpose::Receiving => "receiving",
        ApiPurpose::Reserve => "reserve",
        ApiPurpose::Pick => "pick",
        ApiPurpose::Staging => "staging",
        ApiPurpose::Packing => "packing",
        ApiPurpose::Shipping => "shipping",
        ApiPurpose::Quarantine => "quarantine",
        ApiPurpose::Damage => "damage",
    }
}

const fn status_name(value: ApiStatus) -> &'static str {
    match value {
        ApiStatus::Active => "active",
        ApiStatus::Retired => "retired",
    }
}

fn encode_cursor(
    cursor: ItemStoragePolicyCursor,
    request: &ItemStoragePolicyPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        cursor.after_item_storage_policy_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid item storage policy cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &ItemStoragePolicyPageRequest,
) -> V1Result<ItemStoragePolicyCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("item storage policy"))?;
    let (filter, id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("item storage policy"))?;
    if filter != cursor_filter(request) || id.len() != 16 {
        return Err(V1Error::invalid_cursor_for("item storage policy"));
    }
    let id = i64::from_str_radix(id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("item storage policy"))?;
    Ok(ItemStoragePolicyCursor {
        after_item_storage_policy_id: ItemStoragePolicyId::new(id)
            .map_err(|_| V1Error::invalid_cursor_for("item storage policy"))?,
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
        let request = ItemStoragePolicyPageRequest {
            inventory_owner_id: Some(2),
            facility_id: Some(3),
            item_id: Some(4),
            purpose: Some(ApiPurpose::Pick),
            status: None,
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = ItemStoragePolicyCursor {
            after_item_storage_policy_id: ItemStoragePolicyId::new(9).unwrap(),
        };
        let encoded = encode_cursor(cursor, &request).unwrap();
        assert_eq!(decode_cursor(&encoded, &request).unwrap(), cursor);
        let mut changed = request;
        changed.purpose = Some(ApiPurpose::Reserve);
        assert!(decode_cursor(&encoded, &changed).is_err());
    }
}
