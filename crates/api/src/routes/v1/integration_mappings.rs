use axum::extract::{Path, Query, State};
use axum::Json;
use sha2::{Digest, Sha256};
use wareboxes_api_contract::v1::{
    ConfigureIntegrationOrderItemMappingRequest, IntegrationOrderItemMappingPage as ApiPage,
    IntegrationOrderItemMappingPageRequest, IntegrationOrderItemMappingResponse,
    IntegrationOrderItemMappingStatus as ApiStatus, OpaqueCursor,
    RetireIntegrationOrderItemMappingRequest, Revision,
};
use wareboxes_application::integration_mapping::{
    ConfigureIntegrationOrderItemMappingCommand, IntegrationOrderItemMappingCursor,
    IntegrationOrderItemMappingPageQuery, IntegrationOrderItemMappingReadModel,
    RetireIntegrationOrderItemMappingCommand,
};
use wareboxes_domain::{
    CatalogItemId, ExternalItemKey, ExternalItemUom, IntegrationMappedUom,
    IntegrationOrderItemMappingDefinition, IntegrationOrderItemMappingId,
    IntegrationOrderItemMappingRevision, IntegrationOrderItemMappingStatus, IntegrationSourceKey,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "admin";
const CURSOR_PREFIX: &str = "iom1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<IntegrationOrderItemMappingPageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| wareboxes_domain::InventoryOwnerId::new(id).map_err(validation))
        .transpose()?;
    let item_id = request
        .item_id
        .map(|id| CatalogItemId::new(id).map_err(validation))
        .transpose()?;
    let source_key = request
        .source_key
        .as_ref()
        .map(|value| IntegrationSourceKey::new(value.clone()).map_err(validation))
        .transpose()?
        .map(|value| value.as_str().to_owned());
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let page = repo::integration_mapping::page(
        &state.db,
        &user.tenant,
        IntegrationOrderItemMappingPageQuery {
            inventory_owner_id,
            source_key,
            item_id,
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
    Ok(Json(ApiPage::new(
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
    Json(body): Json<ConfigureIntegrationOrderItemMappingRequest>,
) -> V1Result<Json<IntegrationOrderItemMappingResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ConfigureIntegrationOrderItemMappingCommand {
        definition: IntegrationOrderItemMappingDefinition {
            tenant_id: user.tenant.tenant_id,
            inventory_owner_id: wareboxes_domain::InventoryOwnerId::new(body.inventory_owner_id)
                .map_err(validation)?,
            source_key: IntegrationSourceKey::new(body.source_key).map_err(validation)?,
            external_item_key: ExternalItemKey::new(body.external_item_key).map_err(validation)?,
            external_uom: ExternalItemUom::new(body.external_uom).map_err(validation)?,
            item_id: CatalogItemId::new(body.item_id).map_err(validation)?,
            requested_uom: IntegrationMappedUom::new(body.requested_uom).map_err(validation)?,
        },
        expected_revision: body
            .expected_revision
            .map(|revision| {
                IntegrationOrderItemMappingRevision::new(revision.get()).map_err(validation)
            })
            .transpose()?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::integration_mapping::configure(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_response(result)?))
}

pub async fn retire(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(mapping_id): Path<i64>,
    Json(body): Json<RetireIntegrationOrderItemMappingRequest>,
) -> V1Result<Json<IntegrationOrderItemMappingResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = RetireIntegrationOrderItemMappingCommand {
        mapping_id: IntegrationOrderItemMappingId::new(mapping_id).map_err(validation)?,
        expected_revision: IntegrationOrderItemMappingRevision::new(body.expected_revision.get())
            .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::integration_mapping::retire(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_response(result)?))
}

fn map_response(
    value: IntegrationOrderItemMappingReadModel,
) -> V1Result<IntegrationOrderItemMappingResponse> {
    Ok(IntegrationOrderItemMappingResponse {
        mapping_id: value.mapping_id.get(),
        inventory_owner_id: value.definition.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        source_key: value.definition.source_key.as_str().to_owned(),
        external_item_key: value.definition.external_item_key.as_str().to_owned(),
        external_uom: value.definition.external_uom.as_str().to_owned(),
        item_id: value.definition.item_id.get(),
        item_description: value.item_description,
        requested_uom: value.definition.requested_uom.as_str().to_owned(),
        status: map_status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        retired_by: value.retired_by.map(|user| user.get()),
        retired_at: value.retired_at.map(|time| time.to_rfc3339()),
    })
}

const fn map_status(value: ApiStatus) -> IntegrationOrderItemMappingStatus {
    match value {
        ApiStatus::Active => IntegrationOrderItemMappingStatus::Active,
        ApiStatus::Retired => IntegrationOrderItemMappingStatus::Retired,
    }
}

const fn map_status_to_api(value: IntegrationOrderItemMappingStatus) -> ApiStatus {
    match value {
        IntegrationOrderItemMappingStatus::Active => ApiStatus::Active,
        IntegrationOrderItemMappingStatus::Retired => ApiStatus::Retired,
    }
}

fn cursor_filter(request: &IntegrationOrderItemMappingPageRequest) -> String {
    let canonical = format!(
        "{}|{}|{}|{}",
        request
            .inventory_owner_id
            .map_or_else(|| "-".into(), |id| id.to_string()),
        request.source_key.as_deref().unwrap_or("-"),
        request
            .item_id
            .map_or_else(|| "-".into(), |id| id.to_string()),
        request.status.map_or("active", status_name),
    );
    let digest = Sha256::digest(canonical.as_bytes());
    hex::encode(&digest[..8])
}

const fn status_name(value: ApiStatus) -> &'static str {
    match value {
        ApiStatus::Active => "active",
        ApiStatus::Retired => "retired",
    }
}

fn encode_cursor(
    cursor: IntegrationOrderItemMappingCursor,
    request: &IntegrationOrderItemMappingPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        cursor.after_mapping_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid integration mapping cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &IntegrationOrderItemMappingPageRequest,
) -> V1Result<IntegrationOrderItemMappingCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("integration order item mapping"))?;
    let (filter, id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("integration order item mapping"))?;
    if filter != cursor_filter(request) || id.len() != 16 {
        return Err(V1Error::invalid_cursor_for(
            "integration order item mapping",
        ));
    }
    let id = i64::from_str_radix(id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("integration order item mapping"))?;
    Ok(IntegrationOrderItemMappingCursor {
        after_mapping_id: IntegrationOrderItemMappingId::new(id)
            .map_err(|_| V1Error::invalid_cursor_for("integration order item mapping"))?,
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
    fn cursor_is_filter_bound() {
        let request = IntegrationOrderItemMappingPageRequest {
            inventory_owner_id: Some(7),
            source_key: Some("acme-edi".into()),
            item_id: None,
            status: None,
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = IntegrationOrderItemMappingCursor {
            after_mapping_id: IntegrationOrderItemMappingId::new(9).unwrap(),
        };
        let encoded = encode_cursor(cursor, &request).unwrap();
        assert_eq!(decode_cursor(&encoded, &request).unwrap(), cursor);
        let mut changed = request;
        changed.source_key = Some("other".into());
        assert!(decode_cursor(&encoded, &changed).is_err());
    }
}
