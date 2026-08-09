use wareboxes_api_contract::v1::{
    AmendFulfillmentOrderRequest, AmendFulfillmentOrderResponse, CreateFulfillmentOrderRequest,
    CreateFulfillmentOrderResponse, OrderEntryItemResponse, ReplaceFulfillmentOrderLinesRequest,
    ReplaceFulfillmentOrderLinesResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn order_entry_items(
    inventory_owner_id: i64,
    search: &str,
) -> Result<Vec<OrderEntryItemResponse>, ApiError> {
    let mut path =
        format!("/api/v1/inventory-owners/{inventory_owner_id}/order-entry-items?limit=50");
    if !search.trim().is_empty() {
        path.push_str("&search=");
        path.push_str(&urlencoding::encode(search.trim()));
    }
    super::browser::get(&path).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn order_entry_items(
    _inventory_owner_id: i64,
    _search: &str,
) -> Result<Vec<OrderEntryItemResponse>, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_fulfillment_order(
    request: &CreateFulfillmentOrderRequest,
    idempotency_key: &str,
) -> Result<CreateFulfillmentOrderResponse, ApiError> {
    super::browser::post("/api/v1/orders", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_fulfillment_order(
    _request: &CreateFulfillmentOrderRequest,
    _idempotency_key: &str,
) -> Result<CreateFulfillmentOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn amend_fulfillment_order(
    order_id: i64,
    request: &AmendFulfillmentOrderRequest,
    idempotency_key: &str,
) -> Result<AmendFulfillmentOrderResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/orders/{order_id}/amendments"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn amend_fulfillment_order(
    _order_id: i64,
    _request: &AmendFulfillmentOrderRequest,
    _idempotency_key: &str,
) -> Result<AmendFulfillmentOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn replace_fulfillment_order_lines(
    order_id: i64,
    request: &ReplaceFulfillmentOrderLinesRequest,
    idempotency_key: &str,
) -> Result<ReplaceFulfillmentOrderLinesResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/orders/{order_id}/line-amendments"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn replace_fulfillment_order_lines(
    _order_id: i64,
    _request: &ReplaceFulfillmentOrderLinesRequest,
    _idempotency_key: &str,
) -> Result<ReplaceFulfillmentOrderLinesResponse, ApiError> {
    Err(ApiError::unavailable())
}
