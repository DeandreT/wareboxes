use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use wareboxes_api_contract::v1::{
    ApproveSupportAccessRequest, OpaqueCursor, RejectSupportAccessRequest,
    RequestSupportAccessRequest, RevokeSupportAccessRequest,
    SupportAccessEventPage as ApiEventPage, SupportAccessEventPageRequest,
    SupportAccessEventResponse, SupportAccessOptionsRequest, SupportAccessOptionsResponse,
    SupportAccessPage as ApiPage, SupportAccessPageRequest, SupportAccessPolicyRequest,
    SupportAccessResourceOptionResponse, SupportAccessResponse, SupportAccessStatus as ApiStatus,
};
use wareboxes_application::support_access::{
    ApproveSupportAccessCommand, RejectSupportAccessCommand, RequestSupportAccessCommand,
    RevokeSupportAccessCommand, SupportAccessCursor, SupportAccessEventCursor,
    SupportAccessEventPageQuery, SupportAccessEventReadModel, SupportAccessPageQuery,
    SupportAccessReadModel,
};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, SupportAccessGrantId, SupportAccessPolicy, SupportAccessReason,
    SupportAccessRevision, SupportAccessStatus as DomainStatus, TenantId, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const CURSOR_PREFIX: &str = "sag1.";
const EVENT_CURSOR_PREFIX: &str = "sae1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<SupportAccessPageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_platform_administrator(&state.db).await?;
    let tenant_id = request
        .tenant_id
        .map(TenantId::new)
        .transpose()
        .map_err(validation)?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, tenant_id, request.status))
        .transpose()?;
    let page = repo::support_access::page(
        &state.db,
        &user.tenant,
        &SupportAccessPageQuery {
            tenant_id,
            status: request.status.map(status_from_api),
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_cursor(cursor, tenant_id, request.status))
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
    Path(support_access_grant_id): Path<i64>,
) -> V1Result<Json<SupportAccessResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let result = repo::support_access::by_id(
        &state.db,
        &user.tenant,
        SupportAccessGrantId::new(support_access_grant_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn options(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<SupportAccessOptionsRequest>,
) -> V1Result<Json<SupportAccessOptionsResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let result = repo::support_access::options(
        &state.db,
        &user.tenant,
        TenantId::new(request.tenant_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(SupportAccessOptionsResponse {
        tenant_id: result.tenant_id.get(),
        tenant_name: result.tenant_name,
        facilities: result
            .facilities
            .into_iter()
            .map(|value| SupportAccessResourceOptionResponse {
                id: value.id,
                name: value.name,
            })
            .collect(),
        inventory_owners: result
            .inventory_owners
            .into_iter()
            .map(|value| SupportAccessResourceOptionResponse {
                id: value.id,
                name: value.name,
            })
            .collect(),
        permission_names: result.permission_names,
        max_duration_hours: wareboxes_domain::MAX_SUPPORT_ACCESS_DURATION_HOURS,
    }))
}

pub async fn events(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(support_access_grant_id): Path<i64>,
    Query(request): Query<SupportAccessEventPageRequest>,
) -> V1Result<Json<ApiEventPage>> {
    user.require_platform_administrator(&state.db).await?;
    let cursor = request
        .cursor
        .as_ref()
        .map(decode_event_cursor)
        .transpose()?;
    let page = repo::support_access::event_page(
        &state.db,
        &user.tenant,
        &SupportAccessEventPageQuery {
            support_access_grant_id: SupportAccessGrantId::new(support_access_grant_id)
                .map_err(validation)?,
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

pub async fn request(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RequestSupportAccessRequest>,
) -> V1Result<Json<SupportAccessResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = RequestSupportAccessCommand {
        tenant_id: TenantId::new(body.tenant_id).map_err(validation)?,
        reason: SupportAccessReason::new(body.reason).map_err(validation)?,
        expires_at: parse_timestamp(&body.expires_at)?,
        access: policy_from_api(body.access)?,
    };
    let result = repo::support_access::request(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn approve(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(support_access_grant_id): Path<i64>,
    Json(body): Json<ApproveSupportAccessRequest>,
) -> V1Result<Json<SupportAccessResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = ApproveSupportAccessCommand {
        support_access_grant_id: SupportAccessGrantId::new(support_access_grant_id)
            .map_err(validation)?,
        expected_revision: SupportAccessRevision::new(body.expected_revision.get())
            .map_err(validation)?,
    };
    let result = repo::support_access::approve(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn reject(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(support_access_grant_id): Path<i64>,
    Json(body): Json<RejectSupportAccessRequest>,
) -> V1Result<Json<SupportAccessResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = RejectSupportAccessCommand {
        support_access_grant_id: SupportAccessGrantId::new(support_access_grant_id)
            .map_err(validation)?,
        expected_revision: SupportAccessRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        reason: SupportAccessReason::new(body.reason).map_err(validation)?,
    };
    let result = repo::support_access::reject(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn revoke(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(support_access_grant_id): Path<i64>,
    Json(body): Json<RevokeSupportAccessRequest>,
) -> V1Result<Json<SupportAccessResponse>> {
    user.require_platform_administrator(&state.db).await?;
    let command = RevokeSupportAccessCommand {
        support_access_grant_id: SupportAccessGrantId::new(support_access_grant_id)
            .map_err(validation)?,
        expected_revision: SupportAccessRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        reason: SupportAccessReason::new(body.reason).map_err(validation)?,
    };
    let result = repo::support_access::revoke(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

fn policy_from_api(value: SupportAccessPolicyRequest) -> V1Result<SupportAccessPolicy> {
    let policy = SupportAccessPolicy {
        all_facilities: value.all_facilities,
        facility_ids: value
            .facility_ids
            .into_iter()
            .map(FacilityId::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(validation)?,
        all_inventory_owners: value.all_inventory_owners,
        inventory_owner_ids: value
            .inventory_owner_ids
            .into_iter()
            .map(InventoryOwnerId::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(validation)?,
        permission_names: value.permission_names,
    };
    policy.validate().map_err(validation)?;
    Ok(policy)
}

fn parse_timestamp(value: &str) -> V1Result<Timestamp> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.to_utc())
        .map_err(|_| AppError::bad_request("expires_at must be an RFC 3339 timestamp").into())
}

fn map_response(value: SupportAccessReadModel) -> V1Result<SupportAccessResponse> {
    Ok(SupportAccessResponse {
        support_access_grant_id: value.support_access_grant_id.get(),
        tenant_id: value.tenant_id.get(),
        tenant_slug: value.tenant_slug,
        tenant_name: value.tenant_name,
        status: status_to_api(value.status),
        revision: wareboxes_api_contract::v1::Revision::new(value.revision.get())
            .map_err(invalid_result)?,
        reason: value.reason,
        access: SupportAccessPolicyRequest {
            all_facilities: value.access.all_facilities,
            facility_ids: value
                .access
                .facility_ids
                .into_iter()
                .map(FacilityId::get)
                .collect(),
            all_inventory_owners: value.access.all_inventory_owners,
            inventory_owner_ids: value
                .access
                .inventory_owner_ids
                .into_iter()
                .map(InventoryOwnerId::get)
                .collect(),
            permission_names: value.access.permission_names,
        },
        requested_at: value.requested_at.to_rfc3339(),
        requested_by: value.requested_by.get(),
        requested_by_email: value.requested_by_email,
        expires_at: value.expires_at.to_rfc3339(),
        approved_at: value.approved_at.map(|value| value.to_rfc3339()),
        approved_by: value.approved_by.map(|value| value.get()),
        approved_by_email: value.approved_by_email,
        rejected_at: value.rejected_at.map(|value| value.to_rfc3339()),
        rejected_by: value.rejected_by.map(|value| value.get()),
        rejection_reason: value.rejection_reason,
        revoked_at: value.revoked_at.map(|value| value.to_rfc3339()),
        revoked_by: value.revoked_by.map(|value| value.get()),
        revocation_reason: value.revocation_reason,
    })
}

#[cfg(feature = "ssr")]
pub(crate) fn map_response_for_web(
    value: SupportAccessReadModel,
) -> crate::error::AppResult<SupportAccessResponse> {
    map_response(value).map_err(|_| AppError::internal("could not map support access response"))
}

fn map_event(value: SupportAccessEventReadModel) -> V1Result<SupportAccessEventResponse> {
    Ok(SupportAccessEventResponse {
        event_id: value.event_id,
        support_access_grant_id: value.support_access_grant_id.get(),
        tenant_id: value.tenant_id.get(),
        action: value.action,
        grant_revision: wareboxes_api_contract::v1::Revision::new(value.grant_revision.get())
            .map_err(invalid_result)?,
        actor_id: value.actor_id.get(),
        occurred_at: value.occurred_at.to_rfc3339(),
        reason: value.reason,
        evidence: value.evidence,
    })
}

const fn status_from_api(value: ApiStatus) -> DomainStatus {
    match value {
        ApiStatus::Pending => DomainStatus::Pending,
        ApiStatus::Active => DomainStatus::Active,
        ApiStatus::Rejected => DomainStatus::Rejected,
        ApiStatus::Revoked => DomainStatus::Revoked,
        ApiStatus::Expired => DomainStatus::Expired,
    }
}

const fn status_to_api(value: DomainStatus) -> ApiStatus {
    match value {
        DomainStatus::Pending => ApiStatus::Pending,
        DomainStatus::Active => ApiStatus::Active,
        DomainStatus::Rejected => ApiStatus::Rejected,
        DomainStatus::Revoked => ApiStatus::Revoked,
        DomainStatus::Expired => ApiStatus::Expired,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPayload {
    requested_at: String,
    support_access_grant_id: i64,
    tenant_id: Option<i64>,
    status: Option<ApiStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EventCursorPayload {
    occurred_at: String,
    event_id: i64,
}

fn encode_cursor(
    cursor: SupportAccessCursor,
    tenant_id: Option<TenantId>,
    status: Option<ApiStatus>,
) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&CursorPayload {
        requested_at: cursor.after_requested_at.to_rfc3339(),
        support_access_grant_id: cursor.after_support_access_grant_id.get(),
        tenant_id: tenant_id.map(TenantId::get),
        status,
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| AppError::internal(error.to_string()).into())
}

#[cfg(feature = "ssr")]
pub(crate) fn encode_cursor_for_web(
    cursor: SupportAccessCursor,
) -> crate::error::AppResult<OpaqueCursor> {
    encode_cursor(cursor, None, None)
        .map_err(|_| AppError::internal("could not encode support access cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    tenant_id: Option<TenantId>,
    status: Option<ApiStatus>,
) -> V1Result<SupportAccessCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| AppError::bad_request("support access cursor is invalid"))?;
    let payload: CursorPayload = serde_json::from_slice(
        &hex::decode(encoded)
            .map_err(|_| AppError::bad_request("support access cursor is invalid"))?,
    )
    .map_err(|_| AppError::bad_request("support access cursor is invalid"))?;
    if payload.tenant_id != tenant_id.map(TenantId::get) || payload.status != status {
        return Err(AppError::bad_request("cursor does not match support access filters").into());
    }
    Ok(SupportAccessCursor {
        after_requested_at: parse_timestamp(&payload.requested_at)?,
        after_support_access_grant_id: SupportAccessGrantId::new(payload.support_access_grant_id)
            .map_err(validation)?,
    })
}

fn encode_event_cursor(cursor: SupportAccessEventCursor) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&EventCursorPayload {
        occurred_at: cursor.after_occurred_at.to_rfc3339(),
        event_id: cursor.after_event_id,
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{EVENT_CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| AppError::internal(error.to_string()).into())
}

fn decode_event_cursor(cursor: &OpaqueCursor) -> V1Result<SupportAccessEventCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(EVENT_CURSOR_PREFIX)
        .ok_or_else(|| AppError::bad_request("support access event cursor is invalid"))?;
    let payload: EventCursorPayload = serde_json::from_slice(
        &hex::decode(encoded)
            .map_err(|_| AppError::bad_request("support access event cursor is invalid"))?,
    )
    .map_err(|_| AppError::bad_request("support access event cursor is invalid"))?;
    if payload.event_id <= 0 {
        return Err(AppError::bad_request("support access event cursor is invalid").into());
    }
    Ok(SupportAccessEventCursor {
        after_occurred_at: parse_timestamp(&payload.occurred_at)?,
        after_event_id: payload.event_id,
    })
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    AppError::internal(error.to_string()).into()
}
