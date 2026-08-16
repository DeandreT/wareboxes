use wareboxes_api_contract::v1::{
    ChangeLicensePlateParentRequest, ChangeLicensePlateParentResponse,
    LicensePlateHierarchyResponse,
};

use super::{internal_get, internal_post_idempotent, ApiError};

pub async fn license_plate_hierarchy(
    license_plate_id: i64,
) -> Result<LicensePlateHierarchyResponse, ApiError> {
    internal_get(&format!(
        "/api/v1/license-plates/{license_plate_id}/hierarchy"
    ))
    .await
}

pub async fn change_license_plate_parent(
    license_plate_id: i64,
    request: &ChangeLicensePlateParentRequest,
    idempotency_key: &str,
) -> Result<ChangeLicensePlateParentResponse, ApiError> {
    internal_post_idempotent(
        &format!("/api/v1/license-plates/{license_plate_id}/parent-changes"),
        request,
        idempotency_key,
    )
    .await
}
