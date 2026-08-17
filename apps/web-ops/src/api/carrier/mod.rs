use wareboxes_api_contract::v1::{
    CancelCarrierManifestRequest, CarrierAccountPage, CarrierAccountResponse,
    CarrierManifestJobPage, CarrierManifestJobResponse, ChangeCarrierAccountStatusRequest,
    CreateCarrierAccountRequest, OpaqueCursor, QueueCarrierManifestRequest,
    ReconfigureCarrierAccountRequest, RetryCarrierManifestRequest,
};

use super::ApiError;

#[cfg(any(target_arch = "wasm32", test))]
fn account_page_path(
    owner_id: i64,
    facility_id: i64,
    include_disabled: bool,
    cursor: Option<&OpaqueCursor>,
    limit: u16,
) -> String {
    let mut path = format!(
        "/api/v1/carrier-accounts?inventory_owner_id={owner_id}&facility_id={facility_id}&include_disabled={include_disabled}&limit={limit}"
    );
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(cursor.as_str());
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn job_page_path(shipment_id: i64, cursor: Option<&OpaqueCursor>, limit: u16) -> String {
    let mut path = format!("/api/v1/shipments/{shipment_id}/carrier-manifest-jobs?limit={limit}");
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(cursor.as_str());
    }
    path
}

#[cfg(target_arch = "wasm32")]
pub async fn carrier_accounts(
    owner_id: i64,
    facility_id: i64,
    include_disabled: bool,
    cursor: Option<&OpaqueCursor>,
    limit: u16,
) -> Result<CarrierAccountPage, ApiError> {
    super::browser::get(&account_page_path(
        owner_id,
        facility_id,
        include_disabled,
        cursor,
        limit,
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn carrier_accounts(
    _owner_id: i64,
    _facility_id: i64,
    _include_disabled: bool,
    _cursor: Option<&OpaqueCursor>,
    _limit: u16,
) -> Result<CarrierAccountPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_carrier_account(
    request: &CreateCarrierAccountRequest,
    idempotency_key: &str,
) -> Result<CarrierAccountResponse, ApiError> {
    super::browser::post("/api/v1/carrier-accounts", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_carrier_account(
    _request: &CreateCarrierAccountRequest,
    _idempotency_key: &str,
) -> Result<CarrierAccountResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn reconfigure_carrier_account(
    account_id: i64,
    request: &ReconfigureCarrierAccountRequest,
    idempotency_key: &str,
) -> Result<CarrierAccountResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/carrier-accounts/{account_id}/reconfigurations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn reconfigure_carrier_account(
    _account_id: i64,
    _request: &ReconfigureCarrierAccountRequest,
    _idempotency_key: &str,
) -> Result<CarrierAccountResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn change_carrier_account_status(
    account_id: i64,
    request: &ChangeCarrierAccountStatusRequest,
    idempotency_key: &str,
) -> Result<CarrierAccountResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/carrier-accounts/{account_id}/status-changes"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn change_carrier_account_status(
    _account_id: i64,
    _request: &ChangeCarrierAccountStatusRequest,
    _idempotency_key: &str,
) -> Result<CarrierAccountResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn carrier_manifest_jobs(
    shipment_id: i64,
    cursor: Option<&OpaqueCursor>,
    limit: u16,
) -> Result<CarrierManifestJobPage, ApiError> {
    super::browser::get(&job_page_path(shipment_id, cursor, limit)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn carrier_manifest_jobs(
    _shipment_id: i64,
    _cursor: Option<&OpaqueCursor>,
    _limit: u16,
) -> Result<CarrierManifestJobPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn queue_carrier_manifest(
    shipment_id: i64,
    request: &QueueCarrierManifestRequest,
    idempotency_key: &str,
) -> Result<CarrierManifestJobResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/shipments/{shipment_id}/carrier-manifest-jobs"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn queue_carrier_manifest(
    _shipment_id: i64,
    _request: &QueueCarrierManifestRequest,
    _idempotency_key: &str,
) -> Result<CarrierManifestJobResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn cancel_carrier_manifest_job(
    shipment_id: i64,
    job_id: i64,
    request: &CancelCarrierManifestRequest,
    idempotency_key: &str,
) -> Result<CarrierManifestJobResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/shipments/{shipment_id}/carrier-manifest-jobs/{job_id}/cancellations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel_carrier_manifest_job(
    _shipment_id: i64,
    _job_id: i64,
    _request: &CancelCarrierManifestRequest,
    _idempotency_key: &str,
) -> Result<CarrierManifestJobResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn retry_carrier_manifest_job(
    shipment_id: i64,
    job_id: i64,
    request: &RetryCarrierManifestRequest,
    idempotency_key: &str,
) -> Result<CarrierManifestJobResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/shipments/{shipment_id}/carrier-manifest-jobs/{job_id}/retries"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn retry_carrier_manifest_job(
    _shipment_id: i64,
    _job_id: i64,
    _request: &RetryCarrierManifestRequest,
    _idempotency_key: &str,
) -> Result<CarrierManifestJobResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carrier_history_paths_bind_exact_scopes_and_cursors() {
        let cursor = OpaqueCursor::new("cmj1.0001.0002").unwrap();
        assert_eq!(
            account_page_path(3, 4, true, None, 20),
            "/api/v1/carrier-accounts?inventory_owner_id=3&facility_id=4&include_disabled=true&limit=20"
        );
        assert_eq!(
            job_page_path(7, Some(&cursor), 10),
            "/api/v1/shipments/7/carrier-manifest-jobs?limit=10&cursor=cmj1.0001.0002"
        );
    }
}
