use wareboxes_api_contract::v1::{
    ConfigureStorageZoneRequest, OpaqueCursor, RetireStorageZoneRequest, StorageZonePage,
    StorageZonePurpose, StorageZoneResponse, StorageZoneStatus,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn storage_zones(
    facility_id: Option<i64>,
    purpose: Option<StorageZonePurpose>,
    status: Option<StorageZoneStatus>,
    cursor: Option<&OpaqueCursor>,
) -> Result<StorageZonePage, ApiError> {
    super::browser::get(&page_path(facility_id, purpose, status, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn storage_zones(
    _facility_id: Option<i64>,
    _purpose: Option<StorageZonePurpose>,
    _status: Option<StorageZoneStatus>,
    _cursor: Option<&OpaqueCursor>,
) -> Result<StorageZonePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn configure_storage_zone(
    request: &ConfigureStorageZoneRequest,
    idempotency_key: &str,
) -> Result<StorageZoneResponse, ApiError> {
    super::browser::post("/api/v1/storage-zones", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_storage_zone(
    _request: &ConfigureStorageZoneRequest,
    _idempotency_key: &str,
) -> Result<StorageZoneResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn retire_storage_zone(
    storage_zone_id: i64,
    request: &RetireStorageZoneRequest,
    idempotency_key: &str,
) -> Result<StorageZoneResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/storage-zones/{storage_zone_id}/retirements"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn retire_storage_zone(
    _storage_zone_id: i64,
    _request: &RetireStorageZoneRequest,
    _idempotency_key: &str,
) -> Result<StorageZoneResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(
    facility_id: Option<i64>,
    purpose: Option<StorageZonePurpose>,
    status: Option<StorageZoneStatus>,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = "/api/v1/storage-zones?limit=100".to_owned();
    if let Some(facility_id) = facility_id {
        path.push_str(&format!("&facility_id={facility_id}"));
    }
    if let Some(purpose) = purpose {
        path.push_str("&purpose=");
        path.push_str(purpose_wire(purpose));
    }
    if let Some(status) = status {
        path.push_str("&status=");
        path.push_str(match status {
            StorageZoneStatus::Active => "active",
            StorageZoneStatus::Retired => "retired",
        });
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(cursor.as_str());
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
const fn purpose_wire(value: StorageZonePurpose) -> &'static str {
    match value {
        StorageZonePurpose::Receiving => "receiving",
        StorageZonePurpose::Reserve => "reserve",
        StorageZonePurpose::Pick => "pick",
        StorageZonePurpose::Staging => "staging",
        StorageZonePurpose::Packing => "packing",
        StorageZonePurpose::Shipping => "shipping",
        StorageZonePurpose::Quarantine => "quarantine",
        StorageZonePurpose::Damage => "damage",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_path_contains_stable_filter_identity() {
        let cursor = OpaqueCursor::new("sz1.filter.00000000.0000000000000001").unwrap();
        assert_eq!(
            page_path(
                Some(4),
                Some(StorageZonePurpose::Pick),
                Some(StorageZoneStatus::Retired),
                Some(&cursor),
            ),
            "/api/v1/storage-zones?limit=100&facility_id=4&purpose=pick&status=retired&cursor=sz1.filter.00000000.0000000000000001"
        );
    }
}
