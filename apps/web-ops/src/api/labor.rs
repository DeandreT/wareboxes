use wareboxes_api_contract::v1::{
    AttendanceAdjustmentResponse, AttendanceIntervalResponse, CancelLaborActivityRequest,
    CertifyEmployeeRequest, ChangeEquipmentStatusRequest, ClockInRequest, ClockOutRequest,
    CompleteLaborActivityRequest, ConfigureEquipmentClassRequest, ConfigureLaborSkillRequest,
    ConfigureLaborStandardRequest, CorrectAttendanceRequest, CorrectLaborActivityRequest,
    CreateEquipmentAssetRequest, EmployeeCertificationResponse, EquipmentAssetResponse,
    EquipmentClassResponse, LaborActivityAdjustmentResponse, LaborActivityResponse,
    LaborReferenceCandidatePageRequest, LaborReferenceCandidatePageResponse,
    LaborRosterPageRequest, LaborRosterPageResponse, LaborSkillResponse, LaborStandardResponse,
    LaborWorkspaceResponse, RevokeEmployeeCertificationRequest, StartLaborActivityRequest,
};

use super::ApiError;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct LaborFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub employee_id: Option<i64>,
    pub from: Option<String>,
    pub until: Option<String>,
    pub include_history: bool,
}

#[cfg(target_arch = "wasm32")]
pub async fn labor_workspace(filters: LaborFilters) -> Result<LaborWorkspaceResponse, ApiError> {
    super::browser::get(&workspace_path(&filters)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn labor_workspace(_filters: LaborFilters) -> Result<LaborWorkspaceResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn labor_roster(
    request: &LaborRosterPageRequest,
) -> Result<LaborRosterPageResponse, ApiError> {
    super::browser::get(&roster_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn labor_roster(
    _request: &LaborRosterPageRequest,
) -> Result<LaborRosterPageResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn labor_reference_candidates(
    request: &LaborReferenceCandidatePageRequest,
) -> Result<LaborReferenceCandidatePageResponse, ApiError> {
    super::browser::get(&reference_candidates_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn labor_reference_candidates(
    _request: &LaborReferenceCandidatePageRequest,
) -> Result<LaborReferenceCandidatePageResponse, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! command {
    ($name:ident, $request:ty, $response:ty, $path:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            request: &$request,
            idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            super::browser::post($path, request, idempotency_key).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _request: &$request,
            _idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

command!(
    configure_labor_skill,
    ConfigureLaborSkillRequest,
    LaborSkillResponse,
    "/api/v1/labor/skills"
);
command!(
    certify_labor_employee,
    CertifyEmployeeRequest,
    EmployeeCertificationResponse,
    "/api/v1/labor/certifications"
);
command!(
    configure_labor_equipment_class,
    ConfigureEquipmentClassRequest,
    EquipmentClassResponse,
    "/api/v1/labor/equipment-classes"
);
command!(
    create_labor_equipment_asset,
    CreateEquipmentAssetRequest,
    EquipmentAssetResponse,
    "/api/v1/labor/equipment-assets"
);
command!(
    configure_labor_standard,
    ConfigureLaborStandardRequest,
    LaborStandardResponse,
    "/api/v1/labor/standards"
);
command!(
    clock_in_labor,
    ClockInRequest,
    AttendanceIntervalResponse,
    "/api/v1/labor/attendance"
);
command!(
    start_labor_activity,
    StartLaborActivityRequest,
    LaborActivityResponse,
    "/api/v1/labor/activities"
);

macro_rules! target_command {
    ($name:ident, $request:ty, $response:ty, $path:expr) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            target_id: i64,
            request: &$request,
            idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            super::browser::post(&$path(target_id), request, idempotency_key).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _target_id: i64,
            _request: &$request,
            _idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

target_command!(
    clock_out_labor,
    ClockOutRequest,
    AttendanceIntervalResponse,
    |id| format!("/api/v1/labor/attendance/{id}/clock-outs")
);
target_command!(
    complete_labor_activity,
    CompleteLaborActivityRequest,
    LaborActivityResponse,
    |id| format!("/api/v1/labor/activities/{id}/completions")
);
target_command!(
    cancel_labor_activity,
    CancelLaborActivityRequest,
    LaborActivityResponse,
    |id| format!("/api/v1/labor/activities/{id}/cancellations")
);
target_command!(
    correct_labor_attendance,
    CorrectAttendanceRequest,
    AttendanceAdjustmentResponse,
    |id| format!("/api/v1/labor/attendance/{id}/corrections")
);
target_command!(
    correct_labor_activity,
    CorrectLaborActivityRequest,
    LaborActivityAdjustmentResponse,
    |id| format!("/api/v1/labor/activities/{id}/corrections")
);
target_command!(
    change_labor_equipment_status,
    ChangeEquipmentStatusRequest,
    EquipmentAssetResponse,
    |id| format!("/api/v1/labor/equipment-assets/{id}/status-changes")
);
target_command!(
    revoke_labor_certification,
    RevokeEmployeeCertificationRequest,
    EmployeeCertificationResponse,
    |id| format!("/api/v1/labor/certifications/{id}/revocations")
);

#[cfg(any(target_arch = "wasm32", test))]
fn workspace_path(filters: &LaborFilters) -> String {
    let mut path = format!(
        "/api/v1/labor/workspace?include_history={}",
        filters.include_history
    );
    if let Some(facility_id) = filters.facility_id {
        path.push_str(&format!("&facility_id={facility_id}"));
    }
    if let Some(owner_id) = filters.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    if let Some(employee_id) = filters.employee_id {
        path.push_str(&format!("&employee_id={employee_id}"));
    }
    if let Some(from) = filters.from.as_deref() {
        path.push_str("&from=");
        path.push_str(&urlencoding::encode(from));
    }
    if let Some(until) = filters.until.as_deref() {
        path.push_str("&until=");
        path.push_str(&urlencoding::encode(until));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn roster_path(request: &LaborRosterPageRequest) -> String {
    let mut path = format!(
        "/api/v1/labor/roster?facility_id={}&limit={}",
        request.facility_id,
        request.limit.get()
    );
    if let Some(owner_id) = request.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    if let Some(cursor) = request.cursor.as_ref() {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn reference_candidates_path(request: &LaborReferenceCandidatePageRequest) -> String {
    let mut path = format!(
        "/api/v1/labor/reference-candidates?facility_id={}&employee_id={}&activity_kind={}&quantity_basis={}&limit={}",
        request.facility_id,
        request.employee_id,
        activity_kind_wire(request.activity_kind),
        quantity_basis_wire(request.quantity_basis),
        request.limit.get(),
    );
    if let Some(owner_id) = request.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    if let Some(cursor) = request.cursor.as_ref() {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
const fn activity_kind_wire(value: wareboxes_api_contract::v1::LaborActivityKind) -> &'static str {
    use wareboxes_api_contract::v1::LaborActivityKind::*;
    match value {
        Receiving => "receiving",
        Putaway => "putaway",
        Replenishment => "replenishment",
        Picking => "picking",
        Packing => "packing",
        Shipping => "shipping",
        CycleCount => "cycle_count",
        InventoryRelocation => "inventory_relocation",
        CrossDock => "cross_dock",
        Yard => "yard",
        CustomerReturn => "customer_return",
        VendorReturn => "vendor_return",
        ValueAddedWork => "value_added_work",
        Break => "break",
        Meeting => "meeting",
        Training => "training",
        Maintenance => "maintenance",
        Delay => "delay",
        OtherIndirect => "other_indirect",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn quantity_basis_wire(
    value: wareboxes_api_contract::v1::LaborQuantityBasis,
) -> &'static str {
    use wareboxes_api_contract::v1::LaborQuantityBasis::*;
    match value {
        Unit => "unit",
        Line => "line",
        Container => "container",
        Task => "task",
        WeightGram => "weight_gram",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{
        LaborActivityKind, LaborQuantityBasis, OpaqueCursor, PageLimit,
    };

    #[test]
    fn workspace_path_binds_scope_employee_window_and_history() {
        assert_eq!(
            workspace_path(&LaborFilters {
                facility_id: Some(7),
                inventory_owner_id: Some(9),
                employee_id: Some(11),
                from: Some("2026-08-14T07:00:00Z".into()),
                until: Some("2026-08-15T07:00:00Z".into()),
                include_history: true,
            }),
            "/api/v1/labor/workspace?include_history=true&facility_id=7&inventory_owner_id=9&employee_id=11&from=2026-08-14T07%3A00%3A00Z&until=2026-08-15T07%3A00%3A00Z"
        );
    }

    #[test]
    fn workspace_path_keeps_server_default_window_when_dates_are_absent() {
        assert_eq!(
            workspace_path(&LaborFilters::default()),
            "/api/v1/labor/workspace?include_history=false"
        );
    }

    #[test]
    fn candidate_paths_preserve_typed_scope_and_cursor() {
        let cursor = OpaqueCursor::new("lr1.abc").unwrap();
        assert_eq!(
            roster_path(&LaborRosterPageRequest {
                facility_id: 7,
                inventory_owner_id: Some(9),
                limit: PageLimit::new(25).unwrap(),
                cursor: Some(cursor),
            }),
            "/api/v1/labor/roster?facility_id=7&limit=25&inventory_owner_id=9&cursor=lr1.abc"
        );
        assert_eq!(
            reference_candidates_path(&LaborReferenceCandidatePageRequest {
                facility_id: 7,
                inventory_owner_id: Some(9),
                employee_id: 11,
                activity_kind: LaborActivityKind::Picking,
                quantity_basis: LaborQuantityBasis::Unit,
                limit: PageLimit::new(50).unwrap(),
                cursor: None,
            }),
            "/api/v1/labor/reference-candidates?facility_id=7&employee_id=11&activity_kind=picking&quantity_basis=unit&limit=50&inventory_owner_id=9"
        );
    }
}
