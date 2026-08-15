use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::labor::{
    AttendanceIntervalReadModel, CancelLaborActivityCommand, CertifyEmployeeCommand,
    ChangeEquipmentStatusCommand, ClockInCommand, ClockOutCommand, CompleteLaborActivityCommand,
    ConfigureEquipmentClassCommand, ConfigureLaborSkillCommand, ConfigureLaborStandardCommand,
    CreateEquipmentAssetCommand, EmployeeCertificationReadModel, EquipmentAssetReadModel,
    EquipmentClassReadModel, LaborActivityReadModel, LaborSkillReadModel, LaborStandardReadModel,
    RevokeEmployeeCertificationCommand, StartLaborActivityCommand, CANCEL_LABOR_ACTIVITY_OPERATION,
    CERTIFY_EMPLOYEE_OPERATION, CHANGE_EQUIPMENT_STATUS_OPERATION, CLOCK_IN_OPERATION,
    CLOCK_OUT_OPERATION, COMPLETE_LABOR_ACTIVITY_OPERATION, CONFIGURE_EQUIPMENT_CLASS_OPERATION,
    CONFIGURE_LABOR_SKILL_OPERATION, CONFIGURE_LABOR_STANDARD_OPERATION,
    CREATE_EQUIPMENT_ASSET_OPERATION, REVOKE_EMPLOYEE_CERTIFICATION_OPERATION,
    START_LABOR_ACTIVITY_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    assess_eligibility, efficiency_basis_points, validate_attendance_close,
    validate_labor_completion, validate_labor_start, AttendanceIntervalId, AttendanceStatus,
    EligibilityEvidence, EmployeeCertificationId, EmployeeId, EquipmentAssetId, EquipmentClassId,
    EquipmentStatus, FacilityId, InventoryOwnerId, LaborActivityId, LaborActivityKind,
    LaborActivityStatus, LaborRevision, LaborSkillId, LaborStandard, LaborStandardId,
    StartLaborActivity, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::models::{
    read_activity_tx, read_attendance_tx, read_certification_tx, read_equipment_asset_tx,
    read_equipment_class_tx, read_skill_tx, read_standard_tx,
};
use super::{
    enqueue_event_tx, internal, lock_key_tx, parse_activity_kind, parse_activity_status,
    parse_attendance_status, parse_equipment_status, require_access_actor, require_facility,
    require_scope, require_tenant_global_scope, LaborOutboxEvent, CERTIFY_PERMISSION,
    CONFIGURE_PERMISSION, EQUIPMENT_PERMISSION, EXECUTE_PERMISSION, SUPERVISE_PERMISSION,
};
use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{
    lock_current_scope_tx, require_any_permission_tx, require_permission_tx, ScopeBindings,
};

async fn begin_scoped<'a>(
    db: &'a Db,
    access: &TenantAccess,
    context: &CommandContext,
) -> AppResult<(sqlx::Transaction<'a, sqlx::Postgres>, ScopeBindings)> {
    require_access_actor(access, context)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    Ok((tx, scope))
}

async fn begin_command<'a>(
    db: &'a Db,
    access: &TenantAccess,
    context: &CommandContext,
    permission: &str,
) -> AppResult<(sqlx::Transaction<'a, sqlx::Postgres>, ScopeBindings)> {
    let (mut tx, scope) = begin_scoped(db, access, context).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        permission,
    )
    .await?;
    Ok((tx, scope))
}

async fn require_self_or_supervisor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: i64,
    employee_id: EmployeeId,
) -> AppResult<()> {
    let employee_user_id: Option<Option<i64>> =
        sqlx::query_scalar("SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$2 FOR SHARE")
            .bind(tenant_id.get())
            .bind(employee_id.get())
            .fetch_optional(&mut **tx)
            .await?;
    let employee_user_id = employee_user_id.ok_or_else(|| AppError::not_found("employee"))?;
    if employee_user_id == Some(actor_id) {
        require_any_permission_tx(
            tx,
            tenant_id,
            actor_id,
            &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION],
        )
        .await
    } else {
        require_permission_tx(tx, tenant_id, actor_id, SUPERVISE_PERMISSION).await
    }
}

async fn active_skill_exists_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    skill_id: LaborSkillId,
) -> AppResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM labor_skills WHERE tenant_id=$1 AND id=$2 AND active FOR SHARE",
    )
    .bind(tenant_id.get())
    .bind(skill_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .is_some())
}

async fn employee_is_active_in_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    employee_id: EmployeeId,
    facility_id: FacilityId,
    at: Timestamp,
) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        r#"SELECT EXISTS(
          SELECT 1 FROM employees employee
          JOIN employee_facilities assignment ON assignment.tenant_id=employee.tenant_id
            AND assignment.employee_id=employee.id AND assignment.facility_id=$3
            AND assignment.deleted IS NULL
          JOIN facilities facility ON facility.tenant_id=employee.tenant_id
            AND facility.id=assignment.facility_id AND facility.deleted IS NULL
          WHERE employee.tenant_id=$1 AND employee.id=$2 AND employee.deleted IS NULL
            AND employee.hired<=$4
            AND (employee.terminated IS NULL OR employee.terminated>$4))"#,
    )
    .bind(tenant_id.get())
    .bind(employee_id.get())
    .bind(facility_id.get())
    .bind(at)
    .fetch_one(&mut **tx)
    .await?)
}

async fn owner_facility_exists_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM inventory_owner_facilities assignment
          JOIN inventory_owners owner ON owner.tenant_id=assignment.tenant_id
            AND owner.id=assignment.inventory_owner_id AND owner.deleted IS NULL
          JOIN facilities facility ON facility.tenant_id=assignment.tenant_id
            AND facility.id=assignment.facility_id AND facility.deleted IS NULL
          WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=$2
            AND assignment.facility_id=$3 AND assignment.deleted IS NULL)"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?)
}

struct AttendanceLock {
    employee_id: EmployeeId,
    facility_id: FacilityId,
    status: AttendanceStatus,
}

async fn lock_attendance_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    attendance_id: AttendanceIntervalId,
) -> AppResult<AttendanceLock> {
    let row = sqlx::query(
        r#"SELECT employee_id,facility_id,status FROM attendance_intervals
          WHERE tenant_id=$1 AND id=$2 FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(attendance_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("attendance interval"))?;
    Ok(AttendanceLock {
        employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        status: parse_attendance_status(row.try_get("status")?)?,
    })
}

struct StandardSnapshot {
    standard_id: LaborStandardId,
    setup_seconds: i64,
    seconds_per_unit: i64,
    quantity_basis: wareboxes_domain::LaborQuantityBasis,
    required_skill_id: Option<LaborSkillId>,
    required_equipment_class_id: Option<EquipmentClassId>,
}

async fn load_standard_snapshot_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    standard_id: LaborStandardId,
    facility_id: FacilityId,
    owner_id: Option<InventoryOwnerId>,
    kind: LaborActivityKind,
    at: Timestamp,
) -> AppResult<StandardSnapshot> {
    let row = sqlx::query(
        r#"SELECT setup_seconds,seconds_per_unit,quantity_basis,required_skill_id,
          required_equipment_class_id FROM labor_standards
          WHERE tenant_id=$1 AND id=$2 AND facility_id=$3
            AND (($4::BIGINT IS NULL AND inventory_owner_id IS NULL)
              OR ($4::BIGINT IS NOT NULL AND (inventory_owner_id=$4 OR inventory_owner_id IS NULL)))
            AND activity_kind=$5
            AND effective_from<=$6 AND (effective_until IS NULL OR effective_until>$6)
          FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(standard_id.get())
    .bind(facility_id.get())
    .bind(owner_id.map(|id| id.get()))
    .bind(kind.as_str())
    .bind(at)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("effective labor standard"))?;
    Ok(StandardSnapshot {
        standard_id,
        setup_seconds: row.try_get("setup_seconds")?,
        seconds_per_unit: row.try_get("seconds_per_unit")?,
        quantity_basis: wareboxes_domain::LaborQuantityBasis::parse(
            row.try_get::<&str, _>("quantity_basis")?,
        )
        .ok_or_else(|| AppError::internal("invalid stored labor quantity basis"))?,
        required_skill_id: row
            .try_get::<Option<i64>, _>("required_skill_id")?
            .map(LaborSkillId::new)
            .transpose()
            .map_err(internal)?,
        required_equipment_class_id: row
            .try_get::<Option<i64>, _>("required_equipment_class_id")?
            .map(EquipmentClassId::new)
            .transpose()
            .map_err(internal)?,
    })
}

struct EquipmentLock {
    asset_id: EquipmentAssetId,
    class_id: EquipmentClassId,
    class_required_skill_id: Option<LaborSkillId>,
    status: EquipmentStatus,
    facility_id: FacilityId,
}

async fn lock_equipment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    asset_id: EquipmentAssetId,
) -> AppResult<EquipmentLock> {
    let row = sqlx::query(
        r#"SELECT equipment.id,equipment.facility_id,equipment.equipment_class_id,
          equipment.status,class.required_skill_id
          FROM equipment_assets equipment
          JOIN equipment_classes class ON class.tenant_id=equipment.tenant_id
            AND class.id=equipment.equipment_class_id AND class.active
          WHERE equipment.tenant_id=$1 AND equipment.id=$2
          FOR UPDATE OF equipment FOR SHARE OF class"#,
    )
    .bind(tenant_id.get())
    .bind(asset_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("equipment asset"))?;
    Ok(EquipmentLock {
        asset_id,
        class_id: EquipmentClassId::new(row.try_get("equipment_class_id")?).map_err(internal)?,
        class_required_skill_id: row
            .try_get::<Option<i64>, _>("required_skill_id")?
            .map(LaborSkillId::new)
            .transpose()
            .map_err(internal)?,
        status: parse_equipment_status(row.try_get("status")?)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
    })
}

async fn certification_evidence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    employee_id: EmployeeId,
    facility_id: FacilityId,
    skill_id: LaborSkillId,
    at: Timestamp,
) -> AppResult<Option<EmployeeCertificationId>> {
    sqlx::query_scalar::<_, i64>(
        r#"SELECT id FROM employee_certifications
          WHERE tenant_id=$1 AND employee_id=$2 AND facility_id=$3 AND skill_id=$4
            AND issued_at<=$5 AND (expires_at IS NULL OR expires_at>$5)
            AND (revoked_at IS NULL OR revoked_at>$5)
          ORDER BY skill_id,id LIMIT 1 FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(employee_id.get())
    .bind(facility_id.get())
    .bind(skill_id.get())
    .bind(at)
    .fetch_optional(&mut **tx)
    .await?
    .map(EmployeeCertificationId::new)
    .transpose()
    .map_err(internal)
}

struct LaborReference<'a> {
    tenant_id: TenantId,
    employee_id: EmployeeId,
    facility_id: FacilityId,
    kind: LaborActivityKind,
    reference_type: &'a str,
    reference_id: i64,
    at: Timestamp,
}

async fn direct_reference_exists_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner_id: InventoryOwnerId,
    reference: &LaborReference<'_>,
) -> AppResult<bool> {
    let tenant_id = reference.tenant_id;
    let employee_id = reference.employee_id;
    let facility_id = reference.facility_id;
    let kind = reference.kind;
    let reference_type = reference.reference_type;
    let reference_id = reference.reference_id;
    let at = reference.at;
    let exists = match (kind, reference_type) {
        (LaborActivityKind::Receiving, "inbound_load") => sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM loads WHERE tenant_id=$1 AND id=$2
              AND inventory_owner_id=$3 AND facility_id=$4 AND type='inbound'
              AND status IN('arrived','receiving') AND deleted IS NULL)"#,
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::Putaway, "work_task") => sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM work_tasks WHERE tenant_id=$1 AND id=$2
              AND inventory_owner_id=$3 AND facility_id=$4 AND task_type IN('putaway','license_plate_putaway')
              AND status IN('assigned','in_progress') AND lease_expires_at>$6
              AND assigned_user_id=(
                SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$5)
              AND deleted IS NULL)"#,
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(employee_id.get())
        .bind(at)
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::Replenishment, "work_task") => sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM work_tasks WHERE tenant_id=$1 AND id=$2
              AND inventory_owner_id=$3 AND facility_id=$4 AND task_type='replenishment'
              AND status IN('assigned','in_progress') AND lease_expires_at>$6
              AND assigned_user_id=(
                SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$5)
              AND deleted IS NULL)"#,
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(employee_id.get())
        .bind(at)
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::Picking, "pick_task") => sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pick_tasks WHERE tenant_id=$1 AND id=$2 AND inventory_owner_id=$3 AND facility_id=$4 AND status='in_progress' AND lease_expires_at>$6 AND assigned_user_id=(SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$5))",
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(employee_id.get())
        .bind(at)
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::Packing, "packing_session") => sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM packing_sessions WHERE tenant_id=$1 AND id=$2 AND inventory_owner_id=$3 AND facility_id=$4 AND state='open' AND started_by_user_id=(SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$5))",
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(employee_id.get())
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::Shipping, "shipment") => sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM shipments WHERE tenant_id=$1 AND id=$2 AND inventory_owner_id=$3 AND facility_id=$4 AND state IN('awaiting manifest','manifested','partially departed'))",
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::CycleCount, "work_task") => sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM work_tasks WHERE tenant_id=$1 AND id=$2
              AND inventory_owner_id=$3 AND facility_id=$4
              AND task_type='cycle_count_item_location' AND status IN('assigned','in_progress')
              AND lease_expires_at>$6
              AND assigned_user_id=(SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$5)
              AND deleted IS NULL)"#,
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(employee_id.get())
        .bind(at)
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::InventoryRelocation, "work_task") => sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM work_tasks WHERE tenant_id=$1 AND id=$2
              AND inventory_owner_id=$3 AND facility_id=$4 AND task_type='inventory_relocation'
              AND status IN('assigned','in_progress') AND lease_expires_at>$6
              AND assigned_user_id=(
                SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$5)
              AND deleted IS NULL)"#,
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(employee_id.get())
        .bind(at)
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::CrossDock, "work_task") => sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM cross_dock_tasks cross_dock
              JOIN work_tasks task ON task.tenant_id=cross_dock.tenant_id
                AND task.id=cross_dock.task_id
              WHERE cross_dock.tenant_id=$1 AND cross_dock.task_id=$2
                AND cross_dock.inventory_owner_id=$3 AND cross_dock.facility_id=$4
                AND task.status IN('assigned','in_progress') AND task.lease_expires_at>$6
                AND task.assigned_user_id=(
                  SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$5)
                AND task.deleted IS NULL)"#,
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(employee_id.get())
        .bind(at)
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::Yard, "yard_visit") => sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM yard_visits WHERE tenant_id=$1 AND id=$2 AND inventory_owner_id=$3 AND facility_id=$4 AND status IN('at_door','loading','unloading'))",
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::CustomerReturn, "customer_return") => sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM customer_returns customer_return
              JOIN inbound_asns asn ON asn.tenant_id=customer_return.tenant_id
                AND asn.id=customer_return.inbound_asn_id
              JOIN loads load ON load.tenant_id=asn.tenant_id AND load.id=asn.load_id
              WHERE customer_return.tenant_id=$1 AND customer_return.id=$2
                AND customer_return.inventory_owner_id=$3
                AND customer_return.facility_id=$4 AND asn.status='planned'
                AND load.status IN('arrived','receiving'))"#,
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::VendorReturn, "vendor_return") => sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM vendor_returns WHERE tenant_id=$1 AND id=$2 AND inventory_owner_id=$3 AND facility_id=$4 AND status='released')",
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_one(&mut **tx)
        .await?,
        (LaborActivityKind::ValueAddedWork, "value_added_work_order") => sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM value_added_work_orders WHERE tenant_id=$1 AND id=$2 AND inventory_owner_id=$3 AND facility_id=$4 AND status='released')",
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_one(&mut **tx)
        .await?,
        _ => false,
    };
    Ok(exists)
}

async fn facility_shared_reference_exists_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reference: &LaborReference<'_>,
) -> AppResult<bool> {
    let tenant_id = reference.tenant_id;
    let employee_id = reference.employee_id;
    let facility_id = reference.facility_id;
    let kind = reference.kind;
    let reference_type = reference.reference_type;
    let reference_id = reference.reference_id;
    let at = reference.at;
    match (kind, reference_type) {
        (LaborActivityKind::CycleCount, "work_task") => Ok(sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM work_tasks WHERE tenant_id=$1 AND id=$2
              AND inventory_owner_id IS NULL AND facility_id=$3
              AND task_type='cycle_count_location' AND status IN('assigned','in_progress')
              AND lease_expires_at>$5
              AND assigned_user_id=(SELECT user_id FROM employees WHERE tenant_id=$1 AND id=$4)
              AND deleted IS NULL)"#,
        )
        .bind(tenant_id.get())
        .bind(reference_id)
        .bind(facility_id.get())
        .bind(employee_id.get())
        .bind(at)
        .fetch_one(&mut **tx)
        .await?),
        _ => Ok(false),
    }
}

async fn reference_quantity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    kind: LaborActivityKind,
    basis: wareboxes_domain::LaborQuantityBasis,
    reference_id: i64,
) -> AppResult<i64> {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT public.resolve_labor_reference_quantity($1,$2,$3,$4)",
    )
    .bind(tenant_id.get())
    .bind(kind.as_str())
    .bind(basis.as_str())
    .bind(reference_id)
    .fetch_one(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("labor quantity basis has no canonical work evidence"))
}

pub async fn configure_skill(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureLaborSkillCommand,
) -> AppResult<LaborSkillReadModel> {
    let prepared = PreparedCommand::new_v1(context, CONFIGURE_LABOR_SKILL_OPERATION, command)?;
    let (mut tx, scope) = begin_command(db, access, context, CONFIGURE_PERMISSION).await?;
    require_tenant_global_scope(&scope)?;
    if let Some(result) = prepared.replayed::<LaborSkillReadModel>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        &format!("labor-skill:{}:{}", access.tenant_id, command.code.as_str()),
    )
    .await?;
    let now = now_iso();
    let skill_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_skills
          (tenant_id,code,name,certification_required,active,configured_by_user_id,configured_at)
          VALUES($1,$2,$3,$4,true,$5,$6)
          ON CONFLICT(tenant_id,code) DO UPDATE SET
            name=EXCLUDED.name,certification_required=EXCLUDED.certification_required,active=true,
            revision=labor_skills.revision+1,
            configured_by_user_id=EXCLUDED.configured_by_user_id,
            configured_at=EXCLUDED.configured_at RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.code.as_str())
    .bind(command.name.as_str())
    .bind(command.certification_required)
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_skill_tx(
        &mut tx,
        access.tenant_id,
        LaborSkillId::new(skill_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: None,
            owner_id: None,
            aggregate_type: "skill",
            aggregate_id: skill_id,
            transition: "configured",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn certify_employee(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CertifyEmployeeCommand,
) -> AppResult<EmployeeCertificationReadModel> {
    let prepared = PreparedCommand::new_v1(context, CERTIFY_EMPLOYEE_OPERATION, command)?;
    let (mut tx, scope) = begin_command(db, access, context, CERTIFY_PERMISSION).await?;
    require_facility(&scope, command.facility_id)?;
    if let Some(result) = prepared
        .replayed::<EmployeeCertificationReadModel>(&mut tx)
        .await?
    {
        require_facility(&scope, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        &format!(
            "labor-certification:{}:{}:{}:{}",
            access.tenant_id, command.employee_id, command.skill_id, command.facility_id
        ),
    )
    .await?;
    let now = now_iso();
    if !employee_is_active_in_facility_tx(
        &mut tx,
        access.tenant_id,
        command.employee_id,
        command.facility_id,
        now,
    )
    .await?
    {
        return Err(AppError::not_found("employee"));
    }
    if !active_skill_exists_tx(&mut tx, access.tenant_id, command.skill_id).await? {
        return Err(AppError::not_found("labor skill"));
    }
    let certification_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO employee_certifications
          (tenant_id,employee_id,skill_id,facility_id,certification_number,issued_at,expires_at,
           note,certified_by_user_id,certified_at)
          VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.employee_id.get())
    .bind(command.skill_id.get())
    .bind(command.facility_id.get())
    .bind(
        command
            .certification_number
            .as_ref()
            .map(|value| value.as_str()),
    )
    .bind(command.window.issued_at)
    .bind(command.window.expires_at)
    .bind(command.note.as_ref().map(|value| value.as_str()))
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_certification_tx(
        &mut tx,
        access.tenant_id,
        EmployeeCertificationId::new(certification_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(command.facility_id),
            owner_id: None,
            aggregate_type: "certification",
            aggregate_id: certification_id,
            transition: "granted",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn revoke_certification(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RevokeEmployeeCertificationCommand,
) -> AppResult<EmployeeCertificationReadModel> {
    let prepared =
        PreparedCommand::new_v1(context, REVOKE_EMPLOYEE_CERTIFICATION_OPERATION, command)?;
    let (mut tx, scope) = begin_command(db, access, context, CERTIFY_PERMISSION).await?;
    if let Some(result) = prepared
        .replayed::<EmployeeCertificationReadModel>(&mut tx)
        .await?
    {
        require_facility(&scope, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        r#"SELECT facility_id,revision,revoked_at FROM employee_certifications
          WHERE tenant_id=$1 AND id=$2 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.certification_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("employee certification"))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?).map_err(internal)?;
    require_facility(&scope, facility_id)?;
    if row.try_get::<i64, _>("revision")? != command.expected_revision.get() {
        return Err(AppError::conflict(
            "employee certification revision is stale",
        ));
    }
    if row.try_get::<Option<Timestamp>, _>("revoked_at")?.is_some() {
        return Err(AppError::conflict(
            "employee certification is already revoked",
        ));
    }
    let now = now_iso();
    sqlx::query(
        r#"UPDATE employee_certifications SET revision=revision+1,revoked_by_user_id=$3,
          revoked_at=$4,revocation_note=$5 WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.certification_id.get())
    .bind(context.actor_id.get())
    .bind(now)
    .bind(command.note.as_str())
    .execute(&mut *tx)
    .await?;
    let result = read_certification_tx(&mut tx, access.tenant_id, command.certification_id).await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(facility_id),
            owner_id: None,
            aggregate_type: "certification",
            aggregate_id: command.certification_id.get(),
            transition: "revoked",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn configure_equipment_class(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureEquipmentClassCommand,
) -> AppResult<EquipmentClassReadModel> {
    let prepared = PreparedCommand::new_v1(context, CONFIGURE_EQUIPMENT_CLASS_OPERATION, command)?;
    let (mut tx, scope) = begin_command(db, access, context, CONFIGURE_PERMISSION).await?;
    require_tenant_global_scope(&scope)?;
    if let Some(result) = prepared
        .replayed::<EquipmentClassReadModel>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        &format!(
            "equipment-class:{}:{}",
            access.tenant_id,
            command.code.as_str()
        ),
    )
    .await?;
    if let Some(skill_id) = command.required_skill_id {
        if !active_skill_exists_tx(&mut tx, access.tenant_id, skill_id).await? {
            return Err(AppError::not_found("labor skill"));
        }
    }
    let now = now_iso();
    let class_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO equipment_classes
          (tenant_id,code,name,required_skill_id,active,configured_by_user_id,configured_at)
          VALUES($1,$2,$3,$4,true,$5,$6)
          ON CONFLICT(tenant_id,code) DO UPDATE SET name=EXCLUDED.name,
            required_skill_id=EXCLUDED.required_skill_id,active=true,
            revision=equipment_classes.revision+1,
            configured_by_user_id=EXCLUDED.configured_by_user_id,
            configured_at=EXCLUDED.configured_at RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.code.as_str())
    .bind(command.name.as_str())
    .bind(command.required_skill_id.map(|id| id.get()))
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_equipment_class_tx(
        &mut tx,
        access.tenant_id,
        EquipmentClassId::new(class_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: None,
            owner_id: None,
            aggregate_type: "equipment_class",
            aggregate_id: class_id,
            transition: "configured",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn create_equipment_asset(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateEquipmentAssetCommand,
) -> AppResult<EquipmentAssetReadModel> {
    let prepared = PreparedCommand::new_v1(context, CREATE_EQUIPMENT_ASSET_OPERATION, command)?;
    let (mut tx, scope) = begin_command(db, access, context, EQUIPMENT_PERMISSION).await?;
    require_facility(&scope, command.facility_id)?;
    if let Some(result) = prepared
        .replayed::<EquipmentAssetReadModel>(&mut tx)
        .await?
    {
        require_facility(&scope, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        &format!(
            "equipment-asset:{}:{}",
            access.tenant_id,
            command.equipment_number.as_str()
        ),
    )
    .await?;
    let class_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM equipment_classes WHERE tenant_id=$1 AND id=$2 AND active)",
    )
    .bind(access.tenant_id.get())
    .bind(command.equipment_class_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !class_exists {
        return Err(AppError::not_found("equipment class"));
    }
    let facility_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM facilities WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL)",
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !facility_exists {
        return Err(AppError::not_found("facility"));
    }
    let now = now_iso();
    let asset_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO equipment_assets
          (tenant_id,facility_id,equipment_class_id,equipment_number,name,status,
           configured_by_user_id,configured_at)
          VALUES($1,$2,$3,$4,$5,'available',$6,$7) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.equipment_class_id.get())
    .bind(command.equipment_number.as_str())
    .bind(command.name.as_str())
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_equipment_asset_tx(
        &mut tx,
        access.tenant_id,
        EquipmentAssetId::new(asset_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(command.facility_id),
            owner_id: None,
            aggregate_type: "equipment",
            aggregate_id: asset_id,
            transition: "created",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn change_equipment_status(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ChangeEquipmentStatusCommand,
) -> AppResult<EquipmentAssetReadModel> {
    if command.status == EquipmentStatus::Assigned {
        return Err(AppError::bad_request(
            "assigned equipment status is managed by labor activity commands",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, CHANGE_EQUIPMENT_STATUS_OPERATION, command)?;
    let (mut tx, scope) = begin_command(db, access, context, EQUIPMENT_PERMISSION).await?;
    if let Some(result) = prepared
        .replayed::<EquipmentAssetReadModel>(&mut tx)
        .await?
    {
        require_facility(&scope, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        r#"SELECT facility_id,status,revision FROM equipment_assets
          WHERE tenant_id=$1 AND id=$2 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.equipment_asset_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("equipment asset"))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?).map_err(internal)?;
    require_facility(&scope, facility_id)?;
    if row.try_get::<i64, _>("revision")? != command.expected_revision.get() {
        return Err(AppError::conflict("equipment asset revision is stale"));
    }
    let current_status = parse_equipment_status(row.try_get("status")?)?;
    if current_status == EquipmentStatus::Assigned {
        return Err(AppError::conflict("equipment is assigned to active labor"));
    }
    if current_status == EquipmentStatus::Retired {
        return Err(AppError::conflict("retired equipment cannot change status"));
    }
    if current_status == command.status {
        return Err(AppError::conflict("equipment already has requested status"));
    }
    let now = now_iso();
    sqlx::query(
        r#"UPDATE equipment_assets SET status=$3,assigned_employee_id=NULL,
          revision=revision+1,status_note=$4,status_changed_by_user_id=$5,status_changed_at=$6
          WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.equipment_asset_id.get())
    .bind(command.status.as_str())
    .bind(command.note.as_str())
    .bind(context.actor_id.get())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let result =
        read_equipment_asset_tx(&mut tx, access.tenant_id, command.equipment_asset_id).await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(facility_id),
            owner_id: None,
            aggregate_type: "equipment",
            aggregate_id: command.equipment_asset_id.get(),
            transition: command.status.as_str(),
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn configure_standard(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureLaborStandardCommand,
) -> AppResult<LaborStandardReadModel> {
    if !command.activity_kind.is_direct() {
        return Err(AppError::bad_request(
            "labor standards can only be configured for direct activities",
        ));
    }
    if !command
        .activity_kind
        .supports_quantity_basis(command.quantity_basis)
    {
        return Err(AppError::bad_request(
            "labor quantity basis is not supported by this activity kind",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, CONFIGURE_LABOR_STANDARD_OPERATION, command)?;
    let (mut tx, scope) = begin_command(db, access, context, CONFIGURE_PERMISSION).await?;
    require_scope(&scope, command.facility_id, command.inventory_owner_id)?;
    if command.inventory_owner_id.is_none() && !scope.all_inventory_owners {
        return Err(AppError::forbidden());
    }
    if let Some(result) = prepared.replayed::<LaborStandardReadModel>(&mut tx).await? {
        require_scope(&scope, result.facility_id, result.inventory_owner_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        &format!(
            "labor-standard:{}:{}:{}:{}",
            access.tenant_id,
            command.facility_id,
            command.inventory_owner_id.map_or(0, |id| id.get()),
            command.code.as_str()
        ),
    )
    .await?;
    let facility_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM facilities WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL)",
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !facility_exists {
        return Err(AppError::not_found("facility"));
    }
    if let Some(owner_id) = command.inventory_owner_id {
        if !owner_facility_exists_tx(&mut tx, access.tenant_id, owner_id, command.facility_id)
            .await?
        {
            return Err(AppError::not_found("inventory owner facility assignment"));
        }
    }
    if let Some(skill_id) = command.required_skill_id {
        if !active_skill_exists_tx(&mut tx, access.tenant_id, skill_id).await? {
            return Err(AppError::not_found("labor skill"));
        }
    }
    if let Some(class_id) = command.required_equipment_class_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM equipment_classes WHERE tenant_id=$1 AND id=$2 AND active)",
        )
        .bind(access.tenant_id.get())
        .bind(class_id.get())
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(AppError::not_found("equipment class"));
        }
    }
    let now = now_iso();
    let prior = sqlx::query(
        r#"SELECT id,revision,effective_from,effective_until FROM labor_standards
          WHERE tenant_id=$1 AND facility_id=$2
            AND inventory_owner_id IS NOT DISTINCT FROM $3 AND code=$4
          ORDER BY effective_from DESC,id DESC LIMIT 1 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.inventory_owner_id.map(|id| id.get()))
    .bind(command.code.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let (revision, supersedes_standard_id) = if let Some(prior) = prior {
        let prior_id = LaborStandardId::new(prior.try_get("id")?).map_err(internal)?;
        let prior_revision = LaborRevision::new(prior.try_get("revision")?).map_err(internal)?;
        let prior_effective_from: Timestamp = prior.try_get("effective_from")?;
        let prior_effective_until: Option<Timestamp> = prior.try_get("effective_until")?;
        if command.effective_from <= prior_effective_from {
            return Err(AppError::conflict(
                "labor standard successor must start after the prior version",
            ));
        }
        if prior_effective_until.is_some_and(|until| until > command.effective_from) {
            return Err(AppError::conflict(
                "labor standard successor overlaps a bounded prior version",
            ));
        }
        if prior_effective_until.is_none() {
            let incompatible_active: bool = sqlx::query_scalar(
                r#"SELECT EXISTS(SELECT 1 FROM labor_activities
                  WHERE tenant_id=$1 AND labor_standard_id=$2 AND status='active'
                    AND started_at>=$3)"#,
            )
            .bind(access.tenant_id.get())
            .bind(prior_id.get())
            .bind(command.effective_from)
            .fetch_one(&mut *tx)
            .await?;
            if incompatible_active {
                return Err(AppError::conflict(
                    "labor standard successor would invalidate active labor",
                ));
            }
            sqlx::query(
                r#"UPDATE labor_standards SET effective_until=$3,
                  retired_by_user_id=$4,retired_at=$5 WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(access.tenant_id.get())
            .bind(prior_id.get())
            .bind(command.effective_from)
            .bind(context.actor_id.get())
            .bind(now)
            .execute(&mut *tx)
            .await?;
            let retired = read_standard_tx(&mut tx, access.tenant_id, prior_id).await?;
            enqueue_event_tx(
                &mut tx,
                LaborOutboxEvent {
                    tenant_id: access.tenant_id,
                    actor_id: context.actor_id,
                    facility_id: Some(command.facility_id),
                    owner_id: command.inventory_owner_id,
                    aggregate_type: "standard",
                    aggregate_id: prior_id.get(),
                    transition: "retired",
                    occurred_at: now,
                },
                &retired,
            )
            .await?;
        }
        (prior_revision.next().map_err(internal)?, Some(prior_id))
    } else {
        (LaborRevision::new(1).map_err(internal)?, None)
    };
    let standard_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_standards
          (tenant_id,facility_id,inventory_owner_id,code,name,activity_kind,quantity_basis,setup_seconds,
           seconds_per_unit,required_skill_id,required_equipment_class_id,effective_from,
           effective_until,revision,supersedes_standard_id,configured_by_user_id,configured_at)
          VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.inventory_owner_id.map(|id| id.get()))
    .bind(command.code.as_str())
    .bind(command.name.as_str())
    .bind(command.activity_kind.as_str())
    .bind(command.quantity_basis.as_str())
    .bind(command.standard.setup_seconds)
    .bind(command.standard.seconds_per_unit)
    .bind(command.required_skill_id.map(|id| id.get()))
    .bind(command.required_equipment_class_id.map(|id| id.get()))
    .bind(command.effective_from)
    .bind(command.effective_until)
    .bind(revision.get())
    .bind(supersedes_standard_id.map(|id| id.get()))
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_standard_tx(
        &mut tx,
        access.tenant_id,
        LaborStandardId::new(standard_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(command.facility_id),
            owner_id: command.inventory_owner_id,
            aggregate_type: "standard",
            aggregate_id: standard_id,
            transition: "configured",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn clock_in(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ClockInCommand,
) -> AppResult<AttendanceIntervalReadModel> {
    let prepared = PreparedCommand::new_v1(context, CLOCK_IN_OPERATION, command)?;
    let (mut tx, scope) = begin_scoped(db, access, context).await?;
    require_self_or_supervisor_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command.employee_id,
    )
    .await?;
    require_facility(&scope, command.facility_id)?;
    if let Some(result) = prepared
        .replayed::<AttendanceIntervalReadModel>(&mut tx)
        .await?
    {
        require_facility(&scope, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        &format!(
            "labor-attendance:{}:{}",
            access.tenant_id, command.employee_id
        ),
    )
    .await?;
    let now = now_iso();
    if !employee_is_active_in_facility_tx(
        &mut tx,
        access.tenant_id,
        command.employee_id,
        command.facility_id,
        now,
    )
    .await?
    {
        return Err(AppError::not_found("employee"));
    }
    let attendance_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO attendance_intervals
          (tenant_id,employee_id,facility_id,status,clocked_in_at,clock_in_note,
           clocked_in_by_user_id)
          VALUES($1,$2,$3,'open',$4,$5,$6) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.employee_id.get())
    .bind(command.facility_id.get())
    .bind(now)
    .bind(command.note.as_ref().map(|value| value.as_str()))
    .bind(context.actor_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let result = read_attendance_tx(
        &mut tx,
        access.tenant_id,
        AttendanceIntervalId::new(attendance_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(command.facility_id),
            owner_id: None,
            aggregate_type: "attendance",
            aggregate_id: attendance_id,
            transition: "clocked_in",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn clock_out(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ClockOutCommand,
) -> AppResult<AttendanceIntervalReadModel> {
    let prepared = PreparedCommand::new_v1(context, CLOCK_OUT_OPERATION, command)?;
    let (mut tx, scope) = begin_scoped(db, access, context).await?;
    let authorization_row = sqlx::query(
        "SELECT employee_id,facility_id FROM attendance_intervals WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(command.attendance_interval_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("attendance interval"))?;
    let authorization_employee =
        EmployeeId::new(authorization_row.try_get("employee_id")?).map_err(internal)?;
    let authorization_facility =
        FacilityId::new(authorization_row.try_get("facility_id")?).map_err(internal)?;
    require_facility(&scope, authorization_facility)?;
    require_self_or_supervisor_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        authorization_employee,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AttendanceIntervalReadModel>(&mut tx)
        .await?
    {
        require_facility(&scope, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        r#"SELECT facility_id,status,revision,clocked_in_at FROM attendance_intervals
          WHERE tenant_id=$1 AND id=$2 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.attendance_interval_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("attendance interval"))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?).map_err(internal)?;
    require_facility(&scope, facility_id)?;
    if row.try_get::<i64, _>("revision")? != command.expected_revision.get() {
        return Err(AppError::conflict("attendance interval revision is stale"));
    }
    let status = parse_attendance_status(row.try_get("status")?)?;
    let clocked_in_at: Timestamp = row.try_get("clocked_in_at")?;
    let active: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM labor_activities
          WHERE tenant_id=$1 AND attendance_interval_id=$2 AND status='active')"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.attendance_interval_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let now = now_iso();
    let paid_seconds = validate_attendance_close(status, clocked_in_at, now, active)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    sqlx::query(
        r#"UPDATE attendance_intervals SET status='closed',revision=revision+1,
          clocked_out_at=$3,paid_seconds=$4,clock_out_note=$5,clocked_out_by_user_id=$6
          WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.attendance_interval_id.get())
    .bind(now)
    .bind(paid_seconds)
    .bind(command.note.as_ref().map(|value| value.as_str()))
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let result =
        read_attendance_tx(&mut tx, access.tenant_id, command.attendance_interval_id).await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(facility_id),
            owner_id: None,
            aggregate_type: "attendance",
            aggregate_id: command.attendance_interval_id.get(),
            transition: "clocked_out",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn start_activity(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &StartLaborActivityCommand,
) -> AppResult<LaborActivityReadModel> {
    if command.activity_kind.is_direct()
        && command.inventory_owner_id.is_none()
        && command.activity_kind != LaborActivityKind::CycleCount
    {
        return Err(AppError::bad_request(
            "direct labor requires an inventory owner",
        ));
    }
    if !command.activity_kind.is_direct() && command.inventory_owner_id.is_some() {
        return Err(AppError::bad_request(
            "indirect labor cannot carry an inventory owner",
        ));
    }
    if command.activity_kind.is_direct() != command.quantity_basis.is_some() {
        return Err(AppError::bad_request(
            "direct labor requires a quantity basis and indirect labor forbids one",
        ));
    }
    if command
        .quantity_basis
        .is_some_and(|basis| !command.activity_kind.supports_quantity_basis(basis))
    {
        return Err(AppError::bad_request(
            "labor quantity basis is not supported by this activity kind",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, START_LABOR_ACTIVITY_OPERATION, command)?;
    let (mut tx, scope) = begin_scoped(db, access, context).await?;
    let authorization_row = sqlx::query(
        "SELECT employee_id,facility_id FROM attendance_intervals WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(command.attendance_interval_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("attendance interval"))?;
    let authorization_employee =
        EmployeeId::new(authorization_row.try_get("employee_id")?).map_err(internal)?;
    let authorization_facility =
        FacilityId::new(authorization_row.try_get("facility_id")?).map_err(internal)?;
    require_scope(&scope, authorization_facility, command.inventory_owner_id)?;
    require_self_or_supervisor_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        authorization_employee,
    )
    .await?;
    if let Some(result) = prepared.replayed::<LaborActivityReadModel>(&mut tx).await? {
        require_scope(&scope, result.facility_id, result.inventory_owner_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    let attendance =
        lock_attendance_tx(&mut tx, access.tenant_id, command.attendance_interval_id).await?;
    require_scope(&scope, attendance.facility_id, command.inventory_owner_id)?;
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM labor_activities WHERE tenant_id=$1 AND employee_id=$2 AND status='active')",
    )
    .bind(access.tenant_id.get())
    .bind(attendance.employee_id.get())
    .fetch_one(&mut *tx)
    .await?;
    validate_labor_start(
        attendance.status,
        active,
        &StartLaborActivity {
            kind: command.activity_kind,
            reference_type: command.reference_type.clone(),
            reference_id: command.reference_id,
        },
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    let now = now_iso();
    let employee_active = employee_is_active_in_facility_tx(
        &mut tx,
        access.tenant_id,
        attendance.employee_id,
        attendance.facility_id,
        now,
    )
    .await?;
    if !employee_active {
        return Err(AppError::not_found("eligible employee"));
    }
    if let Some(owner_id) = command.inventory_owner_id {
        if !owner_facility_exists_tx(&mut tx, access.tenant_id, owner_id, attendance.facility_id)
            .await?
        {
            return Err(AppError::not_found("inventory owner facility assignment"));
        }
        let reference_type = command
            .reference_type
            .as_ref()
            .ok_or_else(|| AppError::bad_request("direct labor requires a reference type"))?;
        let reference_id = command
            .reference_id
            .ok_or_else(|| AppError::bad_request("direct labor requires a reference ID"))?;
        let reference = LaborReference {
            tenant_id: access.tenant_id,
            employee_id: attendance.employee_id,
            facility_id: attendance.facility_id,
            kind: command.activity_kind,
            reference_type: reference_type.as_str(),
            reference_id,
            at: now,
        };
        if !direct_reference_exists_tx(&mut tx, owner_id, &reference).await? {
            return Err(AppError::not_found("labor work reference"));
        }
    } else if command.activity_kind.is_direct() {
        let reference_type = command
            .reference_type
            .as_ref()
            .ok_or_else(|| AppError::bad_request("direct labor requires a reference type"))?;
        let reference_id = command
            .reference_id
            .ok_or_else(|| AppError::bad_request("direct labor requires a reference ID"))?;
        let reference = LaborReference {
            tenant_id: access.tenant_id,
            employee_id: attendance.employee_id,
            facility_id: attendance.facility_id,
            kind: command.activity_kind,
            reference_type: reference_type.as_str(),
            reference_id,
            at: now,
        };
        if !facility_shared_reference_exists_tx(&mut tx, &reference).await? {
            return Err(AppError::not_found("facility-shared labor work reference"));
        }
    }
    let reference_quantity = if command.activity_kind.is_direct() {
        Some(
            reference_quantity_tx(
                &mut tx,
                access.tenant_id,
                command.activity_kind,
                command.quantity_basis.ok_or_else(|| {
                    AppError::bad_request("direct labor requires a quantity basis")
                })?,
                command
                    .reference_id
                    .ok_or_else(|| AppError::bad_request("direct labor requires a reference ID"))?,
            )
            .await?,
        )
    } else {
        None
    };
    let standard = match command.labor_standard_id {
        Some(standard_id) => Some(
            load_standard_snapshot_tx(
                &mut tx,
                access.tenant_id,
                standard_id,
                attendance.facility_id,
                command.inventory_owner_id,
                command.activity_kind,
                now,
            )
            .await?,
        ),
        None => None,
    };
    if let Some(standard) = &standard {
        if Some(standard.quantity_basis) != command.quantity_basis {
            return Err(AppError::bad_request(
                "labor activity quantity basis does not match its standard",
            ));
        }
    }
    let required_class_id = standard
        .as_ref()
        .and_then(|standard| standard.required_equipment_class_id);
    let equipment = match command.equipment_asset_id {
        Some(asset_id) => Some(lock_equipment_tx(&mut tx, access.tenant_id, asset_id).await?),
        None => None,
    };
    let equipment_present = equipment.is_some();
    let equipment_available = equipment
        .as_ref()
        .is_none_or(|equipment| equipment.status == EquipmentStatus::Available);
    let equipment_in_facility = equipment
        .as_ref()
        .is_none_or(|equipment| equipment.facility_id == attendance.facility_id);
    let equipment_class_matches = match (required_class_id, equipment.as_ref()) {
        (Some(required), Some(equipment)) => equipment.class_id == required,
        (Some(_), None) => false,
        (None, _) => true,
    };
    let standard_skill_id = standard
        .as_ref()
        .and_then(|standard| standard.required_skill_id);
    let equipment_skill_id = equipment
        .as_ref()
        .and_then(|equipment| equipment.class_required_skill_id);
    let standard_skill_active = match standard_skill_id {
        Some(skill_id) => active_skill_exists_tx(&mut tx, access.tenant_id, skill_id).await?,
        None => true,
    };
    let equipment_skill_active = match equipment_skill_id {
        Some(skill_id) => active_skill_exists_tx(&mut tx, access.tenant_id, skill_id).await?,
        None => true,
    };
    let standard_certification_id = match standard_skill_id {
        Some(skill_id) => {
            certification_evidence_tx(
                &mut tx,
                access.tenant_id,
                attendance.employee_id,
                attendance.facility_id,
                skill_id,
                now,
            )
            .await?
        }
        None => None,
    };
    let equipment_certification_id = match equipment_skill_id {
        Some(skill_id) if Some(skill_id) != standard_skill_id => {
            certification_evidence_tx(
                &mut tx,
                access.tenant_id,
                attendance.employee_id,
                attendance.facility_id,
                skill_id,
                now,
            )
            .await?
        }
        Some(_) => standard_certification_id,
        None => None,
    };
    assess_eligibility(EligibilityEvidence {
        employee_active,
        facility_assigned: employee_active,
        attendance_open: attendance.status == AttendanceStatus::Open,
        required_skill_present: standard_skill_active && equipment_skill_active,
        certification_valid: (standard_skill_id.is_none() || standard_certification_id.is_some())
            && (equipment_skill_id.is_none() || equipment_certification_id.is_some()),
        equipment_required: required_class_id.is_some(),
        equipment_present,
        equipment_available,
        equipment_in_facility,
        equipment_class_matches,
    })
    .map_err(|error| AppError::conflict(error.to_string()))?;
    let activity_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_activities
          (tenant_id,attendance_interval_id,employee_id,facility_id,inventory_owner_id,
           activity_kind,quantity_basis,status,labor_standard_id,equipment_asset_id,reference_type,reference_id,
           reference_quantity,
           standard_setup_seconds,standard_seconds_per_unit,required_skill_id,
           required_skill_certification_id,required_equipment_class_id,
           equipment_required_skill_id,equipment_skill_certification_id,
           started_at,started_by_user_id,note)
          VALUES($1,$2,$3,$4,$5,$6,$7,'active',$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
          RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.attendance_interval_id.get())
    .bind(attendance.employee_id.get())
    .bind(attendance.facility_id.get())
    .bind(command.inventory_owner_id.map(|id| id.get()))
    .bind(command.activity_kind.as_str())
    .bind(command.quantity_basis.map(|basis| basis.as_str()))
    .bind(standard.as_ref().map(|value| value.standard_id.get()))
    .bind(equipment.as_ref().map(|value| value.asset_id.get()))
    .bind(command.reference_type.as_ref().map(|value| value.as_str()))
    .bind(command.reference_id)
    .bind(reference_quantity)
    .bind(standard.as_ref().map(|value| value.setup_seconds))
    .bind(standard.as_ref().map(|value| value.seconds_per_unit))
    .bind(standard_skill_id.map(|id| id.get()))
    .bind(standard_certification_id.map(|id| id.get()))
    .bind(required_class_id.map(|id| id.get()))
    .bind(equipment_skill_id.map(|id| id.get()))
    .bind(equipment_certification_id.map(|id| id.get()))
    .bind(now)
    .bind(context.actor_id.get())
    .bind(command.note.as_ref().map(|value| value.as_str()))
    .fetch_one(&mut *tx)
    .await?;
    if let Some(equipment) = &equipment {
        sqlx::query(
            r#"UPDATE equipment_assets SET status='assigned',assigned_employee_id=$3,
              revision=revision+1,status_note='assigned by labor activity',
              status_changed_by_user_id=$4,status_changed_at=$5
              WHERE tenant_id=$1 AND id=$2"#,
        )
        .bind(access.tenant_id.get())
        .bind(equipment.asset_id.get())
        .bind(attendance.employee_id.get())
        .bind(context.actor_id.get())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let equipment_result =
            read_equipment_asset_tx(&mut tx, access.tenant_id, equipment.asset_id).await?;
        enqueue_event_tx(
            &mut tx,
            LaborOutboxEvent {
                tenant_id: access.tenant_id,
                actor_id: context.actor_id,
                facility_id: Some(attendance.facility_id),
                owner_id: command.inventory_owner_id,
                aggregate_type: "equipment",
                aggregate_id: equipment.asset_id.get(),
                transition: "assigned",
                occurred_at: now,
            },
            &equipment_result,
        )
        .await?;
    }
    let result = read_activity_tx(
        &mut tx,
        access.tenant_id,
        LaborActivityId::new(activity_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(attendance.facility_id),
            owner_id: command.inventory_owner_id,
            aggregate_type: "activity",
            aggregate_id: activity_id,
            transition: "started",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

struct ActivityLock {
    employee_id: EmployeeId,
    facility_id: FacilityId,
    owner_id: Option<InventoryOwnerId>,
    kind: LaborActivityKind,
    status: LaborActivityStatus,
    revision: LaborRevision,
    started_at: Timestamp,
    equipment_asset_id: Option<EquipmentAssetId>,
    quantity_basis: Option<wareboxes_domain::LaborQuantityBasis>,
    reference_type: Option<String>,
    reference_id: Option<i64>,
    reference_quantity: Option<i64>,
    standard: Option<LaborStandard>,
}

async fn lock_activity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    activity_id: LaborActivityId,
) -> AppResult<ActivityLock> {
    let attendance_id: i64 = sqlx::query_scalar(
        "SELECT attendance_interval_id FROM labor_activities WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(activity_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("labor activity"))?;
    let attendance_id = AttendanceIntervalId::new(attendance_id).map_err(internal)?;
    let _attendance = lock_attendance_tx(tx, tenant_id, attendance_id).await?;
    let row = sqlx::query(
        r#"SELECT employee_id,facility_id,inventory_owner_id,
          activity_kind,status,revision,started_at,equipment_asset_id,quantity_basis,
          reference_type,reference_id,reference_quantity,
          standard_setup_seconds,standard_seconds_per_unit
          FROM labor_activities WHERE tenant_id=$1 AND id=$2 FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(activity_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("labor activity"))?;
    let setup_seconds: Option<i64> = row.try_get("standard_setup_seconds")?;
    let seconds_per_unit: Option<i64> = row.try_get("standard_seconds_per_unit")?;
    let standard = match (setup_seconds, seconds_per_unit) {
        (Some(setup_seconds), Some(seconds_per_unit)) => {
            Some(LaborStandard::new(setup_seconds, seconds_per_unit).map_err(internal)?)
        }
        (None, None) => None,
        _ => {
            return Err(AppError::internal(
                "incomplete stored labor standard snapshot",
            ))
        }
    };
    Ok(ActivityLock {
        employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        owner_id: row
            .try_get::<Option<i64>, _>("inventory_owner_id")?
            .map(InventoryOwnerId::new)
            .transpose()
            .map_err(internal)?,
        kind: parse_activity_kind(row.try_get("activity_kind")?)?,
        status: parse_activity_status(row.try_get("status")?)?,
        revision: LaborRevision::new(row.try_get("revision")?).map_err(internal)?,
        started_at: row.try_get("started_at")?,
        equipment_asset_id: row
            .try_get::<Option<i64>, _>("equipment_asset_id")?
            .map(EquipmentAssetId::new)
            .transpose()
            .map_err(internal)?,
        quantity_basis: row
            .try_get::<Option<String>, _>("quantity_basis")?
            .map(|value| {
                wareboxes_domain::LaborQuantityBasis::parse(&value)
                    .ok_or_else(|| AppError::internal("invalid stored labor quantity basis"))
            })
            .transpose()?,
        reference_type: row.try_get("reference_type")?,
        reference_id: row.try_get("reference_id")?,
        reference_quantity: row.try_get("reference_quantity")?,
        standard,
    })
}

async fn release_equipment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    context: &CommandContext,
    activity: &ActivityLock,
    owner_id: Option<InventoryOwnerId>,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let Some(asset_id) = activity.equipment_asset_id else {
        return Ok(());
    };
    let affected = sqlx::query(
        r#"UPDATE equipment_assets SET status='available',assigned_employee_id=NULL,
          revision=revision+1,status_note='released by labor activity',
          status_changed_by_user_id=$4,status_changed_at=$5
          WHERE tenant_id=$1 AND id=$2 AND status='assigned' AND assigned_employee_id=$3"#,
    )
    .bind(tenant_id.get())
    .bind(asset_id.get())
    .bind(activity.employee_id.get())
    .bind(context.actor_id.get())
    .bind(occurred_at)
    .execute(&mut **tx)
    .await?;
    if affected.rows_affected() != 1 {
        return Err(AppError::conflict(
            "labor equipment assignment no longer matches activity",
        ));
    }
    let result = read_equipment_asset_tx(tx, tenant_id, asset_id).await?;
    enqueue_event_tx(
        tx,
        LaborOutboxEvent {
            tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(activity.facility_id),
            owner_id,
            aggregate_type: "equipment",
            aggregate_id: asset_id.get(),
            transition: "released",
            occurred_at,
        },
        &result,
    )
    .await
}

async fn authorize_activity_command_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &ScopeBindings,
    tenant_id: TenantId,
    actor_id: i64,
    activity_id: LaborActivityId,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"SELECT employee_id,facility_id,inventory_owner_id FROM labor_activities
          WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(activity_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("labor activity"))?;
    let employee_id = EmployeeId::new(row.try_get("employee_id")?).map_err(internal)?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?).map_err(internal)?;
    let owner_id = row
        .try_get::<Option<i64>, _>("inventory_owner_id")?
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(internal)?;
    require_scope(scope, facility_id, owner_id)?;
    require_self_or_supervisor_tx(tx, tenant_id, actor_id, employee_id).await
}

pub async fn complete_activity(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CompleteLaborActivityCommand,
) -> AppResult<LaborActivityReadModel> {
    if command.exception_seconds < 0 {
        return Err(AppError::bad_request(
            "labor exception seconds cannot be negative",
        ));
    }
    if (command.exception_seconds == 0)
        != (command.exception_reason.is_none() && command.exception_note.is_none())
    {
        return Err(AppError::bad_request(
            "nonzero labor exception time requires a typed reason and note; zero forbids them",
        ));
    }
    if command.exception_seconds > 0
        && (command.exception_reason.is_none() || command.exception_note.is_none())
    {
        return Err(AppError::bad_request(
            "nonzero labor exception time requires a typed reason and note",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, COMPLETE_LABOR_ACTIVITY_OPERATION, command)?;
    let (mut tx, scope) = begin_scoped(db, access, context).await?;
    if command.exception_seconds > 0 {
        require_permission_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            SUPERVISE_PERMISSION,
        )
        .await?;
    }
    authorize_activity_command_tx(
        &mut tx,
        &scope,
        access.tenant_id,
        context.actor_id.get(),
        command.labor_activity_id,
    )
    .await?;
    if let Some(result) = prepared.replayed::<LaborActivityReadModel>(&mut tx).await? {
        require_scope(&scope, result.facility_id, result.inventory_owner_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    let activity = lock_activity_tx(&mut tx, access.tenant_id, command.labor_activity_id).await?;
    require_scope(&scope, activity.facility_id, activity.owner_id)?;
    if activity.revision != command.expected_revision {
        return Err(AppError::conflict("labor activity revision is stale"));
    }
    if activity.kind.is_direct() {
        let reference_type = activity
            .reference_type
            .as_deref()
            .ok_or_else(|| AppError::internal("direct labor reference type is missing"))?;
        let reference_id = activity
            .reference_id
            .ok_or_else(|| AppError::internal("direct labor reference ID is missing"))?;
        let quantity_basis = activity
            .quantity_basis
            .ok_or_else(|| AppError::internal("direct labor quantity basis is missing"))?;
        let reference_quantity = activity
            .reference_quantity
            .ok_or_else(|| AppError::internal("direct labor reference quantity is missing"))?;
        lock_key_tx(
            &mut tx,
            &format!(
                "labor_reference:{}:{reference_type}:{reference_id}",
                access.tenant_id
            ),
        )
        .await?;
        let previously_reported: i64 = sqlx::query_scalar(
            r#"SELECT COALESCE(SUM(completed_quantity),0)::BIGINT
              FROM labor_activities WHERE tenant_id=$1 AND id<>$2
                AND facility_id=$3 AND inventory_owner_id IS NOT DISTINCT FROM $4
                AND activity_kind=$5 AND quantity_basis=$6 AND reference_type=$7
                AND reference_id=$8 AND status='completed'"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.labor_activity_id.get())
        .bind(activity.facility_id.get())
        .bind(activity.owner_id.map(|id| id.get()))
        .bind(activity.kind.as_str())
        .bind(quantity_basis.as_str())
        .bind(reference_type)
        .bind(reference_id)
        .fetch_one(&mut *tx)
        .await?;
        let reported = command
            .quantity
            .ok_or_else(|| AppError::bad_request("direct labor completion requires quantity"))?
            .get();
        if previously_reported
            .checked_add(reported)
            .is_none_or(|total| total > reference_quantity)
        {
            return Err(AppError::conflict(
                "reported labor quantity exceeds canonical work evidence",
            ));
        }
    }
    let now = now_iso();
    let actual_seconds = validate_labor_completion(
        activity.status,
        activity.kind,
        activity.started_at,
        now,
        command.quantity,
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    if command.exception_seconds > actual_seconds {
        return Err(AppError::bad_request(
            "labor exception seconds cannot exceed actual seconds",
        ));
    }
    let expected_seconds = match (activity.standard, command.quantity) {
        (Some(standard), Some(quantity)) => Some(
            standard
                .expected_seconds(quantity)
                .map_err(|error| AppError::bad_request(error.to_string()))?,
        ),
        (Some(_), None) => {
            return Err(AppError::bad_request(
                "standardized labor completion requires quantity",
            ));
        }
        (None, _) => None,
    };
    let efficiency = expected_seconds
        .map(|expected| efficiency_basis_points(expected, actual_seconds))
        .transpose()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    sqlx::query(
        r#"UPDATE labor_activities SET status='completed',revision=revision+1,
          completed_at=$3,actual_seconds=$4,exception_seconds=$5,exception_reason=$6,
          exception_note=$7,exception_approved_by_user_id=$8,completed_quantity=$9,
          expected_seconds=$10,efficiency_basis_points=$11,completed_by_user_id=$12,
          note=COALESCE($13,note) WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.labor_activity_id.get())
    .bind(now)
    .bind(actual_seconds)
    .bind(command.exception_seconds)
    .bind(command.exception_reason.map(|reason| reason.as_str()))
    .bind(command.exception_note.as_ref().map(|note| note.as_str()))
    .bind((command.exception_seconds > 0).then_some(context.actor_id.get()))
    .bind(command.quantity.map(|quantity| quantity.get()))
    .bind(expected_seconds)
    .bind(efficiency)
    .bind(context.actor_id.get())
    .bind(command.note.as_ref().map(|note| note.as_str()))
    .execute(&mut *tx)
    .await?;
    release_equipment_tx(
        &mut tx,
        access.tenant_id,
        context,
        &activity,
        activity.owner_id,
        now,
    )
    .await?;
    let result = read_activity_tx(&mut tx, access.tenant_id, command.labor_activity_id).await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(activity.facility_id),
            owner_id: activity.owner_id,
            aggregate_type: "activity",
            aggregate_id: command.labor_activity_id.get(),
            transition: "completed",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn cancel_activity(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelLaborActivityCommand,
) -> AppResult<LaborActivityReadModel> {
    let prepared = PreparedCommand::new_v1(context, CANCEL_LABOR_ACTIVITY_OPERATION, command)?;
    let (mut tx, scope) = begin_scoped(db, access, context).await?;
    authorize_activity_command_tx(
        &mut tx,
        &scope,
        access.tenant_id,
        context.actor_id.get(),
        command.labor_activity_id,
    )
    .await?;
    if let Some(result) = prepared.replayed::<LaborActivityReadModel>(&mut tx).await? {
        require_scope(&scope, result.facility_id, result.inventory_owner_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    let activity = lock_activity_tx(&mut tx, access.tenant_id, command.labor_activity_id).await?;
    require_scope(&scope, activity.facility_id, activity.owner_id)?;
    if activity.revision != command.expected_revision {
        return Err(AppError::conflict("labor activity revision is stale"));
    }
    if activity.status != LaborActivityStatus::Active {
        return Err(AppError::conflict("labor activity is not active"));
    }
    let now = now_iso();
    let actual_seconds = (now - activity.started_at).num_seconds();
    if actual_seconds <= 0 {
        return Err(AppError::conflict(
            "labor activity must have positive duration",
        ));
    }
    sqlx::query(
        r#"UPDATE labor_activities SET status='cancelled',revision=revision+1,
          completed_at=$3,actual_seconds=$4,cancelled_by_user_id=$5,note=$6
          WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.labor_activity_id.get())
    .bind(now)
    .bind(actual_seconds)
    .bind(context.actor_id.get())
    .bind(command.note.as_str())
    .execute(&mut *tx)
    .await?;
    release_equipment_tx(
        &mut tx,
        access.tenant_id,
        context,
        &activity,
        activity.owner_id,
        now,
    )
    .await?;
    let result = read_activity_tx(&mut tx, access.tenant_id, command.labor_activity_id).await?;
    enqueue_event_tx(
        &mut tx,
        LaborOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            facility_id: Some(activity.facility_id),
            owner_id: activity.owner_id,
            aggregate_type: "activity",
            aggregate_id: command.labor_activity_id.get(),
            transition: "cancelled",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
