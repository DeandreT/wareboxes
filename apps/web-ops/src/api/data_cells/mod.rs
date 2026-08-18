use wareboxes_api_contract::v1::{
    ChangeDataCellStatusRequest, DataCellEventPage, DataCellEventPageRequest, DataCellPage,
    DataCellPageRequest, DataCellResponse, ReconfigureDataCellRequest, RegisterDataCellRequest,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn data_cells(request: &DataCellPageRequest) -> Result<DataCellPage, ApiError> {
    super::browser::get(&page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn data_cells(_request: &DataCellPageRequest) -> Result<DataCellPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn data_cell(id: i64) -> Result<DataCellResponse, ApiError> {
    super::browser::get(&format!("/api/v1/platform/data-cells/{id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn data_cell(_id: i64) -> Result<DataCellResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn data_cell_events(
    id: i64,
    request: &DataCellEventPageRequest,
) -> Result<DataCellEventPage, ApiError> {
    super::browser::get(&event_path(id, request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn data_cell_events(
    _id: i64,
    _request: &DataCellEventPageRequest,
) -> Result<DataCellEventPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn register_data_cell(
    request: &RegisterDataCellRequest,
    idempotency_key: &str,
) -> Result<DataCellResponse, ApiError> {
    super::browser::post("/api/v1/platform/data-cells", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn register_data_cell(
    _request: &RegisterDataCellRequest,
    _idempotency_key: &str,
) -> Result<DataCellResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn reconfigure_data_cell(
    id: i64,
    request: &ReconfigureDataCellRequest,
    idempotency_key: &str,
) -> Result<DataCellResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/platform/data-cells/{id}/reconfigurations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn reconfigure_data_cell(
    _id: i64,
    _request: &ReconfigureDataCellRequest,
    _idempotency_key: &str,
) -> Result<DataCellResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn change_data_cell_status(
    id: i64,
    request: &ChangeDataCellStatusRequest,
    idempotency_key: &str,
) -> Result<DataCellResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/platform/data-cells/{id}/status-changes"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn change_data_cell_status(
    _id: i64,
    _request: &ChangeDataCellStatusRequest,
    _idempotency_key: &str,
) -> Result<DataCellResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(request: &DataCellPageRequest) -> String {
    let mut path = format!("/api/v1/platform/data-cells?limit={}", request.limit.get());
    if let Some(status) = request.status {
        path.push_str("&status=");
        path.push_str(status_wire(status));
    }
    if let Some(region) = request.region.as_deref() {
        path.push_str("&region=");
        path.push_str(&urlencoding::encode(region));
    }
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn event_path(id: i64, request: &DataCellEventPageRequest) -> String {
    let mut path = format!(
        "/api/v1/platform/data-cells/{id}/events?limit={}",
        request.limit.get()
    );
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_cursor(path: &mut String, cursor: Option<&wareboxes_api_contract::v1::OpaqueCursor>) {
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn status_wire(status: wareboxes_api_contract::v1::DataCellStatus) -> &'static str {
    use wareboxes_api_contract::v1::DataCellStatus;
    match status {
        DataCellStatus::Provisioning => "provisioning",
        DataCellStatus::Active => "active",
        DataCellStatus::Draining => "draining",
        DataCellStatus::Retired => "retired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{DataCellStatus, OpaqueCursor, PageLimit};

    #[test]
    fn paths_bind_filters_and_cursors() {
        let path = page_path(&DataCellPageRequest {
            status: Some(DataCellStatus::Active),
            region: Some("us-west-2".into()),
            cursor: Some(OpaqueCursor::new("dcp1.a/b+c").unwrap()),
            limit: PageLimit::new(25).unwrap(),
        });
        assert_eq!(
            path,
            "/api/v1/platform/data-cells?limit=25&status=active&region=us-west-2&cursor=dcp1.a%2Fb%2Bc"
        );
        let events = event_path(
            42,
            &DataCellEventPageRequest {
                cursor: Some(OpaqueCursor::new("dce1.a/b+c").unwrap()),
                limit: PageLimit::new(10).unwrap(),
            },
        );
        assert_eq!(
            events,
            "/api/v1/platform/data-cells/42/events?limit=10&cursor=dce1.a%2Fb%2Bc"
        );
    }
}
