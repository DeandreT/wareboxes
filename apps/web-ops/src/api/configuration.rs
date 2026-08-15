use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationPage, ConfigurationResponse,
    ConfigurationSimulationResponse, ConfigurationStatus, CreateConfigurationRequest,
    DecisionRuleKind, OpaqueCursor, RollbackConfigurationRequest, SimulateConfigurationRequest,
};

use super::ApiError;

#[derive(Clone, Copy, Default)]
pub struct ConfigurationFilters {
    pub kind: Option<DecisionRuleKind>,
    pub status: Option<ConfigurationStatus>,
    pub inventory_owner_id: Option<i64>,
    pub facility_id: Option<i64>,
}

#[cfg(target_arch = "wasm32")]
pub async fn configurations(
    filters: ConfigurationFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<ConfigurationPage, ApiError> {
    super::browser::get(&page_path(filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configurations(
    _filters: ConfigurationFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<ConfigurationPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_configuration(
    request: &CreateConfigurationRequest,
    idempotency_key: &str,
) -> Result<ConfigurationResponse, ApiError> {
    super::browser::post("/api/v1/configurations", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_configuration(
    _request: &CreateConfigurationRequest,
    _idempotency_key: &str,
) -> Result<ConfigurationResponse, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! lifecycle_command {
    ($name:ident, $segment:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            configuration_id: i64,
            request: &ConfigurationLifecycleRequest,
            idempotency_key: &str,
        ) -> Result<ConfigurationResponse, ApiError> {
            super::browser::post(
                &format!("/api/v1/configurations/{configuration_id}/{}", $segment),
                request,
                idempotency_key,
            )
            .await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _configuration_id: i64,
            _request: &ConfigurationLifecycleRequest,
            _idempotency_key: &str,
        ) -> Result<ConfigurationResponse, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

lifecycle_command!(submit_configuration, "submissions");
lifecycle_command!(approve_configuration, "approvals");
lifecycle_command!(activate_configuration, "activations");
lifecycle_command!(retire_configuration, "retirements");

#[cfg(target_arch = "wasm32")]
pub async fn rollback_configuration(
    configuration_id: i64,
    request: &RollbackConfigurationRequest,
    idempotency_key: &str,
) -> Result<ConfigurationResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/configurations/{configuration_id}/rollbacks"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn rollback_configuration(
    _configuration_id: i64,
    _request: &RollbackConfigurationRequest,
    _idempotency_key: &str,
) -> Result<ConfigurationResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn simulate_configuration(
    request: &SimulateConfigurationRequest,
) -> Result<ConfigurationSimulationResponse, ApiError> {
    super::internal_post("/api/v1/configuration-simulations", request).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn simulate_configuration(
    _request: &SimulateConfigurationRequest,
) -> Result<ConfigurationSimulationResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(filters: ConfigurationFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = "/api/v1/configurations?limit=100".to_owned();
    if let Some(kind) = filters.kind {
        path.push_str("&kind=");
        path.push_str(kind_wire(kind));
    }
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(status_wire(status));
    }
    if let Some(owner_id) = filters.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    if let Some(facility_id) = filters.facility_id {
        path.push_str(&format!("&facility_id={facility_id}"));
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) const fn kind_wire(value: DecisionRuleKind) -> &'static str {
    match value {
        DecisionRuleKind::Receipt => "receipt",
        DecisionRuleKind::Putaway => "putaway",
        DecisionRuleKind::Allocation => "allocation",
        DecisionRuleKind::Replenishment => "replenishment",
        DecisionRuleKind::Wave => "wave",
        DecisionRuleKind::Pick => "pick",
        DecisionRuleKind::Pack => "pack",
        DecisionRuleKind::Count => "count",
        DecisionRuleKind::Document => "document",
        DecisionRuleKind::Billing => "billing",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) const fn status_wire(value: ConfigurationStatus) -> &'static str {
    match value {
        ConfigurationStatus::Draft => "draft",
        ConfigurationStatus::PendingApproval => "pending_approval",
        ConfigurationStatus::Approved => "approved",
        ConfigurationStatus::Active => "active",
        ConfigurationStatus::Retired => "retired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_path_binds_every_filter_and_encodes_cursor() {
        let cursor = OpaqueCursor::new("cfg1.filter/next".to_owned()).unwrap();
        assert_eq!(
            page_path(
                ConfigurationFilters {
                    kind: Some(DecisionRuleKind::Billing),
                    status: Some(ConfigurationStatus::Active),
                    inventory_owner_id: Some(2),
                    facility_id: Some(3),
                },
                Some(&cursor),
            ),
            "/api/v1/configurations?limit=100&kind=billing&status=active&inventory_owner_id=2&facility_id=3&cursor=cfg1.filter%2Fnext"
        );
    }
}
