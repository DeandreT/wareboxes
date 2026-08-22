use wareboxes_api_contract::v1::{
    CancelTenantCellMoveRequest, CheckpointTenantCellMoveRequest, CompleteTenantCellMoveRequest,
    CutoverTenantCellMoveRequest, FreezeTenantCellMoveRequest, PlanTenantCellMoveRequest,
    RollbackTenantCellMoveRequest, StartTenantCellMoveCopyRequest, TenantCellMoveEventPage,
    TenantCellMoveEventPageRequest, TenantCellMovePage, TenantCellMovePageRequest,
    TenantCellMoveResponse, ValidateTenantCellMoveRequest, VerifyTenantCellMoveCutoverRequest,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn tenant_cell_moves(
    request: &TenantCellMovePageRequest,
) -> Result<TenantCellMovePage, ApiError> {
    super::browser::get(&page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn tenant_cell_moves(
    _request: &TenantCellMovePageRequest,
) -> Result<TenantCellMovePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn tenant_cell_move(id: i64) -> Result<TenantCellMoveResponse, ApiError> {
    super::browser::get(&format!("/api/v1/platform/tenant-cell-moves/{id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn tenant_cell_move(_id: i64) -> Result<TenantCellMoveResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn tenant_cell_move_events(
    id: i64,
    request: &TenantCellMoveEventPageRequest,
) -> Result<TenantCellMoveEventPage, ApiError> {
    super::browser::get(&event_path(id, request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn tenant_cell_move_events(
    _id: i64,
    _request: &TenantCellMoveEventPageRequest,
) -> Result<TenantCellMoveEventPage, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! command_client {
    ($name:ident, $request:ty, $suffix:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            id: i64,
            request: &$request,
            idempotency_key: &str,
        ) -> Result<TenantCellMoveResponse, ApiError> {
            super::browser::post(
                &format!(
                    concat!("/api/v1/platform/tenant-cell-moves/{}/", $suffix),
                    id
                ),
                request,
                idempotency_key,
            )
            .await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _id: i64,
            _request: &$request,
            _idempotency_key: &str,
        ) -> Result<TenantCellMoveResponse, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

#[cfg(target_arch = "wasm32")]
pub async fn plan_tenant_cell_move(
    tenant_id: i64,
    request: &PlanTenantCellMoveRequest,
    idempotency_key: &str,
) -> Result<TenantCellMoveResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/platform/tenants/{tenant_id}/cell-moves"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan_tenant_cell_move(
    _tenant_id: i64,
    _request: &PlanTenantCellMoveRequest,
    _idempotency_key: &str,
) -> Result<TenantCellMoveResponse, ApiError> {
    Err(ApiError::unavailable())
}

command_client!(
    start_tenant_cell_move_copy,
    StartTenantCellMoveCopyRequest,
    "copy-starts"
);
command_client!(
    checkpoint_tenant_cell_move,
    CheckpointTenantCellMoveRequest,
    "checkpoints"
);
command_client!(
    freeze_tenant_cell_move,
    FreezeTenantCellMoveRequest,
    "write-freezes"
);
command_client!(
    validate_tenant_cell_move,
    ValidateTenantCellMoveRequest,
    "validations"
);
command_client!(
    cutover_tenant_cell_move,
    CutoverTenantCellMoveRequest,
    "cutovers"
);
command_client!(
    verify_tenant_cell_move_cutover,
    VerifyTenantCellMoveCutoverRequest,
    "cutover-verifications"
);
command_client!(
    complete_tenant_cell_move,
    CompleteTenantCellMoveRequest,
    "completions"
);
command_client!(
    rollback_tenant_cell_move,
    RollbackTenantCellMoveRequest,
    "rollbacks"
);
command_client!(
    cancel_tenant_cell_move,
    CancelTenantCellMoveRequest,
    "cancellations"
);

#[cfg(target_arch = "wasm32")]
fn page_path(request: &TenantCellMovePageRequest) -> String {
    let mut path = format!(
        "/api/v1/platform/tenant-cell-moves?limit={}",
        request.limit.get()
    );
    if let Some(tenant_id) = request.tenant_id {
        path.push_str("&tenant_id=");
        path.push_str(&tenant_id.to_string());
    }
    if let Some(data_cell_id) = request.data_cell_id {
        path.push_str("&data_cell_id=");
        path.push_str(&data_cell_id.to_string());
    }
    if let Some(status) = request.status {
        path.push_str("&status=");
        path.push_str(status_wire(status));
    }
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(target_arch = "wasm32")]
fn event_path(id: i64, request: &TenantCellMoveEventPageRequest) -> String {
    let mut path = format!(
        "/api/v1/platform/tenant-cell-moves/{id}/events?limit={}",
        request.limit.get()
    );
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(target_arch = "wasm32")]
fn append_cursor(path: &mut String, cursor: Option<&wareboxes_api_contract::v1::OpaqueCursor>) {
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
}

#[cfg(target_arch = "wasm32")]
const fn status_wire(status: wareboxes_api_contract::v1::TenantCellMoveStatus) -> &'static str {
    use wareboxes_api_contract::v1::TenantCellMoveStatus;
    match status {
        TenantCellMoveStatus::Planned => "planned",
        TenantCellMoveStatus::Copying => "copying",
        TenantCellMoveStatus::Frozen => "frozen",
        TenantCellMoveStatus::Validated => "validated",
        TenantCellMoveStatus::CutOver => "cut_over",
        TenantCellMoveStatus::Completed => "completed",
        TenantCellMoveStatus::Cancelled => "cancelled",
        TenantCellMoveStatus::RolledBack => "rolled_back",
    }
}
