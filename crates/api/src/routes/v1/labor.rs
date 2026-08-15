use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::Duration;
use wareboxes_api_contract::v1::{
    AttendanceAdjustmentResponse, AttendanceIntervalResponse,
    AttendanceStatus as ApiAttendanceStatus, CancelLaborActivityRequest, CertifyEmployeeRequest,
    ChangeEquipmentStatusRequest, ClockInRequest, ClockOutRequest, CompleteLaborActivityRequest,
    ConfigureEquipmentClassRequest, ConfigureLaborSkillRequest, ConfigureLaborStandardRequest,
    CorrectAttendanceRequest, CorrectLaborActivityRequest, CreateEquipmentAssetRequest,
    EmployeeCertificationResponse, EmployeeLaborSummaryResponse, EquipmentAssetResponse,
    EquipmentClassResponse, EquipmentStatus as ApiEquipmentStatus, LaborActivityAdjustmentResponse,
    LaborActivityKind as ApiActivityKind, LaborActivityResponse,
    LaborActivityStatus as ApiActivityStatus, LaborCorrectionReason as ApiCorrectionReason,
    LaborExceptionReason as ApiExceptionReason, LaborQuantityBasis as ApiQuantityBasis,
    LaborSkillResponse, LaborStandardResponse, LaborWorkspaceRequest, LaborWorkspaceResponse,
    Revision, RevokeEmployeeCertificationRequest, StartLaborActivityRequest,
};
use wareboxes_application::labor::{
    AttendanceAdjustmentReadModel, AttendanceIntervalReadModel, CancelLaborActivityCommand,
    CertifyEmployeeCommand, ChangeEquipmentStatusCommand, ClockInCommand, ClockOutCommand,
    CompleteLaborActivityCommand, ConfigureEquipmentClassCommand, ConfigureLaborSkillCommand,
    ConfigureLaborStandardCommand, CorrectAttendanceCommand, CorrectLaborActivityCommand,
    CreateEquipmentAssetCommand, EmployeeCertificationReadModel, EmployeeLaborSummary,
    EquipmentAssetReadModel, EquipmentClassReadModel, LaborActivityAdjustmentReadModel,
    LaborActivityReadModel, LaborSkillReadModel, LaborStandardReadModel,
    RevokeEmployeeCertificationCommand, StartLaborActivityCommand,
};
use wareboxes_domain::{
    AttendanceIntervalId, AttendanceStatus, CertificationWindow, EmployeeCertificationId,
    EmployeeId, EquipmentAssetId, EquipmentClassId, EquipmentNumber, EquipmentStatus, FacilityId,
    InventoryOwnerId, LaborActivityId, LaborActivityKind, LaborActivityStatus, LaborCode,
    LaborCorrectionReason, LaborExceptionReason, LaborName, LaborNote, LaborQuantity,
    LaborQuantityBasis, LaborReferenceType, LaborRevision, LaborSkillId, LaborStandard,
    LaborStandardId, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::db::now_iso;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

mod candidates;
pub use candidates::{reference_candidates, roster_candidates};

const VIEW_PERMISSION: &str = "labor_view";
const CONFIGURE_PERMISSION: &str = "labor_configure";
const CERTIFY_PERMISSION: &str = "labor_certify";
const EQUIPMENT_PERMISSION: &str = "labor_equipment";
const SUPERVISE_PERMISSION: &str = "labor_supervise";
const EXECUTE_PERMISSION: &str = "labor_execute";
const DEFAULT_WORKSPACE_DAYS: i64 = 1;
const MAX_WORKSPACE_DAYS: i64 = 31;

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<LaborWorkspaceRequest>,
) -> V1Result<Json<LaborWorkspaceResponse>> {
    user.require_permission(&state.db, VIEW_PERMISSION).await?;

    let until = request
        .until
        .as_deref()
        .map(|value| parse_timestamp(value, "until"))
        .transpose()?
        .unwrap_or_else(now_iso);
    let from = request
        .from
        .as_deref()
        .map(|value| parse_timestamp(value, "from"))
        .transpose()?
        .unwrap_or_else(|| until - Duration::days(DEFAULT_WORKSPACE_DAYS));
    validate_workspace_window(from, until)?;

    let value = repo::labor::workspace(
        &state.db,
        &user.tenant,
        &repo::labor::LaborWorkspaceFilter {
            facility_id: request
                .facility_id
                .map(FacilityId::new)
                .transpose()
                .map_err(validation)?,
            inventory_owner_id: request
                .inventory_owner_id
                .map(InventoryOwnerId::new)
                .transpose()
                .map_err(validation)?,
            employee_id: request
                .employee_id
                .map(EmployeeId::new)
                .transpose()
                .map_err(validation)?,
            from,
            until,
            include_history: request.include_history,
        },
    )
    .await?;

    Ok(Json(LaborWorkspaceResponse {
        skills: value
            .skills
            .into_iter()
            .map(map_skill)
            .collect::<V1Result<Vec<_>>>()?,
        certifications: value
            .certifications
            .into_iter()
            .map(map_certification)
            .collect::<V1Result<Vec<_>>>()?,
        equipment_classes: value
            .equipment_classes
            .into_iter()
            .map(map_equipment_class)
            .collect::<V1Result<Vec<_>>>()?,
        equipment_assets: value
            .equipment_assets
            .into_iter()
            .map(map_equipment_asset)
            .collect::<V1Result<Vec<_>>>()?,
        standards: value
            .standards
            .into_iter()
            .map(map_standard)
            .collect::<V1Result<Vec<_>>>()?,
        attendance: value
            .attendance
            .into_iter()
            .map(map_attendance)
            .collect::<V1Result<Vec<_>>>()?,
        activities: value
            .activities
            .into_iter()
            .map(map_activity)
            .collect::<V1Result<Vec<_>>>()?,
        attendance_adjustments: value
            .attendance_adjustments
            .into_iter()
            .map(map_attendance_adjustment)
            .collect::<V1Result<Vec<_>>>()?,
        activity_adjustments: value
            .activity_adjustments
            .into_iter()
            .map(map_activity_adjustment)
            .collect::<V1Result<Vec<_>>>()?,
        summaries: value.summaries.into_iter().map(map_summary).collect(),
    }))
}

pub async fn configure_skill(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureLaborSkillRequest>,
) -> V1Result<Json<LaborSkillResponse>> {
    user.require_permission(&state.db, CONFIGURE_PERMISSION)
        .await?;
    let command = ConfigureLaborSkillCommand {
        code: LaborCode::new(body.code).map_err(validation)?,
        name: LaborName::new(body.name).map_err(validation)?,
        certification_required: body.certification_required,
    };
    let value = repo::labor::configure_skill(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_skill(value)?))
}

pub async fn certify_employee(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CertifyEmployeeRequest>,
) -> V1Result<Json<EmployeeCertificationResponse>> {
    user.require_permission(&state.db, CERTIFY_PERMISSION)
        .await?;
    let issued_at = parse_timestamp(&body.issued_at, "issued_at")?;
    let expires_at = body
        .expires_at
        .as_deref()
        .map(|value| parse_timestamp(value, "expires_at"))
        .transpose()?;
    let command = CertifyEmployeeCommand {
        employee_id: EmployeeId::new(body.employee_id).map_err(validation)?,
        skill_id: LaborSkillId::new(body.skill_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        certification_number: body
            .certification_number
            .map(LaborCode::new)
            .transpose()
            .map_err(validation)?,
        window: CertificationWindow::new(issued_at, expires_at).map_err(validation)?,
        note: optional_note(body.note)?,
    };
    let value = repo::labor::certify_employee(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_certification(value)?))
}

pub async fn revoke_certification(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(certification_id): Path<i64>,
    Json(body): Json<RevokeEmployeeCertificationRequest>,
) -> V1Result<Json<EmployeeCertificationResponse>> {
    user.require_permission(&state.db, CERTIFY_PERMISSION)
        .await?;
    let command = RevokeEmployeeCertificationCommand {
        certification_id: EmployeeCertificationId::new(certification_id).map_err(validation)?,
        expected_revision: labor_revision(body.expected_revision)?,
        note: LaborNote::new(body.note).map_err(validation)?,
    };
    let value = repo::labor::revoke_certification(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_certification(value)?))
}

pub async fn configure_equipment_class(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureEquipmentClassRequest>,
) -> V1Result<Json<EquipmentClassResponse>> {
    user.require_permission(&state.db, CONFIGURE_PERMISSION)
        .await?;
    let command = ConfigureEquipmentClassCommand {
        code: LaborCode::new(body.code).map_err(validation)?,
        name: LaborName::new(body.name).map_err(validation)?,
        required_skill_id: body
            .required_skill_id
            .map(LaborSkillId::new)
            .transpose()
            .map_err(validation)?,
    };
    let value = repo::labor::configure_equipment_class(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_equipment_class(value)?))
}

pub async fn create_equipment_asset(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateEquipmentAssetRequest>,
) -> V1Result<Json<EquipmentAssetResponse>> {
    user.require_permission(&state.db, EQUIPMENT_PERMISSION)
        .await?;
    let command = CreateEquipmentAssetCommand {
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        equipment_class_id: EquipmentClassId::new(body.equipment_class_id).map_err(validation)?,
        equipment_number: EquipmentNumber::new(body.equipment_number).map_err(validation)?,
        name: LaborName::new(body.name).map_err(validation)?,
    };
    let value = repo::labor::create_equipment_asset(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_equipment_asset(value)?))
}

pub async fn change_equipment_status(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(equipment_asset_id): Path<i64>,
    Json(body): Json<ChangeEquipmentStatusRequest>,
) -> V1Result<Json<EquipmentAssetResponse>> {
    user.require_permission(&state.db, EQUIPMENT_PERMISSION)
        .await?;
    let status = equipment_status_from_api(body.status);
    if status == EquipmentStatus::Assigned {
        return Err(validation(
            "assigned equipment status is controlled by labor activities",
        ));
    }
    let command = ChangeEquipmentStatusCommand {
        equipment_asset_id: EquipmentAssetId::new(equipment_asset_id).map_err(validation)?,
        expected_revision: labor_revision(body.expected_revision)?,
        status,
        note: LaborNote::new(body.note).map_err(validation)?,
    };
    let value = repo::labor::change_equipment_status(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_equipment_asset(value)?))
}

pub async fn configure_standard(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureLaborStandardRequest>,
) -> V1Result<Json<LaborStandardResponse>> {
    user.require_permission(&state.db, CONFIGURE_PERMISSION)
        .await?;
    let activity_kind = activity_kind_from_api(body.activity_kind);
    if !activity_kind.is_direct() {
        return Err(validation(
            "labor standards can only be configured for direct activities",
        ));
    }
    let effective_from = parse_timestamp(&body.effective_from, "effective_from")?;
    let effective_until = body
        .effective_until
        .as_deref()
        .map(|value| parse_timestamp(value, "effective_until"))
        .transpose()?;
    if effective_until.is_some_and(|until| until <= effective_from) {
        return Err(validation("effective_until must be after effective_from"));
    }
    let command = ConfigureLaborStandardCommand {
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        inventory_owner_id: body
            .inventory_owner_id
            .map(InventoryOwnerId::new)
            .transpose()
            .map_err(validation)?,
        code: LaborCode::new(body.code).map_err(validation)?,
        name: LaborName::new(body.name).map_err(validation)?,
        activity_kind,
        quantity_basis: quantity_basis_from_api(body.quantity_basis),
        standard: LaborStandard::new(body.setup_seconds, body.seconds_per_unit)
            .map_err(validation)?,
        required_skill_id: body
            .required_skill_id
            .map(LaborSkillId::new)
            .transpose()
            .map_err(validation)?,
        required_equipment_class_id: body
            .required_equipment_class_id
            .map(EquipmentClassId::new)
            .transpose()
            .map_err(validation)?,
        effective_from,
        effective_until,
    };
    let value = repo::labor::configure_standard(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_standard(value)?))
}

pub async fn clock_in(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ClockInRequest>,
) -> V1Result<Json<AttendanceIntervalResponse>> {
    user.require_any_permission(&state.db, &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION])
        .await?;
    let command = ClockInCommand {
        employee_id: EmployeeId::new(body.employee_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        note: optional_note(body.note)?,
    };
    let value = repo::labor::clock_in(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_attendance(value)?))
}

pub async fn clock_out(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(attendance_interval_id): Path<i64>,
    Json(body): Json<ClockOutRequest>,
) -> V1Result<Json<AttendanceIntervalResponse>> {
    user.require_any_permission(&state.db, &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION])
        .await?;
    let command = ClockOutCommand {
        attendance_interval_id: AttendanceIntervalId::new(attendance_interval_id)
            .map_err(validation)?,
        expected_revision: labor_revision(body.expected_revision)?,
        note: optional_note(body.note)?,
    };
    let value = repo::labor::clock_out(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_attendance(value)?))
}

pub async fn start_activity(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<StartLaborActivityRequest>,
) -> V1Result<Json<LaborActivityResponse>> {
    user.require_any_permission(&state.db, &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION])
        .await?;
    let activity_kind = activity_kind_from_api(body.activity_kind);
    validate_activity_shape(
        activity_kind,
        body.inventory_owner_id,
        body.quantity_basis,
        body.labor_standard_id,
        body.reference_type.as_deref(),
        body.reference_id,
    )?;
    let command = StartLaborActivityCommand {
        attendance_interval_id: AttendanceIntervalId::new(body.attendance_interval_id)
            .map_err(validation)?,
        inventory_owner_id: body
            .inventory_owner_id
            .map(InventoryOwnerId::new)
            .transpose()
            .map_err(validation)?,
        activity_kind,
        quantity_basis: body.quantity_basis.map(quantity_basis_from_api),
        labor_standard_id: body
            .labor_standard_id
            .map(LaborStandardId::new)
            .transpose()
            .map_err(validation)?,
        equipment_asset_id: body
            .equipment_asset_id
            .map(EquipmentAssetId::new)
            .transpose()
            .map_err(validation)?,
        reference_type: body
            .reference_type
            .map(LaborReferenceType::new)
            .transpose()
            .map_err(validation)?,
        reference_id: body.reference_id,
        note: optional_note(body.note)?,
    };
    let value = repo::labor::start_activity(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_activity(value)?))
}

pub async fn complete_activity(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(labor_activity_id): Path<i64>,
    Json(body): Json<CompleteLaborActivityRequest>,
) -> V1Result<Json<LaborActivityResponse>> {
    user.require_any_permission(&state.db, &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION])
        .await?;
    if body.exception_seconds < 0 {
        return Err(validation("exception_seconds cannot be negative"));
    }
    if (body.exception_seconds == 0)
        != (body.exception_reason.is_none() && body.exception_note.is_none())
    {
        return Err(validation(
            "nonzero exception_seconds require exception_reason and exception_note; zero forbids them",
        ));
    }
    if body.exception_seconds > 0
        && (body.exception_reason.is_none() || body.exception_note.is_none())
    {
        return Err(validation(
            "nonzero exception_seconds require exception_reason and exception_note",
        ));
    }
    let command = CompleteLaborActivityCommand {
        labor_activity_id: LaborActivityId::new(labor_activity_id).map_err(validation)?,
        expected_revision: labor_revision(body.expected_revision)?,
        quantity: body
            .quantity
            .map(LaborQuantity::new)
            .transpose()
            .map_err(validation)?,
        exception_seconds: body.exception_seconds,
        exception_reason: body.exception_reason.map(exception_reason_from_api),
        exception_note: optional_note(body.exception_note)?,
        note: optional_note(body.note)?,
    };
    let value = repo::labor::complete_activity(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_activity(value)?))
}

pub async fn cancel_activity(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(labor_activity_id): Path<i64>,
    Json(body): Json<CancelLaborActivityRequest>,
) -> V1Result<Json<LaborActivityResponse>> {
    user.require_any_permission(&state.db, &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION])
        .await?;
    let command = CancelLaborActivityCommand {
        labor_activity_id: LaborActivityId::new(labor_activity_id).map_err(validation)?,
        expected_revision: labor_revision(body.expected_revision)?,
        note: LaborNote::new(body.note).map_err(validation)?,
    };
    let value = repo::labor::cancel_activity(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_activity(value)?))
}

pub async fn correct_attendance(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(attendance_interval_id): Path<i64>,
    Json(body): Json<CorrectAttendanceRequest>,
) -> V1Result<Json<AttendanceAdjustmentResponse>> {
    user.require_permission(&state.db, SUPERVISE_PERMISSION)
        .await?;
    let command = CorrectAttendanceCommand {
        attendance_interval_id: AttendanceIntervalId::new(attendance_interval_id)
            .map_err(validation)?,
        expected_revision: labor_revision(body.expected_revision)?,
        corrected_clocked_in_at: parse_timestamp(
            &body.corrected_clocked_in_at,
            "corrected_clocked_in_at",
        )?,
        corrected_clocked_out_at: parse_timestamp(
            &body.corrected_clocked_out_at,
            "corrected_clocked_out_at",
        )?,
        reason: correction_reason_from_api(body.reason),
        note: LaborNote::new(body.note).map_err(validation)?,
    };
    let value = repo::labor::correct_attendance(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_attendance_adjustment(value)?))
}

pub async fn correct_activity(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(labor_activity_id): Path<i64>,
    Json(body): Json<CorrectLaborActivityRequest>,
) -> V1Result<Json<LaborActivityAdjustmentResponse>> {
    user.require_permission(&state.db, SUPERVISE_PERMISSION)
        .await?;
    if body.exception_seconds < 0 {
        return Err(validation("exception_seconds cannot be negative"));
    }
    if (body.exception_seconds == 0)
        != (body.exception_reason.is_none() && body.exception_note.is_none())
    {
        return Err(validation(
            "nonzero exception_seconds require exception_reason and exception_note; zero forbids them",
        ));
    }
    let command = CorrectLaborActivityCommand {
        labor_activity_id: LaborActivityId::new(labor_activity_id).map_err(validation)?,
        expected_revision: labor_revision(body.expected_revision)?,
        corrected_started_at: body
            .corrected_started_at
            .as_deref()
            .map(|value| parse_timestamp(value, "corrected_started_at"))
            .transpose()?,
        corrected_completed_at: body
            .corrected_completed_at
            .as_deref()
            .map(|value| parse_timestamp(value, "corrected_completed_at"))
            .transpose()?,
        quantity: body
            .quantity
            .map(LaborQuantity::new)
            .transpose()
            .map_err(validation)?,
        exception_seconds: body.exception_seconds,
        exception_reason: body.exception_reason.map(exception_reason_from_api),
        exception_note: optional_note(body.exception_note)?,
        reason: correction_reason_from_api(body.reason),
        note: LaborNote::new(body.note).map_err(validation)?,
    };
    let value = repo::labor::correct_activity(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_activity_adjustment(value)?))
}

fn map_skill(value: LaborSkillReadModel) -> V1Result<LaborSkillResponse> {
    Ok(LaborSkillResponse {
        skill_id: value.skill_id.get(),
        code: value.code,
        name: value.name,
        certification_required: value.certification_required,
        active: value.active,
        revision: revision(value.revision)?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
    })
}

fn map_certification(
    value: EmployeeCertificationReadModel,
) -> V1Result<EmployeeCertificationResponse> {
    Ok(EmployeeCertificationResponse {
        certification_id: value.certification_id.get(),
        employee_id: value.employee_id.get(),
        employee_name: value.employee_name,
        skill_id: value.skill_id.get(),
        skill_code: value.skill_code,
        facility_id: value.facility_id.get(),
        certification_number: value.certification_number,
        issued_at: value.issued_at.to_rfc3339(),
        expires_at: value.expires_at.map(|time| time.to_rfc3339()),
        revoked_at: value.revoked_at.map(|time| time.to_rfc3339()),
        revision: revision(value.revision)?,
        certified_by: value.certified_by.get(),
        certified_at: value.certified_at.to_rfc3339(),
        note: value.note,
        revoked_by: value.revoked_by.map(|id| id.get()),
        revocation_note: value.revocation_note,
    })
}

fn map_equipment_class(value: EquipmentClassReadModel) -> V1Result<EquipmentClassResponse> {
    Ok(EquipmentClassResponse {
        equipment_class_id: value.equipment_class_id.get(),
        code: value.code,
        name: value.name,
        required_skill_id: value.required_skill_id.map(|id| id.get()),
        active: value.active,
        revision: revision(value.revision)?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
    })
}

fn map_equipment_asset(value: EquipmentAssetReadModel) -> V1Result<EquipmentAssetResponse> {
    Ok(EquipmentAssetResponse {
        equipment_asset_id: value.equipment_asset_id.get(),
        facility_id: value.facility_id.get(),
        equipment_class_id: value.equipment_class_id.get(),
        equipment_class_code: value.equipment_class_code,
        equipment_number: value.equipment_number,
        name: value.name,
        status: equipment_status_to_api(value.status),
        assigned_employee_id: value.assigned_employee_id.map(|id| id.get()),
        revision: revision(value.revision)?,
        status_note: value.status_note,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        status_changed_by: value.status_changed_by.map(|id| id.get()),
        status_changed_at: value.status_changed_at.map(|time| time.to_rfc3339()),
    })
}

fn map_standard(value: LaborStandardReadModel) -> V1Result<LaborStandardResponse> {
    Ok(LaborStandardResponse {
        labor_standard_id: value.labor_standard_id.get(),
        facility_id: value.facility_id.get(),
        inventory_owner_id: value.inventory_owner_id.map(|id| id.get()),
        code: value.code,
        name: value.name,
        activity_kind: activity_kind_to_api(value.activity_kind),
        quantity_basis: quantity_basis_to_api(value.quantity_basis),
        setup_seconds: value.setup_seconds,
        seconds_per_unit: value.seconds_per_unit,
        required_skill_id: value.required_skill_id.map(|id| id.get()),
        required_equipment_class_id: value.required_equipment_class_id.map(|id| id.get()),
        effective_from: value.effective_from.to_rfc3339(),
        effective_until: value.effective_until.map(|time| time.to_rfc3339()),
        revision: revision(value.revision)?,
        supersedes_standard_id: value.supersedes_standard_id.map(|id| id.get()),
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        retired_by: value.retired_by.map(|id| id.get()),
        retired_at: value.retired_at.map(|time| time.to_rfc3339()),
    })
}

fn map_attendance(value: AttendanceIntervalReadModel) -> V1Result<AttendanceIntervalResponse> {
    Ok(AttendanceIntervalResponse {
        attendance_interval_id: value.attendance_interval_id.get(),
        employee_id: value.employee_id.get(),
        employee_name: value.employee_name,
        facility_id: value.facility_id.get(),
        status: attendance_status_to_api(value.status),
        revision: revision(value.revision)?,
        clocked_in_at: value.clocked_in_at.to_rfc3339(),
        clocked_out_at: value.clocked_out_at.map(|time| time.to_rfc3339()),
        paid_seconds: value.paid_seconds,
        clocked_in_by: value.clocked_in_by.get(),
        clocked_out_by: value.clocked_out_by.map(|id| id.get()),
        clock_in_note: value.clock_in_note,
        clock_out_note: value.clock_out_note,
        effective_revision: revision(value.effective_revision)?,
        effective_clocked_in_at: value.effective_clocked_in_at.to_rfc3339(),
        effective_clocked_out_at: value.effective_clocked_out_at.map(|time| time.to_rfc3339()),
        effective_paid_seconds: value.effective_paid_seconds,
    })
}

fn map_activity(value: LaborActivityReadModel) -> V1Result<LaborActivityResponse> {
    Ok(LaborActivityResponse {
        labor_activity_id: value.labor_activity_id.get(),
        attendance_interval_id: value.attendance_interval_id.get(),
        employee_id: value.employee_id.get(),
        employee_name: value.employee_name,
        facility_id: value.facility_id.get(),
        inventory_owner_id: value.inventory_owner_id.map(|id| id.get()),
        activity_kind: activity_kind_to_api(value.activity_kind),
        status: activity_status_to_api(value.status),
        labor_standard_id: value.labor_standard_id.map(|id| id.get()),
        equipment_asset_id: value.equipment_asset_id.map(|id| id.get()),
        required_skill_id: value.required_skill_id.map(|id| id.get()),
        required_skill_certification_id: value.required_skill_certification_id.map(|id| id.get()),
        required_equipment_class_id: value.required_equipment_class_id.map(|id| id.get()),
        equipment_required_skill_id: value.equipment_required_skill_id.map(|id| id.get()),
        equipment_skill_certification_id: value.equipment_skill_certification_id.map(|id| id.get()),
        standard_setup_seconds: value.standard_setup_seconds,
        standard_seconds_per_unit: value.standard_seconds_per_unit,
        quantity_basis: value.quantity_basis.map(quantity_basis_to_api),
        reference_type: value.reference_type,
        reference_id: value.reference_id,
        reference_quantity: value.reference_quantity,
        revision: revision(value.revision)?,
        started_at: value.started_at.to_rfc3339(),
        completed_at: value.completed_at.map(|time| time.to_rfc3339()),
        actual_seconds: value.actual_seconds,
        exception_seconds: value.exception_seconds,
        exception_reason: value.exception_reason.map(exception_reason_to_api),
        exception_note: value.exception_note,
        exception_approved_by: value.exception_approved_by.map(|id| id.get()),
        quantity: value.quantity,
        expected_seconds: value.expected_seconds,
        efficiency_basis_points: value.efficiency_basis_points,
        started_by: value.started_by.get(),
        completed_by: value.completed_by.map(|id| id.get()),
        cancelled_by: value.cancelled_by.map(|id| id.get()),
        note: value.note,
        effective_revision: revision(value.effective_revision)?,
        effective_started_at: value.effective_started_at.to_rfc3339(),
        effective_completed_at: value.effective_completed_at.map(|time| time.to_rfc3339()),
        effective_actual_seconds: value.effective_actual_seconds,
        effective_exception_seconds: value.effective_exception_seconds,
        effective_exception_reason: value
            .effective_exception_reason
            .map(exception_reason_to_api),
        effective_exception_note: value.effective_exception_note,
        effective_exception_approved_by: value.effective_exception_approved_by.map(|id| id.get()),
        effective_quantity: value.effective_quantity,
        effective_expected_seconds: value.effective_expected_seconds,
        effective_efficiency_basis_points: value.effective_efficiency_basis_points,
    })
}

fn map_attendance_adjustment(
    value: AttendanceAdjustmentReadModel,
) -> V1Result<AttendanceAdjustmentResponse> {
    Ok(AttendanceAdjustmentResponse {
        attendance_adjustment_id: value.attendance_adjustment_id.get(),
        attendance_interval_id: value.attendance_interval_id.get(),
        employee_id: value.employee_id.get(),
        employee_name: value.employee_name,
        facility_id: value.facility_id.get(),
        supersedes_adjustment_id: value.supersedes_adjustment_id.map(|id| id.get()),
        expected_revision: revision(value.expected_revision)?,
        resulting_revision: revision(value.resulting_revision)?,
        before_clocked_in_at: value.before_clocked_in_at.to_rfc3339(),
        before_clocked_out_at: value.before_clocked_out_at.to_rfc3339(),
        before_paid_seconds: value.before_paid_seconds,
        corrected_clocked_in_at: value.corrected_clocked_in_at.to_rfc3339(),
        corrected_clocked_out_at: value.corrected_clocked_out_at.to_rfc3339(),
        corrected_paid_seconds: value.corrected_paid_seconds,
        reason: correction_reason_to_api(value.reason),
        note: value.note,
        adjusted_by: value.adjusted_by.get(),
        adjusted_at: value.adjusted_at.to_rfc3339(),
    })
}

fn map_activity_adjustment(
    value: LaborActivityAdjustmentReadModel,
) -> V1Result<LaborActivityAdjustmentResponse> {
    Ok(LaborActivityAdjustmentResponse {
        labor_activity_adjustment_id: value.labor_activity_adjustment_id.get(),
        labor_activity_id: value.labor_activity_id.get(),
        employee_id: value.employee_id.get(),
        employee_name: value.employee_name,
        facility_id: value.facility_id.get(),
        inventory_owner_id: value.inventory_owner_id.map(|id| id.get()),
        supersedes_adjustment_id: value.supersedes_adjustment_id.map(|id| id.get()),
        expected_revision: revision(value.expected_revision)?,
        resulting_revision: revision(value.resulting_revision)?,
        before_started_at: value.before_started_at.to_rfc3339(),
        corrected_started_at: value.corrected_started_at.to_rfc3339(),
        before_completed_at: value.before_completed_at.to_rfc3339(),
        corrected_completed_at: value.corrected_completed_at.to_rfc3339(),
        before_actual_seconds: value.before_actual_seconds,
        corrected_actual_seconds: value.corrected_actual_seconds,
        before_quantity: value.before_quantity,
        corrected_quantity: value.corrected_quantity,
        before_exception_seconds: value.before_exception_seconds,
        corrected_exception_seconds: value.corrected_exception_seconds,
        before_exception_reason: value.before_exception_reason.map(exception_reason_to_api),
        corrected_exception_reason: value
            .corrected_exception_reason
            .map(exception_reason_to_api),
        before_exception_note: value.before_exception_note,
        corrected_exception_note: value.corrected_exception_note,
        before_exception_approved_by: value.before_exception_approved_by.map(|id| id.get()),
        corrected_exception_approved_by: value.corrected_exception_approved_by.map(|id| id.get()),
        before_expected_seconds: value.before_expected_seconds,
        corrected_expected_seconds: value.corrected_expected_seconds,
        before_efficiency_basis_points: value.before_efficiency_basis_points,
        corrected_efficiency_basis_points: value.corrected_efficiency_basis_points,
        reason: correction_reason_to_api(value.reason),
        note: value.note,
        adjusted_by: value.adjusted_by.get(),
        adjusted_at: value.adjusted_at.to_rfc3339(),
    })
}

fn map_summary(value: EmployeeLaborSummary) -> EmployeeLaborSummaryResponse {
    EmployeeLaborSummaryResponse {
        employee_id: value.employee_id.get(),
        employee_name: value.employee_name,
        paid_seconds: value.paid_seconds,
        direct_seconds: value.direct_seconds,
        indirect_seconds: value.indirect_seconds,
        exception_seconds: value.exception_seconds,
        expected_seconds: value.expected_seconds,
        utilization_basis_points: value.utilization_basis_points,
        efficiency_basis_points: value.efficiency_basis_points,
    }
}

const fn attendance_status_to_api(value: AttendanceStatus) -> ApiAttendanceStatus {
    match value {
        AttendanceStatus::Open => ApiAttendanceStatus::Open,
        AttendanceStatus::Closed => ApiAttendanceStatus::Closed,
    }
}

const fn activity_kind_from_api(value: ApiActivityKind) -> LaborActivityKind {
    match value {
        ApiActivityKind::Receiving => LaborActivityKind::Receiving,
        ApiActivityKind::Putaway => LaborActivityKind::Putaway,
        ApiActivityKind::Replenishment => LaborActivityKind::Replenishment,
        ApiActivityKind::Picking => LaborActivityKind::Picking,
        ApiActivityKind::Packing => LaborActivityKind::Packing,
        ApiActivityKind::Shipping => LaborActivityKind::Shipping,
        ApiActivityKind::CycleCount => LaborActivityKind::CycleCount,
        ApiActivityKind::InventoryRelocation => LaborActivityKind::InventoryRelocation,
        ApiActivityKind::CrossDock => LaborActivityKind::CrossDock,
        ApiActivityKind::Yard => LaborActivityKind::Yard,
        ApiActivityKind::CustomerReturn => LaborActivityKind::CustomerReturn,
        ApiActivityKind::VendorReturn => LaborActivityKind::VendorReturn,
        ApiActivityKind::ValueAddedWork => LaborActivityKind::ValueAddedWork,
        ApiActivityKind::Break => LaborActivityKind::Break,
        ApiActivityKind::Meeting => LaborActivityKind::Meeting,
        ApiActivityKind::Training => LaborActivityKind::Training,
        ApiActivityKind::Maintenance => LaborActivityKind::Maintenance,
        ApiActivityKind::Delay => LaborActivityKind::Delay,
        ApiActivityKind::OtherIndirect => LaborActivityKind::OtherIndirect,
    }
}

const fn activity_kind_to_api(value: LaborActivityKind) -> ApiActivityKind {
    match value {
        LaborActivityKind::Receiving => ApiActivityKind::Receiving,
        LaborActivityKind::Putaway => ApiActivityKind::Putaway,
        LaborActivityKind::Replenishment => ApiActivityKind::Replenishment,
        LaborActivityKind::Picking => ApiActivityKind::Picking,
        LaborActivityKind::Packing => ApiActivityKind::Packing,
        LaborActivityKind::Shipping => ApiActivityKind::Shipping,
        LaborActivityKind::CycleCount => ApiActivityKind::CycleCount,
        LaborActivityKind::InventoryRelocation => ApiActivityKind::InventoryRelocation,
        LaborActivityKind::CrossDock => ApiActivityKind::CrossDock,
        LaborActivityKind::Yard => ApiActivityKind::Yard,
        LaborActivityKind::CustomerReturn => ApiActivityKind::CustomerReturn,
        LaborActivityKind::VendorReturn => ApiActivityKind::VendorReturn,
        LaborActivityKind::ValueAddedWork => ApiActivityKind::ValueAddedWork,
        LaborActivityKind::Break => ApiActivityKind::Break,
        LaborActivityKind::Meeting => ApiActivityKind::Meeting,
        LaborActivityKind::Training => ApiActivityKind::Training,
        LaborActivityKind::Maintenance => ApiActivityKind::Maintenance,
        LaborActivityKind::Delay => ApiActivityKind::Delay,
        LaborActivityKind::OtherIndirect => ApiActivityKind::OtherIndirect,
    }
}

const fn activity_status_to_api(value: LaborActivityStatus) -> ApiActivityStatus {
    match value {
        LaborActivityStatus::Active => ApiActivityStatus::Active,
        LaborActivityStatus::Completed => ApiActivityStatus::Completed,
        LaborActivityStatus::Cancelled => ApiActivityStatus::Cancelled,
    }
}

const fn equipment_status_from_api(value: ApiEquipmentStatus) -> EquipmentStatus {
    match value {
        ApiEquipmentStatus::Available => EquipmentStatus::Available,
        ApiEquipmentStatus::Assigned => EquipmentStatus::Assigned,
        ApiEquipmentStatus::OutOfService => EquipmentStatus::OutOfService,
        ApiEquipmentStatus::Retired => EquipmentStatus::Retired,
    }
}

const fn equipment_status_to_api(value: EquipmentStatus) -> ApiEquipmentStatus {
    match value {
        EquipmentStatus::Available => ApiEquipmentStatus::Available,
        EquipmentStatus::Assigned => ApiEquipmentStatus::Assigned,
        EquipmentStatus::OutOfService => ApiEquipmentStatus::OutOfService,
        EquipmentStatus::Retired => ApiEquipmentStatus::Retired,
    }
}

const fn quantity_basis_from_api(value: ApiQuantityBasis) -> LaborQuantityBasis {
    match value {
        ApiQuantityBasis::Unit => LaborQuantityBasis::Unit,
        ApiQuantityBasis::Line => LaborQuantityBasis::Line,
        ApiQuantityBasis::Container => LaborQuantityBasis::Container,
        ApiQuantityBasis::Task => LaborQuantityBasis::Task,
        ApiQuantityBasis::WeightGram => LaborQuantityBasis::WeightGram,
    }
}

const fn quantity_basis_to_api(value: LaborQuantityBasis) -> ApiQuantityBasis {
    match value {
        LaborQuantityBasis::Unit => ApiQuantityBasis::Unit,
        LaborQuantityBasis::Line => ApiQuantityBasis::Line,
        LaborQuantityBasis::Container => ApiQuantityBasis::Container,
        LaborQuantityBasis::Task => ApiQuantityBasis::Task,
        LaborQuantityBasis::WeightGram => ApiQuantityBasis::WeightGram,
    }
}

const fn exception_reason_from_api(value: ApiExceptionReason) -> LaborExceptionReason {
    match value {
        ApiExceptionReason::Equipment => LaborExceptionReason::Equipment,
        ApiExceptionReason::Congestion => LaborExceptionReason::Congestion,
        ApiExceptionReason::Inventory => LaborExceptionReason::Inventory,
        ApiExceptionReason::Quality => LaborExceptionReason::Quality,
        ApiExceptionReason::Safety => LaborExceptionReason::Safety,
        ApiExceptionReason::System => LaborExceptionReason::System,
        ApiExceptionReason::Training => LaborExceptionReason::Training,
        ApiExceptionReason::Personal => LaborExceptionReason::Personal,
        ApiExceptionReason::Other => LaborExceptionReason::Other,
    }
}

const fn exception_reason_to_api(value: LaborExceptionReason) -> ApiExceptionReason {
    match value {
        LaborExceptionReason::Equipment => ApiExceptionReason::Equipment,
        LaborExceptionReason::Congestion => ApiExceptionReason::Congestion,
        LaborExceptionReason::Inventory => ApiExceptionReason::Inventory,
        LaborExceptionReason::Quality => ApiExceptionReason::Quality,
        LaborExceptionReason::Safety => ApiExceptionReason::Safety,
        LaborExceptionReason::System => ApiExceptionReason::System,
        LaborExceptionReason::Training => ApiExceptionReason::Training,
        LaborExceptionReason::Personal => ApiExceptionReason::Personal,
        LaborExceptionReason::Other => ApiExceptionReason::Other,
    }
}

const fn correction_reason_from_api(value: ApiCorrectionReason) -> LaborCorrectionReason {
    match value {
        ApiCorrectionReason::MissedPunch => LaborCorrectionReason::MissedPunch,
        ApiCorrectionReason::TimekeepingError => LaborCorrectionReason::TimekeepingError,
        ApiCorrectionReason::QuantityError => LaborCorrectionReason::QuantityError,
        ApiCorrectionReason::ExceptionError => LaborCorrectionReason::ExceptionError,
        ApiCorrectionReason::SystemError => LaborCorrectionReason::SystemError,
        ApiCorrectionReason::Other => LaborCorrectionReason::Other,
    }
}

const fn correction_reason_to_api(value: LaborCorrectionReason) -> ApiCorrectionReason {
    match value {
        LaborCorrectionReason::MissedPunch => ApiCorrectionReason::MissedPunch,
        LaborCorrectionReason::TimekeepingError => ApiCorrectionReason::TimekeepingError,
        LaborCorrectionReason::QuantityError => ApiCorrectionReason::QuantityError,
        LaborCorrectionReason::ExceptionError => ApiCorrectionReason::ExceptionError,
        LaborCorrectionReason::SystemError => ApiCorrectionReason::SystemError,
        LaborCorrectionReason::Other => ApiCorrectionReason::Other,
    }
}

fn validate_activity_shape(
    kind: LaborActivityKind,
    inventory_owner_id: Option<i64>,
    quantity_basis: Option<ApiQuantityBasis>,
    labor_standard_id: Option<i64>,
    reference_type: Option<&str>,
    reference_id: Option<i64>,
) -> V1Result<()> {
    let has_reference = reference_type.is_some() && reference_id.is_some();
    let has_partial_reference = reference_type.is_some() != reference_id.is_some();
    if has_partial_reference {
        return Err(validation(
            "reference_type and reference_id must be provided together",
        ));
    }
    if reference_id.is_some_and(|id| id <= 0) {
        return Err(validation("reference_id must be a positive integer"));
    }
    if kind.is_direct() {
        if inventory_owner_id.is_none() && kind != LaborActivityKind::CycleCount {
            return Err(validation("direct labor requires inventory_owner_id"));
        }
        if quantity_basis.is_none() {
            return Err(validation("direct labor requires quantity_basis"));
        }
        if !has_reference {
            return Err(validation(
                "direct labor requires reference_type and reference_id",
            ));
        }
        let expected_reference_type = expected_reference_type(kind)
            .ok_or_else(|| V1Error::internal("direct labor kind has no reference contract"))?;
        if reference_type != Some(expected_reference_type) {
            return Err(validation(format!(
                "{} labor requires reference_type {expected_reference_type}",
                kind.as_str()
            )));
        }
    } else if inventory_owner_id.is_some()
        || quantity_basis.is_some()
        || labor_standard_id.is_some()
        || has_reference
    {
        return Err(validation(
            "indirect labor forbids inventory owner, quantity basis, standard, and business reference",
        ));
    }
    Ok(())
}

const fn expected_reference_type(kind: LaborActivityKind) -> Option<&'static str> {
    match kind {
        LaborActivityKind::Receiving => Some("inbound_load"),
        LaborActivityKind::Putaway
        | LaborActivityKind::Replenishment
        | LaborActivityKind::CycleCount
        | LaborActivityKind::InventoryRelocation
        | LaborActivityKind::CrossDock => Some("work_task"),
        LaborActivityKind::Picking => Some("pick_task"),
        LaborActivityKind::Packing => Some("packing_session"),
        LaborActivityKind::Shipping => Some("shipment"),
        LaborActivityKind::Yard => Some("yard_visit"),
        LaborActivityKind::CustomerReturn => Some("customer_return"),
        LaborActivityKind::VendorReturn => Some("vendor_return"),
        LaborActivityKind::ValueAddedWork => Some("value_added_work_order"),
        LaborActivityKind::Break
        | LaborActivityKind::Meeting
        | LaborActivityKind::Training
        | LaborActivityKind::Maintenance
        | LaborActivityKind::Delay
        | LaborActivityKind::OtherIndirect => None,
    }
}

fn validate_workspace_window(from: Timestamp, until: Timestamp) -> V1Result<()> {
    let duration = until.signed_duration_since(from);
    if duration <= Duration::zero() {
        return Err(validation("until must be after from"));
    }
    if duration > Duration::days(MAX_WORKSPACE_DAYS) {
        return Err(validation(format!(
            "labor workspace interval cannot exceed {MAX_WORKSPACE_DAYS} days"
        )));
    }
    Ok(())
}

fn optional_note(value: Option<String>) -> V1Result<Option<LaborNote>> {
    value.map(LaborNote::new).transpose().map_err(validation)
}

fn labor_revision(value: Revision) -> V1Result<LaborRevision> {
    LaborRevision::new(value.get()).map_err(validation)
}

fn revision(value: LaborRevision) -> V1Result<Revision> {
    Revision::new(value.get()).map_err(invalid_result)
}

fn parse_timestamp(value: &str, field: &str) -> V1Result<Timestamp> {
    value
        .parse::<Timestamp>()
        .map_err(|error| AppError::bad_request(format!("{field} is invalid: {error}")).into())
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_direct_activity_has_an_explicit_reference_contract() {
        for kind in [
            LaborActivityKind::Receiving,
            LaborActivityKind::Putaway,
            LaborActivityKind::Replenishment,
            LaborActivityKind::Picking,
            LaborActivityKind::Packing,
            LaborActivityKind::Shipping,
            LaborActivityKind::CycleCount,
            LaborActivityKind::InventoryRelocation,
            LaborActivityKind::CrossDock,
            LaborActivityKind::Yard,
            LaborActivityKind::CustomerReturn,
            LaborActivityKind::VendorReturn,
            LaborActivityKind::ValueAddedWork,
        ] {
            assert!(kind.is_direct());
            assert!(expected_reference_type(kind).is_some());
        }
    }

    #[test]
    fn facility_shared_cycle_count_is_a_valid_direct_shape() {
        assert!(validate_activity_shape(
            LaborActivityKind::CycleCount,
            None,
            Some(ApiQuantityBasis::Task),
            Some(7),
            Some("work_task"),
            Some(11),
        )
        .is_ok());
    }

    #[test]
    fn indirect_activity_rejects_owner_and_quantity_dimensions() {
        assert!(validate_activity_shape(
            LaborActivityKind::Break,
            Some(3),
            Some(ApiQuantityBasis::Unit),
            None,
            None,
            None,
        )
        .is_err());
    }
}
