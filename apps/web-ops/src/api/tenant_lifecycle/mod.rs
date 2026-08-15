use wareboxes_api_contract::v1::{
    ChangeTenantStatusRequest, CreateTenantRequest, TenantLifecycleEventPage,
    TenantLifecycleEventPageRequest, TenantLifecyclePage, TenantLifecyclePageRequest,
    TenantLifecycleResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn tenant_lifecycle_page(
    request: &TenantLifecyclePageRequest,
) -> Result<TenantLifecyclePage, ApiError> {
    super::browser::get(&page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn tenant_lifecycle_page(
    _request: &TenantLifecyclePageRequest,
) -> Result<TenantLifecyclePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn tenant_lifecycle_detail(id: i64) -> Result<TenantLifecycleResponse, ApiError> {
    super::browser::get(&format!("/api/v1/platform/tenants/{id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn tenant_lifecycle_detail(_id: i64) -> Result<TenantLifecycleResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn tenant_lifecycle_events(
    id: i64,
    request: &TenantLifecycleEventPageRequest,
) -> Result<TenantLifecycleEventPage, ApiError> {
    super::browser::get(&event_page_path(id, request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn tenant_lifecycle_events(
    _id: i64,
    _request: &TenantLifecycleEventPageRequest,
) -> Result<TenantLifecycleEventPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_tenant(
    request: &CreateTenantRequest,
    idempotency_key: &str,
) -> Result<TenantLifecycleResponse, ApiError> {
    super::browser::post("/api/v1/platform/tenants", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_tenant(
    _request: &CreateTenantRequest,
    _idempotency_key: &str,
) -> Result<TenantLifecycleResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn change_tenant_status(
    id: i64,
    request: &ChangeTenantStatusRequest,
    idempotency_key: &str,
) -> Result<TenantLifecycleResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/platform/tenants/{id}/status-changes"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn change_tenant_status(
    _id: i64,
    _request: &ChangeTenantStatusRequest,
    _idempotency_key: &str,
) -> Result<TenantLifecycleResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(request: &TenantLifecyclePageRequest) -> String {
    let mut path = format!("/api/v1/platform/tenants?limit={}", request.limit.get());
    if let Some(status) = request.status {
        path.push_str("&status=");
        path.push_str(match status {
            wareboxes_api_contract::v1::TenantStatus::Active => "active",
            wareboxes_api_contract::v1::TenantStatus::Suspended => "suspended",
        });
    }
    if let Some(search) = request.search.as_deref() {
        path.push_str("&search=");
        path.push_str(&urlencoding::encode(search));
    }
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn event_page_path(id: i64, request: &TenantLifecycleEventPageRequest) -> String {
    let mut path = format!(
        "/api/v1/platform/tenants/{id}/events?limit={}",
        request.limit.get()
    );
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_cursor(path: &mut String, cursor: Option<&wareboxes_api_contract::v1::OpaqueCursor>) {
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{OpaqueCursor, PageLimit, TenantStatus};

    #[test]
    fn lifecycle_paths_bind_filters_target_and_cursor() {
        let path = page_path(&TenantLifecyclePageRequest {
            status: Some(TenantStatus::Suspended),
            search: Some("North west/3PL".into()),
            cursor: Some(OpaqueCursor::new("tlc1.a/b+c").unwrap()),
            limit: PageLimit::new(25).unwrap(),
        });
        assert_eq!(
            path,
            "/api/v1/platform/tenants?limit=25&status=suspended&search=North%20west%2F3PL&cursor=tlc1.a%2Fb%2Bc"
        );
        assert_eq!(
            event_page_path(
                9,
                &TenantLifecycleEventPageRequest {
                    cursor: None,
                    limit: PageLimit::new(50).unwrap(),
                }
            ),
            "/api/v1/platform/tenants/9/events?limit=50"
        );
    }
}
