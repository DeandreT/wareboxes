//! Tenant-scoped, explainable advisory planning over canonical work tasks.

mod dispatch;
mod events;
mod query;
mod scope;
mod workers;

pub use dispatch::{activate_dispatch, cancel_dispatch};
use events::{enqueue_event_tx, OrchestrationEvent};
pub use query::{plan_by_id, plan_page, policy_page, signal_workspace};
use scope::{
    bind_actor_tx, invalid_data, require_command_scope, require_facility_scope,
    require_owner_facility_tx,
};
pub use workers::worker_page;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::work_orchestration::{
    ConfigureWorkOrchestrationPolicyCommand, ConfigureWorkOrchestrationPolicyResult,
    GenerateWorkOrchestrationPlanCommand, GenerateWorkOrchestrationPlanResult,
    RecordResourceCapacityCommand, RecordResourceCapacityResult, RecordZoneCongestionCommand,
    RecordZoneCongestionResult, ResourceCapacitySignalReadModel, WorkOrchestrationPolicyReadModel,
    ZoneCongestionSignalReadModel, CONFIGURE_WORK_ORCHESTRATION_POLICY_OPERATION,
    GENERATE_WORK_ORCHESTRATION_PLAN_OPERATION, RECORD_RESOURCE_CAPACITY_OPERATION,
    RECORD_ZONE_CONGESTION_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    due_urgency, orchestration_plan_mode, proximity_basis_points,
    resource_utilization_basis_points, score_orchestration_candidate, FacilityId, InventoryOwnerId,
    LocationId, OrchestrationPlanMode, OrchestrationScore, OrchestrationScoreEvidence,
    OrchestrationWorkKind, TenantId, Timestamp, UserId, WorkOrchestrationPlanId,
    WorkOrchestrationPolicyId, WorkOrchestrationPolicyRevision, WorkOrchestrationSignalId,
    WorkResourceKind,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

const SUPERVISOR_PERMISSION: &str = "wms_supervisor";

fn bad_domain(error: impl std::fmt::Display) -> AppError {
    AppError::bad_request(error.to_string())
}

async fn read_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: WorkOrchestrationPolicyId,
) -> AppResult<WorkOrchestrationPolicyReadModel> {
    let row = sqlx::query("SELECT * FROM work_orchestration_policies WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(policy_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("work orchestration policy"))?;
    query::policy_from_row(&row)
}

pub async fn configure_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureWorkOrchestrationPolicyCommand,
) -> AppResult<ConfigureWorkOrchestrationPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    command.definition.validate().map_err(bad_domain)?;
    if command.definition.tenant_id != access.tenant_id {
        return Err(AppError::not_found("work orchestration policy"));
    }
    let prepared = PreparedCommand::new_v1(
        context,
        CONFIGURE_WORK_ORCHESTRATION_POLICY_OPERATION,
        command,
    )?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    bind_actor_tx(&mut tx, context.actor_id).await?;
    require_command_scope(
        &scope,
        command.definition.facility_id,
        command.definition.inventory_owner_id,
        "work orchestration policy",
    )?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    require_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        command.definition.facility_id,
        command.definition.inventory_owner_id,
        "work orchestration policy",
    )
    .await?;
    let scope_key = command
        .definition
        .inventory_owner_id
        .map_or_else(|| "facility".to_owned(), |id| id.get().to_string());
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "work-orchestration-policy:{}:{}:{scope_key}",
            access.tenant_id.get(),
            command.definition.facility_id.get()
        ))
        .execute(&mut *tx)
        .await?;
    let latest = sqlx::query(
        r#"SELECT id,revision,effective_to FROM work_orchestration_policies
        WHERE tenant_id=$1 AND facility_id=$2
          AND inventory_owner_id IS NOT DISTINCT FROM $3
        ORDER BY revision DESC,id DESC LIMIT 1 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.definition.facility_id.get())
    .bind(
        command
            .definition
            .inventory_owner_id
            .map(InventoryOwnerId::get),
    )
    .fetch_optional(&mut *tx)
    .await?;
    let (predecessor_id, revision) = match (latest.as_ref(), command.expected_revision) {
        (None, None) => (
            None,
            WorkOrchestrationPolicyRevision::new(1).map_err(invalid_data)?,
        ),
        (Some(row), Some(expected))
            if row
                .try_get::<Option<Timestamp>, _>("effective_to")?
                .is_none()
                && row.try_get::<i64, _>("revision")? == expected.get() =>
        {
            let current = WorkOrchestrationPolicyRevision::new(row.try_get("revision")?)
                .map_err(invalid_data)?;
            (
                Some(row.try_get::<i64, _>("id")?),
                current.checked_next().ok_or_else(|| {
                    AppError::internal("work orchestration policy revision overflow")
                })?,
            )
        }
        (Some(_), None) => {
            return Err(AppError::conflict(
                "work orchestration policy already exists",
            ))
        }
        _ => {
            return Err(AppError::conflict(
                "work orchestration policy revision does not match expected revision",
            ))
        }
    };
    let configured_at = now_iso();
    if let Some(predecessor_id) = predecessor_id {
        sqlx::query(
            "UPDATE work_orchestration_policies SET effective_to=$3 WHERE tenant_id=$1 AND id=$2 AND effective_to IS NULL",
        )
        .bind(access.tenant_id.get())
        .bind(predecessor_id)
        .bind(configured_at)
        .execute(&mut *tx)
        .await?;
    }
    let policy_id = WorkOrchestrationPolicyId::new(
        sqlx::query_scalar(
            r#"INSERT INTO work_orchestration_policies (
              tenant_id,facility_id,inventory_owner_id,mode,priority_weight,
              due_urgency_weight,proximity_weight,interleaving_weight,
              congestion_penalty_weight,bottleneck_penalty_weight,due_horizon_minutes,
              max_candidates,revision,supersedes_policy_id,effective_from,
              configured_by_user_id,configured_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$15)
            RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.definition.facility_id.get())
        .bind(
            command
                .definition
                .inventory_owner_id
                .map(InventoryOwnerId::get),
        )
        .bind(command.definition.mode.as_str())
        .bind(i64::from(command.definition.priority_weight))
        .bind(i64::from(command.definition.due_urgency_weight))
        .bind(i64::from(command.definition.proximity_weight))
        .bind(i64::from(command.definition.interleaving_weight))
        .bind(i64::from(command.definition.congestion_penalty_weight))
        .bind(i64::from(command.definition.bottleneck_penalty_weight))
        .bind(i64::from(command.definition.due_horizon_minutes))
        .bind(i64::from(command.definition.max_candidates))
        .bind(revision.get())
        .bind(predecessor_id)
        .bind(configured_at)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(invalid_data)?;
    let result = read_policy_tx(&mut tx, access.tenant_id, policy_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        OrchestrationEvent {
            inventory_owner_id: result.definition.inventory_owner_id,
            facility_id: result.definition.facility_id,
            actor_id: context.actor_id,
            aggregate_type: "policy",
            aggregate_id: policy_id.get(),
            ordering_key: format!(
                "work_orchestration_policy:{}:{}",
                result.definition.facility_id.get(),
                result
                    .definition
                    .inventory_owner_id
                    .map_or_else(|| "facility_default".to_owned(), |id| id.get().to_string())
            ),
            transition: "configured",
            occurred_at: configured_at,
            payload: &serde_json::to_value(&result).map_err(invalid_data)?,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn command_tx<'a>(
    db: &'a Db,
    access: &TenantAccess,
    context: &CommandContext,
    facility_id: FacilityId,
    label: &str,
) -> AppResult<(sqlx::Transaction<'a, sqlx::Postgres>, ScopeBindings)> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    bind_actor_tx(&mut tx, context.actor_id).await?;
    require_facility_scope(&scope, facility_id.get(), label)?;
    Ok((tx, scope))
}

pub async fn record_zone_congestion(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RecordZoneCongestionCommand,
) -> AppResult<RecordZoneCongestionResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    command.signal.validate().map_err(bad_domain)?;
    if command.tenant_id != access.tenant_id {
        return Err(AppError::not_found("work orchestration zone signal"));
    }
    let prepared = PreparedCommand::new_v1(context, RECORD_ZONE_CONGESTION_OPERATION, command)?;
    let (mut tx, _) = command_tx(
        db,
        access,
        context,
        command.facility_id,
        "work orchestration zone signal",
    )
    .await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let zone_code: String = sqlx::query_scalar(
        "SELECT code FROM storage_zones WHERE tenant_id=$1 AND facility_id=$2 AND id=$3 AND effective_to IS NULL",
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.storage_zone_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("work orchestration zone signal"))?;
    let observed_at = now_iso();
    let expires_at = observed_at + chrono::Duration::seconds(i64::from(command.signal.ttl_seconds));
    let signal_id = WorkOrchestrationSignalId::new(
        sqlx::query_scalar(
            r#"INSERT INTO work_orchestration_zone_signals (
              tenant_id,facility_id,storage_zone_id,storage_zone_code,
              congestion_basis_points,queue_depth,ttl_seconds,recorded_by_user_id,
              observed_at,expires_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.facility_id.get())
        .bind(command.storage_zone_id.get())
        .bind(zone_code)
        .bind(i64::from(command.signal.congestion_basis_points))
        .bind(command.signal.queue_depth)
        .bind(i64::from(command.signal.ttl_seconds))
        .bind(context.actor_id.get())
        .bind(observed_at)
        .bind(expires_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(invalid_data)?;
    let row =
        sqlx::query("SELECT * FROM work_orchestration_zone_signals WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(signal_id.get())
            .fetch_one(&mut *tx)
            .await?;
    let result: ZoneCongestionSignalReadModel = query::zone_signal_from_row(&row)?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        OrchestrationEvent {
            inventory_owner_id: None,
            facility_id: command.facility_id,
            actor_id: context.actor_id,
            aggregate_type: "zone_signal",
            aggregate_id: signal_id.get(),
            ordering_key: format!(
                "work_orchestration_zone_signal:{}:{}",
                command.facility_id.get(),
                command.storage_zone_id.get()
            ),
            transition: "recorded",
            occurred_at: observed_at,
            payload: &serde_json::to_value(&result).map_err(invalid_data)?,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn record_resource_capacity(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RecordResourceCapacityCommand,
) -> AppResult<RecordResourceCapacityResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    command.signal.validate().map_err(bad_domain)?;
    if command.tenant_id != access.tenant_id {
        return Err(AppError::not_found("work orchestration resource signal"));
    }
    let prepared = PreparedCommand::new_v1(context, RECORD_RESOURCE_CAPACITY_OPERATION, command)?;
    let (mut tx, _) = command_tx(
        db,
        access,
        context,
        command.facility_id,
        "work orchestration resource signal",
    )
    .await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let observed_at = now_iso();
    let expires_at = observed_at + chrono::Duration::seconds(i64::from(command.signal.ttl_seconds));
    let utilization = resource_utilization_basis_points(
        command.signal.available_units,
        command.signal.demand_units,
    );
    let signal_id = WorkOrchestrationSignalId::new(
        sqlx::query_scalar(
            r#"INSERT INTO work_orchestration_resource_signals (
              tenant_id,facility_id,resource_kind,available_units,demand_units,
              utilization_basis_points,ttl_seconds,recorded_by_user_id,observed_at,expires_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.facility_id.get())
        .bind(command.resource_kind.as_str())
        .bind(command.signal.available_units)
        .bind(command.signal.demand_units)
        .bind(i64::from(utilization))
        .bind(i64::from(command.signal.ttl_seconds))
        .bind(context.actor_id.get())
        .bind(observed_at)
        .bind(expires_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(invalid_data)?;
    let row = sqlx::query(
        "SELECT * FROM work_orchestration_resource_signals WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(signal_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let result: ResourceCapacitySignalReadModel = query::resource_signal_from_row(&row)?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        OrchestrationEvent {
            inventory_owner_id: None,
            facility_id: command.facility_id,
            actor_id: context.actor_id,
            aggregate_type: "resource_signal",
            aggregate_id: signal_id.get(),
            ordering_key: format!(
                "work_orchestration_resource_signal:{}:{}",
                command.facility_id.get(),
                command.resource_kind.as_str()
            ),
            transition: "recorded",
            occurred_at: observed_at,
            payload: &serde_json::to_value(&result).map_err(invalid_data)?,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

#[derive(Debug)]
struct Candidate {
    work_task_id: i64,
    work_kind: OrchestrationWorkKind,
    inventory_owner_id: Option<InventoryOwnerId>,
    title: String,
    instructions: Option<String>,
    task_priority: i64,
    due_at: Option<Timestamp>,
    task_created_at: Timestamp,
    source_location_id: LocationId,
    source_location_label: String,
    destination_location_id: Option<LocationId>,
    destination_location_label: Option<String>,
    source_zone_id: Option<i64>,
    source_zone_code: Option<String>,
    source_travel_sequence: i64,
    destination_travel_sequence: Option<i64>,
    zone_signal_id: Option<WorkOrchestrationSignalId>,
    congestion_basis_points: u16,
    congestion_queue_depth: i64,
    resource_kind: WorkResourceKind,
    resource_signal_id: Option<WorkOrchestrationSignalId>,
    resource_available_units: i64,
    resource_demand_units: i64,
    resource_utilization_basis_points: u16,
}

struct ScoredCandidate {
    candidate: Candidate,
    evidence: OrchestrationScoreEvidence,
    score: OrchestrationScore,
}

async fn active_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    inventory_owner_id: Option<InventoryOwnerId>,
    expected_policy_id: WorkOrchestrationPolicyId,
    expected_revision: WorkOrchestrationPolicyRevision,
) -> AppResult<WorkOrchestrationPolicyReadModel> {
    let row = sqlx::query(
        r#"SELECT * FROM work_orchestration_policies
        WHERE tenant_id=$1 AND facility_id=$2 AND effective_to IS NULL
          AND (inventory_owner_id IS NOT DISTINCT FROM $3 OR inventory_owner_id IS NULL)
        ORDER BY (inventory_owner_id IS NOT NULL) DESC LIMIT 1 FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(inventory_owner_id.map(InventoryOwnerId::get))
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("work orchestration policy is not configured"))?;
    let policy = query::policy_from_row(&row)?;
    if policy.policy_id != expected_policy_id || policy.revision != expected_revision {
        return Err(AppError::conflict(
            "resolved work orchestration policy does not match expected policy",
        ));
    }
    Ok(policy)
}

async fn current_location_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    location_id: LocationId,
) -> AppResult<(String, i64)> {
    let row = sqlx::query(
        r#"SELECT COALESCE(NULLIF(location.name,''),location.barcode,
            'Location #'||location.id::text) AS label,
          COALESCE(path.travel_sequence,0)::bigint AS travel_sequence
        FROM locations location
        LEFT JOIN LATERAL (
          SELECT zone.travel_sequence*1000000+member.location_sequence AS travel_sequence
          FROM storage_zone_locations member
          JOIN storage_zones zone ON zone.tenant_id=member.tenant_id
            AND zone.facility_id=member.facility_id AND zone.id=member.storage_zone_id
            AND zone.effective_to IS NULL
          WHERE member.tenant_id=location.tenant_id
            AND member.facility_id=location.facility_id AND member.location_id=location.id
          ORDER BY zone.travel_sequence,member.location_sequence,zone.id LIMIT 1
        ) path ON true
        WHERE location.tenant_id=$1 AND location.facility_id=$2 AND location.id=$3
          AND location.deleted IS NULL AND location.active"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(location_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("work orchestration current location"))?;
    Ok((row.try_get("label")?, row.try_get("travel_sequence")?))
}

async fn candidate_rows_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    inventory_owner_id: Option<InventoryOwnerId>,
    snapshot_at: Timestamp,
    generated_for_user_id: Option<UserId>,
) -> AppResult<Vec<Candidate>> {
    let task_ids: Vec<i64> = sqlx::query_scalar(
        r#"WITH RECURSIVE granted_roles AS (
          SELECT role.id,role.parent_id
          FROM tenant_memberships membership
          JOIN user_roles user_role ON user_role.tenant_id=membership.tenant_id
            AND user_role.user_id=membership.user_id AND user_role.deleted IS NULL
          JOIN roles role ON role.tenant_id=user_role.tenant_id
            AND role.id=user_role.role_id AND role.deleted IS NULL
          WHERE membership.tenant_id=$1 AND membership.user_id=$5
            AND membership.deleted IS NULL
          UNION
          SELECT parent.id,parent.parent_id FROM granted_roles child
          JOIN roles parent ON parent.tenant_id=$1 AND parent.id=child.parent_id
            AND parent.deleted IS NULL
        )
        SELECT task.id FROM work_tasks task
        WHERE task.tenant_id=$1 AND task.facility_id=$2
          AND task.inventory_owner_id IS NOT DISTINCT FROM $3
          AND task.status='open' AND task.deleted IS NULL
          AND task.created<=$4
          AND (task.scheduled_for IS NULL OR task.scheduled_for<=$4)
          AND task.task_type IN ('cycle_count_item_location','cycle_count_location','putaway',
            'license_plate_putaway','inventory_relocation','replenishment','cross_dock')
          AND ($5::bigint IS NULL OR task.task_type<>'cycle_count_location')
          AND ($5::bigint IS NULL OR EXISTS(
            SELECT 1 FROM tenant_memberships membership
            WHERE membership.tenant_id=task.tenant_id AND membership.user_id=$5
              AND membership.deleted IS NULL
              AND (membership.all_facilities OR EXISTS(
                SELECT 1 FROM user_facilities site
                WHERE site.tenant_id=membership.tenant_id
                  AND site.user_id=membership.user_id
                  AND site.facility_id=task.facility_id AND site.deleted IS NULL))
              AND (task.inventory_owner_id IS NULL OR membership.all_inventory_owners
                OR EXISTS(SELECT 1 FROM user_inventory_owners owner_scope
                  WHERE owner_scope.tenant_id=membership.tenant_id
                    AND owner_scope.user_id=membership.user_id
                    AND owner_scope.inventory_owner_id=task.inventory_owner_id
                    AND owner_scope.deleted IS NULL))
              AND EXISTS(SELECT 1 FROM granted_roles role
                JOIN role_permissions role_permission
                  ON role_permission.tenant_id=membership.tenant_id
                  AND role_permission.role_id=role.id AND role_permission.deleted IS NULL
                JOIN permissions permission
                  ON permission.tenant_id=role_permission.tenant_id
                  AND permission.id=role_permission.permission_id
                  AND permission.deleted IS NULL
                WHERE lower(permission.name) IN ('admin',lower(task.required_permission))))
          )
        ORDER BY task.created,task.id"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(inventory_owner_id.map(InventoryOwnerId::get))
    .bind(snapshot_at)
    .bind(generated_for_user_id.map(UserId::get))
    .fetch_all(&mut **tx)
    .await?;
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"WITH typed AS (
          SELECT task.*,
            CASE task.task_type
              WHEN 'cycle_count_item_location' THEN count_item.location_id
              WHEN 'cycle_count_location' THEN count_location.location_id
              WHEN 'putaway' THEN putaway.source_location_id
              WHEN 'license_plate_putaway' THEN plate_putaway.source_location_id
              WHEN 'inventory_relocation' THEN relocation.source_location_id
              WHEN 'replenishment' THEN replenishment.source_location_id
              WHEN 'cross_dock' THEN cross_dock.source_location_id END AS source_location_id,
            CASE task.task_type
              WHEN 'putaway' THEN putaway.destination_location_id
              WHEN 'license_plate_putaway' THEN plate_putaway.destination_location_id
              WHEN 'inventory_relocation' THEN relocation.destination_location_id
              WHEN 'replenishment' THEN replenishment.destination_location_id
              WHEN 'cross_dock' THEN cross_dock.destination_location_id END AS destination_location_id
          FROM work_tasks task
          LEFT JOIN cycle_count_item_location_tasks count_item
            ON count_item.tenant_id=task.tenant_id AND count_item.task_id=task.id
          LEFT JOIN cycle_count_location_tasks count_location
            ON count_location.tenant_id=task.tenant_id AND count_location.task_id=task.id
          LEFT JOIN putaway_tasks putaway
            ON putaway.tenant_id=task.tenant_id AND putaway.task_id=task.id
          LEFT JOIN license_plate_putaway_tasks plate_putaway
            ON plate_putaway.tenant_id=task.tenant_id AND plate_putaway.task_id=task.id
          LEFT JOIN inventory_relocation_tasks relocation
            ON relocation.tenant_id=task.tenant_id AND relocation.task_id=task.id
          LEFT JOIN replenishment_tasks replenishment
            ON replenishment.tenant_id=task.tenant_id AND replenishment.task_id=task.id
          LEFT JOIN cross_dock_tasks cross_dock
            ON cross_dock.tenant_id=task.tenant_id AND cross_dock.task_id=task.id
          WHERE task.tenant_id=$1 AND task.id=ANY($2)
            AND task.status='open' AND task.deleted IS NULL AND task.created<=$3
            AND (task.scheduled_for IS NULL OR task.scheduled_for<=$3)
        )
        SELECT typed.id AS work_task_id,typed.task_type,typed.inventory_owner_id,
          typed.title,typed.instructions,typed.priority,typed.due_at,typed.created,
          typed.source_location_id,
          COALESCE(NULLIF(source.name,''),source.barcode,'Location #'||source.id::text)
            AS source_location_label,
          typed.destination_location_id,
          CASE WHEN destination.id IS NULL THEN NULL ELSE
            COALESCE(NULLIF(destination.name,''),destination.barcode,
              'Location #'||destination.id::text) END AS destination_location_label,
          source_path.storage_zone_id AS source_zone_id,
          source_path.storage_zone_code AS source_zone_code,
          COALESCE(source_path.travel_sequence,0)::bigint AS source_travel_sequence,
          destination_path.travel_sequence::bigint AS destination_travel_sequence,
          zone_signal.id AS zone_signal_id,
          COALESCE(zone_signal.congestion_basis_points,0)::bigint
            AS congestion_basis_points,
          COALESCE(zone_signal.queue_depth,0)::bigint AS congestion_queue_depth,
          CASE WHEN typed.task_type IN ('cycle_count_item_location','cycle_count_location')
            THEN 'inventory_control' ELSE 'material_handling' END AS resource_kind,
          resource_signal.id AS resource_signal_id,
          COALESCE(resource_signal.available_units,0)::bigint AS resource_available_units,
          COALESCE(resource_signal.demand_units,0)::bigint AS resource_demand_units,
          COALESCE(resource_signal.utilization_basis_points,0)::bigint
            AS resource_utilization_basis_points
        FROM typed
        JOIN locations source ON source.tenant_id=typed.tenant_id
          AND source.facility_id=typed.facility_id AND source.id=typed.source_location_id
          AND source.deleted IS NULL AND source.active
        LEFT JOIN locations destination ON destination.tenant_id=typed.tenant_id
          AND destination.facility_id=typed.facility_id
          AND destination.id=typed.destination_location_id
          AND destination.deleted IS NULL AND destination.active
        LEFT JOIN LATERAL (
          SELECT zone.id AS storage_zone_id,zone.code AS storage_zone_code,
            zone.travel_sequence*1000000+member.location_sequence AS travel_sequence
          FROM storage_zone_locations member JOIN storage_zones zone
            ON zone.tenant_id=member.tenant_id AND zone.facility_id=member.facility_id
            AND zone.id=member.storage_zone_id AND zone.effective_to IS NULL
          WHERE member.tenant_id=typed.tenant_id AND member.facility_id=typed.facility_id
            AND member.location_id=typed.source_location_id
          ORDER BY zone.travel_sequence,member.location_sequence,zone.id LIMIT 1
        ) source_path ON true
        LEFT JOIN LATERAL (
          SELECT zone.travel_sequence*1000000+member.location_sequence AS travel_sequence
          FROM storage_zone_locations member JOIN storage_zones zone
            ON zone.tenant_id=member.tenant_id AND zone.facility_id=member.facility_id
            AND zone.id=member.storage_zone_id AND zone.effective_to IS NULL
          WHERE member.tenant_id=typed.tenant_id AND member.facility_id=typed.facility_id
            AND member.location_id=typed.destination_location_id
          ORDER BY zone.travel_sequence,member.location_sequence,zone.id LIMIT 1
        ) destination_path ON true
        LEFT JOIN LATERAL (
          SELECT signal.* FROM work_orchestration_zone_signals signal
          WHERE signal.tenant_id=typed.tenant_id AND signal.facility_id=typed.facility_id
            AND signal.storage_zone_id=source_path.storage_zone_id
            AND signal.observed_at<=$3 AND signal.expires_at>$3
          ORDER BY signal.observed_at DESC,signal.id DESC LIMIT 1
        ) zone_signal ON true
        LEFT JOIN LATERAL (
          SELECT signal.* FROM work_orchestration_resource_signals signal
          WHERE signal.tenant_id=typed.tenant_id AND signal.facility_id=typed.facility_id
            AND signal.resource_kind=CASE WHEN typed.task_type IN
              ('cycle_count_item_location','cycle_count_location')
              THEN 'inventory_control' ELSE 'material_handling' END
            AND signal.observed_at<=$3 AND signal.expires_at>$3
          ORDER BY signal.observed_at DESC,signal.id DESC LIMIT 1
        ) resource_signal ON true
        ORDER BY typed.created,typed.id"#,
    )
    .bind(tenant_id.get())
    .bind(&task_ids)
    .bind(snapshot_at)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Candidate {
                work_task_id: row.try_get("work_task_id")?,
                work_kind: OrchestrationWorkKind::parse(&row.try_get::<String, _>("task_type")?)
                    .ok_or_else(|| AppError::internal("invalid orchestration task kind"))?,
                inventory_owner_id: row
                    .try_get::<Option<i64>, _>("inventory_owner_id")?
                    .map(InventoryOwnerId::new)
                    .transpose()
                    .map_err(invalid_data)?,
                title: row.try_get("title")?,
                instructions: row.try_get("instructions")?,
                task_priority: row.try_get("priority")?,
                due_at: row.try_get("due_at")?,
                task_created_at: row.try_get("created")?,
                source_location_id: LocationId::new(row.try_get("source_location_id")?)
                    .map_err(invalid_data)?,
                source_location_label: row.try_get("source_location_label")?,
                destination_location_id: row
                    .try_get::<Option<i64>, _>("destination_location_id")?
                    .map(LocationId::new)
                    .transpose()
                    .map_err(invalid_data)?,
                destination_location_label: row.try_get("destination_location_label")?,
                source_zone_id: row.try_get("source_zone_id")?,
                source_zone_code: row.try_get("source_zone_code")?,
                source_travel_sequence: row.try_get("source_travel_sequence")?,
                destination_travel_sequence: row.try_get("destination_travel_sequence")?,
                zone_signal_id: row
                    .try_get::<Option<i64>, _>("zone_signal_id")?
                    .map(WorkOrchestrationSignalId::new)
                    .transpose()
                    .map_err(invalid_data)?,
                congestion_basis_points: u16::try_from(
                    row.try_get::<i64, _>("congestion_basis_points")?,
                )
                .map_err(invalid_data)?,
                congestion_queue_depth: row.try_get("congestion_queue_depth")?,
                resource_kind: WorkResourceKind::parse(&row.try_get::<String, _>("resource_kind")?)
                    .ok_or_else(|| AppError::internal("invalid orchestration resource kind"))?,
                resource_signal_id: row
                    .try_get::<Option<i64>, _>("resource_signal_id")?
                    .map(WorkOrchestrationSignalId::new)
                    .transpose()
                    .map_err(invalid_data)?,
                resource_available_units: row.try_get("resource_available_units")?,
                resource_demand_units: row.try_get("resource_demand_units")?,
                resource_utilization_basis_points: u16::try_from(
                    row.try_get::<i64, _>("resource_utilization_basis_points")?,
                )
                .map_err(invalid_data)?,
            })
        })
        .collect()
}

fn score_candidate(
    policy: &WorkOrchestrationPolicyReadModel,
    command: &GenerateWorkOrchestrationPlanCommand,
    current_travel_sequence: i64,
    snapshot_at: Timestamp,
    candidate: Candidate,
) -> AppResult<ScoredCandidate> {
    let (overdue_seconds, due_basis_points) = due_urgency(
        candidate.due_at,
        snapshot_at,
        policy.definition.due_horizon_minutes,
    )
    .map_err(invalid_data)?;
    let travel_distance = current_travel_sequence.abs_diff(candidate.source_travel_sequence);
    let travel_distance = i64::try_from(travel_distance).unwrap_or(i64::MAX);
    let evidence = OrchestrationScoreEvidence {
        work_kind: candidate.work_kind,
        task_priority: candidate.task_priority,
        due_at: candidate.due_at,
        overdue_seconds,
        due_urgency_basis_points: due_basis_points,
        current_location_id: command.current_location_id,
        source_location_id: candidate.source_location_id,
        destination_location_id: candidate.destination_location_id,
        current_travel_sequence,
        source_travel_sequence: candidate.source_travel_sequence,
        destination_travel_sequence: candidate.destination_travel_sequence,
        travel_distance,
        proximity_basis_points: proximity_basis_points(travel_distance).map_err(invalid_data)?,
        previous_work_kind: command.previous_work_kind,
        interleaving_compatible: command
            .previous_work_kind
            .is_some_and(|previous| candidate.work_kind.interleaves_with(previous)),
        source_zone_id: candidate.source_zone_id,
        source_zone_code: candidate.source_zone_code.clone(),
        congestion_basis_points: candidate.congestion_basis_points,
        congestion_queue_depth: candidate.congestion_queue_depth,
        resource_kind: candidate.resource_kind,
        resource_available_units: candidate.resource_available_units,
        resource_demand_units: candidate.resource_demand_units,
        resource_utilization_basis_points: candidate.resource_utilization_basis_points,
    };
    let score =
        if orchestration_plan_mode(policy.definition.mode) == OrchestrationPlanMode::ManualFifo {
            OrchestrationScore {
                priority_component: 0,
                due_urgency_component: 0,
                proximity_component: 0,
                interleaving_component: 0,
                congestion_penalty: 0,
                bottleneck_penalty: 0,
                total: 0,
            }
        } else {
            score_orchestration_candidate(&policy.definition, &evidence).map_err(invalid_data)?
        };
    Ok(ScoredCandidate {
        candidate,
        evidence,
        score,
    })
}

async fn insert_plan_item_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    plan_id: WorkOrchestrationPlanId,
    sequence: u16,
    item: &ScoredCandidate,
) -> AppResult<()> {
    let candidate = &item.candidate;
    let evidence = &item.evidence;
    let score = item.score;
    sqlx::query(
        r#"INSERT INTO work_orchestration_plan_items (
          tenant_id,facility_id,plan_id,sequence,work_task_id,work_kind,inventory_owner_id,
          title,instructions,task_status,task_created_at,source_location_id,
          source_location_label,destination_location_id,destination_location_label,
          task_priority,due_at,overdue_seconds,due_urgency_basis_points,current_location_id,
          current_travel_sequence,source_travel_sequence,destination_travel_sequence,
          travel_distance,proximity_basis_points,previous_work_kind,interleaving_compatible,
          source_zone_id,source_zone_code,zone_signal_id,congestion_basis_points,
          congestion_queue_depth,resource_kind,resource_signal_id,resource_available_units,
          resource_demand_units,resource_utilization_basis_points,priority_score,
          due_urgency_score,proximity_score,interleaving_score,congestion_penalty,
          bottleneck_penalty,total_score
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'open',$10,$11,$12,$13,$14,$15,$16,$17,
          $18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35,
          $36,$37,$38,$39,$40,$41,$42,$43)"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(plan_id.get())
    .bind(i64::from(sequence))
    .bind(candidate.work_task_id)
    .bind(candidate.work_kind.as_str())
    .bind(candidate.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(&candidate.title)
    .bind(&candidate.instructions)
    .bind(candidate.task_created_at)
    .bind(evidence.source_location_id.get())
    .bind(&candidate.source_location_label)
    .bind(evidence.destination_location_id.map(LocationId::get))
    .bind(&candidate.destination_location_label)
    .bind(evidence.task_priority)
    .bind(evidence.due_at)
    .bind(evidence.overdue_seconds)
    .bind(i64::from(evidence.due_urgency_basis_points))
    .bind(evidence.current_location_id.get())
    .bind(evidence.current_travel_sequence)
    .bind(evidence.source_travel_sequence)
    .bind(evidence.destination_travel_sequence)
    .bind(evidence.travel_distance)
    .bind(i64::from(evidence.proximity_basis_points))
    .bind(
        evidence
            .previous_work_kind
            .map(OrchestrationWorkKind::as_str),
    )
    .bind(evidence.interleaving_compatible)
    .bind(evidence.source_zone_id)
    .bind(&evidence.source_zone_code)
    .bind(candidate.zone_signal_id.map(WorkOrchestrationSignalId::get))
    .bind(i64::from(evidence.congestion_basis_points))
    .bind(evidence.congestion_queue_depth)
    .bind(evidence.resource_kind.as_str())
    .bind(
        candidate
            .resource_signal_id
            .map(WorkOrchestrationSignalId::get),
    )
    .bind(evidence.resource_available_units)
    .bind(evidence.resource_demand_units)
    .bind(i64::from(evidence.resource_utilization_basis_points))
    .bind(score.priority_component)
    .bind(score.due_urgency_component)
    .bind(score.proximity_component)
    .bind(score.interleaving_component)
    .bind(score.congestion_penalty)
    .bind(score.bottleneck_penalty)
    .bind(score.total)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn generate_plan(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &GenerateWorkOrchestrationPlanCommand,
) -> AppResult<GenerateWorkOrchestrationPlanResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.tenant_id != access.tenant_id {
        return Err(AppError::not_found("work orchestration plan"));
    }
    let prepared =
        PreparedCommand::new_v1(context, GENERATE_WORK_ORCHESTRATION_PLAN_OPERATION, command)?;
    let (mut tx, scope) = command_tx(
        db,
        access,
        context,
        command.facility_id,
        "work orchestration plan",
    )
    .await?;
    require_command_scope(
        &scope,
        command.facility_id,
        command.inventory_owner_id,
        "work orchestration plan",
    )?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    require_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.inventory_owner_id,
        "work orchestration plan",
    )
    .await?;
    if let Some(user_id) = command.generated_for_user_id {
        let eligible: bool = sqlx::query_scalar(
            r#"WITH RECURSIVE granted_roles AS (
              SELECT role.id,role.parent_id
              FROM tenant_memberships membership
              JOIN user_roles user_role ON user_role.tenant_id=membership.tenant_id
                AND user_role.user_id=membership.user_id AND user_role.deleted IS NULL
              JOIN roles role ON role.tenant_id=user_role.tenant_id
                AND role.id=user_role.role_id AND role.deleted IS NULL
              WHERE membership.tenant_id=$1 AND membership.user_id=$2
                AND membership.deleted IS NULL
              UNION
              SELECT parent.id,parent.parent_id FROM granted_roles child
              JOIN roles parent ON parent.tenant_id=$1 AND parent.id=child.parent_id
                AND parent.deleted IS NULL
            )
            SELECT EXISTS(SELECT 1 FROM employees employee
            JOIN employee_facilities assignment ON assignment.tenant_id=employee.tenant_id
              AND assignment.employee_id=employee.id AND assignment.facility_id=$3
              AND assignment.deleted IS NULL
            JOIN tenant_memberships membership ON membership.tenant_id=employee.tenant_id
              AND membership.user_id=employee.user_id AND membership.deleted IS NULL
            WHERE employee.tenant_id=$1 AND employee.user_id=$2 AND employee.deleted IS NULL
              AND employee.hired<=transaction_timestamp()
              AND (employee.terminated IS NULL OR employee.terminated>transaction_timestamp())
              AND (membership.all_facilities OR EXISTS(
                SELECT 1 FROM user_facilities site
                WHERE site.tenant_id=membership.tenant_id AND site.user_id=membership.user_id
                  AND site.facility_id=$3 AND site.deleted IS NULL))
              AND ($4::bigint IS NULL OR membership.all_inventory_owners OR EXISTS(
                SELECT 1 FROM user_inventory_owners owner_scope
                WHERE owner_scope.tenant_id=membership.tenant_id
                  AND owner_scope.user_id=membership.user_id
                  AND owner_scope.inventory_owner_id=$4 AND owner_scope.deleted IS NULL))
              AND EXISTS(SELECT 1 FROM granted_roles role
                JOIN role_permissions role_permission
                  ON role_permission.tenant_id=membership.tenant_id
                  AND role_permission.role_id=role.id AND role_permission.deleted IS NULL
                JOIN permissions permission
                  ON permission.tenant_id=role_permission.tenant_id
                  AND permission.id=role_permission.permission_id
                  AND permission.deleted IS NULL
                WHERE lower(permission.name) IN ('admin','wms')))"#,
        )
        .bind(access.tenant_id.get())
        .bind(user_id.get())
        .bind(command.facility_id.get())
        .bind(command.inventory_owner_id.map(InventoryOwnerId::get))
        .fetch_one(&mut *tx)
        .await?;
        if !eligible {
            return Err(AppError::not_found("work orchestration worker"));
        }
    }
    let policy = active_policy_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.inventory_owner_id,
        command.expected_policy_id,
        command.expected_policy_revision,
    )
    .await?;
    let (current_location_label, current_travel_sequence) = current_location_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.current_location_id,
    )
    .await?;
    let generated_at = now_iso();
    let candidates = candidate_rows_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.inventory_owner_id,
        generated_at,
        command.generated_for_user_id,
    )
    .await?;
    let mut scored = candidates
        .into_iter()
        .map(|candidate| {
            score_candidate(
                &policy,
                command,
                current_travel_sequence,
                generated_at,
                candidate,
            )
        })
        .collect::<AppResult<Vec<_>>>()?;
    if orchestration_plan_mode(policy.definition.mode) == OrchestrationPlanMode::Optimized {
        scored.sort_by(|left, right| {
            right
                .score
                .total
                .cmp(&left.score.total)
                .then_with(|| {
                    right
                        .evidence
                        .task_priority
                        .cmp(&left.evidence.task_priority)
                })
                .then_with(|| match (left.evidence.due_at, right.evidence.due_at) {
                    (Some(left_due), Some(right_due)) => left_due.cmp(&right_due),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                .then_with(|| {
                    left.candidate
                        .task_created_at
                        .cmp(&right.candidate.task_created_at)
                })
                .then_with(|| {
                    left.candidate
                        .work_task_id
                        .cmp(&right.candidate.work_task_id)
                })
        });
    }
    scored.truncate(usize::from(policy.definition.max_candidates));
    let candidate_count = i64::try_from(scored.len()).map_err(invalid_data)?;
    let item_count = i64::try_from(scored.len()).map_err(invalid_data)?;
    let configuration_snapshot: serde_json::Value = sqlx::query_scalar(
        "SELECT to_jsonb(policy) FROM work_orchestration_policies policy WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(policy.policy_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let plan_mode = orchestration_plan_mode(policy.definition.mode);
    let plan_id = WorkOrchestrationPlanId::new(
        sqlx::query_scalar(
            r#"INSERT INTO work_orchestration_plans (
              tenant_id,facility_id,requested_inventory_owner_id,current_location_id,
              current_location_label,previous_work_kind,generated_for_user_id,policy_id,
              policy_revision,policy_inventory_owner_id,plan_mode,input_snapshot_at,
              configuration_snapshot,candidate_count,item_count,generated_by_user_id,generated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$12)
            RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.facility_id.get())
        .bind(command.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(command.current_location_id.get())
        .bind(&current_location_label)
        .bind(
            command
                .previous_work_kind
                .map(OrchestrationWorkKind::as_str),
        )
        .bind(command.generated_for_user_id.map(UserId::get))
        .bind(policy.policy_id.get())
        .bind(policy.revision.get())
        .bind(
            policy
                .definition
                .inventory_owner_id
                .map(InventoryOwnerId::get),
        )
        .bind(plan_mode.as_str())
        .bind(generated_at)
        .bind(&configuration_snapshot)
        .bind(candidate_count)
        .bind(item_count)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(invalid_data)?;
    for (index, item) in scored.iter().enumerate() {
        let sequence = u16::try_from(index + 1).map_err(invalid_data)?;
        insert_plan_item_tx(
            &mut tx,
            access.tenant_id,
            command.facility_id,
            plan_id,
            sequence,
            item,
        )
        .await?;
    }
    let result = query::read_plan_tx(&mut tx, access.tenant_id, plan_id).await?;
    let payload = serde_json::json!({
        "plan_id": plan_id.get(),
        "facility_id": command.facility_id.get(),
        "inventory_owner_id": command.inventory_owner_id.map(InventoryOwnerId::get),
        "policy_id": policy.policy_id.get(),
        "policy_revision": policy.revision.get(),
        "plan_mode": plan_mode.as_str(),
        "candidate_count": candidate_count,
        "item_count": item_count,
        "generated_for_user_id": command.generated_for_user_id.map(UserId::get),
        "generated_at": generated_at,
    });
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        OrchestrationEvent {
            inventory_owner_id: command.inventory_owner_id,
            facility_id: command.facility_id,
            actor_id: context.actor_id,
            aggregate_type: "plan",
            aggregate_id: plan_id.get(),
            ordering_key: format!(
                "work_orchestration_plan:{}:{}",
                command.facility_id.get(),
                command
                    .inventory_owner_id
                    .map_or_else(|| "facility_shared".to_owned(), |id| id.get().to_string())
            ),
            transition: "generated",
            occurred_at: generated_at,
            payload: &payload,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
