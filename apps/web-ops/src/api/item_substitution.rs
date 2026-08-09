use wareboxes_api_contract::v1::{
    ConfigureItemSubstitutionPolicyRequest, ItemSubstitutionPolicyResponse,
    RetireItemSubstitutionPolicyRequest, SubstitutePickShortageRequest,
    SubstitutePickShortageResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn item_substitution_policies(
    inventory_owner_id: i64,
    facility_id: i64,
    source_item_id: i64,
    active_only: bool,
) -> Result<Vec<ItemSubstitutionPolicyResponse>, ApiError> {
    super::browser::get(&policy_list_path(
        inventory_owner_id,
        facility_id,
        source_item_id,
        active_only,
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn item_substitution_policies(
    _inventory_owner_id: i64,
    _facility_id: i64,
    _source_item_id: i64,
    _active_only: bool,
) -> Result<Vec<ItemSubstitutionPolicyResponse>, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn configure_item_substitution_policy(
    request: &ConfigureItemSubstitutionPolicyRequest,
    idempotency_key: &str,
) -> Result<ItemSubstitutionPolicyResponse, ApiError> {
    super::browser::post(
        "/api/v1/item-substitution-policies",
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_item_substitution_policy(
    _request: &ConfigureItemSubstitutionPolicyRequest,
    _idempotency_key: &str,
) -> Result<ItemSubstitutionPolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn retire_item_substitution_policy(
    policy_id: i64,
    request: &RetireItemSubstitutionPolicyRequest,
    idempotency_key: &str,
) -> Result<ItemSubstitutionPolicyResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/item-substitution-policies/{policy_id}/retirements"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn retire_item_substitution_policy(
    _policy_id: i64,
    _request: &RetireItemSubstitutionPolicyRequest,
    _idempotency_key: &str,
) -> Result<ItemSubstitutionPolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn substitute_pick_shortage(
    shortage_id: i64,
    request: &SubstitutePickShortageRequest,
    idempotency_key: &str,
) -> Result<SubstitutePickShortageResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/pick-shortages/{shortage_id}/substitutions"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn substitute_pick_shortage(
    _shortage_id: i64,
    _request: &SubstitutePickShortageRequest,
    _idempotency_key: &str,
) -> Result<SubstitutePickShortageResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn policy_list_path(
    inventory_owner_id: i64,
    facility_id: i64,
    source_item_id: i64,
    active_only: bool,
) -> String {
    format!(
        "/api/v1/item-substitution-policies?inventory_owner_id={inventory_owner_id}&facility_id={facility_id}&source_item_id={source_item_id}&active_only={active_only}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_list_path_binds_scope_source_and_lifecycle() {
        assert_eq!(
            policy_list_path(2, 3, 4, true),
            "/api/v1/item-substitution-policies?inventory_owner_id=2&facility_id=3&source_item_id=4&active_only=true"
        );
    }
}
