use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use wareboxes_api_contract::v1::{
    ChangeServiceAccountStatusRequest, CreateServiceAccountRequest,
    IssueServiceAccountCredentialRequest, IssuedServiceAccountCredentialResponse, OpaqueCursor,
    RevokeServiceAccountCredentialRequest, ServiceAccountAccessRequest,
    ServiceAccountCredentialResponse, ServiceAccountEventPage as ApiEventPage,
    ServiceAccountEventPageRequest, ServiceAccountEventResponse, ServiceAccountOptionsResponse,
    ServiceAccountPage as ApiPage, ServiceAccountPageRequest, ServiceAccountResponse,
    ServiceAccountStatus as ApiStatus, UpdateServiceAccountAccessRequest,
};
use wareboxes_application::service_account::{
    ChangeServiceAccountStatusCommand, CreateServiceAccountCommand,
    IssueServiceAccountCredentialCommand, IssuedServiceAccountCredential,
    RevokeServiceAccountCredentialCommand, ServiceAccountCursor, ServiceAccountEventCursor,
    ServiceAccountEventPageQuery, ServiceAccountEventReadModel, ServiceAccountPageQuery,
    ServiceAccountReadModel, UpdateServiceAccountAccessCommand,
};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, ServiceAccountAccessPolicy, ServiceAccountBearerToken,
    ServiceAccountCredentialId, ServiceAccountCredentialLabel, ServiceAccountDescription,
    ServiceAccountId, ServiceAccountName, ServiceAccountReason, ServiceAccountRevision,
    ServiceAccountStatus as DomainStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const ADMIN_PERMISSION: &str = "admin";
const CURSOR_PREFIX: &str = "sac1.";
const EVENT_CURSOR_PREFIX: &str = "sae1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<ServiceAccountPageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, request.status))
        .transpose()?;
    let page = repo::service_accounts::page(
        &state.db,
        &user.tenant,
        &ServiceAccountPageQuery {
            status: request.status.map(map_status),
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_cursor(cursor, request.status))
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
    Path(service_account_id): Path<i64>,
) -> V1Result<Json<ServiceAccountResponse>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    let result = repo::service_accounts::by_id(
        &state.db,
        &user.tenant,
        ServiceAccountId::new(service_account_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn events(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(service_account_id): Path<i64>,
    Query(request): Query<ServiceAccountEventPageRequest>,
) -> V1Result<Json<ApiEventPage>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    let service_account_id = ServiceAccountId::new(service_account_id).map_err(validation)?;
    let cursor = request
        .cursor
        .as_ref()
        .map(decode_event_cursor)
        .transpose()?;
    let page = repo::service_accounts::event_page(
        &state.db,
        &user.tenant,
        &ServiceAccountEventPageQuery {
            service_account_id,
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

pub async fn options(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<ServiceAccountOptionsResponse>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    Ok(Json(ServiceAccountOptionsResponse {
        permission_names: repo::service_accounts::permission_options(&state.db, &user.tenant)
            .await?,
        can_delegate_all_facilities: user.tenant.site_scope.all_facilities,
        can_delegate_all_inventory_owners: user.tenant.owner_scope.all_inventory_owners,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateServiceAccountRequest>,
) -> V1Result<Json<ServiceAccountResponse>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    let command = CreateServiceAccountCommand {
        tenant_id: user.tenant.tenant_id,
        name: ServiceAccountName::new(body.name).map_err(validation)?,
        description: body
            .description
            .map(ServiceAccountDescription::new)
            .transpose()
            .map_err(validation)?,
        access: map_access(body.access)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::service_accounts::create(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_response(result)?))
}

pub async fn update_access(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(service_account_id): Path<i64>,
    Json(body): Json<UpdateServiceAccountAccessRequest>,
) -> V1Result<Json<ServiceAccountResponse>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    let command = UpdateServiceAccountAccessCommand {
        service_account_id: ServiceAccountId::new(service_account_id).map_err(validation)?,
        expected_revision: ServiceAccountRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        access: map_access(body.access)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::service_accounts::update_access(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_response(result)?))
}

pub async fn change_status(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(service_account_id): Path<i64>,
    Json(body): Json<ChangeServiceAccountStatusRequest>,
) -> V1Result<Json<ServiceAccountResponse>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    let command = ChangeServiceAccountStatusCommand {
        service_account_id: ServiceAccountId::new(service_account_id).map_err(validation)?,
        expected_revision: ServiceAccountRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        status: map_status(body.status),
        reason: ServiceAccountReason::new(body.reason).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::service_accounts::change_status(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_response(result)?))
}

pub async fn issue_credential(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(service_account_id): Path<i64>,
    Json(body): Json<IssueServiceAccountCredentialRequest>,
) -> V1Result<Json<IssuedServiceAccountCredentialResponse>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    let expires_at = body
        .expires_at
        .as_deref()
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| AppError::bad_request("expires_at must be an RFC 3339 timestamp"))?
        .map(|value| value.with_timezone(&chrono::Utc));
    let command = IssueServiceAccountCredentialCommand {
        service_account_id: ServiceAccountId::new(service_account_id).map_err(validation)?,
        expected_revision: ServiceAccountRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        label: ServiceAccountCredentialLabel::new(body.label).map_err(validation)?,
        expires_at,
        bearer_token: ServiceAccountBearerToken::new(body.bearer_token).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let issued =
        repo::service_accounts::issue_credential(&state.db, &user.tenant, &context, &command)
            .await?;
    map_issued(issued).map(Json)
}

pub async fn revoke_credential(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((service_account_id, credential_id)): Path<(i64, i64)>,
    Json(body): Json<RevokeServiceAccountCredentialRequest>,
) -> V1Result<Json<ServiceAccountResponse>> {
    user.require_permission(&state.db, ADMIN_PERMISSION).await?;
    let command = RevokeServiceAccountCredentialCommand {
        service_account_id: ServiceAccountId::new(service_account_id).map_err(validation)?,
        credential_id: ServiceAccountCredentialId::new(credential_id).map_err(validation)?,
        expected_revision: ServiceAccountRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        reason: ServiceAccountReason::new(body.reason).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::service_accounts::revoke_credential(&state.db, &user.tenant, &context, &command)
            .await?;
    Ok(Json(map_response(result)?))
}

fn map_access(value: ServiceAccountAccessRequest) -> V1Result<ServiceAccountAccessPolicy> {
    let mut permission_names = value.permission_names;
    permission_names.sort();
    let mut facility_ids = value
        .facility_ids
        .into_iter()
        .map(|id| FacilityId::new(id).map_err(validation))
        .collect::<V1Result<Vec<_>>>()?;
    facility_ids.sort_by_key(|id| id.get());
    let mut inventory_owner_ids = value
        .inventory_owner_ids
        .into_iter()
        .map(|id| InventoryOwnerId::new(id).map_err(validation))
        .collect::<V1Result<Vec<_>>>()?;
    inventory_owner_ids.sort_by_key(|id| id.get());
    let access = ServiceAccountAccessPolicy {
        all_facilities: value.all_facilities,
        facility_ids,
        all_inventory_owners: value.all_inventory_owners,
        inventory_owner_ids,
        permission_names,
    };
    access.validate().map_err(validation)?;
    Ok(access)
}

fn map_response(value: ServiceAccountReadModel) -> V1Result<ServiceAccountResponse> {
    Ok(ServiceAccountResponse {
        service_account_id: value.service_account_id.get(),
        name: value.name,
        description: value.description,
        status: map_status_to_api(value.status),
        revision: wareboxes_api_contract::v1::Revision::new(value.revision.get())
            .map_err(invalid_result)?,
        access: ServiceAccountAccessRequest {
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
        created_at: value.created_at.to_rfc3339(),
        created_by: value.created_by.get(),
        updated_at: value.updated_at.to_rfc3339(),
        updated_by: value.updated_by.get(),
        disabled_at: value.disabled_at.map(|value| value.to_rfc3339()),
        disabled_by: value.disabled_by.map(|value| value.get()),
        disabled_reason: value.disabled_reason,
        last_used_at: value.last_used_at.map(|value| value.to_rfc3339()),
        credentials: value.credentials.into_iter().map(map_credential).collect(),
    })
}

fn map_credential(
    value: wareboxes_application::service_account::ServiceAccountCredentialReadModel,
) -> ServiceAccountCredentialResponse {
    ServiceAccountCredentialResponse {
        credential_id: value.credential_id.get(),
        label: value.label,
        token_prefix: value.token_prefix,
        created_at: value.created_at.to_rfc3339(),
        created_by: value.created_by.get(),
        expires_at: value.expires_at.map(|value| value.to_rfc3339()),
        revoked_at: value.revoked_at.map(|value| value.to_rfc3339()),
        revoked_by: value.revoked_by.map(|value| value.get()),
        revocation_reason: value.revocation_reason,
        last_used_at: value.last_used_at.map(|value| value.to_rfc3339()),
    }
}

fn map_issued(
    value: IssuedServiceAccountCredential,
) -> V1Result<IssuedServiceAccountCredentialResponse> {
    Ok(IssuedServiceAccountCredentialResponse {
        service_account: map_response(value.service_account)?,
        credential: map_credential(value.credential),
    })
}

fn map_event(value: ServiceAccountEventReadModel) -> V1Result<ServiceAccountEventResponse> {
    Ok(ServiceAccountEventResponse {
        event_id: value.event_id,
        service_account_id: value.service_account_id.get(),
        credential_id: value.credential_id.map(ServiceAccountCredentialId::get),
        action: value.action,
        account_revision: wareboxes_api_contract::v1::Revision::new(value.account_revision.get())
            .map_err(invalid_result)?,
        actor_id: value.actor_id.get(),
        occurred_at: value.occurred_at.to_rfc3339(),
        evidence: value.evidence,
    })
}

const fn map_status(value: ApiStatus) -> DomainStatus {
    match value {
        ApiStatus::Active => DomainStatus::Active,
        ApiStatus::Disabled => DomainStatus::Disabled,
    }
}

const fn map_status_to_api(value: DomainStatus) -> ApiStatus {
    match value {
        DomainStatus::Active => ApiStatus::Active,
        DomainStatus::Disabled => ApiStatus::Disabled,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPayload {
    created_at: String,
    service_account_id: i64,
    status: Option<ApiStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EventCursorPayload {
    occurred_at: String,
    event_id: i64,
}

fn encode_event_cursor(cursor: ServiceAccountEventCursor) -> V1Result<OpaqueCursor> {
    let bytes = serde_json::to_vec(&EventCursorPayload {
        occurred_at: cursor.after_occurred_at.to_rfc3339(),
        event_id: cursor.after_event_id,
    })
    .map_err(invalid_result)?;
    OpaqueCursor::new(format!("{EVENT_CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| V1Error::from(AppError::internal(error.to_string())))
}

fn decode_event_cursor(cursor: &OpaqueCursor) -> V1Result<ServiceAccountEventCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(EVENT_CURSOR_PREFIX)
        .ok_or_else(|| AppError::bad_request("service account event cursor is invalid"))?;
    let bytes = hex::decode(encoded)
        .map_err(|_| AppError::bad_request("service account event cursor is invalid"))?;
    let payload: EventCursorPayload = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::bad_request("service account event cursor is invalid"))?;
    let occurred_at = chrono::DateTime::parse_from_rfc3339(&payload.occurred_at)
        .map_err(|_| AppError::bad_request("service account event cursor is invalid"))?
        .with_timezone(&chrono::Utc);
    if payload.event_id <= 0 {
        return Err(AppError::bad_request("service account event cursor is invalid").into());
    }
    Ok(ServiceAccountEventCursor {
        after_occurred_at: occurred_at,
        after_event_id: payload.event_id,
    })
}

fn encode_cursor(
    cursor: ServiceAccountCursor,
    status: Option<ApiStatus>,
) -> V1Result<OpaqueCursor> {
    let payload = CursorPayload {
        created_at: cursor.after_created_at.to_rfc3339(),
        service_account_id: cursor.after_service_account_id.get(),
        status,
    };
    let bytes = serde_json::to_vec(&payload).map_err(invalid_result)?;
    OpaqueCursor::new(format!("{CURSOR_PREFIX}{}", hex::encode(bytes)))
        .map_err(|error| V1Error::from(AppError::internal(error.to_string())))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    expected_status: Option<ApiStatus>,
) -> V1Result<ServiceAccountCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| AppError::bad_request("service account cursor is invalid"))?;
    let bytes = hex::decode(encoded)
        .map_err(|_| AppError::bad_request("service account cursor is invalid"))?;
    let payload: CursorPayload = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::bad_request("service account cursor is invalid"))?;
    if payload.status != expected_status {
        return Err(AppError::bad_request("service account cursor does not match filters").into());
    }
    let created_at = chrono::DateTime::parse_from_rfc3339(&payload.created_at)
        .map_err(|_| AppError::bad_request("service account cursor is invalid"))?
        .with_timezone(&chrono::Utc);
    Ok(ServiceAccountCursor {
        after_created_at: created_at,
        after_service_account_id: ServiceAccountId::new(payload.service_account_id)
            .map_err(validation)?,
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
    fn service_account_cursor_is_bound_to_status_filter() {
        let cursor = ServiceAccountCursor {
            after_created_at: chrono::DateTime::parse_from_rfc3339("2026-08-15T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            after_service_account_id: ServiceAccountId::new(17).unwrap(),
        };
        let encoded = encode_cursor(cursor, Some(ApiStatus::Active)).unwrap();
        assert_eq!(
            decode_cursor(&encoded, Some(ApiStatus::Active)).unwrap(),
            cursor
        );
        assert!(decode_cursor(&encoded, Some(ApiStatus::Disabled)).is_err());
    }
}
