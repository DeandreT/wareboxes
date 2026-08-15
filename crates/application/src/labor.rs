use wareboxes_domain::{
    AttendanceAdjustmentId, AttendanceIntervalId, AttendanceStatus, CertificationWindow,
    EmployeeCertificationId, EmployeeId, EquipmentAssetId, EquipmentClassId, EquipmentNumber,
    EquipmentStatus, FacilityId, InventoryOwnerId, LaborActivityAdjustmentId, LaborActivityId,
    LaborActivityKind, LaborActivityStatus, LaborCode, LaborCorrectionReason, LaborExceptionReason,
    LaborName, LaborNote, LaborQuantity, LaborQuantityBasis, LaborReferenceType, LaborRevision,
    LaborSkillId, LaborStandard, LaborStandardId, Timestamp, UserId,
};

pub const CONFIGURE_LABOR_SKILL_OPERATION: &str = "labor.skill.configure.v1";
pub const CERTIFY_EMPLOYEE_OPERATION: &str = "labor.employee.certify.v1";
pub const REVOKE_EMPLOYEE_CERTIFICATION_OPERATION: &str = "labor.employee.certification.revoke.v1";
pub const CONFIGURE_EQUIPMENT_CLASS_OPERATION: &str = "labor.equipment_class.configure.v1";
pub const CREATE_EQUIPMENT_ASSET_OPERATION: &str = "labor.equipment.create.v1";
pub const CHANGE_EQUIPMENT_STATUS_OPERATION: &str = "labor.equipment.status.change.v1";
pub const CONFIGURE_LABOR_STANDARD_OPERATION: &str = "labor.standard.configure.v1";
pub const CLOCK_IN_OPERATION: &str = "labor.attendance.clock_in.v1";
pub const CLOCK_OUT_OPERATION: &str = "labor.attendance.clock_out.v1";
pub const START_LABOR_ACTIVITY_OPERATION: &str = "labor.activity.start.v1";
pub const COMPLETE_LABOR_ACTIVITY_OPERATION: &str = "labor.activity.complete.v1";
pub const CANCEL_LABOR_ACTIVITY_OPERATION: &str = "labor.activity.cancel.v1";
pub const CORRECT_ATTENDANCE_OPERATION: &str = "labor.attendance.correct.v1";
pub const CORRECT_LABOR_ACTIVITY_OPERATION: &str = "labor.activity.correct.v1";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfigureLaborSkillCommand {
    pub code: LaborCode,
    pub name: LaborName,
    pub certification_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CertifyEmployeeCommand {
    pub employee_id: EmployeeId,
    pub skill_id: LaborSkillId,
    pub facility_id: FacilityId,
    pub certification_number: Option<LaborCode>,
    pub window: CertificationWindow,
    pub note: Option<LaborNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RevokeEmployeeCertificationCommand {
    pub certification_id: EmployeeCertificationId,
    pub expected_revision: LaborRevision,
    pub note: LaborNote,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfigureEquipmentClassCommand {
    pub code: LaborCode,
    pub name: LaborName,
    pub required_skill_id: Option<LaborSkillId>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CreateEquipmentAssetCommand {
    pub facility_id: FacilityId,
    pub equipment_class_id: EquipmentClassId,
    pub equipment_number: EquipmentNumber,
    pub name: LaborName,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChangeEquipmentStatusCommand {
    pub equipment_asset_id: EquipmentAssetId,
    pub expected_revision: LaborRevision,
    pub status: EquipmentStatus,
    pub note: LaborNote,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfigureLaborStandardCommand {
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub code: LaborCode,
    pub name: LaborName,
    pub activity_kind: LaborActivityKind,
    pub quantity_basis: LaborQuantityBasis,
    pub standard: LaborStandard,
    pub required_skill_id: Option<LaborSkillId>,
    pub required_equipment_class_id: Option<EquipmentClassId>,
    pub effective_from: Timestamp,
    pub effective_until: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClockInCommand {
    pub employee_id: EmployeeId,
    pub facility_id: FacilityId,
    pub note: Option<LaborNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClockOutCommand {
    pub attendance_interval_id: AttendanceIntervalId,
    pub expected_revision: LaborRevision,
    pub note: Option<LaborNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StartLaborActivityCommand {
    pub attendance_interval_id: AttendanceIntervalId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub activity_kind: LaborActivityKind,
    pub quantity_basis: Option<LaborQuantityBasis>,
    pub labor_standard_id: Option<LaborStandardId>,
    pub equipment_asset_id: Option<EquipmentAssetId>,
    pub reference_type: Option<LaborReferenceType>,
    pub reference_id: Option<i64>,
    pub note: Option<LaborNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CompleteLaborActivityCommand {
    pub labor_activity_id: LaborActivityId,
    pub expected_revision: LaborRevision,
    pub quantity: Option<LaborQuantity>,
    pub exception_seconds: i64,
    pub exception_reason: Option<LaborExceptionReason>,
    pub exception_note: Option<LaborNote>,
    pub note: Option<LaborNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CancelLaborActivityCommand {
    pub labor_activity_id: LaborActivityId,
    pub expected_revision: LaborRevision,
    pub note: LaborNote,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CorrectAttendanceCommand {
    pub attendance_interval_id: AttendanceIntervalId,
    pub expected_revision: LaborRevision,
    pub corrected_clocked_in_at: Timestamp,
    pub corrected_clocked_out_at: Timestamp,
    pub reason: LaborCorrectionReason,
    pub note: LaborNote,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CorrectLaborActivityCommand {
    pub labor_activity_id: LaborActivityId,
    pub expected_revision: LaborRevision,
    pub corrected_started_at: Option<Timestamp>,
    pub corrected_completed_at: Option<Timestamp>,
    pub quantity: Option<LaborQuantity>,
    pub exception_seconds: i64,
    pub exception_reason: Option<LaborExceptionReason>,
    pub exception_note: Option<LaborNote>,
    pub reason: LaborCorrectionReason,
    pub note: LaborNote,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborSkillReadModel {
    pub skill_id: LaborSkillId,
    pub code: String,
    pub name: String,
    pub certification_required: bool,
    pub active: bool,
    pub revision: LaborRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EmployeeCertificationReadModel {
    pub certification_id: EmployeeCertificationId,
    pub employee_id: EmployeeId,
    pub employee_name: String,
    pub skill_id: LaborSkillId,
    pub skill_code: String,
    pub facility_id: FacilityId,
    pub certification_number: Option<String>,
    pub issued_at: Timestamp,
    pub expires_at: Option<Timestamp>,
    pub revoked_at: Option<Timestamp>,
    pub revision: LaborRevision,
    pub certified_by: UserId,
    pub certified_at: Timestamp,
    pub note: Option<String>,
    pub revoked_by: Option<UserId>,
    pub revocation_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentClassReadModel {
    pub equipment_class_id: EquipmentClassId,
    pub code: String,
    pub name: String,
    pub required_skill_id: Option<LaborSkillId>,
    pub active: bool,
    pub revision: LaborRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentAssetReadModel {
    pub equipment_asset_id: EquipmentAssetId,
    pub facility_id: FacilityId,
    pub equipment_class_id: EquipmentClassId,
    pub equipment_class_code: String,
    pub equipment_number: String,
    pub name: String,
    pub status: EquipmentStatus,
    pub assigned_employee_id: Option<EmployeeId>,
    pub revision: LaborRevision,
    pub status_note: Option<String>,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub status_changed_by: Option<UserId>,
    pub status_changed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborStandardReadModel {
    pub labor_standard_id: LaborStandardId,
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub code: String,
    pub name: String,
    pub activity_kind: LaborActivityKind,
    pub quantity_basis: LaborQuantityBasis,
    pub setup_seconds: i64,
    pub seconds_per_unit: i64,
    pub required_skill_id: Option<LaborSkillId>,
    pub required_equipment_class_id: Option<EquipmentClassId>,
    pub effective_from: Timestamp,
    pub effective_until: Option<Timestamp>,
    pub revision: LaborRevision,
    pub supersedes_standard_id: Option<LaborStandardId>,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub retired_by: Option<UserId>,
    pub retired_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttendanceIntervalReadModel {
    pub attendance_interval_id: AttendanceIntervalId,
    pub employee_id: EmployeeId,
    pub employee_name: String,
    pub facility_id: FacilityId,
    pub status: AttendanceStatus,
    pub revision: LaborRevision,
    pub clocked_in_at: Timestamp,
    pub clocked_out_at: Option<Timestamp>,
    pub paid_seconds: Option<i64>,
    pub clocked_in_by: UserId,
    pub clocked_out_by: Option<UserId>,
    pub clock_in_note: Option<String>,
    pub clock_out_note: Option<String>,
    pub effective_revision: LaborRevision,
    pub effective_clocked_in_at: Timestamp,
    pub effective_clocked_out_at: Option<Timestamp>,
    pub effective_paid_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborActivityReadModel {
    pub labor_activity_id: LaborActivityId,
    pub attendance_interval_id: AttendanceIntervalId,
    pub employee_id: EmployeeId,
    pub employee_name: String,
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub activity_kind: LaborActivityKind,
    pub status: LaborActivityStatus,
    pub labor_standard_id: Option<LaborStandardId>,
    pub equipment_asset_id: Option<EquipmentAssetId>,
    pub required_skill_id: Option<LaborSkillId>,
    pub required_skill_certification_id: Option<EmployeeCertificationId>,
    pub required_equipment_class_id: Option<EquipmentClassId>,
    pub equipment_required_skill_id: Option<LaborSkillId>,
    pub equipment_skill_certification_id: Option<EmployeeCertificationId>,
    pub standard_setup_seconds: Option<i64>,
    pub standard_seconds_per_unit: Option<i64>,
    pub quantity_basis: Option<LaborQuantityBasis>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub reference_quantity: Option<i64>,
    pub revision: LaborRevision,
    pub started_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub actual_seconds: Option<i64>,
    pub exception_seconds: Option<i64>,
    pub exception_reason: Option<LaborExceptionReason>,
    pub exception_note: Option<String>,
    pub exception_approved_by: Option<UserId>,
    pub quantity: Option<i64>,
    pub expected_seconds: Option<i64>,
    pub efficiency_basis_points: Option<i64>,
    pub started_by: UserId,
    pub completed_by: Option<UserId>,
    pub cancelled_by: Option<UserId>,
    pub note: Option<String>,
    pub effective_revision: LaborRevision,
    pub effective_started_at: Timestamp,
    pub effective_completed_at: Option<Timestamp>,
    pub effective_actual_seconds: Option<i64>,
    pub effective_exception_seconds: Option<i64>,
    pub effective_exception_reason: Option<LaborExceptionReason>,
    pub effective_exception_note: Option<String>,
    pub effective_exception_approved_by: Option<UserId>,
    pub effective_quantity: Option<i64>,
    pub effective_expected_seconds: Option<i64>,
    pub effective_efficiency_basis_points: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttendanceAdjustmentReadModel {
    pub attendance_adjustment_id: AttendanceAdjustmentId,
    pub attendance_interval_id: AttendanceIntervalId,
    pub employee_id: EmployeeId,
    pub employee_name: String,
    pub facility_id: FacilityId,
    pub supersedes_adjustment_id: Option<AttendanceAdjustmentId>,
    pub expected_revision: LaborRevision,
    pub resulting_revision: LaborRevision,
    pub before_clocked_in_at: Timestamp,
    pub before_clocked_out_at: Timestamp,
    pub before_paid_seconds: i64,
    pub corrected_clocked_in_at: Timestamp,
    pub corrected_clocked_out_at: Timestamp,
    pub corrected_paid_seconds: i64,
    pub reason: LaborCorrectionReason,
    pub note: String,
    pub adjusted_by: UserId,
    pub adjusted_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborActivityAdjustmentReadModel {
    pub labor_activity_adjustment_id: LaborActivityAdjustmentId,
    pub labor_activity_id: LaborActivityId,
    pub employee_id: EmployeeId,
    pub employee_name: String,
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub supersedes_adjustment_id: Option<LaborActivityAdjustmentId>,
    pub expected_revision: LaborRevision,
    pub resulting_revision: LaborRevision,
    pub before_started_at: Timestamp,
    pub corrected_started_at: Timestamp,
    pub before_completed_at: Timestamp,
    pub corrected_completed_at: Timestamp,
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
    pub before_exception_approved_by: Option<UserId>,
    pub corrected_exception_approved_by: Option<UserId>,
    pub before_expected_seconds: Option<i64>,
    pub corrected_expected_seconds: Option<i64>,
    pub before_efficiency_basis_points: Option<i64>,
    pub corrected_efficiency_basis_points: Option<i64>,
    pub reason: LaborCorrectionReason,
    pub note: String,
    pub adjusted_by: UserId,
    pub adjusted_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EmployeeLaborSummary {
    pub employee_id: EmployeeId,
    pub employee_name: String,
    pub paid_seconds: i64,
    pub direct_seconds: i64,
    pub indirect_seconds: i64,
    pub exception_seconds: i64,
    pub expected_seconds: i64,
    pub utilization_basis_points: Option<i64>,
    pub efficiency_basis_points: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborWorkspaceReadModel {
    pub skills: Vec<LaborSkillReadModel>,
    pub certifications: Vec<EmployeeCertificationReadModel>,
    pub equipment_classes: Vec<EquipmentClassReadModel>,
    pub equipment_assets: Vec<EquipmentAssetReadModel>,
    pub standards: Vec<LaborStandardReadModel>,
    pub attendance: Vec<AttendanceIntervalReadModel>,
    pub activities: Vec<LaborActivityReadModel>,
    pub attendance_adjustments: Vec<AttendanceAdjustmentReadModel>,
    pub activity_adjustments: Vec<LaborActivityAdjustmentReadModel>,
    pub summaries: Vec<EmployeeLaborSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborRosterCandidateReadModel {
    pub employee_id: EmployeeId,
    pub display_name: String,
    pub title: String,
    pub facility_id: FacilityId,
    pub attendance_interval_id: Option<AttendanceIntervalId>,
    pub attendance_revision: Option<LaborRevision>,
    pub active_activity_id: Option<LaborActivityId>,
    pub certified_skill_ids: Vec<LaborSkillId>,
    pub can_clock_in: bool,
    pub can_start_activity: bool,
    pub eligibility_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborRosterPageReadModel {
    pub items: Vec<LaborRosterCandidateReadModel>,
    pub next_after: Option<EmployeeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborReferenceCandidateReadModel {
    pub reference_id: i64,
    pub display_label: String,
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub canonical_quantity: i64,
    pub eligibility_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaborReferenceCandidatePageReadModel {
    pub employee_id: EmployeeId,
    pub attendance_interval_id: AttendanceIntervalId,
    pub items: Vec<LaborReferenceCandidateReadModel>,
    pub next_after: Option<i64>,
}
