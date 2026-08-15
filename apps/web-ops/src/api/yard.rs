use wareboxes_api_contract::v1::{
    AssignYardVisitDoorRequest, ConfigureYardLocationRequest, CreateYardAppointmentRequest,
    GateInYardVisitRequest, MoveYardVisitRequest, RegisterYardAssetRequest,
    YardAppointmentResponse, YardAssetResponse, YardDockOperationRequest, YardLifecycleRequest,
    YardLocationResponse, YardVisitResponse, YardWorkspaceResponse,
};

use super::ApiError;

#[derive(Clone, Copy, Default)]
pub struct YardFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub include_completed: bool,
}

#[cfg(target_arch = "wasm32")]
pub async fn yard_workspace(filters: YardFilters) -> Result<YardWorkspaceResponse, ApiError> {
    super::browser::get(&workspace_path(filters)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn yard_workspace(_filters: YardFilters) -> Result<YardWorkspaceResponse, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! root_command {
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

macro_rules! target_command {
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

root_command!(
    configure_yard_location,
    ConfigureYardLocationRequest,
    YardLocationResponse,
    "/api/v1/yard/locations"
);
root_command!(
    register_yard_asset,
    RegisterYardAssetRequest,
    YardAssetResponse,
    "/api/v1/yard/assets"
);
root_command!(
    create_yard_appointment,
    CreateYardAppointmentRequest,
    YardAppointmentResponse,
    "/api/v1/yard/appointments"
);
root_command!(
    gate_in_yard_visit,
    GateInYardVisitRequest,
    YardVisitResponse,
    "/api/v1/yard/visits"
);
target_command!(
    cancel_yard_appointment,
    YardLifecycleRequest,
    YardAppointmentResponse,
    |id| format!("/api/v1/yard/appointments/{id}/cancellations")
);
target_command!(
    no_show_yard_appointment,
    YardLifecycleRequest,
    YardAppointmentResponse,
    |id| format!("/api/v1/yard/appointments/{id}/no-shows")
);
target_command!(
    spot_yard_visit,
    MoveYardVisitRequest,
    YardVisitResponse,
    |id| format!("/api/v1/yard/visits/{id}/spot-moves")
);
target_command!(
    assign_yard_visit_door,
    AssignYardVisitDoorRequest,
    YardVisitResponse,
    |id| format!("/api/v1/yard/visits/{id}/door-assignments")
);
target_command!(
    start_yard_operation,
    YardDockOperationRequest,
    YardVisitResponse,
    |id| format!("/api/v1/yard/visits/{id}/operation-starts")
);
target_command!(
    complete_yard_operation,
    YardDockOperationRequest,
    YardVisitResponse,
    |id| format!("/api/v1/yard/visits/{id}/operation-completions")
);
target_command!(
    reject_yard_visit,
    YardLifecycleRequest,
    YardVisitResponse,
    |id| format!("/api/v1/yard/visits/{id}/rejections")
);
target_command!(
    gate_out_yard_visit,
    YardLifecycleRequest,
    YardVisitResponse,
    |id| format!("/api/v1/yard/visits/{id}/gate-outs")
);

#[cfg(any(target_arch = "wasm32", test))]
fn workspace_path(filters: YardFilters) -> String {
    let mut path = format!(
        "/api/v1/yard/workspace?limit=100&include_completed={}",
        filters.include_completed
    );
    if let Some(facility_id) = filters.facility_id {
        path.push_str(&format!("&facility_id={facility_id}"));
    }
    if let Some(owner_id) = filters.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_path_binds_scope_and_history_mode() {
        assert_eq!(
            workspace_path(YardFilters {
                facility_id: Some(7),
                inventory_owner_id: Some(9),
                include_completed: true,
            }),
            "/api/v1/yard/workspace?limit=100&include_completed=true&facility_id=7&inventory_owner_id=9"
        );
    }
}
