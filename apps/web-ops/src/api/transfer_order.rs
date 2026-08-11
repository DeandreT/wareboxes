use wareboxes_api_contract::v1::{
    CancelTransferOrderRequest, CancelTransferOrderResponse, CreateTransferOrderRequest,
    CreateTransferOrderResponse, DispatchTransferOrderRequest, DispatchTransferOrderResponse,
    OpaqueCursor, ReceiveTransferOrderRequest, ReceiveTransferOrderResponse,
    ReleaseTransferOrderRequest, ReleaseTransferOrderResponse, TransferExecutionReadinessResponse,
    TransferOrderDetailResponse, TransferOrderPage, TransferOrderStatus,
};

use super::ApiError;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct TransferOrderFilters {
    pub source_facility_id: Option<i64>,
    pub destination_facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub status: Option<TransferOrderStatus>,
    pub search: Option<String>,
}

#[cfg(target_arch = "wasm32")]
pub async fn transfer_orders(
    filters: TransferOrderFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<TransferOrderPage, ApiError> {
    super::browser::get(&page_path(&filters, cursor)).await
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn transfer_orders(
    _filters: TransferOrderFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<TransferOrderPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn transfer_order_detail(id: i64) -> Result<TransferOrderDetailResponse, ApiError> {
    super::browser::get(&format!("/api/v1/transfer-orders/{id}")).await
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn transfer_order_detail(_id: i64) -> Result<TransferOrderDetailResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn transfer_execution_readiness(
    id: i64,
) -> Result<TransferExecutionReadinessResponse, ApiError> {
    super::browser::get(&format!("/api/v1/transfer-orders/{id}/execution-readiness")).await
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn transfer_execution_readiness(
    _id: i64,
) -> Result<TransferExecutionReadinessResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_transfer_order(
    request: &CreateTransferOrderRequest,
    key: &str,
) -> Result<CreateTransferOrderResponse, ApiError> {
    super::browser::post("/api/v1/transfer-orders", request, key).await
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn create_transfer_order(
    _request: &CreateTransferOrderRequest,
    _key: &str,
) -> Result<CreateTransferOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn release_transfer_order(
    id: i64,
    request: &ReleaseTransferOrderRequest,
    key: &str,
) -> Result<ReleaseTransferOrderResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/transfer-orders/{id}/releases"),
        request,
        key,
    )
    .await
}

#[cfg(target_arch = "wasm32")]
pub async fn dispatch_transfer_order(
    id: i64,
    request: &DispatchTransferOrderRequest,
    key: &str,
) -> Result<DispatchTransferOrderResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/transfer-orders/{id}/dispatches"),
        request,
        key,
    )
    .await
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn dispatch_transfer_order(
    _id: i64,
    _request: &DispatchTransferOrderRequest,
    _key: &str,
) -> Result<DispatchTransferOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn receive_transfer_order(
    id: i64,
    request: &ReceiveTransferOrderRequest,
    key: &str,
) -> Result<ReceiveTransferOrderResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/transfer-orders/{id}/receipts"),
        request,
        key,
    )
    .await
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn receive_transfer_order(
    _id: i64,
    _request: &ReceiveTransferOrderRequest,
    _key: &str,
) -> Result<ReceiveTransferOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn release_transfer_order(
    _id: i64,
    _request: &ReleaseTransferOrderRequest,
    _key: &str,
) -> Result<ReleaseTransferOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn cancel_transfer_order(
    id: i64,
    request: &CancelTransferOrderRequest,
    key: &str,
) -> Result<CancelTransferOrderResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/transfer-orders/{id}/cancellations"),
        request,
        key,
    )
    .await
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel_transfer_order(
    _id: i64,
    _request: &CancelTransferOrderRequest,
    _key: &str,
) -> Result<CancelTransferOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(filters: &TransferOrderFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = "/api/v1/transfer-orders?limit=100".to_owned();
    append_id(&mut path, "source_facility_id", filters.source_facility_id);
    append_id(
        &mut path,
        "destination_facility_id",
        filters.destination_facility_id,
    );
    append_id(&mut path, "inventory_owner_id", filters.inventory_owner_id);
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(match status {
            TransferOrderStatus::Draft => "draft",
            TransferOrderStatus::Released => "released",
            TransferOrderStatus::InTransit => "in_transit",
            TransferOrderStatus::Received => "received",
            TransferOrderStatus::Cancelled => "cancelled",
        });
    }
    if let Some(search) = filters.search.as_deref().filter(|value| !value.is_empty()) {
        path.push_str("&search=");
        path.push_str(&urlencoding::encode(search));
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_id(path: &mut String, name: &str, value: Option<i64>) {
    if let Some(value) = value {
        path.push('&');
        path.push_str(name);
        path.push('=');
        path.push_str(&value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_binds_both_facilities() {
        let path = page_path(
            &TransferOrderFilters {
                source_facility_id: Some(2),
                destination_facility_id: Some(3),
                inventory_owner_id: Some(4),
                status: Some(TransferOrderStatus::Released),
                search: Some("TO 1".into()),
            },
            None,
        );
        assert_eq!(path, "/api/v1/transfer-orders?limit=100&source_facility_id=2&destination_facility_id=3&inventory_owner_id=4&status=released&search=TO%201");
    }
}
