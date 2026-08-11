use wareboxes_api_contract::v1::{
    InboundLoadEntryItemResponse, PlanInboundLoadRequest, PlanInboundLoadResponse,
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

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan_inbound_load(
    _request: &PlanInboundLoadRequest,
    _idempotency_key: &str,
) -> Result<PlanInboundLoadResponse, ApiError> {
    Err(ApiError::unavailable())
}
