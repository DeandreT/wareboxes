use wareboxes_api_contract::v1::{
    CancelPickWaveRequest, OpaqueCursor, PickWavePage, PickWavePolicyResolutionsResponse,
    PickWaveResponse, PickWaveSort, PickWaveSortDirection, PickWaveStatus, PlanPickWaveRequest,
    ReleasePickWaveRequest, ResolvePickWavePoliciesRequest,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn pick_waves(
    facility_id: Option<i64>,
    status: Option<PickWaveStatus>,
    sort: PickWaveSort,
    direction: PickWaveSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<PickWavePage, ApiError> {
    super::browser::get(&pick_wave_page_path(
        facility_id,
        status,
        sort,
        direction,
        cursor,
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn pick_waves(
    _facility_id: Option<i64>,
    _status: Option<PickWaveStatus>,
    _sort: PickWaveSort,
    _direction: PickWaveSortDirection,
    _cursor: Option<&OpaqueCursor>,
) -> Result<PickWavePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn pick_wave(wave_id: i64) -> Result<PickWaveResponse, ApiError> {
    super::browser::get(&format!("/api/v1/pick-waves/{wave_id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn pick_wave(_wave_id: i64) -> Result<PickWaveResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn plan_pick_wave(
    request: &PlanPickWaveRequest,
    idempotency_key: &str,
) -> Result<PickWaveResponse, ApiError> {
    super::browser::post("/api/v1/pick-waves", request, idempotency_key).await
}

#[cfg(target_arch = "wasm32")]
pub async fn resolve_pick_wave_policies(
    request: &ResolvePickWavePoliciesRequest,
) -> Result<PickWavePolicyResolutionsResponse, ApiError> {
    super::browser::post(
        "/api/v1/pick-waves/policy-resolutions",
        request,
        &super::browser::new_idempotency_key(),
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn resolve_pick_wave_policies(
    _request: &ResolvePickWavePoliciesRequest,
) -> Result<PickWavePolicyResolutionsResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan_pick_wave(
    _request: &PlanPickWaveRequest,
    _idempotency_key: &str,
) -> Result<PickWaveResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn release_pick_wave(
    wave_id: i64,
    request: &ReleasePickWaveRequest,
    idempotency_key: &str,
) -> Result<PickWaveResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/pick-waves/{wave_id}/releases"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn release_pick_wave(
    _wave_id: i64,
    _request: &ReleasePickWaveRequest,
    _idempotency_key: &str,
) -> Result<PickWaveResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn cancel_pick_wave(
    wave_id: i64,
    request: &CancelPickWaveRequest,
    idempotency_key: &str,
) -> Result<PickWaveResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/pick-waves/{wave_id}/cancellations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel_pick_wave(
    _wave_id: i64,
    _request: &CancelPickWaveRequest,
    _idempotency_key: &str,
) -> Result<PickWaveResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn pick_wave_page_path(
    facility_id: Option<i64>,
    status: Option<PickWaveStatus>,
    sort: PickWaveSort,
    direction: PickWaveSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/pick-waves?limit=100&sort={}&direction={}",
        sort_wire(sort),
        direction_wire(direction)
    );
    if let Some(facility_id) = facility_id {
        path.push_str(&format!("&facility_id={facility_id}"));
    }
    if let Some(status) = status {
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
const fn sort_wire(sort: PickWaveSort) -> &'static str {
    match sort {
        PickWaveSort::Name => "name",
        PickWaveSort::Status => "status",
        PickWaveSort::Orders => "orders",
        PickWaveSort::Tasks => "tasks",
        PickWaveSort::Units => "units",
        PickWaveSort::PlannedAt => "planned_at",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn direction_wire(direction: PickWaveSortDirection) -> &'static str {
    match direction {
        PickWaveSortDirection::Asc => "asc",
        PickWaveSortDirection::Desc => "desc",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn status_wire(status: PickWaveStatus) -> &'static str {
    match status {
        PickWaveStatus::Planned => "planned",
        PickWaveStatus::Released => "released",
        PickWaveStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_path_preserves_global_sort_filters_and_cursor() {
        let cursor = OpaqueCursor::new("pw1.a.p.units.desc.0000000000000064").unwrap();
        assert_eq!(
            pick_wave_page_path(
                Some(4),
                Some(PickWaveStatus::Planned),
                PickWaveSort::Units,
                PickWaveSortDirection::Desc,
                Some(&cursor),
            ),
            "/api/v1/pick-waves?limit=100&sort=units&direction=desc&facility_id=4&status=planned&cursor=pw1.a.p.units.desc.0000000000000064"
        );
    }
}
