use wareboxes_api_contract::v1::{
    BackorderPolicyResponse, ConfigureBackorderPolicyRequest, SplitOrderBackorderRequest,
    SplitOrderBackorderResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn configure_backorder_policy(
    request: &ConfigureBackorderPolicyRequest,
    idempotency_key: &str,
) -> Result<BackorderPolicyResponse, ApiError> {
    super::browser::post("/api/v1/backorder-policies", request, idempotency_key).await
}

#[cfg(target_arch = "wasm32")]
pub async fn split_order_backorder(
    order_id: i64,
    request: &SplitOrderBackorderRequest,
    idempotency_key: &str,
) -> Result<SplitOrderBackorderResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/orders/{order_id}/backorder-splits"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_backorder_policy(
    _request: &ConfigureBackorderPolicyRequest,
    _idempotency_key: &str,
) -> Result<BackorderPolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn split_order_backorder(
    _order_id: i64,
    _request: &SplitOrderBackorderRequest,
    _idempotency_key: &str,
) -> Result<SplitOrderBackorderResponse, ApiError> {
    Err(ApiError::unavailable())
}
