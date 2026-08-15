use chrono::Duration;
use wareboxes_application::labor::LaborWorkspaceReadModel;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{EmployeeId, FacilityId, InventoryOwnerId, Timestamp};

use super::models::{
    activity, activity_adjustment, attendance, attendance_adjustment, certification,
    employee_summary, equipment_asset, equipment_class, skill, standard,
};
use super::LABOR_VIEW_PERMISSION;
use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

const MAX_WORKSPACE_ROWS: i64 = 1_000;
const WORKSPACE_FETCH_LIMIT: i64 = MAX_WORKSPACE_ROWS + 1;
pub const MAX_LABOR_WORKSPACE_DAYS: i64 = 31;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaborWorkspaceFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub employee_id: Option<EmployeeId>,
    pub from: Timestamp,
    pub until: Timestamp,
    pub include_history: bool,
}

fn validate_filter(filter: &LaborWorkspaceFilter) -> AppResult<()> {
    let duration = filter.until.signed_duration_since(filter.from);
    if duration <= Duration::zero() {
        return Err(AppError::bad_request(
            "labor workspace until must be after from",
        ));
    }
    if duration > Duration::days(MAX_LABOR_WORKSPACE_DAYS) {
        return Err(AppError::bad_request(format!(
            "labor workspace interval cannot exceed {MAX_LABOR_WORKSPACE_DAYS} days"
        )));
    }
    Ok(())
}

fn reject_truncated(collection: &str, row_count: usize) -> AppResult<()> {
    if row_count > MAX_WORKSPACE_ROWS as usize {
        Err(AppError::conflict(format!(
            "labor workspace {collection} result exceeds {MAX_WORKSPACE_ROWS} rows; narrow the filters"
        )))
    } else {
        Ok(())
    }
}

pub async fn workspace(
    db: &Db,
    access: &TenantAccess,
    filter: &LaborWorkspaceFilter,
) -> AppResult<LaborWorkspaceReadModel> {
    validate_filter(filter)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        LABOR_VIEW_PERMISSION,
    )
    .await?;

    if filter
        .facility_id
        .is_some_and(|facility_id| !scope.includes_facility(facility_id.get()))
    {
        return Err(AppError::not_found("labor workspace scope"));
    }
    if filter
        .inventory_owner_id
        .is_some_and(|owner_id| !scope.includes_inventory_owner(owner_id.get()))
    {
        return Err(AppError::not_found("labor workspace scope"));
    }
    if let Some(employee_id) = filter.employee_id {
        let accessible = sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS(
                 SELECT 1 FROM employees employee
                 WHERE employee.tenant_id=$1 AND employee.id=$2
                   AND ((employee.deleted IS NULL
                     AND employee.hired<=statement_timestamp()
                     AND (employee.terminated IS NULL
                       OR employee.terminated>statement_timestamp())
                     AND EXISTS(
                       SELECT 1 FROM employee_facilities assignment
                       WHERE assignment.tenant_id=employee.tenant_id
                         AND assignment.employee_id=employee.id
                         AND assignment.deleted IS NULL
                         AND ($3 OR assignment.facility_id=ANY($4))
                         AND ($5::BIGINT IS NULL OR assignment.facility_id=$5)
                     )) OR ($9 AND (
                       EXISTS(SELECT 1 FROM employee_certifications certification
                         WHERE certification.tenant_id=employee.tenant_id
                           AND certification.employee_id=employee.id
                           AND ($3 OR certification.facility_id=ANY($4))
                           AND ($5::BIGINT IS NULL OR certification.facility_id=$5))
                       OR EXISTS(SELECT 1 FROM attendance_intervals attendance
                         WHERE attendance.tenant_id=employee.tenant_id
                           AND attendance.employee_id=employee.id
                           AND ($3 OR attendance.facility_id=ANY($4))
                           AND ($5::BIGINT IS NULL OR attendance.facility_id=$5))
                       OR EXISTS(SELECT 1 FROM labor_activities activity
                         WHERE activity.tenant_id=employee.tenant_id
                           AND activity.employee_id=employee.id
                           AND ($3 OR activity.facility_id=ANY($4))
                           AND ($5::BIGINT IS NULL OR activity.facility_id=$5)
                           AND ($6 OR activity.inventory_owner_id IS NULL
                             OR activity.inventory_owner_id=ANY($7))
                           AND ($8::BIGINT IS NULL OR activity.inventory_owner_id IS NULL
                             OR activity.inventory_owner_id=$8))
                     )))
               )"#,
        )
        .bind(access.tenant_id.get())
        .bind(employee_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(filter.facility_id.map(FacilityId::get))
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(filter.include_history)
        .fetch_one(&mut *tx)
        .await?;
        if !accessible {
            return Err(AppError::not_found("labor employee"));
        }
    }

    let skill_rows = sqlx::query(
        r#"SELECT * FROM labor_skills skill
           WHERE skill.tenant_id=$1 AND ($2 OR skill.active)
           ORDER BY skill.code,skill.id LIMIT $3"#,
    )
    .bind(access.tenant_id.get())
    .bind(filter.include_history)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let certification_rows = sqlx::query(
        r#"SELECT certification.*,
                  employee.first_name || ' ' || employee.last_name AS employee_name,
                  skill.code AS skill_code
           FROM employee_certifications certification
           JOIN employees employee ON employee.tenant_id=certification.tenant_id
             AND employee.id=certification.employee_id
           JOIN labor_skills skill ON skill.tenant_id=certification.tenant_id
             AND skill.id=certification.skill_id
           WHERE certification.tenant_id=$1
             AND ($2 OR certification.facility_id=ANY($3))
             AND ($4::BIGINT IS NULL OR certification.facility_id=$4)
             AND ($5::BIGINT IS NULL OR certification.employee_id=$5)
             AND (($6 AND certification.issued_at<$8
               AND COALESCE(LEAST(certification.expires_at,certification.revoked_at),
                 certification.expires_at,certification.revoked_at,$8)>$7)
               OR (NOT $6 AND employee.deleted IS NULL
               AND employee.hired<=statement_timestamp()
               AND (employee.terminated IS NULL OR employee.terminated>statement_timestamp())
               AND EXISTS(SELECT 1 FROM employee_facilities assignment
                 WHERE assignment.tenant_id=certification.tenant_id
                   AND assignment.employee_id=certification.employee_id
                   AND assignment.facility_id=certification.facility_id
                   AND assignment.deleted IS NULL)
               AND certification.revoked_at IS NULL
               AND certification.issued_at<=statement_timestamp()
               AND (certification.expires_at IS NULL
                 OR certification.expires_at>statement_timestamp())))
           ORDER BY employee.last_name,employee.first_name,skill.code,certification.id DESC
           LIMIT $9"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.employee_id.map(EmployeeId::get))
    .bind(filter.include_history)
    .bind(filter.from)
    .bind(filter.until)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let equipment_class_rows = sqlx::query(
        r#"SELECT * FROM equipment_classes class
           WHERE class.tenant_id=$1 AND ($2 OR class.active)
           ORDER BY class.code,class.id LIMIT $3"#,
    )
    .bind(access.tenant_id.get())
    .bind(filter.include_history)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let equipment_asset_rows = sqlx::query(
        r#"SELECT asset.*,class.code AS equipment_class_code
           FROM equipment_assets asset
           JOIN equipment_classes class ON class.tenant_id=asset.tenant_id
             AND class.id=asset.equipment_class_id
           WHERE asset.tenant_id=$1
             AND ($2 OR asset.facility_id=ANY($3))
             AND ($4::BIGINT IS NULL OR asset.facility_id=$4)
             AND ($5 OR asset.status<>'retired')
           ORDER BY asset.facility_id,class.code,asset.equipment_number,asset.id LIMIT $6"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.include_history)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let standard_rows = sqlx::query(
        r#"SELECT * FROM labor_standards standard
           WHERE standard.tenant_id=$1
             AND ($2 OR standard.facility_id=ANY($3))
             AND ($4 OR standard.inventory_owner_id IS NULL
               OR standard.inventory_owner_id=ANY($5))
             AND ($6::BIGINT IS NULL OR standard.facility_id=$6)
             AND ($7::BIGINT IS NULL OR standard.inventory_owner_id IS NULL
               OR standard.inventory_owner_id=$7)
             AND (($8 AND standard.effective_from<$10
               AND COALESCE(standard.effective_until,$10)>$9)
               OR (NOT $8 AND standard.effective_from<=statement_timestamp()
                 AND (standard.effective_until IS NULL
                   OR standard.effective_until>statement_timestamp())))
           ORDER BY standard.facility_id,standard.code,standard.effective_from DESC,standard.id DESC
           LIMIT $11"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(filter.include_history)
    .bind(filter.from)
    .bind(filter.until)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let attendance_rows = sqlx::query(
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
           WHERE attendance.tenant_id=$1
             AND ($2 OR attendance.facility_id=ANY($3))
             AND ($4::BIGINT IS NULL OR attendance.facility_id=$4)
             AND ($5::BIGINT IS NULL OR attendance.employee_id=$5)
             AND COALESCE(correction.corrected_clocked_in_at,attendance.clocked_in_at)<$7
             AND COALESCE(correction.corrected_clocked_out_at,
               attendance.clocked_out_at,statement_timestamp())>$6
             AND ($8 OR (attendance.status='open' AND employee.deleted IS NULL
               AND employee.hired<=statement_timestamp()
               AND (employee.terminated IS NULL OR employee.terminated>statement_timestamp())
               AND EXISTS(SELECT 1 FROM employee_facilities assignment
                 WHERE assignment.tenant_id=attendance.tenant_id
                   AND assignment.employee_id=attendance.employee_id
                   AND assignment.facility_id=attendance.facility_id
                   AND assignment.deleted IS NULL)))
           ORDER BY COALESCE(correction.corrected_clocked_in_at,attendance.clocked_in_at) DESC,
             attendance.id DESC LIMIT $9"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.employee_id.map(EmployeeId::get))
    .bind(filter.from)
    .bind(filter.until)
    .bind(filter.include_history)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let activity_rows = sqlx::query(
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
           WHERE activity.tenant_id=$1
             AND ($2 OR activity.facility_id=ANY($3))
             AND ($4 OR activity.inventory_owner_id IS NULL
               OR activity.inventory_owner_id=ANY($5))
             AND ($6::BIGINT IS NULL OR activity.facility_id=$6)
             AND ($7::BIGINT IS NULL OR activity.inventory_owner_id IS NULL
               OR activity.inventory_owner_id=$7)
             AND ($8::BIGINT IS NULL OR activity.employee_id=$8)
             AND COALESCE(correction.corrected_started_at,activity.started_at)<$10
             AND COALESCE(correction.corrected_completed_at,
               activity.completed_at,statement_timestamp())>$9
             AND ($11 OR (activity.status='active' AND employee.deleted IS NULL
               AND employee.hired<=statement_timestamp()
               AND (employee.terminated IS NULL OR employee.terminated>statement_timestamp())
               AND EXISTS(SELECT 1 FROM employee_facilities assignment
                 WHERE assignment.tenant_id=activity.tenant_id
                   AND assignment.employee_id=activity.employee_id
                   AND assignment.facility_id=activity.facility_id
                   AND assignment.deleted IS NULL)))
           ORDER BY COALESCE(correction.corrected_started_at,activity.started_at) DESC,
             activity.id DESC LIMIT $12"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(filter.employee_id.map(EmployeeId::get))
    .bind(filter.from)
    .bind(filter.until)
    .bind(filter.include_history)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let attendance_adjustment_rows = sqlx::query(
        r#"SELECT adjustment.*,attendance.employee_id,attendance.facility_id,
                  employee.first_name || ' ' || employee.last_name AS employee_name
           FROM attendance_adjustments adjustment
           JOIN attendance_intervals attendance ON attendance.tenant_id=adjustment.tenant_id
             AND attendance.id=adjustment.attendance_interval_id
           JOIN employees employee ON employee.tenant_id=attendance.tenant_id
             AND employee.id=attendance.employee_id
           WHERE adjustment.tenant_id=$1 AND $6
             AND ($2 OR attendance.facility_id=ANY($3))
             AND ($4::BIGINT IS NULL OR attendance.facility_id=$4)
             AND ($5::BIGINT IS NULL OR attendance.employee_id=$5)
             AND LEAST(adjustment.before_clocked_in_at,adjustment.corrected_clocked_in_at)<$8
             AND GREATEST(adjustment.before_clocked_out_at,adjustment.corrected_clocked_out_at)>$7
           ORDER BY adjustment.adjusted_at DESC,adjustment.id DESC LIMIT $9"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.employee_id.map(EmployeeId::get))
    .bind(filter.include_history)
    .bind(filter.from)
    .bind(filter.until)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let activity_adjustment_rows = sqlx::query(
        r#"SELECT adjustment.*,activity.employee_id,activity.facility_id,
                  activity.inventory_owner_id,
                  employee.first_name || ' ' || employee.last_name AS employee_name
           FROM labor_activity_adjustments adjustment
           JOIN labor_activities activity ON activity.tenant_id=adjustment.tenant_id
             AND activity.id=adjustment.labor_activity_id
           JOIN employees employee ON employee.tenant_id=activity.tenant_id
             AND employee.id=activity.employee_id
           WHERE adjustment.tenant_id=$1 AND $9
             AND ($2 OR activity.facility_id=ANY($3))
             AND ($4 OR activity.inventory_owner_id IS NULL
               OR activity.inventory_owner_id=ANY($5))
             AND ($6::BIGINT IS NULL OR activity.facility_id=$6)
             AND ($7::BIGINT IS NULL OR activity.inventory_owner_id IS NULL
               OR activity.inventory_owner_id=$7)
             AND ($8::BIGINT IS NULL OR activity.employee_id=$8)
             AND LEAST(adjustment.before_started_at,adjustment.corrected_started_at)<$11
             AND GREATEST(adjustment.before_completed_at,adjustment.corrected_completed_at)>$10
           ORDER BY adjustment.adjusted_at DESC,adjustment.id DESC LIMIT $12"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(filter.employee_id.map(EmployeeId::get))
    .bind(filter.include_history)
    .bind(filter.from)
    .bind(filter.until)
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    let summary_rows = sqlx::query(
        r#"WITH attendance_durations AS (
             SELECT attendance.employee_id,
                    GREATEST(0,trunc(EXTRACT(EPOCH FROM
                      LEAST(COALESCE(correction.corrected_clocked_out_at,
                        attendance.clocked_out_at,statement_timestamp()),$7)
                      - GREATEST(COALESCE(correction.corrected_clocked_in_at,
                        attendance.clocked_in_at),$6)))::BIGINT) AS duration_seconds
             FROM attendance_intervals attendance
             LEFT JOIN LATERAL (
               SELECT adjustment.corrected_clocked_in_at,adjustment.corrected_clocked_out_at
               FROM attendance_adjustments adjustment
               WHERE adjustment.tenant_id=attendance.tenant_id
                 AND adjustment.attendance_interval_id=attendance.id
               ORDER BY adjustment.resulting_revision DESC,adjustment.id DESC LIMIT 1
             ) correction ON true
             WHERE attendance.tenant_id=$1
               AND $10::BIGINT IS NULL
               AND ($2 OR attendance.facility_id=ANY($3))
               AND ($4::BIGINT IS NULL OR attendance.facility_id=$4)
               AND ($5::BIGINT IS NULL OR attendance.employee_id=$5)
               AND COALESCE(correction.corrected_clocked_in_at,
                 attendance.clocked_in_at)<$7
               AND COALESCE(correction.corrected_clocked_out_at,
                 attendance.clocked_out_at,statement_timestamp())>$6
           ), paid AS (
             SELECT employee_id,COALESCE(SUM(duration_seconds),0)::BIGINT AS paid_seconds
             FROM attendance_durations GROUP BY employee_id
           ), activity_durations AS (
             SELECT activity.employee_id,activity.activity_kind,activity.status,
                    COALESCE(correction.corrected_completed_at,activity.completed_at)
                      AS completed_at,
                    CASE WHEN correction.id IS NULL THEN activity.actual_seconds
                      ELSE correction.corrected_actual_seconds END AS actual_seconds,
                    CASE WHEN correction.id IS NULL THEN activity.exception_seconds
                      ELSE correction.corrected_exception_seconds END AS exception_seconds,
                    CASE WHEN correction.id IS NULL THEN activity.expected_seconds
                      ELSE correction.corrected_expected_seconds END AS expected_seconds,
                    GREATEST(0,trunc(EXTRACT(EPOCH FROM
                      LEAST(COALESCE(correction.corrected_completed_at,
                        activity.completed_at,statement_timestamp()),$7)
                      - GREATEST(COALESCE(correction.corrected_started_at,
                        activity.started_at),$6)))::BIGINT) AS duration_seconds
             FROM labor_activities activity
             LEFT JOIN LATERAL (
               SELECT adjustment.* FROM labor_activity_adjustments adjustment
               WHERE adjustment.tenant_id=activity.tenant_id
                 AND adjustment.labor_activity_id=activity.id
               ORDER BY adjustment.resulting_revision DESC,adjustment.id DESC LIMIT 1
             ) correction ON true
             WHERE activity.tenant_id=$1
               AND ($2 OR activity.facility_id=ANY($3))
               AND ($8 OR activity.inventory_owner_id IS NULL
                 OR activity.inventory_owner_id=ANY($9))
               AND ($4::BIGINT IS NULL OR activity.facility_id=$4)
               AND ($10::BIGINT IS NULL OR activity.inventory_owner_id IS NULL
                 OR activity.inventory_owner_id=$10)
               AND ($5::BIGINT IS NULL OR activity.employee_id=$5)
               AND COALESCE(correction.corrected_started_at,activity.started_at)<$7
               AND COALESCE(correction.corrected_completed_at,
                 activity.completed_at,statement_timestamp())>$6
               AND activity.status<>'cancelled'
           ), labor AS (
             SELECT employee_id,
               COALESCE(SUM(duration_seconds) FILTER (WHERE activity_kind IN(
                 'receiving','putaway','replenishment','picking','packing','shipping',
                 'cycle_count','inventory_relocation','cross_dock','yard','customer_return',
                 'vendor_return','value_added_work')),0)::BIGINT AS direct_seconds,
               COALESCE(SUM(duration_seconds) FILTER (WHERE activity_kind IN(
                 'break','meeting','training','maintenance','delay','other_indirect')),0)::BIGINT
                 AS indirect_seconds,
               COALESCE(SUM(exception_seconds) FILTER (WHERE status='completed'
                 AND completed_at>=$6 AND completed_at<$7),0)::BIGINT
                 AS exception_seconds,
               COALESCE(SUM(expected_seconds) FILTER (WHERE status='completed'
                 AND completed_at>=$6 AND completed_at<$7),0)::BIGINT
                 AS expected_seconds,
               COALESCE(SUM(actual_seconds) FILTER (WHERE status='completed'
                 AND completed_at>=$6 AND completed_at<$7
                 AND expected_seconds IS NOT NULL),0)::BIGINT
                 AS standardized_actual_seconds
             FROM activity_durations GROUP BY employee_id
           ), employees_with_time AS (
             SELECT employee_id FROM paid UNION SELECT employee_id FROM labor
           )
           SELECT timed.employee_id,
                  employee.first_name || ' ' || employee.last_name AS employee_name,
                  COALESCE(paid.paid_seconds,0)::BIGINT AS paid_seconds,
                  COALESCE(labor.direct_seconds,0)::BIGINT AS direct_seconds,
                  COALESCE(labor.indirect_seconds,0)::BIGINT AS indirect_seconds,
                  COALESCE(labor.exception_seconds,0)::BIGINT AS exception_seconds,
                  COALESCE(labor.expected_seconds,0)::BIGINT AS expected_seconds,
                  CASE WHEN COALESCE(paid.paid_seconds,0)>0 THEN
                    (COALESCE(labor.direct_seconds,0)::NUMERIC*10000
                      /paid.paid_seconds)::BIGINT END
                    AS utilization_basis_points,
                  CASE WHEN COALESCE(labor.standardized_actual_seconds,0)>0 THEN
                    LEAST(1000000::NUMERIC,labor.expected_seconds::NUMERIC*10000
                      /labor.standardized_actual_seconds)::BIGINT END
                    AS efficiency_basis_points
           FROM employees_with_time timed
           JOIN employees employee ON employee.tenant_id=$1 AND employee.id=timed.employee_id
           LEFT JOIN paid ON paid.employee_id=timed.employee_id
           LEFT JOIN labor ON labor.employee_id=timed.employee_id
           ORDER BY employee.last_name,employee.first_name,timed.employee_id
           LIMIT $11"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.employee_id.map(EmployeeId::get))
    .bind(filter.from)
    .bind(filter.until)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(WORKSPACE_FETCH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;

    for (collection, row_count) in [
        ("skills", skill_rows.len()),
        ("certifications", certification_rows.len()),
        ("equipment classes", equipment_class_rows.len()),
        ("equipment assets", equipment_asset_rows.len()),
        ("standards", standard_rows.len()),
        ("attendance", attendance_rows.len()),
        ("activities", activity_rows.len()),
        ("attendance adjustments", attendance_adjustment_rows.len()),
        ("activity adjustments", activity_adjustment_rows.len()),
        ("summaries", summary_rows.len()),
    ] {
        reject_truncated(collection, row_count)?;
    }

    let result = LaborWorkspaceReadModel {
        skills: skill_rows
            .iter()
            .map(skill)
            .collect::<AppResult<Vec<_>>>()?,
        certifications: certification_rows
            .iter()
            .map(certification)
            .collect::<AppResult<Vec<_>>>()?,
        equipment_classes: equipment_class_rows
            .iter()
            .map(equipment_class)
            .collect::<AppResult<Vec<_>>>()?,
        equipment_assets: equipment_asset_rows
            .iter()
            .map(equipment_asset)
            .collect::<AppResult<Vec<_>>>()?,
        standards: standard_rows
            .iter()
            .map(standard)
            .collect::<AppResult<Vec<_>>>()?,
        attendance: attendance_rows
            .iter()
            .map(attendance)
            .collect::<AppResult<Vec<_>>>()?,
        activities: activity_rows
            .iter()
            .map(activity)
            .collect::<AppResult<Vec<_>>>()?,
        attendance_adjustments: attendance_adjustment_rows
            .iter()
            .map(attendance_adjustment)
            .collect::<AppResult<Vec<_>>>()?,
        activity_adjustments: activity_adjustment_rows
            .iter()
            .map(activity_adjustment)
            .collect::<AppResult<Vec<_>>>()?,
        summaries: summary_rows
            .iter()
            .map(employee_summary)
            .collect::<AppResult<Vec<_>>>()?,
    };
    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn filter(from: Timestamp, until: Timestamp) -> LaborWorkspaceFilter {
        LaborWorkspaceFilter {
            facility_id: None,
            inventory_owner_id: None,
            employee_id: None,
            from,
            until,
            include_history: false,
        }
    }

    #[test]
    fn workspace_interval_must_be_positive_and_bounded() {
        let from = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        assert!(validate_filter(&filter(from, from + Duration::days(31))).is_ok());
        assert!(validate_filter(&filter(from, from)).is_err());
        assert!(validate_filter(&filter(from, from - Duration::seconds(1))).is_err());
        assert!(validate_filter(&filter(
            from,
            from + Duration::days(31) + Duration::seconds(1)
        ))
        .is_err());
    }

    #[test]
    fn workspace_rejects_only_proven_truncation() {
        assert!(reject_truncated("activities", MAX_WORKSPACE_ROWS as usize).is_ok());
        assert!(reject_truncated("activities", WORKSPACE_FETCH_LIMIT as usize).is_err());
    }
}
