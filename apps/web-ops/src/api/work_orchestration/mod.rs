use wareboxes_api_contract::v1::{
    ConfigureWorkOrchestrationPolicyRequest, GenerateWorkOrchestrationPlanRequest,
    OrchestrationSignalWorkspaceRequest, OrchestrationSignalWorkspaceResponse,
    RecordResourceCapacitySignalRequest, RecordZoneCongestionSignalRequest,
    ResourceCapacitySignalResponse, WorkOrchestrationPlanPage, WorkOrchestrationPlanPageRequest,
    WorkOrchestrationPlanResponse, WorkOrchestrationPolicyPage, WorkOrchestrationPolicyPageRequest,
    WorkOrchestrationPolicyResponse, WorkOrchestrationWorkerPage,
    WorkOrchestrationWorkerPageRequest, ZoneCongestionSignalResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn work_orchestration_policies(
    request: &WorkOrchestrationPolicyPageRequest,
) -> Result<WorkOrchestrationPolicyPage, ApiError> {
    super::browser::get(&policy_page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn work_orchestration_policies(
    _request: &WorkOrchestrationPolicyPageRequest,
) -> Result<WorkOrchestrationPolicyPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn configure_work_orchestration_policy(
    request: &ConfigureWorkOrchestrationPolicyRequest,
    idempotency_key: &str,
) -> Result<WorkOrchestrationPolicyResponse, ApiError> {
    super::browser::post(
        "/api/v1/work-orchestration/policies",
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_work_orchestration_policy(
    _request: &ConfigureWorkOrchestrationPolicyRequest,
    _idempotency_key: &str,
) -> Result<WorkOrchestrationPolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn work_orchestration_signals(
    request: &OrchestrationSignalWorkspaceRequest,
) -> Result<OrchestrationSignalWorkspaceResponse, ApiError> {
    super::browser::get(&signal_workspace_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn work_orchestration_signals(
    _request: &OrchestrationSignalWorkspaceRequest,
) -> Result<OrchestrationSignalWorkspaceResponse, ApiError> {
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
    record_zone_congestion_signal,
    RecordZoneCongestionSignalRequest,
    ZoneCongestionSignalResponse,
    "/api/v1/work-orchestration/signals/congestion"
);
command!(
    record_resource_capacity_signal,
    RecordResourceCapacitySignalRequest,
    ResourceCapacitySignalResponse,
    "/api/v1/work-orchestration/signals/resources"
);
command!(
    generate_work_orchestration_plan,
    GenerateWorkOrchestrationPlanRequest,
    WorkOrchestrationPlanResponse,
    "/api/v1/work-orchestration/plans"
);

#[cfg(target_arch = "wasm32")]
pub async fn work_orchestration_plans(
    request: &WorkOrchestrationPlanPageRequest,
) -> Result<WorkOrchestrationPlanPage, ApiError> {
    super::browser::get(&plan_page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn work_orchestration_plans(
    _request: &WorkOrchestrationPlanPageRequest,
) -> Result<WorkOrchestrationPlanPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn work_orchestration_plan(
    plan_id: i64,
) -> Result<WorkOrchestrationPlanResponse, ApiError> {
    super::browser::get(&format!("/api/v1/work-orchestration/plans/{plan_id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn work_orchestration_plan(
    _plan_id: i64,
) -> Result<WorkOrchestrationPlanResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn work_orchestration_workers(
    request: &WorkOrchestrationWorkerPageRequest,
) -> Result<WorkOrchestrationWorkerPage, ApiError> {
    super::browser::get(&worker_page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn work_orchestration_workers(
    _request: &WorkOrchestrationWorkerPageRequest,
) -> Result<WorkOrchestrationWorkerPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn policy_page_path(request: &WorkOrchestrationPolicyPageRequest) -> String {
    let mut path = format!(
        "/api/v1/work-orchestration/policies?limit={}&include_facility_defaults={}&include_history={}",
        request.limit.get(), request.include_facility_defaults, request.include_history
    );
    append_scope(&mut path, request.facility_id, request.inventory_owner_id);
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn signal_workspace_path(request: &OrchestrationSignalWorkspaceRequest) -> String {
    let mut path = format!(
        "/api/v1/work-orchestration/signals?facility_id={}&include_history={}&limit={}",
        request.facility_id,
        request.include_history,
        request.limit.get()
    );
    if let Some(cursor) = request.zone_cursor.as_ref() {
        path.push_str("&zone_cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    if let Some(cursor) = request.resource_cursor.as_ref() {
        path.push_str("&resource_cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn plan_page_path(request: &WorkOrchestrationPlanPageRequest) -> String {
    let mut path = format!(
        "/api/v1/work-orchestration/plans?limit={}",
        request.limit.get()
    );
    append_scope(&mut path, request.facility_id, request.inventory_owner_id);
    if let Some(mode) = request.plan_mode {
        path.push_str("&plan_mode=");
        path.push_str(match mode {
            wareboxes_api_contract::v1::OrchestrationPlanMode::Optimized => "optimized",
            wareboxes_api_contract::v1::OrchestrationPlanMode::ManualFifo => "manual_fifo",
        });
    }
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn worker_page_path(request: &WorkOrchestrationWorkerPageRequest) -> String {
    let mut path = format!(
        "/api/v1/work-orchestration/workers?facility_id={}&limit={}",
        request.facility_id,
        request.limit.get()
    );
    if let Some(owner_id) = request.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_scope(path: &mut String, facility_id: Option<i64>, inventory_owner_id: Option<i64>) {
    if let Some(value) = facility_id {
        path.push_str(&format!("&facility_id={value}"));
    }
    if let Some(value) = inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={value}"));
    }
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
    use wareboxes_api_contract::v1::{OpaqueCursor, OrchestrationPlanMode, PageLimit};

    #[test]
    fn policy_path_preserves_scope_history_defaults_and_cursor() {
        let request = WorkOrchestrationPolicyPageRequest {
            facility_id: Some(8),
            inventory_owner_id: Some(12),
            include_facility_defaults: true,
            include_history: true,
            cursor: Some(OpaqueCursor::new("wop1.scope/a+b").unwrap()),
            limit: PageLimit::new(75).unwrap(),
        };
        assert_eq!(
            policy_page_path(&request),
            "/api/v1/work-orchestration/policies?limit=75&include_facility_defaults=true&include_history=true&facility_id=8&inventory_owner_id=12&cursor=wop1.scope%2Fa%2Bb"
        );
    }

    #[test]
    fn plan_path_preserves_fallback_filter_and_cursor() {
        let request = WorkOrchestrationPlanPageRequest {
            facility_id: Some(8),
            inventory_owner_id: None,
            plan_mode: Some(OrchestrationPlanMode::ManualFifo),
            cursor: Some(OpaqueCursor::new("wopl1.manual/a+b").unwrap()),
            limit: PageLimit::new(25).unwrap(),
        };
        assert_eq!(
            plan_page_path(&request),
            "/api/v1/work-orchestration/plans?limit=25&facility_id=8&plan_mode=manual_fifo&cursor=wopl1.manual%2Fa%2Bb"
        );
    }

    #[test]
    fn signal_path_has_stable_facility_and_history_identity() {
        assert_eq!(
            signal_workspace_path(&OrchestrationSignalWorkspaceRequest {
                facility_id: 8,
                include_history: true,
                zone_cursor: Some(OpaqueCursor::new("woz1.zone/a+b").unwrap()),
                resource_cursor: Some(OpaqueCursor::new("wor1.resource/a+b").unwrap()),
                limit: PageLimit::new(40).unwrap(),
            }),
            "/api/v1/work-orchestration/signals?facility_id=8&include_history=true&limit=40&zone_cursor=woz1.zone%2Fa%2Bb&resource_cursor=wor1.resource%2Fa%2Bb"
        );
    }

    #[test]
    fn worker_path_binds_scope_and_encodes_cursor() {
        let request = WorkOrchestrationWorkerPageRequest {
            facility_id: 8,
            inventory_owner_id: Some(12),
            cursor: Some(OpaqueCursor::new("wow1.worker/a+b").unwrap()),
            limit: PageLimit::new(60).unwrap(),
        };
        assert_eq!(
            worker_page_path(&request),
            "/api/v1/work-orchestration/workers?facility_id=8&limit=60&inventory_owner_id=12&cursor=wow1.worker%2Fa%2Bb"
        );
    }
}
