//! Atomic fulfillment-order creation and owner-scoped order-entry catalog queries.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{InventoryOwnerId, NewFulfillmentOrder, OrderStatus};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::access::{lock_current_scope_tx, require_permission_tx};
use super::{address, orders};
use crate::error::{AppError, AppResult};

const CREATE_OPERATION: &str = "order.create.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedOrderLineResult {
    pub order_line_id: i64,
    pub line_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateFulfillmentOrderResult {
    pub order_id: i64,
    pub order_key: String,
    pub status: OrderStatus,
    pub revision: i64,
    pub lines: Vec<CreatedOrderLineResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderEntryItem {
    pub item_id: i64,
    pub description: Option<String>,
    pub requested_uom: String,
}

pub async fn order_entry_items(
    db: &Db,
    access: &TenantAccess,
    inventory_owner_id: InventoryOwnerId,
    search: Option<&str>,
    limit: i64,
) -> AppResult<Option<Vec<OrderEntryItem>>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "orders").await?;
    if !scope.includes_inventory_owner(inventory_owner_id.get()) {
        return Ok(None);
    }
    let owner_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM inventory_owners
            WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !owner_exists {
        tx.commit().await?;
        return Ok(None);
    }
    let rows = sqlx::query(
        r#"
        SELECT item.id, item.description, item.packaging_unit
        FROM inventory_owner_items owner_item
        INNER JOIN items item
            ON item.tenant_id = owner_item.tenant_id
           AND item.id = owner_item.item_id
        WHERE owner_item.tenant_id = $1
          AND owner_item.inventory_owner_id = $2
          AND owner_item.deleted IS NULL
          AND item.deleted IS NULL
          AND (
              $3::text IS NULL
              OR item.id::text = $3
              OR lower(item.description) LIKE lower($3) || '%'
              OR EXISTS (
                  SELECT 1 FROM skus sku
                  WHERE sku.tenant_id = item.tenant_id
                    AND sku.item_id = item.id
                    AND sku.deleted IS NULL
                    AND lower(sku.name) LIKE lower($3) || '%'
              )
              OR EXISTS (
                  SELECT 1 FROM barcodes barcode
                  WHERE barcode.tenant_id = item.tenant_id
                    AND barcode.item_id = item.id
                    AND barcode.deleted IS NULL
                    AND lower(barcode.name) = lower($3)
              )
          )
        ORDER BY COALESCE(item.description, ''), item.id
        LIMIT $4
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(search)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let items = rows
        .iter()
        .map(|row| {
            Ok(OrderEntryItem {
                item_id: row.try_get("id")?,
                description: row.try_get("description")?,
                requested_uom: row.try_get("packaging_unit")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Some(items))
}

pub async fn create_fulfillment_order(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    order: &NewFulfillmentOrder,
) -> AppResult<CreateFulfillmentOrderResult> {
    command.require_actor(access.tenant_id, access.user_id)?;
    let request_lines = order
        .demand_lines()
        .iter()
        .map(|line| {
            (
                line.line_key().as_str(),
                line.item_id().get(),
                line.quantity().get(),
                line.requested_uom().as_str(),
            )
        })
        .collect::<Vec<_>>();
    let destination = order.destination();
    let request_identity = (
        order.inventory_owner_id().get(),
        order.order_key().as_str(),
        order.rush(),
        order.ship_by(),
        (
            destination.line1(),
            destination.line2(),
            destination.city(),
            destination.region(),
            destination.postal_code(),
            destination.country(),
        ),
        &request_lines,
    );
    let prepared = PreparedCommand::new_v1(command, CREATE_OPERATION, &request_identity)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, command.actor_id.get(), "orders").await?;
    if let Some(result) = prepared
        .replayed::<CreateFulfillmentOrderResult>(&mut tx)
        .await?
    {
        orders::require_replayed_order_visible_tx(
            &mut tx,
            access.tenant_id,
            result.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }
    if !scope.includes_inventory_owner(order.inventory_owner_id().get()) {
        return Err(AppError::forbidden());
    }

    lock_order_key(&mut tx, access, order).await?;
    lock_active_owner(&mut tx, access, order.inventory_owner_id()).await?;
    let catalog_uoms = lock_order_items(&mut tx, access, order).await?;
    for line in order.demand_lines() {
        let catalog_uom = catalog_uoms.get(&line.item_id().get()).ok_or_else(|| {
            AppError::conflict("one or more items are inactive or not linked to the client")
        })?;
        if catalog_uom != line.requested_uom().as_str() {
            return Err(AppError::conflict(format!(
                "requested UOM for line {} does not match the active client item",
                line.line_key()
            )));
        }
    }

    let occurred_at = now_iso();
    let address_id = address::insert_address_tx(
        &mut tx,
        access.tenant_id,
        Some(destination.line1()),
        destination.line2(),
        Some(destination.city()),
        Some(destination.region()),
        Some(destination.postal_code()),
        Some(destination.country()),
    )
    .await?;
    let (order_id, revision): (i64, i64) = sqlx::query_as(
        r#"
        INSERT INTO orders
            (tenant_id, inventory_owner_id, order_key, created, rush, status,
             address_id, ship_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, revision
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order.inventory_owner_id().get())
    .bind(order.order_key().as_str())
    .bind(occurred_at)
    .bind(order.rush())
    .bind(order.initial_status().as_str())
    .bind(address_id)
    .bind(order.ship_by())
    .fetch_one(&mut *tx)
    .await?;

    let mut created_lines = Vec::with_capacity(order.demand_lines().len());
    let mut ordered_quantity = 0_i64;
    for (index, line) in order.demand_lines().iter().enumerate() {
        let line_number = i64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| AppError::bad_request("order contains too many demand lines"))?;
        let order_line_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO order_items
                (tenant_id, inventory_owner_id, created, line_key, line_number,
                 qty, item_id, order_id, uom)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(order.inventory_owner_id().get())
        .bind(occurred_at)
        .bind(line.line_key().as_str())
        .bind(line_number)
        .bind(line.quantity().get())
        .bind(line.item_id().get())
        .bind(order_id)
        .bind(line.requested_uom().as_str())
        .fetch_one(&mut *tx)
        .await?;
        ordered_quantity = ordered_quantity
            .checked_add(line.quantity().get())
            .ok_or_else(|| AppError::bad_request("order quantity total exceeds i64"))?;
        created_lines.push(CreatedOrderLineResult {
            order_line_id,
            line_key: line.line_key().as_str().to_owned(),
        });
    }

    orders::insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id(),
        order_id,
        Some(command.actor_id.get()),
        "created fulfillment order",
    )
    .await?;
    enqueue_created_event(
        &mut tx,
        access,
        command,
        order,
        order_id,
        revision,
        ordered_quantity,
        occurred_at,
    )
    .await?;

    let result = CreateFulfillmentOrderResult {
        order_id,
        order_key: order.order_key().as_str().to_owned(),
        status: order.initial_status(),
        revision,
        lines: created_lines,
    };
    Ok(prepared.commit(tx, result).await?)
}

async fn lock_order_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    order: &NewFulfillmentOrder,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "order-key:{}:{}:{}",
            access.tenant_id,
            order.inventory_owner_id(),
            order.order_key()
        ))
        .execute(&mut **tx)
        .await?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM orders
            WHERE tenant_id = $1 AND inventory_owner_id = $2 AND order_key = $3
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order.inventory_owner_id().get())
    .bind(order.order_key().as_str())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(AppError::conflict("order key already exists for client"))
    } else {
        Ok(())
    }
}

async fn lock_active_owner(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    inventory_owner_id: InventoryOwnerId,
) -> AppResult<()> {
    let owner: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM inventory_owners
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    owner
        .map(|_| ())
        .ok_or_else(|| AppError::not_found("inventory owner"))
}

async fn lock_order_items(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    order: &NewFulfillmentOrder,
) -> AppResult<HashMap<i64, String>> {
    let mut item_ids = order
        .demand_lines()
        .iter()
        .map(|line| line.item_id().get())
        .collect::<Vec<_>>();
    item_ids.sort_unstable();
    item_ids.dedup();
    let rows = sqlx::query(
        r#"
        SELECT item.id, item.packaging_unit
        FROM items item
        INNER JOIN inventory_owner_items owner_item
            ON owner_item.tenant_id = item.tenant_id
           AND owner_item.inventory_owner_id = $2
           AND owner_item.item_id = item.id
        WHERE item.tenant_id = $1
          AND item.id = ANY($3)
          AND item.deleted IS NULL
          AND owner_item.deleted IS NULL
        ORDER BY item.id
        FOR SHARE OF item, owner_item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order.inventory_owner_id().get())
    .bind(&item_ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != item_ids.len() {
        return Err(AppError::conflict(
            "one or more items are inactive or not linked to the client",
        ));
    }
    rows.iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("packaging_unit")?)))
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_created_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CommandContext,
    order: &NewFulfillmentOrder,
    order_id: i64,
    revision: i64,
    ordered_quantity: i64,
    occurred_at: chrono::DateTime<chrono::Utc>,
) -> AppResult<()> {
    let event_key = format!("order:{order_id}:created");
    let aggregate_id = order_id.to_string();
    let ordering_key = format!("order:{order_id}");
    let lines = order
        .demand_lines()
        .iter()
        .enumerate()
        .map(|(index, line)| {
            serde_json::json!({
                "line_key": line.line_key().as_str(),
                "line_number": index + 1,
                "item_id": line.item_id().get(),
                "quantity": line.quantity().get(),
                "uom": line.requested_uom().as_str(),
            })
        })
        .collect::<Vec<_>>();
    let destination = order.destination();
    let payload = serde_json::json!({
        "order_id": order_id,
        "order_key": order.order_key().as_str(),
        "inventory_owner_id": order.inventory_owner_id().get(),
        "status": order.initial_status().as_str(),
        "revision": revision,
        "line_count": order.demand_lines().len(),
        "ordered_quantity": ordered_quantity,
        "ship_by": order.ship_by(),
        "destination": {
            "line1": destination.line1(),
            "line2": destination.line2(),
            "city": destination.city(),
            "region": destination.region(),
            "postal_code": destination.postal_code(),
            "country": destination.country(),
        },
        "lines": lines,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(order.inventory_owner_id()),
            facility_id: None,
            actor_user_id: Some(command.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: 1,
            event_type: "order.created",
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}
