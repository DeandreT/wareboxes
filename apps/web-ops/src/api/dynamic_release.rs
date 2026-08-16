use wareboxes_api_contract::v1::{
    DynamicReleaseReadinessResponse, DynamicReleaseRunResponse, RunDynamicReleaseRequest,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn dynamic_release_readiness(
    facility_id: i64,
    inventory_owner_id: i64,
) -> Result<DynamicReleaseReadinessResponse, ApiError> {
    super::browser::get(&readiness_path(facility_id, inventory_owner_id)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn dynamic_release_readiness(
    _facility_id: i64,
    _inventory_owner_id: i64,
) -> Result<DynamicReleaseReadinessResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn run_dynamic_release(
    request: &RunDynamicReleaseRequest,
    idempotency_key: &str,
) -> Result<DynamicReleaseRunResponse, ApiError> {
    super::browser::post("/api/v1/dynamic-releases", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn run_dynamic_release(
    _request: &RunDynamicReleaseRequest,
    _idempotency_key: &str,
) -> Result<DynamicReleaseRunResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn readiness_path(facility_id: i64, inventory_owner_id: i64) -> String {
    format!(
        "/api/v1/dynamic-releases/readiness?facility_id={facility_id}&inventory_owner_id={inventory_owner_id}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_path_preserves_exact_owner_facility_scope() {
        assert_eq!(
            readiness_path(17, 23),
            "/api/v1/dynamic-releases/readiness?facility_id=17&inventory_owner_id=23"
        );
    }
}
