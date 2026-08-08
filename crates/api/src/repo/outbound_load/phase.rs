use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbound_load::{
    CancelOutboundLoadCommand, CancelOutboundLoadResult, CompleteOutboundLoadLoadingCommand,
    CompleteOutboundLoadLoadingResult, ReleaseOutboundLoadCommand, ReleaseOutboundLoadResult,
    StartOutboundLoadLoadingCommand, StartOutboundLoadLoadingResult,
    CANCEL_OUTBOUND_LOAD_OPERATION, COMPLETE_OUTBOUND_LOAD_LOADING_OPERATION,
    RELEASE_OUTBOUND_LOAD_OPERATION, START_OUTBOUND_LOAD_LOADING_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    cancel_outbound_load, complete_outbound_load_loading, release_outbound_load,
    start_outbound_load_loading, LocationId, OutboundLoadCancellationId,
    OutboundLoadCancellationReason, OutboundLoadRevision,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

use super::{
    enqueue_load_event_tx, load_progress_tx, lock_load_tx, positive, progress_read, LockedLoad,
};

pub async fn release(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReleaseOutboundLoadCommand,
) -> AppResult<ReleaseOutboundLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RELEASE_OUTBOUND_LOAD_OPERATION, command)?;
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
    if let Some(result) = prepared
        .replayed::<ReleaseOutboundLoadResult>(&mut tx)
        .await?
    {
        super::require_load_visible_tx(&mut tx, access.tenant_id, result.outbound_load_id, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }
    let load = lock_load_tx(&mut tx, access.tenant_id, command.outbound_load_id, &scope).await?;
    require_revision(&load, command.expected_revision)?;
    let transition =
        release_outbound_load(load_progress_tx(&mut tx, access.tenant_id, &load).await?)
            .map_err(workflow_conflict)?;
    let revision = next_revision(load.revision)?;
    let released_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE outbound_loads
        SET state='staging',revision=$3,released_by_user_id=$4,released_at=$5
        WHERE tenant_id=$1 AND id=$2 AND state='planned' AND revision=$6
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(revision.get())
    .bind(context.actor_id.get())
    .bind(released_at)
    .bind(load.revision.get())
    .execute(&mut *tx)
    .await?;
    require_one_update(updated.rows_affected())?;
    let result = ReleaseOutboundLoadResult {
        outbound_load_id: load.id,
        status: transition.progress.status(),
        revision,
        progress: progress_read(transition.progress),
        released_by: context.actor_id,
        released_at,
    };
    enqueue_result_event(
        &mut tx,
        access,
        context,
        &load,
        "outbound.load.released",
        "released",
        &result,
        released_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn start_loading(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &StartOutboundLoadLoadingCommand,
) -> AppResult<StartOutboundLoadLoadingResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, START_OUTBOUND_LOAD_LOADING_OPERATION, command)?;
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
    if let Some(result) = prepared
        .replayed::<StartOutboundLoadLoadingResult>(&mut tx)
        .await?
    {
        super::require_load_visible_tx(&mut tx, access.tenant_id, result.outbound_load_id, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }
    let load = lock_load_tx(&mut tx, access.tenant_id, command.outbound_load_id, &scope).await?;
    require_revision(&load, command.expected_revision)?;
    require_load_and_staging_scans_tx(
        &mut tx,
        access,
        &load,
        command.load_barcode.as_str(),
        command.staging_location_barcode.as_str(),
    )
    .await?;
    let dock_location_id = lock_execution_location_by_barcode_tx(
        &mut tx,
        access,
        load.facility_id,
        command.dock_location_barcode.as_str(),
        "dock",
    )
    .await?;
    if dock_location_id == load.staging_location_id
        || dock_location_id == load.virtual_trailer_location_id
    {
        return Err(AppError::conflict("dock door is not available"));
    }
    let transition =
        start_outbound_load_loading(load_progress_tx(&mut tx, access.tenant_id, &load).await?)
            .map_err(workflow_conflict)?;
    let revision = next_revision(load.revision)?;
    let started_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE outbound_loads
        SET state='loading', revision=$3, dock_door_location_id=$4,
            trailer_number=$5, loading_started_by_user_id=$6, loading_started_at=$7
        WHERE tenant_id=$1 AND id=$2 AND state='staging' AND revision=$8
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(revision.get())
    .bind(dock_location_id)
    .bind(command.trailer_number.as_str())
    .bind(context.actor_id.get())
    .bind(started_at)
    .bind(load.revision.get())
    .execute(&mut *tx)
    .await?;
    require_one_update(updated.rows_affected())?;
    let result = StartOutboundLoadLoadingResult {
        outbound_load_id: load.id,
        status: transition.progress.status(),
        revision,
        dock_location_id: positive(dock_location_id, LocationId::new)?,
        trailer_number: command.trailer_number.clone(),
        started_by: context.actor_id,
        started_at,
    };
    enqueue_result_event(
        &mut tx,
        access,
        context,
        &load,
        "outbound.load.loading_started",
        "loading-started",
        &result,
        started_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn complete_loading(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CompleteOutboundLoadLoadingCommand,
) -> AppResult<CompleteOutboundLoadLoadingResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, COMPLETE_OUTBOUND_LOAD_LOADING_OPERATION, command)?;
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
    if let Some(result) = prepared
        .replayed::<CompleteOutboundLoadLoadingResult>(&mut tx)
        .await?
    {
        super::require_load_visible_tx(&mut tx, access.tenant_id, result.outbound_load_id, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }
    let load = lock_load_tx(&mut tx, access.tenant_id, command.outbound_load_id, &scope).await?;
    require_revision(&load, command.expected_revision)?;
    validate_loading_scans_tx(
        &mut tx,
        access,
        &load,
        command.load_barcode.as_str(),
        command.dock_location_barcode.as_str(),
        command.trailer_number.as_str(),
    )
    .await?;
    let transition =
        complete_outbound_load_loading(load_progress_tx(&mut tx, access.tenant_id, &load).await?)
            .map_err(workflow_conflict)?;
    let revision = next_revision(load.revision)?;
    let completed_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE outbound_loads
        SET state='ready_to_depart', revision=$3, seal_number=$4,
            ready_to_depart_by_user_id=$5, ready_to_depart_at=$6
        WHERE tenant_id=$1 AND id=$2 AND state='loading' AND revision=$7
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(revision.get())
    .bind(command.seal_number.as_str())
    .bind(context.actor_id.get())
    .bind(completed_at)
    .bind(load.revision.get())
    .execute(&mut *tx)
    .await?;
    require_one_update(updated.rows_affected())?;
    let result = CompleteOutboundLoadLoadingResult {
        outbound_load_id: load.id,
        status: transition.progress.status(),
        revision,
        seal_number: command.seal_number.clone(),
        completed_by: context.actor_id,
        completed_at,
    };
    enqueue_result_event(
        &mut tx,
        access,
        context,
        &load,
        "outbound.load.loading_completed",
        "loading-completed",
        &result,
        completed_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn cancel(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelOutboundLoadCommand,
) -> AppResult<CancelOutboundLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_OUTBOUND_LOAD_OPERATION, command)?;
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
    if let Some(result) = prepared
        .replayed::<CancelOutboundLoadResult>(&mut tx)
        .await?
    {
        super::require_load_visible_tx(&mut tx, access.tenant_id, result.outbound_load_id, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }
    let load = lock_load_tx(&mut tx, access.tenant_id, command.outbound_load_id, &scope).await?;
    require_revision(&load, command.expected_revision)?;
    lock_load_cartons_tx(&mut tx, access, &load).await?;
    let all_restored: bool = sqlx::query_scalar(
        r#"
        SELECT NOT EXISTS (
            SELECT 1 FROM outbound_load_cartons carton
            WHERE carton.tenant_id=$1 AND carton.outbound_load_id=$2
              AND (carton.state <> 'planned' OR EXISTS (
                  SELECT 1 FROM packed_inventory_positions position
                  WHERE position.tenant_id=carton.tenant_id
                    AND position.inventory_owner_id=carton.inventory_owner_id
                    AND position.carton_id=carton.carton_id
                    AND (position.state <> 'packed'
                         OR position.current_location_id <> carton.original_location_id)
              ))
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .fetch_one(&mut *tx)
    .await?;
    let transition = cancel_outbound_load(
        load_progress_tx(&mut tx, access.tenant_id, &load).await?,
        all_restored,
    )
    .map_err(workflow_conflict)?;
    let revision = next_revision(load.revision)?;
    let cancelled_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE outbound_loads
        SET state='cancelled', revision=$3,
            cancelled_by_user_id=$4, cancelled_at=$5
        WHERE tenant_id=$1 AND id=$2 AND revision=$6
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(revision.get())
    .bind(context.actor_id.get())
    .bind(cancelled_at)
    .bind(load.revision.get())
    .execute(&mut *tx)
    .await?;
    require_one_update(updated.rows_affected())?;
    sqlx::query(
        "UPDATE outbound_load_shipments SET closed_at=$3 WHERE tenant_id=$1 AND outbound_load_id=$2 AND closed_at IS NULL",
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(cancelled_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE outbound_load_cartons
        SET revision=revision+1, closed_at=$3
        WHERE tenant_id=$1 AND outbound_load_id=$2 AND state='planned' AND closed_at IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(cancelled_at)
    .execute(&mut *tx)
    .await?;
    let cancellation_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO outbound_load_cancellations (
            tenant_id,facility_id,outbound_load_id,expected_revision,resulting_revision,
            reason_code,note,cancelled_by_user_id,cancelled_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.facility_id.get())
    .bind(load.id.get())
    .bind(load.revision.get())
    .bind(revision.get())
    .bind(cancellation_reason(command.details.reason))
    .bind(command.details.note.as_ref().map(|note| note.as_str()))
    .bind(context.actor_id.get())
    .bind(cancelled_at)
    .fetch_one(&mut *tx)
    .await?;
    let result = CancelOutboundLoadResult {
        cancellation_id: positive(cancellation_id, OutboundLoadCancellationId::new)?,
        outbound_load_id: load.id,
        status: transition.progress.status(),
        revision,
        cancelled_by: context.actor_id,
        cancelled_at,
    };
    enqueue_result_event(
        &mut tx,
        access,
        context,
        &load,
        "outbound.load.cancelled",
        "cancelled",
        &result,
        cancelled_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

fn require_revision(load: &LockedLoad, expected: OutboundLoadRevision) -> AppResult<()> {
    if load.revision == expected {
        Ok(())
    } else {
        Err(AppError::conflict("outbound load revision is stale"))
    }
}

fn next_revision(revision: OutboundLoadRevision) -> AppResult<OutboundLoadRevision> {
    revision
        .checked_next()
        .ok_or_else(|| AppError::internal("outbound load revision overflow"))
}

fn workflow_conflict(error: impl std::fmt::Display) -> AppError {
    AppError::conflict(error.to_string())
}

fn require_one_update(rows: u64) -> AppResult<()> {
    if rows == 1 {
        Ok(())
    } else {
        Err(AppError::conflict("outbound load changed concurrently"))
    }
}

async fn lock_execution_location_by_barcode_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    facility_id: wareboxes_domain::FacilityId,
    barcode: &str,
    kind: &str,
) -> AppResult<i64> {
    sqlx::query_scalar(
        r#"
        SELECT id FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND barcode=$3
          AND active AND deleted IS NULL AND NOT pickable AND NOT receivable
          AND lower(type)=$4
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id.get())
    .bind(barcode)
    .bind(kind)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::bad_request(format!("{kind} scan does not match an active location")))
}

async fn require_load_and_staging_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    load: &LockedLoad,
    load_scan: &str,
    staging_scan: &str,
) -> AppResult<()> {
    if load.load_barcode != load_scan {
        return Err(AppError::bad_request(
            "load scan does not match outbound load",
        ));
    }
    let staging_id = lock_execution_location_by_barcode_tx(
        tx,
        access,
        load.facility_id,
        staging_scan,
        "staging",
    )
    .await?;
    if staging_id != load.staging_location_id {
        return Err(AppError::bad_request(
            "staging scan does not match outbound load",
        ));
    }
    Ok(())
}

async fn validate_loading_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    load: &LockedLoad,
    load_scan: &str,
    dock_scan: &str,
    trailer_scan: &str,
) -> AppResult<()> {
    if load.load_barcode != load_scan || load.trailer_number.as_deref() != Some(trailer_scan) {
        return Err(AppError::bad_request(
            "load or trailer scan does not match outbound load",
        ));
    }
    let dock_id =
        lock_execution_location_by_barcode_tx(tx, access, load.facility_id, dock_scan, "dock")
            .await?;
    if load.dock_location_id != Some(dock_id) {
        return Err(AppError::bad_request(
            "dock scan does not match outbound load",
        ));
    }
    Ok(())
}

async fn lock_load_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    load: &LockedLoad,
) -> AppResult<()> {
    sqlx::query(
        "SELECT id FROM outbound_load_cartons WHERE tenant_id=$1 AND outbound_load_id=$2 ORDER BY id FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        "SELECT id FROM packed_inventory_positions WHERE tenant_id=$1 AND outbound_load_id=$2 ORDER BY id FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .fetch_all(&mut **tx)
    .await?;
    Ok(())
}

fn cancellation_reason(reason: OutboundLoadCancellationReason) -> &'static str {
    match reason {
        OutboundLoadCancellationReason::RouteCancelled => "route_cancelled",
        OutboundLoadCancellationReason::CarrierCancelled => "carrier_cancelled",
        OutboundLoadCancellationReason::EquipmentUnavailable => "equipment_unavailable",
        OutboundLoadCancellationReason::PlanningError => "planning_error",
        OutboundLoadCancellationReason::Other => "other",
    }
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_result_event<T: serde::Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    load: &LockedLoad,
    event_type: &str,
    event_key: &str,
    result: &T,
    occurred_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    enqueue_load_event_tx(
        tx,
        super::LoadEvent {
            tenant_id: access.tenant_id,
            facility_id: load.facility_id,
            actor_user_id: context.actor_id.get(),
            load_id: load.id,
            event_type,
            event_key,
            payload: serde_json::to_value(result)
                .map_err(|error| AppError::internal(error.to_string()))?,
            occurred_at,
        },
    )
    .await
}
