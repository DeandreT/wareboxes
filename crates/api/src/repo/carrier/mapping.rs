use sqlx::Row;
use wareboxes_application::carrier::{CarrierAccountReadModel, CarrierManifestJobReadModel};
use wareboxes_domain::{
    CarrierAccountId, CarrierAccountKey, CarrierAccountName, CarrierAccountStatus, CarrierCode,
    CarrierFailureCode, CarrierFailureMessage, CarrierManifestId, CarrierManifestJobId,
    CarrierManifestJobStatus, CarrierServiceCode, FacilityId, InventoryOwnerId, ManifestReference,
    ShipmentId, TenantId, Timestamp, UserId,
};

use crate::error::{AppError, AppResult};

pub const ACCOUNT_COLUMNS: &str = r#"account.id,account.tenant_id,account.inventory_owner_id,
account.facility_id,account.display_name,account.carrier_code,account.account_key,
account.status,account.revision,account.configured_by_user_id,account.configured_at,
account.updated_by_user_id,account.updated_at"#;

pub const JOB_COLUMNS: &str = r#"job.id,job.tenant_id,job.inventory_owner_id,job.facility_id,
job.shipment_id,job.carrier_account_id,job.carrier_account_revision,job.account_key,
job.carrier_code,job.service_code,job.request_key,encode(job.request_sha256,'hex') AS request_sha256,
job.status,job.revision,job.attempt_count,job.next_attempt_at,job.last_error_code,
job.last_error_message,job.carrier_manifest_id,job.manifest_reference,
job.requested_by_user_id,job.requested_at,job.completed_at"#;

pub fn account(row: &sqlx::postgres::PgRow) -> AppResult<CarrierAccountReadModel> {
    Ok(CarrierAccountReadModel {
        account_id: positive(row.try_get("id")?, CarrierAccountId::new)?,
        tenant_id: positive(row.try_get("tenant_id")?, TenantId::new)?,
        inventory_owner_id: positive(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        display_name: CarrierAccountName::new(row.try_get::<String, _>("display_name")?)
            .map_err(invalid_data)?,
        carrier_code: CarrierCode::new(row.try_get::<String, _>("carrier_code")?)
            .map_err(invalid_data)?,
        account_key: CarrierAccountKey::new(row.try_get::<String, _>("account_key")?)
            .map_err(invalid_data)?,
        status: account_status(&row.try_get::<String, _>("status")?)?,
        revision: positive_u32(row.try_get("revision")?, "carrier account revision")?,
        configured_by: positive(row.try_get("configured_by_user_id")?, UserId::new)?,
        configured_at: row.try_get("configured_at")?,
        updated_by: positive(row.try_get("updated_by_user_id")?, UserId::new)?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub fn job(row: &sqlx::postgres::PgRow) -> AppResult<CarrierManifestJobReadModel> {
    Ok(CarrierManifestJobReadModel {
        job_id: positive(row.try_get("id")?, CarrierManifestJobId::new)?,
        tenant_id: positive(row.try_get("tenant_id")?, TenantId::new)?,
        inventory_owner_id: positive(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        shipment_id: positive(row.try_get("shipment_id")?, ShipmentId::new)?,
        account_id: positive(row.try_get("carrier_account_id")?, CarrierAccountId::new)?,
        account_revision: positive_u32(
            row.try_get("carrier_account_revision")?,
            "carrier account revision",
        )?,
        account_key: CarrierAccountKey::new(row.try_get::<String, _>("account_key")?)
            .map_err(invalid_data)?,
        carrier_code: CarrierCode::new(row.try_get::<String, _>("carrier_code")?)
            .map_err(invalid_data)?,
        service_code: row
            .try_get::<Option<String>, _>("service_code")?
            .map(CarrierServiceCode::new)
            .transpose()
            .map_err(invalid_data)?,
        request_key: row.try_get("request_key")?,
        request_sha256: row.try_get("request_sha256")?,
        status: job_status(&row.try_get::<String, _>("status")?)?,
        revision: positive_u32(row.try_get("revision")?, "carrier manifest job revision")?,
        attempt_count: nonnegative_u32(
            row.try_get("attempt_count")?,
            "carrier manifest attempt count",
        )?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        last_error_code: row
            .try_get::<Option<String>, _>("last_error_code")?
            .map(CarrierFailureCode::new)
            .transpose()
            .map_err(invalid_data)?,
        last_error_message: row
            .try_get::<Option<String>, _>("last_error_message")?
            .map(CarrierFailureMessage::new)
            .transpose()
            .map_err(invalid_data)?,
        manifest_id: row
            .try_get::<Option<i64>, _>("carrier_manifest_id")?
            .map(|value| positive(value, CarrierManifestId::new))
            .transpose()?,
        manifest_reference: row
            .try_get::<Option<String>, _>("manifest_reference")?
            .map(ManifestReference::new)
            .transpose()
            .map_err(invalid_data)?,
        requested_by: positive(row.try_get("requested_by_user_id")?, UserId::new)?,
        requested_at: row.try_get::<Timestamp, _>("requested_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn account_status(value: &str) -> AppResult<CarrierAccountStatus> {
    match value {
        "active" => Ok(CarrierAccountStatus::Active),
        "disabled" => Ok(CarrierAccountStatus::Disabled),
        _ => Err(AppError::internal(
            "database returned invalid carrier account status",
        )),
    }
}

fn job_status(value: &str) -> AppResult<CarrierManifestJobStatus> {
    match value {
        "queued" => Ok(CarrierManifestJobStatus::Queued),
        "processing" => Ok(CarrierManifestJobStatus::Processing),
        "retry_scheduled" => Ok(CarrierManifestJobStatus::RetryScheduled),
        "succeeded" => Ok(CarrierManifestJobStatus::Succeeded),
        "failed" => Ok(CarrierManifestJobStatus::Failed),
        "cancelled" => Ok(CarrierManifestJobStatus::Cancelled),
        _ => Err(AppError::internal(
            "database returned invalid carrier manifest job status",
        )),
    }
}

fn positive<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>) -> AppResult<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(invalid_data)
}

fn positive_u32(value: i32, label: &str) -> AppResult<u32> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::internal(format!("database returned invalid {label}")))
}

fn nonnegative_u32(value: i32, label: &str) -> AppResult<u32> {
    u32::try_from(value)
        .map_err(|_| AppError::internal(format!("database returned invalid {label}")))
}

fn invalid_data(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}
