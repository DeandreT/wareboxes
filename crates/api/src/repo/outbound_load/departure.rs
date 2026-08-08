use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbound_load::{
    ConfirmOutboundLoadDepartureCommand, ConfirmOutboundLoadDepartureResult,
    CONFIRM_OUTBOUND_LOAD_DEPARTURE_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    depart_outbound_load, OrderId, OrderRevision, ShipmentId, ShipmentRevision,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::shipping::{depart_for_outbound_load_tx, OutboundLoadShipmentTarget};

use super::{enqueue_load_event_tx, load_progress_tx, lock_load_tx, require_load_visible_tx};

#[derive(Debug)]
struct LinkedShipment {
    shipment_id: ShipmentId,
    order_id: OrderId,
    expected_shipment_revision: ShipmentRevision,
    expected_order_revision: OrderRevision,
}

pub async fn confirm_departure(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfirmOutboundLoadDepartureCommand,
) -> AppResult<ConfirmOutboundLoadDepartureResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, CONFIRM_OUTBOUND_LOAD_DEPARTURE_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<ConfirmOutboundLoadDepartureResult>(&mut tx)
        .await?
    {
        require_load_visible_tx(&mut tx, access.tenant_id, result.outbound_load_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let load = lock_load_tx(&mut tx, access.tenant_id, command.outbound_load_id, &scope).await?;
    if load.revision != command.expected_revision {
        return Err(AppError::conflict("outbound load revision is stale"));
    }
    validate_scans_tx(&mut tx, access, &load, command).await?;
    let progress = load_progress_tx(&mut tx, access.tenant_id, &load).await?;
    let transition =
        depart_outbound_load(progress).map_err(|error| AppError::conflict(error.to_string()))?;
    let linked_shipments = lock_membership_tx(&mut tx, access, &load).await?;
    let revision = load
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("outbound load revision overflow"))?;
    let departed_at = now_iso();
    let updated = sqlx::query(
        r#"
        UPDATE outbound_loads
        SET state='departed',revision=$3,departed_by_user_id=$4,departed_at=$5
        WHERE tenant_id=$1 AND id=$2 AND state='ready_to_depart' AND revision=$6
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(revision.get())
    .bind(context.actor_id.get())
    .bind(departed_at)
    .bind(load.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("outbound load changed during departure"));
    }
    let mut shipment_departures = Vec::with_capacity(linked_shipments.len());
    for linked in linked_shipments {
        shipment_departures.push(
            depart_for_outbound_load_tx(
                &mut tx,
                access,
                &scope,
                context,
                &prepared,
                OutboundLoadShipmentTarget {
                    shipment_id: linked.shipment_id,
                    order_id: linked.order_id,
                    expected_shipment_revision: linked.expected_shipment_revision,
                    expected_order_revision: linked.expected_order_revision,
                },
                departed_at,
            )
            .await?,
        );
    }
    let cartons = sqlx::query(
        r#"
        UPDATE outbound_load_cartons
        SET state='departed',revision=revision+1,departed_at=$3,closed_at=$3
        WHERE tenant_id=$1 AND outbound_load_id=$2 AND state='loaded' AND closed_at IS NULL
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(departed_at)
    .fetch_all(&mut *tx)
    .await?;
    if i64::try_from(cartons.len()).ok() != Some(load.carton_count) {
        return Err(AppError::conflict(
            "outbound load carton set changed during departure",
        ));
    }
    let links = sqlx::query(
        r#"
        UPDATE outbound_load_shipments SET closed_at=$3
        WHERE tenant_id=$1 AND outbound_load_id=$2 AND closed_at IS NULL
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(departed_at)
    .fetch_all(&mut *tx)
    .await?;
    if i64::try_from(links.len()).ok() != Some(load.shipment_count) {
        return Err(AppError::conflict(
            "outbound load shipment set changed during departure",
        ));
    }
    let result = ConfirmOutboundLoadDepartureResult {
        outbound_load_id: load.id,
        status: transition.progress.status(),
        revision,
        shipment_departures,
        departed_by: context.actor_id,
        departed_at,
    };
    enqueue_load_event_tx(
        &mut tx,
        super::LoadEvent {
            tenant_id: access.tenant_id,
            facility_id: load.facility_id,
            actor_user_id: context.actor_id.get(),
            load_id: load.id,
            event_type: "outbound.load.departed",
            event_key: "departed",
            payload: serde_json::to_value(&result)
                .map_err(|error| AppError::internal(error.to_string()))?,
            occurred_at: departed_at,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    load: &super::LockedLoad,
    command: &ConfirmOutboundLoadDepartureCommand,
) -> AppResult<()> {
    if load.load_barcode != command.load_barcode.as_str()
        || load.trailer_number.as_deref() != Some(command.trailer_number.as_str())
        || load.seal_number.as_deref() != Some(command.seal_number.as_str())
    {
        return Err(AppError::bad_request(
            "load, trailer, or seal scan does not match outbound load",
        ));
    }
    let dock_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND barcode=$3
          AND active AND deleted IS NULL AND lower(type)='dock'
          AND NOT pickable AND NOT receivable
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.facility_id.get())
    .bind(command.dock_location_barcode.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    if dock_id != load.dock_location_id {
        return Err(AppError::bad_request(
            "dock scan does not match outbound load",
        ));
    }
    Ok(())
}

async fn lock_membership_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    load: &super::LockedLoad,
) -> AppResult<Vec<LinkedShipment>> {
    let order_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT order_id FROM outbound_load_shipments WHERE tenant_id=$1 AND outbound_load_id=$2 AND closed_at IS NULL ORDER BY order_id",
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query("SELECT id FROM orders WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE")
        .bind(access.tenant_id.get())
        .bind(&order_ids)
        .fetch_all(&mut **tx)
        .await?;
    let shipment_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT shipment_id FROM outbound_load_shipments WHERE tenant_id=$1 AND outbound_load_id=$2 AND closed_at IS NULL ORDER BY shipment_id",
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        "SELECT id FROM shipments WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(&shipment_ids)
    .fetch_all(&mut **tx)
    .await?;
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
    let rows = sqlx::query(
        r#"
        SELECT shipment_id,order_id,expected_shipment_revision,expected_order_revision
        FROM outbound_load_shipments
        WHERE tenant_id=$1 AND outbound_load_id=$2 AND closed_at IS NULL
        ORDER BY shipment_sequence,id
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .fetch_all(&mut **tx)
    .await?;
    if i64::try_from(rows.len()).ok() != Some(load.shipment_count) {
        return Err(AppError::conflict("outbound load shipment set changed"));
    }
    rows.into_iter()
        .map(|row| {
            Ok(LinkedShipment {
                shipment_id: super::positive(row.try_get("shipment_id")?, ShipmentId::new)?,
                order_id: super::positive(row.try_get("order_id")?, OrderId::new)?,
                expected_shipment_revision: super::positive(
                    row.try_get("expected_shipment_revision")?,
                    ShipmentRevision::new,
                )?,
                expected_order_revision: super::positive(
                    row.try_get("expected_order_revision")?,
                    OrderRevision::new,
                )?,
            })
        })
        .collect()
}
