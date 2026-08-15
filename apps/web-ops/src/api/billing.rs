use wareboxes_api_contract::v1::{
    BillableEventResponse, BillingContractResponse, BillingFinancialExportResponse,
    BillingLifecycleRequest, BillingRateResponse, BillingStorageSnapshotResponse,
    BillingWorkspaceResponse, CaptureBillableEventRequest, CaptureBillingStorageSnapshotRequest,
    ConfigureBillingRateRequest, CreateBillingContractRequest, ExportBillingRunRequest,
    GenerateBillingRunRequest, ReviewBillingRunRequest,
};

use super::ApiError;

#[derive(Clone, Copy, Default)]
pub struct BillingFilters {
    pub inventory_owner_id: Option<i64>,
    pub contract_id: Option<i64>,
}

#[cfg(target_arch = "wasm32")]
pub async fn billing_workspace(
    filters: BillingFilters,
) -> Result<BillingWorkspaceResponse, ApiError> {
    super::browser::get(&workspace_path(filters)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn billing_workspace(
    _filters: BillingFilters,
) -> Result<BillingWorkspaceResponse, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! post_command {
    ($name:ident, $request:ty, $response:ty, $path:expr) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            target_id: i64,
            request: &$request,
            idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            super::browser::post(&$path(target_id), request, idempotency_key).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _target_id: i64,
            _request: &$request,
            _idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

#[cfg(target_arch = "wasm32")]
pub async fn create_billing_contract(
    request: &CreateBillingContractRequest,
    idempotency_key: &str,
) -> Result<BillingContractResponse, ApiError> {
    super::browser::post("/api/v1/billing/contracts", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_billing_contract(
    _request: &CreateBillingContractRequest,
    _idempotency_key: &str,
) -> Result<BillingContractResponse, ApiError> {
    Err(ApiError::unavailable())
}

post_command!(
    activate_billing_contract,
    BillingLifecycleRequest,
    BillingContractResponse,
    |id| format!("/api/v1/billing/contracts/{id}/activations")
);
post_command!(
    close_billing_contract,
    BillingLifecycleRequest,
    BillingContractResponse,
    |id| format!("/api/v1/billing/contracts/{id}/closures")
);
post_command!(
    configure_billing_rate,
    ConfigureBillingRateRequest,
    BillingRateResponse,
    |id| format!("/api/v1/billing/contracts/{id}/rates")
);
post_command!(
    capture_billable_event,
    CaptureBillableEventRequest,
    BillableEventResponse,
    |id| format!("/api/v1/billing/contracts/{id}/billable-events")
);
post_command!(
    capture_billing_storage_snapshot,
    CaptureBillingStorageSnapshotRequest,
    BillingStorageSnapshotResponse,
    |id| format!("/api/v1/billing/contracts/{id}/storage-snapshots")
);
post_command!(
    generate_billing_run,
    GenerateBillingRunRequest,
    wareboxes_api_contract::v1::BillingRunResponse,
    |id| format!("/api/v1/billing/contracts/{id}/reconciliation-runs")
);
post_command!(
    review_billing_run,
    ReviewBillingRunRequest,
    wareboxes_api_contract::v1::BillingRunResponse,
    |id| format!("/api/v1/billing/reconciliation-runs/{id}/reviews")
);
post_command!(
    export_billing_run,
    ExportBillingRunRequest,
    BillingFinancialExportResponse,
    |id| format!("/api/v1/billing/reconciliation-runs/{id}/exports")
);

#[cfg(any(target_arch = "wasm32", test))]
fn workspace_path(filters: BillingFilters) -> String {
    let mut path = "/api/v1/billing/workspace?limit=100".to_owned();
    if let Some(owner_id) = filters.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    if let Some(contract_id) = filters.contract_id {
        path.push_str(&format!("&contract_id={contract_id}"));
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_path_binds_owner_and_contract() {
        assert_eq!(
            workspace_path(BillingFilters {
                inventory_owner_id: Some(7),
                contract_id: Some(9),
            }),
            "/api/v1/billing/workspace?limit=100&inventory_owner_id=7&contract_id=9"
        );
    }
}
