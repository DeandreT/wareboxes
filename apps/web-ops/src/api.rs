#[cfg(target_arch = "wasm32")]
use wareboxes_api_contract::v1::ReleaseInventoryHoldRequest;
use wareboxes_api_contract::v1::{
    AbandonPackSessionRequest, AbandonPackSessionResponse, AcceptPickShortageAsShortShipRequest,
    AcceptPickShortageAsShortShipResponse, CancelOrderRequest, CancelOrderResponse,
    CancelReplenishmentWorkRequest, CloseCartonRequest, CloseCartonResponse,
    ConfigureFacilityShippingOriginRequest, ConfigureFacilityShippingOriginResponse,
    ConfigureReplenishmentPolicyRequest, ConfigureReplenishmentPolicyResponse, CreateCartonRequest,
    CreateCartonResponse, InventoryBalancePage, InventoryBalanceSort, InventoryHoldPage,
    InventoryHoldStatus, InventorySortDirection, InventoryStatusTransitionResponse, OpaqueCursor,
    OpenPackSessionRequest, OpenPackSessionResponse, OrderAllocationReadinessResponse,
    PackPickedAllocationRequest, PackPickedAllocationResponse, PackSessionResponse,
    PackingQueuePage, PickConfirmationHistoryPage, PickShortagePage, PickShortageQueueSort,
    PickShortageQueueSortDirection, PickShortageResponse, PickShortageStatus,
    PlaceInventoryHoldRequest, PlaceInventoryHoldResponse, PlaceOrderHoldRequest,
    PlaceOrderHoldResponse, PlanOrderAllocationRequest, PlanOrderAllocationResponse,
    PlanReplenishmentRequest, PlanReplenishmentResponse, ReallocatePickShortageRequest,
    ReallocatePickShortageResponse, ReleaseInventoryHoldResponse, ReleaseOrderHoldRequest,
    ReleaseOrderHoldResponse, ReleaseOrderRequest, ReleaseOrderResponse,
    RemovePackedContentRequest, RemovePackedContentResponse, ReopenCartonRequest,
    ReopenCartonResponse, ReplenishmentPolicyPage, ReplenishmentPolicySort,
    ReplenishmentPolicySortDirection, ReplenishmentQueuePage,
    ReplenishmentWorkCancellationResponse, ReplenishmentWorkSort, ReplenishmentWorkSortDirection,
    ReplenishmentWorkStatus, RetireReplenishmentPolicyRequest, RetireReplenishmentPolicyResponse,
    ReversePickConfirmationRequest, ReversePickConfirmationResponse, VoidCartonRequest,
    VoidCartonResponse,
};
use wareboxes_api_contract::v1::{
    CreateInventoryRelocationTaskRequest, CreateInventoryRelocationTaskResponse,
    CreateInventoryStatusTransitionRequest,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::dto::{OrderPage, WebSessionContext};

mod backorder;
pub use backorder::{configure_backorder_policy, split_order_backorder};
mod cycle_count;
pub use cycle_count::{
    configure_cycle_count_policy, create_cycle_count_task, cycle_count_candidates,
    cycle_count_policies, cycle_count_variances, cycle_count_work, decide_cycle_count_variance,
};
mod expected_receiving;
pub use expected_receiving::expected_receiving_session;
mod item_substitution;
pub use item_substitution::{
    configure_item_substitution_policy, item_substitution_policies,
    retire_item_substitution_policy, substitute_pick_shortage,
};
mod inbound_inspection;
pub use inbound_inspection::dispose_inbound_inspection;
mod inbound_load;
pub use inbound_load::{
    arrive_inbound_load, cancel_inbound_load, close_inbound_load, inbound_load_entry_items,
    plan_inbound_load, reject_inbound_load, schedule_inbound_load, start_inbound_load_unloading,
};
mod integration_monitor;
pub use integration_monitor::{
    correct_inbound_order, discard_outbound_dead_letter, inbound_integration_detail,
    inbound_integrations, inbound_payload_download_path, outbound_integration_detail,
    outbound_integrations, replay_outbound_dead_letter, reprocess_inbound_order,
    InboundIntegrationFilters, OutboundIntegrationFilters,
};
mod integration_mapping;
pub use integration_mapping::{
    configure_integration_order_item_mapping, configure_integration_order_owner_mapping,
    integration_order_item_mappings, integration_order_owner_mappings,
    retire_integration_order_item_mapping, retire_integration_order_owner_mapping,
    IntegrationMappingFilters, IntegrationOwnerMappingFilters,
};
mod inventory_integrity;
pub use inventory_integrity::{
    create_inventory_recall, inventory_aging, inventory_integrity_issues, inventory_journal,
    inventory_recalls, release_inventory_recall, AgingFilters, IntegrityFilters, JournalFilters,
};
mod item_storage_policy;
pub use item_storage_policy::{
    configure_item_storage_policy, item_storage_policies, retire_item_storage_policy,
};
mod item_traceability_policy;
pub use item_traceability_policy::{
    configure_item_traceability_policy, item_traceability_policies,
    retire_item_traceability_policy, ItemTraceabilityPolicyFilters,
};
mod order;
pub use order::{
    amend_fulfillment_order, create_fulfillment_order, order_entry_items,
    replace_fulfillment_order_lines,
};
mod pick_wave;
pub use pick_wave::{cancel_pick_wave, pick_wave, pick_waves, plan_pick_wave, release_pick_wave};
mod putaway;
pub use putaway::{create_license_plate_putaway, create_putaway, putaway_candidates, putaway_work};
mod storage_zone;
pub use storage_zone::{configure_storage_zone, retire_storage_zone, storage_zones};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub message: String,
    pub unauthorized: bool,
    /// The request may have reached the server, so an idempotent command must reuse its key.
    pub ambiguous_outcome: bool,
}

#[cfg(target_arch = "wasm32")]
pub type BrowserUploadFile = web_sys::File;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct BrowserUploadFile;

#[derive(Clone, Copy)]
pub struct ReplenishmentQueueFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub item_id: Option<i64>,
    pub pick_face_location_id: Option<i64>,
    pub status: Option<ReplenishmentWorkStatus>,
    pub sort: ReplenishmentWorkSort,
    pub direction: ReplenishmentWorkSortDirection,
}

#[derive(Clone)]
pub struct PickShortageFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub order_id: Option<i64>,
    pub order_key: Option<String>,
    pub status: Option<PickShortageStatus>,
    pub sort: PickShortageQueueSort,
    pub direction: PickShortageQueueSortDirection,
}

#[derive(Clone, Copy)]
pub struct ReplenishmentPolicyFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub item_id: Option<i64>,
    pub pick_face_location_id: Option<i64>,
    pub sort: ReplenishmentPolicySort,
    pub direction: ReplenishmentPolicySortDirection,
}

impl Default for ReplenishmentPolicyFilters {
    fn default() -> Self {
        Self {
            facility_id: None,
            inventory_owner_id: None,
            item_id: None,
            pick_face_location_id: None,
            sort: ReplenishmentPolicySort::TargetGap,
            direction: ReplenishmentPolicySortDirection::Descending,
        }
    }
}

impl Default for ReplenishmentQueueFilters {
    fn default() -> Self {
        Self {
            facility_id: None,
            inventory_owner_id: None,
            item_id: None,
            pick_face_location_id: None,
            status: None,
            sort: ReplenishmentWorkSort::Priority,
            direction: ReplenishmentWorkSortDirection::Descending,
        }
    }
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
        AbandonPackSessionRequest, AbandonPackSessionResponse,
        AcceptPickShortageAsShortShipRequest, AcceptPickShortageAsShortShipResponse,
        AccessScopeWorkspace, ApiError, CancelOrderRequest, CancelOrderResponse,
        CancelReplenishmentWorkRequest, CloseCartonRequest, CloseCartonResponse,
        ConfigureFacilityShippingOriginRequest, ConfigureFacilityShippingOriginResponse,
        ConfigureReplenishmentPolicyRequest, ConfigureReplenishmentPolicyResponse,
        CreateCartonRequest, CreateCartonResponse, CreateInventoryRelocationTaskRequest,
        CreateInventoryRelocationTaskResponse, CreateInventoryStatusTransitionRequest,
        InventoryBalancePage, InventoryBalanceSort, InventoryHoldPage, InventoryHoldStatus,
        InventorySortDirection, InventoryStatusTransitionResponse, OpaqueCursor,
        OpenPackSessionRequest, OpenPackSessionResponse, OrderAllocationReadinessResponse,
        OrderPage, PackPickedAllocationRequest, PackPickedAllocationResponse, PackSessionResponse,
        PackingQueuePage, PickConfirmationHistoryPage, PickShortageFilters, PickShortagePage,
        PickShortageResponse, PlaceInventoryHoldRequest, PlaceInventoryHoldResponse,
        PlaceOrderHoldRequest, PlaceOrderHoldResponse, PlanOrderAllocationRequest,
        PlanOrderAllocationResponse, PlanReplenishmentRequest, PlanReplenishmentResponse,
        ReallocatePickShortageRequest, ReallocatePickShortageResponse, ReleaseInventoryHoldRequest,
        ReleaseInventoryHoldResponse, ReleaseOrderHoldRequest, ReleaseOrderHoldResponse,
        ReleaseOrderRequest, ReleaseOrderResponse, RemovePackedContentRequest,
        RemovePackedContentResponse, ReopenCartonRequest, ReopenCartonResponse,
        ReplenishmentPolicyPage, ReplenishmentQueuePage, ReplenishmentWorkCancellationResponse,
        RetireReplenishmentPolicyRequest, RetireReplenishmentPolicyResponse,
        ReversePickConfirmationRequest, ReversePickConfirmationResponse, VoidCartonRequest,
        VoidCartonResponse, WebSessionContext,
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

    pub async fn orders_workbench() -> Result<OrderPage, ApiError> {
        get("/api/orders?limit=100&offset=0&sort=order&direction=desc").await
    }

    pub async fn balances(cursor: Option<&OpaqueCursor>) -> Result<InventoryBalancePage, ApiError> {
        let path = balance_page_path(
            None,
            InventoryBalanceSort::Position,
            InventorySortDirection::Ascending,
            cursor,
            false,
        );
        get(&path).await
    }

    pub async fn search_balances(
        query: &str,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<InventoryBalancePage, ApiError> {
        let path = balance_page_path(
            Some(query),
            InventoryBalanceSort::Position,
            InventorySortDirection::Ascending,
            cursor,
            false,
        );
        get(&path).await
    }

    pub async fn sorted_balances(
        query: Option<&str>,
        sort: InventoryBalanceSort,
        direction: InventorySortDirection,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<InventoryBalancePage, ApiError> {
        get(&balance_page_path(query, sort, direction, cursor, false)).await
    }

    pub async fn sorted_movable_balances(
        query: Option<&str>,
        sort: InventoryBalanceSort,
        direction: InventorySortDirection,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<InventoryBalancePage, ApiError> {
        get(&balance_page_path(query, sort, direction, cursor, true)).await
    }

    pub async fn access() -> Result<AccessScopeWorkspace, ApiError> {
        get("/api/web/access").await
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

    pub async fn release_order(
        order_id: i64,
        request: &ReleaseOrderRequest,
        idempotency_key: &str,
    ) -> Result<ReleaseOrderResponse, ApiError> {
        post(
            &format!("/api/v1/orders/{order_id}/releases"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn cancel_order(
        order_id: i64,
        request: &CancelOrderRequest,
        idempotency_key: &str,
    ) -> Result<CancelOrderResponse, ApiError> {
        post(
            &format!("/api/v1/orders/{order_id}/cancellations"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn pack_session_for_order(
        order_id: i64,
    ) -> Result<Option<PackSessionResponse>, ApiError> {
        get(&format!("/api/v1/orders/{order_id}/packing-session")).await
    }

    pub async fn packing_queue(
        facility_id: Option<i64>,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<PackingQueuePage, ApiError> {
        let mut path = "/api/v1/packing-queue?limit=100".to_owned();
        if let Some(facility_id) = facility_id {
            path.push_str("&facility_id=");
            path.push_str(&facility_id.to_string());
        }
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&urlencoding::encode(cursor.as_str()));
        }
        get(&path).await
    }

    pub async fn open_pack_session(
        order_id: i64,
        request: &OpenPackSessionRequest,
        idempotency_key: &str,
    ) -> Result<OpenPackSessionResponse, ApiError> {
        post(
            &format!("/api/v1/orders/{order_id}/packing-sessions"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn create_pack_carton(
        session_id: i64,
        request: &CreateCartonRequest,
        idempotency_key: &str,
    ) -> Result<CreateCartonResponse, ApiError> {
        post(
            &format!("/api/v1/packing-sessions/{session_id}/cartons"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn pack_allocation(
        session_id: i64,
        carton_id: i64,
        request: &PackPickedAllocationRequest,
        idempotency_key: &str,
    ) -> Result<PackPickedAllocationResponse, ApiError> {
        post(
            &format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/contents"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn close_pack_carton(
        session_id: i64,
        carton_id: i64,
        request: &CloseCartonRequest,
        idempotency_key: &str,
    ) -> Result<CloseCartonResponse, ApiError> {
        post(
            &format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/closures"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn remove_pack_content(
        session_id: i64,
        carton_id: i64,
        content_id: i64,
        request: &RemovePackedContentRequest,
        idempotency_key: &str,
    ) -> Result<RemovePackedContentResponse, ApiError> {
        post(
            &format!(
                "/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/contents/{content_id}/removals"
            ),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn void_pack_carton(
        session_id: i64,
        carton_id: i64,
        request: &VoidCartonRequest,
        idempotency_key: &str,
    ) -> Result<VoidCartonResponse, ApiError> {
        post(
            &format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/voids"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn abandon_pack_session(
        session_id: i64,
        request: &AbandonPackSessionRequest,
        idempotency_key: &str,
    ) -> Result<AbandonPackSessionResponse, ApiError> {
        post(
            &format!("/api/v1/packing-sessions/{session_id}/abandonments"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn reopen_pack_carton(
        session_id: i64,
        carton_id: i64,
        request: &ReopenCartonRequest,
        idempotency_key: &str,
    ) -> Result<ReopenCartonResponse, ApiError> {
        post(
            &format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/reopenings"),
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

    pub async fn upload_load_file(
        load_id: i64,
        category: &str,
        file: super::BrowserUploadFile,
    ) -> Result<i64, ApiError> {
        let form = web_sys::FormData::new().map_err(|error| ApiError {
            message: format!("Could not prepare the document upload: {error:?}"),
            unauthorized: false,
            ambiguous_outcome: false,
        })?;
        form.append_with_str("category", category)
            .map_err(|error| ApiError {
                message: format!("Could not prepare the document category: {error:?}"),
                unauthorized: false,
                ambiguous_outcome: false,
            })?;
        form.append_with_blob_and_filename("file", file.as_ref(), &file.name())
            .map_err(|error| ApiError {
                message: format!("Could not prepare the selected document: {error:?}"),
                unauthorized: false,
                ambiguous_outcome: false,
            })?;
        let request = Request::post(&url(&format!("/api/loads/{load_id}/files/upload")))
            .body(form)
            .map_err(|error| ApiError {
                message: format!("Could not prepare the document upload: {error}"),
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

    pub async fn configure_facility_shipping_origin(
        facility_id: i64,
        request: &ConfigureFacilityShippingOriginRequest,
        idempotency_key: &str,
    ) -> Result<ConfigureFacilityShippingOriginResponse, ApiError> {
        post(
            &format!("/api/v1/facilities/{facility_id}/shipping-origin-configurations"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn pick_shortages(
        filters: &PickShortageFilters,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<PickShortagePage, ApiError> {
        get(&super::pick_shortage_page_path(filters, cursor)).await
    }

    pub async fn pick_shortage(shortage_id: i64) -> Result<PickShortageResponse, ApiError> {
        get(&format!("/api/v1/pick-shortages/{shortage_id}")).await
    }

    pub async fn reallocate_pick_shortage(
        shortage_id: i64,
        request: &ReallocatePickShortageRequest,
        idempotency_key: &str,
    ) -> Result<ReallocatePickShortageResponse, ApiError> {
        post(
            &format!("/api/v1/pick-shortages/{shortage_id}/reallocations"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn accept_pick_shortage_as_short_ship(
        shortage_id: i64,
        request: &AcceptPickShortageAsShortShipRequest,
        idempotency_key: &str,
    ) -> Result<AcceptPickShortageAsShortShipResponse, ApiError> {
        post(
            &format!("/api/v1/pick-shortages/{shortage_id}/short-ship-dispositions"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn replenishment_policies(
        filters: super::ReplenishmentPolicyFilters,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<ReplenishmentPolicyPage, ApiError> {
        get(&super::replenishment_policy_page_path(filters, cursor)).await
    }

    pub async fn replenishment_queue(
        filters: super::ReplenishmentQueueFilters,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<ReplenishmentQueuePage, ApiError> {
        get(&super::replenishment_queue_page_path(filters, cursor)).await
    }

    pub async fn configure_replenishment_policy(
        request: &ConfigureReplenishmentPolicyRequest,
        idempotency_key: &str,
    ) -> Result<ConfigureReplenishmentPolicyResponse, ApiError> {
        post("/api/v1/replenishment-policies", request, idempotency_key).await
    }

    pub async fn retire_replenishment_policy(
        policy_id: i64,
        request: &RetireReplenishmentPolicyRequest,
        idempotency_key: &str,
    ) -> Result<RetireReplenishmentPolicyResponse, ApiError> {
        post(
            &format!("/api/v1/replenishment-policies/{policy_id}/retirements"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn plan_replenishment(
        policy_id: i64,
        request: &PlanReplenishmentRequest,
        idempotency_key: &str,
    ) -> Result<PlanReplenishmentResponse, ApiError> {
        post(
            &format!("/api/v1/replenishment-policies/{policy_id}/plan-runs"),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn cancel_replenishment_work(
        work_id: i64,
        request: &CancelReplenishmentWorkRequest,
        idempotency_key: &str,
    ) -> Result<ReplenishmentWorkCancellationResponse, ApiError> {
        post(
            &super::replenishment_cancellation_path(work_id),
            request,
            idempotency_key,
        )
        .await
    }

    pub async fn pick_confirmation_history(
        order_id: i64,
        cursor: Option<&OpaqueCursor>,
    ) -> Result<PickConfirmationHistoryPage, ApiError> {
        get(&super::pick_confirmation_history_path(order_id, cursor)).await
    }

    pub async fn reverse_pick_confirmation(
        confirmation_id: i64,
        request: &ReversePickConfirmationRequest,
        idempotency_key: &str,
    ) -> Result<ReversePickConfirmationResponse, ApiError> {
        post(
            &super::pick_reversal_path(confirmation_id),
            request,
            idempotency_key,
        )
        .await
    }

    pub fn new_idempotency_key() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    pub(super) async fn get<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
        decode(Request::get(&url(path)).send().await).await
    }

    pub(super) async fn post<TRequest, TResponse>(
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

    fn balance_page_path(
        query: Option<&str>,
        sort: InventoryBalanceSort,
        direction: InventorySortDirection,
        cursor: Option<&OpaqueCursor>,
        movable_only: bool,
    ) -> String {
        let mut path = format!(
            "/api/v1/inventory/balances?limit=100&sort={}&direction={}",
            inventory_balance_sort_wire(sort),
            inventory_sort_direction_wire(direction)
        );
        if movable_only {
            path.push_str("&movable_only=true");
        }
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

    const fn inventory_balance_sort_wire(sort: InventoryBalanceSort) -> &'static str {
        match sort {
            InventoryBalanceSort::Position => "position",
            InventoryBalanceSort::Facility => "facility",
            InventoryBalanceSort::Client => "client",
            InventoryBalanceSort::Location => "location",
            InventoryBalanceSort::Item => "item",
            InventoryBalanceSort::Tracking => "tracking",
            InventoryBalanceSort::LicensePlate => "license_plate",
            InventoryBalanceSort::Status => "status",
            InventoryBalanceSort::OnHand => "on_hand",
            InventoryBalanceSort::Reserved => "reserved",
            InventoryBalanceSort::Held => "held",
            InventoryBalanceSort::Available => "available",
        }
    }

    const fn inventory_sort_direction_wire(direction: InventorySortDirection) -> &'static str {
        match direction {
            InventorySortDirection::Ascending => "ascending",
            InventorySortDirection::Descending => "descending",
        }
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
pub async fn orders_workbench() -> Result<OrderPage, ApiError> {
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
pub async fn sorted_balances(
    _query: Option<&str>,
    _sort: InventoryBalanceSort,
    _direction: InventorySortDirection,
    _cursor: Option<&OpaqueCursor>,
) -> Result<InventoryBalancePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn sorted_movable_balances(
    _query: Option<&str>,
    _sort: InventoryBalanceSort,
    _direction: InventorySortDirection,
    _cursor: Option<&OpaqueCursor>,
) -> Result<InventoryBalancePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn access() -> Result<AccessScopeWorkspace, ApiError> {
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
pub async fn release_order(
    _order_id: i64,
    _request: &ReleaseOrderRequest,
    _idempotency_key: &str,
) -> Result<ReleaseOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel_order(
    _order_id: i64,
    _request: &CancelOrderRequest,
    _idempotency_key: &str,
) -> Result<CancelOrderResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn pack_session_for_order(
    _order_id: i64,
) -> Result<Option<PackSessionResponse>, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn packing_queue(
    _facility_id: Option<i64>,
    _cursor: Option<&OpaqueCursor>,
) -> Result<PackingQueuePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn open_pack_session(
    _order_id: i64,
    _request: &OpenPackSessionRequest,
    _idempotency_key: &str,
) -> Result<OpenPackSessionResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_pack_carton(
    _session_id: i64,
    _request: &CreateCartonRequest,
    _idempotency_key: &str,
) -> Result<CreateCartonResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn pack_allocation(
    _session_id: i64,
    _carton_id: i64,
    _request: &PackPickedAllocationRequest,
    _idempotency_key: &str,
) -> Result<PackPickedAllocationResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn close_pack_carton(
    _session_id: i64,
    _carton_id: i64,
    _request: &CloseCartonRequest,
    _idempotency_key: &str,
) -> Result<CloseCartonResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn remove_pack_content(
    _session_id: i64,
    _carton_id: i64,
    _content_id: i64,
    _request: &RemovePackedContentRequest,
    _idempotency_key: &str,
) -> Result<RemovePackedContentResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn void_pack_carton(
    _session_id: i64,
    _carton_id: i64,
    _request: &VoidCartonRequest,
    _idempotency_key: &str,
) -> Result<VoidCartonResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn abandon_pack_session(
    _session_id: i64,
    _request: &AbandonPackSessionRequest,
    _idempotency_key: &str,
) -> Result<AbandonPackSessionResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn reopen_pack_carton(
    _session_id: i64,
    _carton_id: i64,
    _request: &ReopenCartonRequest,
    _idempotency_key: &str,
) -> Result<ReopenCartonResponse, ApiError> {
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
pub async fn upload_load_file(
    _load_id: i64,
    _category: &str,
    _file: BrowserUploadFile,
) -> Result<i64, ApiError> {
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
pub async fn configure_facility_shipping_origin(
    _facility_id: i64,
    _request: &ConfigureFacilityShippingOriginRequest,
    _idempotency_key: &str,
) -> Result<ConfigureFacilityShippingOriginResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn pick_shortages(
    _filters: &PickShortageFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<PickShortagePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn pick_shortage(_shortage_id: i64) -> Result<PickShortageResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn reallocate_pick_shortage(
    _shortage_id: i64,
    _request: &ReallocatePickShortageRequest,
    _idempotency_key: &str,
) -> Result<ReallocatePickShortageResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn accept_pick_shortage_as_short_ship(
    _shortage_id: i64,
    _request: &AcceptPickShortageAsShortShipRequest,
    _idempotency_key: &str,
) -> Result<AcceptPickShortageAsShortShipResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn replenishment_policies(
    _filters: ReplenishmentPolicyFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<ReplenishmentPolicyPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn replenishment_queue(
    _filters: ReplenishmentQueueFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<ReplenishmentQueuePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_replenishment_policy(
    _request: &ConfigureReplenishmentPolicyRequest,
    _idempotency_key: &str,
) -> Result<ConfigureReplenishmentPolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn retire_replenishment_policy(
    _policy_id: i64,
    _request: &RetireReplenishmentPolicyRequest,
    _idempotency_key: &str,
) -> Result<RetireReplenishmentPolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan_replenishment(
    _policy_id: i64,
    _request: &PlanReplenishmentRequest,
    _idempotency_key: &str,
) -> Result<PlanReplenishmentResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel_replenishment_work(
    _work_id: i64,
    _request: &CancelReplenishmentWorkRequest,
    _idempotency_key: &str,
) -> Result<ReplenishmentWorkCancellationResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn pick_confirmation_history(
    _order_id: i64,
    _cursor: Option<&OpaqueCursor>,
) -> Result<PickConfirmationHistoryPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn reverse_pick_confirmation(
    _confirmation_id: i64,
    _request: &ReversePickConfirmationRequest,
    _idempotency_key: &str,
) -> Result<ReversePickConfirmationResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn new_idempotency_key() -> String {
    "server-rendering-does-not-submit-commands".to_owned()
}

#[cfg(any(target_arch = "wasm32", test))]
fn replenishment_policy_page_path(
    filters: ReplenishmentPolicyFilters,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/replenishment-policies?limit=100&sort={}&direction={}",
        replenishment_policy_sort_wire(filters.sort),
        replenishment_policy_sort_direction_wire(filters.direction),
    );
    append_optional_id(&mut path, "facility_id", filters.facility_id);
    append_optional_id(&mut path, "inventory_owner_id", filters.inventory_owner_id);
    append_optional_id(&mut path, "item_id", filters.item_id);
    append_optional_id(
        &mut path,
        "pick_face_location_id",
        filters.pick_face_location_id,
    );
    append_cursor(&mut path, cursor);
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn replenishment_queue_page_path(
    filters: ReplenishmentQueueFilters,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/replenishment-queue?limit=100&sort={}&direction={}",
        replenishment_work_sort_wire(filters.sort),
        replenishment_work_sort_direction_wire(filters.direction),
    );
    append_optional_id(&mut path, "facility_id", filters.facility_id);
    append_optional_id(&mut path, "inventory_owner_id", filters.inventory_owner_id);
    append_optional_id(&mut path, "item_id", filters.item_id);
    append_optional_id(
        &mut path,
        "pick_face_location_id",
        filters.pick_face_location_id,
    );
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(replenishment_work_status_wire(status));
    }
    append_cursor(&mut path, cursor);
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn pick_shortage_page_path(filters: &PickShortageFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = format!(
        "/api/v1/pick-shortages?limit=100&sort={}&direction={}",
        pick_shortage_sort_wire(filters.sort),
        pick_shortage_sort_direction_wire(filters.direction),
    );
    append_optional_id(&mut path, "facility_id", filters.facility_id);
    append_optional_id(&mut path, "inventory_owner_id", filters.inventory_owner_id);
    append_optional_id(&mut path, "order_id", filters.order_id);
    if let Some(order_key) = filters.order_key.as_deref() {
        path.push_str("&order_key=");
        path.push_str(&urlencoding::encode(order_key));
    }
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(pick_shortage_status_wire(status));
    }
    append_cursor(&mut path, cursor);
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn replenishment_cancellation_path(work_id: i64) -> String {
    format!("/api/v1/replenishment-tasks/{work_id}/cancellations")
}

#[cfg(any(target_arch = "wasm32", test))]
fn pick_confirmation_history_path(order_id: i64, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = format!("/api/v1/orders/{order_id}/pick-confirmations?limit=50");
    append_cursor(&mut path, cursor);
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn pick_reversal_path(confirmation_id: i64) -> String {
    format!("/api/v1/pick-confirmations/{confirmation_id}/reversals")
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_optional_id(path: &mut String, name: &str, value: Option<i64>) {
    if let Some(value) = value {
        path.push('&');
        path.push_str(name);
        path.push('=');
        path.push_str(&value.to_string());
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_cursor(path: &mut String, cursor: Option<&OpaqueCursor>) {
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn pick_shortage_status_wire(status: PickShortageStatus) -> &'static str {
    match status {
        PickShortageStatus::AwaitingInventory => "awaiting_inventory",
        PickShortageStatus::RecoveryInProgress => "recovery_in_progress",
        PickShortageStatus::Resolved => "resolved",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn pick_shortage_sort_wire(sort: PickShortageQueueSort) -> &'static str {
    match sort {
        PickShortageQueueSort::Reported => "reported",
        PickShortageQueueSort::Order => "order",
        PickShortageQueueSort::Status => "status",
        PickShortageQueueSort::ShortQuantity => "short_quantity",
        PickShortageQueueSort::RemainingQuantity => "remaining_quantity",
        PickShortageQueueSort::InventoryOwner => "inventory_owner",
        PickShortageQueueSort::Item => "item",
        PickShortageQueueSort::Facility => "facility",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn pick_shortage_sort_direction_wire(
    direction: PickShortageQueueSortDirection,
) -> &'static str {
    match direction {
        PickShortageQueueSortDirection::Ascending => "ascending",
        PickShortageQueueSortDirection::Descending => "descending",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn replenishment_work_status_wire(status: ReplenishmentWorkStatus) -> &'static str {
    match status {
        ReplenishmentWorkStatus::Pending => "pending",
        ReplenishmentWorkStatus::Claimed => "claimed",
        ReplenishmentWorkStatus::Completed => "completed",
        ReplenishmentWorkStatus::Cancelled => "cancelled",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn replenishment_policy_sort_wire(sort: ReplenishmentPolicySort) -> &'static str {
    match sort {
        ReplenishmentPolicySort::InventoryOwner => "inventory_owner",
        ReplenishmentPolicySort::Facility => "facility",
        ReplenishmentPolicySort::Item => "item",
        ReplenishmentPolicySort::PickFace => "pick_face",
        ReplenishmentPolicySort::Projected => "projected",
        ReplenishmentPolicySort::Demand => "demand",
        ReplenishmentPolicySort::Reserve => "reserve",
        ReplenishmentPolicySort::TargetGap => "target_gap",
        ReplenishmentPolicySort::Outcome => "outcome",
        ReplenishmentPolicySort::ActiveWork => "active_work",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn replenishment_policy_sort_direction_wire(
    direction: ReplenishmentPolicySortDirection,
) -> &'static str {
    match direction {
        ReplenishmentPolicySortDirection::Ascending => "ascending",
        ReplenishmentPolicySortDirection::Descending => "descending",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn replenishment_work_sort_wire(sort: ReplenishmentWorkSort) -> &'static str {
    match sort {
        ReplenishmentWorkSort::Created => "created",
        ReplenishmentWorkSort::Priority => "priority",
        ReplenishmentWorkSort::InventoryOwner => "inventory_owner",
        ReplenishmentWorkSort::Facility => "facility",
        ReplenishmentWorkSort::Item => "item",
        ReplenishmentWorkSort::Source => "source",
        ReplenishmentWorkSort::Destination => "destination",
        ReplenishmentWorkSort::Quantity => "quantity",
        ReplenishmentWorkSort::Status => "status",
        ReplenishmentWorkSort::Lease => "lease",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn replenishment_work_sort_direction_wire(
    direction: ReplenishmentWorkSortDirection,
) -> &'static str {
    match direction {
        ReplenishmentWorkSortDirection::Ascending => "ascending",
        ReplenishmentWorkSortDirection::Descending => "descending",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replenishment_policy_cursor_is_encoded_and_filters_are_stable() {
        let cursor = OpaqueCursor::new("rp2.a.2.a.a.g.d.0000000000000001".to_owned()).unwrap();
        assert_eq!(
            replenishment_policy_page_path(
                ReplenishmentPolicyFilters {
                    facility_id: Some(3),
                    inventory_owner_id: Some(4),
                    item_id: Some(5),
                    pick_face_location_id: Some(6),
                    sort: ReplenishmentPolicySort::Outcome,
                    direction: ReplenishmentPolicySortDirection::Ascending,
                },
                Some(&cursor),
            ),
            "/api/v1/replenishment-policies?limit=100&sort=outcome&direction=ascending&facility_id=3&inventory_owner_id=4&item_id=5&pick_face_location_id=6&cursor=rp2.a.2.a.a.g.d.0000000000000001"
        );
    }

    #[test]
    fn replenishment_queue_omits_status_for_the_open_work_default() {
        assert_eq!(
            replenishment_queue_page_path(ReplenishmentQueueFilters::default(), None,),
            "/api/v1/replenishment-queue?limit=100&sort=priority&direction=descending"
        );
        let claimed = replenishment_queue_page_path(
            ReplenishmentQueueFilters {
                status: Some(ReplenishmentWorkStatus::Claimed),
                sort: ReplenishmentWorkSort::Lease,
                direction: ReplenishmentWorkSortDirection::Ascending,
                ..ReplenishmentQueueFilters::default()
            },
            None,
        );
        assert!(claimed.contains("sort=lease&direction=ascending"));
        assert!(claimed.ends_with("&status=claimed"));
    }

    #[test]
    fn replenishment_cancellation_targets_one_typed_work_resource() {
        assert_eq!(
            replenishment_cancellation_path(42),
            "/api/v1/replenishment-tasks/42/cancellations"
        );
    }

    #[test]
    fn pick_shortage_path_uses_business_order_key_and_server_sort() {
        let cursor = OpaqueCursor::new("ps3.cursor".to_owned()).unwrap();
        let path = pick_shortage_page_path(
            &PickShortageFilters {
                facility_id: Some(3),
                inventory_owner_id: Some(4),
                order_id: None,
                order_key: Some("ORDER 5/BLUE".to_owned()),
                status: Some(PickShortageStatus::RecoveryInProgress),
                sort: PickShortageQueueSort::RemainingQuantity,
                direction: PickShortageQueueSortDirection::Ascending,
            },
            Some(&cursor),
        );
        assert_eq!(
            path,
            "/api/v1/pick-shortages?limit=100&sort=remaining_quantity&direction=ascending&facility_id=3&inventory_owner_id=4&order_key=ORDER%205%2FBLUE&status=recovery_in_progress&cursor=ps3.cursor"
        );
    }

    #[test]
    fn pick_history_cursor_and_reversal_paths_are_stable() {
        let cursor = OpaqueCursor::new("pc1.encoded".to_owned()).unwrap();
        assert_eq!(
            pick_confirmation_history_path(17, Some(&cursor)),
            "/api/v1/orders/17/pick-confirmations?limit=50&cursor=pc1.encoded"
        );
        assert_eq!(
            pick_reversal_path(23),
            "/api/v1/pick-confirmations/23/reversals"
        );
    }
}
