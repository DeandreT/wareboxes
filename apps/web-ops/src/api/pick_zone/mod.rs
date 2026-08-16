use wareboxes_api_contract::v1::PickZoneWorkspaceResponse;

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn workspace(
    facility_id: i64,
    inventory_owner_id: i64,
) -> Result<PickZoneWorkspaceResponse, ApiError> {
    super::browser::get(&workspace_path(facility_id, inventory_owner_id)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn workspace(
    _facility_id: i64,
    _inventory_owner_id: i64,
) -> Result<PickZoneWorkspaceResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn workspace_path(facility_id: i64, inventory_owner_id: i64) -> String {
    format!(
        "/api/v1/pick-zones/workspace?facility_id={facility_id}&inventory_owner_id={inventory_owner_id}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_path_keeps_exact_owner_and_facility_scope() {
        assert_eq!(
            workspace_path(4, 9),
            "/api/v1/pick-zones/workspace?facility_id=4&inventory_owner_id=9"
        );
    }
}
