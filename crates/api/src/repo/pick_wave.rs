//! Multi-order pick-wave planning, atomic release, cancellation, and scoped reads.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::order_release::{ReleaseOrderCommand, ReleaseOrderResult};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::pick_wave::{
    CancelPickWaveCommand, CancelPickWaveResult, PickWaveCursor, PickWaveOrderReadModel,
    PickWavePage, PickWaveQuery, PickWaveReadModel, PlanPickWaveCommand, PlanPickWaveResult,
    ReleasePickWaveCommand, ReleasePickWaveResult, CANCEL_PICK_WAVE_OPERATION,
    PLAN_PICK_WAVE_OPERATION, RELEASE_PICK_WAVE_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    cancel_pick_wave as cancel_transition, release_pick_wave as release_transition,
    validate_pick_wave_plan, FacilityId, InventoryOwnerId, LocationId, OrderId, OrderRevision,
    OrderStatus, PickWaveCancellationNote, PickWaveCancellationReason, PickWaveId, PickWaveName,
    PickWaveOrderPrecondition, PickWaveRevision, PickWaveStatus, TenantId, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::order_release::{release_order_tx, OrderReleaseMode};
use crate::repo::orders::next_outbox_sequence_tx;

#[derive(Debug)]
struct PlannedOrder {
    order_id: OrderId,
    owner_id: InventoryOwnerId,
    order_key: String,
    expected_revision: OrderRevision,
    sequence: u32,
}

#[derive(Debug)]
struct LockedWave {
    wave_id: PickWaveId,
    facility_id: FacilityId,
    destination_location_id: LocationId,
    status: PickWaveStatus,
    revision: PickWaveRevision,
}

pub async fn plan_wave(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanPickWaveCommand,
) -> AppResult<PlanPickWaveResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let preconditions = command
        .orders
        .iter()
        .map(|order| PickWaveOrderPrecondition {
            order_id: order.order_id,
            expected_revision: order.expected_revision,
            sequence: order.sequence,
        })
        .collect::<Vec<_>>();
    validate_pick_wave_plan(&preconditions)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let prepared = PreparedCommand::new_v1(context, PLAN_PICK_WAVE_OPERATION, command)?;
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
    require_stored_wave_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<PlanPickWaveResult>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    if !scope.includes_facility(command.facility_id.get()) {
        return Err(AppError::not_found("pick wave"));
    }
    let (facility_name, destination_name) = lock_destination_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.destination_location_id,
    )
    .await?;
    let planned_orders = lock_plan_orders_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        &scope,
        command,
    )
    .await?;
    let planned_at = now_iso();
    let wave_id = PickWaveId::new(
        sqlx::query_scalar(
            r#"INSERT INTO pick_waves (
                 tenant_id,facility_id,destination_location_id,name,status,revision,
                 order_count,planned_by_user_id,planned_at)
               VALUES ($1,$2,$3,$4,'planned',1,$5,$6,$7) RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.facility_id.get())
        .bind(command.destination_location_id.get())
        .bind(command.name.as_str())
        .bind(
            i64::try_from(planned_orders.len())
                .map_err(|_| AppError::bad_request("too many wave orders"))?,
        )
        .bind(context.actor_id.get())
        .bind(planned_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    for order in &planned_orders {
        let inserted = sqlx::query(
            r#"INSERT INTO pick_wave_orders (
                 tenant_id,facility_id,pick_wave_id,inventory_owner_id,order_id,
                 order_key,wave_sequence,expected_order_revision)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.facility_id.get())
        .bind(wave_id.get())
        .bind(order.owner_id.get())
        .bind(order.order_id.get())
        .bind(&order.order_key)
        .bind(i64::from(order.sequence))
        .bind(order.expected_revision.get())
        .execute(&mut *tx)
        .await;
        if let Err(error) = inserted {
            return Err(map_membership_insert_error(error));
        }
    }
    let result = PickWaveReadModel {
        wave_id,
        facility_id: command.facility_id,
        facility_name,
        destination_location_id: command.destination_location_id,
        destination_location_name: destination_name,
        name: command.name.clone(),
        status: PickWaveStatus::Planned,
        revision: PickWaveRevision::new(1).map_err(internal)?,
        order_count: i64::try_from(planned_orders.len()).map_err(internal)?,
        allocation_count: 0,
        pick_task_count: 0,
        released_quantity: 0,
        orders: planned_orders
            .into_iter()
            .map(|order| PickWaveOrderReadModel {
                order_id: order.order_id,
                inventory_owner_id: order.owner_id,
                order_key: order.order_key,
                sequence: order.sequence,
                expected_revision: order.expected_revision,
                resulting_revision: None,
                release_id: None,
                status: OrderStatus::Open,
                allocation_count: 0,
                pick_task_count: 0,
                released_quantity: 0,
            })
            .collect(),
        planned_by: context.actor_id,
        planned_at,
        released_by: None,
        released_at: None,
        cancelled_by: None,
        cancelled_at: None,
        cancellation_reason: None,
        cancellation_note: None,
    };
    ensure_consistent(&result)?;
    enqueue_wave_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "outbound.pick_wave.planned",
        "planned",
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn release_wave(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReleasePickWaveCommand,
) -> AppResult<ReleasePickWaveResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RELEASE_PICK_WAVE_OPERATION, command)?;
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
    require_stored_wave_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<ReleasePickWaveResult>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let wave = lock_wave_tx(&mut tx, access.tenant_id, command.wave_id, &scope).await?;
    if wave.revision != command.expected_revision {
        return Err(AppError::conflict("pick wave revision is stale"));
    }
    let resulting_revision = release_transition(wave.status, wave.revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let members = lock_wave_members_tx(&mut tx, access.tenant_id, wave.wave_id, &scope).await?;
    let released_at = now_iso();
    let mut release_results = Vec::with_capacity(members.len());
    let mut by_id = members.iter().collect::<Vec<_>>();
    by_id.sort_by_key(|member| member.order_id.get());
    for member in by_id {
        let command = ReleaseOrderCommand {
            order_id: member.order_id,
            facility_id: wave.facility_id,
            destination_location_id: wave.destination_location_id,
            expected_revision: member.expected_revision,
        };
        release_results.push(
            release_order_tx(
                &mut tx,
                access.tenant_id,
                context.actor_id.get(),
                &scope,
                &command,
                OrderReleaseMode::Wave(wave.wave_id),
                released_at,
            )
            .await?,
        );
    }
    let results_by_order = release_results
        .iter()
        .map(|result| (result.order_id, result))
        .collect::<HashMap<_, _>>();
    for member in &members {
        let result = results_by_order
            .get(&member.order_id)
            .ok_or_else(|| AppError::internal("wave release lost an order result"))?;
        let updated = sqlx::query(
            r#"UPDATE pick_wave_orders SET active=false,resulting_order_revision=$1,
                 order_release_id=$2,allocation_count=$3,pick_task_count=$4,released_qty=$5
               WHERE tenant_id=$6 AND pick_wave_id=$7 AND order_id=$8 AND active"#,
        )
        .bind(result.revision.get())
        .bind(result.release_id.get())
        .bind(result.allocation_count)
        .bind(result.pick_task_count)
        .bind(result.released_quantity)
        .bind(access.tenant_id.get())
        .bind(wave.wave_id.get())
        .bind(member.order_id.get())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "pick wave membership changed during release",
            ));
        }
    }
    let (allocation_count, pick_task_count, released_quantity) =
        sum_release_results(&release_results)?;
    let updated = sqlx::query(
        r#"UPDATE pick_waves SET status='released',revision=$1,allocation_count=$2,
             pick_task_count=$3,released_qty=$4,released_by_user_id=$5,released_at=$6
           WHERE tenant_id=$7 AND id=$8 AND status='planned' AND revision=$9"#,
    )
    .bind(resulting_revision.get())
    .bind(allocation_count)
    .bind(pick_task_count)
    .bind(released_quantity)
    .bind(context.actor_id.get())
    .bind(released_at)
    .bind(access.tenant_id.get())
    .bind(wave.wave_id.get())
    .bind(wave.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("pick wave changed during release"));
    }
    let result = load_wave_tx(&mut tx, access.tenant_id, wave.wave_id, &scope, false).await?;
    ensure_consistent(&result)?;
    enqueue_wave_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "outbound.pick_wave.released",
        "released",
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn cancel_wave(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelPickWaveCommand,
) -> AppResult<CancelPickWaveResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_PICK_WAVE_OPERATION, command)?;
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
    require_stored_wave_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<CancelPickWaveResult>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let wave = lock_wave_tx(&mut tx, access.tenant_id, command.wave_id, &scope).await?;
    if wave.revision != command.expected_revision {
        return Err(AppError::conflict("pick wave revision is stale"));
    }
    let resulting_revision = cancel_transition(
        wave.status,
        wave.revision,
        command.reason,
        command.note.as_ref(),
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    lock_wave_members_tx(&mut tx, access.tenant_id, wave.wave_id, &scope).await?;
    let closed = sqlx::query(
        "UPDATE pick_wave_orders SET active=false WHERE tenant_id=$1 AND pick_wave_id=$2 AND active",
    )
    .bind(access.tenant_id.get())
    .bind(wave.wave_id.get())
    .execute(&mut *tx)
    .await?;
    if closed.rows_affected() == 0 {
        return Err(AppError::conflict("pick wave has no active orders"));
    }
    let cancelled_at = now_iso();
    let updated = sqlx::query(
        r#"UPDATE pick_waves SET status='cancelled',revision=$1,cancelled_by_user_id=$2,
             cancelled_at=$3,cancellation_reason=$4,cancellation_note=$5
           WHERE tenant_id=$6 AND id=$7 AND status='planned' AND revision=$8"#,
    )
    .bind(resulting_revision.get())
    .bind(context.actor_id.get())
    .bind(cancelled_at)
    .bind(command.reason.as_str())
    .bind(command.note.as_ref().map(|note| note.as_str()))
    .bind(access.tenant_id.get())
    .bind(wave.wave_id.get())
    .bind(wave.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("pick wave changed during cancellation"));
    }
    let result = load_wave_tx(&mut tx, access.tenant_id, wave.wave_id, &scope, false).await?;
    ensure_consistent(&result)?;
    enqueue_wave_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "outbound.pick_wave.cancelled",
        "cancelled",
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn get_wave(
    db: &Db,
    access: &TenantAccess,
    wave_id: PickWaveId,
) -> AppResult<PickWaveReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "orders").await?;
    let result = load_wave_tx(&mut tx, access.tenant_id, wave_id, &scope, false).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn list_waves(
    db: &Db,
    access: &TenantAccess,
    query: &PickWaveQuery,
) -> AppResult<PickWavePage> {
    if query.limit == 0 || query.limit > 100 {
        return Err(AppError::bad_request(
            "pick wave page limit must be 1..=100",
        ));
    }
    if query
        .facility_id
        .is_some_and(|facility_id| !access.site_scope.includes(facility_id))
    {
        return Err(AppError::not_found("pick wave"));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "orders").await?;
    let fetch_limit = i64::from(query.limit) + 1;
    let offset = query.cursor.map_or(0_i64, |cursor| {
        i64::try_from(cursor.offset).unwrap_or(i64::MAX)
    });
    let rows = sqlx::query(
        r#"SELECT wave.id
           FROM pick_waves wave
           WHERE wave.tenant_id=$1
             AND ($2::bigint IS NULL OR wave.facility_id=$2)
             AND ($3::text IS NULL OR wave.status=$3)
             AND ($4 OR wave.facility_id=ANY($5))
             AND NOT EXISTS (
               SELECT 1 FROM pick_wave_orders member
               WHERE member.tenant_id=wave.tenant_id AND member.pick_wave_id=wave.id
                 AND NOT ($6 OR member.inventory_owner_id=ANY($7)))
           ORDER BY
             CASE WHEN $8='name' AND $9='asc' THEN lower(wave.name) END ASC,
             CASE WHEN $8='name' AND $9='desc' THEN lower(wave.name) END DESC,
             CASE WHEN $8='status' AND $9='asc' THEN wave.status END ASC,
             CASE WHEN $8='status' AND $9='desc' THEN wave.status END DESC,
             CASE WHEN $8='orders' AND $9='asc' THEN wave.order_count END ASC,
             CASE WHEN $8='orders' AND $9='desc' THEN wave.order_count END DESC,
             CASE WHEN $8='tasks' AND $9='asc' THEN wave.pick_task_count END ASC,
             CASE WHEN $8='tasks' AND $9='desc' THEN wave.pick_task_count END DESC,
             CASE WHEN $8='units' AND $9='asc' THEN wave.released_qty END ASC,
             CASE WHEN $8='units' AND $9='desc' THEN wave.released_qty END DESC,
             CASE WHEN $8='planned_at' AND $9='asc' THEN wave.planned_at END ASC,
             CASE WHEN $8='planned_at' AND $9='desc' THEN wave.planned_at END DESC,
             CASE WHEN $9='asc' THEN wave.id END ASC,
             wave.id DESC
           LIMIT $10 OFFSET $11"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.status.map(PickWaveStatus::as_str))
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.sort.as_str())
    .bind(query.direction.as_str())
    .bind(fetch_limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let visible = rows
        .into_iter()
        .take(usize::from(query.limit))
        .collect::<Vec<_>>();
    let mut entries = Vec::with_capacity(visible.len());
    for row in &visible {
        let wave_id = PickWaveId::new(row.try_get("id")?).map_err(internal)?;
        entries.push(load_wave_tx(&mut tx, access.tenant_id, wave_id, &scope, false).await?);
    }
    let next_cursor = if has_more {
        Some(PickWaveCursor {
            offset: u64::try_from(offset)
                .map_err(internal)?
                .checked_add(u64::from(query.limit))
                .ok_or_else(|| AppError::bad_request("pick wave cursor offset overflow"))?,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(PickWavePage {
        entries,
        next_cursor,
    })
}

async fn lock_destination_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    location_id: LocationId,
) -> AppResult<(String, String)> {
    let row = sqlx::query(
        r#"SELECT facility.name AS facility_name,location.name AS location_name,
                  location.active,location.pickable,location.receivable,location.barcode
           FROM facilities facility JOIN locations location
             ON location.tenant_id=facility.tenant_id AND location.facility_id=facility.id
           WHERE facility.tenant_id=$1 AND facility.id=$2 AND facility.deleted IS NULL
             AND location.id=$3 AND location.deleted IS NULL
           FOR SHARE OF facility,location"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(location_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick wave destination"))?;
    if !row.try_get::<bool, _>("active")?
        || row.try_get::<bool, _>("pickable")?
        || row.try_get::<bool, _>("receivable")?
        || row
            .try_get::<Option<String>, _>("barcode")?
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(AppError::conflict(
            "pick wave destination is not scanner-ready staging inventory",
        ));
    }
    Ok((row.try_get("facility_name")?, row.try_get("location_name")?))
}

async fn lock_plan_orders_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    scope: &ScopeBindings,
    command: &PlanPickWaveCommand,
) -> AppResult<Vec<PlannedOrder>> {
    let expected = command
        .orders
        .iter()
        .map(|order| (order.order_id, (order.expected_revision, order.sequence)))
        .collect::<HashMap<_, _>>();
    let mut ids = expected.keys().map(|id| id.get()).collect::<Vec<_>>();
    ids.sort_unstable();
    let rows = sqlx::query(
        r#"SELECT id,inventory_owner_id,order_key,status,revision
           FROM orders WHERE tenant_id=$1 AND id=ANY($2) AND deleted IS NULL
             AND ($3 OR inventory_owner_id=ANY($4)) ORDER BY id FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(&ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != ids.len() {
        return Err(AppError::not_found("pick wave order"));
    }
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let order_id = OrderId::new(row.try_get("id")?).map_err(internal)?;
        let owner_id =
            InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?;
        let (expected_revision, sequence) = expected
            .get(&order_id)
            .copied()
            .ok_or_else(|| AppError::internal("wave plan lost an order precondition"))?;
        let status: String = row.try_get("status")?;
        let revision = OrderRevision::new(row.try_get("revision")?).map_err(internal)?;
        if status != "open" || revision != expected_revision {
            return Err(AppError::conflict(
                "pick wave order revision or status is stale",
            ));
        }
        lock_owner_facility_tx(tx, tenant_id, owner_id, facility_id).await?;
        result.push(PlannedOrder {
            order_id,
            owner_id,
            order_key: row.try_get("order_key")?,
            expected_revision,
            sequence,
        });
    }
    result.sort_by_key(|order| order.sequence);
    Ok(result)
}

async fn lock_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let found = sqlx::query_scalar::<_, i64>(
        r#"SELECT assignment.id FROM inventory_owner_facilities assignment
           WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=$2
             AND assignment.facility_id=$3 AND assignment.deleted IS NULL
           FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    if found.is_some() {
        Ok(())
    } else {
        Err(AppError::not_found("pick wave order"))
    }
}

async fn lock_wave_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    wave_id: PickWaveId,
    scope: &ScopeBindings,
) -> AppResult<LockedWave> {
    let row = sqlx::query(
        r#"SELECT id,facility_id,destination_location_id,status,revision
           FROM pick_waves wave WHERE tenant_id=$1 AND id=$2
             AND ($3 OR facility_id=ANY($4))
             AND NOT EXISTS (SELECT 1 FROM pick_wave_orders member
               WHERE member.tenant_id=wave.tenant_id AND member.pick_wave_id=wave.id
                 AND NOT ($5 OR member.inventory_owner_id=ANY($6))) FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(wave_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick wave"))?;
    LockedWave::from_row(&row)
}

impl LockedWave {
    fn from_row(row: &sqlx::postgres::PgRow) -> AppResult<Self> {
        let status: String = row.try_get("status")?;
        Ok(Self {
            wave_id: PickWaveId::new(row.try_get("id")?).map_err(internal)?,
            facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
            destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
                .map_err(internal)?,
            status: PickWaveStatus::parse(&status)
                .ok_or_else(|| AppError::internal("pick wave has invalid status"))?,
            revision: PickWaveRevision::new(row.try_get("revision")?).map_err(internal)?,
        })
    }
}

async fn lock_wave_members_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    wave_id: PickWaveId,
    scope: &ScopeBindings,
) -> AppResult<Vec<PlannedOrder>> {
    let rows = sqlx::query(
        r#"SELECT order_id,inventory_owner_id,order_key,wave_sequence,expected_order_revision
           FROM pick_wave_orders WHERE tenant_id=$1 AND pick_wave_id=$2 AND active
           ORDER BY order_id FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(wave_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Err(AppError::conflict("pick wave has no active orders"));
    }
    rows.into_iter()
        .map(|row| {
            let owner_id =
                InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?;
            if !scope.includes_inventory_owner(owner_id.get()) {
                return Err(AppError::not_found("pick wave"));
            }
            Ok(PlannedOrder {
                order_id: OrderId::new(row.try_get("order_id")?).map_err(internal)?,
                owner_id,
                order_key: row.try_get("order_key")?,
                expected_revision: OrderRevision::new(row.try_get("expected_order_revision")?)
                    .map_err(internal)?,
                sequence: u32::try_from(row.try_get::<i64, _>("wave_sequence")?)
                    .map_err(internal)?,
            })
        })
        .collect()
}

async fn load_wave_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    wave_id: PickWaveId,
    scope: &ScopeBindings,
    lock: bool,
) -> AppResult<PickWaveReadModel> {
    let lock_clause = if lock { " FOR SHARE OF wave" } else { "" };
    let sql = format!(
        r#"SELECT wave.*,facility.name AS facility_name,location.name AS destination_name
           FROM pick_waves wave
           JOIN facilities facility ON facility.tenant_id=wave.tenant_id AND facility.id=wave.facility_id
           JOIN locations location ON location.tenant_id=wave.tenant_id AND location.id=wave.destination_location_id
           WHERE wave.tenant_id=$1 AND wave.id=$2 AND ($3 OR wave.facility_id=ANY($4))
             AND NOT EXISTS (SELECT 1 FROM pick_wave_orders member
               WHERE member.tenant_id=wave.tenant_id AND member.pick_wave_id=wave.id
                 AND NOT ($5 OR member.inventory_owner_id=ANY($6))){lock_clause}"#,
    );
    let row = sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(wave_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("pick wave"))?;
    let member_rows = sqlx::query(
        r#"SELECT inventory_owner_id,order_id,order_key,wave_sequence,
                  expected_order_revision,resulting_order_revision,order_release_id,
                  allocation_count,pick_task_count,released_qty
           FROM pick_wave_orders WHERE tenant_id=$1 AND pick_wave_id=$2
           ORDER BY wave_sequence"#,
    )
    .bind(tenant_id.get())
    .bind(wave_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let status_text: String = row.try_get("status")?;
    let status = PickWaveStatus::parse(&status_text)
        .ok_or_else(|| AppError::internal("pick wave has invalid status"))?;
    let orders = member_rows
        .into_iter()
        .map(|member| {
            let resulting_revision = member
                .try_get::<Option<i64>, _>("resulting_order_revision")?
                .map(OrderRevision::new)
                .transpose()
                .map_err(internal)?;
            Ok(PickWaveOrderReadModel {
                order_id: OrderId::new(member.try_get("order_id")?).map_err(internal)?,
                inventory_owner_id: InventoryOwnerId::new(member.try_get("inventory_owner_id")?)
                    .map_err(internal)?,
                order_key: member.try_get("order_key")?,
                sequence: u32::try_from(member.try_get::<i64, _>("wave_sequence")?)
                    .map_err(internal)?,
                expected_revision: OrderRevision::new(member.try_get("expected_order_revision")?)
                    .map_err(internal)?,
                resulting_revision,
                release_id: member
                    .try_get::<Option<i64>, _>("order_release_id")?
                    .map(wareboxes_domain::OrderReleaseId::new)
                    .transpose()
                    .map_err(internal)?,
                status: if resulting_revision.is_some() {
                    OrderStatus::Processing
                } else {
                    OrderStatus::Open
                },
                allocation_count: member.try_get("allocation_count")?,
                pick_task_count: member.try_get("pick_task_count")?,
                released_quantity: member.try_get("released_qty")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let cancellation_reason = row
        .try_get::<Option<String>, _>("cancellation_reason")?
        .map(|value| {
            PickWaveCancellationReason::parse(&value)
                .ok_or_else(|| AppError::internal("pick wave has invalid cancellation reason"))
        })
        .transpose()?;
    let result = PickWaveReadModel {
        wave_id,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        facility_name: row.try_get("facility_name")?,
        destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(internal)?,
        destination_location_name: row.try_get("destination_name")?,
        name: PickWaveName::new(row.try_get::<String, _>("name")?).map_err(internal)?,
        status,
        revision: PickWaveRevision::new(row.try_get("revision")?).map_err(internal)?,
        order_count: row.try_get("order_count")?,
        allocation_count: row.try_get("allocation_count")?,
        pick_task_count: row.try_get("pick_task_count")?,
        released_quantity: row.try_get("released_qty")?,
        orders,
        planned_by: UserId::new(row.try_get("planned_by_user_id")?).map_err(internal)?,
        planned_at: row.try_get("planned_at")?,
        released_by: row
            .try_get::<Option<i64>, _>("released_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        released_at: row.try_get("released_at")?,
        cancelled_by: row
            .try_get::<Option<i64>, _>("cancelled_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        cancelled_at: row.try_get("cancelled_at")?,
        cancellation_reason,
        cancellation_note: row
            .try_get::<Option<String>, _>("cancellation_note")?
            .map(PickWaveCancellationNote::new)
            .transpose()
            .map_err(internal)?,
    };
    ensure_consistent(&result)?;
    Ok(result)
}

fn sum_release_results(results: &[ReleaseOrderResult]) -> AppResult<(i64, i64, i64)> {
    results
        .iter()
        .try_fold((0_i64, 0_i64, 0_i64), |totals, result| {
            Ok((
                totals
                    .0
                    .checked_add(result.allocation_count)
                    .ok_or_else(|| AppError::internal("wave allocation count overflow"))?,
                totals
                    .1
                    .checked_add(result.pick_task_count)
                    .ok_or_else(|| AppError::internal("wave task count overflow"))?,
                totals
                    .2
                    .checked_add(result.released_quantity)
                    .ok_or_else(|| AppError::internal("wave quantity overflow"))?,
            ))
        })
}

fn ensure_consistent(result: &PickWaveReadModel) -> AppResult<()> {
    if result.is_consistent() {
        Ok(())
    } else {
        Err(AppError::internal("pick wave read model is inconsistent"))
    }
}

async fn require_stored_wave_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let wave_id: Option<i64> = sqlx::query_scalar(
        r#"SELECT (result_json->>'wave_id')::bigint FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3"#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    if let Some(wave_id) = wave_id {
        load_wave_tx(
            tx,
            prepared.tenant_id(),
            PickWaveId::new(wave_id).map_err(internal)?,
            scope,
            false,
        )
        .await?;
    }
    Ok(())
}

async fn enqueue_wave_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    wave: &PickWaveReadModel,
    event_type: &str,
    suffix: &str,
) -> AppResult<()> {
    let ordering_key = format!("pick-wave:{}", wave.wave_id);
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let aggregate_id = wave.wave_id.to_string();
    let event_key = format!("{ordering_key}:{suffix}");
    let payload = serde_json::to_value(wave).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: None,
            facility_id: Some(wave.facility_id),
            actor_user_id: Some(actor_id.get()),
            event_key: &event_key,
            aggregate_type: "pick_wave",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at: wave
                .released_at
                .or(wave.cancelled_at)
                .unwrap_or(wave.planned_at),
        },
    )
    .await?;
    Ok(())
}

fn map_membership_insert_error(error: sqlx::Error) -> AppError {
    if error
        .as_database_error()
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
    {
        AppError::conflict("order already belongs to an active pick wave")
    } else {
        error.into()
    }
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}
