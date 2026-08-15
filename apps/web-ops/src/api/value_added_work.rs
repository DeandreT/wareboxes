use wareboxes_api_contract::v1::{
    CreateValueAddedWorkRequest, OpaqueCursor, ValueAddedWorkLifecycleRequest,
    ValueAddedWorkPageResponse, ValueAddedWorkResponse, ValueAddedWorkStatus,
};

use super::ApiError;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct ValueAddedWorkFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub status: Option<ValueAddedWorkStatus>,
}

#[cfg(target_arch = "wasm32")]
pub async fn value_added_work(
    filters: ValueAddedWorkFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<ValueAddedWorkPageResponse, ApiError> {
    super::browser::get(&page_path(filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn value_added_work(
    _filters: ValueAddedWorkFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<ValueAddedWorkPageResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn value_added_work_detail(work_id: i64) -> Result<ValueAddedWorkResponse, ApiError> {
    super::browser::get(&format!("/api/v1/value-added-work/{work_id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn value_added_work_detail(_work_id: i64) -> Result<ValueAddedWorkResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_value_added_work(
    request: &CreateValueAddedWorkRequest,
    idempotency_key: &str,
) -> Result<ValueAddedWorkResponse, ApiError> {
    super::browser::post("/api/v1/value-added-work", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_value_added_work(
    _request: &CreateValueAddedWorkRequest,
    _idempotency_key: &str,
) -> Result<ValueAddedWorkResponse, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! lifecycle_command {
    ($name:ident, $suffix:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            work_id: i64,
            request: &ValueAddedWorkLifecycleRequest,
            idempotency_key: &str,
        ) -> Result<ValueAddedWorkResponse, ApiError> {
            super::browser::post(
                &format!("/api/v1/value-added-work/{work_id}/{}", $suffix),
                request,
                idempotency_key,
            )
            .await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _work_id: i64,
            _request: &ValueAddedWorkLifecycleRequest,
            _idempotency_key: &str,
        ) -> Result<ValueAddedWorkResponse, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

lifecycle_command!(release_value_added_work, "releases");
lifecycle_command!(complete_value_added_work, "completions");
lifecycle_command!(cancel_value_added_work, "cancellations");

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(filters: ValueAddedWorkFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = "/api/v1/value-added-work?limit=100".to_owned();
    if let Some(owner_id) = filters.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    if let Some(facility_id) = filters.facility_id {
        path.push_str(&format!("&facility_id={facility_id}"));
    }
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(status_wire(status));
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
const fn status_wire(status: ValueAddedWorkStatus) -> &'static str {
    match status {
        ValueAddedWorkStatus::Draft => "draft",
        ValueAddedWorkStatus::Released => "released",
        ValueAddedWorkStatus::Completed => "completed",
        ValueAddedWorkStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_path_binds_scope_status_and_cursor() {
        let cursor = OpaqueCursor::new("vas1.scope.cursor+/=").unwrap();
        assert_eq!(
            page_path(
                ValueAddedWorkFilters {
                    facility_id: Some(7),
                    inventory_owner_id: Some(9),
                    status: Some(ValueAddedWorkStatus::Released),
                },
                Some(&cursor),
            ),
            "/api/v1/value-added-work?limit=100&inventory_owner_id=9&facility_id=7&status=released&cursor=vas1.scope.cursor%2B%2F%3D"
        );
    }
}
