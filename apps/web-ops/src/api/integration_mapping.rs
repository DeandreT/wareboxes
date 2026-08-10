use wareboxes_api_contract::v1::{
    ConfigureIntegrationOrderItemMappingRequest, ConfigureIntegrationOrderOwnerMappingRequest,
    IntegrationOrderItemMappingPage, IntegrationOrderItemMappingResponse,
    IntegrationOrderItemMappingStatus, IntegrationOrderOwnerMappingPage,
    IntegrationOrderOwnerMappingResponse, IntegrationOrderOwnerMappingStatus, OpaqueCursor,
    RetireIntegrationOrderItemMappingRequest, RetireIntegrationOrderOwnerMappingRequest,
};

use super::ApiError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntegrationMappingFilters {
    pub inventory_owner_id: Option<i64>,
    pub source_key: Option<String>,
    pub item_id: Option<i64>,
    pub status: Option<IntegrationOrderItemMappingStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntegrationOwnerMappingFilters {
    pub inventory_owner_id: Option<i64>,
    pub source_key: Option<String>,
    pub status: Option<IntegrationOrderOwnerMappingStatus>,
}

#[cfg(target_arch = "wasm32")]
pub async fn integration_order_item_mappings(
    filters: &IntegrationMappingFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<IntegrationOrderItemMappingPage, ApiError> {
    super::browser::get(&mapping_page_path(filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn integration_order_item_mappings(
    _filters: &IntegrationMappingFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<IntegrationOrderItemMappingPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn configure_integration_order_item_mapping(
    request: &ConfigureIntegrationOrderItemMappingRequest,
    idempotency_key: &str,
) -> Result<IntegrationOrderItemMappingResponse, ApiError> {
    super::browser::post(
        "/api/v1/integration-order-item-mappings",
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_integration_order_item_mapping(
    _request: &ConfigureIntegrationOrderItemMappingRequest,
    _idempotency_key: &str,
) -> Result<IntegrationOrderItemMappingResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn retire_integration_order_item_mapping(
    mapping_id: i64,
    request: &RetireIntegrationOrderItemMappingRequest,
    idempotency_key: &str,
) -> Result<IntegrationOrderItemMappingResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/integration-order-item-mappings/{mapping_id}/retirements"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(target_arch = "wasm32")]
pub async fn integration_order_owner_mappings(
    filters: &IntegrationOwnerMappingFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<IntegrationOrderOwnerMappingPage, ApiError> {
    super::browser::get(&owner_mapping_page_path(filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn integration_order_owner_mappings(
    _filters: &IntegrationOwnerMappingFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<IntegrationOrderOwnerMappingPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn configure_integration_order_owner_mapping(
    request: &ConfigureIntegrationOrderOwnerMappingRequest,
    idempotency_key: &str,
) -> Result<IntegrationOrderOwnerMappingResponse, ApiError> {
    super::browser::post(
        "/api/v1/integration-order-owner-mappings",
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_integration_order_owner_mapping(
    _request: &ConfigureIntegrationOrderOwnerMappingRequest,
    _idempotency_key: &str,
) -> Result<IntegrationOrderOwnerMappingResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn retire_integration_order_owner_mapping(
    mapping_id: i64,
    request: &RetireIntegrationOrderOwnerMappingRequest,
    idempotency_key: &str,
) -> Result<IntegrationOrderOwnerMappingResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/integration-order-owner-mappings/{mapping_id}/retirements"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn retire_integration_order_owner_mapping(
    _mapping_id: i64,
    _request: &RetireIntegrationOrderOwnerMappingRequest,
    _idempotency_key: &str,
) -> Result<IntegrationOrderOwnerMappingResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn retire_integration_order_item_mapping(
    _mapping_id: i64,
    _request: &RetireIntegrationOrderItemMappingRequest,
    _idempotency_key: &str,
) -> Result<IntegrationOrderItemMappingResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn mapping_page_path(filters: &IntegrationMappingFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut params = vec!["limit=100".to_owned()];
    if let Some(owner_id) = filters.inventory_owner_id {
        params.push(format!("inventory_owner_id={owner_id}"));
    }
    if let Some(source_key) = filters
        .source_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        params.push(format!("source_key={}", urlencoding::encode(source_key)));
    }
    if let Some(item_id) = filters.item_id {
        params.push(format!("item_id={item_id}"));
    }
    if let Some(status) = filters.status {
        params.push(format!(
            "status={}",
            match status {
                IntegrationOrderItemMappingStatus::Active => "active",
                IntegrationOrderItemMappingStatus::Retired => "retired",
            }
        ));
    }
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", urlencoding::encode(cursor.as_str())));
    }
    format!(
        "/api/v1/integration-order-item-mappings?{}",
        params.join("&")
    )
}

#[cfg(any(target_arch = "wasm32", test))]
fn owner_mapping_page_path(
    filters: &IntegrationOwnerMappingFilters,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut params = vec!["limit=100".to_owned()];
    if let Some(owner_id) = filters.inventory_owner_id {
        params.push(format!("inventory_owner_id={owner_id}"));
    }
    if let Some(source_key) = filters
        .source_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        params.push(format!("source_key={}", urlencoding::encode(source_key)));
    }
    if let Some(status) = filters.status {
        params.push(format!(
            "status={}",
            match status {
                IntegrationOrderOwnerMappingStatus::Active => "active",
                IntegrationOrderOwnerMappingStatus::Retired => "retired",
            }
        ));
    }
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", urlencoding::encode(cursor.as_str())));
    }
    format!(
        "/api/v1/integration-order-owner-mappings?{}",
        params.join("&")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_path_encodes_scope_and_cursor() {
        let filters = IntegrationMappingFilters {
            inventory_owner_id: Some(7),
            source_key: Some("retail api".into()),
            item_id: Some(11),
            status: Some(IntegrationOrderItemMappingStatus::Retired),
        };
        let cursor = OpaqueCursor::new("iom1.filter.0000000000000009").unwrap();
        assert_eq!(
            mapping_page_path(&filters, Some(&cursor)),
            "/api/v1/integration-order-item-mappings?limit=100&inventory_owner_id=7&source_key=retail%20api&item_id=11&status=retired&cursor=iom1.filter.0000000000000009"
        );
    }

    #[test]
    fn owner_page_path_encodes_scope_and_cursor() {
        let filters = IntegrationOwnerMappingFilters {
            inventory_owner_id: Some(7),
            source_key: Some("retail api".into()),
            status: Some(IntegrationOrderOwnerMappingStatus::Retired),
        };
        let cursor = OpaqueCursor::new("ioo1.filter.0000000000000009").unwrap();
        assert_eq!(
            owner_mapping_page_path(&filters, Some(&cursor)),
            "/api/v1/integration-order-owner-mappings?limit=100&inventory_owner_id=7&source_key=retail%20api&status=retired&cursor=ioo1.filter.0000000000000009"
        );
    }
}
