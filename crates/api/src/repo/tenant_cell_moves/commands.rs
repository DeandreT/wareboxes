use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::tenant_cell_move::{
    CancelTenantCellMoveCommand, CancelTenantCellMoveResult, CheckpointTenantCellMoveCommand,
    CheckpointTenantCellMoveResult, CompleteTenantCellMoveCommand, CompleteTenantCellMoveResult,
    CutoverTenantCellMoveCommand, CutoverTenantCellMoveResult, FreezeTenantCellMoveCommand,
    FreezeTenantCellMoveResult, PlanTenantCellMoveCommand, PlanTenantCellMoveResult,
    RollbackTenantCellMoveCommand, RollbackTenantCellMoveResult, StartTenantCellMoveCopyCommand,
    StartTenantCellMoveCopyResult, ValidateTenantCellMoveCommand, ValidateTenantCellMoveResult,
    VerifyTenantCellMoveCutoverCommand, VerifyTenantCellMoveCutoverResult,
    CANCEL_TENANT_CELL_MOVE_OPERATION, CHECKPOINT_TENANT_CELL_MOVE_OPERATION,
    COMPLETE_TENANT_CELL_MOVE_OPERATION, CUTOVER_TENANT_CELL_MOVE_OPERATION,
    FREEZE_TENANT_CELL_MOVE_OPERATION, PLAN_TENANT_CELL_MOVE_OPERATION,
    ROLLBACK_TENANT_CELL_MOVE_OPERATION, START_TENANT_CELL_MOVE_COPY_OPERATION,
    VALIDATE_TENANT_CELL_MOVE_OPERATION, VERIFY_TENANT_CELL_MOVE_CUTOVER_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    DataCellId, DataCellPlacementRevision, DataResidencyCode, PostgresLsn, TenantCellMoveId,
    TenantCellMoveRevision, TenantCellMoveStatus, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::events::{self, TenantCellMoveEvent};
use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
struct LockedMove {
    id: TenantCellMoveId,
    tenant_id: TenantId,
    source_data_cell_id: DataCellId,
    target_data_cell_id: DataCellId,
    source_placement_revision: DataCellPlacementRevision,
    cutover_placement_revision: Option<DataCellPlacementRevision>,
    residency_requirement: DataResidencyCode,
    status: TenantCellMoveStatus,
    revision: TenantCellMoveRevision,
    latest_source_lsn: Option<PostgresLsn>,
    latest_target_replay_lsn: Option<PostgresLsn>,
    copied_row_count: Option<i64>,
    copied_bytes: Option<i64>,
    checkpointed_at: Option<Timestamp>,
    latest_checkpoint_revision: Option<TenantCellMoveRevision>,
    write_fence_epoch: Option<TenantCellMoveRevision>,
    post_cutover_verified_at: Option<Timestamp>,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::internal(message.into())
}

fn revision_conflict() -> AppError {
    AppError::revision_conflict("tenant-cell-move revision does not match expected revision")
}

fn placement_revision_conflict() -> AppError {
    AppError::revision_conflict("tenant placement revision does not match expected revision")
}

async fn database_now_tx(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> AppResult<Timestamp> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?)
}

async fn authorize_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_access: &TenantAccess,
    context: &CommandContext,
) -> AppResult<()> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    super::super::tenant_lifecycle::authorize_tx(tx, actor_access, context.actor_id).await
}

fn require_switched_tenant(actor_access: &TenantAccess, tenant_id: TenantId) -> AppResult<()> {
    if actor_access.tenant_id == tenant_id {
        Err(AppError::bad_request(
            "switch to another active tenant before managing this tenant's data-cell move",
        ))
    } else {
        Ok(())
    }
}

fn next_revision(current: TenantCellMoveRevision) -> AppResult<TenantCellMoveRevision> {
    current
        .checked_next()
        .ok_or_else(|| invalid("tenant-cell-move revision overflow"))
}

fn parse_optional_lsn(value: Option<String>) -> AppResult<Option<PostgresLsn>> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|error: wareboxes_domain::TenantCellMoveError| invalid(error.to_string()))
        })
        .transpose()
}

async fn lock_move_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_cell_move_id: TenantCellMoveId,
) -> AppResult<LockedMove> {
    let row = sqlx::query(
        r#"SELECT id,tenant_id,source_data_cell_id,target_data_cell_id,
        source_placement_revision,cutover_placement_revision,residency_requirement,
        status,revision,latest_source_wal_lsn::TEXT AS latest_source_lsn,
        latest_target_replay_lsn::TEXT AS latest_target_replay_lsn,
        copied_row_count,copied_bytes,checkpointed_at,
        (SELECT event.move_revision FROM tenant_cell_move_events event
          WHERE event.tenant_cell_move_id=tenant_cell_moves.id
            AND event.action='checkpoint_recorded'
          ORDER BY event.move_revision DESC LIMIT 1) AS latest_checkpoint_revision,
        (SELECT fence.fence_epoch FROM tenant_write_fences fence
          WHERE fence.tenant_id=tenant_cell_moves.tenant_id
            AND fence.tenant_cell_move_id=tenant_cell_moves.id) AS write_fence_epoch,
        post_cutover_verified_at
        FROM tenant_cell_moves WHERE id=$1 FOR UPDATE"#,
    )
    .bind(tenant_cell_move_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("tenant cell move"))?;
    Ok(LockedMove {
        id: TenantCellMoveId::new(row.try_get("id")?)
            .map_err(|error| invalid(error.to_string()))?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        source_data_cell_id: DataCellId::new(row.try_get("source_data_cell_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        target_data_cell_id: DataCellId::new(row.try_get("target_data_cell_id")?)
            .map_err(|error| invalid(error.to_string()))?,
        source_placement_revision: DataCellPlacementRevision::new(
            row.try_get("source_placement_revision")?,
        )
        .map_err(|error| invalid(error.to_string()))?,
        cutover_placement_revision: row
            .try_get::<Option<i64>, _>("cutover_placement_revision")?
            .map(DataCellPlacementRevision::new)
            .transpose()
            .map_err(|error| invalid(error.to_string()))?,
        residency_requirement: DataResidencyCode::new(
            row.try_get::<String, _>("residency_requirement")?,
        )
        .map_err(|error| invalid(error.to_string()))?,
        status: TenantCellMoveStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or_else(|| invalid("stored tenant-cell-move status is invalid"))?,
        revision: TenantCellMoveRevision::new(row.try_get("revision")?)
            .map_err(|error| invalid(error.to_string()))?,
        latest_source_lsn: parse_optional_lsn(row.try_get("latest_source_lsn")?)?,
        latest_target_replay_lsn: parse_optional_lsn(row.try_get("latest_target_replay_lsn")?)?,
        copied_row_count: row.try_get("copied_row_count")?,
        copied_bytes: row.try_get("copied_bytes")?,
        checkpointed_at: row.try_get("checkpointed_at")?,
        latest_checkpoint_revision: row
            .try_get::<Option<i64>, _>("latest_checkpoint_revision")?
            .map(TenantCellMoveRevision::new)
            .transpose()
            .map_err(|error| invalid(error.to_string()))?,
        write_fence_epoch: row
            .try_get::<Option<i64>, _>("write_fence_epoch")?
            .map(TenantCellMoveRevision::new)
            .transpose()
            .map_err(|error| invalid(error.to_string()))?,
        post_cutover_verified_at: row.try_get("post_cutover_verified_at")?,
    })
}

fn require_revision(current: &LockedMove, expected: TenantCellMoveRevision) -> AppResult<()> {
    if current.revision == expected {
        Ok(())
    } else {
        Err(revision_conflict())
    }
}

async fn lock_write_fence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(tenant_cell_fence_lock_key($1))")
        .bind(tenant_id.get())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn restore_actor_tenant_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_access: &TenantAccess,
) -> AppResult<()> {
    super::super::tenant_lifecycle::bind_platform_tenant_tx(tx, actor_access.tenant_id).await
}

struct EventTransition<'a> {
    action: &'a str,
    revision: TenantCellMoveRevision,
    previous_status: Option<TenantCellMoveStatus>,
    resulting_status: TenantCellMoveStatus,
    occurred_at: Timestamp,
    reason: Option<&'a str>,
    evidence: &'a serde_json::Value,
}

async fn record_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    current: &LockedMove,
    context: &CommandContext,
    transition: EventTransition<'_>,
) -> AppResult<()> {
    events::record_tx(
        tx,
        &TenantCellMoveEvent {
            tenant_cell_move_id: current.id,
            tenant_id: current.tenant_id,
            action: transition.action,
            revision: transition.revision.get(),
            previous_status: transition.previous_status,
            resulting_status: transition.resulting_status,
            actor_id: context.actor_id,
            occurred_at: transition.occurred_at,
            reason: transition.reason,
            request_id: &context.request_id,
            evidence: transition.evidence,
        },
    )
    .await
}

async fn require_target_viability_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    current: &LockedMove,
) -> AppResult<()> {
    sqlx::query("SELECT id FROM data_cells WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(vec![
            current.source_data_cell_id.get(),
            current.target_data_cell_id.get(),
        ])
        .fetch_all(&mut **tx)
        .await?;
    let row = sqlx::query(
        r#"SELECT status,mode,max_tenants,residency_code,
        (SELECT COUNT(*) FROM tenant_cell_placements placement
          WHERE placement.data_cell_id=cell.id) AS placement_count,
        (SELECT COUNT(*) FROM tenant_cell_moves move
          WHERE move.id<>$2 AND ((move.target_data_cell_id=cell.id
            AND move.status IN ('planned','copying','frozen','validated'))
            OR (move.source_data_cell_id=cell.id AND move.status='cut_over')))
          AS reserved_move_count
        FROM data_cells cell WHERE cell.id=$1"#,
    )
    .bind(current.target_data_cell_id.get())
    .bind(current.id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("target data cell"))?;
    let status: String = row.try_get("status")?;
    let mode: String = row.try_get("mode")?;
    let max_tenants: i64 = row.try_get("max_tenants")?;
    let placement_count: i64 = row.try_get("placement_count")?;
    let reserved_move_count: i64 = row.try_get("reserved_move_count")?;
    let residency = DataResidencyCode::new(row.try_get::<String, _>("residency_code")?)
        .map_err(|error| invalid(error.to_string()))?;
    if status != "active" {
        return Err(AppError::conflict(
            "target data cell is not accepting tenant moves",
        ));
    }
    if placement_count + reserved_move_count >= max_tenants
        || (mode == "dedicated" && placement_count + reserved_move_count != 0)
    {
        return Err(AppError::conflict(
            "target data cell has no available tenant capacity",
        ));
    }
    if !current.residency_requirement.allows(&residency) {
        return Err(AppError::conflict(
            "target data cell no longer satisfies the tenant residency requirement",
        ));
    }
    Ok(())
}

pub async fn plan(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &PlanTenantCellMoveCommand,
) -> AppResult<PlanTenantCellMoveResult> {
    let prepared = PreparedCommand::new_v1(context, PLAN_TENANT_CELL_MOVE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    require_switched_tenant(actor_access, command.tenant_id)?;
    let placement = sqlx::query(
        r#"SELECT placement.data_cell_id,placement.revision,
        placement.residency_requirement FROM tenants tenant
        JOIN tenant_cell_placements placement ON placement.tenant_id=tenant.id
        WHERE tenant.id=$1 AND tenant.deleted IS NULL"#,
    )
    .bind(command.tenant_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("tenant placement"))?;
    let source_data_cell_id = DataCellId::new(placement.try_get("data_cell_id")?)
        .map_err(|error| invalid(error.to_string()))?;
    let placement_revision = DataCellPlacementRevision::new(placement.try_get("revision")?)
        .map_err(|error| invalid(error.to_string()))?;
    if placement_revision != command.expected_placement_revision {
        return Err(placement_revision_conflict());
    }
    if source_data_cell_id == command.target_data_cell_id {
        return Err(AppError::bad_request(
            "source and target data cells must differ",
        ));
    }
    let residency_requirement =
        DataResidencyCode::new(placement.try_get::<String, _>("residency_requirement")?)
            .map_err(|error| invalid(error.to_string()))?;
    sqlx::query("SELECT id FROM data_cells WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(vec![
            source_data_cell_id.get(),
            command.target_data_cell_id.get(),
        ])
        .fetch_all(&mut *tx)
        .await?;
    crate::repo::data_cells::require_available_tx(
        &mut tx,
        command.target_data_cell_id,
        &residency_requirement,
    )
    .await?;
    let active_move_exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM tenant_cell_moves WHERE tenant_id=$1
        AND status IN ('planned','copying','frozen','validated','cut_over'))"#,
    )
    .bind(command.tenant_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if active_move_exists {
        return Err(AppError::conflict(
            "tenant already has an active data-cell move",
        ));
    }
    let requested_at = database_now_tx(&mut tx).await?;
    let tenant_cell_move_id = TenantCellMoveId::new(
        sqlx::query_scalar(
            r#"INSERT INTO tenant_cell_moves
            (tenant_id,source_data_cell_id,target_data_cell_id,
             source_placement_revision,residency_requirement,status,revision,last_action,
             reason,requested_at,requested_by_user_id)
            VALUES($1,$2,$3,$4,$5,'planned',1,'planned',$6,$7,$8) RETURNING id"#,
        )
        .bind(command.tenant_id.get())
        .bind(source_data_cell_id.get())
        .bind(command.target_data_cell_id.get())
        .bind(placement_revision.get())
        .bind(residency_requirement.as_str())
        .bind(command.reason.as_str())
        .bind(requested_at)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| invalid(error.to_string()))?;
    let current = LockedMove {
        id: tenant_cell_move_id,
        tenant_id: command.tenant_id,
        source_data_cell_id,
        target_data_cell_id: command.target_data_cell_id,
        source_placement_revision: placement_revision,
        cutover_placement_revision: None,
        residency_requirement,
        status: TenantCellMoveStatus::Planned,
        revision: TenantCellMoveRevision::new(1).map_err(|error| invalid(error.to_string()))?,
        latest_source_lsn: None,
        latest_target_replay_lsn: None,
        copied_row_count: None,
        copied_bytes: None,
        checkpointed_at: None,
        latest_checkpoint_revision: None,
        write_fence_epoch: None,
        post_cutover_verified_at: None,
    };
    let evidence = serde_json::json!({
        "tenant_cell_move_id": tenant_cell_move_id.get(),
        "tenant_id": command.tenant_id.get(),
        "action": "planned",
        "move_revision": 1,
        "previous_status": null,
        "resulting_status": "planned",
        "source_data_cell_id": source_data_cell_id.get(),
        "target_data_cell_id": command.target_data_cell_id.get(),
        "source_placement_revision": placement_revision.get(),
        "resulting_placement_revision": null,
        "residency_requirement": current.residency_requirement.as_str(),
        "reason": command.reason.as_str(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": requested_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "planned",
            revision: current.revision,
            previous_status: None,
            resulting_status: TenantCellMoveStatus::Planned,
            occurred_at: requested_at,
            reason: Some(command.reason.as_str()),
            evidence: &evidence,
        },
    )
    .await?;
    let result =
        super::query::read_tx(&mut tx, actor_access.tenant_id, tenant_cell_move_id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn start_copy(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &StartTenantCellMoveCopyCommand,
) -> AppResult<StartTenantCellMoveCopyResult> {
    let prepared =
        PreparedCommand::new_v1(context, START_TENANT_CELL_MOVE_COPY_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    current
        .status
        .require_transition(TenantCellMoveStatus::Copying)
        .map_err(|error| AppError::invalid_state_transition(error.to_string()))?;
    require_target_viability_tx(&mut tx, &current).await?;
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET status='copying',revision=$3,
        last_action='copy_started',changed_at=$4,changed_by_user_id=$5,
        copy_reference=$6,copy_started_at=$4,copy_started_by_user_id=$5
        WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(command.copy_reference.as_str())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "copy_started",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": "copying",
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": null,
        "copy_reference": command.copy_reference.as_str(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "copy_started",
            revision,
            previous_status: Some(current.status),
            resulting_status: TenantCellMoveStatus::Copying,
            occurred_at,
            reason: None,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn checkpoint(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &CheckpointTenantCellMoveCommand,
) -> AppResult<CheckpointTenantCellMoveResult> {
    let prepared =
        PreparedCommand::new_v1(context, CHECKPOINT_TENANT_CELL_MOVE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    if !matches!(
        current.status,
        TenantCellMoveStatus::Copying | TenantCellMoveStatus::Frozen
    ) {
        return Err(AppError::invalid_state_transition(
            "copy checkpoints are accepted only while copying or frozen",
        ));
    }
    let checkpoint = &command.checkpoint;
    if current
        .latest_source_lsn
        .is_some_and(|value| checkpoint.source_lsn() < value)
        || current
            .latest_target_replay_lsn
            .is_some_and(|value| checkpoint.target_replay_lsn() < value)
        || current
            .copied_row_count
            .is_some_and(|value| checkpoint.copied_row_count() < value)
        || current
            .copied_bytes
            .is_some_and(|value| checkpoint.copied_bytes() < value)
    {
        return Err(AppError::conflict(
            "copy checkpoint progress cannot move backwards",
        ));
    }
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let source_lsn = checkpoint.source_lsn().to_string();
    let target_replay_lsn = checkpoint.target_replay_lsn().to_string();
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET revision=$3,last_action='checkpoint_recorded',
        changed_at=$4,changed_by_user_id=$5,latest_source_wal_lsn=$6::TEXT::PG_LSN,
        latest_target_replay_lsn=$7::TEXT::PG_LSN,copied_row_count=$8,
        copied_bytes=$9,checkpointed_at=$4,checkpointed_by_user_id=$5
        WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(&source_lsn)
    .bind(&target_replay_lsn)
    .bind(checkpoint.copied_row_count())
    .bind(checkpoint.copied_bytes())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "checkpoint_recorded",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": current.status.as_str(),
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": null,
        "checkpoint": checkpoint,
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "checkpoint_recorded",
            revision,
            previous_status: Some(current.status),
            resulting_status: current.status,
            occurred_at,
            reason: None,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn freeze(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &FreezeTenantCellMoveCommand,
) -> AppResult<FreezeTenantCellMoveResult> {
    let prepared = PreparedCommand::new_v1(context, FREEZE_TENANT_CELL_MOVE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    current
        .status
        .require_transition(TenantCellMoveStatus::Frozen)
        .map_err(|error| AppError::invalid_state_transition(error.to_string()))?;
    if current.checkpointed_at.is_none() {
        return Err(AppError::conflict(
            "record a copy checkpoint before freezing tenant writes",
        ));
    }
    lock_write_fence_tx(&mut tx, current.tenant_id).await?;
    require_target_viability_tx(&mut tx, &current).await?;
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET status='frozen',revision=$3,
        last_action='writes_frozen',changed_at=$4,changed_by_user_id=$5,
        frozen_at=$4,frozen_by_user_id=$5 WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    sqlx::query(
        r#"INSERT INTO tenant_write_fences
        (tenant_id,tenant_cell_move_id,fence_epoch,frozen_at,frozen_by_user_id)
        VALUES($1,$2,$3,$4,$5)"#,
    )
    .bind(current.tenant_id.get())
    .bind(current.id.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "writes_frozen",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": "frozen",
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": null,
        "fence_epoch": revision.get(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "writes_frozen",
            revision,
            previous_status: Some(current.status),
            resulting_status: TenantCellMoveStatus::Frozen,
            occurred_at,
            reason: None,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn validate(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &ValidateTenantCellMoveCommand,
) -> AppResult<ValidateTenantCellMoveResult> {
    let prepared = PreparedCommand::new_v1(context, VALIDATE_TENANT_CELL_MOVE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    current
        .status
        .require_transition(TenantCellMoveStatus::Validated)
        .map_err(|error| AppError::invalid_state_transition(error.to_string()))?;
    if current
        .latest_checkpoint_revision
        .zip(current.write_fence_epoch)
        .is_none_or(|(checkpoint_revision, freeze_revision)| {
            checkpoint_revision <= freeze_revision || checkpoint_revision != current.revision
        })
    {
        return Err(AppError::conflict(
            "record a final copy checkpoint after freezing tenant writes",
        ));
    }
    require_target_viability_tx(&mut tx, &current).await?;
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let validation = &command.validation;
    let source_lsn = validation.source_lsn().to_string();
    let target_replay_lsn = validation.target_replay_lsn().to_string();
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET status='validated',revision=$3,
        last_action='validated',changed_at=$4,changed_by_user_id=$5,
        validated_at=$4,validated_by_user_id=$5 WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    sqlx::query(
        r#"INSERT INTO tenant_cell_move_validations
        (tenant_id,tenant_cell_move_id,move_revision,source_row_count,target_row_count,
         source_data_checksum,target_data_checksum,source_schema_fingerprint,
         target_schema_fingerprint,source_object_manifest_checksum,
         target_object_manifest_checksum,source_wal_lsn,target_replay_lsn,
         inventory_reconciled,idempotency_verified,outbox_verified,tool_version,
         validated_at,validated_by_user_id)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,
               $12::TEXT::PG_LSN,$13::TEXT::PG_LSN,$14,$15,$16,$17,$18,$19)"#,
    )
    .bind(current.tenant_id.get())
    .bind(current.id.get())
    .bind(revision.get())
    .bind(validation.source_row_count())
    .bind(validation.target_row_count())
    .bind(validation.source_data_checksum().as_str())
    .bind(validation.target_data_checksum().as_str())
    .bind(validation.source_schema_checksum().as_str())
    .bind(validation.target_schema_checksum().as_str())
    .bind(validation.source_object_manifest_checksum().as_str())
    .bind(validation.target_object_manifest_checksum().as_str())
    .bind(&source_lsn)
    .bind(&target_replay_lsn)
    .bind(validation.inventory_reconciled())
    .bind(validation.idempotency_verified())
    .bind(validation.outbox_verified())
    .bind(validation.tool_version().as_str())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "validated",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": "validated",
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": null,
        "validation": validation,
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "validated",
            revision,
            previous_status: Some(current.status),
            resulting_status: TenantCellMoveStatus::Validated,
            occurred_at,
            reason: None,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn cutover(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &CutoverTenantCellMoveCommand,
) -> AppResult<CutoverTenantCellMoveResult> {
    let prepared = PreparedCommand::new_v1(context, CUTOVER_TENANT_CELL_MOVE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    current
        .status
        .require_transition(TenantCellMoveStatus::CutOver)
        .map_err(|error| AppError::invalid_state_transition(error.to_string()))?;
    if command.expected_placement_revision != current.source_placement_revision {
        return Err(placement_revision_conflict());
    }
    lock_write_fence_tx(&mut tx, current.tenant_id).await?;
    require_target_viability_tx(&mut tx, &current).await?;
    let placement =
        sqlx::query("SELECT data_cell_id,revision FROM tenant_cell_placements WHERE tenant_id=$1")
            .bind(current.tenant_id.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::not_found("tenant placement"))?;
    let placement_data_cell_id: i64 = placement.try_get("data_cell_id")?;
    let placement_revision: i64 = placement.try_get("revision")?;
    if placement_data_cell_id != current.source_data_cell_id.get()
        || placement_revision != command.expected_placement_revision.get()
    {
        return Err(placement_revision_conflict());
    }
    let fence_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM tenant_write_fences WHERE tenant_id=$1 AND tenant_cell_move_id=$2)",
    )
    .bind(current.tenant_id.get())
    .bind(current.id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !fence_exists {
        return Err(AppError::conflict("tenant write fence is missing"));
    }
    let cutover_placement_revision = current
        .source_placement_revision
        .checked_next()
        .ok_or_else(|| invalid("tenant placement revision overflow"))?;
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET status='cut_over',revision=$3,
        last_action='cut_over',changed_at=$4,changed_by_user_id=$5,
        cutover_at=$4,cutover_by_user_id=$5,cutover_placement_revision=$6
        WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(cutover_placement_revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    let applied_revision: i64 =
        sqlx::query_scalar("SELECT apply_tenant_cell_move_placement($1,$2,$3)")
            .bind(current.id.get())
            .bind(context.actor_id.get())
            .bind(&context.request_id)
            .fetch_one(&mut *tx)
            .await?;
    if applied_revision != cutover_placement_revision.get() {
        return Err(invalid(
            "tenant placement cutover returned an invalid revision",
        ));
    }
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "cut_over",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": "cut_over",
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": cutover_placement_revision.get(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "cut_over",
            revision,
            previous_status: Some(current.status),
            resulting_status: TenantCellMoveStatus::CutOver,
            occurred_at,
            reason: None,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn verify_cutover(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &VerifyTenantCellMoveCutoverCommand,
) -> AppResult<VerifyTenantCellMoveCutoverResult> {
    let prepared =
        PreparedCommand::new_v1(context, VERIFY_TENANT_CELL_MOVE_CUTOVER_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    if current.status != TenantCellMoveStatus::CutOver {
        return Err(AppError::invalid_state_transition(
            "cutover verification requires a cut-over move",
        ));
    }
    if current.post_cutover_verified_at.is_some() {
        return Err(AppError::conflict("tenant cutover is already verified"));
    }
    let cutover_placement_revision = current
        .cutover_placement_revision
        .ok_or_else(|| invalid("cut-over move has no placement revision"))?;
    let verification = &command.verification;
    if verification.observed_data_cell_id() != current.target_data_cell_id
        || verification.observed_placement_revision() != cutover_placement_revision
    {
        return Err(AppError::conflict(
            "cutover verification does not match the active tenant placement",
        ));
    }
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET revision=$3,last_action='post_cutover_verified',
        changed_at=$4,changed_by_user_id=$5,post_cutover_verified_at=$4,
        post_cutover_verified_by_user_id=$5 WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    sqlx::query(
        r#"INSERT INTO tenant_cell_move_cutover_verifications
        (tenant_id,tenant_cell_move_id,move_revision,tool_version,routing_reference,
         observed_data_cell_id,observed_placement_revision,routing_verified,
         target_read_verified,write_fence_verified,inventory_reconciled,
         idempotency_verified,outbox_verified,verified_at,verified_by_user_id)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)"#,
    )
    .bind(current.tenant_id.get())
    .bind(current.id.get())
    .bind(revision.get())
    .bind(verification.tool_version().as_str())
    .bind(verification.routing_reference().as_str())
    .bind(verification.observed_data_cell_id().get())
    .bind(verification.observed_placement_revision().get())
    .bind(verification.routing_verified())
    .bind(verification.target_read_verified())
    .bind(verification.write_fence_verified())
    .bind(verification.inventory_reconciled())
    .bind(verification.idempotency_verified())
    .bind(verification.outbox_verified())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "post_cutover_verified",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": current.status.as_str(),
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": cutover_placement_revision.get(),
        "verification": verification,
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "post_cutover_verified",
            revision,
            previous_status: Some(current.status),
            resulting_status: current.status,
            occurred_at,
            reason: None,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn complete(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &CompleteTenantCellMoveCommand,
) -> AppResult<CompleteTenantCellMoveResult> {
    let prepared = PreparedCommand::new_v1(context, COMPLETE_TENANT_CELL_MOVE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    current
        .status
        .require_transition(TenantCellMoveStatus::Completed)
        .map_err(|error| AppError::invalid_state_transition(error.to_string()))?;
    if current.post_cutover_verified_at.is_none() {
        return Err(AppError::conflict(
            "verify target routing and controls before completing the move",
        ));
    }
    lock_write_fence_tx(&mut tx, current.tenant_id).await?;
    let cutover_placement_revision = current
        .cutover_placement_revision
        .ok_or_else(|| invalid("cut-over move has no placement revision"))?;
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET status='completed',revision=$3,
        last_action='completed',changed_at=$4,changed_by_user_id=$5,change_reason=$6,
        completed_at=$4,completed_by_user_id=$5 WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    sqlx::query("DELETE FROM tenant_write_fences WHERE tenant_id=$1 AND tenant_cell_move_id=$2")
        .bind(current.tenant_id.get())
        .bind(current.id.get())
        .execute(&mut *tx)
        .await?;
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "completed",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": "completed",
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": cutover_placement_revision.get(),
        "reason": command.reason.as_str(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "completed",
            revision,
            previous_status: Some(current.status),
            resulting_status: TenantCellMoveStatus::Completed,
            occurred_at,
            reason: Some(command.reason.as_str()),
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn rollback(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &RollbackTenantCellMoveCommand,
) -> AppResult<RollbackTenantCellMoveResult> {
    let prepared = PreparedCommand::new_v1(context, ROLLBACK_TENANT_CELL_MOVE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    current
        .status
        .require_transition(TenantCellMoveStatus::RolledBack)
        .map_err(|error| AppError::invalid_state_transition(error.to_string()))?;
    lock_write_fence_tx(&mut tx, current.tenant_id).await?;
    let cutover_placement_revision = current
        .cutover_placement_revision
        .ok_or_else(|| invalid("cut-over move has no placement revision"))?;
    let rollback_placement_revision = cutover_placement_revision
        .checked_next()
        .ok_or_else(|| invalid("tenant placement revision overflow"))?;
    let verification = &command.verification;
    if verification.observed_data_cell_id() != current.source_data_cell_id {
        return Err(AppError::conflict(
            "rollback verification does not identify the source data cell",
        ));
    }
    if verification.expected_rollback_placement_revision() != rollback_placement_revision {
        return Err(AppError::conflict(
            "rollback verification does not match the next tenant placement revision",
        ));
    }
    let rollback_state = sqlx::query(
        r#"SELECT placement.data_cell_id,placement.revision,
        EXISTS(SELECT 1 FROM tenant_write_fences fence
          WHERE fence.tenant_id=placement.tenant_id
            AND fence.tenant_cell_move_id=$2) AS fence_exists
        FROM tenant_cell_placements placement WHERE placement.tenant_id=$1"#,
    )
    .bind(current.tenant_id.get())
    .bind(current.id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("tenant placement"))?;
    let active_data_cell_id: i64 = rollback_state.try_get("data_cell_id")?;
    let active_placement_revision: i64 = rollback_state.try_get("revision")?;
    let fence_exists: bool = rollback_state.try_get("fence_exists")?;
    if active_data_cell_id != current.target_data_cell_id.get()
        || active_placement_revision != cutover_placement_revision.get()
    {
        return Err(placement_revision_conflict());
    }
    if !fence_exists {
        return Err(AppError::conflict("tenant write fence is missing"));
    }
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET status='rolled_back',revision=$3,
        last_action='rolled_back',changed_at=$4,changed_by_user_id=$5,change_reason=$6,
        rolled_back_at=$4,rolled_back_by_user_id=$5,rollback_placement_revision=$7
        WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .bind(rollback_placement_revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    sqlx::query(
        r#"INSERT INTO tenant_cell_move_rollback_verifications
        (tenant_id,tenant_cell_move_id,move_revision,tool_version,routing_reference,
         observed_data_cell_id,expected_rollback_placement_revision,routing_verified,
         source_read_verified,write_fence_verified,inventory_reconciled,
         idempotency_verified,outbox_verified,verified_at,verified_by_user_id)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)"#,
    )
    .bind(current.tenant_id.get())
    .bind(current.id.get())
    .bind(revision.get())
    .bind(verification.tool_version().as_str())
    .bind(verification.routing_reference().as_str())
    .bind(verification.observed_data_cell_id().get())
    .bind(verification.expected_rollback_placement_revision().get())
    .bind(verification.routing_verified())
    .bind(verification.source_read_verified())
    .bind(verification.write_fence_verified())
    .bind(verification.inventory_reconciled())
    .bind(verification.idempotency_verified())
    .bind(verification.outbox_verified())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    let applied_revision: i64 =
        sqlx::query_scalar("SELECT apply_tenant_cell_move_placement($1,$2,$3)")
            .bind(current.id.get())
            .bind(context.actor_id.get())
            .bind(&context.request_id)
            .fetch_one(&mut *tx)
            .await?;
    if applied_revision != rollback_placement_revision.get() {
        return Err(invalid(
            "tenant placement rollback returned an invalid revision",
        ));
    }
    sqlx::query("DELETE FROM tenant_write_fences WHERE tenant_id=$1 AND tenant_cell_move_id=$2")
        .bind(current.tenant_id.get())
        .bind(current.id.get())
        .execute(&mut *tx)
        .await?;
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "rolled_back",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": "rolled_back",
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": rollback_placement_revision.get(),
        "rollback_verification": verification,
        "reason": command.reason.as_str(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "rolled_back",
            revision,
            previous_status: Some(current.status),
            resulting_status: TenantCellMoveStatus::RolledBack,
            occurred_at,
            reason: Some(command.reason.as_str()),
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn cancel(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &CancelTenantCellMoveCommand,
) -> AppResult<CancelTenantCellMoveResult> {
    let prepared = PreparedCommand::new_v1(context, CANCEL_TENANT_CELL_MOVE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    authorize_tx(&mut tx, actor_access, context).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_move_tx(&mut tx, command.tenant_cell_move_id).await?;
    require_switched_tenant(actor_access, current.tenant_id)?;
    require_revision(&current, command.expected_revision)?;
    current
        .status
        .require_transition(TenantCellMoveStatus::Cancelled)
        .map_err(|error| AppError::invalid_state_transition(error.to_string()))?;
    if current.status.is_write_fenced() {
        lock_write_fence_tx(&mut tx, current.tenant_id).await?;
    }
    let revision = next_revision(current.revision)?;
    let occurred_at = database_now_tx(&mut tx).await?;
    let updated = sqlx::query(
        r#"UPDATE tenant_cell_moves SET status='cancelled',revision=$3,
        last_action='cancelled',changed_at=$4,changed_by_user_id=$5,change_reason=$6,
        cancelled_at=$4,cancelled_by_user_id=$5 WHERE id=$1 AND revision=$2"#,
    )
    .bind(current.id.get())
    .bind(current.revision.get())
    .bind(revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(revision_conflict());
    }
    sqlx::query("DELETE FROM tenant_write_fences WHERE tenant_id=$1 AND tenant_cell_move_id=$2")
        .bind(current.tenant_id.get())
        .bind(current.id.get())
        .execute(&mut *tx)
        .await?;
    let evidence = serde_json::json!({
        "tenant_cell_move_id": current.id.get(),
        "tenant_id": current.tenant_id.get(),
        "action": "cancelled",
        "move_revision": revision.get(),
        "previous_status": current.status.as_str(),
        "resulting_status": "cancelled",
        "source_data_cell_id": current.source_data_cell_id.get(),
        "target_data_cell_id": current.target_data_cell_id.get(),
        "source_placement_revision": current.source_placement_revision.get(),
        "resulting_placement_revision": null,
        "reason": command.reason.as_str(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    record_event_tx(
        &mut tx,
        &current,
        context,
        EventTransition {
            action: "cancelled",
            revision,
            previous_status: Some(current.status),
            resulting_status: TenantCellMoveStatus::Cancelled,
            occurred_at,
            reason: Some(command.reason.as_str()),
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, actor_access.tenant_id, current.id).await?;
    restore_actor_tenant_tx(&mut tx, actor_access).await?;
    Ok(prepared.commit(tx, result).await?)
}
