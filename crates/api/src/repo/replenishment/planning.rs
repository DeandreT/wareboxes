use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::replenishment::{
    PlanReplenishmentCommand, PlanReplenishmentResult, PlannedReplenishmentWork,
    PLAN_REPLENISHMENT_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{TenantAccess, WorkTaskType};
use wareboxes_domain::{
    assess_replenishment_source, plan_replenishment, select_replenishment_sources,
    InventoryBalanceId, ItemBatchId, LocationId, ReplenishmentInventoryStatus, ReplenishmentPlanId,
    ReplenishmentPlanningOutcome, ReplenishmentPlanningSnapshot, ReplenishmentPolicyId,
    ReplenishmentSourceCandidate, ReplenishmentSourceEligibility, ReplenishmentWorkId, TenantId,
    Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::tasks::{insert_task_tx, NewWorkTask};

use super::{
    enqueue_event_tx, level, policy_from_row, policy_sources_tx, require_scope,
    require_stored_policy_visible_before_replay_tx, scan, PolicyRow,
};

#[derive(Debug, Clone)]
struct SourceRow {
    balance_id: InventoryBalanceId,
    batch_id: ItemBatchId,
    location_id: LocationId,
    barcode: String,
    name: Option<String>,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
    received_at: Timestamp,
    free: i64,
}

pub async fn plan_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanReplenishmentCommand,
) -> AppResult<PlanReplenishmentResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, PLAN_REPLENISHMENT_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        "wms_supervisor",
    )
    .await?;
    require_stored_policy_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<PlanReplenishmentResult>(&mut tx)
        .await?
    {
        require_replayed_plan_visible_tx(&mut tx, access.tenant_id, result.plan_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let policy = lock_policy_tx(&mut tx, access.tenant_id, command.policy_id, &scope).await?;
    if policy.effective_to.is_some() {
        return Err(AppError::conflict("replenishment policy is retired"));
    }
    if policy.revision != command.expected_policy_revision {
        return Err(AppError::conflict(
            "replenishment policy revision does not match expected revision",
        ));
    }
    let (snapshot, sources) = lock_snapshot_tx(&mut tx, access.tenant_id, &policy).await?;
    let decision = plan_replenishment(policy.definition.thresholds(), snapshot);
    let eligible = sources
        .iter()
        .map(|source| {
            let candidate = source.domain_candidate(&policy)?;
            match assess_replenishment_source(&policy.definition, candidate) {
                ReplenishmentSourceEligibility::Eligible(candidate) => Ok(candidate),
                ReplenishmentSourceEligibility::Ineligible(reason) => Err(AppError::internal(
                    format!("locked replenishment source is ineligible: {reason:?}"),
                )),
            }
        })
        .collect::<AppResult<Vec<_>>>()?;
    let selected = select_replenishment_sources(decision, eligible)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let planned_at = now_iso();
    let plan_id = insert_plan_tx(
        &mut tx,
        access.tenant_id,
        &policy,
        decision,
        i64::try_from(selected.len())
            .map_err(|_| AppError::internal("replenishment work count overflow"))?,
        context.actor_id.get(),
        planned_at,
    )
    .await?;
    let source_by_id = sources
        .into_iter()
        .map(|source| (source.balance_id, source))
        .collect::<HashMap<_, _>>();
    let mut work = Vec::with_capacity(selected.len());
    for selected_source in selected {
        let source = source_by_id
            .get(&selected_source.source_inventory_balance_id)
            .ok_or_else(|| AppError::internal("planned replenishment source disappeared"))?;
        let work_id = insert_work_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            &policy,
            plan_id,
            &selected_source,
            source.free,
            decision.snapshot.projected_free().get() < decision.snapshot.unallocated_demand().get(),
        )
        .await?;
        work.push(PlannedReplenishmentWork::from_domain(
            work_id,
            selected_source,
            scan(source.barcode.clone(), "replenishment source location")?,
            source.name.clone(),
        ));
    }
    let result = PlanReplenishmentResult {
        plan_id,
        policy_id: policy.id,
        policy_revision: policy.revision,
        scope: policy.scope().clone(),
        snapshot: decision.snapshot,
        required_level: decision.required_level,
        target_gap: decision.target_gap,
        planned: decision.planned,
        remaining: decision.remaining,
        outcome: decision.outcome,
        work,
        planned_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        planned_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        policy.scope().inventory_owner_id,
        policy.scope().facility_id,
        context.actor_id.get(),
        "replenishment_policy",
        policy.id.get(),
        "inventory.replenishment.planned",
        &format!("plan:{}", plan_id.get()),
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
        planned_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

impl SourceRow {
    fn domain_candidate(&self, policy: &PolicyRow) -> AppResult<ReplenishmentSourceCandidate> {
        Ok(ReplenishmentSourceCandidate {
            tenant_id: policy.scope().tenant_id,
            inventory_owner_id: policy.scope().inventory_owner_id,
            facility_id: policy.scope().facility_id,
            source_location_id: self.location_id,
            source_inventory_balance_id: self.balance_id,
            item_batch_id: self.batch_id,
            item_id: policy.scope().item_id,
            uom: policy.scope().uom.clone(),
            lot: self.lot.clone(),
            serial: self.serial.clone(),
            expiration: self.expiration,
            received_at: self.received_at,
            inventory_status: ReplenishmentInventoryStatus::Available,
            free_quantity: level(self.free)?,
        })
    }
}

async fn lock_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ReplenishmentPolicyId,
    scope: &ScopeBindings,
) -> AppResult<PolicyRow> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, facility_id,
               pick_face_location_id, item_id, uom, minimum_qty, target_qty,
               revision, effective_from, effective_to
        FROM replenishment_policies
        WHERE tenant_id=$1 AND id=$2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(policy_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment policy"))?;
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    require_scope(scope, owner_id, facility_id)?;
    let sources = policy_sources_tx(tx, tenant_id, policy_id).await?;
    policy_from_row(&row, sources)
}

async fn lock_snapshot_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy: &PolicyRow,
) -> AppResult<(ReplenishmentPlanningSnapshot, Vec<SourceRow>)> {
    let scope = policy.scope();
    let source_ids = policy
        .definition
        .reserve_source_location_ids()
        .as_slice()
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let executable_location_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)::bigint FROM locations location
        WHERE location.tenant_id=$1 AND location.facility_id=$2 AND location.deleted IS NULL
          AND location.active AND NULLIF(btrim(location.barcode),'') IS NOT NULL
          AND ((location.id=$3 AND location.pickable AND NOT location.receivable)
            OR (location.id=ANY($4) AND NOT location.pickable AND NOT location.receivable))
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.pick_face_location_id.get())
    .bind(&source_ids)
    .fetch_one(&mut **tx)
    .await?;
    let configured_location_count = source_ids
        .len()
        .checked_add(1)
        .and_then(|count| i64::try_from(count).ok())
        .ok_or_else(|| AppError::internal("replenishment location count overflow"))?;
    if executable_location_count != configured_location_count {
        return Err(AppError::conflict(
            "replenishment policy locations are no longer executable",
        ));
    }
    sqlx::query(
        r#"
        SELECT work.id
        FROM work_tasks work
        JOIN replenishment_tasks task ON task.tenant_id=work.tenant_id AND task.task_id=work.id
        WHERE task.tenant_id=$1 AND task.policy_id=$2 AND task.closed_at IS NULL
        ORDER BY work.id
        FOR UPDATE OF work
        "#,
    )
    .bind(tenant_id.get())
    .bind(policy.id.get())
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        SELECT reservation.id
        FROM inventory_reservations reservation
        WHERE reservation.tenant_id=$1 AND reservation.inventory_owner_id=$2
          AND reservation.facility_id=$3 AND reservation.item_id=$4
          AND reservation.uom=$5 AND reservation.status='active' AND reservation.deleted IS NULL
        ORDER BY reservation.id
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.item_id.get())
    .bind(scope.uom.as_str())
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        SELECT allocation.id
        FROM inventory_allocations allocation
        JOIN inventory_reservations reservation
          ON reservation.tenant_id=allocation.tenant_id
         AND reservation.inventory_owner_id=allocation.inventory_owner_id
         AND reservation.id=allocation.reservation_id
        WHERE allocation.tenant_id=$1 AND allocation.inventory_owner_id=$2
          AND reservation.facility_id=$3 AND allocation.item_id=$4
          AND allocation.uom=$5 AND allocation.status='allocated' AND allocation.deleted IS NULL
        ORDER BY allocation.id
        FOR UPDATE OF allocation
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.item_id.get())
    .bind(scope.uom.as_str())
    .fetch_all(&mut **tx)
    .await?;

    let rows = sqlx::query(
        r#"
        SELECT balance.id, balance.item_batch_id, balance.location_id,
               location.barcode, location.name, batch.lot, batch.serial,
               batch.expiration, batch.created AS received_at,
               GREATEST(balance.qty_on_hand-balance.qty_reserved-balance.qty_held,0)::bigint AS free_qty
        FROM inventory_balances balance
        JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
          AND batch.inventory_owner_id=balance.inventory_owner_id
          AND batch.id=balance.item_batch_id
        JOIN locations location ON location.tenant_id=balance.tenant_id
          AND location.facility_id=balance.facility_id AND location.id=balance.location_id
        WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
          AND balance.facility_id=$3 AND balance.item_id=$4 AND balance.uom=$5
          AND balance.license_plate_id IS NULL AND balance.status='available'
          AND balance.deleted IS NULL
          AND batch.deleted IS NULL
          AND location.deleted IS NULL AND location.active
          AND NULLIF(btrim(location.barcode),'') IS NOT NULL
          AND (
            (balance.location_id=$6 AND location.pickable AND NOT location.receivable)
            OR (
              balance.location_id=ANY($7)
              AND NOT location.pickable AND NOT location.receivable
              AND NOT EXISTS (
                SELECT 1 FROM loose_inventory_movement_claims claim
                WHERE claim.tenant_id=balance.tenant_id
                  AND claim.inventory_owner_id=balance.inventory_owner_id
                  AND claim.facility_id=balance.facility_id
                  AND claim.source_inventory_balance_id=balance.id
                  AND claim.released_at IS NULL
              )
            )
          )
        ORDER BY balance.id
        FOR UPDATE OF balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.item_id.get())
    .bind(scope.uom.as_str())
    .bind(scope.pick_face_location_id.get())
    .bind(source_ids)
    .fetch_all(&mut **tx)
    .await?;
    // The balance lock may have waited for another movement command whose claim
    // was not visible to the statement snapshot. Re-read claims after locking.
    let active_claimed_balance_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT source_inventory_balance_id
        FROM loose_inventory_movement_claims
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND released_at IS NULL
        ORDER BY source_inventory_balance_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let pick_face_free = rows
        .iter()
        .filter(|row| {
            row.try_get::<i64, _>("location_id").ok() == Some(scope.pick_face_location_id.get())
        })
        .try_fold(0_i64, |sum, row| {
            sum.checked_add(row.try_get::<i64, _>("free_qty").ok()?)
        })
        .ok_or_else(|| AppError::internal("pick-face free quantity overflow"))?;
    let sources = rows
        .into_iter()
        .filter_map(|row| {
            let location_id: i64 = row.try_get("location_id").ok()?;
            let balance_id: i64 = row.try_get("id").ok()?;
            (location_id != scope.pick_face_location_id.get()
                && active_claimed_balance_ids
                    .binary_search(&balance_id)
                    .is_err())
            .then_some(row)
        })
        .map(|row| {
            Ok(SourceRow {
                balance_id: InventoryBalanceId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                location_id: LocationId::new(row.try_get("location_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                barcode: row.try_get("barcode")?,
                name: row.try_get("name")?,
                lot: row.try_get("lot")?,
                serial: row.try_get("serial")?,
                expiration: row.try_get("expiration")?,
                received_at: row.try_get("received_at")?,
                free: row.try_get("free_qty")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let active_inbound: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(planned_qty),0)::bigint FROM replenishment_tasks WHERE tenant_id=$1 AND policy_id=$2 AND closed_at IS NULL",
    )
    .bind(tenant_id.get())
    .bind(policy.id.get())
    .fetch_one(&mut **tx)
    .await?;
    let unallocated: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(sum(GREATEST(
          reservation.qty-COALESCE(disposition.accepted,0)
            -COALESCE(backorder.qty,0)-COALESCE(allocation.allocated,0),0
        )),0)::bigint
        FROM inventory_reservations reservation
        LEFT JOIN LATERAL (
          SELECT sum(value.accepted_short_qty)::bigint accepted
          FROM pick_short_ship_dispositions value
          WHERE value.tenant_id=reservation.tenant_id
            AND value.inventory_owner_id=reservation.inventory_owner_id
            AND value.reservation_id=reservation.id
        ) disposition ON true
        LEFT JOIN LATERAL (
          SELECT sum(value.newly_backordered_qty)::bigint qty
          FROM order_backorder_split_lines value
          WHERE value.tenant_id=reservation.tenant_id
            AND value.inventory_owner_id=reservation.inventory_owner_id
            AND value.parent_order_id=reservation.order_id
            AND value.parent_order_item_id=reservation.order_item_id
        ) backorder ON true
        LEFT JOIN LATERAL (
          SELECT sum(value.qty)::bigint allocated
          FROM inventory_allocations value
          WHERE value.tenant_id=reservation.tenant_id
            AND value.inventory_owner_id=reservation.inventory_owner_id
            AND value.reservation_id=reservation.id
            AND value.status='allocated' AND value.deleted IS NULL
        ) allocation ON true
        WHERE reservation.tenant_id=$1 AND reservation.inventory_owner_id=$2
          AND reservation.facility_id=$3 AND reservation.item_id=$4
          AND reservation.uom=$5 AND reservation.status='active' AND reservation.deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.item_id.get())
    .bind(scope.uom.as_str())
    .fetch_one(&mut **tx)
    .await?;
    let reserve_free = sources.iter().try_fold(0_i64, |sum, source| {
        sum.checked_add(source.free)
            .ok_or_else(|| AppError::internal("reserve free quantity overflow"))
    })?;
    let snapshot = ReplenishmentPlanningSnapshot::new(
        level(pick_face_free)?,
        level(active_inbound)?,
        level(unallocated)?,
        level(reserve_free)?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    Ok((snapshot, sources))
}

#[allow(clippy::too_many_arguments)]
async fn insert_plan_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy: &PolicyRow,
    decision: wareboxes_domain::ReplenishmentPlanDecision,
    work_count: i64,
    actor_id: i64,
    planned_at: Timestamp,
) -> AppResult<ReplenishmentPlanId> {
    let scope = policy.scope();
    let thresholds = policy.definition.thresholds();
    let source_count = i64::try_from(
        policy
            .definition
            .reserve_source_location_ids()
            .as_slice()
            .len(),
    )
    .map_err(|_| AppError::internal("replenishment source count overflow"))?;
    let outcome = match decision.outcome {
        ReplenishmentPlanningOutcome::NotNeeded => "not_needed",
        ReplenishmentPlanningOutcome::InsufficientReserve => "insufficient_reserve",
        ReplenishmentPlanningOutcome::PartiallyPlanned => "partially_planned",
        ReplenishmentPlanningOutcome::FullyPlanned => "fully_planned",
    };
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO replenishment_plan_runs (
          tenant_id,inventory_owner_id,facility_id,policy_id,policy_revision,
          pick_face_location_id,item_id,uom,minimum_qty,target_qty,source_location_count,
          pick_face_free_qty,active_inbound_qty,projected_free_qty,unallocated_demand_qty,
          required_level_qty,target_gap_qty,reserve_free_qty,planned_qty,work_count,outcome,
          planned_by_user_id,planned_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get()).bind(scope.inventory_owner_id.get()).bind(scope.facility_id.get())
    .bind(policy.id.get()).bind(policy.revision.get()).bind(scope.pick_face_location_id.get())
    .bind(scope.item_id.get()).bind(scope.uom.as_str()).bind(thresholds.minimum().get())
    .bind(thresholds.target().get()).bind(source_count)
    .bind(decision.snapshot.pick_face_free().get()).bind(decision.snapshot.active_inbound().get())
    .bind(decision.snapshot.projected_free().get()).bind(decision.snapshot.unallocated_demand().get())
    .bind(decision.required_level.get()).bind(decision.target_gap.get())
    .bind(decision.snapshot.reserve_free().get()).bind(decision.planned.get())
    .bind(work_count).bind(outcome).bind(actor_id).bind(planned_at)
    .fetch_one(&mut **tx).await?;
    ReplenishmentPlanId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: i64,
    policy: &PolicyRow,
    plan_id: ReplenishmentPlanId,
    source: &wareboxes_domain::PlannedReplenishmentSource,
    source_free: i64,
    demand_driven: bool,
) -> AppResult<ReplenishmentWorkId> {
    let scope = policy.scope();
    let task_id = insert_task_tx(
        tx,
        tenant_id,
        NewWorkTask {
            facility_id: Some(scope.facility_id.get()),
            inventory_owner_id: Some(scope.inventory_owner_id.get()),
            task_type: WorkTaskType::Replenishment,
            title: "Replenish pick face".into(),
            instructions: None,
            required_permission: "wms".into(),
            priority: if demand_driven { 90 } else { 60 },
            task_timeout_seconds: 30 * 60,
            assigned_user_id: None,
            created_by: Some(actor_id),
            scheduled_for: None,
            due_at: None,
            metadata_json: None,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO replenishment_tasks (
          tenant_id,task_id,plan_run_id,policy_id,policy_revision,inventory_owner_id,facility_id,
          source_inventory_balance_id,source_location_id,destination_location_id,item_batch_id,
          item_id,uom,inventory_status,source_free_qty,planned_qty,source_lot,source_serial,
          source_expiration,source_received_at,travel_sequence
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'available',$14,$15,$16,$17,$18,$19,$20)
        "#,
    )
    .bind(tenant_id.get()).bind(task_id).bind(plan_id.get()).bind(policy.id.get())
    .bind(policy.revision.get()).bind(scope.inventory_owner_id.get()).bind(scope.facility_id.get())
    .bind(source.source_inventory_balance_id.get()).bind(source.source_location_id.get())
    .bind(scope.pick_face_location_id.get()).bind(source.item_batch_id.get()).bind(scope.item_id.get())
    .bind(scope.uom.as_str()).bind(source_free).bind(source.quantity.get())
    .bind(&source.lot).bind(&source.serial).bind(source.expiration)
    .bind(source.received_at).bind(i64::from(source.sequence))
    .execute(&mut **tx).await?;
    ReplenishmentWorkId::new(task_id).map_err(|error| AppError::internal(error.to_string()))
}

async fn require_replayed_plan_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    plan_id: ReplenishmentPlanId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM replenishment_plan_runs WHERE tenant_id=$1 AND id=$2",
    ).bind(tenant_id.get()).bind(plan_id.get()).fetch_optional(&mut **tx).await?
      .ok_or_else(|| AppError::not_found("replenishment plan"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}
