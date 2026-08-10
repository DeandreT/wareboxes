use wareboxes_api_contract::v1::{
    CreateCycleCountTaskRequest, CreateCycleCountTaskResponse, CycleCountCandidatePage,
    CycleCountCandidateSort, CycleCountSortDirection, CycleCountWorkPage, CycleCountWorkSort,
    CycleCountWorkStatus, InventoryBalanceStatus, OpaqueCursor,
};

use super::ApiError;

#[allow(clippy::too_many_arguments)]
#[cfg(target_arch = "wasm32")]
pub async fn cycle_count_candidates(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    inventory_status: Option<InventoryBalanceStatus>,
    sort: CycleCountCandidateSort,
    direction: CycleCountSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<CycleCountCandidatePage, ApiError> {
    super::browser::get(&candidate_page_path(
        facility_id,
        inventory_owner_id,
        inventory_status,
        sort,
        direction,
        cursor,
    ))
    .await
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
pub async fn cycle_count_candidates(
    _facility_id: Option<i64>,
    _inventory_owner_id: Option<i64>,
    _inventory_status: Option<InventoryBalanceStatus>,
    _sort: CycleCountCandidateSort,
    _direction: CycleCountSortDirection,
    _cursor: Option<&OpaqueCursor>,
) -> Result<CycleCountCandidatePage, ApiError> {
    Err(ApiError::unavailable())
}

#[allow(clippy::too_many_arguments)]
#[cfg(target_arch = "wasm32")]
pub async fn cycle_count_work(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    status: Option<CycleCountWorkStatus>,
    sort: CycleCountWorkSort,
    direction: CycleCountSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<CycleCountWorkPage, ApiError> {
    super::browser::get(&work_page_path(
        facility_id,
        inventory_owner_id,
        status,
        sort,
        direction,
        cursor,
    ))
    .await
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
pub async fn cycle_count_work(
    _facility_id: Option<i64>,
    _inventory_owner_id: Option<i64>,
    _status: Option<CycleCountWorkStatus>,
    _sort: CycleCountWorkSort,
    _direction: CycleCountSortDirection,
    _cursor: Option<&OpaqueCursor>,
) -> Result<CycleCountWorkPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_cycle_count_task(
    request: &CreateCycleCountTaskRequest,
    idempotency_key: &str,
) -> Result<CreateCycleCountTaskResponse, ApiError> {
    super::browser::post("/api/v1/cycle-count-tasks", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_cycle_count_task(
    _request: &CreateCycleCountTaskRequest,
    _idempotency_key: &str,
) -> Result<CreateCycleCountTaskResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn candidate_page_path(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    inventory_status: Option<InventoryBalanceStatus>,
    sort: CycleCountCandidateSort,
    direction: CycleCountSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/cycle-count-candidates?limit=100&sort={}&direction={}",
        candidate_sort_wire(sort),
        direction_wire(direction),
    );
    append_common(&mut path, facility_id, inventory_owner_id, cursor);
    if let Some(status) = inventory_status {
        path.push_str("&inventory_status=");
        path.push_str(inventory_status_wire(status));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn work_page_path(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    status: Option<CycleCountWorkStatus>,
    sort: CycleCountWorkSort,
    direction: CycleCountSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/cycle-count-tasks?limit=100&sort={}&direction={}",
        work_sort_wire(sort),
        direction_wire(direction),
    );
    append_common(&mut path, facility_id, inventory_owner_id, cursor);
    if let Some(status) = status {
        path.push_str("&status=");
        path.push_str(work_status_wire(status));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_common(
    path: &mut String,
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    cursor: Option<&OpaqueCursor>,
) {
    if let Some(value) = facility_id {
        path.push_str(&format!("&facility_id={value}"));
    }
    if let Some(value) = inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={value}"));
    }
    if let Some(value) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(value.as_str()));
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn candidate_sort_wire(value: CycleCountCandidateSort) -> &'static str {
    match value {
        CycleCountCandidateSort::LastCounted => "last_counted",
        CycleCountCandidateSort::Client => "client",
        CycleCountCandidateSort::Facility => "facility",
        CycleCountCandidateSort::Location => "location",
        CycleCountCandidateSort::Item => "item",
        CycleCountCandidateSort::Quantity => "quantity",
        CycleCountCandidateSort::InventoryStatus => "inventory_status",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn work_sort_wire(value: CycleCountWorkSort) -> &'static str {
    match value {
        CycleCountWorkSort::Priority => "priority",
        CycleCountWorkSort::CreatedAt => "created_at",
        CycleCountWorkSort::Client => "client",
        CycleCountWorkSort::Facility => "facility",
        CycleCountWorkSort::Location => "location",
        CycleCountWorkSort::Item => "item",
        CycleCountWorkSort::Quantity => "quantity",
        CycleCountWorkSort::Variance => "variance",
        CycleCountWorkSort::Status => "status",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn direction_wire(value: CycleCountSortDirection) -> &'static str {
    match value {
        CycleCountSortDirection::Asc => "asc",
        CycleCountSortDirection::Desc => "desc",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn inventory_status_wire(value: InventoryBalanceStatus) -> &'static str {
    match value {
        InventoryBalanceStatus::Available => "available",
        InventoryBalanceStatus::Hold => "hold",
        InventoryBalanceStatus::Damaged => "damaged",
        InventoryBalanceStatus::Quarantine => "quarantine",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn work_status_wire(value: CycleCountWorkStatus) -> &'static str {
    match value {
        CycleCountWorkStatus::Pending => "pending",
        CycleCountWorkStatus::Claimed => "claimed",
        CycleCountWorkStatus::Completed => "completed",
        CycleCountWorkStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_path_keeps_sort_and_filters_server_side() {
        let cursor = OpaqueCursor::new("cc1.cursor").unwrap();
        let path = candidate_page_path(
            Some(7),
            Some(9),
            Some(InventoryBalanceStatus::Quarantine),
            CycleCountCandidateSort::Quantity,
            CycleCountSortDirection::Desc,
            Some(&cursor),
        );
        assert!(path.contains("sort=quantity&direction=desc"));
        assert!(path.contains("facility_id=7"));
        assert!(path.contains("inventory_owner_id=9"));
        assert!(path.contains("inventory_status=quarantine"));
        assert!(path.contains("cursor=cc1.cursor"));
    }

    #[test]
    fn work_path_encodes_explicit_history_status() {
        let path = work_page_path(
            None,
            None,
            Some(CycleCountWorkStatus::Completed),
            CycleCountWorkSort::Variance,
            CycleCountSortDirection::Asc,
            None,
        );
        assert!(path.contains("sort=variance&direction=asc"));
        assert!(path.contains("status=completed"));
    }
}
