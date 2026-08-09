//! Atomic exact replacement of pre-execution fulfillment demand lines.

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::order_line_amendment::{
    ReplaceFulfillmentOrderLinesCommand, ReplaceFulfillmentOrderLinesResult,
    ReplacedOrderLineReadModel, REPLACE_FULFILLMENT_ORDER_LINES_OPERATION,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    replace_fulfillment_order_lines as transition_order_lines, CatalogItemId,
    FulfillmentOrderDemandLine, InventoryOwnerId, OrderLineAmendmentError, OrderLineAmendmentId,
    OrderLineId, OrderLineKey, OrderQuantity, OrderRevision, OrderStatus, RequestedUom, TenantId,
    Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::{inventory_allocation, orders};

struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    status: OrderStatus,
    revision: OrderRevision,
}

struct CurrentLine {
    id: OrderLineId,
    definition: FulfillmentOrderDemandLine,
}

pub async fn replace_fulfillment_order_lines(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReplaceFulfillmentOrderLinesCommand,
) -> AppResult<ReplaceFulfillmentOrderLinesResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, REPLACE_FULFILLMENT_ORDER_LINES_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "orders").await?;

    require_stored_amendment_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ReplaceFulfillmentOrderLinesResult>(&mut tx)
        .await?
    {
        require_replay_visible_tx(&mut tx, access.tenant_id, &result, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order = lock_order_tx(&mut tx, access.tenant_id, command.order_id().get(), &scope).await?;
    if order.revision != command.expected_revision() {
        return Err(AppError::conflict(format!(
            "order revision changed from {} to {}",
            command.expected_revision().get(),
            order.revision.get()
        )));
    }
    let current =
        lock_current_lines_tx(&mut tx, access.tenant_id, command.order_id().get()).await?;
    let requested = command
        .lines()
        .iter()
        .map(|line| {
            line.as_domain()
                .map_err(|error| AppError::bad_request(error.to_string()))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let transition = transition_order_lines(
        order.status,
        order.revision,
        &current
            .iter()
            .map(|line| line.definition.clone())
            .collect::<Vec<_>>(),
        &requested,
    )
    .map_err(map_transition_error)?;
    lock_requested_catalog_tx(&mut tx, access.tenant_id, order.inventory_owner_id, command).await?;

    let amended_at = now_iso();
    let commitments = inventory_allocation::cancel_order_commitments_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command.order_id().get(),
        &scope,
        amended_at,
        "order_lines_replaced",
    )
    .await?;
    let previous_quantity =
        sum_quantities(current.iter().map(|line| line.definition.quantity().get()))?;
    let resulting_quantity = sum_quantities(command.lines().iter().map(|line| line.quantity()))?;
    let amendment_id = insert_amendment_tx(
        &mut tx,
        access.tenant_id,
        &order,
        context,
        command,
        transition.revision,
        count_i64(current.len(), "previous line count overflow")?,
        previous_quantity,
        count_i64(command.lines().len(), "resulting line count overflow")?,
        resulting_quantity,
        count_i64(
            commitments.reservation_count,
            "released reservation count overflow",
        )?,
        count_i64(
            commitments.allocation_count,
            "released allocation count overflow",
        )?,
        commitments.released_quantity,
        amended_at,
    )
    .await?;
    set_amendment_context_tx(&mut tx, amendment_id).await?;
    snapshot_previous_lines_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id().get(),
        amendment_id,
        &current,
    )
    .await?;
    retire_previous_lines_tx(
        &mut tx,
        access.tenant_id,
        command.order_id().get(),
        amended_at,
        current.len(),
    )
    .await?;
    let lines = insert_resulting_lines_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id().get(),
        amendment_id,
        command,
        amended_at,
    )
    .await?;
    update_order_revision_tx(
        &mut tx,
        access.tenant_id,
        command.order_id().get(),
        order.status,
        command.expected_revision(),
    )
    .await?;
    orders::insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id().get(),
        Some(context.actor_id.get()),
        "replaced fulfillment order demand lines",
    )
    .await?;

    let result = ReplaceFulfillmentOrderLinesResult {
        amendment_id,
        order_id: command.order_id(),
        inventory_owner_id: order.inventory_owner_id,
        order_status: order.status,
        previous_revision: order.revision,
        revision: transition.revision,
        previous_line_count: count_i64(current.len(), "previous line count overflow")?,
        previous_quantity,
        resulting_quantity,
        released_reservation_count: count_i64(
            commitments.reservation_count,
            "released reservation count overflow",
        )?,
        released_allocation_count: count_i64(
            commitments.allocation_count,
            "released allocation count overflow",
        )?,
        released_quantity: commitments.released_quantity,
        lines,
        amended_by: context.actor_id,
        amended_at,
    };
    enqueue_event_tx(&mut tx, access.tenant_id, &result).await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn require_stored_amendment_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored = sqlx::query(
        r#"SELECT (result_json->>'amendment_id')::bigint amendment_id,
                  (result_json->>'order_id')::bigint order_id,
                  (result_json->>'inventory_owner_id')::bigint inventory_owner_id
           FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3"#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(stored) = stored else {
        return Ok(());
    };
    let inventory_owner_id: i64 = stored.try_get("inventory_owner_id")?;
    if !scope.includes_inventory_owner(inventory_owner_id) {
        return Err(AppError::not_found("order line amendment"));
    }
    let visible: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM order_line_amendments amendment
             INNER JOIN orders order_header
               ON order_header.tenant_id=amendment.tenant_id
              AND order_header.inventory_owner_id=amendment.inventory_owner_id
              AND order_header.id=amendment.order_id
             WHERE amendment.tenant_id=$1 AND amendment.id=$2
               AND amendment.order_id=$3
               AND amendment.inventory_owner_id=$4
               AND order_header.deleted IS NULL)"#,
    )
    .bind(prepared.tenant_id().get())
    .bind(stored.try_get::<i64, _>("amendment_id")?)
    .bind(stored.try_get::<i64, _>("order_id")?)
    .bind(inventory_owner_id)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("order line amendment"))
    }
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
    scope: &ScopeBindings,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"SELECT inventory_owner_id, status, revision, wave_id
           FROM orders
           WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
             AND ($3 OR inventory_owner_id=ANY($4))
           FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    if row.try_get::<Option<i64>, _>("wave_id")?.is_some() {
        return Err(AppError::conflict(
            "orders assigned to an active pick wave cannot replace demand lines",
        ));
    }
    let released: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM order_releases WHERE tenant_id=$1 AND order_id=$2)",
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_one(&mut **tx)
    .await?;
    if released {
        return Err(AppError::conflict(
            "released orders cannot replace demand lines",
        ));
    }
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

async fn lock_current_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
) -> AppResult<Vec<CurrentLine>> {
    let rows = sqlx::query(
        r#"SELECT id, line_key, item_id, qty, uom
           FROM order_items
           WHERE tenant_id=$1 AND order_id=$2 AND deleted IS NULL
           ORDER BY line_number, id
           FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(CurrentLine {
                id: OrderLineId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                definition: FulfillmentOrderDemandLine::new(
                    OrderLineKey::new(row.try_get::<String, _>("line_key")?)
                        .map_err(|error| AppError::internal(error.to_string()))?,
                    CatalogItemId::new(row.try_get("item_id")?)
                        .map_err(|error| AppError::internal(error.to_string()))?,
                    OrderQuantity::new(row.try_get("qty")?)
                        .map_err(|error| AppError::internal(error.to_string()))?,
                    RequestedUom::new(row.try_get::<String, _>("uom")?)
                        .map_err(|error| AppError::internal(error.to_string()))?,
                ),
            })
        })
        .collect()
}

async fn lock_requested_catalog_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    command: &ReplaceFulfillmentOrderLinesCommand,
) -> AppResult<()> {
    let mut item_ids = command
        .lines()
        .iter()
        .map(|line| line.item_id())
        .collect::<Vec<_>>();
    item_ids.sort_unstable();
    item_ids.dedup();
    let rows = sqlx::query(
        r#"SELECT item.id, item.packaging_unit
           FROM inventory_owner_items owner_item
           INNER JOIN items item ON item.tenant_id=owner_item.tenant_id
                                AND item.id=owner_item.item_id
           WHERE owner_item.tenant_id=$1 AND owner_item.inventory_owner_id=$2
             AND owner_item.item_id=ANY($3) AND owner_item.deleted IS NULL
             AND item.deleted IS NULL
           ORDER BY item.id
           FOR SHARE OF owner_item, item"#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(&item_ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != item_ids.len() {
        return Err(AppError::conflict(
            "one or more replacement items are inactive or not linked to the client",
        ));
    }
    for line in command.lines() {
        let catalog_uom = rows
            .iter()
            .find(|row| row.try_get::<i64, _>("id").ok() == Some(line.item_id()))
            .map(|row| row.try_get::<String, _>("packaging_unit"))
            .transpose()?
            .ok_or_else(|| AppError::conflict("replacement item is not available"))?;
        if catalog_uom != line.requested_uom() {
            return Err(AppError::conflict(format!(
                "requested UOM for line {} does not match the active client item",
                line.line_key()
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_amendment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order: &LockedOrder,
    context: &CommandContext,
    command: &ReplaceFulfillmentOrderLinesCommand,
    resulting_revision: OrderRevision,
    previous_line_count: i64,
    previous_quantity: i64,
    resulting_line_count: i64,
    resulting_quantity: i64,
    released_reservation_count: i64,
    released_allocation_count: i64,
    released_quantity: i64,
    amended_at: Timestamp,
) -> AppResult<OrderLineAmendmentId> {
    let id: i64 = sqlx::query_scalar(
        r#"INSERT INTO order_line_amendments (
             tenant_id,inventory_owner_id,order_id,order_status,
             expected_order_revision,resulting_order_revision,
             previous_line_count,previous_qty,resulting_line_count,resulting_qty,
             released_reservation_count,released_allocation_count,released_qty,
             amended_by_user_id,amended_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
           RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(order.inventory_owner_id.get())
    .bind(command.order_id().get())
    .bind(order.status.as_str())
    .bind(command.expected_revision().get())
    .bind(resulting_revision.get())
    .bind(previous_line_count)
    .bind(previous_quantity)
    .bind(resulting_line_count)
    .bind(resulting_quantity)
    .bind(released_reservation_count)
    .bind(released_allocation_count)
    .bind(released_quantity)
    .bind(context.actor_id.get())
    .bind(amended_at)
    .fetch_one(&mut **tx)
    .await?;
    OrderLineAmendmentId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn set_amendment_context_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    amendment_id: OrderLineAmendmentId,
) -> AppResult<()> {
    sqlx::query("SELECT set_config('wareboxes.order_line_amendment_id',$1,true)")
        .bind(amendment_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn snapshot_previous_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: i64,
    amendment_id: OrderLineAmendmentId,
    lines: &[CurrentLine],
) -> AppResult<()> {
    for (index, line) in lines.iter().enumerate() {
        insert_line_snapshot_tx(
            tx,
            tenant_id,
            inventory_owner_id,
            order_id,
            amendment_id,
            "previous",
            line.id,
            count_i64(index + 1, "line number overflow")?,
            &line.definition,
        )
        .await?;
    }
    Ok(())
}

async fn retire_previous_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
    amended_at: Timestamp,
    expected_count: usize,
) -> AppResult<()> {
    let updated = sqlx::query(
        "UPDATE order_items SET deleted=$1 WHERE tenant_id=$2 AND order_id=$3 AND deleted IS NULL",
    )
    .bind(amended_at)
    .bind(tenant_id.get())
    .bind(order_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != u64::try_from(expected_count).unwrap_or(u64::MAX) {
        return Err(AppError::conflict(
            "order demand lines changed during replacement",
        ));
    }
    Ok(())
}

async fn insert_resulting_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: i64,
    amendment_id: OrderLineAmendmentId,
    command: &ReplaceFulfillmentOrderLinesCommand,
    amended_at: Timestamp,
) -> AppResult<Vec<ReplacedOrderLineReadModel>> {
    let mut result = Vec::with_capacity(command.lines().len());
    for (index, line) in command.lines().iter().enumerate() {
        let line_number = count_i64(index + 1, "line number overflow")?;
        let id: i64 = sqlx::query_scalar(
            r#"INSERT INTO order_items
               (tenant_id,inventory_owner_id,created,line_key,line_number,qty,item_id,order_id,uom)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(amended_at)
        .bind(line.line_key())
        .bind(line_number)
        .bind(line.quantity())
        .bind(line.item_id())
        .bind(order_id)
        .bind(line.requested_uom())
        .fetch_one(&mut **tx)
        .await?;
        let id = OrderLineId::new(id).map_err(|error| AppError::internal(error.to_string()))?;
        let definition = line
            .as_domain()
            .map_err(|error| AppError::internal(error.to_string()))?;
        insert_line_snapshot_tx(
            tx,
            tenant_id,
            inventory_owner_id,
            order_id,
            amendment_id,
            "resulting",
            id,
            line_number,
            &definition,
        )
        .await?;
        result.push(ReplacedOrderLineReadModel {
            order_line_id: id,
            line_key: line.line_key().to_owned(),
            line_number: u32::try_from(line_number)
                .map_err(|_| AppError::internal("line number overflow"))?,
            item_id: definition.item_id(),
            quantity: line.quantity(),
            requested_uom: line.requested_uom().to_owned(),
        });
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn insert_line_snapshot_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: i64,
    amendment_id: OrderLineAmendmentId,
    kind: &'static str,
    order_item_id: OrderLineId,
    line_number: i64,
    line: &FulfillmentOrderDemandLine,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO order_line_amendment_lines
           (tenant_id,inventory_owner_id,order_id,order_line_amendment_id,
            snapshot_kind,order_item_id,line_key,line_number,item_id,qty,uom)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id)
    .bind(amendment_id.get())
    .bind(kind)
    .bind(order_item_id.get())
    .bind(line.line_key().as_str())
    .bind(line_number)
    .bind(line.item_id().get())
    .bind(line.quantity().get())
    .bind(line.requested_uom().as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn update_order_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
    status: OrderStatus,
    expected_revision: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE orders SET revision=revision+1
           WHERE tenant_id=$1 AND id=$2 AND status=$3 AND revision=$4
             AND wave_id IS NULL AND deleted IS NULL"#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(status.as_str())
    .bind(expected_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed during line replacement"));
    }
    Ok(())
}

async fn require_replay_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &ReplaceFulfillmentOrderLinesResult,
    scope: &ScopeBindings,
) -> AppResult<()> {
    if !scope.includes_inventory_owner(result.inventory_owner_id.get()) {
        return Err(AppError::not_found("order line amendment"));
    }
    let visible: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM order_line_amendments amendment
             INNER JOIN orders order_header
               ON order_header.tenant_id=amendment.tenant_id
              AND order_header.inventory_owner_id=amendment.inventory_owner_id
              AND order_header.id=amendment.order_id
             WHERE amendment.tenant_id=$1 AND amendment.id=$2
               AND amendment.order_id=$3 AND order_header.deleted IS NULL)"#,
    )
    .bind(tenant_id.get())
    .bind(result.amendment_id.get())
    .bind(result.order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("order line amendment"))
    }
}

async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &ReplaceFulfillmentOrderLinesResult,
) -> AppResult<()> {
    let ordering_key = format!("order:{}", result.order_id.get());
    let sequence = orders::next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "order:{}:lines-replaced:{}",
        result.order_id.get(),
        result.revision.get()
    );
    let aggregate_id = result.order_id.to_string();
    let payload = serde_json::json!({
        "amendment_id": result.amendment_id.get(),
        "order_id": result.order_id.get(),
        "inventory_owner_id": result.inventory_owner_id.get(),
        "status": result.order_status.as_str(),
        "previous_revision": result.previous_revision.get(),
        "revision": result.revision.get(),
        "previous_line_count": result.previous_line_count,
        "previous_quantity": result.previous_quantity,
        "resulting_line_count": result.lines.len(),
        "resulting_quantity": result.resulting_quantity,
        "released_reservation_count": result.released_reservation_count,
        "released_allocation_count": result.released_allocation_count,
        "released_quantity": result.released_quantity,
        "lines": &result.lines,
        "amended_by": result.amended_by.get(),
        "amended_at": result.amended_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(result.inventory_owner_id),
            facility_id: None,
            actor_user_id: Some(result.amended_by.get()),
            event_key: &event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.order.lines_replaced",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.amended_at,
        },
    )
    .await?;
    Ok(())
}

fn map_transition_error(error: OrderLineAmendmentError) -> AppError {
    match error {
        OrderLineAmendmentError::InvalidOrderStatus => AppError::conflict(error.to_string()),
        OrderLineAmendmentError::MissingDemandLines
        | OrderLineAmendmentError::DuplicateLineKey { .. }
        | OrderLineAmendmentError::NoChanges => AppError::bad_request(error.to_string()),
        OrderLineAmendmentError::RevisionOverflow => AppError::internal(error.to_string()),
    }
}

fn sum_quantities(mut values: impl Iterator<Item = i64>) -> AppResult<i64> {
    values.try_fold(0_i64, |total, value| {
        total
            .checked_add(value)
            .ok_or_else(|| AppError::bad_request("order quantity overflow"))
    })
}

fn count_i64(value: usize, message: &'static str) -> AppResult<i64> {
    i64::try_from(value).map_err(|_| AppError::internal(message))
}
