use wareboxes_api_contract::v1::{
    CancelCustomerReturnRequest, CancelCustomerReturnResponse, CreateCustomerReturnRequest,
    CreateCustomerReturnResponse, CustomerReturnDetailResponse, CustomerReturnPage,
    CustomerReturnStatus, OpaqueCursor, PlanCustomerReturnLoadRequest,
    PlanCustomerReturnLoadResponse,
};

use super::ApiError;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct CustomerReturnFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub status: Option<CustomerReturnStatus>,
    pub search: Option<String>,
}

#[cfg(target_arch = "wasm32")]
pub async fn customer_returns(
    filters: CustomerReturnFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<CustomerReturnPage, ApiError> {
    super::browser::get(&page_path(&filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn customer_returns(
    _filters: CustomerReturnFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<CustomerReturnPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn customer_return_detail(
    customer_return_id: i64,
) -> Result<CustomerReturnDetailResponse, ApiError> {
    super::browser::get(&format!("/api/v1/customer-returns/{customer_return_id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn customer_return_detail(
    _customer_return_id: i64,
) -> Result<CustomerReturnDetailResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_customer_return(
    request: &CreateCustomerReturnRequest,
    idempotency_key: &str,
) -> Result<CreateCustomerReturnResponse, ApiError> {
    super::browser::post("/api/v1/customer-returns", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_customer_return(
    _request: &CreateCustomerReturnRequest,
    _idempotency_key: &str,
) -> Result<CreateCustomerReturnResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn plan_customer_return_load(
    customer_return_id: i64,
    request: &PlanCustomerReturnLoadRequest,
    idempotency_key: &str,
) -> Result<PlanCustomerReturnLoadResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/customer-returns/{customer_return_id}/load-plans"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan_customer_return_load(
    _customer_return_id: i64,
    _request: &PlanCustomerReturnLoadRequest,
    _idempotency_key: &str,
) -> Result<PlanCustomerReturnLoadResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn cancel_customer_return(
    customer_return_id: i64,
    request: &CancelCustomerReturnRequest,
    idempotency_key: &str,
) -> Result<CancelCustomerReturnResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/customer-returns/{customer_return_id}/cancellations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel_customer_return(
    _customer_return_id: i64,
    _request: &CancelCustomerReturnRequest,
    _idempotency_key: &str,
) -> Result<CancelCustomerReturnResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(filters: &CustomerReturnFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = "/api/v1/customer-returns?limit=100".to_owned();
    append_id(&mut path, "facility_id", filters.facility_id);
    append_id(&mut path, "inventory_owner_id", filters.inventory_owner_id);
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(match status {
            CustomerReturnStatus::Open => "open",
            CustomerReturnStatus::Planned => "planned",
            CustomerReturnStatus::Cancelled => "cancelled",
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
    fn return_page_path_binds_filters_and_cursor() {
        let cursor = OpaqueCursor::new("cr1.cursor+/=").unwrap();
        assert_eq!(
            page_path(
                &CustomerReturnFilters {
                    facility_id: Some(7),
                    inventory_owner_id: Some(9),
                    status: Some(CustomerReturnStatus::Open),
                    search: Some("RMA 100/2".into()),
                },
                Some(&cursor),
            ),
            "/api/v1/customer-returns?limit=100&facility_id=7&inventory_owner_id=9&status=open&search=RMA%20100%2F2&cursor=cr1.cursor%2B%2F%3D"
        );
    }
}
