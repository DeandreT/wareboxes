use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CancelCarrierManifestRequest, CarrierAccountPage, CarrierAccountPageRequest,
    CarrierAccountResponse, CarrierAccountStatus as ApiCarrierAccountStatus,
    CarrierManifestJobPage, CarrierManifestJobPageRequest, CarrierManifestJobResponse,
    CarrierManifestJobStatus as ApiCarrierManifestJobStatus, ChangeCarrierAccountStatusRequest,
    CreateCarrierAccountRequest, CursorPage, OpaqueCursor, QueueCarrierManifestRequest,
    ReconfigureCarrierAccountRequest, RetryCarrierManifestRequest, Revision,
};
use wareboxes_application::carrier::{
    CancelCarrierManifestCommand, CarrierAccountReadModel, CarrierManifestJobReadModel,
    ChangeCarrierAccountStatusCommand, CreateCarrierAccountCommand, QueueCarrierManifestCommand,
    ReconfigureCarrierAccountCommand, RetryCarrierManifestCommand,
};
use wareboxes_domain::{
    CarrierAccountId, CarrierAccountKey, CarrierAccountName, CarrierAccountStatus, CarrierCode,
    CarrierManifestJobId, CarrierManifestJobStatus, CarrierServiceCode, FacilityId,
    InventoryOwnerId, ShipmentId, ShipmentRevision,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const ACCOUNT_CURSOR_PREFIX: &str = "ca1.";
const JOB_CURSOR_PREFIX: &str = "cmj1.";

pub async fn list_accounts(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<CarrierAccountPageRequest>,
) -> V1Result<Json<CarrierAccountPage>> {
    user.require_permission(&state.db, "wms").await?;
    let owner_id = positive(
        query.inventory_owner_id,
        InventoryOwnerId::new,
        "inventory owner ID",
    )?;
    let facility_id = positive(query.facility_id, FacilityId::new, "facility ID")?;
    let after = query
        .cursor
        .as_ref()
        .map(|cursor| decode_account_cursor(cursor, owner_id, facility_id))
        .transpose()?;
    let page = repo::carrier::list(
        &state.db,
        &user.tenant,
        repo::carrier::CarrierAccountPageFilter {
            inventory_owner_id: owner_id,
            facility_id,
            include_disabled: query.include_disabled,
            after_account_id: after,
            limit: query.limit.get(),
        },
    )
    .await?;
    Ok(Json(CursorPage::new(
        page.items
            .into_iter()
            .map(map_account)
            .collect::<V1Result<_>>()?,
        page.next_account_id
            .map(|id| encode_account_cursor(owner_id, facility_id, id))
            .transpose()?,
    )))
}

pub async fn create_account(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateCarrierAccountRequest>,
) -> V1Result<Json<CarrierAccountResponse>> {
    user.require_permission(&state.db, "admin").await?;
    let command = CreateCarrierAccountCommand {
        inventory_owner_id: positive(
            body.inventory_owner_id,
            InventoryOwnerId::new,
            "inventory owner ID",
        )?,
        facility_id: positive(body.facility_id, FacilityId::new, "facility ID")?,
        display_name: CarrierAccountName::new(body.display_name).map_err(invalid)?,
        carrier_code: CarrierCode::new(body.carrier_code).map_err(invalid)?,
        account_key: CarrierAccountKey::new(body.account_key).map_err(invalid)?,
    };
    let result = repo::carrier::create(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_account(result)?))
}

pub async fn reconfigure_account(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(account_id): Path<i64>,
    Json(body): Json<ReconfigureCarrierAccountRequest>,
) -> V1Result<Json<CarrierAccountResponse>> {
    user.require_permission(&state.db, "admin").await?;
    let command = ReconfigureCarrierAccountCommand {
        account_id: positive(account_id, CarrierAccountId::new, "carrier account ID")?,
        display_name: CarrierAccountName::new(body.display_name).map_err(invalid)?,
        account_key: CarrierAccountKey::new(body.account_key).map_err(invalid)?,
        expected_revision: revision_u32(body.expected_revision, "carrier account revision")?,
    };
    let result = repo::carrier::reconfigure(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_account(result)?))
}

pub async fn change_account_status(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(account_id): Path<i64>,
    Json(body): Json<ChangeCarrierAccountStatusRequest>,
) -> V1Result<Json<CarrierAccountResponse>> {
    user.require_permission(&state.db, "admin").await?;
    let command = ChangeCarrierAccountStatusCommand {
        account_id: positive(account_id, CarrierAccountId::new, "carrier account ID")?,
        status: map_account_status_in(body.status),
        expected_revision: revision_u32(body.expected_revision, "carrier account revision")?,
    };
    let result = repo::carrier::change_status(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_account(result)?))
}

pub async fn list_manifest_jobs(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(shipment_id): Path<i64>,
    Query(query): Query<CarrierManifestJobPageRequest>,
) -> V1Result<Json<CarrierManifestJobPage>> {
    user.require_permission(&state.db, "wms").await?;
    let shipment_id = positive(shipment_id, ShipmentId::new, "shipment ID")?;
    let after = query
        .cursor
        .as_ref()
        .map(|cursor| decode_job_cursor(cursor, shipment_id))
        .transpose()?;
    let page = repo::carrier::list_jobs(
        &state.db,
        &user.tenant,
        repo::carrier::CarrierManifestJobPageFilter {
            shipment_id,
            after_job_id: after,
            limit: query.limit.get(),
        },
    )
    .await?;
    Ok(Json(CursorPage::new(
        page.items
            .into_iter()
            .map(map_job)
            .collect::<V1Result<_>>()?,
        page.next_job_id
            .map(|id| encode_job_cursor(shipment_id, id))
            .transpose()?,
    )))
}

pub async fn queue_manifest(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shipment_id): Path<i64>,
    Json(body): Json<QueueCarrierManifestRequest>,
) -> V1Result<Json<CarrierManifestJobResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = QueueCarrierManifestCommand {
        shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        account_id: positive(body.account_id, CarrierAccountId::new, "carrier account ID")?,
        service_code: body
            .service_code
            .map(CarrierServiceCode::new)
            .transpose()
            .map_err(invalid)?,
        expected_shipment_revision: ShipmentRevision::new(body.expected_shipment_revision.get())
            .map_err(invalid)?,
    };
    let result = repo::carrier::queue(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_job(result.job)?))
}

pub async fn get_manifest_job(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path((shipment_id, job_id)): Path<(i64, i64)>,
) -> V1Result<Json<CarrierManifestJobResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let result = repo::carrier::get_job(
        &state.db,
        &user.tenant,
        positive(shipment_id, ShipmentId::new, "shipment ID")?,
        positive(job_id, CarrierManifestJobId::new, "carrier manifest job ID")?,
    )
    .await?;
    Ok(Json(map_job(result)?))
}

pub async fn cancel_manifest_job(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((shipment_id, job_id)): Path<(i64, i64)>,
    Json(body): Json<CancelCarrierManifestRequest>,
) -> V1Result<Json<CarrierManifestJobResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = CancelCarrierManifestCommand {
        shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        job_id: positive(job_id, CarrierManifestJobId::new, "carrier manifest job ID")?,
        expected_revision: revision_u32(body.expected_revision, "carrier manifest job revision")?,
    };
    let result = repo::carrier::cancel(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_job(result)?))
}

pub async fn retry_manifest_job(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((shipment_id, job_id)): Path<(i64, i64)>,
    Json(body): Json<RetryCarrierManifestRequest>,
) -> V1Result<Json<CarrierManifestJobResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = RetryCarrierManifestCommand {
        shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        job_id: positive(job_id, CarrierManifestJobId::new, "carrier manifest job ID")?,
        expected_revision: revision_u32(body.expected_revision, "carrier manifest job revision")?,
    };
    let result = repo::carrier::retry(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_job(result)?))
}

fn map_account(value: CarrierAccountReadModel) -> V1Result<CarrierAccountResponse> {
    Ok(CarrierAccountResponse {
        account_id: value.account_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        display_name: value.display_name.into_inner(),
        carrier_code: value.carrier_code.into_inner(),
        account_key: value.account_key.into_inner(),
        status: match value.status {
            CarrierAccountStatus::Active => ApiCarrierAccountStatus::Active,
            CarrierAccountStatus::Disabled => ApiCarrierAccountStatus::Disabled,
        },
        revision: api_revision(value.revision, "carrier account revision")?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        updated_by: value.updated_by.get(),
        updated_at: value.updated_at.to_rfc3339(),
    })
}

fn map_job(value: CarrierManifestJobReadModel) -> V1Result<CarrierManifestJobResponse> {
    Ok(CarrierManifestJobResponse {
        job_id: value.job_id.get(),
        shipment_id: value.shipment_id.get(),
        account_id: value.account_id.get(),
        account_revision: api_revision(value.account_revision, "carrier account revision")?,
        account_key: value.account_key.into_inner(),
        carrier_code: value.carrier_code.into_inner(),
        service_code: value.service_code.map(CarrierServiceCode::into_inner),
        request_key: value.request_key,
        request_sha256: value.request_sha256,
        status: match value.status {
            CarrierManifestJobStatus::Queued => ApiCarrierManifestJobStatus::Queued,
            CarrierManifestJobStatus::Processing => ApiCarrierManifestJobStatus::Processing,
            CarrierManifestJobStatus::RetryScheduled => ApiCarrierManifestJobStatus::RetryScheduled,
            CarrierManifestJobStatus::Succeeded => ApiCarrierManifestJobStatus::Succeeded,
            CarrierManifestJobStatus::Failed => ApiCarrierManifestJobStatus::Failed,
            CarrierManifestJobStatus::Cancelled => ApiCarrierManifestJobStatus::Cancelled,
        },
        revision: api_revision(value.revision, "carrier manifest job revision")?,
        attempt_count: value.attempt_count,
        next_attempt_at: value.next_attempt_at.map(|time| time.to_rfc3339()),
        last_error_code: value.last_error_code.map(|code| code.as_str().to_owned()),
        last_error_message: value
            .last_error_message
            .map(|message| message.as_str().to_owned()),
        manifest_id: value.manifest_id.map(|id| id.get()),
        manifest_reference: value
            .manifest_reference
            .map(wareboxes_domain::ManifestReference::into_inner),
        requested_by: value.requested_by.get(),
        requested_at: value.requested_at.to_rfc3339(),
        completed_at: value.completed_at.map(|time| time.to_rfc3339()),
    })
}

const fn map_account_status_in(value: ApiCarrierAccountStatus) -> CarrierAccountStatus {
    match value {
        ApiCarrierAccountStatus::Active => CarrierAccountStatus::Active,
        ApiCarrierAccountStatus::Disabled => CarrierAccountStatus::Disabled,
    }
}

fn revision_u32(value: Revision, label: &str) -> V1Result<u32> {
    u32::try_from(value.get()).map_err(|_| invalid(format!("{label} exceeds supported range")))
}

fn api_revision(value: u32, label: &str) -> V1Result<Revision> {
    Revision::new(i64::from(value)).map_err(|_| V1Error::internal(format!("invalid {label}")))
}

fn positive<T, E>(
    value: i64,
    constructor: impl FnOnce(i64) -> Result<T, E>,
    label: &str,
) -> V1Result<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(|error| invalid(format!("invalid {label}: {error}")))
}

fn encode_account_cursor(
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    account_id: CarrierAccountId,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{ACCOUNT_CURSOR_PREFIX}{:016x}.{:016x}.{:016x}",
        owner_id.get(),
        facility_id.get(),
        account_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid carrier account cursor"))
}

fn decode_account_cursor(
    cursor: &OpaqueCursor,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> V1Result<CarrierAccountId> {
    let value = cursor
        .as_str()
        .strip_prefix(ACCOUNT_CURSOR_PREFIX)
        .ok_or_else(|| invalid("carrier account cursor is invalid"))?;
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parse_hex(parts[0]) != Some(owner_id.get())
        || parse_hex(parts[1]) != Some(facility_id.get())
    {
        return Err(invalid("carrier account cursor does not match the scope"));
    }
    parse_hex(parts[2])
        .and_then(|value| CarrierAccountId::new(value).ok())
        .ok_or_else(|| invalid("carrier account cursor is invalid"))
}

fn encode_job_cursor(
    shipment_id: ShipmentId,
    job_id: CarrierManifestJobId,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{JOB_CURSOR_PREFIX}{:016x}.{:016x}",
        shipment_id.get(),
        job_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid carrier manifest cursor"))
}

fn decode_job_cursor(
    cursor: &OpaqueCursor,
    shipment_id: ShipmentId,
) -> V1Result<CarrierManifestJobId> {
    let value = cursor
        .as_str()
        .strip_prefix(JOB_CURSOR_PREFIX)
        .ok_or_else(|| invalid("carrier manifest cursor is invalid"))?;
    let (shipment, job) = value
        .split_once('.')
        .ok_or_else(|| invalid("carrier manifest cursor is invalid"))?;
    if parse_hex(shipment) != Some(shipment_id.get()) {
        return Err(invalid(
            "carrier manifest cursor does not match the shipment",
        ));
    }
    parse_hex(job)
        .and_then(|value| CarrierManifestJobId::new(value).ok())
        .ok_or_else(|| invalid("carrier manifest cursor is invalid"))
}

fn parse_hex(value: &str) -> Option<i64> {
    i64::from_str_radix(value, 16)
        .ok()
        .filter(|value| *value > 0)
}

fn invalid(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_are_bound_to_their_exact_scope() {
        let owner = InventoryOwnerId::new(1).unwrap();
        let facility = FacilityId::new(2).unwrap();
        let cursor =
            encode_account_cursor(owner, facility, CarrierAccountId::new(3).unwrap()).unwrap();
        assert_eq!(
            decode_account_cursor(&cursor, owner, facility)
                .unwrap()
                .get(),
            3
        );
        assert!(decode_account_cursor(&cursor, owner, FacilityId::new(9).unwrap()).is_err());
    }
}
