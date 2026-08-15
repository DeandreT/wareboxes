use serde::{Deserialize, Serialize};

use super::{OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttendanceStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborActivityKind {
    Receiving,
    Putaway,
    Replenishment,
    Picking,
    Packing,
    Shipping,
    CycleCount,
    InventoryRelocation,
    CrossDock,
    Yard,
    CustomerReturn,
    VendorReturn,
    ValueAddedWork,
    Break,
    Meeting,
    Training,
    Maintenance,
    Delay,
    OtherIndirect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborActivityStatus {
    Active,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentStatus {
    Available,
    Assigned,
    OutOfService,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborQuantityBasis {
    Unit,
    Line,
    Container,
    Task,
    WeightGram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborReferenceType {
    WorkTask,
    InboundLoad,
    PickTask,
    PackingSession,
    Shipment,
    YardVisit,
    CustomerReturn,
    VendorReturn,
    ValueAddedWorkOrder,
}

impl LaborReferenceType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkTask => "work_task",
            Self::InboundLoad => "inbound_load",
            Self::PickTask => "pick_task",
            Self::PackingSession => "packing_session",
            Self::Shipment => "shipment",
            Self::YardVisit => "yard_visit",
            Self::CustomerReturn => "customer_return",
            Self::VendorReturn => "vendor_return",
            Self::ValueAddedWorkOrder => "value_added_work_order",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborExceptionReason {
    Equipment,
    Congestion,
    Inventory,
    Quality,
    Safety,
    System,
    Training,
    Personal,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborCorrectionReason {
    MissedPunch,
    TimekeepingError,
    QuantityError,
    ExceptionError,
    SystemError,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureLaborSkillRequest {
    pub code: String,
    pub name: String,
    pub certification_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertifyEmployeeRequest {
    pub employee_id: i64,
    pub skill_id: i64,
    pub facility_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certification_number: Option<String>,
    pub issued_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeEmployeeCertificationRequest {
    pub expected_revision: Revision,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureEquipmentClassRequest {
    pub code: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_skill_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEquipmentAssetRequest {
    pub facility_id: i64,
    pub equipment_class_id: i64,
    pub equipment_number: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeEquipmentStatusRequest {
    pub expected_revision: Revision,
    pub status: EquipmentStatus,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureLaborStandardRequest {
    pub facility_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    pub code: String,
    pub name: String,
    pub activity_kind: LaborActivityKind,
    pub quantity_basis: LaborQuantityBasis,
    pub setup_seconds: i64,
    pub seconds_per_unit: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_skill_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_equipment_class_id: Option<i64>,
    pub effective_from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_until: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockInRequest {
    pub employee_id: i64,
    pub facility_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockOutRequest {
    pub expected_revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartLaborActivityRequest {
    pub attendance_interval_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    pub activity_kind: LaborActivityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_basis: Option<LaborQuantityBasis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labor_standard_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equipment_asset_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborRosterPageRequest {
    pub facility_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborRosterCandidateResponse {
    pub employee_id: i64,
    pub display_name: String,
    pub title: String,
    pub facility_id: i64,
    pub attendance_interval_id: Option<i64>,
    pub attendance_revision: Option<Revision>,
    pub active_activity_id: Option<i64>,
    pub certified_skill_ids: Vec<i64>,
    pub can_clock_in: bool,
    pub can_start_activity: bool,
    pub eligibility_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborRosterPageResponse {
    pub items: Vec<LaborRosterCandidateResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborReferenceCandidatePageRequest {
    pub facility_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    pub employee_id: i64,
    pub activity_kind: LaborActivityKind,
    pub quantity_basis: LaborQuantityBasis,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborReferenceCandidateResponse {
    pub reference_type: LaborReferenceType,
    pub reference_id: i64,
    pub display_label: String,
    pub facility_id: i64,
    pub inventory_owner_id: Option<i64>,
    pub activity_kind: LaborActivityKind,
    pub quantity_basis: LaborQuantityBasis,
    pub canonical_quantity: i64,
    pub eligibility_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborReferenceCandidatePageResponse {
    pub employee_id: i64,
    pub attendance_interval_id: i64,
    pub items: Vec<LaborReferenceCandidateResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteLaborActivityRequest {
    pub expected_revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,
    #[serde(default)]
    pub exception_seconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_reason: Option<LaborExceptionReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelLaborActivityRequest {
    pub expected_revision: Revision,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectAttendanceRequest {
    pub expected_revision: Revision,
    pub corrected_clocked_in_at: String,
    pub corrected_clocked_out_at: String,
    pub reason: LaborCorrectionReason,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectLaborActivityRequest {
    pub expected_revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrected_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrected_completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,
    #[serde(default)]
    pub exception_seconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_reason: Option<LaborExceptionReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_note: Option<String>,
    pub reason: LaborCorrectionReason,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborWorkspaceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub employee_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(default)]
    pub include_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborSkillResponse {
    pub skill_id: i64,
    pub code: String,
    pub name: String,
    pub certification_required: bool,
    pub active: bool,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmployeeCertificationResponse {
    pub certification_id: i64,
    pub employee_id: i64,
    pub employee_name: String,
    pub skill_id: i64,
    pub skill_code: String,
    pub facility_id: i64,
    pub certification_number: Option<String>,
    pub issued_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub revision: Revision,
    pub certified_by: i64,
    pub certified_at: String,
    pub note: Option<String>,
    pub revoked_by: Option<i64>,
    pub revocation_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentClassResponse {
    pub equipment_class_id: i64,
    pub code: String,
    pub name: String,
    pub required_skill_id: Option<i64>,
    pub active: bool,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentAssetResponse {
    pub equipment_asset_id: i64,
    pub facility_id: i64,
    pub equipment_class_id: i64,
    pub equipment_class_code: String,
    pub equipment_number: String,
    pub name: String,
    pub status: EquipmentStatus,
    pub assigned_employee_id: Option<i64>,
    pub revision: Revision,
    pub status_note: Option<String>,
    pub configured_by: i64,
    pub configured_at: String,
    pub status_changed_by: Option<i64>,
    pub status_changed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborStandardResponse {
    pub labor_standard_id: i64,
    pub facility_id: i64,
    pub inventory_owner_id: Option<i64>,
    pub code: String,
    pub name: String,
    pub activity_kind: LaborActivityKind,
    pub quantity_basis: LaborQuantityBasis,
    pub setup_seconds: i64,
    pub seconds_per_unit: i64,
    pub required_skill_id: Option<i64>,
    pub required_equipment_class_id: Option<i64>,
    pub effective_from: String,
    pub effective_until: Option<String>,
    pub revision: Revision,
    pub supersedes_standard_id: Option<i64>,
    pub configured_by: i64,
    pub configured_at: String,
    pub retired_by: Option<i64>,
    pub retired_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttendanceIntervalResponse {
    pub attendance_interval_id: i64,
    pub employee_id: i64,
    pub employee_name: String,
    pub facility_id: i64,
    pub status: AttendanceStatus,
    pub revision: Revision,
    pub clocked_in_at: String,
    pub clocked_out_at: Option<String>,
    pub paid_seconds: Option<i64>,
    pub clocked_in_by: i64,
    pub clocked_out_by: Option<i64>,
    pub clock_in_note: Option<String>,
    pub clock_out_note: Option<String>,
    pub effective_revision: Revision,
    pub effective_clocked_in_at: String,
    pub effective_clocked_out_at: Option<String>,
    pub effective_paid_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborActivityResponse {
    pub labor_activity_id: i64,
    pub attendance_interval_id: i64,
    pub employee_id: i64,
    pub employee_name: String,
    pub facility_id: i64,
    pub inventory_owner_id: Option<i64>,
    pub activity_kind: LaborActivityKind,
    pub status: LaborActivityStatus,
    pub labor_standard_id: Option<i64>,
    pub equipment_asset_id: Option<i64>,
    pub required_skill_id: Option<i64>,
    pub required_skill_certification_id: Option<i64>,
    pub required_equipment_class_id: Option<i64>,
    pub equipment_required_skill_id: Option<i64>,
    pub equipment_skill_certification_id: Option<i64>,
    pub standard_setup_seconds: Option<i64>,
    pub standard_seconds_per_unit: Option<i64>,
    pub quantity_basis: Option<LaborQuantityBasis>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub reference_quantity: Option<i64>,
    pub revision: Revision,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub actual_seconds: Option<i64>,
    pub exception_seconds: Option<i64>,
    pub exception_reason: Option<LaborExceptionReason>,
    pub exception_note: Option<String>,
    pub exception_approved_by: Option<i64>,
    pub quantity: Option<i64>,
    pub expected_seconds: Option<i64>,
    pub efficiency_basis_points: Option<i64>,
    pub started_by: i64,
    pub completed_by: Option<i64>,
    pub cancelled_by: Option<i64>,
    pub note: Option<String>,
    pub effective_revision: Revision,
    pub effective_started_at: String,
    pub effective_completed_at: Option<String>,
    pub effective_actual_seconds: Option<i64>,
    pub effective_exception_seconds: Option<i64>,
    pub effective_exception_reason: Option<LaborExceptionReason>,
    pub effective_exception_note: Option<String>,
    pub effective_exception_approved_by: Option<i64>,
    pub effective_quantity: Option<i64>,
    pub effective_expected_seconds: Option<i64>,
    pub effective_efficiency_basis_points: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttendanceAdjustmentResponse {
    pub attendance_adjustment_id: i64,
    pub attendance_interval_id: i64,
    pub employee_id: i64,
    pub employee_name: String,
    pub facility_id: i64,
    pub supersedes_adjustment_id: Option<i64>,
    pub expected_revision: Revision,
    pub resulting_revision: Revision,
    pub before_clocked_in_at: String,
    pub before_clocked_out_at: String,
    pub before_paid_seconds: i64,
    pub corrected_clocked_in_at: String,
    pub corrected_clocked_out_at: String,
    pub corrected_paid_seconds: i64,
    pub reason: LaborCorrectionReason,
    pub note: String,
    pub adjusted_by: i64,
    pub adjusted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborActivityAdjustmentResponse {
    pub labor_activity_adjustment_id: i64,
    pub labor_activity_id: i64,
    pub employee_id: i64,
    pub employee_name: String,
    pub facility_id: i64,
    pub inventory_owner_id: Option<i64>,
    pub supersedes_adjustment_id: Option<i64>,
    pub expected_revision: Revision,
    pub resulting_revision: Revision,
    pub before_started_at: String,
    pub corrected_started_at: String,
    pub before_completed_at: String,
    pub corrected_completed_at: String,
    pub before_actual_seconds: i64,
    pub corrected_actual_seconds: i64,
    pub before_quantity: Option<i64>,
    pub corrected_quantity: Option<i64>,
    pub before_exception_seconds: i64,
    pub corrected_exception_seconds: i64,
    pub before_exception_reason: Option<LaborExceptionReason>,
    pub corrected_exception_reason: Option<LaborExceptionReason>,
    pub before_exception_note: Option<String>,
    pub corrected_exception_note: Option<String>,
    pub before_exception_approved_by: Option<i64>,
    pub corrected_exception_approved_by: Option<i64>,
    pub before_expected_seconds: Option<i64>,
    pub corrected_expected_seconds: Option<i64>,
    pub before_efficiency_basis_points: Option<i64>,
    pub corrected_efficiency_basis_points: Option<i64>,
    pub reason: LaborCorrectionReason,
    pub note: String,
    pub adjusted_by: i64,
    pub adjusted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmployeeLaborSummaryResponse {
    pub employee_id: i64,
    pub employee_name: String,
    pub paid_seconds: i64,
    pub direct_seconds: i64,
    pub indirect_seconds: i64,
    pub exception_seconds: i64,
    pub expected_seconds: i64,
    pub utilization_basis_points: Option<i64>,
    pub efficiency_basis_points: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaborWorkspaceResponse {
    pub skills: Vec<LaborSkillResponse>,
    pub certifications: Vec<EmployeeCertificationResponse>,
    pub equipment_classes: Vec<EquipmentClassResponse>,
    pub equipment_assets: Vec<EquipmentAssetResponse>,
    pub standards: Vec<LaborStandardResponse>,
    pub attendance: Vec<AttendanceIntervalResponse>,
    pub activities: Vec<LaborActivityResponse>,
    pub attendance_adjustments: Vec<AttendanceAdjustmentResponse>,
    pub activity_adjustments: Vec<LaborActivityAdjustmentResponse>,
    pub summaries: Vec<EmployeeLaborSummaryResponse>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn direct_activity_request_preserves_typed_reference_and_equipment() {
        let request: StartLaborActivityRequest = serde_json::from_value(json!({
            "attendance_interval_id": 9,
            "inventory_owner_id": 4,
            "activity_kind": "picking",
            "labor_standard_id": 3,
            "equipment_asset_id": 8,
            "reference_type": "pick_task",
            "reference_id": 17
        }))
        .unwrap();
        assert_eq!(request.activity_kind, LaborActivityKind::Picking);
        assert_eq!(request.reference_id, Some(17));
    }

    #[test]
    fn labor_requests_reject_unknown_fields_and_invalid_revisions() {
        assert!(serde_json::from_value::<ClockInRequest>(json!({
            "employee_id": 1,
            "facility_id": 2,
            "unexpected": true
        }))
        .is_err());
        assert!(serde_json::from_value::<ClockOutRequest>(json!({
            "expected_revision": 0
        }))
        .is_err());
    }
}
