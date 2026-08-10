use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CreateInventoryRecallRequest, InventoryRecallPage as ApiRecallPage, InventoryRecallPageRequest,
    InventoryRecallReason as ApiReason, InventoryRecallResponse,
    InventoryRecallStatus as ApiStatus, OpaqueCursor, ReleaseInventoryRecallRequest, Revision,
};
use wareboxes_application::inventory_recall::{
    CreateInventoryRecallCommand, InventoryRecallCursor, InventoryRecallPageQuery,
    InventoryRecallReadModel, ReleaseInventoryRecallCommand,
};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, InventoryRecallDetails, InventoryRecallId, InventoryRecallNote,
    InventoryRecallReason, InventoryRecallRevision, InventoryRecallStatus, ItemBatchId,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms_supervisor";
const CURSOR_PREFIX: &str = "ir1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<InventoryRecallPageRequest>,
) -> V1Result<Json<ApiRecallPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| {
            user.require_inventory_owner(id)?;
            InventoryOwnerId::new(id).map_err(validation)
        })
        .transpose()?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let page = repo::inventory_recall::inventory_recall_page(
        &state.db,
        &user.tenant,
        &InventoryRecallPageQuery {
            facility_id,
            inventory_owner_id,
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
    Ok(Json(ApiRecallPage::new(
        page.items
            .into_iter()
            .map(map_response)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateInventoryRecallRequest>,
) -> V1Result<Json<InventoryRecallResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let facility_id = FacilityId::new(body.facility_id).map_err(validation)?;
    let note = body
        .note
        .map(InventoryRecallNote::new)
        .transpose()
        .map_err(validation)?;
    let details = InventoryRecallDetails::new(map_reason(body.reason), note).map_err(validation)?;
    let command = CreateInventoryRecallCommand {
        facility_id,
        item_batch_id: ItemBatchId::new(body.item_batch_id).map_err(validation)?,
        details,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::inventory_recall::create_inventory_recall(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(recall_id): Path<i64>,
    Json(body): Json<ReleaseInventoryRecallRequest>,
) -> V1Result<Json<InventoryRecallResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ReleaseInventoryRecallCommand {
        recall_id: InventoryRecallId::new(recall_id).map_err(validation)?,
        expected_revision: InventoryRecallRevision::new(body.expected_revision.get())
            .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::inventory_recall::release_inventory_recall(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

fn map_response(value: InventoryRecallReadModel) -> V1Result<InventoryRecallResponse> {
    Ok(InventoryRecallResponse {
        recall_id: value.recall_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        item_batch_id: value.item_batch_id.get(),
        item_id: value.item_id,
        primary_sku: value.primary_sku,
        item_description: value.item_description,
        uom: value.uom,
        lot: value.lot,
        expiration: value.expiration.map(|timestamp| timestamp.to_rfc3339()),
        serial: value.serial,
        status: map_status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        reason: map_reason_to_api(value.details.reason()),
        note: value.details.note().map(|note| note.as_str().to_owned()),
        affected_position_count: value.affected_position_count,
        held_quantity: value.held_quantity,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        released_by: value.released_by.map(|user| user.get()),
        released_at: value.released_at.map(|timestamp| timestamp.to_rfc3339()),
    })
}

const fn map_reason(value: ApiReason) -> InventoryRecallReason {
    match value {
        ApiReason::Regulatory => InventoryRecallReason::Regulatory,
        ApiReason::SupplierNotice => InventoryRecallReason::SupplierNotice,
        ApiReason::CustomerRequest => InventoryRecallReason::CustomerRequest,
        ApiReason::QualityConcern => InventoryRecallReason::QualityConcern,
        ApiReason::Other => InventoryRecallReason::Other,
    }
}

const fn map_reason_to_api(value: InventoryRecallReason) -> ApiReason {
    match value {
        InventoryRecallReason::Regulatory => ApiReason::Regulatory,
        InventoryRecallReason::SupplierNotice => ApiReason::SupplierNotice,
        InventoryRecallReason::CustomerRequest => ApiReason::CustomerRequest,
        InventoryRecallReason::QualityConcern => ApiReason::QualityConcern,
        InventoryRecallReason::Other => ApiReason::Other,
    }
}

const fn map_status(value: ApiStatus) -> InventoryRecallStatus {
    match value {
        ApiStatus::Active => InventoryRecallStatus::Active,
        ApiStatus::Released => InventoryRecallStatus::Released,
    }
}

const fn map_status_to_api(value: InventoryRecallStatus) -> ApiStatus {
    match value {
        InventoryRecallStatus::Active => ApiStatus::Active,
        InventoryRecallStatus::Released => ApiStatus::Released,
    }
}

fn cursor_filter(request: &InventoryRecallPageRequest) -> String {
    format!(
        "{}.{}.{}",
        request
            .facility_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        match request.status {
            None => "all",
            Some(ApiStatus::Active) => "active",
            Some(ApiStatus::Released) => "released",
        }
    )
}

fn encode_cursor(
    cursor: InventoryRecallCursor,
    request: &InventoryRecallPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        cursor.before_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid inventory recall cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &InventoryRecallPageRequest,
) -> V1Result<InventoryRecallCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("inventory recall"))?;
    let (filter, id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("inventory recall"))?;
    if filter != cursor_filter(request) || id.len() != 16 {
        return Err(V1Error::invalid_cursor_for("inventory recall"));
    }
    let id =
        i64::from_str_radix(id, 16).map_err(|_| V1Error::invalid_cursor_for("inventory recall"))?;
    Ok(InventoryRecallCursor {
        before_id: InventoryRecallId::new(id)
            .map_err(|_| V1Error::invalid_cursor_for("inventory recall"))?,
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
        let request = InventoryRecallPageRequest {
            facility_id: Some(4),
            inventory_owner_id: None,
            status: Some(ApiStatus::Active),
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = encode_cursor(
            InventoryRecallCursor {
                before_id: InventoryRecallId::new(9).unwrap(),
            },
            &request,
        )
        .unwrap();
        assert_eq!(decode_cursor(&cursor, &request).unwrap().before_id.get(), 9);
        let mut changed = request;
        changed.status = Some(ApiStatus::Released);
        assert!(decode_cursor(&cursor, &changed).is_err());
    }
}
