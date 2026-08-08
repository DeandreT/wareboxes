//! Atomic, optimistic fulfillment-order cancellation.

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::order_cancellation::{
    CancelOrderCommand, CancelOrderResult, ORDER_CANCELLATION_OPERATION,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    InventoryOwnerId, OrderCancellationId, OrderId, OrderRevision, OrderStatus, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::{inventory_allocation, orders};

#[derive(Debug)]
struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    status: OrderStatus,
    revision: OrderRevision,
}

pub async fn cancel_order(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelOrderCommand,
) -> AppResult<CancelOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, ORDER_CANCELLATION_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "orders").await?;

    if let Some(result) = prepared.replayed::<CancelOrderResult>(&mut tx).await? {
        require_replayed_cancellation_visible_tx(
            &mut tx,
            access.tenant_id,
            result.cancellation_id,
            result.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order = lock_order_tx(&mut tx, access.tenant_id, command.order_id(), &scope).await?;
    if order.revision != command.expected_revision() {
        return Err(AppError::conflict(format!(
            "order revision changed from {} to {}",
            command.expected_revision().get(),
            order.revision.get()
        )));
    }
    if !matches!(order.status, OrderStatus::Open | OrderStatus::Held) {
        return Err(AppError::conflict(
            "only open or held orders can be cancelled before physical execution",
        ));
    }

    let commitments = inventory_allocation::cancel_order_commitments_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command.order_id().get(),
        &scope,
    )
    .await?;
    let occurred_at = now_iso();
    let released_hold_count = release_active_holds_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        context.actor_id.get(),
        command.order_id(),
        occurred_at,
    )
    .await?;
    let resulting_revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    update_order_status_tx(
        &mut tx,
        access.tenant_id,
        command.order_id(),
        order.status,
        order.revision,
    )
    .await?;

    let released_reservation_count = count_to_i64(
        commitments.reservation_count,
        "released reservation count overflow",
    )?;
    let released_allocation_count = count_to_i64(
        commitments.allocation_count,
        "released allocation count overflow",
    )?;
    let cancellation_id = insert_cancellation_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        context.actor_id.get(),
        command,
        order.status,
        resulting_revision,
        &commitments.affected_facility_ids,
        released_hold_count,
        released_reservation_count,
        released_allocation_count,
        commitments.released_quantity,
        occurred_at,
    )
    .await?;
    orders::insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id().get(),
        Some(context.actor_id.get()),
        &format!("cancelled order ({})", command.reason()),
    )
    .await?;

    let result = CancelOrderResult {
        cancellation_id,
        order_id: command.order_id(),
        inventory_owner_id: order.inventory_owner_id,
        previous_status: order.status,
        status: OrderStatus::Cancelled,
        revision: resulting_revision,
        reason: command.reason(),
        note: command.note().cloned(),
        released_hold_count,
        released_reservation_count,
        released_allocation_count,
        released_quantity: commitments.released_quantity,
    };
    enqueue_order_cancelled_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command.expected_revision(),
        &commitments.affected_facility_ids,
        &result,
        occurred_at,
    )
    .await?;

    Ok(prepared.commit(tx, result).await?)
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
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
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
    let status_value: String = row.try_get("status")?;
    Ok(LockedOrder {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: OrderStatus::parse(&status_value)
            .ok_or_else(|| AppError::internal("order has an invalid status"))?,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn require_replayed_cancellation_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    cancellation_id: OrderCancellationId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, affected_facility_ids
        FROM order_cancellations
        WHERE tenant_id = $1 AND id = $2 AND order_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(cancellation_id.get())
    .bind(order_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order cancellation"))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_ids: Vec<i64> = row.try_get("affected_facility_ids")?;
    if !scope.includes_inventory_owner(inventory_owner_id)
        || facility_ids
            .iter()
            .any(|facility_id| !scope.includes_facility(*facility_id))
    {
        return Err(AppError::not_found("order cancellation"));
    }
    Ok(())
}

async fn release_active_holds_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    actor_user_id: i64,
    order_id: OrderId,
    occurred_at: Timestamp,
) -> AppResult<i64> {
    let rows = sqlx::query(
        r#"
        SELECT id, reason_code
        FROM order_holds
        WHERE tenant_id = $1 AND order_id = $2 AND released_at IS NULL
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;

    for (index, row) in rows.iter().enumerate() {
        let hold_id: i64 = row.try_get("id")?;
        let reason: String = row.try_get("reason_code")?;
        let updated = sqlx::query(
            r#"
            UPDATE order_holds
            SET released_at = $1, released_by_user_id = $2,
                release_note = 'Order cancelled'
            WHERE tenant_id = $3 AND id = $4 AND released_at IS NULL
            "#,
        )
        .bind(occurred_at)
        .bind(actor_user_id)
        .bind(tenant_id.get())
        .bind(hold_id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict("order hold could not be released"));
        }
        let active_hold_count =
            count_to_i64(rows.len() - index - 1, "active order hold count overflow")?;
        orders::enqueue_order_hold_event(
            tx,
            tenant_id,
            inventory_owner_id,
            actor_user_id,
            order_id.get(),
            hold_id,
            "released",
            occurred_at,
            serde_json::json!({
                "order_id": order_id.get(),
                "order_hold_id": hold_id,
                "reason": reason,
                "release_reason": "order_cancelled",
                "order_status": "cancelled",
                "active_hold_count": active_hold_count,
            }),
        )
        .await?;
    }

    count_to_i64(rows.len(), "released hold count overflow")
}

async fn update_order_status_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    previous_status: OrderStatus,
    expected_revision: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders
        SET status = 'cancelled', revision = revision + 1
        WHERE tenant_id = $1 AND id = $2 AND status = $3 AND revision = $4
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(previous_status.as_str())
    .bind(expected_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed during cancellation"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_cancellation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    actor_user_id: i64,
    command: &CancelOrderCommand,
    previous_status: OrderStatus,
    resulting_revision: OrderRevision,
    affected_facility_ids: &[i64],
    released_hold_count: i64,
    released_reservation_count: i64,
    released_allocation_count: i64,
    released_quantity: i64,
    occurred_at: Timestamp,
) -> AppResult<OrderCancellationId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO order_cancellations (
            tenant_id, inventory_owner_id, order_id, actor_user_id, occurred_at,
            reason, note, previous_status, expected_revision, resulting_revision,
            affected_facility_ids, released_hold_count, released_reservation_count,
            released_allocation_count, released_quantity
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(command.order_id().get())
    .bind(actor_user_id)
    .bind(occurred_at)
    .bind(command.reason().as_str())
    .bind(command.note().map(|note| note.as_str()))
    .bind(previous_status.as_str())
    .bind(command.expected_revision().get())
    .bind(resulting_revision.get())
    .bind(affected_facility_ids)
    .bind(released_hold_count)
    .bind(released_reservation_count)
    .bind(released_allocation_count)
    .bind(released_quantity)
    .fetch_one(&mut **tx)
    .await?;
    OrderCancellationId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn enqueue_order_cancelled_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    expected_revision: OrderRevision,
    affected_facility_ids: &[i64],
    result: &CancelOrderResult,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let ordering_key = format!("order:{}", result.order_id.get());
    let sequence = orders::next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "order:{}:cancelled:{}",
        result.order_id.get(),
        result.revision.get()
    );
    let aggregate_id = result.order_id.to_string();
    let payload = serde_json::json!({
        "cancellation_id": result.cancellation_id.get(),
        "order_id": result.order_id.get(),
        "inventory_owner_id": result.inventory_owner_id.get(),
        "previous_status": result.previous_status.as_str(),
        "status": result.status.as_str(),
        "expected_revision": expected_revision.get(),
        "revision": result.revision.get(),
        "reason": result.reason.as_str(),
        "note": result.note.as_ref().map(|note| note.as_str()),
        "affected_facility_ids": affected_facility_ids,
        "released_hold_count": result.released_hold_count,
        "released_reservation_count": result.released_reservation_count,
        "released_allocation_count": result.released_allocation_count,
        "released_quantity": result.released_quantity,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(result.inventory_owner_id),
            facility_id: None,
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "order.cancelled",
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

fn count_to_i64(count: usize, error: &'static str) -> AppResult<i64> {
    i64::try_from(count).map_err(|_| AppError::internal(error))
}
