use sqlx::Row;
use wareboxes_application::carrier::{
    CarrierManifestAdapterRequest, CarrierManifestClaim, CarrierManifestJobReadModel,
};
use wareboxes_domain::{
    CarrierAccountId, CarrierAccountKey, CarrierCode, CarrierFailureCode, CarrierFailureMessage,
    CarrierManifestId, CarrierManifestJobId, CarrierManifestJobStatus, CarrierServiceCode,
    FacilityId, InventoryOwnerId, ManifestReference, ShipmentId, TenantId, Timestamp, UserId,
};

pub const CLAIM_COLUMNS: &str = r#"job.id,job.tenant_id,job.inventory_owner_id,job.facility_id,
job.shipment_id,job.carrier_account_id,job.carrier_account_revision,job.account_key,
job.carrier_code,job.service_code,job.request_key,encode(job.request_sha256,'hex') AS request_sha256,
job.request_payload,job.status,job.revision,job.attempt_count,job.next_attempt_at,
job.last_error_code,job.last_error_message,job.carrier_manifest_id,job.manifest_reference,
job.requested_by_user_id,job.requested_at,job.completed_at"#;

pub fn claim(row: &sqlx::postgres::PgRow) -> anyhow::Result<CarrierManifestClaim> {
    let request =
        serde_json::from_value::<CarrierManifestAdapterRequest>(row.try_get("request_payload")?)?;
    let job = CarrierManifestJobReadModel {
        job_id: CarrierManifestJobId::new(row.try_get("id")?)?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)?,
        shipment_id: ShipmentId::new(row.try_get("shipment_id")?)?,
        account_id: CarrierAccountId::new(row.try_get("carrier_account_id")?)?,
        account_revision: positive_u32(row.try_get("carrier_account_revision")?, "account")?,
        account_key: CarrierAccountKey::new(row.try_get::<String, _>("account_key")?)?,
        carrier_code: CarrierCode::new(row.try_get::<String, _>("carrier_code")?)?,
        service_code: row
            .try_get::<Option<String>, _>("service_code")?
            .map(CarrierServiceCode::new)
            .transpose()?,
        request_key: row.try_get("request_key")?,
        request_sha256: row.try_get("request_sha256")?,
        status: job_status(&row.try_get::<String, _>("status")?)?,
        revision: positive_u32(row.try_get("revision")?, "job")?,
        attempt_count: u32::try_from(row.try_get::<i32, _>("attempt_count")?)?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        last_error_code: row
            .try_get::<Option<String>, _>("last_error_code")?
            .map(CarrierFailureCode::new)
            .transpose()?,
        last_error_message: row
            .try_get::<Option<String>, _>("last_error_message")?
            .map(CarrierFailureMessage::new)
            .transpose()?,
        manifest_id: row
            .try_get::<Option<i64>, _>("carrier_manifest_id")?
            .map(CarrierManifestId::new)
            .transpose()?,
        manifest_reference: row
            .try_get::<Option<String>, _>("manifest_reference")?
            .map(ManifestReference::new)
            .transpose()?,
        requested_by: UserId::new(row.try_get("requested_by_user_id")?)?,
        requested_at: row.try_get::<Timestamp, _>("requested_at")?,
        completed_at: row.try_get("completed_at")?,
    };
    if request.request_key != job.request_key
        || request.tenant_id != job.tenant_id
        || request.shipment_id != job.shipment_id
        || request.account_key != job.account_key
        || request.carrier_code != job.carrier_code
        || request.service_code != job.service_code
    {
        anyhow::bail!("carrier job request payload does not match its envelope");
    }
    Ok(CarrierManifestClaim {
        claim_version: job.revision,
        job,
        request,
    })
}

fn positive_u32(value: i32, label: &str) -> anyhow::Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow::anyhow!("invalid carrier {label} revision"))
}

fn job_status(value: &str) -> anyhow::Result<CarrierManifestJobStatus> {
    match value {
        "queued" => Ok(CarrierManifestJobStatus::Queued),
        "processing" => Ok(CarrierManifestJobStatus::Processing),
        "retry_scheduled" => Ok(CarrierManifestJobStatus::RetryScheduled),
        "succeeded" => Ok(CarrierManifestJobStatus::Succeeded),
        "failed" => Ok(CarrierManifestJobStatus::Failed),
        "cancelled" => Ok(CarrierManifestJobStatus::Cancelled),
        _ => anyhow::bail!("invalid carrier manifest job status: {value}"),
    }
}
