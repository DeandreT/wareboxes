use wareboxes_api_contract::v1::{
    ChangeServiceAccountStatusRequest, CreateServiceAccountRequest,
    IssueServiceAccountCredentialRequest, IssuedServiceAccountCredentialResponse,
    RevokeServiceAccountCredentialRequest, ServiceAccountEventPage, ServiceAccountEventPageRequest,
    ServiceAccountOptionsResponse, ServiceAccountPage, ServiceAccountPageRequest,
    ServiceAccountResponse, UpdateServiceAccountAccessRequest,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn service_accounts(
    request: &ServiceAccountPageRequest,
) -> Result<ServiceAccountPage, ApiError> {
    super::browser::get(&page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn service_accounts(
    _request: &ServiceAccountPageRequest,
) -> Result<ServiceAccountPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn service_account(id: i64) -> Result<ServiceAccountResponse, ApiError> {
    super::browser::get(&format!("/api/v1/service-accounts/{id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn service_account(_id: i64) -> Result<ServiceAccountResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn service_account_options() -> Result<ServiceAccountOptionsResponse, ApiError> {
    super::browser::get("/api/v1/service-account-options").await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn service_account_options() -> Result<ServiceAccountOptionsResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn service_account_events(
    id: i64,
    request: &ServiceAccountEventPageRequest,
) -> Result<ServiceAccountEventPage, ApiError> {
    super::browser::get(&event_page_path(id, request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn service_account_events(
    _id: i64,
    _request: &ServiceAccountEventPageRequest,
) -> Result<ServiceAccountEventPage, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! command {
    ($name:ident, $request:ty, $response:ty, $path:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            request: &$request,
            idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            super::browser::post($path, request, idempotency_key).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _request: &$request,
            _idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

command!(
    create_service_account,
    CreateServiceAccountRequest,
    ServiceAccountResponse,
    "/api/v1/service-accounts"
);

macro_rules! account_command {
    ($name:ident, $request:ty, $response:ty, $suffix:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            id: i64,
            request: &$request,
            idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            super::browser::post(
                &format!("/api/v1/service-accounts/{id}/{}", $suffix),
                request,
                idempotency_key,
            )
            .await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _id: i64,
            _request: &$request,
            _idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

account_command!(
    update_service_account_access,
    UpdateServiceAccountAccessRequest,
    ServiceAccountResponse,
    "access-changes"
);
account_command!(
    change_service_account_status,
    ChangeServiceAccountStatusRequest,
    ServiceAccountResponse,
    "status-changes"
);
account_command!(
    issue_service_account_credential,
    IssueServiceAccountCredentialRequest,
    IssuedServiceAccountCredentialResponse,
    "credentials"
);

#[cfg(target_arch = "wasm32")]
pub async fn revoke_service_account_credential(
    account_id: i64,
    credential_id: i64,
    request: &RevokeServiceAccountCredentialRequest,
    idempotency_key: &str,
) -> Result<ServiceAccountResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/service-accounts/{account_id}/credentials/{credential_id}/revocations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn revoke_service_account_credential(
    _account_id: i64,
    _credential_id: i64,
    _request: &RevokeServiceAccountCredentialRequest,
    _idempotency_key: &str,
) -> Result<ServiceAccountResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub fn generate_service_account_bearer() -> String {
    let secret = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .chain(uuid::Uuid::new_v4().simple().to_string().chars())
        .take(48)
        .collect::<String>();
    format!("wbs_sa_{secret}")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn generate_service_account_bearer() -> String {
    String::new()
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(request: &ServiceAccountPageRequest) -> String {
    let mut path = format!("/api/v1/service-accounts?limit={}", request.limit.get());
    if let Some(status) = request.status {
        path.push_str("&status=");
        path.push_str(match status {
            wareboxes_api_contract::v1::ServiceAccountStatus::Active => "active",
            wareboxes_api_contract::v1::ServiceAccountStatus::Disabled => "disabled",
        });
    }
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn event_page_path(id: i64, request: &ServiceAccountEventPageRequest) -> String {
    let mut path = format!(
        "/api/v1/service-accounts/{id}/events?limit={}",
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
    use wareboxes_api_contract::v1::{OpaqueCursor, PageLimit, ServiceAccountStatus};

    #[test]
    fn service_account_paths_bind_status_account_and_cursor() {
        let page = page_path(&ServiceAccountPageRequest {
            status: Some(ServiceAccountStatus::Disabled),
            cursor: Some(OpaqueCursor::new("sac1.a/b+c").unwrap()),
            limit: PageLimit::new(25).unwrap(),
        });
        assert_eq!(
            page,
            "/api/v1/service-accounts?limit=25&status=disabled&cursor=sac1.a%2Fb%2Bc"
        );
        let events = event_page_path(
            41,
            &ServiceAccountEventPageRequest {
                cursor: Some(OpaqueCursor::new("sae1.a/b+c").unwrap()),
                limit: PageLimit::new(50).unwrap(),
            },
        );
        assert_eq!(
            events,
            "/api/v1/service-accounts/41/events?limit=50&cursor=sae1.a%2Fb%2Bc"
        );
    }
}
