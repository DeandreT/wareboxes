use wareboxes_api_contract::v1::{
    ApproveSupportAccessRequest, RejectSupportAccessRequest, RequestSupportAccessRequest,
    RevokeSupportAccessRequest, SupportAccessEventPage, SupportAccessEventPageRequest,
    SupportAccessOptionsResponse, SupportAccessPage, SupportAccessPageRequest,
    SupportAccessResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn support_access_page(
    request: &SupportAccessPageRequest,
) -> Result<SupportAccessPage, ApiError> {
    super::browser::get(&page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn support_access_page(
    _request: &SupportAccessPageRequest,
) -> Result<SupportAccessPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn support_access_options(
    tenant_id: i64,
) -> Result<SupportAccessOptionsResponse, ApiError> {
    super::browser::get(&format!(
        "/api/v1/platform/support-access/options?tenant_id={tenant_id}"
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn support_access_options(
    _tenant_id: i64,
) -> Result<SupportAccessOptionsResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn support_access_events(
    id: i64,
    request: &SupportAccessEventPageRequest,
) -> Result<SupportAccessEventPage, ApiError> {
    super::browser::get(&event_path(id, request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn support_access_events(
    _id: i64,
    _request: &SupportAccessEventPageRequest,
) -> Result<SupportAccessEventPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn request_support_access(
    request: &RequestSupportAccessRequest,
    idempotency_key: &str,
) -> Result<SupportAccessResponse, ApiError> {
    super::browser::post("/api/v1/platform/support-access", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn request_support_access(
    _request: &RequestSupportAccessRequest,
    _idempotency_key: &str,
) -> Result<SupportAccessResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn approve_support_access(
    id: i64,
    request: &ApproveSupportAccessRequest,
    idempotency_key: &str,
) -> Result<SupportAccessResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/platform/support-access/{id}/approvals"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn approve_support_access(
    _id: i64,
    _request: &ApproveSupportAccessRequest,
    _idempotency_key: &str,
) -> Result<SupportAccessResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn reject_support_access(
    id: i64,
    request: &RejectSupportAccessRequest,
    idempotency_key: &str,
) -> Result<SupportAccessResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/platform/support-access/{id}/rejections"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn reject_support_access(
    _id: i64,
    _request: &RejectSupportAccessRequest,
    _idempotency_key: &str,
) -> Result<SupportAccessResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn revoke_support_access(
    id: i64,
    request: &RevokeSupportAccessRequest,
    idempotency_key: &str,
) -> Result<SupportAccessResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/platform/support-access/{id}/revocations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn revoke_support_access(
    _id: i64,
    _request: &RevokeSupportAccessRequest,
    _idempotency_key: &str,
) -> Result<SupportAccessResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(request: &SupportAccessPageRequest) -> String {
    let mut path = format!(
        "/api/v1/platform/support-access?limit={}",
        request.limit.get()
    );
    if let Some(tenant_id) = request.tenant_id {
        path.push_str(&format!("&tenant_id={tenant_id}"));
    }
    if let Some(status) = request.status {
        path.push_str("&status=");
        path.push_str(match status {
            wareboxes_api_contract::v1::SupportAccessStatus::Pending => "pending",
            wareboxes_api_contract::v1::SupportAccessStatus::Active => "active",
            wareboxes_api_contract::v1::SupportAccessStatus::Rejected => "rejected",
            wareboxes_api_contract::v1::SupportAccessStatus::Revoked => "revoked",
            wareboxes_api_contract::v1::SupportAccessStatus::Expired => "expired",
        });
    }
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn event_path(id: i64, request: &SupportAccessEventPageRequest) -> String {
    let mut path = format!(
        "/api/v1/platform/support-access/{id}/events?limit={}",
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
    use wareboxes_api_contract::v1::{OpaqueCursor, PageLimit, SupportAccessStatus};

    #[test]
    fn support_paths_bind_every_filter_and_cursor() {
        assert_eq!(
            page_path(&SupportAccessPageRequest {
                tenant_id: Some(9),
                status: Some(SupportAccessStatus::Active),
                cursor: Some(OpaqueCursor::new("sag1.a/b+c").unwrap()),
                limit: PageLimit::new(25).unwrap(),
            }),
            "/api/v1/platform/support-access?limit=25&tenant_id=9&status=active&cursor=sag1.a%2Fb%2Bc"
        );
        assert_eq!(
            event_path(
                4,
                &SupportAccessEventPageRequest {
                    cursor: None,
                    limit: PageLimit::new(50).unwrap(),
                }
            ),
            "/api/v1/platform/support-access/4/events?limit=50"
        );
    }
}
