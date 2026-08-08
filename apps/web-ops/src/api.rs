#[cfg(target_arch = "wasm32")]
use wareboxes_api_contract::v1::ReleaseInventoryHoldRequest;
use wareboxes_api_contract::v1::{
    CreateFulfillmentOrderRequest, CreateFulfillmentOrderResponse, InventoryBalancePage,
    InventoryHoldPage, InventoryHoldStatus, InventoryStatusTransitionResponse, OpaqueCursor,
    OrderAllocationReadinessResponse, OrderEntryItemResponse, PlaceInventoryHoldRequest,
    PlaceInventoryHoldResponse, PlaceOrderHoldRequest, PlaceOrderHoldResponse,
    PlanOrderAllocationRequest, PlanOrderAllocationResponse, ReleaseInventoryHoldResponse,
    ReleaseOrderHoldRequest, ReleaseOrderHoldResponse,
};
use wareboxes_api_contract::v1::{
    CreateInventoryRelocationTaskRequest, CreateInventoryRelocationTaskResponse,
    CreateInventoryStatusTransitionRequest,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::dto::{OrderPage, WebSessionContext};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub message: String,
    pub unauthorized: bool,
    /// The request may have reached the server, so an idempotent command must reuse its key.
    pub ambiguous_outcome: bool,
}

impl ApiError {
    #[cfg(not(target_arch = "wasm32"))]
    fn unavailable() -> Self {
        Self {
            message: "The browser API client is unavailable during server rendering.".to_owned(),
            unauthorized: false,
            ambiguous_outcome: false,
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use gloo_net::http::{Request, Response};
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use wareboxes_core::dto::{LoginRequest, SelectTenantRequest};

    use super::{
        AccessScopeWorkspace, ApiError, CreateFulfillmentOrderRequest,
        CreateFulfillmentOrderResponse, CreateInventoryRelocationTaskRequest,
        CreateInventoryRelocationTaskResponse, CreateInventoryStatusTransitionRequest,
        InventoryBalancePage, InventoryHoldPage, InventoryHoldStatus,
        InventoryStatusTransitionResponse, OpaqueCursor, OrderAllocationReadinessResponse,
        OrderEntryItemResponse, OrderPage, PlaceInventoryHoldRequest, PlaceInventoryHoldResponse,
        PlaceOrderHoldRequest, PlaceOrderHoldResponse, PlanOrderAllocationRequest,
        PlanOrderAllocationResponse, ReleaseInventoryHoldRequest, ReleaseInventoryHoldResponse,
        ReleaseOrderHoldRequest, ReleaseOrderHoldResponse, WebSessionContext,
    };

    #[derive(Deserialize)]
    struct WireError {
        message: String,
        #[serde(default)]
        request_id: String,
    }

    pub async fn login(email: String, password: String) -> Result<WebSessionContext, ApiError> {
        let request = Request::post(&url("/api/web/auth/login"))
            .json(&LoginRequest { email, password })
            .map_err(|error| ApiError {
                message: format!("Could not prepare the sign-in request: {error}"),
                unauthorized: false,
                ambiguous_outcome: false,
            })?;
        decode(request.send().await).await
    }

    pub async fn select_tenant(tenant_id: i64) -> Result<WebSessionContext, ApiError> {
        let request = Request::post(&url("/api/web/auth/tenant"))
            .json(&SelectTenantRequest { tenant_id })
            .map_err(|error| ApiError {
                message: format!("Could not prepare the organization switch request: {error}"),
                unauthorized: false,
                ambiguous_outcome: false,
            })?;
        decode(request.send().await).await
    }

    pub async fn logout() {
        let _ = Request::post(&url("/api/web/auth/logout")).send().await;
    }

    pub async fn orders() -> Result<OrderPage, ApiError> {
        get("/api/orders?limit=50&offset=0").await
    }

    pub async fn balances(cursor: Option<&OpaqueCursor>) -> Result<InventoryBalancePage, ApiError> {
        let path = balance_page_path(None, cursor);
        get(&path).await
    }

    pub async fn search_balances(
        query: &str,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<InventoryBalancePage, ApiError> {
        let path = balance_page_path(Some(query), cursor);
        get(&path).await
    }

    pub async fn access() -> Result<AccessScopeWorkspace, ApiError> {
        get("/api/web/access").await
    }

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
        get(&path).await
    }

    pub async fn create_fulfillment_order(
        request: &CreateFulfillmentOrderRequest,
        idempotency_key: &str,
    ) -> Result<CreateFulfillmentOrderResponse, ApiError> {
        post("/api/v1/orders", request, idempotency_key).await
    }

    pub async fn order_allocation_readiness(
        order_id: i64,
        facility_id: i64,
    ) -> Result<OrderAllocationReadinessResponse, ApiError> {
        get(&format!(
            "/api/v1/orders/{order_id}/allocation-readiness?facility_id={facility_id}"
        ))
        .await
    }

    pub async fn plan_order_allocation(
        order_id: i64,
        request: &PlanOrderAllocationRequest,
        idempotency_key: &str,
    ) -> Result<PlanOrderAllocationResponse, ApiError> {
        post(
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn internal_get<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
        get(path).await
    }

    pub async fn internal_post<TRequest, TResponse>(
        path: &str,
        request: &TRequest,
    ) -> Result<TResponse, ApiError>
    where
        TRequest: serde::Serialize,
        TResponse: DeserializeOwned,
    {
        let request = Request::post(&url(path))
            .json(request)
            .map_err(|error| ApiError {
                message: format!("Could not prepare the command: {error}"),
                unauthorized: false,
                ambiguous_outcome: false,
            })?;
        decode(request.send().await).await
    }

    pub async fn internal_post_idempotent<TRequest, TResponse>(
        path: &str,
        request: &TRequest,
        idempotency_key: &str,
    ) -> Result<TResponse, ApiError>
    where
        TRequest: serde::Serialize,
        TResponse: DeserializeOwned,
    {
        post(path, request, idempotency_key).await
    }

    pub async fn holds(
        status: InventoryHoldStatus,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<InventoryHoldPage, ApiError> {
        let status = match status {
            InventoryHoldStatus::Active => "active",
            InventoryHoldStatus::Released => "released",
        };
        let mut path = format!("/api/v1/inventory/holds?limit=100&status={status}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(cursor.as_str());
        }
        get(&path).await
    }

    pub async fn place_hold(
        request: &PlaceInventoryHoldRequest,
        idempotency_key: &str,
    ) -> Result<PlaceInventoryHoldResponse, ApiError> {
        post("/api/v1/inventory/holds", request, idempotency_key).await
    }

    pub async fn release_hold(
        hold_id: i64,
        idempotency_key: &str,
    ) -> Result<ReleaseInventoryHoldResponse, ApiError> {
        post(
            &format!("/api/v1/inventory/holds/{hold_id}/releases"),
            &ReleaseInventoryHoldRequest::default(),
            idempotency_key,
        )
        .await
    }

    pub async fn place_order_hold(
        order_id: i64,
        request: &PlaceOrderHoldRequest,
        idempotency_key: &str,
    ) -> Result<PlaceOrderHoldResponse, ApiError> {
        post(
            &format!("/api/v1/orders/{order_id}/holds"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn release_order_hold(
        order_id: i64,
        hold_id: i64,
        request: &ReleaseOrderHoldRequest,
        idempotency_key: &str,
    ) -> Result<ReleaseOrderHoldResponse, ApiError> {
        post(
            &format!("/api/v1/orders/{order_id}/holds/{hold_id}/releases"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn transition_inventory_status(
        balance_id: i64,
        request: &CreateInventoryStatusTransitionRequest,
        idempotency_key: &str,
    ) -> Result<InventoryStatusTransitionResponse, ApiError> {
        post(
            &format!("/api/v1/inventory/balances/{balance_id}/status-transitions"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn create_inventory_relocation_task(
        request: &CreateInventoryRelocationTaskRequest,
        idempotency_key: &str,
    ) -> Result<CreateInventoryRelocationTaskResponse, ApiError> {
        post(
            "/api/v1/inventory-relocation-tasks",
            request,
            idempotency_key,
        )
        .await
    }

    pub fn new_idempotency_key() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    async fn get<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
        decode(Request::get(&url(path)).send().await).await
    }

    async fn post<TRequest, TResponse>(
        path: &str,
        body: &TRequest,
        idempotency_key: &str,
    ) -> Result<TResponse, ApiError>
    where
        TRequest: serde::Serialize,
        TResponse: DeserializeOwned,
    {
        let request = Request::post(&url(path))
            .header("Idempotency-Key", idempotency_key)
            .json(body)
            .map_err(|error| ApiError {
                message: format!("Could not prepare the command: {error}"),
                unauthorized: false,
                ambiguous_outcome: false,
            })?;
        decode(request.send().await).await
    }

    async fn decode<T: DeserializeOwned>(
        response: Result<Response, gloo_net::Error>,
    ) -> Result<T, ApiError> {
        let response = response.map_err(|error| ApiError {
            message: format!("Wareboxes could not reach the server: {error}"),
            unauthorized: false,
            ambiguous_outcome: true,
        })?;
        let status = response.status();
        if (200..300).contains(&status) {
            return response.json::<T>().await.map_err(|error| ApiError {
                message: format!("The server returned an unreadable response: {error}"),
                unauthorized: false,
                ambiguous_outcome: true,
            });
        }

        let unauthorized = status == 401;
        let message = response
            .json::<WireError>()
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
            ambiguous_outcome: false,
        })
    }

    fn url(path: &str) -> String {
        path.to_owned()
    }

    fn balance_page_path(query: Option<&str>, cursor: Option<&OpaqueCursor>) -> String {
        let mut path = "/api/v1/inventory/balances?limit=100".to_owned();
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            path.push_str("&query=");
            path.push_str(&urlencoding::encode(query));
        }
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(cursor.as_str());
        }
        path
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
pub async fn balances(_cursor: Option<&OpaqueCursor>) -> Result<InventoryBalancePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn search_balances(
    _query: &str,
    _cursor: Option<&OpaqueCursor>,
) -> Result<InventoryBalancePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn access() -> Result<AccessScopeWorkspace, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn order_entry_items(
    _inventory_owner_id: i64,
    _search: &str,
) -> Result<Vec<OrderEntryItemResponse>, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_fulfillment_order(
    _request: &CreateFulfillmentOrderRequest,
    _idempotency_key: &str,
) -> Result<CreateFulfillmentOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn order_allocation_readiness(
    _order_id: i64,
    _facility_id: i64,
) -> Result<OrderAllocationReadinessResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan_order_allocation(
    _order_id: i64,
    _request: &PlanOrderAllocationRequest,
    _idempotency_key: &str,
) -> Result<PlanOrderAllocationResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn internal_get<T>(_path: &str) -> Result<T, ApiError>
where
    T: serde::de::DeserializeOwned,
{
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn internal_post<TRequest, TResponse>(
    _path: &str,
    _request: &TRequest,
) -> Result<TResponse, ApiError>
where
    TRequest: serde::Serialize,
    TResponse: serde::de::DeserializeOwned,
{
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn internal_post_idempotent<TRequest, TResponse>(
    _path: &str,
    _request: &TRequest,
    _idempotency_key: &str,
) -> Result<TResponse, ApiError>
where
    TRequest: serde::Serialize,
    TResponse: serde::de::DeserializeOwned,
{
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn holds(
    _status: InventoryHoldStatus,
    _cursor: Option<&OpaqueCursor>,
) -> Result<InventoryHoldPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn place_hold(
    _request: &PlaceInventoryHoldRequest,
    _idempotency_key: &str,
) -> Result<PlaceInventoryHoldResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn release_hold(
    _hold_id: i64,
    _idempotency_key: &str,
) -> Result<ReleaseInventoryHoldResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn place_order_hold(
    _order_id: i64,
    _request: &PlaceOrderHoldRequest,
    _idempotency_key: &str,
) -> Result<PlaceOrderHoldResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn release_order_hold(
    _order_id: i64,
    _hold_id: i64,
    _request: &ReleaseOrderHoldRequest,
    _idempotency_key: &str,
) -> Result<ReleaseOrderHoldResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn transition_inventory_status(
    _balance_id: i64,
    _request: &CreateInventoryStatusTransitionRequest,
    _idempotency_key: &str,
) -> Result<InventoryStatusTransitionResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_inventory_relocation_task(
    _request: &CreateInventoryRelocationTaskRequest,
    _idempotency_key: &str,
) -> Result<CreateInventoryRelocationTaskResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn new_idempotency_key() -> String {
    "server-rendering-does-not-submit-commands".to_owned()
}
