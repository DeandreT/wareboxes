use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::shipping::{
    CancelShipmentCommand, CancelShipmentResult, CANCEL_SHIPMENT_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    cancel_shipment as validate_cancellation, OrderRevision, OrderStatus, ShipmentCancellationId,
    ShipmentCancellationReason, ShipmentStatus, TenantId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::insert_order_activity_tx;

use super::read_model::load_shipment_tx;
use super::{
    enqueue_order_event_tx, lock_order_tx, lock_shipment_tx, order_hint_for_shipment_tx, positive,
    require_replayed_shipment_id_visible_tx,
};

pub async fn cancel_shipment(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelShipmentCommand,
) -> AppResult<CancelShipmentResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_SHIPMENT_OPERATION, command)?;
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
    require_stored_cancellation_visible_before_replay_tx(
        &mut tx,
        access.tenant_id,
        prepared.idempotency_key(),
        &scope,
    )
    .await?;
    if let Some(result) = prepared.replayed::<CancelShipmentResult>(&mut tx).await? {
        require_replayed_shipment_id_visible_tx(
            &mut tx,
            access.tenant_id,
            result.shipment.shipment_id,
            result.shipment.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id =
        order_hint_for_shipment_tx(&mut tx, access.tenant_id, command.shipment_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("shipment-order:{}:{order_id}", access.tenant_id))
        .execute(&mut *tx)
        .await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    let shipment = lock_shipment_tx(&mut tx, access.tenant_id, command.shipment_id, &scope).await?;
    if shipment.order_id != order.id || shipment.inventory_owner_id != order.inventory_owner_id {
        return Err(AppError::not_found("shipment"));
    }
    if order.status != OrderStatus::AwaitingShipment {
        return Err(AppError::conflict(
            "shipment order is no longer awaiting shipment",
        ));
    }
    if shipment.revision != command.expected_shipment_revision {
        return Err(AppError::conflict(
            "shipment cancellation revision is stale",
        ));
    }
    if order.revision != command.expected_order_revision
        || order.revision != shipment.creation_resulting_order_revision
    {
        return Err(AppError::conflict(
            "shipment cancellation order revision is stale",
        ));
    }
    let status = validate_cancellation(shipment.status)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let active_load_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM outbound_load_shipments
            WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
              AND shipment_id=$4 AND closed_at IS NULL)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment.inventory_owner_id.get())
    .bind(shipment.facility_id.get())
    .bind(shipment.id.get())
    .fetch_one(&mut *tx)
    .await?;
    if active_load_exists {
        return Err(AppError::conflict(
            "shipment is assigned to an active outbound load",
        ));
    }
    let carrier_manifest_id: Option<i64> = if shipment.status == ShipmentStatus::Manifested {
        Some(
            sqlx::query_scalar(
                r#"
                SELECT id FROM shipment_manifests
                WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
                  AND shipment_id=$4
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(shipment.inventory_owner_id.get())
            .bind(shipment.facility_id.get())
            .bind(shipment.id.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::internal("manifested shipment has no manifest"))?,
        )
    } else {
        None
    };
    let next_shipment_revision = shipment
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("shipment revision overflow"))?;
    let packing_revision = lock_ready_packing_revision_tx(
        &mut tx,
        access.tenant_id,
        shipment.packing_session_id.get(),
        shipment.order_id.get(),
    )
    .await?;
    if packing_revision != shipment.creation_expected_order_revision {
        return Err(AppError::conflict(
            "shipment cancellation packing revision is stale",
        ));
    }
    let resulting_packing_revision = packing_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("packing revision overflow"))?;
    if resulting_packing_revision != order.revision {
        return Err(AppError::conflict(
            "shipment and packing revisions cannot be resynchronized",
        ));
    }

    let cancelled_at = now_iso();
    let cancellation_id_raw: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO shipment_cancellations (
            tenant_id,inventory_owner_id,facility_id,shipment_id,
            previous_shipment_state,carrier_manifest_id,
            packing_session_id,order_release_id,order_id,attempt,
            expected_shipment_revision,resulting_shipment_revision,
            expected_order_revision,resulting_order_revision,
            expected_packing_revision,resulting_packing_revision,
            carton_count,content_count,packed_qty,reason_code,note,
            cancelled_by_user_id,cancelled_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$13,$14,$15,
                $16,$17,$18,$19,$20,$21,$22)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment.inventory_owner_id.get())
    .bind(shipment.facility_id.get())
    .bind(shipment.id.get())
    .bind(shipment.status.as_str())
    .bind(carrier_manifest_id)
    .bind(shipment.packing_session_id.get())
    .bind(shipment.order_release_id)
    .bind(shipment.order_id.get())
    .bind(shipment.attempt)
    .bind(shipment.revision.get())
    .bind(next_shipment_revision.get())
    .bind(order.revision.get())
    .bind(packing_revision.get())
    .bind(resulting_packing_revision.get())
    .bind(shipment.carton_count)
    .bind(shipment.content_count)
    .bind(shipment.shipped_qty)
    .bind(cancellation_reason_wire(command.details.reason()))
    .bind(command.details.note().map(|note| note.as_str()))
    .bind(context.actor_id.get())
    .bind(cancelled_at)
    .fetch_one(&mut *tx)
    .await?;

    let updated = sqlx::query(
        r#"
        UPDATE shipments
        SET state=$1,revision=$2,cancelled_by_user_id=$3,cancelled_at=$4
        WHERE tenant_id=$5 AND id=$6 AND state=$7 AND revision=$8
        "#,
    )
    .bind(status.as_str())
    .bind(next_shipment_revision.get())
    .bind(context.actor_id.get())
    .bind(cancelled_at)
    .bind(access.tenant_id.get())
    .bind(shipment.id.get())
    .bind(shipment.status.as_str())
    .bind(shipment.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("shipment changed while cancelling"));
    }
    let updated = sqlx::query(
        r#"
        UPDATE packing_sessions SET revision=$1
        WHERE tenant_id=$2 AND id=$3 AND state='ready_to_manifest' AND revision=$4
        "#,
    )
    .bind(resulting_packing_revision.get())
    .bind(access.tenant_id.get())
    .bind(shipment.packing_session_id.get())
    .bind(packing_revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "packing session changed while cancelling shipment",
        ));
    }

    let cancellation_id = positive(cancellation_id_raw, ShipmentCancellationId::new)?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "cancelled shipment {} attempt {} before departure ({})",
            shipment.id,
            shipment.attempt,
            cancellation_reason_wire(command.details.reason())
        ),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
        context.actor_id.get(),
        shipment.order_id,
        "shipping.shipment_cancelled",
        &format!("shipment:{}:cancelled", shipment.id),
        serde_json::json!({
            "cancellation_id": cancellation_id,
            "shipment_id": shipment.id,
            "attempt": shipment.attempt,
            "previous_status": shipment.status.as_str(),
            "carrier_manifest_id": carrier_manifest_id,
            "packing_session_id": shipment.packing_session_id,
            "order_id": shipment.order_id,
            "shipment_revision": next_shipment_revision,
            "packing_session_revision": resulting_packing_revision,
            "order_revision": order.revision,
            "reason": cancellation_reason_wire(command.details.reason()),
            "note": command.details.note().map(|note| note.as_str()),
            "cancelled_by": context.actor_id,
            "cancelled_at": cancelled_at,
        }),
        cancelled_at,
    )
    .await?;
    let shipment = load_shipment_tx(&mut tx, access.tenant_id, shipment.id, &scope).await?;
    Ok(prepared
        .commit(
            tx,
            CancelShipmentResult {
                shipment,
                packing_session_revision: resulting_packing_revision,
            },
        )
        .await?)
}

async fn lock_ready_packing_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    packing_session_id: i64,
    order_id: i64,
) -> AppResult<OrderRevision> {
    let revision: i64 = sqlx::query_scalar(
        r#"
        SELECT revision FROM packing_sessions
        WHERE tenant_id=$1 AND id=$2 AND order_id=$3 AND state='ready_to_manifest'
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(packing_session_id)
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("packing session is no longer ready to manifest"))?;
    positive(revision, OrderRevision::new)
}

async fn require_stored_cancellation_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    idempotency_key: &str,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored: Option<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT (result_json->'shipment'->>'shipment_id')::bigint,
               (result_json->'shipment'->>'order_id')::bigint
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(tenant_id.get())
    .bind(CANCEL_SHIPMENT_OPERATION)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((shipment_id, order_id)) = stored {
        require_replayed_shipment_id_visible_tx(
            tx,
            tenant_id,
            positive(shipment_id, wareboxes_domain::ShipmentId::new)?,
            positive(order_id, wareboxes_domain::OrderId::new)?,
            scope,
        )
        .await?;
    }
    Ok(())
}

const fn cancellation_reason_wire(reason: ShipmentCancellationReason) -> &'static str {
    match reason {
        ShipmentCancellationReason::PackingCorrection => "packing_correction",
        ShipmentCancellationReason::ShippingDataCorrection => "shipping_data_correction",
        ShipmentCancellationReason::DuplicateShipment => "duplicate_shipment",
        ShipmentCancellationReason::OperatorError => "operator_error",
        ShipmentCancellationReason::Other => "other",
    }
}
