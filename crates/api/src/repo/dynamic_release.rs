//! Policy-bound selection and atomic release of the allocation-ready order queue.

use sqlx::Row;
use wareboxes_application::dynamic_release::{
    DynamicReleaseCandidateReadModel, DynamicReleaseCommand, DynamicReleaseReadinessQuery,
    DynamicReleaseReadinessReadModel, DynamicReleaseRunReadModel, DYNAMIC_RELEASE_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::pick_wave::{
    PlanPickWaveCommand, PlanPickWaveOrder, ReleasePickWaveCommand,
};
use wareboxes_application::wave_policy::WavePolicyReadModel;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    DynamicReleaseRunId, InventoryOwnerId, OrderId, OrderRevision, PickWaveName, PickWaveRevision,
    TenantId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

const MUTATE_PERMISSION: &str = "wms_supervisor";
const READ_PERMISSION: &str = "orders";

pub async fn readiness(
    db: &Db,
    access: &TenantAccess,
    query: &DynamicReleaseReadinessQuery,
) -> AppResult<DynamicReleaseReadinessReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        READ_PERMISSION,
    )
    .await?;
    require_scope(
        &scope,
        query.facility_id.get(),
        query.inventory_owner_id.get(),
    )?;
    lock_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        query.inventory_owner_id,
        query.facility_id.get(),
    )
    .await?;
    let input_snapshot_at = now_iso();
    let policy = crate::repo::pick_wave::resolve_policy_tx(
        &mut tx,
        access.tenant_id,
        query.inventory_owner_id,
        query.facility_id,
        input_snapshot_at,
        false,
    )
    .await?;
    let (eligible_order_count, selected_orders) = load_selection_tx(
        &mut tx,
        access.tenant_id,
        query.inventory_owner_id,
        query.facility_id.get(),
        input_snapshot_at,
        policy.max_orders,
    )
    .await?;
    let result = readiness_model(
        query.facility_id,
        query.inventory_owner_id,
        input_snapshot_at,
        policy,
        eligible_order_count,
        selected_orders,
    )?;
    tx.commit().await?;
    Ok(result)
}

pub async fn run(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &DynamicReleaseCommand,
) -> AppResult<DynamicReleaseRunReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, DYNAMIC_RELEASE_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(context.actor_id.to_string())
        .execute(&mut *tx)
        .await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        MUTATE_PERMISSION,
    )
    .await?;
    require_replayed_run_visible_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<DynamicReleaseRunReadModel>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    require_scope(
        &scope,
        command.facility_id.get(),
        command.inventory_owner_id.get(),
    )?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "dynamic-release:{}:{}:{}",
            access.tenant_id, command.facility_id, command.inventory_owner_id
        ))
        .execute(&mut *tx)
        .await?;
    lock_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id.get(),
    )
    .await?;
    crate::repo::pick_wave::lock_destination_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.destination_location_id,
    )
    .await?;

    let released_at = now_iso();
    let policy = crate::repo::pick_wave::resolve_policy_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        released_at,
        true,
    )
    .await?;
    crate::repo::pick_wave::require_expected_policy(&policy, &command.expected_policy)?;
    let (eligible_order_count, selected_orders) = load_selection_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id.get(),
        released_at,
        policy.max_orders,
    )
    .await?;
    lock_selected_orders_tx(&mut tx, access.tenant_id, &selected_orders).await?;

    let wave = if selected_orders.is_empty() {
        None
    } else {
        let wave_command = PlanPickWaveCommand {
            facility_id: command.facility_id,
            destination_location_id: command.destination_location_id,
            name: PickWaveName::new(format!(
                "Dynamic release {}",
                released_at.format("%Y%m%d-%H%M%S%.3fZ")
            ))
            .map_err(internal)?,
            orders: selected_orders
                .iter()
                .map(|candidate| PlanPickWaveOrder {
                    order_id: candidate.order_id,
                    expected_revision: candidate.revision,
                    sequence: candidate.rank,
                    expected_policy: policy.expectation(),
                })
                .collect(),
        };
        Some(
            crate::repo::pick_wave::plan_wave_tx(
                &mut tx,
                access,
                context,
                &scope,
                &wave_command,
                released_at,
            )
            .await?,
        )
    };
    let selected_order_count = i64::try_from(selected_orders.len()).map_err(internal)?;
    let deferred_order_count = eligible_order_count
        .checked_sub(selected_order_count)
        .ok_or_else(|| AppError::internal("dynamic release selected more than eligible"))?;
    let run_id = insert_run_tx(
        &mut tx,
        access.tenant_id,
        context,
        command,
        &policy,
        wave.as_ref().map(|value| value.wave_id.get()),
        released_at,
        eligible_order_count,
        selected_order_count,
        deferred_order_count,
    )
    .await?;
    if let Some(planned_wave) = &wave {
        insert_candidates_tx(
            &mut tx,
            access.tenant_id,
            run_id,
            command,
            planned_wave.wave_id.get(),
            &selected_orders,
        )
        .await?;
    }
    let sealed = sqlx::query(
        "UPDATE dynamic_release_runs SET status='sealed' WHERE tenant_id=$1 AND id=$2 AND status='building'",
    )
    .bind(access.tenant_id.get())
    .bind(run_id.get())
    .execute(&mut *tx)
    .await?;
    if sealed.rows_affected() != 1 {
        return Err(AppError::conflict(
            "dynamic release selection changed before sealing",
        ));
    }
    let wave = match wave {
        Some(planned_wave) => Some(
            crate::repo::pick_wave::release_wave_tx(
                &mut tx,
                access,
                context,
                &scope,
                &ReleasePickWaveCommand {
                    wave_id: planned_wave.wave_id,
                    expected_revision: PickWaveRevision::new(1).map_err(internal)?,
                },
                released_at,
            )
            .await?,
        ),
        None => None,
    };
    let result = DynamicReleaseRunReadModel {
        run_id,
        facility_id: command.facility_id,
        inventory_owner_id: command.inventory_owner_id,
        destination_location_id: command.destination_location_id,
        input_snapshot_at: released_at,
        policy,
        eligible_order_count,
        selected_order_count,
        deferred_order_count,
        selected_orders,
        wave,
        released_by: context.actor_id,
        released_at,
    };
    if !result.is_consistent() {
        return Err(AppError::internal("dynamic release result is inconsistent"));
    }
    enqueue_event_tx(&mut tx, access.tenant_id, &result).await?;
    Ok(prepared.commit(tx, result).await?)
}

fn readiness_model(
    facility_id: wareboxes_domain::FacilityId,
    inventory_owner_id: InventoryOwnerId,
    input_snapshot_at: Timestamp,
    policy: WavePolicyReadModel,
    eligible_order_count: i64,
    selected_orders: Vec<DynamicReleaseCandidateReadModel>,
) -> AppResult<DynamicReleaseReadinessReadModel> {
    let selected_order_count = i64::try_from(selected_orders.len()).map_err(internal)?;
    let deferred_order_count = eligible_order_count
        .checked_sub(selected_order_count)
        .ok_or_else(|| AppError::internal("dynamic release selected more than eligible"))?;
    let result = DynamicReleaseReadinessReadModel {
        facility_id,
        inventory_owner_id,
        input_snapshot_at,
        policy,
        eligible_order_count,
        selected_order_count,
        deferred_order_count,
        selected_orders,
    };
    if result.is_consistent() {
        Ok(result)
    } else {
        Err(AppError::internal(
            "dynamic release readiness is inconsistent",
        ))
    }
}

async fn load_selection_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    input_snapshot_at: Timestamp,
    max_orders: u32,
) -> AppResult<(i64, Vec<DynamicReleaseCandidateReadModel>)> {
    let eligible_sql = r#"
      SELECT order_header.id,order_header.order_key,order_header.revision,
             order_header.rush,order_header.ship_by,order_header.created,
             SUM(demand.effective_qty)::bigint AS demand_qty,
             SUM(COALESCE(allocated.qty,0))::bigint AS allocated_qty
      FROM orders order_header
      JOIN inventory_owner_facilities assignment
        ON assignment.tenant_id=order_header.tenant_id
       AND assignment.inventory_owner_id=order_header.inventory_owner_id
       AND assignment.facility_id=$3 AND assignment.deleted IS NULL
      JOIN outbound_effective_demand demand
        ON demand.tenant_id=order_header.tenant_id
       AND demand.inventory_owner_id=order_header.inventory_owner_id
       AND demand.order_id=order_header.id
      LEFT JOIN LATERAL (
        SELECT SUM(allocation.qty)::bigint AS qty
        FROM inventory_reservations reservation
        JOIN inventory_allocations allocation
          ON allocation.tenant_id=reservation.tenant_id
         AND allocation.inventory_owner_id=reservation.inventory_owner_id
         AND allocation.reservation_id=reservation.id
        WHERE reservation.tenant_id=demand.tenant_id
          AND reservation.inventory_owner_id=demand.inventory_owner_id
          AND reservation.order_id=demand.order_id
          AND reservation.order_item_id=demand.order_item_id
          AND reservation.facility_id=$3
          AND reservation.status='active' AND reservation.deleted IS NULL
          AND allocation.facility_id=$3 AND allocation.status='allocated'
          AND allocation.deleted IS NULL AND allocation.execution_stage='pick_source'
      ) allocated ON true
      WHERE order_header.tenant_id=$1 AND order_header.inventory_owner_id=$2
        AND order_header.status='open' AND order_header.deleted IS NULL
        AND order_header.created<=$4
        AND NOT EXISTS(SELECT 1 FROM order_holds hold
          WHERE hold.tenant_id=order_header.tenant_id
            AND hold.inventory_owner_id=order_header.inventory_owner_id
            AND hold.order_id=order_header.id AND hold.released_at IS NULL)
        AND NOT EXISTS(SELECT 1 FROM cross_dock_tasks detail
          JOIN work_tasks work ON work.tenant_id=detail.tenant_id AND work.id=detail.task_id
          WHERE detail.tenant_id=order_header.tenant_id
            AND detail.inventory_owner_id=order_header.inventory_owner_id
            AND detail.order_id=order_header.id AND detail.closed_at IS NULL
            AND work.status IN ('open','assigned','in_progress'))
        AND NOT EXISTS(SELECT 1 FROM pick_wave_orders member
          WHERE member.tenant_id=order_header.tenant_id
            AND member.order_id=order_header.id AND member.active)
      GROUP BY order_header.id,order_header.order_key,order_header.revision,
               order_header.rush,order_header.ship_by,order_header.created
      HAVING SUM(demand.effective_qty)>0
         AND SUM(demand.effective_qty)=SUM(COALESCE(allocated.qty,0))
    "#;
    let count_sql = format!("SELECT COUNT(*)::bigint FROM ({eligible_sql}) eligible");
    let eligible_order_count: i64 = sqlx::query_scalar(&count_sql)
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(facility_id)
        .bind(input_snapshot_at)
        .fetch_one(&mut **tx)
        .await?;
    let selection_sql = format!(
        "SELECT eligible.*,row_number() OVER(ORDER BY rush DESC,ship_by ASC NULLS LAST,created,id) AS selection_rank FROM ({eligible_sql}) eligible ORDER BY selection_rank LIMIT $5"
    );
    let rows = sqlx::query(&selection_sql)
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(facility_id)
        .bind(input_snapshot_at)
        .bind(i64::from(max_orders))
        .fetch_all(&mut **tx)
        .await?;
    let selected_orders = rows
        .iter()
        .map(map_candidate)
        .collect::<AppResult<Vec<_>>>()?;
    Ok((eligible_order_count, selected_orders))
}

fn map_candidate(row: &sqlx::postgres::PgRow) -> AppResult<DynamicReleaseCandidateReadModel> {
    Ok(DynamicReleaseCandidateReadModel {
        order_id: OrderId::new(row.try_get("id")?).map_err(internal)?,
        order_key: row.try_get("order_key")?,
        revision: OrderRevision::new(row.try_get("revision")?).map_err(internal)?,
        rank: u32::try_from(row.try_get::<i64, _>("selection_rank")?).map_err(internal)?,
        rush: row.try_get("rush")?,
        ship_by: row.try_get("ship_by")?,
        order_created_at: row.try_get("created")?,
        demand_quantity: row.try_get("demand_qty")?,
        allocated_quantity: row.try_get("allocated_qty")?,
    })
}

async fn lock_selected_orders_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    selected: &[DynamicReleaseCandidateReadModel],
) -> AppResult<()> {
    let mut ids = selected
        .iter()
        .map(|order| order.order_id.get())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    if ids.is_empty() {
        return Ok(());
    }
    let locked = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM orders WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked == ids {
        Ok(())
    } else {
        Err(AppError::conflict("dynamic release candidates changed"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_run_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    context: &CommandContext,
    command: &DynamicReleaseCommand,
    policy: &WavePolicyReadModel,
    pick_wave_id: Option<i64>,
    released_at: Timestamp,
    eligible_order_count: i64,
    selected_order_count: i64,
    deferred_order_count: i64,
) -> AppResult<DynamicReleaseRunId> {
    let scope = crate::repo::pick_wave::policy_scope_values(policy.configuration_scope);
    let id: i64 = sqlx::query_scalar(
        r#"INSERT INTO dynamic_release_runs(
          tenant_id,facility_id,inventory_owner_id,destination_location_id,pick_wave_id,
          status,input_snapshot_at,policy_source,policy_configuration_id,
          policy_configuration_revision,policy_scope_level,policy_scope_owner_id,
          policy_scope_facility_id,policy_definition,policy_hash,eligible_order_count,
          selected_order_count,deferred_order_count,released_by_user_id,released_at)
        VALUES($1,$2,$3,$4,$5,'building',$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$6)
        RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.destination_location_id.get())
    .bind(pick_wave_id)
    .bind(released_at)
    .bind(crate::repo::pick_wave::policy_source_text(policy.source))
    .bind(policy.configuration_id.map(|id| id.get()))
    .bind(policy.configuration_revision)
    .bind(scope.0)
    .bind(scope.1)
    .bind(scope.2)
    .bind(crate::repo::pick_wave::policy_definition_json(policy))
    .bind(&policy.policy_hash)
    .bind(eligible_order_count)
    .bind(selected_order_count)
    .bind(deferred_order_count)
    .bind(context.actor_id.get())
    .fetch_one(&mut **tx)
    .await?;
    DynamicReleaseRunId::new(id).map_err(internal)
}

async fn insert_candidates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    run_id: DynamicReleaseRunId,
    command: &DynamicReleaseCommand,
    wave_id: i64,
    candidates: &[DynamicReleaseCandidateReadModel],
) -> AppResult<()> {
    for candidate in candidates {
        sqlx::query(
            r#"INSERT INTO dynamic_release_candidates(
              tenant_id,dynamic_release_run_id,facility_id,inventory_owner_id,pick_wave_id,
              order_id,order_key,order_revision,selection_rank,rush,ship_by,
              order_created_at,demand_qty,allocated_qty)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)"#,
        )
        .bind(tenant_id.get())
        .bind(run_id.get())
        .bind(command.facility_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(wave_id)
        .bind(candidate.order_id.get())
        .bind(&candidate.order_key)
        .bind(candidate.revision.get())
        .bind(i64::from(candidate.rank))
        .bind(candidate.rush)
        .bind(candidate.ship_by)
        .bind(candidate.order_created_at)
        .bind(candidate.demand_quantity)
        .bind(candidate.allocated_quantity)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn lock_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
) -> AppResult<()> {
    let found = sqlx::query_scalar::<_, i64>(
        r#"SELECT assignment.id FROM inventory_owner_facilities assignment
        JOIN inventory_owners owner ON owner.tenant_id=assignment.tenant_id
          AND owner.id=assignment.inventory_owner_id AND owner.deleted IS NULL
        JOIN facilities facility ON facility.tenant_id=assignment.tenant_id
          AND facility.id=assignment.facility_id AND facility.deleted IS NULL
        WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=$2
          AND assignment.facility_id=$3 AND assignment.deleted IS NULL
        FOR SHARE OF assignment,owner,facility"#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id)
    .fetch_optional(&mut **tx)
    .await?;
    found
        .map(|_| ())
        .ok_or_else(|| AppError::not_found("dynamic release scope"))
}

fn require_scope(scope: &ScopeBindings, facility_id: i64, owner_id: i64) -> AppResult<()> {
    if scope.includes_facility(facility_id) && scope.includes_inventory_owner(owner_id) {
        Ok(())
    } else {
        Err(AppError::not_found("dynamic release"))
    }
}

async fn require_replayed_run_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored: Option<(i64, i64)> = sqlx::query_as(
        r#"SELECT (result_json->>'facility_id')::bigint,
                  (result_json->>'inventory_owner_id')::bigint
           FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3"#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((facility_id, owner_id)) = stored {
        require_scope(scope, facility_id, owner_id)?;
    }
    Ok(())
}

async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &DynamicReleaseRunReadModel,
) -> AppResult<()> {
    let ordering_key = format!("dynamic-release:{}", result.run_id);
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let aggregate_id = result.run_id.to_string();
    let event_key = format!("{ordering_key}:sealed");
    let payload = serde_json::to_value(result).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(result.inventory_owner_id),
            facility_id: Some(result.facility_id),
            actor_user_id: Some(result.released_by.get()),
            event_key: &event_key,
            aggregate_type: "dynamic_release",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.dynamic_release.sealed",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.released_at,
        },
    )
    .await?;
    Ok(())
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}
