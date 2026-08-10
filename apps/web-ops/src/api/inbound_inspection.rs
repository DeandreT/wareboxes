use wareboxes_api_contract::v1::{
    DisposeInboundInspectionRequest, DisposeInboundInspectionResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn dispose_inbound_inspection(
    inventory_hold_id: i64,
    request: &DisposeInboundInspectionRequest,
    idempotency_key: &str,
) -> Result<DisposeInboundInspectionResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/inbound-inspections/{inventory_hold_id}/dispositions"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn dispose_inbound_inspection(
    _inventory_hold_id: i64,
    _request: &DisposeInboundInspectionRequest,
    _idempotency_key: &str,
) -> Result<DisposeInboundInspectionResponse, ApiError> {
    Err(ApiError::unavailable())
}
