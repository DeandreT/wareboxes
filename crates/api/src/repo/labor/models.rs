use sqlx::{Postgres, Row, Transaction};
use wareboxes_application::labor::{
    AttendanceAdjustmentReadModel, AttendanceIntervalReadModel, EmployeeCertificationReadModel,
    EmployeeLaborSummary, EquipmentAssetReadModel, EquipmentClassReadModel,
    LaborActivityAdjustmentReadModel, LaborActivityReadModel, LaborSkillReadModel,
    LaborStandardReadModel,
};
use wareboxes_domain::{
    AttendanceAdjustmentId, AttendanceIntervalId, AttendanceStatus, EmployeeCertificationId,
    EmployeeId, EquipmentAssetId, EquipmentClassId, EquipmentStatus, FacilityId, InventoryOwnerId,
    LaborActivityAdjustmentId, LaborActivityId, LaborActivityKind, LaborActivityStatus,
    LaborQuantityBasis, LaborRevision, LaborSkillId, LaborStandardId, TenantId, UserId,
};

use crate::error::{AppError, AppResult};

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

fn optional_id<T>(
    value: Option<i64>,
    constructor: impl Fn(i64) -> Result<T, wareboxes_domain::InvalidId>,
) -> AppResult<Option<T>> {
    value.map(constructor).transpose().map_err(internal)
}

fn parse_attendance_status(value: &str) -> AppResult<AttendanceStatus> {
    AttendanceStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored attendance status"))
}

fn parse_activity_kind(value: &str) -> AppResult<LaborActivityKind> {
    LaborActivityKind::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored labor activity kind"))
}

fn parse_activity_status(value: &str) -> AppResult<LaborActivityStatus> {
    LaborActivityStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored labor activity status"))
}

fn parse_quantity_basis(value: &str) -> AppResult<LaborQuantityBasis> {
    LaborQuantityBasis::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored labor quantity basis"))
}

fn parse_equipment_status(value: &str) -> AppResult<EquipmentStatus> {
    EquipmentStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored equipment status"))
}

pub(super) fn skill(row: &sqlx::postgres::PgRow) -> AppResult<LaborSkillReadModel> {
    Ok(LaborSkillReadModel {
        skill_id: LaborSkillId::new(row.try_get("id")?).map_err(internal)?,
        code: row.try_get("code")?,
        name: row.try_get("name")?,
        certification_required: row.try_get("certification_required")?,
        active: row.try_get("active")?,
        revision: LaborRevision::new(row.try_get("revision")?).map_err(internal)?,
        configured_by: UserId::new(row.try_get("configured_by_user_id")?).map_err(internal)?,
        configured_at: row.try_get("configured_at")?,
    })
}

pub(super) fn certification(
    row: &sqlx::postgres::PgRow,
) -> AppResult<EmployeeCertificationReadModel> {
    Ok(EmployeeCertificationReadModel {
        certification_id: EmployeeCertificationId::new(row.try_get("id")?).map_err(internal)?,
        employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?,
        employee_name: row.try_get("employee_name")?,
        skill_id: LaborSkillId::new(row.try_get("skill_id")?).map_err(internal)?,
        skill_code: row.try_get("skill_code")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        certification_number: row.try_get("certification_number")?,
        issued_at: row.try_get("issued_at")?,
        expires_at: row.try_get("expires_at")?,
        revoked_at: row.try_get("revoked_at")?,
        revision: LaborRevision::new(row.try_get("revision")?).map_err(internal)?,
        certified_by: UserId::new(row.try_get("certified_by_user_id")?).map_err(internal)?,
        certified_at: row.try_get("certified_at")?,
        note: row.try_get("note")?,
        revoked_by: optional_id(row.try_get("revoked_by_user_id")?, UserId::new)?,
        revocation_note: row.try_get("revocation_note")?,
    })
}

pub(super) fn equipment_class(row: &sqlx::postgres::PgRow) -> AppResult<EquipmentClassReadModel> {
    Ok(EquipmentClassReadModel {
        equipment_class_id: EquipmentClassId::new(row.try_get("id")?).map_err(internal)?,
        code: row.try_get("code")?,
        name: row.try_get("name")?,
        required_skill_id: optional_id(row.try_get("required_skill_id")?, LaborSkillId::new)?,
        active: row.try_get("active")?,
        revision: LaborRevision::new(row.try_get("revision")?).map_err(internal)?,
        configured_by: UserId::new(row.try_get("configured_by_user_id")?).map_err(internal)?,
        configured_at: row.try_get("configured_at")?,
    })
}

pub(super) fn equipment_asset(row: &sqlx::postgres::PgRow) -> AppResult<EquipmentAssetReadModel> {
    Ok(EquipmentAssetReadModel {
        equipment_asset_id: EquipmentAssetId::new(row.try_get("id")?).map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        equipment_class_id: EquipmentClassId::new(row.try_get("equipment_class_id")?)
            .map_err(internal)?,
        equipment_class_code: row.try_get("equipment_class_code")?,
        equipment_number: row.try_get("equipment_number")?,
        name: row.try_get("name")?,
        status: parse_equipment_status(&row.try_get::<String, _>("status")?)?,
        assigned_employee_id: optional_id(row.try_get("assigned_employee_id")?, EmployeeId::new)?,
        revision: LaborRevision::new(row.try_get("revision")?).map_err(internal)?,
        status_note: row.try_get("status_note")?,
        configured_by: UserId::new(row.try_get("configured_by_user_id")?).map_err(internal)?,
        configured_at: row.try_get("configured_at")?,
        status_changed_by: optional_id(row.try_get("status_changed_by_user_id")?, UserId::new)?,
        status_changed_at: row.try_get("status_changed_at")?,
    })
}

pub(super) fn standard(row: &sqlx::postgres::PgRow) -> AppResult<LaborStandardReadModel> {
    Ok(LaborStandardReadModel {
        labor_standard_id: LaborStandardId::new(row.try_get("id")?).map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        inventory_owner_id: optional_id(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        code: row.try_get("code")?,
        name: row.try_get("name")?,
        activity_kind: parse_activity_kind(&row.try_get::<String, _>("activity_kind")?)?,
        quantity_basis: parse_quantity_basis(&row.try_get::<String, _>("quantity_basis")?)?,
        setup_seconds: row.try_get("setup_seconds")?,
        seconds_per_unit: row.try_get("seconds_per_unit")?,
        required_skill_id: optional_id(row.try_get("required_skill_id")?, LaborSkillId::new)?,
        required_equipment_class_id: optional_id(
            row.try_get("required_equipment_class_id")?,
            EquipmentClassId::new,
        )?,
        effective_from: row.try_get("effective_from")?,
        effective_until: row.try_get("effective_until")?,
        revision: LaborRevision::new(row.try_get("revision")?).map_err(internal)?,
        supersedes_standard_id: optional_id(
            row.try_get("supersedes_standard_id")?,
            LaborStandardId::new,
        )?,
        configured_by: UserId::new(row.try_get("configured_by_user_id")?).map_err(internal)?,
        configured_at: row.try_get("configured_at")?,
        retired_by: optional_id(row.try_get("retired_by_user_id")?, UserId::new)?,
        retired_at: row.try_get("retired_at")?,
    })
}

pub(super) fn attendance(row: &sqlx::postgres::PgRow) -> AppResult<AttendanceIntervalReadModel> {
    Ok(AttendanceIntervalReadModel {
        attendance_interval_id: AttendanceIntervalId::new(row.try_get("id")?).map_err(internal)?,
        employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?,
        employee_name: row.try_get("employee_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        status: parse_attendance_status(&row.try_get::<String, _>("status")?)?,
        revision: LaborRevision::new(row.try_get("revision")?).map_err(internal)?,
        clocked_in_at: row.try_get("clocked_in_at")?,
        clocked_out_at: row.try_get("clocked_out_at")?,
        paid_seconds: row.try_get("paid_seconds")?,
        clocked_in_by: UserId::new(row.try_get("clocked_in_by_user_id")?).map_err(internal)?,
        clocked_out_by: optional_id(row.try_get("clocked_out_by_user_id")?, UserId::new)?,
        clock_in_note: row.try_get("clock_in_note")?,
        clock_out_note: row.try_get("clock_out_note")?,
        effective_revision: LaborRevision::new(row.try_get("effective_revision")?)
            .map_err(internal)?,
        effective_clocked_in_at: row.try_get("effective_clocked_in_at")?,
        effective_clocked_out_at: row.try_get("effective_clocked_out_at")?,
        effective_paid_seconds: row.try_get("effective_paid_seconds")?,
    })
}

pub(super) fn activity(row: &sqlx::postgres::PgRow) -> AppResult<LaborActivityReadModel> {
    Ok(LaborActivityReadModel {
        labor_activity_id: LaborActivityId::new(row.try_get("id")?).map_err(internal)?,
        attendance_interval_id: AttendanceIntervalId::new(row.try_get("attendance_interval_id")?)
            .map_err(internal)?,
        employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?,
        employee_name: row.try_get("employee_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        inventory_owner_id: optional_id(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        activity_kind: parse_activity_kind(&row.try_get::<String, _>("activity_kind")?)?,
        status: parse_activity_status(&row.try_get::<String, _>("status")?)?,
        labor_standard_id: optional_id(row.try_get("labor_standard_id")?, LaborStandardId::new)?,
        equipment_asset_id: optional_id(row.try_get("equipment_asset_id")?, EquipmentAssetId::new)?,
        required_skill_id: optional_id(row.try_get("required_skill_id")?, LaborSkillId::new)?,
        required_skill_certification_id: optional_id(
            row.try_get("required_skill_certification_id")?,
            EmployeeCertificationId::new,
        )?,
        required_equipment_class_id: optional_id(
            row.try_get("required_equipment_class_id")?,
            EquipmentClassId::new,
        )?,
        equipment_required_skill_id: optional_id(
            row.try_get("equipment_required_skill_id")?,
            LaborSkillId::new,
        )?,
        equipment_skill_certification_id: optional_id(
            row.try_get("equipment_skill_certification_id")?,
            EmployeeCertificationId::new,
        )?,
        standard_setup_seconds: row.try_get("standard_setup_seconds")?,
        standard_seconds_per_unit: row.try_get("standard_seconds_per_unit")?,
        quantity_basis: row
            .try_get::<Option<String>, _>("quantity_basis")?
            .map(|value| parse_quantity_basis(&value))
            .transpose()?,
        reference_type: row.try_get("reference_type")?,
        reference_id: row.try_get("reference_id")?,
        reference_quantity: row.try_get("reference_quantity")?,
        revision: LaborRevision::new(row.try_get("revision")?).map_err(internal)?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        actual_seconds: row.try_get("actual_seconds")?,
        exception_seconds: row.try_get("exception_seconds")?,
        exception_reason: row
            .try_get::<Option<String>, _>("exception_reason")?
            .map(|value| super::parse_exception_reason(&value))
            .transpose()?,
        exception_note: row.try_get("exception_note")?,
        exception_approved_by: optional_id(
            row.try_get("exception_approved_by_user_id")?,
            UserId::new,
        )?,
        quantity: row.try_get("completed_quantity")?,
        expected_seconds: row.try_get("expected_seconds")?,
        efficiency_basis_points: row.try_get("efficiency_basis_points")?,
        started_by: UserId::new(row.try_get("started_by_user_id")?).map_err(internal)?,
        completed_by: optional_id(row.try_get("completed_by_user_id")?, UserId::new)?,
        cancelled_by: optional_id(row.try_get("cancelled_by_user_id")?, UserId::new)?,
        note: row.try_get("note")?,
        effective_revision: LaborRevision::new(row.try_get("effective_revision")?)
            .map_err(internal)?,
        effective_started_at: row.try_get("effective_started_at")?,
        effective_completed_at: row.try_get("effective_completed_at")?,
        effective_actual_seconds: row.try_get("effective_actual_seconds")?,
        effective_exception_seconds: row.try_get("effective_exception_seconds")?,
        effective_exception_reason: row
            .try_get::<Option<String>, _>("effective_exception_reason")?
            .map(|value| super::parse_exception_reason(&value))
            .transpose()?,
        effective_exception_note: row.try_get("effective_exception_note")?,
        effective_exception_approved_by: optional_id(
            row.try_get("effective_exception_approved_by_user_id")?,
            UserId::new,
        )?,
        effective_quantity: row.try_get("effective_completed_quantity")?,
        effective_expected_seconds: row.try_get("effective_expected_seconds")?,
        effective_efficiency_basis_points: row.try_get("effective_efficiency_basis_points")?,
    })
}

pub(super) fn attendance_adjustment(
    row: &sqlx::postgres::PgRow,
) -> AppResult<AttendanceAdjustmentReadModel> {
    Ok(AttendanceAdjustmentReadModel {
        attendance_adjustment_id: AttendanceAdjustmentId::new(row.try_get("id")?)
            .map_err(internal)?,
        attendance_interval_id: AttendanceIntervalId::new(row.try_get("attendance_interval_id")?)
            .map_err(internal)?,
        employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?,
        employee_name: row.try_get("employee_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        supersedes_adjustment_id: optional_id(
            row.try_get("supersedes_adjustment_id")?,
            AttendanceAdjustmentId::new,
        )?,
        expected_revision: LaborRevision::new(row.try_get("expected_revision")?)
            .map_err(internal)?,
        resulting_revision: LaborRevision::new(row.try_get("resulting_revision")?)
            .map_err(internal)?,
        before_clocked_in_at: row.try_get("before_clocked_in_at")?,
        before_clocked_out_at: row.try_get("before_clocked_out_at")?,
        before_paid_seconds: row.try_get("before_paid_seconds")?,
        corrected_clocked_in_at: row.try_get("corrected_clocked_in_at")?,
        corrected_clocked_out_at: row.try_get("corrected_clocked_out_at")?,
        corrected_paid_seconds: row.try_get("corrected_paid_seconds")?,
        reason: super::parse_correction_reason(row.try_get("correction_reason")?)?,
        note: row.try_get("correction_note")?,
        adjusted_by: UserId::new(row.try_get("adjusted_by_user_id")?).map_err(internal)?,
        adjusted_at: row.try_get("adjusted_at")?,
    })
}

pub(super) fn activity_adjustment(
    row: &sqlx::postgres::PgRow,
) -> AppResult<LaborActivityAdjustmentReadModel> {
    Ok(LaborActivityAdjustmentReadModel {
        labor_activity_adjustment_id: LaborActivityAdjustmentId::new(row.try_get("id")?)
            .map_err(internal)?,
        labor_activity_id: LaborActivityId::new(row.try_get("labor_activity_id")?)
            .map_err(internal)?,
        employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?,
        employee_name: row.try_get("employee_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        inventory_owner_id: optional_id(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        supersedes_adjustment_id: optional_id(
            row.try_get("supersedes_adjustment_id")?,
            LaborActivityAdjustmentId::new,
        )?,
        expected_revision: LaborRevision::new(row.try_get("expected_revision")?)
            .map_err(internal)?,
        resulting_revision: LaborRevision::new(row.try_get("resulting_revision")?)
            .map_err(internal)?,
        before_started_at: row.try_get("before_started_at")?,
        corrected_started_at: row.try_get("corrected_started_at")?,
        before_completed_at: row.try_get("before_completed_at")?,
        corrected_completed_at: row.try_get("corrected_completed_at")?,
        before_actual_seconds: row.try_get("before_actual_seconds")?,
        corrected_actual_seconds: row.try_get("corrected_actual_seconds")?,
        before_quantity: row.try_get("before_quantity")?,
        corrected_quantity: row.try_get("corrected_quantity")?,
        before_exception_seconds: row.try_get("before_exception_seconds")?,
        corrected_exception_seconds: row.try_get("corrected_exception_seconds")?,
        before_exception_reason: row
            .try_get::<Option<String>, _>("before_exception_reason")?
            .map(|value| super::parse_exception_reason(&value))
            .transpose()?,
        corrected_exception_reason: row
            .try_get::<Option<String>, _>("corrected_exception_reason")?
            .map(|value| super::parse_exception_reason(&value))
            .transpose()?,
        before_exception_note: row.try_get("before_exception_note")?,
        corrected_exception_note: row.try_get("corrected_exception_note")?,
        before_exception_approved_by: optional_id(
            row.try_get("before_exception_approved_by_user_id")?,
            UserId::new,
        )?,
        corrected_exception_approved_by: optional_id(
            row.try_get("corrected_exception_approved_by_user_id")?,
            UserId::new,
        )?,
        before_expected_seconds: row.try_get("before_expected_seconds")?,
        corrected_expected_seconds: row.try_get("corrected_expected_seconds")?,
        before_efficiency_basis_points: row.try_get("before_efficiency_basis_points")?,
        corrected_efficiency_basis_points: row.try_get("corrected_efficiency_basis_points")?,
        reason: super::parse_correction_reason(row.try_get("correction_reason")?)?,
        note: row.try_get("correction_note")?,
        adjusted_by: UserId::new(row.try_get("adjusted_by_user_id")?).map_err(internal)?,
        adjusted_at: row.try_get("adjusted_at")?,
    })
}

pub(super) fn employee_summary(row: &sqlx::postgres::PgRow) -> AppResult<EmployeeLaborSummary> {
    Ok(EmployeeLaborSummary {
        employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?,
        employee_name: row.try_get("employee_name")?,
        paid_seconds: row.try_get("paid_seconds")?,
        direct_seconds: row.try_get("direct_seconds")?,
        indirect_seconds: row.try_get("indirect_seconds")?,
        exception_seconds: row.try_get("exception_seconds")?,
        expected_seconds: row.try_get("expected_seconds")?,
        utilization_basis_points: row.try_get("utilization_basis_points")?,
        efficiency_basis_points: row.try_get("efficiency_basis_points")?,
    })
}

pub(super) async fn read_skill_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    skill_id: LaborSkillId,
) -> AppResult<LaborSkillReadModel> {
    let row = sqlx::query("SELECT * FROM labor_skills WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(skill_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("labor skill"))?;
    skill(&row)
}

pub(super) async fn read_certification_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    certification_id: EmployeeCertificationId,
) -> AppResult<EmployeeCertificationReadModel> {
    let row = sqlx::query(
        r#"SELECT certification.*,
                  employee.first_name || ' ' || employee.last_name AS employee_name,
                  skill.code AS skill_code
           FROM employee_certifications certification
           JOIN employees employee ON employee.tenant_id=certification.tenant_id
             AND employee.id=certification.employee_id
           JOIN labor_skills skill ON skill.tenant_id=certification.tenant_id
             AND skill.id=certification.skill_id
           WHERE certification.tenant_id=$1 AND certification.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(certification_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("employee certification"))?;
    certification(&row)
}

pub(super) async fn read_equipment_class_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    equipment_class_id: EquipmentClassId,
) -> AppResult<EquipmentClassReadModel> {
    let row = sqlx::query("SELECT * FROM equipment_classes WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(equipment_class_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("equipment class"))?;
    equipment_class(&row)
}

pub(super) async fn read_equipment_asset_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    equipment_asset_id: EquipmentAssetId,
) -> AppResult<EquipmentAssetReadModel> {
    let row = sqlx::query(
        r#"SELECT asset.*,class.code AS equipment_class_code
           FROM equipment_assets asset
           JOIN equipment_classes class ON class.tenant_id=asset.tenant_id
             AND class.id=asset.equipment_class_id
           WHERE asset.tenant_id=$1 AND asset.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(equipment_asset_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("equipment asset"))?;
    equipment_asset(&row)
}

pub(super) async fn read_standard_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    labor_standard_id: LaborStandardId,
) -> AppResult<LaborStandardReadModel> {
    let row = sqlx::query("SELECT * FROM labor_standards WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(labor_standard_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("labor standard"))?;
    standard(&row)
}

pub(super) async fn read_attendance_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    attendance_interval_id: AttendanceIntervalId,
) -> AppResult<AttendanceIntervalReadModel> {
    let row = sqlx::query(
        r#"SELECT attendance.*,
                  employee.first_name || ' ' || employee.last_name AS employee_name,
                  COALESCE(correction.resulting_revision,attendance.revision)
                    AS effective_revision,
                  COALESCE(correction.corrected_clocked_in_at,attendance.clocked_in_at)
                    AS effective_clocked_in_at,
                  CASE WHEN correction.id IS NULL THEN attendance.clocked_out_at
                    ELSE correction.corrected_clocked_out_at END AS effective_clocked_out_at,
                  CASE WHEN correction.id IS NULL THEN attendance.paid_seconds
                    ELSE correction.corrected_paid_seconds END AS effective_paid_seconds
           FROM attendance_intervals attendance
           JOIN employees employee ON employee.tenant_id=attendance.tenant_id
             AND employee.id=attendance.employee_id
           LEFT JOIN LATERAL (
             SELECT adjustment.* FROM attendance_adjustments adjustment
             WHERE adjustment.tenant_id=attendance.tenant_id
               AND adjustment.attendance_interval_id=attendance.id
             ORDER BY adjustment.resulting_revision DESC,adjustment.id DESC LIMIT 1
           ) correction ON true
           WHERE attendance.tenant_id=$1 AND attendance.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(attendance_interval_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("attendance interval"))?;
    attendance(&row)
}

pub(super) async fn read_activity_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    labor_activity_id: LaborActivityId,
) -> AppResult<LaborActivityReadModel> {
    let row = sqlx::query(
        r#"SELECT activity.*,
                  employee.first_name || ' ' || employee.last_name AS employee_name,
                  COALESCE(correction.resulting_revision,activity.revision)
                    AS effective_revision,
                  COALESCE(correction.corrected_started_at,activity.started_at)
                    AS effective_started_at,
                  CASE WHEN correction.id IS NULL THEN activity.completed_at
                    ELSE correction.corrected_completed_at END AS effective_completed_at,
                  CASE WHEN correction.id IS NULL THEN activity.actual_seconds
                    ELSE correction.corrected_actual_seconds END AS effective_actual_seconds,
                  CASE WHEN correction.id IS NULL THEN activity.exception_seconds
                    ELSE correction.corrected_exception_seconds END AS effective_exception_seconds,
                  CASE WHEN correction.id IS NULL THEN activity.exception_reason
                    ELSE correction.corrected_exception_reason END AS effective_exception_reason,
                  CASE WHEN correction.id IS NULL THEN activity.exception_note
                    ELSE correction.corrected_exception_note END AS effective_exception_note,
                  CASE WHEN correction.id IS NULL THEN activity.exception_approved_by_user_id
                    ELSE correction.corrected_exception_approved_by_user_id END
                    AS effective_exception_approved_by_user_id,
                  CASE WHEN correction.id IS NULL THEN activity.completed_quantity
                    ELSE correction.corrected_quantity END AS effective_completed_quantity,
                  CASE WHEN correction.id IS NULL THEN activity.expected_seconds
                    ELSE correction.corrected_expected_seconds END AS effective_expected_seconds,
                  CASE WHEN correction.id IS NULL THEN activity.efficiency_basis_points
                    ELSE correction.corrected_efficiency_basis_points END
                    AS effective_efficiency_basis_points
           FROM labor_activities activity
           JOIN employees employee ON employee.tenant_id=activity.tenant_id
             AND employee.id=activity.employee_id
           LEFT JOIN LATERAL (
             SELECT adjustment.* FROM labor_activity_adjustments adjustment
             WHERE adjustment.tenant_id=activity.tenant_id
               AND adjustment.labor_activity_id=activity.id
             ORDER BY adjustment.resulting_revision DESC,adjustment.id DESC LIMIT 1
           ) correction ON true
           WHERE activity.tenant_id=$1 AND activity.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(labor_activity_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("labor activity"))?;
    activity(&row)
}

pub(super) async fn read_attendance_adjustment_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    adjustment_id: AttendanceAdjustmentId,
) -> AppResult<AttendanceAdjustmentReadModel> {
    let row = sqlx::query(
        r#"SELECT adjustment.*,attendance.employee_id,attendance.facility_id,
                  employee.first_name || ' ' || employee.last_name AS employee_name
           FROM attendance_adjustments adjustment
           JOIN attendance_intervals attendance ON attendance.tenant_id=adjustment.tenant_id
             AND attendance.id=adjustment.attendance_interval_id
           JOIN employees employee ON employee.tenant_id=attendance.tenant_id
             AND employee.id=attendance.employee_id
           WHERE adjustment.tenant_id=$1 AND adjustment.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(adjustment_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("attendance adjustment"))?;
    attendance_adjustment(&row)
}

pub(super) async fn read_activity_adjustment_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    adjustment_id: LaborActivityAdjustmentId,
) -> AppResult<LaborActivityAdjustmentReadModel> {
    let row = sqlx::query(
        r#"SELECT adjustment.*,activity.employee_id,activity.facility_id,
                  activity.inventory_owner_id,
                  employee.first_name || ' ' || employee.last_name AS employee_name
           FROM labor_activity_adjustments adjustment
           JOIN labor_activities activity ON activity.tenant_id=adjustment.tenant_id
             AND activity.id=adjustment.labor_activity_id
           JOIN employees employee ON employee.tenant_id=activity.tenant_id
             AND employee.id=activity.employee_id
           WHERE adjustment.tenant_id=$1 AND adjustment.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(adjustment_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("labor activity adjustment"))?;
    activity_adjustment(&row)
}
