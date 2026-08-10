use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ConfigureStorageZoneRequest, OpaqueCursor, RetireStorageZoneRequest, Revision,
    StorageZoneLocationResponse, StorageZonePage as ApiStorageZonePage, StorageZonePageRequest,
    StorageZonePurpose as ApiPurpose, StorageZoneResponse, StorageZoneStatus as ApiStatus,
};
use wareboxes_application::storage_zone::{
    ConfigureStorageZoneCommand, RetireStorageZoneCommand, StorageZoneCursor, StorageZonePageQuery,
    StorageZoneReadModel,
};
use wareboxes_domain::{
    FacilityId, LocationId, StorageZoneCode, StorageZoneDefinition, StorageZoneId,
    StorageZoneLocationIds, StorageZoneName, StorageZonePurpose, StorageZoneRevision,
    StorageZoneStatus, StorageZoneTravelSequence,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const READ_PERMISSION: &str = "wms";
const MUTATE_PERMISSION: &str = "wms_supervisor";
const CURSOR_PREFIX: &str = "sz1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<StorageZonePageRequest>,
) -> V1Result<Json<ApiStorageZonePage>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let page = repo::storage_zone::storage_zone_page(
        &state.db,
        &user.tenant,
        StorageZonePageQuery {
            facility_id,
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
    Ok(Json(ApiStorageZonePage::new(
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
    Json(body): Json<ConfigureStorageZoneRequest>,
) -> V1Result<Json<StorageZoneResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let facility_id = FacilityId::new(body.facility_id).map_err(validation)?;
    let location_ids = body
        .location_ids
        .into_iter()
        .map(|id| LocationId::new(id).map_err(validation))
        .collect::<V1Result<Vec<_>>>()?;
    let command = ConfigureStorageZoneCommand {
        definition: StorageZoneDefinition {
            tenant_id: user.tenant.tenant_id,
            facility_id,
            code: StorageZoneCode::new(body.code).map_err(validation)?,
            name: StorageZoneName::new(body.name).map_err(validation)?,
            purpose: map_purpose(body.purpose),
            travel_sequence: StorageZoneTravelSequence::new(body.travel_sequence),
            location_ids: StorageZoneLocationIds::new(location_ids).map_err(validation)?,
        },
        expected_revision: body
            .expected_revision
            .map(|revision| StorageZoneRevision::new(revision.get()).map_err(validation))
            .transpose()?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::storage_zone::configure_storage_zone(&state.db, &user.tenant, &context, &command)
            .await?;
    Ok(Json(map_response(result)?))
}

pub async fn retire(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(storage_zone_id): Path<i64>,
    Json(body): Json<RetireStorageZoneRequest>,
) -> V1Result<Json<StorageZoneResponse>> {
    user.require_permission(&state.db, MUTATE_PERMISSION)
        .await?;
    let command = RetireStorageZoneCommand {
        storage_zone_id: StorageZoneId::new(storage_zone_id).map_err(validation)?,
        expected_revision: StorageZoneRevision::new(body.expected_revision.get())
            .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::storage_zone::retire_storage_zone(&state.db, &user.tenant, &context, &command)
            .await?;
    Ok(Json(map_response(result)?))
}

fn map_response(value: StorageZoneReadModel) -> V1Result<StorageZoneResponse> {
    Ok(StorageZoneResponse {
        storage_zone_id: value.storage_zone_id.get(),
        facility_id: value.definition.facility_id.get(),
        facility_name: value.facility_name,
        code: value.definition.code.as_str().to_owned(),
        name: value.definition.name.as_str().to_owned(),
        purpose: map_purpose_to_api(value.definition.purpose),
        travel_sequence: value.definition.travel_sequence.get(),
        status: map_status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        locations: value
            .locations
            .into_iter()
            .map(|location| StorageZoneLocationResponse {
                location_id: location.location_id.get(),
                barcode: location.barcode,
                name: location.name,
                location_type: location.location_type,
                pickable: location.pickable,
                receivable: location.receivable,
            })
            .collect(),
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        retired_by: value.retired_by.map(|user| user.get()),
        retired_at: value.retired_at.map(|timestamp| timestamp.to_rfc3339()),
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

const fn map_status(value: ApiStatus) -> StorageZoneStatus {
    match value {
        ApiStatus::Active => StorageZoneStatus::Active,
        ApiStatus::Retired => StorageZoneStatus::Retired,
    }
}

const fn map_status_to_api(value: StorageZoneStatus) -> ApiStatus {
    match value {
        StorageZoneStatus::Active => ApiStatus::Active,
        StorageZoneStatus::Retired => ApiStatus::Retired,
    }
}

fn cursor_filter(request: &StorageZonePageRequest) -> String {
    format!(
        "{}.{}.{}",
        request
            .facility_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request.purpose.map_or("all", purpose_name),
        request.status.map_or("active", status_name),
    )
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
    cursor: StorageZoneCursor,
    request: &StorageZonePageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:08x}.{:016x}",
        cursor_filter(request),
        cursor.after_travel_sequence.get(),
        cursor.after_storage_zone_id.get(),
    ))
    .map_err(|_| V1Error::internal("generated an invalid storage zone cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &StorageZonePageRequest,
) -> V1Result<StorageZoneCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("storage zone"))?;
    let mut parts = encoded.rsplitn(3, '.');
    let id = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("storage zone"))?;
    let sequence = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("storage zone"))?;
    let filter = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("storage zone"))?;
    if filter != cursor_filter(request) || sequence.len() != 8 || id.len() != 16 {
        return Err(V1Error::invalid_cursor_for("storage zone"));
    }
    let sequence = u32::from_str_radix(sequence, 16)
        .map_err(|_| V1Error::invalid_cursor_for("storage zone"))?;
    let id =
        i64::from_str_radix(id, 16).map_err(|_| V1Error::invalid_cursor_for("storage zone"))?;
    Ok(StorageZoneCursor {
        after_travel_sequence: StorageZoneTravelSequence::new(sequence),
        after_storage_zone_id: StorageZoneId::new(id)
            .map_err(|_| V1Error::invalid_cursor_for("storage zone"))?,
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
        let request = StorageZonePageRequest {
            facility_id: Some(2),
            purpose: Some(ApiPurpose::Pick),
            status: None,
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = StorageZoneCursor {
            after_travel_sequence: StorageZoneTravelSequence::new(15),
            after_storage_zone_id: StorageZoneId::new(9).unwrap(),
        };
        let encoded = encode_cursor(cursor, &request).unwrap();
        assert_eq!(decode_cursor(&encoded, &request).unwrap(), cursor);
        let mut changed = request;
        changed.purpose = Some(ApiPurpose::Reserve);
        assert!(decode_cursor(&encoded, &changed).is_err());
    }
}
