use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::picking::{
    AcceptPickShortageAsShortShipCommand, AcceptPickShortageAsShortShipResult,
    ACCEPT_PICK_SHORTAGE_AS_SHORT_SHIP_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    resolve_pick_shortage_as_short_ship, ActualPickQuantity, FacilityId, InventoryHoldId,
    InventoryOwnerId, OrderId, OrderLineId, OrderRevision, OrderStatus, PickQuantity,
    PickShortageDispositionId, PickShortageId, PickShortageQuantities, PickShortageRevision,
    PickShortageStatus, ShortShipDemandQuantities, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::{insert_order_activity_tx, next_outbox_sequence_tx};

use super::readiness::order_pick_readiness_tx;

#[derive(Debug, Clone, Copy)]
struct ShortageHint {
    order_id: OrderId,
}

#[derive(Debug, Clone, Copy)]
struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    status: OrderStatus,
    revision: OrderRevision,
}

#[derive(Debug, Clone, Copy)]
struct LockedShortage {
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    order_id: OrderId,
    order_line_id: OrderLineId,
    order_release_id: i64,
    reservation_id: i64,
    status: PickShortageStatus,
    revision: PickShortageRevision,
    quantities: PickShortageQuantities,
    reallocated_quantity: ActualPickQuantity,
    recovery_terminal_quantity: ActualPickQuantity,
    remaining_to_allocate_quantity: ActualPickQuantity,
    inventory_hold_id: InventoryHoldId,
}

pub async fn accept_short_shipment(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &AcceptPickShortageAsShortShipCommand,
) -> AppResult<AcceptPickShortageAsShortShipResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        ACCEPT_PICK_SHORTAGE_AS_SHORT_SHIP_OPERATION,
        command,
    )?;
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

    require_stored_disposition_visible_before_replay_tx(
        &mut tx,
        access.tenant_id,
        prepared.idempotency_key(),
        &scope,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AcceptPickShortageAsShortShipResult>(&mut tx)
        .await?
    {
        require_replayed_disposition_visible_tx(
            &mut tx,
            access.tenant_id,
            result.disposition_id,
            result.shortage_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let hint = shortage_hint_tx(&mut tx, access.tenant_id, command.shortage_id(), &scope).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, hint.order_id, &scope).await?;
    if order.status != OrderStatus::Processing {
        return Err(AppError::conflict(
            "only an order in picking execution can accept a short shipment",
        ));
    }
    if order.revision != command.expected_order_revision() {
        return Err(AppError::conflict("short-shipment order revision is stale"));
    }
    let shortage =
        lock_shortage_tx(&mut tx, access.tenant_id, command.shortage_id(), &scope).await?;
    if shortage.order_id != hint.order_id || shortage.inventory_owner_id != order.inventory_owner_id
    {
        return Err(AppError::not_found("pick shortage"));
    }
    if shortage.revision != command.expected_shortage_revision() {
        return Err(AppError::conflict(
            "short-shipment shortage revision is stale",
        ));
    }
    require_no_downstream_execution_tx(&mut tx, access.tenant_id, shortage.order_id).await?;

    let transition = resolve_pick_shortage_as_short_ship(
        shortage.status,
        shortage.quantities.short(),
        shortage.reallocated_quantity,
        shortage.recovery_terminal_quantity,
        shortage.remaining_to_allocate_quantity,
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    let current_readiness = order_pick_readiness_tx(
        &mut tx,
        access.tenant_id,
        shortage.inventory_owner_id,
        shortage.order_id,
    )
    .await?;
    if current_readiness.effective_demand_quantity <= transition.accepted_quantity().get() {
        return Err(AppError::conflict(
            "a short shipment must retain positive executable order demand",
        ));
    }
    let shortage_revision = shortage
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("pick shortage revision overflow"))?;
    let order_revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let resolved_at = now_iso();
    let disposition_id = insert_disposition_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        &shortage,
        transition.accepted_quantity(),
        shortage_revision,
        order_revision,
        resolved_at,
    )
    .await?;
    resolve_shortage_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        transition.accepted_quantity(),
        shortage_revision,
        resolved_at,
    )
    .await?;

    let readiness = order_pick_readiness_tx(
        &mut tx,
        access.tenant_id,
        shortage.inventory_owner_id,
        shortage.order_id,
    )
    .await?;
    let order_ready_to_pack = readiness.is_ready_to_pack();
    let order_status = if order_ready_to_pack {
        OrderStatus::AwaitingPacking
    } else {
        OrderStatus::Processing
    };
    update_order_tx(
        &mut tx,
        access.tenant_id,
        shortage.order_id,
        order.revision,
        order_status,
        order_revision,
    )
    .await?;

    let line_demand = line_demand_tx(
        &mut tx,
        access.tenant_id,
        shortage.inventory_owner_id,
        shortage.order_id,
        shortage.order_line_id,
    )
    .await?;
    let order_demand = ShortShipDemandQuantities::new(
        PickQuantity::new(readiness.ordered_quantity)
            .map_err(|error| AppError::internal(error.to_string()))?,
        ActualPickQuantity::new(readiness.accepted_short_quantity)
            .map_err(|error| AppError::internal(error.to_string()))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;

    let result = AcceptPickShortageAsShortShipResult {
        disposition_id,
        shortage_id: command.shortage_id(),
        previous_shortage_status: shortage.status,
        shortage_status: transition.status(),
        shortage_resolution: transition.resolution(),
        shortage_revision,
        order_id: shortage.order_id,
        order_line_id: shortage.order_line_id,
        previous_order_status: order.status,
        order_status,
        order_revision,
        order_ready_to_pack,
        shortage_quantities: shortage.quantities,
        reallocated_quantity: shortage.reallocated_quantity,
        recovery_terminal_quantity: shortage.recovery_terminal_quantity,
        accepted_short_quantity: transition.accepted_quantity(),
        line_demand,
        order_demand,
        inventory_hold_id: shortage.inventory_hold_id,
        reason: command.reason(),
        note: command.note().cloned(),
        resolved_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        resolved_at,
    };
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        shortage.inventory_owner_id,
        shortage.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "accepted {} unit(s) as a short shipment for pick shortage {}",
            result.accepted_short_quantity.get(),
            result.shortage_id.get()
        ),
    )
    .await?;
    enqueue_short_ship_event_tx(
        &mut tx,
        access.tenant_id,
        shortage.inventory_owner_id,
        shortage.facility_id,
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn require_stored_disposition_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    idempotency_key: &str,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let ids = sqlx::query(
        r#"
        SELECT (result_json->>'disposition_id')::BIGINT AS disposition_id,
               (result_json->>'shortage_id')::BIGINT AS shortage_id
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = $2 AND idempotency_key = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(ACCEPT_PICK_SHORTAGE_AS_SHORT_SHIP_OPERATION)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(ids) = ids {
        require_replayed_disposition_visible_tx(
            tx,
            tenant_id,
            PickShortageDispositionId::new(ids.try_get("disposition_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            PickShortageId::new(ids.try_get("shortage_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            scope,
        )
        .await?;
    }
    Ok(())
}

async fn require_replayed_disposition_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    disposition_id: PickShortageDispositionId,
    shortage_id: PickShortageId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id
        FROM pick_short_ship_dispositions
        WHERE tenant_id = $1 AND id = $2 AND pick_shortage_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(disposition_id.get())
    .bind(shortage_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("short-shipment disposition"))?;
    if !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
        || !scope.includes_facility(row.try_get("facility_id")?)
    {
        return Err(AppError::not_found("short-shipment disposition"));
    }
    Ok(())
}

async fn shortage_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage_id: PickShortageId,
    scope: &ScopeBindings,
) -> AppResult<ShortageHint> {
    let order_id: i64 = sqlx::query_scalar(
        r#"
        SELECT order_id FROM pick_shortages
        WHERE tenant_id = $1 AND id = $2
          AND ($3 OR inventory_owner_id = ANY($4))
          AND ($5 OR facility_id = ANY($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick shortage"))?;
    Ok(ShortageHint {
        order_id: OrderId::new(order_id).map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, status, revision
        FROM orders
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
          AND ($3 OR inventory_owner_id = ANY($4))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    Ok(LockedOrder {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: OrderStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or_else(|| AppError::internal("order has an invalid status"))?,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn lock_shortage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage_id: PickShortageId,
    scope: &ScopeBindings,
) -> AppResult<LockedShortage> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, order_id, order_item_id,
               order_release_id, reservation_id, status, revision,
               planned_qty, picked_qty, short_qty, reallocated_qty,
               recovery_terminal_qty, remaining_to_allocate_qty,
               inventory_hold_id
        FROM pick_shortages
        WHERE tenant_id = $1 AND id = $2
          AND ($3 OR inventory_owner_id = ANY($4))
          AND ($5 OR facility_id = ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick shortage"))?;
    let planned = PickQuantity::new(row.try_get("planned_qty")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let picked = ActualPickQuantity::new(row.try_get("picked_qty")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let quantities = PickShortageQuantities::new(planned, picked)
        .map_err(|error| AppError::internal(error.to_string()))?;
    if quantities.short().get() != row.try_get::<i64, _>("short_qty")? {
        return Err(AppError::internal(
            "pick shortage quantities do not conserve",
        ));
    }
    Ok(LockedShortage {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_line_id: OrderLineId::new(row.try_get("order_item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_release_id: row.try_get("order_release_id")?,
        reservation_id: row.try_get("reservation_id")?,
        status: PickShortageStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or_else(|| AppError::internal("pick shortage has an invalid status"))?,
        revision: PickShortageRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        quantities,
        reallocated_quantity: ActualPickQuantity::new(row.try_get("reallocated_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        recovery_terminal_quantity: ActualPickQuantity::new(row.try_get("recovery_terminal_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        remaining_to_allocate_quantity: ActualPickQuantity::new(
            row.try_get("remaining_to_allocate_qty")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_hold_id: InventoryHoldId::new(row.try_get("inventory_hold_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn require_no_downstream_execution_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM packing_sessions
            WHERE tenant_id = $1 AND order_id = $2
            UNION ALL
            SELECT 1 FROM shipments
            WHERE tenant_id = $1 AND order_id = $2
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        return Err(AppError::conflict(
            "short shipment cannot be accepted after packing has started",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_disposition_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &AcceptPickShortageAsShortShipCommand,
    shortage: &LockedShortage,
    accepted_quantity: PickQuantity,
    resulting_shortage_revision: PickShortageRevision,
    resulting_order_revision: OrderRevision,
    resolved_at: Timestamp,
) -> AppResult<PickShortageDispositionId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO pick_short_ship_dispositions (
            tenant_id, inventory_owner_id, facility_id, order_release_id,
            order_id, order_item_id, reservation_id, pick_shortage_id,
            accepted_short_qty, reason_code, note,
            expected_shortage_revision, resulting_shortage_revision,
            expected_order_revision, resulting_order_revision,
            disposed_by_user_id, disposed_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
            $12, $13, $14, $15, $16, $17
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.facility_id.get())
    .bind(shortage.order_release_id)
    .bind(shortage.order_id.get())
    .bind(shortage.order_line_id.get())
    .bind(shortage.reservation_id)
    .bind(command.shortage_id().get())
    .bind(accepted_quantity.get())
    .bind(command.reason().as_str())
    .bind(command.note().map(|note| note.as_str()))
    .bind(command.expected_shortage_revision().get())
    .bind(resulting_shortage_revision.get())
    .bind(command.expected_order_revision().get())
    .bind(resulting_order_revision.get())
    .bind(actor_user_id)
    .bind(resolved_at)
    .fetch_one(&mut **tx)
    .await?;
    PickShortageDispositionId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn resolve_shortage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &AcceptPickShortageAsShortShipCommand,
    accepted_quantity: PickQuantity,
    resulting_revision: PickShortageRevision,
    resolved_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE pick_shortages
        SET status = 'resolved', resolution = 'short_ship',
            accepted_short_qty = $1, revision = $2, modified_at = $3,
            resolved_by_user_id = $4, resolved_at = $3
        WHERE tenant_id = $5 AND id = $6 AND status = 'awaiting_inventory'
          AND revision = $7 AND resolution IS NULL AND accepted_short_qty = 0
        "#,
    )
    .bind(accepted_quantity.get())
    .bind(resulting_revision.get())
    .bind(resolved_at)
    .bind(actor_user_id)
    .bind(tenant_id.get())
    .bind(command.shortage_id().get())
    .bind(command.expected_shortage_revision().get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "pick shortage changed while accepting the short shipment",
        ));
    }
    Ok(())
}

async fn update_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    expected_revision: OrderRevision,
    status: OrderStatus,
    revision: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders SET status = $1, revision = $2
        WHERE tenant_id = $3 AND id = $4 AND status = 'processing' AND revision = $5
        "#,
    )
    .bind(status.as_str())
    .bind(revision.get())
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(expected_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "order changed while accepting the short shipment",
        ));
    }
    Ok(())
}

async fn line_demand_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
    order_line_id: OrderLineId,
) -> AppResult<ShortShipDemandQuantities> {
    let row = sqlx::query(
        r#"
        SELECT original_qty AS ordered_quantity,
               accepted_short_qty AS accepted_short_quantity
        FROM outbound_effective_demand
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND order_id = $3 AND order_item_id = $4
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .bind(order_line_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order line"))?;
    ShortShipDemandQuantities::new(
        PickQuantity::new(row.try_get("ordered_quantity")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        ActualPickQuantity::new(row.try_get("accepted_short_quantity")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))
}

async fn enqueue_short_ship_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    result: &AcceptPickShortageAsShortShipResult,
) -> AppResult<()> {
    let ordering_key = format!("order:{}", result.order_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "pick-shortage:{}:short-ship:{}",
        result.shortage_id.get(),
        result.shortage_revision.get()
    );
    let aggregate_id = result.shortage_id.to_string();
    let payload = serde_json::json!({
        "disposition_id": result.disposition_id,
        "pick_shortage_id": result.shortage_id,
        "shortage_revision": result.shortage_revision,
        "shortage_resolution": result.shortage_resolution,
        "order_id": result.order_id,
        "order_line_id": result.order_line_id,
        "order_status": result.order_status,
        "order_revision": result.order_revision,
        "order_ready_to_pack": result.order_ready_to_pack,
        "accepted_short_quantity": result.accepted_short_quantity,
        "line_demand": result.line_demand,
        "order_demand": result.order_demand,
        "inventory_hold_id": result.inventory_hold_id,
        "reason": result.reason,
        "note": result.note.as_ref().map(|note| note.as_str()),
        "resolved_by": result.resolved_by,
        "resolved_at": result.resolved_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(result.resolved_by.get()),
            event_key: &event_key,
            aggregate_type: "pick_shortage",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.pick.shortage_short_ship_accepted",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.resolved_at,
        },
    )
    .await?;
    Ok(())
}
