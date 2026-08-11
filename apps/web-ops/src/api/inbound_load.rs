use wareboxes_api_contract::v1::{
    ArriveInboundLoadRequest, ArriveInboundLoadResponse, CloseInboundLoadRequest,
    CloseInboundLoadResponse, InboundLoadEntryItemResponse, PlanInboundLoadRequest,
    PlanInboundLoadResponse, ScheduleInboundLoadRequest, ScheduleInboundLoadResponse,
    StartInboundLoadUnloadingRequest, StartInboundLoadUnloadingResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn inbound_load_entry_items(
    inventory_owner_id: i64,
) -> Result<Vec<InboundLoadEntryItemResponse>, ApiError> {
    super::browser::get(&format!(
        "/api/v1/inventory-owners/{inventory_owner_id}/inbound-load-entry-items?limit=100"
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn inbound_load_entry_items(
    _inventory_owner_id: i64,
) -> Result<Vec<InboundLoadEntryItemResponse>, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn plan_inbound_load(
    request: &PlanInboundLoadRequest,
    idempotency_key: &str,
) -> Result<PlanInboundLoadResponse, ApiError> {
    super::browser::post("/api/v1/inbound-loads", request, idempotency_key).await
}

#[cfg(target_arch = "wasm32")]
pub async fn schedule_inbound_load(
    load_id: i64,
    request: &ScheduleInboundLoadRequest,
    idempotency_key: &str,
) -> Result<ScheduleInboundLoadResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/inbound-loads/{load_id}/appointments"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn schedule_inbound_load(
    _load_id: i64,
    _request: &ScheduleInboundLoadRequest,
    _idempotency_key: &str,
) -> Result<ScheduleInboundLoadResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan_inbound_load(
    _request: &PlanInboundLoadRequest,
    _idempotency_key: &str,
) -> Result<PlanInboundLoadResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn arrive_inbound_load(
    load_id: i64,
    request: &ArriveInboundLoadRequest,
    idempotency_key: &str,
) -> Result<ArriveInboundLoadResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/inbound-loads/{load_id}/arrivals"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn arrive_inbound_load(
    _load_id: i64,
    _request: &ArriveInboundLoadRequest,
    _idempotency_key: &str,
) -> Result<ArriveInboundLoadResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn start_inbound_load_unloading(
    load_id: i64,
    request: &StartInboundLoadUnloadingRequest,
    idempotency_key: &str,
) -> Result<StartInboundLoadUnloadingResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/inbound-loads/{load_id}/unloading-starts"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(target_arch = "wasm32")]
pub async fn close_inbound_load(
    load_id: i64,
    request: &CloseInboundLoadRequest,
    idempotency_key: &str,
) -> Result<CloseInboundLoadResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/inbound-loads/{load_id}/closures"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn close_inbound_load(
    _load_id: i64,
    _request: &CloseInboundLoadRequest,
    _idempotency_key: &str,
) -> Result<CloseInboundLoadResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn start_inbound_load_unloading(
    _load_id: i64,
    _request: &StartInboundLoadUnloadingRequest,
    _idempotency_key: &str,
) -> Result<StartInboundLoadUnloadingResponse, ApiError> {
    Err(ApiError::unavailable())
}
