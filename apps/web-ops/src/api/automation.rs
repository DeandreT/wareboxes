use wareboxes_api_contract::v1::{
    AutomationCommandResponse, AutomationDeviceResponse, AutomationWorkspaceResponse,
    ChangeAutomationControlRequest, EnqueueAutomationCommandRequest,
    RegisterAutomationDeviceRequest, ResolveAutomationCommandRequest,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn automation_workspace(
    facility_id: Option<i64>,
    include_history: bool,
) -> Result<AutomationWorkspaceResponse, ApiError> {
    super::browser::get(&workspace_path(facility_id, include_history)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn automation_workspace(
    _facility_id: Option<i64>,
    _include_history: bool,
) -> Result<AutomationWorkspaceResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn register_automation_device(
    request: &RegisterAutomationDeviceRequest,
    idempotency_key: &str,
) -> Result<AutomationDeviceResponse, ApiError> {
    super::browser::post("/api/v1/automation/devices", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn register_automation_device(
    _request: &RegisterAutomationDeviceRequest,
    _idempotency_key: &str,
) -> Result<AutomationDeviceResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn change_automation_control(
    device_id: i64,
    request: &ChangeAutomationControlRequest,
    idempotency_key: &str,
) -> Result<AutomationDeviceResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/automation/devices/{device_id}/control-changes"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn change_automation_control(
    _device_id: i64,
    _request: &ChangeAutomationControlRequest,
    _idempotency_key: &str,
) -> Result<AutomationDeviceResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn enqueue_automation_command(
    device_id: i64,
    request: &EnqueueAutomationCommandRequest,
    idempotency_key: &str,
) -> Result<AutomationCommandResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/automation/devices/{device_id}/commands"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(target_arch = "wasm32")]
pub async fn resolve_automation_command(
    command_id: i64,
    request: &ResolveAutomationCommandRequest,
    idempotency_key: &str,
) -> Result<AutomationCommandResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/automation/commands/{command_id}/resolutions"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn resolve_automation_command(
    _command_id: i64,
    _request: &ResolveAutomationCommandRequest,
    _idempotency_key: &str,
) -> Result<AutomationCommandResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn enqueue_automation_command(
    _device_id: i64,
    _request: &EnqueueAutomationCommandRequest,
    _idempotency_key: &str,
) -> Result<AutomationCommandResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn workspace_path(facility_id: Option<i64>, include_history: bool) -> String {
    let mut path = format!("/api/v1/automation/workspace?include_history={include_history}");
    if let Some(facility_id) = facility_id {
        path.push_str(&format!("&facility_id={facility_id}"));
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_path_binds_exact_facility_and_history() {
        assert_eq!(
            workspace_path(Some(17), true),
            "/api/v1/automation/workspace?include_history=true&facility_id=17"
        );
        assert_eq!(
            workspace_path(None, false),
            "/api/v1/automation/workspace?include_history=false"
        );
    }
}
