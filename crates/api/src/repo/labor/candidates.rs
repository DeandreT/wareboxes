use sqlx::{postgres::PgRow, Row};
use wareboxes_application::labor::{
    LaborReferenceCandidatePageReadModel, LaborReferenceCandidateReadModel,
    LaborRosterCandidateReadModel, LaborRosterPageReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    AttendanceIntervalId, EmployeeId, FacilityId, InventoryOwnerId, LaborActivityId,
    LaborActivityKind, LaborQuantityBasis, LaborRevision, LaborSkillId,
};

use super::{require_facility, require_owner, EXECUTE_PERMISSION, SUPERVISE_PERMISSION};
use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_any_permission_tx, ScopeBindings};

const CERTIFY_PERMISSION: &str = "labor_certify";
pub const MAX_LABOR_CANDIDATE_PAGE_SIZE: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaborRosterFilter {
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub after: Option<EmployeeId>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaborReferenceCandidateFilter {
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub employee_id: EmployeeId,
    pub activity_kind: LaborActivityKind,
    pub quantity_basis: LaborQuantityBasis,
    pub after: Option<i64>,
    pub limit: u32,
}

fn validate_limit(limit: u32) -> AppResult<()> {
    if (1..=MAX_LABOR_CANDIDATE_PAGE_SIZE).contains(&limit) {
        Ok(())
    } else {
        Err(AppError::bad_request(format!(
            "labor candidate page limit must be between 1 and {MAX_LABOR_CANDIDATE_PAGE_SIZE}"
        )))
    }
}

async fn require_candidate_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    scope: &ScopeBindings,
    facility_id: FacilityId,
    inventory_owner_id: Option<InventoryOwnerId>,
) -> AppResult<()> {
    require_facility(scope, facility_id)?;
    require_owner(scope, inventory_owner_id)?;
    let facility_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM facilities WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL)",
    )
    .bind(tenant_id)
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if !facility_exists {
        return Err(AppError::not_found("labor candidate scope"));
    }
    if let Some(owner_id) = inventory_owner_id {
        let assigned: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM inventory_owner_facilities
               WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
                 AND deleted IS NULL)"#,
        )
        .bind(tenant_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_one(&mut **tx)
        .await?;
        if !assigned {
            return Err(AppError::not_found("labor candidate scope"));
        }
    }
    Ok(())
}

pub async fn roster_candidates(
    db: &Db,
    access: &TenantAccess,
    filter: &LaborRosterFilter,
) -> AppResult<LaborRosterPageReadModel> {
    validate_limit(filter.limit)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_any_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION, CERTIFY_PERMISSION],
    )
    .await?;
    require_candidate_scope_tx(
        &mut tx,
        access.tenant_id.get(),
        &scope,
        filter.facility_id,
        filter.inventory_owner_id,
    )
    .await?;

    let fetch_limit = i64::from(filter.limit) + 1;
    let rows = sqlx::query(
        r#"SELECT employee.id AS employee_id,
                  employee.first_name || ' ' || employee.last_name AS display_name,
                  employee.title,
                  attendance.id AS attendance_interval_id,
                  attendance.revision AS attendance_revision,
                  activity.id AS active_activity_id,
                  ARRAY(SELECT DISTINCT certification.skill_id
                    FROM employee_certifications certification
                    JOIN labor_skills skill ON skill.tenant_id=certification.tenant_id
                      AND skill.id=certification.skill_id AND skill.active
                    WHERE certification.tenant_id=employee.tenant_id
                      AND certification.employee_id=employee.id
                      AND certification.facility_id=$2
                      AND certification.issued_at<=statement_timestamp()
                      AND (certification.expires_at IS NULL
                        OR certification.expires_at>statement_timestamp())
                      AND (certification.revoked_at IS NULL
                        OR certification.revoked_at>statement_timestamp())
                    ORDER BY certification.skill_id) AS certified_skill_ids
           FROM employees employee
           JOIN employee_facilities assignment
             ON assignment.tenant_id=employee.tenant_id
            AND assignment.employee_id=employee.id
            AND assignment.facility_id=$2 AND assignment.deleted IS NULL
           LEFT JOIN LATERAL (
             SELECT interval.id,interval.revision
             FROM attendance_intervals interval
             WHERE interval.tenant_id=employee.tenant_id
               AND interval.employee_id=employee.id AND interval.status='open'
             ORDER BY interval.id DESC LIMIT 1
           ) attendance ON true
           LEFT JOIN LATERAL (
             SELECT labor.id FROM labor_activities labor
             WHERE labor.tenant_id=employee.tenant_id
               AND labor.employee_id=employee.id AND labor.status='active'
             ORDER BY labor.id DESC LIMIT 1
           ) activity ON true
           WHERE employee.tenant_id=$1 AND employee.user_id IS NOT NULL
             AND employee.deleted IS NULL AND employee.hired<=statement_timestamp()
             AND (employee.terminated IS NULL OR employee.terminated>statement_timestamp())
             AND ($3::BIGINT IS NULL OR employee.id>$3)
           ORDER BY employee.id LIMIT $4"#,
    )
    .bind(access.tenant_id.get())
    .bind(filter.facility_id.get())
    .bind(filter.after.map(EmployeeId::get))
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > filter.limit as usize;
    let mut items = rows
        .iter()
        .take(filter.limit as usize)
        .map(|row| roster_candidate(row, filter))
        .collect::<AppResult<Vec<_>>>()?;
    let next_after = has_more
        .then(|| items.last().map(|item| item.employee_id))
        .flatten();
    if !has_more {
        items.shrink_to_fit();
    }
    tx.commit().await?;
    Ok(LaborRosterPageReadModel { items, next_after })
}

fn roster_candidate(
    row: &PgRow,
    filter: &LaborRosterFilter,
) -> AppResult<LaborRosterCandidateReadModel> {
    let employee_id = EmployeeId::new(row.try_get("employee_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let attendance_id = row
        .try_get::<Option<i64>, _>("attendance_interval_id")?
        .map(AttendanceIntervalId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let attendance_revision = row
        .try_get::<Option<i64>, _>("attendance_revision")?
        .map(LaborRevision::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let active_activity_id = row
        .try_get::<Option<i64>, _>("active_activity_id")?
        .map(LaborActivityId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let certified_skill_ids = row
        .try_get::<Vec<i64>, _>("certified_skill_ids")?
        .into_iter()
        .map(LaborSkillId::new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let mut evidence = vec!["Active employee with current facility assignment".to_owned()];
    if filter.inventory_owner_id.is_some() {
        evidence.push("Inventory owner is active at this facility".to_owned());
    }
    match attendance_id {
        Some(id) => evidence.push(format!("Open attendance interval #{}", id.get())),
        None => evidence.push("No open attendance interval".to_owned()),
    }
    if let Some(id) = active_activity_id {
        evidence.push(format!(
            "Active labor activity #{} blocks another start",
            id.get()
        ));
    }
    if !certified_skill_ids.is_empty() {
        evidence.push(format!(
            "{} current skill certification(s)",
            certified_skill_ids.len()
        ));
    }
    Ok(LaborRosterCandidateReadModel {
        employee_id,
        display_name: row.try_get("display_name")?,
        title: row.try_get("title")?,
        facility_id: filter.facility_id,
        attendance_interval_id: attendance_id,
        attendance_revision,
        active_activity_id,
        certified_skill_ids,
        can_clock_in: attendance_id.is_none(),
        can_start_activity: attendance_id.is_some() && active_activity_id.is_none(),
        eligibility_evidence: evidence,
    })
}

pub async fn reference_candidates(
    db: &Db,
    access: &TenantAccess,
    filter: &LaborReferenceCandidateFilter,
) -> AppResult<LaborReferenceCandidatePageReadModel> {
    validate_limit(filter.limit)?;
    validate_reference_shape(filter)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_any_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION],
    )
    .await?;
    require_candidate_scope_tx(
        &mut tx,
        access.tenant_id.get(),
        &scope,
        filter.facility_id,
        filter.inventory_owner_id,
    )
    .await?;

    let attendance_id = sqlx::query_scalar::<_, i64>(
        r#"SELECT attendance.id
           FROM employees employee
           JOIN employee_facilities assignment
             ON assignment.tenant_id=employee.tenant_id
            AND assignment.employee_id=employee.id
            AND assignment.facility_id=$3 AND assignment.deleted IS NULL
           JOIN attendance_intervals attendance
             ON attendance.tenant_id=employee.tenant_id
            AND attendance.employee_id=employee.id AND attendance.facility_id=$3
            AND attendance.status='open'
           WHERE employee.tenant_id=$1 AND employee.id=$2 AND employee.user_id IS NOT NULL
             AND employee.deleted IS NULL AND employee.hired<=statement_timestamp()
             AND (employee.terminated IS NULL OR employee.terminated>statement_timestamp())
             AND NOT EXISTS(SELECT 1 FROM labor_activities activity
               WHERE activity.tenant_id=employee.tenant_id
                 AND activity.employee_id=employee.id AND activity.status='active')
           ORDER BY attendance.id DESC LIMIT 1"#,
    )
    .bind(access.tenant_id.get())
    .bind(filter.employee_id.get())
    .bind(filter.facility_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("eligible labor employee"))?;
    let attendance_interval_id = AttendanceIntervalId::new(attendance_id)
        .map_err(|error| AppError::internal(error.to_string()))?;

    let rows = fetch_reference_rows(&mut tx, access.tenant_id.get(), filter).await?;
    let has_more = rows.len() > filter.limit as usize;
    let items = rows
        .iter()
        .take(filter.limit as usize)
        .map(|row| reference_candidate(row, filter))
        .collect::<AppResult<Vec<_>>>()?;
    let next_after = has_more
        .then(|| items.last().map(|item| item.reference_id))
        .flatten();
    tx.commit().await?;
    Ok(LaborReferenceCandidatePageReadModel {
        employee_id: filter.employee_id,
        attendance_interval_id,
        items,
        next_after,
    })
}

fn validate_reference_shape(filter: &LaborReferenceCandidateFilter) -> AppResult<()> {
    if !filter.activity_kind.is_direct() {
        return Err(AppError::bad_request(
            "labor reference candidates require a direct activity kind",
        ));
    }
    if !filter
        .activity_kind
        .supports_quantity_basis(filter.quantity_basis)
    {
        return Err(AppError::bad_request(
            "labor quantity basis is not supported by this activity kind",
        ));
    }
    if filter.inventory_owner_id.is_none() && filter.activity_kind != LaborActivityKind::CycleCount
    {
        return Err(AppError::bad_request(
            "direct labor reference candidates require an inventory owner",
        ));
    }
    Ok(())
}

async fn fetch_reference_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    filter: &LaborReferenceCandidateFilter,
) -> AppResult<Vec<PgRow>> {
    let limit = i64::from(filter.limit) + 1;
    let after = filter.after.unwrap_or(0);
    let owner_id = filter.inventory_owner_id.map(InventoryOwnerId::get);
    let employee_id = filter.employee_id.get();
    let facility_id = filter.facility_id.get();
    let basis = filter.quantity_basis.as_str();
    let kind = filter.activity_kind.as_str();
    let rows = match filter.activity_kind {
        LaborActivityKind::Receiving => sqlx::query(
            r#"SELECT load.id AS reference_id,
                 COALESCE(load.reference_number,'Inbound load #'||load.id::TEXT) AS display_label,
                 'Arrived inbound load · '||load.status AS evidence,
                 public.resolve_labor_reference_quantity($1,$6,$7,load.id) AS canonical_quantity
               FROM loads load WHERE load.tenant_id=$1 AND load.facility_id=$2
                 AND load.inventory_owner_id=$3 AND load.type='inbound'
                 AND load.status IN('arrived','receiving') AND load.deleted IS NULL AND load.id>$5
                 AND public.resolve_labor_reference_quantity($1,$6,$7,load.id) IS NOT NULL
               ORDER BY load.id LIMIT $8"#,
        )
        .bind(tenant_id).bind(facility_id).bind(owner_id).bind(employee_id).bind(after)
        .bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        LaborActivityKind::Putaway | LaborActivityKind::Replenishment
        | LaborActivityKind::CycleCount | LaborActivityKind::InventoryRelocation => {
            let task_types: &[&str] = match filter.activity_kind {
                LaborActivityKind::Putaway => &["putaway", "license_plate_putaway"],
                LaborActivityKind::Replenishment => &["replenishment"],
                LaborActivityKind::CycleCount if owner_id.is_some() => &["cycle_count_item_location"],
                LaborActivityKind::CycleCount => &["cycle_count_location"],
                LaborActivityKind::InventoryRelocation => &["inventory_relocation"],
                _ => &[],
            };
            sqlx::query(
                r#"SELECT task.id AS reference_id,task.title AS display_label,
                     'Assigned work task · lease valid until '
                       ||task.lease_expires_at::TEXT AS evidence,
                     public.resolve_labor_reference_quantity($1,$7,$8,task.id) AS canonical_quantity
                   FROM work_tasks task
                   WHERE task.tenant_id=$1 AND task.facility_id=$2
                     AND task.inventory_owner_id IS NOT DISTINCT FROM $3
                     AND task.task_type=ANY($4) AND task.status IN('assigned','in_progress')
                     AND task.assigned_user_id=(SELECT user_id FROM employees
                       WHERE tenant_id=$1 AND id=$5)
                     AND task.lease_expires_at>statement_timestamp() AND task.deleted IS NULL
                     AND task.id>$6
                     AND public.resolve_labor_reference_quantity($1,$7,$8,task.id) IS NOT NULL
                   ORDER BY task.id LIMIT $9"#,
            )
            .bind(tenant_id).bind(facility_id).bind(owner_id).bind(task_types)
            .bind(employee_id).bind(after).bind(kind).bind(basis).bind(limit)
            .fetch_all(&mut **tx).await?
        }
        LaborActivityKind::Picking => sqlx::query(
            r#"SELECT task.id AS reference_id,
                 'Pick task #'||task.id::TEXT||' · order #'||task.order_id::TEXT AS display_label,
                 'Assigned pick task · lease valid until '||task.lease_expires_at::TEXT AS evidence,
                 public.resolve_labor_reference_quantity($1,$6,$7,task.id) AS canonical_quantity
               FROM pick_tasks task WHERE task.tenant_id=$1 AND task.facility_id=$2
                 AND task.inventory_owner_id=$3 AND task.status='in_progress'
                 AND task.assigned_user_id=(SELECT user_id FROM employees
                   WHERE tenant_id=$1 AND id=$4)
                 AND task.lease_expires_at>statement_timestamp() AND task.id>$5
                 AND public.resolve_labor_reference_quantity($1,$6,$7,task.id) IS NOT NULL
               ORDER BY task.id LIMIT $8"#,
        ).bind(tenant_id).bind(facility_id).bind(owner_id).bind(employee_id).bind(after)
        .bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        LaborActivityKind::Packing => sqlx::query(
            r#"SELECT session.id AS reference_id,
                 'Packing session #'||session.id::TEXT||' · order #'||session.order_id::TEXT AS display_label,
                 'Open packing session assigned to employee identity' AS evidence,
                 public.resolve_labor_reference_quantity($1,$6,$7,session.id) AS canonical_quantity
               FROM packing_sessions session WHERE session.tenant_id=$1 AND session.facility_id=$2
                 AND session.inventory_owner_id=$3 AND session.state='open'
                 AND session.started_by_user_id=(SELECT user_id FROM employees
                   WHERE tenant_id=$1 AND id=$4) AND session.id>$5
                 AND public.resolve_labor_reference_quantity($1,$6,$7,session.id) IS NOT NULL
               ORDER BY session.id LIMIT $8"#,
        ).bind(tenant_id).bind(facility_id).bind(owner_id).bind(employee_id).bind(after)
        .bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        LaborActivityKind::Shipping => sqlx::query(
            r#"SELECT shipment.id AS reference_id,
                 'Shipment #'||shipment.id::TEXT||' · order #'||shipment.order_id::TEXT AS display_label,
                 'Executable shipment · '||shipment.state AS evidence,
                 public.resolve_labor_reference_quantity($1,$5,$6,shipment.id) AS canonical_quantity
               FROM shipments shipment WHERE shipment.tenant_id=$1 AND shipment.facility_id=$2
                 AND shipment.inventory_owner_id=$3
                 AND shipment.state IN('awaiting manifest','manifested','partially departed')
                 AND shipment.id>$4
                 AND public.resolve_labor_reference_quantity($1,$5,$6,shipment.id) IS NOT NULL
               ORDER BY shipment.id LIMIT $7"#,
        ).bind(tenant_id).bind(facility_id).bind(owner_id).bind(after)
        .bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        LaborActivityKind::CrossDock => sqlx::query(
            r#"SELECT task.id AS reference_id,task.title AS display_label,
                 'Assigned cross-dock task · lease valid until '||task.lease_expires_at::TEXT AS evidence,
                 public.resolve_labor_reference_quantity($1,$7,$8,task.id) AS canonical_quantity
               FROM cross_dock_tasks cross_dock
               JOIN work_tasks task ON task.tenant_id=cross_dock.tenant_id
                 AND task.id=cross_dock.task_id
               WHERE cross_dock.tenant_id=$1 AND cross_dock.facility_id=$2
                 AND cross_dock.inventory_owner_id=$3
                 AND task.status IN('assigned','in_progress')
                 AND task.assigned_user_id=(SELECT user_id FROM employees
                   WHERE tenant_id=$1 AND id=$4)
                 AND task.lease_expires_at>statement_timestamp() AND task.deleted IS NULL
                 AND task.id>$5
                 AND public.resolve_labor_reference_quantity($1,$7,$8,task.id) IS NOT NULL
               ORDER BY task.id LIMIT $9"#,
        ).bind(tenant_id).bind(facility_id).bind(owner_id).bind(employee_id).bind(after)
        .bind(0_i64).bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        LaborActivityKind::Yard => sqlx::query(
            r#"SELECT visit.id AS reference_id,
                 'Yard visit #'||visit.id::TEXT||' · '||visit.driver_name AS display_label,
                 'Active yard operation · '||visit.status AS evidence,
                 public.resolve_labor_reference_quantity($1,$5,$6,visit.id) AS canonical_quantity
               FROM yard_visits visit WHERE visit.tenant_id=$1 AND visit.facility_id=$2
                 AND visit.inventory_owner_id=$3 AND visit.status IN('at_door','loading','unloading')
                 AND visit.id>$4
                 AND public.resolve_labor_reference_quantity($1,$5,$6,visit.id) IS NOT NULL
               ORDER BY visit.id LIMIT $7"#,
        ).bind(tenant_id).bind(facility_id).bind(owner_id).bind(after)
        .bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        LaborActivityKind::CustomerReturn => sqlx::query(
            r#"SELECT customer_return.id AS reference_id,customer_return.customer_reference AS display_label,
                 'Planned customer return on arrived inbound load' AS evidence,
                 public.resolve_labor_reference_quantity($1,$5,$6,customer_return.id) AS canonical_quantity
               FROM customer_returns customer_return
               JOIN inbound_asns asn ON asn.tenant_id=customer_return.tenant_id
                 AND asn.id=customer_return.inbound_asn_id
               JOIN loads load ON load.tenant_id=asn.tenant_id AND load.id=asn.load_id
               WHERE customer_return.tenant_id=$1 AND customer_return.facility_id=$2
                 AND customer_return.inventory_owner_id=$3 AND asn.status='planned'
                 AND load.status IN('arrived','receiving') AND customer_return.id>$4
                 AND public.resolve_labor_reference_quantity($1,$5,$6,customer_return.id) IS NOT NULL
               ORDER BY customer_return.id LIMIT $7"#,
        ).bind(tenant_id).bind(facility_id).bind(owner_id).bind(after)
        .bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        LaborActivityKind::VendorReturn => sqlx::query(
            r#"SELECT vendor_return.id AS reference_id,vendor_return.return_number AS display_label,
                 'Released vendor return' AS evidence,
                 public.resolve_labor_reference_quantity($1,$5,$6,vendor_return.id) AS canonical_quantity
               FROM vendor_returns vendor_return WHERE vendor_return.tenant_id=$1
                 AND vendor_return.facility_id=$2 AND vendor_return.inventory_owner_id=$3
                 AND vendor_return.status='released' AND vendor_return.id>$4
                 AND public.resolve_labor_reference_quantity($1,$5,$6,vendor_return.id) IS NOT NULL
               ORDER BY vendor_return.id LIMIT $7"#,
        ).bind(tenant_id).bind(facility_id).bind(owner_id).bind(after)
        .bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        LaborActivityKind::ValueAddedWork => sqlx::query(
            r#"SELECT work.id AS reference_id,work.work_number AS display_label,
                 'Released value-added work order' AS evidence,
                 public.resolve_labor_reference_quantity($1,$5,$6,work.id) AS canonical_quantity
               FROM value_added_work_orders work WHERE work.tenant_id=$1
                 AND work.facility_id=$2 AND work.inventory_owner_id=$3
                 AND work.status='released' AND work.id>$4
                 AND public.resolve_labor_reference_quantity($1,$5,$6,work.id) IS NOT NULL
               ORDER BY work.id LIMIT $7"#,
        ).bind(tenant_id).bind(facility_id).bind(owner_id).bind(after)
        .bind(kind).bind(basis).bind(limit).fetch_all(&mut **tx).await?,
        _ => return Err(AppError::bad_request("indirect labor has no executable reference")),
    };
    Ok(rows)
}

fn reference_candidate(
    row: &PgRow,
    filter: &LaborReferenceCandidateFilter,
) -> AppResult<LaborReferenceCandidateReadModel> {
    let canonical_quantity = row.try_get::<i64, _>("canonical_quantity")?;
    Ok(LaborReferenceCandidateReadModel {
        reference_id: row.try_get("reference_id")?,
        display_label: row.try_get("display_label")?,
        facility_id: filter.facility_id,
        inventory_owner_id: filter.inventory_owner_id,
        canonical_quantity,
        eligibility_evidence: vec![
            row.try_get("evidence")?,
            format!(
                "Canonical {} quantity: {canonical_quantity}",
                filter.quantity_basis.as_str()
            ),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(kind: LaborActivityKind, basis: LaborQuantityBasis) -> LaborReferenceCandidateFilter {
        LaborReferenceCandidateFilter {
            facility_id: FacilityId::new(1).unwrap(),
            inventory_owner_id: Some(InventoryOwnerId::new(2).unwrap()),
            employee_id: EmployeeId::new(3).unwrap(),
            activity_kind: kind,
            quantity_basis: basis,
            after: None,
            limit: 50,
        }
    }

    #[test]
    fn candidate_shape_rejects_indirect_unsupported_and_ownerless_work() {
        assert!(validate_reference_shape(&filter(
            LaborActivityKind::Meeting,
            LaborQuantityBasis::Task
        ))
        .is_err());
        assert!(validate_reference_shape(&filter(
            LaborActivityKind::Receiving,
            LaborQuantityBasis::WeightGram
        ))
        .is_err());
        let mut ownerless = filter(LaborActivityKind::Picking, LaborQuantityBasis::Unit);
        ownerless.inventory_owner_id = None;
        assert!(validate_reference_shape(&ownerless).is_err());
        let mut cycle_count = filter(LaborActivityKind::CycleCount, LaborQuantityBasis::Line);
        cycle_count.inventory_owner_id = None;
        assert!(validate_reference_shape(&cycle_count).is_ok());
    }

    #[test]
    fn labor_candidate_pages_are_tightly_bounded() {
        assert!(validate_limit(1).is_ok());
        assert!(validate_limit(MAX_LABOR_CANDIDATE_PAGE_SIZE).is_ok());
        assert!(validate_limit(0).is_err());
        assert!(validate_limit(MAX_LABOR_CANDIDATE_PAGE_SIZE + 1).is_err());
    }
}
