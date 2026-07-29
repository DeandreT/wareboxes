use wareboxes_core::dto::{OrderPage, SessionUser};
use wareboxes_core::models::{Facility, InventoryBalance, InventoryOwner, User};

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
    use wareboxes_core::dto::{ErrorResponse, LoginRequest};

    use super::{
        ApiError, Facility, InventoryBalance, InventoryOwner, OrderPage, SessionUser, User,
    };

    const API_BASE: Option<&str> = option_env!("WAREBOXES_API_URL");

    pub async fn login(email: String, password: String) -> Result<SessionUser, ApiError> {
        let request = Request::post(&url("/api/auth/login"))
            .json(&LoginRequest { email, password })
            .map_err(|error| ApiError {
                message: format!("Could not prepare the sign-in request: {error}"),
                unauthorized: false,
            })?;
        decode(request.send().await).await
    }

    pub async fn restore(session: &SessionUser) -> Result<User, ApiError> {
        get("/api/auth/me", session).await
    }

    pub async fn logout(session: &SessionUser) {
        let _ = authorized(Request::post(&url("/api/auth/logout")), session)
            .send()
            .await;
    }

    pub async fn orders(session: &SessionUser) -> Result<OrderPage, ApiError> {
        get("/api/orders?limit=50&offset=0", session).await
    }

    pub async fn balances(session: &SessionUser) -> Result<Vec<InventoryBalance>, ApiError> {
        get("/api/inventory/balances", session).await
    }

    pub async fn facilities(session: &SessionUser) -> Result<Vec<Facility>, ApiError> {
        get("/api/facilities", session).await
    }

    pub async fn inventory_owners(session: &SessionUser) -> Result<Vec<InventoryOwner>, ApiError> {
        get("/api/inventory-owners", session).await
    }

    async fn get<T: DeserializeOwned>(path: &str, session: &SessionUser) -> Result<T, ApiError> {
        decode(authorized(Request::get(&url(path)), session).send().await).await
    }

    fn authorized(
        builder: gloo_net::http::RequestBuilder,
        session: &SessionUser,
    ) -> gloo_net::http::RequestBuilder {
        builder
            .header("Authorization", &format!("Bearer {}", session.token))
            .header(
                "X-Wareboxes-Tenant-Id",
                &session.active_tenant.tenant_id.to_string(),
            )
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
        let Some(base) = API_BASE.map(str::trim).filter(|base| !base.is_empty()) else {
            return path.to_owned();
        };
        format!("{}{}", base.trim_end_matches('/'), path)
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::*;

#[cfg(not(target_arch = "wasm32"))]
pub async fn login(_email: String, _password: String) -> Result<SessionUser, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn restore(_session: &SessionUser) -> Result<User, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn logout(_session: &SessionUser) {}

#[cfg(not(target_arch = "wasm32"))]
pub async fn orders(_session: &SessionUser) -> Result<OrderPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn balances(_session: &SessionUser) -> Result<Vec<InventoryBalance>, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn facilities(_session: &SessionUser) -> Result<Vec<Facility>, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn inventory_owners(_session: &SessionUser) -> Result<Vec<InventoryOwner>, ApiError> {
    Err(ApiError::unavailable())
}
