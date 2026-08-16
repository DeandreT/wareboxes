use wareboxes_api_contract::v1::{
    AutomationCommandResponse, PackingScaleDevicePage, RequestPackingScaleWeight,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn packing_scale_devices(session_id: i64) -> Result<PackingScaleDevicePage, ApiError> {
    super::browser::get(&format!(
        "/api/v1/packing-sessions/{session_id}/scale-devices"
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn packing_scale_devices(_session_id: i64) -> Result<PackingScaleDevicePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn request_packing_scale_weight(
    session_id: i64,
    request: &RequestPackingScaleWeight,
    idempotency_key: &str,
) -> Result<AutomationCommandResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/packing-sessions/{session_id}/scale-readings"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn request_packing_scale_weight(
    _session_id: i64,
    _request: &RequestPackingScaleWeight,
    _idempotency_key: &str,
) -> Result<AutomationCommandResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn packing_scale_reading(
    session_id: i64,
    command_id: i64,
) -> Result<AutomationCommandResponse, ApiError> {
    super::browser::get(&format!(
        "/api/v1/packing-sessions/{session_id}/scale-readings/{command_id}"
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn packing_scale_reading(
    _session_id: i64,
    _command_id: i64,
) -> Result<AutomationCommandResponse, ApiError> {
    Err(ApiError::unavailable())
}
