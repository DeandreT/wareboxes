use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use wareboxes_api_contract::v1::{
    ChangeTenantStatusRequest, CreateTenantRequest, OpaqueCursor,
    TenantLifecycleEventPage as ApiEventPage, TenantLifecycleEventPageRequest,
    TenantLifecycleEventResponse, TenantLifecyclePage as ApiPage, TenantLifecyclePageRequest,
    TenantLifecycleResponse, TenantStatus as ApiStatus,
};
use wareboxes_application::tenant_lifecycle::{
    ChangeTenantStatusCommand, CreateTenantCommand, TenantLifecycleCursor,
    TenantLifecycleEventCursor, TenantLifecycleEventPageQuery, TenantLifecycleEventReadModel,
    TenantLifecyclePageQuery, TenantLifecycleReadModel,
};
use wareboxes_domain::{
    TenantId, TenantLifecycleReason, TenantName, TenantRevision, TenantSlug,
    TenantStatus as DomainStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const CURSOR_PREFIX: &str = "tlc1.";
const EVENT_CURSOR_PREFIX: &str = "tle1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<TenantLifecyclePageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_platform_administrator(&state.db).await?;
    let search = normalize_search(request.search)?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, request.status, search.as_deref()))
        .transpose()?;
    let page = repo::tenant_lifecycle::page(
        &state.db,
        &user.tenant,
        &TenantLifecyclePageQuery {
            status: request.status.map(map_status),
            search: search.clone(),
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_cursor(cursor, request.status, search.as_deref()))
        .transpose()?;
    Ok(Json(ApiPage::new(
        page.items
            .into_iter()
            .map(map_response)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(tenant_id): Path<i64>,
) -> V1Result<Json<TenantLifecycleResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let result = repo::tenant_lifecycle::by_id(
        &state.db,
        &user.tenant,
        TenantId::new(tenant_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn events(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(tenant_id): Path<i64>,
    Query(request): Query<TenantLifecycleEventPageRequest>,
) -> V1Result<Json<ApiEventPage>> {
    user.require_platform_administrator(&state.db).await?;
    let cursor = request
        .cursor
        .as_ref()
        .map(decode_event_cursor)
        .transpose()?;
    let page = repo::tenant_lifecycle::event_page(
        &state.db,
        &user.tenant,
        &TenantLifecycleEventPageQuery {
            tenant_id: TenantId::new(tenant_id).map_err(validation)?,
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page.next_cursor.map(encode_event_cursor).transpose()?;
    Ok(Json(ApiEventPage::new(
        page.items
            .into_iter()
            .map(map_event)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateTenantRequest>,
) -> V1Result<Json<TenantLifecycleResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let administrator_email = normalize_email(body.administrator_email)?;
    let command = CreateTenantCommand {
        slug: TenantSlug::new(body.slug).map_err(validation)?,
        name: TenantName::new(body.name).map_err(validation)?,
        administrator_email,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::tenant_lifecycle::create(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_response(result)?))
}

pub async fn change_status(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(tenant_id): Path<i64>,
    Json(body): Json<ChangeTenantStatusRequest>,
) -> V1Result<Json<TenantLifecycleResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = ChangeTenantStatusCommand {
        tenant_id: TenantId::new(tenant_id).map_err(validation)?,
        expected_revision: TenantRevision::new(body.expected_revision.get()).map_err(validation)?,
        status: map_status(body.status),
        reason: TenantLifecycleReason::new(body.reason).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::tenant_lifecycle::change_status(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_response(result)?))
}

fn normalize_email(value: String) -> V1Result<String> {
    let value = value.trim().to_ascii_lowercase();
    let Some((local, domain)) = value.split_once('@') else {
        return Err(AppError::bad_request("administrator email is invalid").into());
    };
    if value.len() > 254
        || local.is_empty()
        || domain.is_empty()
        || domain.starts_with('.')
        || domain.ends_with('.')
        || value.chars().any(char::is_control)
    {
        return Err(AppError::bad_request("administrator email is invalid").into());
    }
    Ok(value)
}

fn normalize_search(value: Option<String>) -> V1Result<Option<String>> {
    let value = value.map(|value| value.trim().to_owned());
    match value {
        Some(value) if value.is_empty() => Ok(None),
        Some(value) if value.chars().count() > 120 || value.chars().any(char::is_control) => {
            Err(AppError::bad_request("tenant search is invalid").into())
        }
        value => Ok(value),
    }
}

fn map_response(value: TenantLifecycleReadModel) -> V1Result<TenantLifecycleResponse> {
    Ok(TenantLifecycleResponse {
        tenant_id: value.tenant_id.get(),
        slug: value.slug,
        name: value.name,
        status: map_status_to_api(value.status),
        revision: wareboxes_api_contract::v1::Revision::new(value.revision.get())
            .map_err(invalid_result)?,
        created_at: value.created_at.to_rfc3339(),
        created_by: value.created_by.map(|value| value.get()),
        initial_admin_user_id: value.initial_admin_user_id.map(|value| value.get()),
        initial_admin_email: value.initial_admin_email,
        status_changed_at: value.status_changed_at.map(|value| value.to_rfc3339()),
        status_changed_by: value.status_changed_by.map(|value| value.get()),
        status_reason: value.status_reason,
        active_member_count: value.active_member_count,
        active_facility_count: value.active_facility_count,
        active_inventory_owner_count: value.active_inventory_owner_count,
        active_service_account_count: value.active_service_account_count,
    })
}

#[cfg(feature = "ssr")]
pub(crate) fn map_response_for_web(
    value: TenantLifecycleReadModel,
) -> crate::error::AppResult<TenantLifecycleResponse> {
    map_response(value).map_err(|_| AppError::internal("could not map tenant lifecycle response"))
}

fn map_event(value: TenantLifecycleEventReadModel) -> V1Result<TenantLifecycleEventResponse> {
    Ok(TenantLifecycleEventResponse {
        event_id: value.event_id,
        tenant_id: value.tenant_id.get(),
        action: value.action,
        previous_status: value.previous_status.map(map_status_to_api),
        resulting_status: map_status_to_api(value.resulting_status),
        tenant_revision: wareboxes_api_contract::v1::Revision::new(value.tenant_revision.get())
            .map_err(invalid_result)?,
        actor_id: value.actor_id.get(),
        occurred_at: value.occurred_at.to_rfc3339(),
        reason: value.reason,
        revoked_session_count: value.revoked_session_count,
        revoked_credential_count: value.revoked_credential_count,
        evidence: value.evidence,
    })
}

const fn map_status(value: ApiStatus) -> DomainStatus {
    match value {
        ApiStatus::Active => DomainStatus::Active,
        ApiStatus::Suspended => DomainStatus::Suspended,
    }
}

const fn map_status_to_api(value: DomainStatus) -> ApiStatus {
    match value {
        DomainStatus::Active => ApiStatus::Active,
        DomainStatus::Suspended => ApiStatus::Suspended,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPayload {
    created_at: String,
    tenant_id: i64,
    status: Option<ApiStatus>,
    search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EventCursorPayload {
    occurred_at: String,
    event_id: i64,
}

fn encode_cursor(
    cursor: TenantLifecycleCursor,
    status: Option<ApiStatus>,
    search: Option<&str>,
) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&CursorPayload {
        created_at: cursor.after_created_at.to_rfc3339(),
        tenant_id: cursor.after_tenant_id.get(),
        status,
        search: search.map(str::to_owned),
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| AppError::internal(error.to_string()).into())
}

#[cfg(feature = "ssr")]
pub(crate) fn encode_cursor_for_web(
    cursor: TenantLifecycleCursor,
) -> crate::error::AppResult<OpaqueCursor> {
    encode_cursor(cursor, None, None)
        .map_err(|_| AppError::internal("could not encode tenant lifecycle cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    status: Option<ApiStatus>,
    search: Option<&str>,
) -> V1Result<TenantLifecycleCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| AppError::bad_request("tenant lifecycle cursor is invalid"))?;
    let payload: CursorPayload = serde_json::from_slice(
        &hex::decode(encoded)
            .map_err(|_| AppError::bad_request("tenant lifecycle cursor is invalid"))?,
    )
    .map_err(|_| AppError::bad_request("tenant lifecycle cursor is invalid"))?;
    if payload.status != status || payload.search.as_deref() != search {
        return Err(AppError::bad_request("tenant lifecycle cursor does not match filters").into());
    }
    Ok(TenantLifecycleCursor {
        after_created_at: chrono::DateTime::parse_from_rfc3339(&payload.created_at)
            .map_err(|_| AppError::bad_request("tenant lifecycle cursor is invalid"))?
            .with_timezone(&chrono::Utc),
        after_tenant_id: TenantId::new(payload.tenant_id).map_err(validation)?,
    })
}

fn encode_event_cursor(cursor: TenantLifecycleEventCursor) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&EventCursorPayload {
        occurred_at: cursor.after_occurred_at.to_rfc3339(),
        event_id: cursor.after_event_id,
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{EVENT_CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| AppError::internal(error.to_string()).into())
}

fn decode_event_cursor(cursor: &OpaqueCursor) -> V1Result<TenantLifecycleEventCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(EVENT_CURSOR_PREFIX)
        .ok_or_else(|| AppError::bad_request("tenant lifecycle event cursor is invalid"))?;
    let payload: EventCursorPayload = serde_json::from_slice(
        &hex::decode(encoded)
            .map_err(|_| AppError::bad_request("tenant lifecycle event cursor is invalid"))?,
    )
    .map_err(|_| AppError::bad_request("tenant lifecycle event cursor is invalid"))?;
    if payload.event_id <= 0 {
        return Err(AppError::bad_request("tenant lifecycle event cursor is invalid").into());
    }
    Ok(TenantLifecycleEventCursor {
        after_occurred_at: chrono::DateTime::parse_from_rfc3339(&payload.occurred_at)
            .map_err(|_| AppError::bad_request("tenant lifecycle event cursor is invalid"))?
            .with_timezone(&chrono::Utc),
        after_event_id: payload.event_id,
    })
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    AppError::internal(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_cursor_is_bound_to_every_filter() {
        let cursor = TenantLifecycleCursor {
            after_created_at: chrono::DateTime::parse_from_rfc3339("2026-08-15T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            after_tenant_id: TenantId::new(7).unwrap(),
        };
        let encoded = encode_cursor(cursor, Some(ApiStatus::Active), Some("north")).unwrap();
        assert_eq!(
            decode_cursor(&encoded, Some(ApiStatus::Active), Some("north")).unwrap(),
            cursor
        );
        assert!(decode_cursor(&encoded, Some(ApiStatus::Suspended), Some("north")).is_err());
        assert!(decode_cursor(&encoded, Some(ApiStatus::Active), Some("south")).is_err());
    }
}
