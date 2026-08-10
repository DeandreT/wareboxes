use wareboxes_api_contract::v1::{
    CreateLicensePlatePutawayTaskRequest, CreateLicensePlatePutawayTaskResponse,
    CreatePutawayTaskRequest, CreatePutawayTaskResponse, OpaqueCursor, PutawayCandidatePage,
    PutawayCandidateSort, PutawaySortDirection, PutawayWorkPage, PutawayWorkSort,
    PutawayWorkStatus, PutawayWorkflow,
};

use super::ApiError;

#[allow(clippy::too_many_arguments)]
#[cfg(target_arch = "wasm32")]
pub async fn putaway_candidates(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    workflow: Option<PutawayWorkflow>,
    sort: PutawayCandidateSort,
    direction: PutawaySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<PutawayCandidatePage, ApiError> {
    super::browser::get(&candidate_page_path(
        facility_id,
        inventory_owner_id,
        workflow,
        sort,
        direction,
        cursor,
    ))
    .await
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
pub async fn putaway_candidates(
    _facility_id: Option<i64>,
    _inventory_owner_id: Option<i64>,
    _workflow: Option<PutawayWorkflow>,
    _sort: PutawayCandidateSort,
    _direction: PutawaySortDirection,
    _cursor: Option<&OpaqueCursor>,
) -> Result<PutawayCandidatePage, ApiError> {
    Err(ApiError::unavailable())
}

#[allow(clippy::too_many_arguments)]
#[cfg(target_arch = "wasm32")]
pub async fn putaway_work(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    workflow: Option<PutawayWorkflow>,
    status: Option<PutawayWorkStatus>,
    sort: PutawayWorkSort,
    direction: PutawaySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<PutawayWorkPage, ApiError> {
    super::browser::get(&work_page_path(
        facility_id,
        inventory_owner_id,
        workflow,
        status,
        sort,
        direction,
        cursor,
    ))
    .await
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
pub async fn putaway_work(
    _facility_id: Option<i64>,
    _inventory_owner_id: Option<i64>,
    _workflow: Option<PutawayWorkflow>,
    _status: Option<PutawayWorkStatus>,
    _sort: PutawayWorkSort,
    _direction: PutawaySortDirection,
    _cursor: Option<&OpaqueCursor>,
) -> Result<PutawayWorkPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_putaway(
    request: &CreatePutawayTaskRequest,
    idempotency_key: &str,
) -> Result<CreatePutawayTaskResponse, ApiError> {
    super::browser::post("/api/v1/putaway-tasks", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_putaway(
    _request: &CreatePutawayTaskRequest,
    _idempotency_key: &str,
) -> Result<CreatePutawayTaskResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_license_plate_putaway(
    request: &CreateLicensePlatePutawayTaskRequest,
    idempotency_key: &str,
) -> Result<CreateLicensePlatePutawayTaskResponse, ApiError> {
    super::browser::post(
        "/api/v1/license-plate-putaway-tasks",
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_license_plate_putaway(
    _request: &CreateLicensePlatePutawayTaskRequest,
    _idempotency_key: &str,
) -> Result<CreateLicensePlatePutawayTaskResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn candidate_page_path(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    workflow: Option<PutawayWorkflow>,
    sort: PutawayCandidateSort,
    direction: PutawaySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/putaway-candidates?limit=100&sort={}&direction={}",
        candidate_sort_wire(sort),
        direction_wire(direction),
    );
    append_filters(&mut path, facility_id, inventory_owner_id, workflow, cursor);
    path
}

#[cfg(any(target_arch = "wasm32", test))]
#[allow(clippy::too_many_arguments)]
fn work_page_path(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    workflow: Option<PutawayWorkflow>,
    status: Option<PutawayWorkStatus>,
    sort: PutawayWorkSort,
    direction: PutawaySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/putaway-tasks?limit=100&sort={}&direction={}",
        work_sort_wire(sort),
        direction_wire(direction),
    );
    append_filters(&mut path, facility_id, inventory_owner_id, workflow, cursor);
    if let Some(status) = status {
        path.push_str("&status=");
        path.push_str(status_wire(status));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_filters(
    path: &mut String,
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    workflow: Option<PutawayWorkflow>,
    cursor: Option<&OpaqueCursor>,
) {
    if let Some(value) = facility_id {
        path.push_str(&format!("&facility_id={value}"));
    }
    if let Some(value) = inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={value}"));
    }
    if let Some(value) = workflow {
        path.push_str("&workflow=");
        path.push_str(workflow_wire(value));
    }
    if let Some(value) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(value.as_str()));
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn workflow_wire(value: PutawayWorkflow) -> &'static str {
    match value {
        PutawayWorkflow::Loose => "loose",
        PutawayWorkflow::LicensePlate => "license_plate",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn status_wire(value: PutawayWorkStatus) -> &'static str {
    match value {
        PutawayWorkStatus::Pending => "pending",
        PutawayWorkStatus::Claimed => "claimed",
        PutawayWorkStatus::Completed => "completed",
        PutawayWorkStatus::Cancelled => "cancelled",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn candidate_sort_wire(value: PutawayCandidateSort) -> &'static str {
    match value {
        PutawayCandidateSort::ReceivedAt => "received_at",
        PutawayCandidateSort::Client => "client",
        PutawayCandidateSort::Facility => "facility",
        PutawayCandidateSort::Source => "source",
        PutawayCandidateSort::Item => "item",
        PutawayCandidateSort::Quantity => "quantity",
        PutawayCandidateSort::Workflow => "workflow",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn work_sort_wire(value: PutawayWorkSort) -> &'static str {
    match value {
        PutawayWorkSort::Priority => "priority",
        PutawayWorkSort::CreatedAt => "created_at",
        PutawayWorkSort::Client => "client",
        PutawayWorkSort::Facility => "facility",
        PutawayWorkSort::Source => "source",
        PutawayWorkSort::Destination => "destination",
        PutawayWorkSort::Quantity => "quantity",
        PutawayWorkSort::Status => "status",
        PutawayWorkSort::Workflow => "workflow",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn direction_wire(value: PutawaySortDirection) -> &'static str {
    match value {
        PutawaySortDirection::Asc => "asc",
        PutawaySortDirection::Desc => "desc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_path_preserves_server_sort_and_cursor() {
        let cursor =
            OpaqueCursor::new("pc1.0000000000000004.a.l.quantity.desc.0000000000000064").unwrap();
        let path = candidate_page_path(
            Some(4),
            None,
            Some(PutawayWorkflow::Loose),
            PutawayCandidateSort::Quantity,
            PutawaySortDirection::Desc,
            Some(&cursor),
        );
        assert!(path.contains("sort=quantity&direction=desc"));
        assert!(path.contains("facility_id=4&workflow=loose"));
        assert!(path.contains(cursor.as_str()));
    }

    #[test]
    fn work_path_preserves_lifecycle_and_server_sort() {
        let path = work_page_path(
            None,
            Some(8),
            Some(PutawayWorkflow::LicensePlate),
            Some(PutawayWorkStatus::Claimed),
            PutawayWorkSort::Priority,
            PutawaySortDirection::Asc,
            None,
        );
        assert!(path.contains("sort=priority&direction=asc"));
        assert!(path.contains("inventory_owner_id=8&workflow=license_plate"));
        assert!(path.contains("status=claimed"));
    }
}
