use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::packing::{
    CloseCartonCommand, CloseCartonResult, CreateCartonCommand, CreateCartonResult, PackCarton,
    PackCartonLifecycle, VoidCartonCommand, VoidCartonResult, CLOSE_CARTON_OPERATION,
    CREATE_CARTON_OPERATION, VOID_CARTON_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    complete_packing, continue_packing, open_carton, CartonId, OrderStatus, PackingProgress,
    TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::inventory_locking;
use crate::repo::orders::insert_order_activity_tx;

use super::{
    enqueue_order_event_tx, lock_order_tx, lock_session_tx, require_replayed_ids_visible_tx,
    require_revision, session_order_hint_tx,
};

pub async fn create_carton(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateCartonCommand,
) -> AppResult<CreateCartonResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let fingerprint = serde_json::json!({
        "session_id": command.session_id,
        "carton_barcode": command.carton_barcode,
        "expected_revision": command.expected_revision,
    });
    let prepared = PreparedCommand::new_v1(context, CREATE_CARTON_OPERATION, &fingerprint)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared.replayed::<CreateCartonResult>(&mut tx).await? {
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
    continue_packing(order.status).map_err(|error| AppError::conflict(error.to_string()))?;
    let session = lock_session_tx(&mut tx, access.tenant_id, command.session_id, &scope).await?;
    if session.order_id != order_id || session.state != "open" {
        return Err(AppError::conflict("packing session is not open"));
    }
    let revision = require_revision(&order, Some(&session), command.expected_revision)?;
    open_carton(session.open_carton_count)
        .map_err(|error| AppError::conflict(error.to_string()))?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "license-plate-barcode:{}:{}",
            access.tenant_id, command.carton_barcode
        ))
        .execute(&mut *tx)
        .await?;
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM license_plates WHERE tenant_id = $1 AND barcode = $2)",
    )
    .bind(access.tenant_id.get())
    .bind(command.carton_barcode.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if existing {
        return Err(AppError::conflict("carton barcode already exists"));
    }
    let created_at = now_iso();
    let plate_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO license_plates (
            tenant_id, inventory_owner_id, created, barcode, facility_id, location_id
        ) VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(created_at)
    .bind(command.carton_barcode.as_str())
    .bind(session.facility_id)
    .bind(session.packing_location_id)
    .fetch_one(&mut *tx)
    .await?;
    let carton_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO cartons (
            tenant_id, inventory_owner_id, facility_id, packing_session_id,
            order_release_id, order_id, packing_location_id, license_plate_id,
            state, created_by_user_id, created_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'open', $9, $10)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(session.inventory_owner_id.get())
    .bind(session.facility_id)
    .bind(session.id.get())
    .bind(session.order_release_id)
    .bind(order_id.get())
    .bind(session.packing_location_id)
    .bind(plate_id)
    .bind(context.actor_id.get())
    .bind(created_at)
    .fetch_one(&mut *tx)
    .await?;
    let carton_id =
        CartonId::new(carton_id).map_err(|error| AppError::internal(error.to_string()))?;
    update_session_revision_and_cartons_tx(
        &mut tx,
        access.tenant_id,
        &session,
        revision,
        session.open_carton_count + 1,
        session.closed_carton_count,
        None,
    )
    .await?;
    update_order_tx(
        &mut tx,
        access.tenant_id,
        order_id,
        order.status,
        order.revision,
        order.status,
        revision,
    )
    .await?;
    let progress = progress(
        &session,
        session.open_carton_count + 1,
        session.closed_carton_count,
    )?;
    let carton = PackCarton {
        carton_id,
        carton_barcode: command.carton_barcode.clone(),
        lifecycle: PackCartonLifecycle::Open,
        content_count: 0,
        created_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        created_at,
    };
    let result = CreateCartonResult {
        session_id: session.id,
        order_id,
        carton,
        revision,
        progress,
    };
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        order_id.get(),
        Some(context.actor_id.get()),
        &format!("created carton {}", command.carton_barcode),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        session.facility_id,
        context.actor_id.get(),
        order_id,
        "packing.carton_created",
        &format!("carton:{}:created", carton_id.get()),
        serde_json::json!({
            "packing_session_id": session.id,
            "carton_id": carton_id,
            "order_id": order_id,
            "carton_barcode": command.carton_barcode,
            "revision": revision,
            "created_at": created_at,
        }),
        created_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn close_carton(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CloseCartonCommand,
) -> AppResult<CloseCartonResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let fingerprint = serde_json::json!({
        "session_id": command.session_id,
        "carton_id": command.carton_id,
        "carton_barcode": command.carton_barcode,
        "measurements": command.measurements,
        "expected_revision": command.expected_revision,
    });
    let prepared = PreparedCommand::new_v1(context, CLOSE_CARTON_OPERATION, &fingerprint)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared.replayed::<CloseCartonResult>(&mut tx).await? {
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
    continue_packing(order.status).map_err(|error| AppError::conflict(error.to_string()))?;
    let session = lock_session_tx(&mut tx, access.tenant_id, command.session_id, &scope).await?;
    if session.order_id != order_id || session.state != "open" {
        return Err(AppError::conflict("packing session is not open"));
    }
    let revision = require_revision(&order, Some(&session), command.expected_revision)?;
    let carton = lock_carton_tx(&mut tx, access.tenant_id, &session, command.carton_id).await?;
    inventory_locking::lock_license_plate(&mut tx, access.tenant_id, Some(carton.license_plate_id))
        .await?;
    if carton.state != "open" || carton.barcode != command.carton_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned carton does not match the open carton",
        ));
    }
    let content_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM carton_contents WHERE tenant_id = $1 AND carton_id = $2",
    )
    .bind(access.tenant_id.get())
    .bind(command.carton_id.get())
    .fetch_one(&mut *tx)
    .await?;
    wareboxes_domain::CartonStatus::Open
        .close(content_count)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let closed_at = now_iso();
    let dimensions = command.measurements.dimensions();
    let updated = sqlx::query(
        r#"
        UPDATE cartons
        SET state = 'closed', closed_by_user_id = $1, closed_at = $2,
            weight_g = $3, length_mm = $4, width_mm = $5, height_mm = $6
        WHERE tenant_id = $7 AND id = $8 AND packing_session_id = $9 AND state = 'open'
        "#,
    )
    .bind(context.actor_id.get())
    .bind(closed_at)
    .bind(command.measurements.weight_grams().map(|value| value.get()))
    .bind(dimensions.map(|value| value.length_mm().get()))
    .bind(dimensions.map(|value| value.width_mm().get()))
    .bind(dimensions.map(|value| value.height_mm().get()))
    .bind(access.tenant_id.get())
    .bind(command.carton_id.get())
    .bind(session.id.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("carton changed while closing"));
    }
    let next_progress = progress(
        &session,
        session.open_carton_count - 1,
        session.closed_carton_count + 1,
    )?;
    let ready = next_progress.ready_to_manifest();
    let next_order_status = if ready {
        complete_packing(order.status, next_progress)
            .map_err(|error| AppError::conflict(error.to_string()))?
    } else {
        order.status
    };
    update_session_revision_and_cartons_tx(
        &mut tx,
        access.tenant_id,
        &session,
        revision,
        next_progress.open_carton_count(),
        next_progress.closed_carton_count(),
        ready.then_some((context.actor_id.get(), closed_at)),
    )
    .await?;
    update_order_tx(
        &mut tx,
        access.tenant_id,
        order_id,
        order.status,
        order.revision,
        next_order_status,
        revision,
    )
    .await?;
    let lifecycle = PackCartonLifecycle::Closed {
        measurements: command.measurements,
        closed_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        closed_at,
    };
    let result = CloseCartonResult {
        session_id: session.id,
        carton_id: command.carton_id,
        order_id,
        lifecycle,
        order_status: next_order_status,
        revision,
        progress: next_progress,
    };
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        order_id.get(),
        Some(context.actor_id.get()),
        if ready {
            "closed final carton; order is ready to manifest"
        } else {
            "closed packing carton"
        },
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        session.facility_id,
        context.actor_id.get(),
        order_id,
        "packing.carton_closed",
        &format!("carton:{}:closed", command.carton_id.get()),
        serde_json::json!({
            "packing_session_id": session.id,
            "carton_id": command.carton_id,
            "order_id": order_id,
            "order_status": next_order_status,
            "revision": revision,
            "ready_to_manifest": ready,
            "closed_at": closed_at,
        }),
        closed_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn void_carton(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &VoidCartonCommand,
) -> AppResult<VoidCartonResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let fingerprint = serde_json::json!({
        "session_id": command.session_id,
        "carton_id": command.carton_id,
        "carton_barcode": command.carton_barcode,
        "expected_revision": command.expected_revision,
    });
    let prepared = PreparedCommand::new_v1(context, VOID_CARTON_OPERATION, &fingerprint)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared.replayed::<VoidCartonResult>(&mut tx).await? {
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
    continue_packing(order.status).map_err(|error| AppError::conflict(error.to_string()))?;
    let session = lock_session_tx(&mut tx, access.tenant_id, command.session_id, &scope).await?;
    if session.order_id != order_id || session.state != "open" {
        return Err(AppError::conflict("packing session is not open"));
    }
    let revision = require_revision(&order, Some(&session), command.expected_revision)?;
    let carton = lock_carton_tx(&mut tx, access.tenant_id, &session, command.carton_id).await?;
    inventory_locking::lock_license_plate(&mut tx, access.tenant_id, Some(carton.license_plate_id))
        .await?;
    if carton.state != "open" || carton.barcode != command.carton_barcode.as_str() {
        return Err(AppError::bad_request(
            "scanned carton does not match the open carton",
        ));
    }
    let content_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM carton_contents WHERE tenant_id = $1 AND carton_id = $2",
    )
    .bind(access.tenant_id.get())
    .bind(command.carton_id.get())
    .fetch_one(&mut *tx)
    .await?;
    wareboxes_domain::CartonStatus::Open
        .void(content_count)
        .map_err(|error| AppError::conflict(error.to_string()))?;

    let voided_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE cartons
        SET state = 'voided', voided_by_user_id = $1, voided_at = $2
        WHERE tenant_id = $3 AND id = $4 AND packing_session_id = $5 AND state = 'open'
        "#,
    )
    .bind(context.actor_id.get())
    .bind(voided_at)
    .bind(access.tenant_id.get())
    .bind(command.carton_id.get())
    .bind(session.id.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("carton changed while voiding"));
    }

    let next_progress = progress(
        &session,
        session.open_carton_count - 1,
        session.closed_carton_count,
    )?;
    update_session_revision_and_cartons_tx(
        &mut tx,
        access.tenant_id,
        &session,
        revision,
        next_progress.open_carton_count(),
        next_progress.closed_carton_count(),
        None,
    )
    .await?;
    update_order_tx(
        &mut tx,
        access.tenant_id,
        order_id,
        order.status,
        order.revision,
        order.status,
        revision,
    )
    .await?;
    let lifecycle = PackCartonLifecycle::Voided {
        voided_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        voided_at,
    };
    let result = VoidCartonResult {
        session_id: session.id,
        carton_id: command.carton_id,
        order_id,
        lifecycle,
        revision,
        progress: next_progress,
    };
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        order_id.get(),
        Some(context.actor_id.get()),
        &format!("voided empty carton {}", command.carton_barcode),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        session.inventory_owner_id,
        session.facility_id,
        context.actor_id.get(),
        order_id,
        "packing.carton_voided",
        &format!("carton:{}:voided", command.carton_id.get()),
        serde_json::json!({
            "packing_session_id": session.id,
            "carton_id": command.carton_id,
            "order_id": order_id,
            "revision": revision,
            "voided_at": voided_at,
        }),
        voided_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

struct LockedCarton {
    license_plate_id: i64,
    barcode: String,
    state: String,
}

async fn lock_carton_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    carton_id: CartonId,
) -> AppResult<LockedCarton> {
    let row = sqlx::query(
        r#"
        SELECT carton.license_plate_id, plate.barcode, carton.state
        FROM cartons carton
        INNER JOIN license_plates plate
          ON plate.tenant_id = carton.tenant_id
         AND plate.inventory_owner_id = carton.inventory_owner_id
         AND plate.facility_id = carton.facility_id
         AND plate.id = carton.license_plate_id
        WHERE carton.tenant_id = $1 AND carton.id = $2
          AND carton.packing_session_id = $3
        FOR UPDATE OF carton
        "#,
    )
    .bind(tenant_id.get())
    .bind(carton_id.get())
    .bind(session.id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("carton"))?;
    Ok(LockedCarton {
        license_plate_id: row.try_get("license_plate_id")?,
        barcode: row.try_get("barcode")?,
        state: row.try_get("state")?,
    })
}

fn progress(
    session: &super::LockedSession,
    open_carton_count: i64,
    closed_carton_count: i64,
) -> AppResult<PackingProgress> {
    PackingProgress::new(
        session.expected_allocation_count,
        session.packed_allocation_count,
        session.expected_qty,
        session.packed_qty,
        open_carton_count,
        closed_carton_count,
    )
    .map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn update_session_revision_and_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &super::LockedSession,
    revision: wareboxes_domain::OrderRevision,
    open_carton_count: i64,
    closed_carton_count: i64,
    ready: Option<(i64, Timestamp)>,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE packing_sessions
        SET revision = $1, open_carton_count = $2, closed_carton_count = $3,
            state = CASE WHEN $4::BIGINT IS NULL THEN state ELSE 'ready_to_manifest' END,
            ready_by_user_id = $4, ready_at = $5
        WHERE tenant_id = $6 AND id = $7 AND state = 'open' AND revision = $8
        "#,
    )
    .bind(revision.get())
    .bind(open_carton_count)
    .bind(closed_carton_count)
    .bind(ready.map(|value| value.0))
    .bind(ready.map(|value| value.1))
    .bind(tenant_id.get())
    .bind(session.id.get())
    .bind(session.revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("packing session changed"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn update_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: wareboxes_domain::OrderId,
    current_status: OrderStatus,
    current_revision: wareboxes_domain::OrderRevision,
    next_status: OrderStatus,
    next_revision: wareboxes_domain::OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders SET status = $1, revision = $2
        WHERE tenant_id = $3 AND id = $4 AND status = $5 AND revision = $6
        "#,
    )
    .bind(next_status.as_str())
    .bind(next_revision.get())
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(current_status.as_str())
    .bind(current_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed during packing"));
    }
    Ok(())
}
