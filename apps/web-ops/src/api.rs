use wareboxes_core::dto::{AccessScopeWorkspace, OrderPage, WebSessionContext};
use wareboxes_core::models::InventoryBalance;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub message: String,
    pub unauthorized: bool,
}

impl ApiError {
    #[cfg(not(target_arch = "wasm32"))]
    fn unavailable() -> Self {
        Self {
            message: "The browser API client is unavailable during server rendering.".to_owned(),
            unauthorized: false,
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use gloo_net::http::{Request, Response};
    use serde::de::DeserializeOwned;
    use wareboxes_core::dto::{ErrorResponse, LoginRequest, SelectTenantRequest};

    use super::{AccessScopeWorkspace, ApiError, InventoryBalance, OrderPage, WebSessionContext};

    pub async fn login(email: String, password: String) -> Result<WebSessionContext, ApiError> {
        let request = Request::post(&url("/api/web/auth/login"))
            .json(&LoginRequest { email, password })
            .map_err(|error| ApiError {
                message: format!("Could not prepare the sign-in request: {error}"),
                unauthorized: false,
            })?;
        decode(request.send().await).await
    }

    pub async fn select_tenant(tenant_id: i64) -> Result<WebSessionContext, ApiError> {
        let request = Request::post(&url("/api/web/auth/tenant"))
            .json(&SelectTenantRequest { tenant_id })
            .map_err(|error| ApiError {
                message: format!("Could not prepare the tenant request: {error}"),
                unauthorized: false,
            })?;
        decode(request.send().await).await
    }

    pub async fn logout() {
        let _ = Request::post(&url("/api/web/auth/logout")).send().await;
    }

    pub async fn orders() -> Result<OrderPage, ApiError> {
        get("/api/orders?limit=50&offset=0").await
    }

    pub async fn balances() -> Result<Vec<InventoryBalance>, ApiError> {
        get("/api/inventory/balances").await
    }

    pub async fn access() -> Result<AccessScopeWorkspace, ApiError> {
        get("/api/web/access").await
    }

    async fn get<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
        decode(Request::get(&url(path)).send().await).await
    }

    async fn decode<T: DeserializeOwned>(
        response: Result<Response, gloo_net::Error>,
    ) -> Result<T, ApiError> {
        let response = response.map_err(|error| ApiError {
            message: format!("Wareboxes could not reach the server: {error}"),
            unauthorized: false,
        })?;
        let status = response.status();
        if (200..300).contains(&status) {
            return response.json::<T>().await.map_err(|error| ApiError {
                message: format!("The server returned an unreadable response: {error}"),
                unauthorized: false,
            });
        }

        let unauthorized = status == 401;
        let message = response
            .json::<ErrorResponse>()
            .await
            .map(|error| {
                if error.request_id.is_empty() {
                    error.message
                } else {
                    format!("{} (request {})", error.message, error.request_id)
                }
            })
            .unwrap_or_else(|_| format!("The server rejected the request with HTTP {status}."));
        Err(ApiError {
            message,
            unauthorized,
        })
    }

    fn url(path: &str) -> String {
        path.to_owned()
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::*;

#[cfg(not(target_arch = "wasm32"))]
pub async fn login(_email: String, _password: String) -> Result<WebSessionContext, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn select_tenant(_tenant_id: i64) -> Result<WebSessionContext, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn logout() {}

#[cfg(not(target_arch = "wasm32"))]
pub async fn orders() -> Result<OrderPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn balances() -> Result<Vec<InventoryBalance>, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn access() -> Result<AccessScopeWorkspace, ApiError> {
    Err(ApiError::unavailable())
}
