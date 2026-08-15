use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::labor::{
    AttendanceAdjustmentReadModel, CorrectAttendanceCommand, CorrectLaborActivityCommand,
    LaborActivityAdjustmentReadModel, CORRECT_ATTENDANCE_OPERATION,
    CORRECT_LABOR_ACTIVITY_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    efficiency_basis_points, validate_activity_correction, validate_attendance_correction,
    AttendanceAdjustmentId, AttendanceStatus, FacilityId, InventoryOwnerId,
    LaborActivityAdjustmentId, LaborActivityStatus, LaborRevision, LaborStandard,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::models::{
    read_activity_adjustment_tx, read_activity_tx, read_attendance_adjustment_tx,
    read_attendance_tx,
};
use super::{
    enqueue_event_tx, internal, lock_key_tx, require_access_actor, require_scope, LaborOutboxEvent,
    SUPERVISE_PERMISSION,
};
use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn correct_attendance(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CorrectAttendanceCommand,
) -> AppResult<AttendanceAdjustmentReadModel> {
    if !command.reason.supports_attendance() {
        return Err(AppError::bad_request(
            "correction reason is not valid for attendance",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, CORRECT_ATTENDANCE_OPERATION, command)?;
    require_access_actor(access, context)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISE_PERMISSION,
    )
    .await?;

    let authorization =
        sqlx::query("SELECT facility_id FROM attendance_intervals WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(command.attendance_interval_id.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::not_found("attendance interval"))?;
    let facility_id = FacilityId::new(authorization.try_get("facility_id")?).map_err(internal)?;
    require_scope(&scope, facility_id, None)?;

    if let Some(result) = prepared
        .replayed::<AttendanceAdjustmentReadModel>(&mut tx)
        .await?
    {
        require_scope(&scope, result.facility_id, None)?;
        tx.commit().await?;
        return Ok(result);
    }

    lock_key_tx(
        &mut tx,
        &format!(
            "attendance_adjustment:{}:{}",
            access.tenant_id, command.attendance_interval_id
        ),
    )
    .await?;
    let current =
        read_attendance_tx(&mut tx, access.tenant_id, command.attendance_interval_id).await?;
    if current.status != AttendanceStatus::Closed {
        return Err(AppError::conflict(
            "attendance correction requires a closed interval",
        ));
    }
    if current.effective_revision != command.expected_revision {
        return Err(AppError::conflict(
            "attendance correction revision is stale",
        ));
    }
    let paid_seconds = validate_attendance_correction(
        current.status,
        command.corrected_clocked_in_at,
        command.corrected_clocked_out_at,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))?;
    let before_clocked_out_at = current
        .effective_clocked_out_at
        .ok_or_else(|| AppError::internal("closed attendance lacks an effective clock-out"))?;
    let before_paid_seconds = current
        .effective_paid_seconds
        .ok_or_else(|| AppError::internal("closed attendance lacks effective paid seconds"))?;
    if current.effective_clocked_in_at == command.corrected_clocked_in_at
        && before_clocked_out_at == command.corrected_clocked_out_at
    {
        return Err(AppError::bad_request(
            "attendance correction must change a timestamp",
        ));
    }
    let supersedes_adjustment_id: Option<i64> = sqlx::query_scalar(
        r#"SELECT id FROM attendance_adjustments
           WHERE tenant_id=$1 AND attendance_interval_id=$2
           ORDER BY resulting_revision DESC,id DESC LIMIT 1"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.attendance_interval_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    let resulting_revision = command
        .expected_revision
        .next()
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let now = now_iso();
    let adjustment_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO attendance_adjustments
          (tenant_id,attendance_interval_id,supersedes_adjustment_id,expected_revision,
           resulting_revision,before_clocked_in_at,before_clocked_out_at,before_paid_seconds,
           corrected_clocked_in_at,corrected_clocked_out_at,corrected_paid_seconds,
           correction_reason,correction_note,adjusted_by_user_id,adjusted_at)
          VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.attendance_interval_id.get())
    .bind(supersedes_adjustment_id)
    .bind(command.expected_revision.get())
    .bind(resulting_revision.get())
    .bind(current.effective_clocked_in_at)
    .bind(before_clocked_out_at)
    .bind(before_paid_seconds)
    .bind(command.corrected_clocked_in_at)
    .bind(command.corrected_clocked_out_at)
    .bind(paid_seconds)
    .bind(command.reason.as_str())
    .bind(command.note.as_str())
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_attendance_adjustment_tx(
        &mut tx,
        access.tenant_id,
        AttendanceAdjustmentId::new(adjustment_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(facility_id),
            owner_id: None,
            aggregate_type: "attendance",
            aggregate_id: command.attendance_interval_id.get(),
            transition: "corrected",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn correct_activity(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CorrectLaborActivityCommand,
) -> AppResult<LaborActivityAdjustmentReadModel> {
    if !command.reason.supports_activity() {
        return Err(AppError::bad_request(
            "correction reason is not valid for labor activity",
        ));
    }
    if (command.exception_seconds == 0)
        != (command.exception_reason.is_none() && command.exception_note.is_none())
    {
        return Err(AppError::bad_request(
            "nonzero exception seconds require reason and note; zero forbids them",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, CORRECT_LABOR_ACTIVITY_OPERATION, command)?;
    require_access_actor(access, context)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISE_PERMISSION,
    )
    .await?;

    let authorization = sqlx::query(
        r#"SELECT facility_id,inventory_owner_id FROM labor_activities
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.labor_activity_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("labor activity"))?;
    let facility_id = FacilityId::new(authorization.try_get("facility_id")?).map_err(internal)?;
    let owner_id = authorization
        .try_get::<Option<i64>, _>("inventory_owner_id")?
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(internal)?;
    require_scope(&scope, facility_id, owner_id)?;
    if let Some(result) = prepared
        .replayed::<LaborActivityAdjustmentReadModel>(&mut tx)
        .await?
    {
        require_scope(&scope, result.facility_id, result.inventory_owner_id)?;
        tx.commit().await?;
        return Ok(result);
    }

    lock_key_tx(
        &mut tx,
        &format!(
            "labor_activity_adjustment:{}:{}",
            access.tenant_id, command.labor_activity_id
        ),
    )
    .await?;
    let current = read_activity_tx(&mut tx, access.tenant_id, command.labor_activity_id).await?;
    if current.status != LaborActivityStatus::Completed {
        return Err(AppError::conflict(
            "labor correction requires a completed activity",
        ));
    }
    if current.effective_revision != command.expected_revision {
        return Err(AppError::conflict("labor correction revision is stale"));
    }
    if let (Some(reference_type), Some(reference_id)) =
        (current.reference_type.as_deref(), current.reference_id)
    {
        lock_key_tx(
            &mut tx,
            &format!(
                "labor_reference:{}:{reference_type}:{reference_id}",
                access.tenant_id
            ),
        )
        .await?;
    }

    let before_started_at = current.effective_started_at;
    let before_completed_at = current
        .effective_completed_at
        .ok_or_else(|| AppError::internal("completed labor lacks an effective completion time"))?;
    let before_actual_seconds = current
        .effective_actual_seconds
        .ok_or_else(|| AppError::internal("completed labor lacks effective actual seconds"))?;
    let before_exception_seconds = current
        .effective_exception_seconds
        .ok_or_else(|| AppError::internal("completed labor lacks effective exception seconds"))?;
    let corrected_started_at = command.corrected_started_at.unwrap_or(before_started_at);
    let corrected_completed_at = command
        .corrected_completed_at
        .unwrap_or(before_completed_at);
    let corrected_actual_seconds = corrected_completed_at
        .signed_duration_since(corrected_started_at)
        .num_seconds();
    validate_activity_correction(
        current.status,
        current.activity_kind,
        corrected_actual_seconds,
        command.quantity,
        command.exception_seconds,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))?;
    let corrected_expected_seconds = match (
        current.standard_setup_seconds,
        current.standard_seconds_per_unit,
        command.quantity,
    ) {
        (Some(setup_seconds), Some(seconds_per_unit), Some(quantity)) => Some(
            LaborStandard::new(setup_seconds, seconds_per_unit)
                .map_err(|error| AppError::internal(error.to_string()))?
                .expected_seconds(quantity)
                .map_err(|error| AppError::bad_request(error.to_string()))?,
        ),
        (None, None, _) => None,
        _ => return Err(AppError::internal("labor standard snapshot is incomplete")),
    };
    let corrected_efficiency = corrected_expected_seconds
        .map(|expected| efficiency_basis_points(expected, corrected_actual_seconds))
        .transpose()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let corrected_exception_approved_by =
        (command.exception_seconds > 0).then_some(context.actor_id);

    if corrected_started_at == before_started_at
        && corrected_completed_at == before_completed_at
        && command.quantity.map(|quantity| quantity.get()) == current.effective_quantity
        && command.exception_seconds == before_exception_seconds
        && command.exception_reason == current.effective_exception_reason
        && command.exception_note.as_ref().map(|note| note.as_str())
            == current.effective_exception_note.as_deref()
    {
        return Err(AppError::bad_request(
            "labor correction must change time, quantity, or exception evidence",
        ));
    }

    if let Some(reference_quantity) = current.reference_quantity {
        let total_effective: i64 = sqlx::query_scalar(
            r#"SELECT COALESCE(SUM(CASE WHEN activity.id=$2 THEN $9
                     ELSE CASE WHEN correction.id IS NULL THEN activity.completed_quantity
                       ELSE correction.corrected_quantity END END),0)::BIGINT
               FROM labor_activities activity
               LEFT JOIN LATERAL (
                 SELECT adjustment.id,adjustment.corrected_quantity
                 FROM labor_activity_adjustments adjustment
                 WHERE adjustment.tenant_id=activity.tenant_id
                   AND adjustment.labor_activity_id=activity.id
                 ORDER BY adjustment.resulting_revision DESC,adjustment.id DESC LIMIT 1
               ) correction ON true
               WHERE activity.tenant_id=$1 AND activity.status='completed'
                 AND activity.facility_id=$3
                 AND activity.inventory_owner_id IS NOT DISTINCT FROM $4
                 AND activity.activity_kind=$5 AND activity.quantity_basis=$6
                 AND activity.reference_type=$7 AND activity.reference_id=$8"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.labor_activity_id.get())
        .bind(current.facility_id.get())
        .bind(current.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(current.activity_kind.as_str())
        .bind(current.quantity_basis.map(|basis| basis.as_str()))
        .bind(current.reference_type.as_deref())
        .bind(current.reference_id)
        .bind(command.quantity.map(|quantity| quantity.get()))
        .fetch_one(&mut *tx)
        .await?;
        if command
            .quantity
            .is_some_and(|quantity| quantity.get() > reference_quantity)
            || total_effective > reference_quantity
        {
            return Err(AppError::conflict(
                "corrected labor quantity exceeds canonical work evidence",
            ));
        }
    }

    let supersedes_adjustment_id: Option<i64> = sqlx::query_scalar(
        r#"SELECT id FROM labor_activity_adjustments
           WHERE tenant_id=$1 AND labor_activity_id=$2
           ORDER BY resulting_revision DESC,id DESC LIMIT 1"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.labor_activity_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    let resulting_revision: LaborRevision = command
        .expected_revision
        .next()
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let now = now_iso();
    let adjustment_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_activity_adjustments
          (tenant_id,labor_activity_id,supersedes_adjustment_id,expected_revision,
           resulting_revision,before_started_at,corrected_started_at,before_completed_at,
           corrected_completed_at,before_actual_seconds,corrected_actual_seconds,
           before_quantity,corrected_quantity,before_exception_seconds,
           corrected_exception_seconds,before_exception_reason,corrected_exception_reason,
           before_exception_note,corrected_exception_note,before_exception_approved_by_user_id,
           corrected_exception_approved_by_user_id,before_expected_seconds,
           corrected_expected_seconds,before_efficiency_basis_points,
           corrected_efficiency_basis_points,correction_reason,correction_note,
           adjusted_by_user_id,adjusted_at)
          VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
            $21,$22,$23,$24,$25,$26,$27,$28,$29) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.labor_activity_id.get())
    .bind(supersedes_adjustment_id)
    .bind(command.expected_revision.get())
    .bind(resulting_revision.get())
    .bind(before_started_at)
    .bind(corrected_started_at)
    .bind(before_completed_at)
    .bind(corrected_completed_at)
    .bind(before_actual_seconds)
    .bind(corrected_actual_seconds)
    .bind(current.effective_quantity)
    .bind(command.quantity.map(|quantity| quantity.get()))
    .bind(before_exception_seconds)
    .bind(command.exception_seconds)
    .bind(
        current
            .effective_exception_reason
            .map(|reason| reason.as_str()),
    )
    .bind(command.exception_reason.map(|reason| reason.as_str()))
    .bind(current.effective_exception_note.as_deref())
    .bind(command.exception_note.as_ref().map(|note| note.as_str()))
    .bind(
        current
            .effective_exception_approved_by
            .map(|user| user.get()),
    )
    .bind(corrected_exception_approved_by.map(|user| user.get()))
    .bind(current.effective_expected_seconds)
    .bind(corrected_expected_seconds)
    .bind(current.effective_efficiency_basis_points)
    .bind(corrected_efficiency)
    .bind(command.reason.as_str())
    .bind(command.note.as_str())
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_activity_adjustment_tx(
        &mut tx,
        access.tenant_id,
        LaborActivityAdjustmentId::new(adjustment_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(facility_id),
            owner_id,
            aggregate_type: "activity",
            aggregate_id: command.labor_activity_id.get(),
            transition: "corrected",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
