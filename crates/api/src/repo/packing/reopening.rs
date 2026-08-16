use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::packing::{
    PackCartonLifecycle, ReopenCartonCommand, ReopenCartonResult, REOPEN_CARTON_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    reopen_carton, CartonDimensions, CartonMeasurements, CartonReopenReason, CartonReopeningId,
    CartonStatus, DimensionMillimeters, OrderStatus, PackSessionId, PackSessionStatus,
    PackingProgress, TenantId, Timestamp, UserId, WeightGrams,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory_locking;
use crate::repo::orders::insert_order_activity_tx;

use super::{
    enqueue_order_event_tx, lock_order_tx, lock_session_tx, require_replayed_ids_visible_tx,
    require_revision, session_order_hint_tx,
};

pub async fn reopen_carton_command(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReopenCartonCommand,
) -> AppResult<ReopenCartonResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, REOPEN_CARTON_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    require_stored_reopening_visible_before_replay_tx(
        &mut tx,
        access.tenant_id,
        prepared.idempotency_key(),
        &scope,
    )
    .await?;
    if let Some(result) = prepared.replayed::<ReopenCartonResult>(&mut tx).await? {
        require_replayed_ids_visible_tx(
            &mut tx,
            access.tenant_id,
            result.session_id,
            result.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id = session_order_hint_tx(&mut tx, access.tenant_id, command.session_id).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    let session = lock_session_tx(&mut tx, access.tenant_id, command.session_id, &scope).await?;
    if session.order_id != order_id {
        return Err(AppError::conflict(
            "packing session does not match the order",
        ));
    }
    let revision = require_revision(&order, Some(&session), command.expected_revision)?;
    let carton = lock_carton_tx(&mut tx, access.tenant_id, command).await?;
    inventory_locking::lock_license_plate(&mut tx, access.tenant_id, Some(carton.license_plate_id))
        .await?;
    if carton.barcode != command.carton_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned carton does not match the closed carton",
        ));
    }
    let content_count = active_content_count_tx(
        &mut tx,
        access.tenant_id,
        command.session_id,
        command.carton_id,
    )
    .await?;
    let progress = PackingProgress::new(
        session.expected_allocation_count,
        session.packed_allocation_count,
        session.expected_qty,
        session.packed_qty,
        session.open_carton_count,
        session.closed_carton_count,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let session_status = session_status(&session.state)?;
    let (order_status, progress) = reopen_carton(
        order.status,
        session_status,
        progress,
        carton_status(&carton.state)?,
        content_count,
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    require_no_downstream_execution_tx(&mut tx, access.tenant_id, command.session_id).await?;

    let previous_measurements = measurements(&carton)?;
    let previous_weight_evidence = carton.weight_evidence.clone();
    let previous_closed_by = carton
        .closed_by
        .ok_or_else(|| AppError::internal("closed carton lacks closing actor"))?;
    let previous_closed_at = carton
        .closed_at
        .ok_or_else(|| AppError::internal("closed carton lacks closing timestamp"))?;
    let reopened_at = now_iso();
    let reason = reason_code(command.details.reason());
    let note = command.details.note().map(|value| value.as_str());
    let reopening_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO carton_reopenings (
            tenant_id,inventory_owner_id,facility_id,packing_session_id,
            order_release_id,order_id,carton_id,
            previous_order_status,resulting_order_status,
            previous_session_state,resulting_session_state,
            expected_revision,resulting_revision,
            previous_reopen_count,resulting_reopen_count,
            previous_closed_by_user_id,previous_closed_at,
            previous_weight_g,previous_length_mm,previous_width_mm,previous_height_mm,
            reason_code,note,reopened_by_user_id,reopened_at
        ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'open',$11,$12,$13,$14,$15,$16,
            $17,$18,$19,$20,$21,$22,$23,$24
        ) RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(session.facility_id)
    .bind(session.id.get())
    .bind(session.order_release_id)
    .bind(order_id.get())
    .bind(command.carton_id.get())
    .bind(order.status.as_str())
    .bind(order_status.as_str())
    .bind(&session.state)
    .bind(command.expected_revision.get())
    .bind(revision.get())
    .bind(carton.reopen_count)
    .bind(carton.reopen_count + 1)
    .bind(previous_closed_by.get())
    .bind(previous_closed_at)
    .bind(carton.weight_g)
    .bind(carton.length_mm)
    .bind(carton.width_mm)
    .bind(carton.height_mm)
    .bind(reason)
    .bind(note)
    .bind(context.actor_id.get())
    .bind(reopened_at)
    .fetch_one(&mut *tx)
    .await?;

    update_session_tx(&mut tx, access.tenant_id, &session, revision, progress).await?;
    update_order_tx(
        &mut tx,
        access.tenant_id,
        order_id,
        order.status,
        order.revision,
        order_status,
        revision,
    )
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE cartons
        SET state='open', reopen_count=reopen_count+1,
            closed_by_user_id=NULL, closed_at=NULL,
            weight_g=NULL, length_mm=NULL, width_mm=NULL, height_mm=NULL
        WHERE tenant_id=$1 AND packing_session_id=$2 AND id=$3
          AND state='closed' AND reopen_count=$4
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(session.id.get())
    .bind(command.carton_id.get())
    .bind(carton.reopen_count)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("carton changed while reopening"));
    }

    let reopening_id = CartonReopeningId::new(reopening_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        order_id.get(),
        Some(context.actor_id.get()),
        &format!("reopened carton {} ({reason})", command.carton_barcode),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        session.facility_id,
        context.actor_id.get(),
        order_id,
        "packing.carton_reopened",
        &format!(
            "carton:{}:reopened:{}",
            command.carton_id,
            carton.reopen_count + 1
        ),
        serde_json::json!({
            "reopening_id": reopening_id,
            "packing_session_id": session.id,
            "carton_id": command.carton_id,
            "order_id": order_id,
            "previous_order_status": order.status,
            "order_status": order_status,
            "expected_revision": command.expected_revision,
            "revision": revision,
            "reason": reason,
            "note": note,
            "reopened_by": context.actor_id,
            "reopened_at": reopened_at,
            "previous_weight_evidence": previous_weight_evidence,
        }),
        reopened_at,
    )
    .await?;

    let result = ReopenCartonResult {
        reopening_id,
        session_id: session.id,
        carton_id: command.carton_id,
        order_id,
        previous_order_status: order.status,
        order_status,
        lifecycle: PackCartonLifecycle::Open,
        previous_measurements,
        previous_weight_evidence,
        previous_closed_by,
        previous_closed_at,
        revision,
        progress,
        details: command.details.clone(),
        reopened_by: context.actor_id,
        reopened_at,
    };
    Ok(prepared.commit(tx, result).await?)
}

struct LockedCarton {
    license_plate_id: i64,
    barcode: String,
    state: String,
    reopen_count: i64,
    closed_by: Option<UserId>,
    closed_at: Option<Timestamp>,
    weight_g: Option<i64>,
    length_mm: Option<i64>,
    width_mm: Option<i64>,
    height_mm: Option<i64>,
    weight_evidence: Option<wareboxes_application::packing::CartonWeightEvidence>,
}

async fn lock_carton_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &ReopenCartonCommand,
) -> AppResult<LockedCarton> {
    let row = sqlx::query(&format!(
        r#"
        SELECT carton.license_plate_id,plate.barcode,carton.state,carton.reopen_count,
               carton.closed_by_user_id,carton.closed_at,carton.weight_g,
               carton.length_mm,carton.width_mm,carton.height_mm,{}
        FROM cartons carton
        INNER JOIN license_plates plate
          ON plate.tenant_id=carton.tenant_id
         AND plate.inventory_owner_id=carton.inventory_owner_id
         AND plate.facility_id=carton.facility_id
         AND plate.id=carton.license_plate_id
        LEFT JOIN carton_weight_evidence evidence
          ON evidence.tenant_id=carton.tenant_id AND evidence.carton_id=carton.id
         AND evidence.carton_reopen_count=carton.reopen_count
        LEFT JOIN automation_commands weight_command
          ON weight_command.tenant_id=evidence.tenant_id
         AND weight_command.id=evidence.automation_command_id
        LEFT JOIN automation_devices weight_device
          ON weight_device.tenant_id=weight_command.tenant_id
         AND weight_device.id=weight_command.device_id
        WHERE carton.tenant_id=$1 AND carton.packing_session_id=$2 AND carton.id=$3
        FOR UPDATE OF carton
        "#,
        super::weight_evidence::SELECT_COLUMNS
    ))
    .bind(tenant_id.get())
    .bind(command.session_id.get())
    .bind(command.carton_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("carton"))?;
    let closed_by = row
        .try_get::<Option<i64>, _>("closed_by_user_id")?
        .map(UserId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(LockedCarton {
        license_plate_id: row.try_get("license_plate_id")?,
        barcode: row.try_get("barcode")?,
        state: row.try_get("state")?,
        reopen_count: row.try_get("reopen_count")?,
        closed_by,
        closed_at: row.try_get("closed_at")?,
        weight_g: row.try_get("weight_g")?,
        length_mm: row.try_get("length_mm")?,
        width_mm: row.try_get("width_mm")?,
        height_mm: row.try_get("height_mm")?,
        weight_evidence: super::weight_evidence::from_row(&row)?,
    })
}

async fn active_content_count_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
    carton_id: wareboxes_domain::CartonId,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM packing_allocation_positions position
        INNER JOIN carton_contents content
          ON content.tenant_id=position.tenant_id
         AND content.inventory_owner_id=position.inventory_owner_id
         AND content.facility_id=position.facility_id
         AND content.packing_session_id=position.packing_session_id
         AND content.packing_session_allocation_id=position.packing_session_allocation_id
         AND content.id=position.current_carton_content_id
        WHERE position.tenant_id=$1 AND position.packing_session_id=$2
          AND content.carton_id=$3 AND position.state='packed'
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .bind(carton_id.get())
    .fetch_one(&mut **tx)
    .await?)
}

async fn require_no_downstream_execution_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (SELECT 1 FROM outbound_qa_sessions
                       WHERE tenant_id=$1 AND packing_session_id=$2
                         AND state <> 'cancelled')
            OR EXISTS (SELECT 1 FROM shipments
                       WHERE tenant_id=$1 AND packing_session_id=$2
                         AND state <> 'cancelled')
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(AppError::conflict(
            "carton cannot be reopened after downstream execution begins",
        ))
    } else {
        Ok(())
    }
}

async fn update_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    revision: wareboxes_domain::OrderRevision,
    progress: PackingProgress,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE packing_sessions
        SET state='open',revision=$1,open_carton_count=$2,closed_carton_count=$3,
            ready_by_user_id=NULL,ready_at=NULL
        WHERE tenant_id=$4 AND id=$5 AND state=$6 AND revision=$7
        "#,
    )
    .bind(revision.get())
    .bind(progress.open_carton_count())
    .bind(progress.closed_carton_count())
    .bind(tenant_id.get())
    .bind(session.id.get())
    .bind(&session.state)
    .bind(session.revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "packing session changed while reopening carton",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn update_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: wareboxes_domain::OrderId,
    previous_status: OrderStatus,
    previous_revision: wareboxes_domain::OrderRevision,
    status: OrderStatus,
    revision: wareboxes_domain::OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders SET status=$1,revision=$2
        WHERE tenant_id=$3 AND id=$4 AND status=$5 AND revision=$6
        "#,
    )
    .bind(status.as_str())
    .bind(revision.get())
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(previous_status.as_str())
    .bind(previous_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed while reopening carton"));
    }
    Ok(())
}

async fn require_stored_reopening_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    idempotency_key: &str,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored: Option<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT (result_json->>'session_id')::BIGINT,(result_json->>'order_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(tenant_id.get())
    .bind(REOPEN_CARTON_OPERATION)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((session_id, order_id)) = stored {
        require_replayed_ids_visible_tx(
            tx,
            tenant_id,
            PackSessionId::new(session_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            wareboxes_domain::OrderId::new(order_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            scope,
        )
        .await?;
    }
    Ok(())
}

fn session_status(value: &str) -> AppResult<PackSessionStatus> {
    match value {
        "open" => Ok(PackSessionStatus::Open),
        "ready_to_manifest" => Ok(PackSessionStatus::ReadyToManifest),
        _ => Err(AppError::conflict("packing session cannot reopen a carton")),
    }
}

fn carton_status(value: &str) -> AppResult<CartonStatus> {
    match value {
        "closed" => Ok(CartonStatus::Closed),
        "open" => Ok(CartonStatus::Open),
        "voided" => Ok(CartonStatus::Voided),
        _ => Err(AppError::internal("carton has an invalid state")),
    }
}

fn measurements(carton: &LockedCarton) -> AppResult<CartonMeasurements> {
    let weight = carton
        .weight_g
        .map(WeightGrams::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let dimensions = match (carton.length_mm, carton.width_mm, carton.height_mm) {
        (None, None, None) => None,
        (Some(length), Some(width), Some(height)) => Some(CartonDimensions::new(
            DimensionMillimeters::new(length)
                .map_err(|error| AppError::internal(error.to_string()))?,
            DimensionMillimeters::new(width)
                .map_err(|error| AppError::internal(error.to_string()))?,
            DimensionMillimeters::new(height)
                .map_err(|error| AppError::internal(error.to_string()))?,
        )),
        _ => return Err(AppError::internal("carton dimensions are incomplete")),
    };
    Ok(CartonMeasurements::new(weight, dimensions))
}

const fn reason_code(reason: CartonReopenReason) -> &'static str {
    match reason {
        CartonReopenReason::PackingCorrection => "packing_correction",
        CartonReopenReason::QualityIssue => "quality_issue",
        CartonReopenReason::OrderCancellation => "order_cancellation",
        CartonReopenReason::Other => "other",
    }
}
