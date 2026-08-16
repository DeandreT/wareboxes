use wareboxes_api_contract::v1::{
    CancelPickClusterRequest, ChangePickCartStatusRequest, CreatePickCartRequest, PickCartResponse,
    PickClusterResponse, PickClusterWorkspaceResponse, PlanPickClusterRequest,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn workspace(
    facility_id: i64,
    inventory_owner_id: i64,
    include_history: bool,
) -> Result<PickClusterWorkspaceResponse, ApiError> {
    super::browser::get(&workspace_path(
        facility_id,
        inventory_owner_id,
        include_history,
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn workspace(
    _facility_id: i64,
    _inventory_owner_id: i64,
    _include_history: bool,
) -> Result<PickClusterWorkspaceResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_cart(
    request: &CreatePickCartRequest,
    key: &str,
) -> Result<PickCartResponse, ApiError> {
    super::browser::post("/api/v1/pick-carts", request, key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_cart(
    _request: &CreatePickCartRequest,
    _key: &str,
) -> Result<PickCartResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn change_cart_status(
    cart_id: i64,
    request: &ChangePickCartStatusRequest,
    key: &str,
) -> Result<PickCartResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/pick-carts/{cart_id}/status-changes"),
        request,
        key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn change_cart_status(
    _cart_id: i64,
    _request: &ChangePickCartStatusRequest,
    _key: &str,
) -> Result<PickCartResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn plan(
    request: &PlanPickClusterRequest,
    key: &str,
) -> Result<PickClusterResponse, ApiError> {
    super::browser::post("/api/v1/pick-clusters", request, key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan(
    _request: &PlanPickClusterRequest,
    _key: &str,
) -> Result<PickClusterResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn cancel(
    cluster_id: i64,
    request: &CancelPickClusterRequest,
    key: &str,
) -> Result<PickClusterResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/pick-clusters/{cluster_id}/cancellations"),
        request,
        key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel(
    _cluster_id: i64,
    _request: &CancelPickClusterRequest,
    _key: &str,
) -> Result<PickClusterResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
pub fn workspace_path(facility_id: i64, inventory_owner_id: i64, include_history: bool) -> String {
    format!(
        "/api/v1/pick-clusters/workspace?facility_id={facility_id}&inventory_owner_id={inventory_owner_id}&include_history={include_history}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_path_keeps_exact_owner_and_facility_scope() {
        assert_eq!(
            workspace_path(4, 9, true),
            "/api/v1/pick-clusters/workspace?facility_id=4&inventory_owner_id=9&include_history=true"
        );
    }
}
