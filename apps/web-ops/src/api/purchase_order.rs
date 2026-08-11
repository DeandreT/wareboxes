use wareboxes_api_contract::v1::{
    CancelPurchaseOrderRequest, CancelPurchaseOrderResponse, CreatePurchaseOrderAsnRequest,
    CreatePurchaseOrderAsnResponse, CreatePurchaseOrderRequest, CreatePurchaseOrderResponse,
    OpaqueCursor, PurchaseOrderDetailResponse, PurchaseOrderPage, PurchaseOrderStatus,
    ReleasePurchaseOrderRequest, ReleasePurchaseOrderResponse,
};

use super::ApiError;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct PurchaseOrderFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub status: Option<PurchaseOrderStatus>,
    pub search: Option<String>,
}

#[cfg(target_arch = "wasm32")]
pub async fn purchase_orders(
    filters: PurchaseOrderFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<PurchaseOrderPage, ApiError> {
    super::browser::get(&purchase_order_page_path(&filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn purchase_orders(
    _filters: PurchaseOrderFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<PurchaseOrderPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn purchase_order_detail(
    purchase_order_id: i64,
) -> Result<PurchaseOrderDetailResponse, ApiError> {
    super::browser::get(&format!("/api/v1/purchase-orders/{purchase_order_id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn purchase_order_detail(
    _purchase_order_id: i64,
) -> Result<PurchaseOrderDetailResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_purchase_order(
    request: &CreatePurchaseOrderRequest,
    idempotency_key: &str,
) -> Result<CreatePurchaseOrderResponse, ApiError> {
    super::browser::post("/api/v1/purchase-orders", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_purchase_order(
    _request: &CreatePurchaseOrderRequest,
    _idempotency_key: &str,
) -> Result<CreatePurchaseOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn release_purchase_order(
    purchase_order_id: i64,
    request: &ReleasePurchaseOrderRequest,
    idempotency_key: &str,
) -> Result<ReleasePurchaseOrderResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/purchase-orders/{purchase_order_id}/releases"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(target_arch = "wasm32")]
pub async fn cancel_purchase_order(
    purchase_order_id: i64,
    request: &CancelPurchaseOrderRequest,
    idempotency_key: &str,
) -> Result<CancelPurchaseOrderResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/purchase-orders/{purchase_order_id}/cancellations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(target_arch = "wasm32")]
pub async fn create_purchase_order_asn(
    purchase_order_id: i64,
    request: &CreatePurchaseOrderAsnRequest,
    idempotency_key: &str,
) -> Result<CreatePurchaseOrderAsnResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/purchase-orders/{purchase_order_id}/asns"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_purchase_order_asn(
    _purchase_order_id: i64,
    _request: &CreatePurchaseOrderAsnRequest,
    _idempotency_key: &str,
) -> Result<CreatePurchaseOrderAsnResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn release_purchase_order(
    _purchase_order_id: i64,
    _request: &ReleasePurchaseOrderRequest,
    _idempotency_key: &str,
) -> Result<ReleasePurchaseOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel_purchase_order(
    _purchase_order_id: i64,
    _request: &CancelPurchaseOrderRequest,
    _idempotency_key: &str,
) -> Result<CancelPurchaseOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn purchase_order_page_path(
    filters: &PurchaseOrderFilters,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = "/api/v1/purchase-orders?limit=100".to_owned();
    append_id(&mut path, "facility_id", filters.facility_id);
    append_id(&mut path, "inventory_owner_id", filters.inventory_owner_id);
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(match status {
            PurchaseOrderStatus::Draft => "draft",
            PurchaseOrderStatus::Released => "released",
            PurchaseOrderStatus::Cancelled => "cancelled",
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
    fn page_path_binds_filters_and_cursor() {
        let cursor = OpaqueCursor::new("po1.cursor+/=").unwrap();
        let path = purchase_order_page_path(
            &PurchaseOrderFilters {
                facility_id: Some(7),
                inventory_owner_id: Some(9),
                status: Some(PurchaseOrderStatus::Draft),
                search: Some("PO 100/2".into()),
            },
            Some(&cursor),
        );
        assert_eq!(
            path,
            "/api/v1/purchase-orders?limit=100&facility_id=7&inventory_owner_id=9&status=draft&search=PO%20100%2F2&cursor=po1.cursor%2B%2F%3D"
        );
    }
}
