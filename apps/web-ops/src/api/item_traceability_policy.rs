use wareboxes_api_contract::v1::{
    ConfigureItemTraceabilityPolicyRequest, ItemTraceabilityPolicyPage,
    ItemTraceabilityPolicyResponse, ItemTraceabilityPolicyStatus, OpaqueCursor,
    RetireItemTraceabilityPolicyRequest, TraceabilityRequirement,
};

use super::ApiError;

#[derive(Clone, Copy)]
pub struct ItemTraceabilityPolicyFilters {
    pub inventory_owner_id: Option<i64>,
    pub facility_id: Option<i64>,
    pub item_id: Option<i64>,
    pub lot: Option<TraceabilityRequirement>,
    pub serial: Option<TraceabilityRequirement>,
    pub expiration: Option<TraceabilityRequirement>,
    pub status: ItemTraceabilityPolicyStatus,
}

#[cfg(target_arch = "wasm32")]
pub async fn item_traceability_policies(
    filters: ItemTraceabilityPolicyFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<ItemTraceabilityPolicyPage, ApiError> {
    super::browser::get(&page_path(filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn item_traceability_policies(
    _filters: ItemTraceabilityPolicyFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<ItemTraceabilityPolicyPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn configure_item_traceability_policy(
    request: &ConfigureItemTraceabilityPolicyRequest,
    idempotency_key: &str,
) -> Result<ItemTraceabilityPolicyResponse, ApiError> {
    super::browser::post(
        "/api/v1/item-traceability-policies",
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_item_traceability_policy(
    _request: &ConfigureItemTraceabilityPolicyRequest,
    _idempotency_key: &str,
) -> Result<ItemTraceabilityPolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn retire_item_traceability_policy(
    item_traceability_policy_id: i64,
    request: &RetireItemTraceabilityPolicyRequest,
    idempotency_key: &str,
) -> Result<ItemTraceabilityPolicyResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/item-traceability-policies/{item_traceability_policy_id}/retirements"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn retire_item_traceability_policy(
    _item_traceability_policy_id: i64,
    _request: &RetireItemTraceabilityPolicyRequest,
    _idempotency_key: &str,
) -> Result<ItemTraceabilityPolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(filters: ItemTraceabilityPolicyFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = format!(
        "/api/v1/item-traceability-policies?limit=100&status={}",
        status_wire(filters.status)
    );
    for (name, value) in [
        ("inventory_owner_id", filters.inventory_owner_id),
        ("facility_id", filters.facility_id),
        ("item_id", filters.item_id),
    ] {
        if let Some(value) = value {
            path.push_str(&format!("&{name}={value}"));
        }
    }
    for (name, value) in [
        ("lot", filters.lot),
        ("serial", filters.serial),
        ("expiration", filters.expiration),
    ] {
        if let Some(value) = value {
            path.push_str(&format!("&{name}={}", requirement_wire(value)));
        }
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(cursor.as_str());
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
const fn status_wire(value: ItemTraceabilityPolicyStatus) -> &'static str {
    match value {
        ItemTraceabilityPolicyStatus::Active => "active",
        ItemTraceabilityPolicyStatus::Retired => "retired",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn requirement_wire(value: TraceabilityRequirement) -> &'static str {
    match value {
        TraceabilityRequirement::NotTracked => "not_tracked",
        TraceabilityRequirement::Required => "required",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_path_includes_identity_filters_and_cursor() {
        let cursor = OpaqueCursor::new("itp1.filter.0000000000000001").unwrap();
        assert_eq!(
            page_path(
                ItemTraceabilityPolicyFilters {
                    inventory_owner_id: Some(2),
                    facility_id: Some(3),
                    item_id: Some(4),
                    lot: Some(TraceabilityRequirement::Required),
                    serial: None,
                    expiration: Some(TraceabilityRequirement::Required),
                    status: ItemTraceabilityPolicyStatus::Retired,
                },
                Some(&cursor),
            ),
            "/api/v1/item-traceability-policies?limit=100&status=retired&inventory_owner_id=2&facility_id=3&item_id=4&lot=required&expiration=required&cursor=itp1.filter.0000000000000001"
        );
    }
}
