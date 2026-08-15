use sqlx::Row;
use wareboxes_application::work_orchestration::{
    ResourceCapacitySignalReadModel, WorkOrchestrationPlanCursor,
    WorkOrchestrationPlanItemReadModel, WorkOrchestrationPlanPage, WorkOrchestrationPlanPageQuery,
    WorkOrchestrationPlanReadModel, WorkOrchestrationPlanSummaryReadModel,
    WorkOrchestrationPolicyCursor, WorkOrchestrationPolicyPage, WorkOrchestrationPolicyPageQuery,
    WorkOrchestrationPolicyReadModel, WorkOrchestrationSignalCursor, WorkOrchestrationSignalQuery,
    WorkOrchestrationSignalWorkspace, ZoneCongestionSignalReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, LocationId, OrchestrationPlanMode, OrchestrationScore,
    OrchestrationScoreEvidence, OrchestrationWorkKind, ResourceCapacitySignal, StorageZoneId,
    TenantId, UserId, WorkOrchestrationMode, WorkOrchestrationPlanId, WorkOrchestrationPlanItemId,
    WorkOrchestrationPolicyDefinition, WorkOrchestrationPolicyId, WorkOrchestrationPolicyRevision,
    WorkOrchestrationSignalId, WorkResourceKind, ZoneCongestionSignal,
};

use super::scope::{invalid_data, require_facility_scope, require_owner_scope};
use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, require_permission_tx};

const READ_PERMISSION: &str = "wms";

fn i64_to_u16(value: i64, label: &str) -> AppResult<u16> {
    u16::try_from(value).map_err(|_| AppError::internal(format!("invalid {label}: {value}")))
}

fn i64_to_u32(value: i64, label: &str) -> AppResult<u32> {
    u32::try_from(value).map_err(|_| AppError::internal(format!("invalid {label}: {value}")))
}

fn parse_mode(value: &str) -> AppResult<WorkOrchestrationMode> {
    WorkOrchestrationMode::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid orchestration mode: {value}")))
}

fn parse_plan_mode(value: &str) -> AppResult<OrchestrationPlanMode> {
    OrchestrationPlanMode::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid orchestration plan mode: {value}")))
}

fn parse_work_kind(value: &str) -> AppResult<OrchestrationWorkKind> {
    OrchestrationWorkKind::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid orchestration work kind: {value}")))
}

fn optional_work_kind(value: Option<String>) -> AppResult<Option<OrchestrationWorkKind>> {
    value.map(|value| parse_work_kind(&value)).transpose()
}

fn parse_resource_kind(value: &str) -> AppResult<WorkResourceKind> {
    WorkResourceKind::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid work resource kind: {value}")))
}

pub(super) fn policy_from_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<WorkOrchestrationPolicyReadModel> {
    Ok(WorkOrchestrationPolicyReadModel {
        policy_id: WorkOrchestrationPolicyId::new(row.try_get("id")?).map_err(invalid_data)?,
        definition: WorkOrchestrationPolicyDefinition {
            tenant_id: TenantId::new(row.try_get("tenant_id")?).map_err(invalid_data)?,
            facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(invalid_data)?,
            inventory_owner_id: row
                .try_get::<Option<i64>, _>("inventory_owner_id")?
                .map(InventoryOwnerId::new)
                .transpose()
                .map_err(invalid_data)?,
            mode: parse_mode(&row.try_get::<String, _>("mode")?)?,
            priority_weight: i64_to_u32(row.try_get("priority_weight")?, "priority weight")?,
            due_urgency_weight: i64_to_u32(
                row.try_get("due_urgency_weight")?,
                "due urgency weight",
            )?,
            proximity_weight: i64_to_u32(row.try_get("proximity_weight")?, "proximity weight")?,
            interleaving_weight: i64_to_u32(
                row.try_get("interleaving_weight")?,
                "interleaving weight",
            )?,
            congestion_penalty_weight: i64_to_u32(
                row.try_get("congestion_penalty_weight")?,
                "congestion penalty weight",
            )?,
            bottleneck_penalty_weight: i64_to_u32(
                row.try_get("bottleneck_penalty_weight")?,
                "bottleneck penalty weight",
            )?,
            due_horizon_minutes: i64_to_u32(row.try_get("due_horizon_minutes")?, "due horizon")?,
            max_candidates: i64_to_u16(row.try_get("max_candidates")?, "candidate limit")?,
        },
        revision: WorkOrchestrationPolicyRevision::new(row.try_get("revision")?)
            .map_err(invalid_data)?,
        configured_by: UserId::new(row.try_get("configured_by_user_id")?).map_err(invalid_data)?,
        configured_at: row.try_get("configured_at")?,
        effective_from: row.try_get("effective_from")?,
        supersedes_policy_id: row
            .try_get::<Option<i64>, _>("supersedes_policy_id")?
            .map(WorkOrchestrationPolicyId::new)
            .transpose()
            .map_err(invalid_data)?,
        effective_to: row.try_get("effective_to")?,
    })
}

pub(super) fn zone_signal_from_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<ZoneCongestionSignalReadModel> {
    Ok(ZoneCongestionSignalReadModel {
        signal_id: WorkOrchestrationSignalId::new(row.try_get("id")?).map_err(invalid_data)?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?).map_err(invalid_data)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(invalid_data)?,
        storage_zone_id: StorageZoneId::new(row.try_get("storage_zone_id")?)
            .map_err(invalid_data)?,
        storage_zone_code: row.try_get("storage_zone_code")?,
        signal: ZoneCongestionSignal {
            congestion_basis_points: i64_to_u16(
                row.try_get("congestion_basis_points")?,
                "congestion basis points",
            )?,
            queue_depth: row.try_get("queue_depth")?,
            ttl_seconds: i64_to_u32(row.try_get("ttl_seconds")?, "signal TTL")?,
        },
        recorded_by: UserId::new(row.try_get("recorded_by_user_id")?).map_err(invalid_data)?,
        observed_at: row.try_get("observed_at")?,
        expires_at: row.try_get("expires_at")?,
    })
}

pub(super) fn resource_signal_from_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<ResourceCapacitySignalReadModel> {
    Ok(ResourceCapacitySignalReadModel {
        signal_id: WorkOrchestrationSignalId::new(row.try_get("id")?).map_err(invalid_data)?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?).map_err(invalid_data)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(invalid_data)?,
        resource_kind: parse_resource_kind(&row.try_get::<String, _>("resource_kind")?)?,
        signal: ResourceCapacitySignal {
            available_units: row.try_get("available_units")?,
            demand_units: row.try_get("demand_units")?,
            ttl_seconds: i64_to_u32(row.try_get("ttl_seconds")?, "signal TTL")?,
        },
        utilization_basis_points: i64_to_u16(
            row.try_get("utilization_basis_points")?,
            "resource utilization",
        )?,
        recorded_by: UserId::new(row.try_get("recorded_by_user_id")?).map_err(invalid_data)?,
        observed_at: row.try_get("observed_at")?,
        expires_at: row.try_get("expires_at")?,
    })
}

fn plan_summary_from_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<WorkOrchestrationPlanSummaryReadModel> {
    Ok(WorkOrchestrationPlanSummaryReadModel {
        plan_id: WorkOrchestrationPlanId::new(row.try_get("id")?).map_err(invalid_data)?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?).map_err(invalid_data)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(invalid_data)?,
        requested_inventory_owner_id: row
            .try_get::<Option<i64>, _>("requested_inventory_owner_id")?
            .map(InventoryOwnerId::new)
            .transpose()
            .map_err(invalid_data)?,
        current_location_id: LocationId::new(row.try_get("current_location_id")?)
            .map_err(invalid_data)?,
        current_location_label: row.try_get("current_location_label")?,
        previous_work_kind: optional_work_kind(row.try_get("previous_work_kind")?)?,
        generated_for_user_id: row
            .try_get::<Option<i64>, _>("generated_for_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(invalid_data)?,
        policy_id: WorkOrchestrationPolicyId::new(row.try_get("policy_id")?)
            .map_err(invalid_data)?,
        policy_revision: WorkOrchestrationPolicyRevision::new(row.try_get("policy_revision")?)
            .map_err(invalid_data)?,
        policy_inventory_owner_id: row
            .try_get::<Option<i64>, _>("policy_inventory_owner_id")?
            .map(InventoryOwnerId::new)
            .transpose()
            .map_err(invalid_data)?,
        plan_mode: parse_plan_mode(&row.try_get::<String, _>("plan_mode")?)?,
        input_snapshot_at: row.try_get("input_snapshot_at")?,
        candidate_count: row.try_get("candidate_count")?,
        item_count: row.try_get("item_count")?,
        generated_by: UserId::new(row.try_get("generated_by_user_id")?).map_err(invalid_data)?,
        generated_at: row.try_get("generated_at")?,
    })
}

fn plan_item_from_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<WorkOrchestrationPlanItemReadModel> {
    let work_kind = parse_work_kind(&row.try_get::<String, _>("work_kind")?)?;
    Ok(WorkOrchestrationPlanItemReadModel {
        plan_item_id: WorkOrchestrationPlanItemId::new(row.try_get("id")?).map_err(invalid_data)?,
        sequence: i64_to_u16(row.try_get("sequence")?, "plan item sequence")?,
        work_task_id: row.try_get("work_task_id")?,
        work_kind,
        inventory_owner_id: row
            .try_get::<Option<i64>, _>("inventory_owner_id")?
            .map(InventoryOwnerId::new)
            .transpose()
            .map_err(invalid_data)?,
        title: row.try_get("title")?,
        instructions: row.try_get("instructions")?,
        task_status: row.try_get("task_status")?,
        task_created_at: row.try_get("task_created_at")?,
        source_location_label: row.try_get("source_location_label")?,
        destination_location_label: row.try_get("destination_location_label")?,
        zone_signal_id: row
            .try_get::<Option<i64>, _>("zone_signal_id")?
            .map(WorkOrchestrationSignalId::new)
            .transpose()
            .map_err(invalid_data)?,
        resource_signal_id: row
            .try_get::<Option<i64>, _>("resource_signal_id")?
            .map(WorkOrchestrationSignalId::new)
            .transpose()
            .map_err(invalid_data)?,
        evidence: OrchestrationScoreEvidence {
            work_kind,
            task_priority: row.try_get("task_priority")?,
            due_at: row.try_get("due_at")?,
            overdue_seconds: row.try_get("overdue_seconds")?,
            due_urgency_basis_points: i64_to_u16(
                row.try_get("due_urgency_basis_points")?,
                "due urgency",
            )?,
            current_location_id: LocationId::new(row.try_get("current_location_id")?)
                .map_err(invalid_data)?,
            source_location_id: LocationId::new(row.try_get("source_location_id")?)
                .map_err(invalid_data)?,
            destination_location_id: row
                .try_get::<Option<i64>, _>("destination_location_id")?
                .map(LocationId::new)
                .transpose()
                .map_err(invalid_data)?,
            current_travel_sequence: row.try_get("current_travel_sequence")?,
            source_travel_sequence: row.try_get("source_travel_sequence")?,
            destination_travel_sequence: row.try_get("destination_travel_sequence")?,
            travel_distance: row.try_get("travel_distance")?,
            proximity_basis_points: i64_to_u16(
                row.try_get("proximity_basis_points")?,
                "proximity",
            )?,
            previous_work_kind: optional_work_kind(row.try_get("previous_work_kind")?)?,
            interleaving_compatible: row.try_get("interleaving_compatible")?,
            source_zone_id: row.try_get("source_zone_id")?,
            source_zone_code: row.try_get("source_zone_code")?,
            congestion_basis_points: i64_to_u16(
                row.try_get("congestion_basis_points")?,
                "congestion",
            )?,
            congestion_queue_depth: row.try_get("congestion_queue_depth")?,
            resource_kind: parse_resource_kind(&row.try_get::<String, _>("resource_kind")?)?,
            resource_available_units: row.try_get("resource_available_units")?,
            resource_demand_units: row.try_get("resource_demand_units")?,
            resource_utilization_basis_points: i64_to_u16(
                row.try_get("resource_utilization_basis_points")?,
                "resource utilization",
            )?,
        },
        score: OrchestrationScore {
            priority_component: row.try_get("priority_score")?,
            due_urgency_component: row.try_get("due_urgency_score")?,
            proximity_component: row.try_get("proximity_score")?,
            interleaving_component: row.try_get("interleaving_score")?,
            congestion_penalty: row.try_get("congestion_penalty")?,
            bottleneck_penalty: row.try_get("bottleneck_penalty")?,
            total: row.try_get("total_score")?,
        },
    })
}

pub(super) async fn read_plan_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    plan_id: WorkOrchestrationPlanId,
) -> AppResult<WorkOrchestrationPlanReadModel> {
    let row = sqlx::query("SELECT * FROM work_orchestration_plans WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(plan_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("work orchestration plan"))?;
    let summary = plan_summary_from_row(&row)?;
    let item_rows = sqlx::query(
        "SELECT * FROM work_orchestration_plan_items WHERE tenant_id=$1 AND plan_id=$2 ORDER BY sequence",
    )
    .bind(tenant_id.get())
    .bind(plan_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let items = item_rows
        .iter()
        .map(plan_item_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    Ok(WorkOrchestrationPlanReadModel {
        plan_id: summary.plan_id,
        tenant_id: summary.tenant_id,
        facility_id: summary.facility_id,
        requested_inventory_owner_id: summary.requested_inventory_owner_id,
        current_location_id: summary.current_location_id,
        current_location_label: summary.current_location_label,
        previous_work_kind: summary.previous_work_kind,
        generated_for_user_id: summary.generated_for_user_id,
        policy_id: summary.policy_id,
        policy_revision: summary.policy_revision,
        policy_inventory_owner_id: summary.policy_inventory_owner_id,
        plan_mode: summary.plan_mode,
        input_snapshot_at: summary.input_snapshot_at,
        configuration_snapshot: row.try_get("configuration_snapshot")?,
        candidate_count: summary.candidate_count,
        item_count: summary.item_count,
        generated_by: summary.generated_by,
        generated_at: summary.generated_at,
        items,
    })
}

async fn read_context<'db>(
    db: &'db Db,
    access: &TenantAccess,
) -> AppResult<(
    sqlx::Transaction<'db, sqlx::Postgres>,
    crate::repo::access::ScopeBindings,
)> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        READ_PERMISSION,
    )
    .await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    Ok((tx, scope))
}

pub async fn policy_page(
    db: &Db,
    access: &TenantAccess,
    query: WorkOrchestrationPolicyPageQuery,
) -> AppResult<WorkOrchestrationPolicyPage> {
    let (mut tx, scope) = read_context(db, access).await?;
    if let Some(facility_id) = query.facility_id {
        require_facility_scope(&scope, facility_id.get(), "work orchestration policy")?;
    }
    if let Some(owner_id) = query.inventory_owner_id {
        require_owner_scope(&scope, owner_id.get(), "work orchestration policy")?;
    }
    let rows = sqlx::query(
        r#"SELECT * FROM work_orchestration_policies policy
        WHERE policy.tenant_id=$1 AND ($2::bigint IS NULL OR policy.facility_id=$2)
          AND ($3::bigint IS NULL OR policy.inventory_owner_id=$3
            OR ($4 AND policy.inventory_owner_id IS NULL))
          AND ($5 OR policy.effective_to IS NULL)
          AND ($6 OR policy.facility_id=ANY($7))
          AND (policy.inventory_owner_id IS NULL OR $8 OR policy.inventory_owner_id=ANY($9))
          AND ($10::timestamptz IS NULL OR (policy.configured_at,policy.id)<($10,$11))
        ORDER BY policy.configured_at DESC,policy.id DESC LIMIT $12"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.include_facility_defaults)
    .bind(query.include_history)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.cursor.map(|cursor| cursor.after_configured_at))
    .bind(query.cursor.map(|cursor| cursor.after_policy_id.get()))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(policy_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if rows.len() > usize::from(query.limit) {
        items.last().map(|item| WorkOrchestrationPolicyCursor {
            after_configured_at: item.configured_at,
            after_policy_id: item.policy_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(WorkOrchestrationPolicyPage { items, next_cursor })
}

pub async fn signal_workspace(
    db: &Db,
    access: &TenantAccess,
    query: WorkOrchestrationSignalQuery,
) -> AppResult<WorkOrchestrationSignalWorkspace> {
    let (mut tx, scope) = read_context(db, access).await?;
    require_facility_scope(
        &scope,
        query.facility_id.get(),
        "work orchestration signals",
    )?;
    let zone_rows = sqlx::query(
        r#"SELECT * FROM work_orchestration_zone_signals signal
        WHERE signal.tenant_id=$1 AND signal.facility_id=$2
          AND ($3 OR signal.expires_at>transaction_timestamp())
          AND ($3 OR NOT EXISTS(SELECT 1 FROM work_orchestration_zone_signals newer
            WHERE newer.tenant_id=signal.tenant_id AND newer.facility_id=signal.facility_id
              AND newer.storage_zone_id=signal.storage_zone_id
              AND (newer.observed_at,newer.id)>(signal.observed_at,signal.id)
              AND newer.expires_at>transaction_timestamp()))
          AND ($4::timestamptz IS NULL OR
            (signal.observed_at,signal.id)<($4,$5))
        ORDER BY signal.observed_at DESC,signal.id DESC LIMIT $6"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.facility_id.get())
    .bind(query.include_history)
    .bind(query.zone_cursor.map(|cursor| cursor.after_observed_at))
    .bind(query.zone_cursor.map(|cursor| cursor.after_signal_id.get()))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let resource_rows = sqlx::query(
        r#"SELECT * FROM work_orchestration_resource_signals signal
        WHERE signal.tenant_id=$1 AND signal.facility_id=$2
          AND ($3 OR signal.expires_at>transaction_timestamp())
          AND ($3 OR NOT EXISTS(SELECT 1 FROM work_orchestration_resource_signals newer
            WHERE newer.tenant_id=signal.tenant_id AND newer.facility_id=signal.facility_id
              AND newer.resource_kind=signal.resource_kind
              AND (newer.observed_at,newer.id)>(signal.observed_at,signal.id)
              AND newer.expires_at>transaction_timestamp()))
          AND ($4::timestamptz IS NULL OR
            (signal.observed_at,signal.id)<($4,$5))
        ORDER BY signal.observed_at DESC,signal.id DESC LIMIT $6"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.facility_id.get())
    .bind(query.include_history)
    .bind(query.resource_cursor.map(|cursor| cursor.after_observed_at))
    .bind(
        query
            .resource_cursor
            .map(|cursor| cursor.after_signal_id.get()),
    )
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let zone_signals = zone_rows
        .iter()
        .take(usize::from(query.limit))
        .map(zone_signal_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let resource_signals = resource_rows
        .iter()
        .take(usize::from(query.limit))
        .map(resource_signal_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_zone_cursor = if zone_rows.len() > usize::from(query.limit) {
        zone_signals
            .last()
            .map(|signal| WorkOrchestrationSignalCursor {
                after_observed_at: signal.observed_at,
                after_signal_id: signal.signal_id,
            })
    } else {
        None
    };
    let next_resource_cursor = if resource_rows.len() > usize::from(query.limit) {
        resource_signals
            .last()
            .map(|signal| WorkOrchestrationSignalCursor {
                after_observed_at: signal.observed_at,
                after_signal_id: signal.signal_id,
            })
    } else {
        None
    };
    let result = WorkOrchestrationSignalWorkspace {
        zone_signals,
        resource_signals,
        next_zone_cursor,
        next_resource_cursor,
    };
    tx.commit().await?;
    Ok(result)
}

pub async fn plan_page(
    db: &Db,
    access: &TenantAccess,
    query: WorkOrchestrationPlanPageQuery,
) -> AppResult<WorkOrchestrationPlanPage> {
    let (mut tx, scope) = read_context(db, access).await?;
    if let Some(facility_id) = query.facility_id {
        require_facility_scope(&scope, facility_id.get(), "work orchestration plan")?;
    }
    if let Some(owner_id) = query.inventory_owner_id {
        require_owner_scope(&scope, owner_id.get(), "work orchestration plan")?;
    }
    let rows = sqlx::query(
        r#"SELECT * FROM work_orchestration_plans plan
        WHERE plan.tenant_id=$1 AND ($2::bigint IS NULL OR plan.facility_id=$2)
          AND ($3::bigint IS NULL OR plan.requested_inventory_owner_id=$3)
          AND ($4::text IS NULL OR plan.plan_mode=$4)
          AND ($5 OR plan.facility_id=ANY($6))
          AND (plan.requested_inventory_owner_id IS NULL OR $7
            OR plan.requested_inventory_owner_id=ANY($8))
          AND ($9::timestamptz IS NULL OR (plan.generated_at,plan.id)<($9,$10))
        ORDER BY plan.generated_at DESC,plan.id DESC LIMIT $11"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.plan_mode.map(OrchestrationPlanMode::as_str))
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.cursor.map(|cursor| cursor.after_generated_at))
    .bind(query.cursor.map(|cursor| cursor.after_plan_id.get()))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(plan_summary_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if rows.len() > usize::from(query.limit) {
        items.last().map(|item| WorkOrchestrationPlanCursor {
            after_generated_at: item.generated_at,
            after_plan_id: item.plan_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(WorkOrchestrationPlanPage { items, next_cursor })
}

pub async fn plan_by_id(
    db: &Db,
    access: &TenantAccess,
    plan_id: WorkOrchestrationPlanId,
) -> AppResult<WorkOrchestrationPlanReadModel> {
    let (mut tx, scope) = read_context(db, access).await?;
    let result = read_plan_tx(&mut tx, access.tenant_id, plan_id).await?;
    require_facility_scope(&scope, result.facility_id.get(), "work orchestration plan")?;
    if let Some(owner_id) = result.requested_inventory_owner_id {
        require_owner_scope(&scope, owner_id.get(), "work orchestration plan")?;
    }
    tx.commit().await?;
    Ok(result)
}
