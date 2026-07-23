//! Ported from `app/utils/orders.ts`. Update/delete keep the original status
//! guards: an order is only mutable while `cancelled|held|open|void`, and only
//! deletable while additionally not closed and not confirmed.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_core::dto::{NewOrder, OrderPage, OrderUpdate, Paged, SummaryCount};
use wareboxes_core::models::{
    InventoryReservation, Order, OrderActivity, OrderItem, OrderStatus, OrderTrackingNumber,
    ReservationStatus, TenantAccess,
};
use wareboxes_domain::{CommandContext, InventoryOwnerId, TenantId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::idempotency::{require_command_context, PreparedCommand};
use crate::repo::{address, tasks};

const MUTABLE: &str = "('cancelled', 'held', 'open', 'void')";

#[derive(Debug, Clone, Copy)]
struct OrderPageParameters<'a> {
    limit: i64,
    offset: i64,
    status: Option<OrderStatus>,
    search: Option<&'a str>,
}

fn map_order(row: &sqlx::postgres::PgRow) -> AppResult<Order> {
    let status: String = row.try_get("status")?;
    let status = OrderStatus::parse(&status)
        .ok_or_else(|| AppError::internal(format!("invalid order status in database: {status}")))?;
    Ok(Order {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_key: row.try_get("order_key")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        rush: row.try_get("rush")?,
        status,
        address_id: row.try_get("address_id")?,
        confirmed: row.try_get("confirmed")?,
        closed: row.try_get("closed")?,
        ship_by: row.try_get("ship_by")?,
        wave_id: row.try_get("wave_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        line1: row.try_get("line1")?,
        line2: row.try_get("line2")?,
        city: row.try_get("city")?,
        state: row.try_get("state")?,
        postal_code: row.try_get("postal_code")?,
        country: row.try_get("country")?,
        order_items: Vec::new(),
        tracking_numbers: Vec::new(),
        reservations: Vec::new(),
        activity: Vec::new(),
        ordered_qty: 0,
        reserved_qty: 0,
        out_of_stock: false,
    })
}

fn map_order_activity(row: &sqlx::postgres::PgRow) -> AppResult<OrderActivity> {
    Ok(OrderActivity {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        order_id: row.try_get("order_id")?,
        action: row.try_get("action")?,
    })
}

fn map_order_item(row: &sqlx::postgres::PgRow) -> AppResult<OrderItem> {
    Ok(OrderItem {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        qty: row.try_get("qty")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        order_id: row.try_get("order_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
    })
}

fn map_tracking_number(row: &sqlx::postgres::PgRow) -> AppResult<OrderTrackingNumber> {
    Ok(OrderTrackingNumber {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        order_id: row.try_get("order_id")?,
        tracking_number: row.try_get("tracking_number")?,
        carrier: row.try_get("carrier")?,
        service: row.try_get("service")?,
    })
}

fn map_reservation(row: &sqlx::postgres::PgRow) -> AppResult<InventoryReservation> {
    let status: String = row.try_get("status")?;
    let status = ReservationStatus::parse(&status).ok_or_else(|| {
        AppError::internal(format!("invalid reservation status in database: {status}"))
    })?;
    Ok(InventoryReservation {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        modified: row.try_get("modified")?,
        deleted: row.try_get("deleted")?,
        order_id: row.try_get("order_id")?,
        order_item_id: row.try_get("order_item_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        facility_id: row.try_get("facility_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        location_id: row.try_get("location_id")?,
        qty: row.try_get("qty")?,
        status,
    })
}

async fn items_by_order(db: &Db, tenant_id: TenantId) -> AppResult<HashMap<i64, Vec<OrderItem>>> {
    let rows = sqlx::query(
        r#"
        SELECT oi.id, oi.tenant_id, oi.inventory_owner_id, oi.created, oi.deleted,
               oi.qty, oi.item_id, i.description AS item_description, oi.order_id, oi.item_batch_id
        FROM order_items oi
        LEFT JOIN items i ON i.tenant_id = oi.tenant_id AND i.id = oi.item_id
        WHERE oi.tenant_id = $1 AND oi.deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<OrderItem>> = HashMap::new();
    for r in &rows {
        let oid = r.try_get("order_id")?;
        map.entry(oid).or_default().push(map_order_item(r)?);
    }
    Ok(map)
}

async fn items_by_order_ids(
    db: &Db,
    tenant_id: TenantId,
    order_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<OrderItem>>> {
    if order_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT oi.id, oi.tenant_id, oi.inventory_owner_id, oi.created, oi.deleted,
               oi.qty, oi.item_id, i.description AS item_description, oi.order_id, oi.item_batch_id
        FROM order_items oi
        LEFT JOIN items i ON i.tenant_id = oi.tenant_id AND i.id = oi.item_id
        WHERE oi.tenant_id = $1 AND oi.deleted IS NULL AND oi.order_id = ANY($2)
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_ids)
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<OrderItem>> = HashMap::new();
    for r in &rows {
        let oid = r.try_get("order_id")?;
        map.entry(oid).or_default().push(map_order_item(r)?);
    }
    Ok(map)
}

async fn available_by_item(db: &Db, tenant_id: TenantId) -> AppResult<HashMap<(i64, i64), i64>> {
    available_by_item_in_scope(db, tenant_id, &ScopeBindings::unrestricted()).await
}

async fn available_by_item_in_scope(
    db: &Db,
    tenant_id: TenantId,
    scope: &ScopeBindings,
) -> AppResult<HashMap<(i64, i64), i64>> {
    let rows = sqlx::query(
        r#"
        SELECT inv.inventory_owner_id AS inventory_owner_id, inv.item_id AS item_id,
               COALESCE(SUM(GREATEST(inv.qty_on_hand - inv.qty_reserved, 0)), 0)::BIGINT AS available_qty
        FROM inventory_balances inv
        WHERE inv.tenant_id = $1
          AND inv.deleted IS NULL
          AND inv.status = 'available'
          AND ($2 OR inv.facility_id = ANY($3))
          AND ($4 OR inv.inventory_owner_id = ANY($5))
        GROUP BY inv.inventory_owner_id, inv.item_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(db)
    .await?;
    let mut map = HashMap::new();
    for r in &rows {
        map.insert(
            (r.try_get("inventory_owner_id")?, r.try_get("item_id")?),
            r.try_get("available_qty")?,
        );
    }
    Ok(map)
}

async fn reserved_by_order_item(
    db: &Db,
    tenant_id: TenantId,
) -> AppResult<HashMap<(i64, i64), i64>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT r.order_id AS order_id,
               ib.item_id AS item_id,
               COALESCE(SUM(r.qty), 0)::BIGINT AS reserved_qty
        FROM inventory_reservations r
        INNER JOIN item_batches ib
            ON ib.tenant_id = r.tenant_id
           AND ib.inventory_owner_id = r.inventory_owner_id
           AND ib.id = r.item_batch_id
        WHERE r.tenant_id = $1
          AND r.deleted IS NULL
          AND r.status = 'reserved'
        GROUP BY r.order_id, ib.item_id
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(&mut *tx)
    .await?;
    let mut map = HashMap::new();
    for r in &rows {
        map.insert(
            (r.try_get("order_id")?, r.try_get("item_id")?),
            r.try_get("reserved_qty")?,
        );
    }
    tx.commit().await?;
    Ok(map)
}

fn apply_order_stock_state(
    order: &mut Order,
    available: &HashMap<(i64, i64), i64>,
    reserved: &HashMap<(i64, i64), i64>,
) {
    order.ordered_qty = order.order_items.iter().map(|item| item.qty).sum();
    order.reserved_qty = reserved
        .iter()
        .filter_map(|((order_id, _), qty)| (*order_id == order.id).then_some(*qty))
        .sum();

    if matches!(
        order.status,
        OrderStatus::Shipped | OrderStatus::Cancelled | OrderStatus::Void
    ) {
        order.out_of_stock = false;
        return;
    }

    order.out_of_stock = order.order_items.iter().any(|item| {
        let already_reserved = reserved
            .get(&(order.id, item.item_id))
            .copied()
            .unwrap_or_default();
        let available_to_reserve = available
            .get(&(order.inventory_owner_id, item.item_id))
            .copied()
            .unwrap_or_default();
        already_reserved + available_to_reserve < item.qty
    });
}

async fn tracking_by_order(
    db: &Db,
    tenant_id: TenantId,
) -> AppResult<HashMap<i64, Vec<OrderTrackingNumber>>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, deleted, order_id,
               tracking_number, carrier, service
        FROM order_tracking_numbers
        WHERE tenant_id = $1 AND deleted IS NULL
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<OrderTrackingNumber>> = HashMap::new();
    for r in &rows {
        let oid = r.try_get("order_id")?;
        map.entry(oid).or_default().push(map_tracking_number(r)?);
    }
    Ok(map)
}

async fn tracking_by_order_ids(
    db: &Db,
    tenant_id: TenantId,
    order_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<OrderTrackingNumber>>> {
    if order_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, deleted, order_id,
               tracking_number, carrier, service
        FROM order_tracking_numbers
        WHERE tenant_id = $1 AND deleted IS NULL AND order_id = ANY($2)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_ids)
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<OrderTrackingNumber>> = HashMap::new();
    for r in &rows {
        let oid = r.try_get("order_id")?;
        map.entry(oid).or_default().push(map_tracking_number(r)?);
    }
    Ok(map)
}

async fn reserved_by_order_ids_in_scope(
    db: &Db,
    tenant_id: TenantId,
    order_ids: &[i64],
    scope: &ScopeBindings,
) -> AppResult<HashMap<(i64, i64), i64>> {
    if order_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT r.order_id AS order_id,
               ib.item_id AS item_id,
               COALESCE(SUM(r.qty), 0)::BIGINT AS reserved_qty
        FROM inventory_reservations r
        INNER JOIN item_batches ib
            ON ib.tenant_id = r.tenant_id
           AND ib.inventory_owner_id = r.inventory_owner_id
           AND ib.id = r.item_batch_id
        WHERE r.tenant_id = $1
          AND r.deleted IS NULL
          AND r.status = 'reserved'
          AND r.order_id = ANY($2)
          AND ($3 OR r.facility_id = ANY($4))
          AND ($5 OR r.inventory_owner_id = ANY($6))
        GROUP BY r.order_id, ib.item_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut map = HashMap::new();
    for r in &rows {
        map.insert(
            (r.try_get("order_id")?, r.try_get("item_id")?),
            r.try_get("reserved_qty")?,
        );
    }
    tx.commit().await?;
    Ok(map)
}

async fn reservations_for_order_in_scope(
    db: &Db,
    tenant_id: TenantId,
    order_id: i64,
    scope: &ScopeBindings,
) -> AppResult<Vec<InventoryReservation>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, modified, deleted,
               order_id, order_item_id, inventory_balance_id, facility_id,
               item_batch_id, location_id, qty, status
        FROM inventory_reservations
        WHERE tenant_id = $1
          AND deleted IS NULL
          AND order_id = $2
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    let reservations = rows
        .iter()
        .map(map_reservation)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(reservations)
}

async fn activity_for_order(
    db: &Db,
    tenant_id: TenantId,
    order_id: i64,
) -> AppResult<Vec<OrderActivity>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, deleted, order_id, action
        FROM order_activity
        WHERE tenant_id = $1
          AND deleted IS NULL
          AND order_id = $2
        ORDER BY created DESC, id DESC
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_all(&mut *tx)
    .await?;
    let activity = rows
        .iter()
        .map(map_order_activity)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(activity)
}

pub async fn get_orders(db: &Db, tenant_id: TenantId) -> AppResult<Vec<Order>> {
    let rows = sqlx::query(
        r#"
        SELECT o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.line1 AS line1, a.line2 AS line2, a.city AS city,
               a.state AS state, a.postal_code AS postal_code, a.country AS country
        FROM orders o
        LEFT JOIN addresses a ON a.tenant_id = o.tenant_id AND a.id = o.address_id
        INNER JOIN inventory_owners acct
            ON acct.tenant_id = o.tenant_id AND acct.id = o.inventory_owner_id
        WHERE o.tenant_id = $1 AND o.deleted IS NULL
        ORDER BY o.created DESC
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let mut items = items_by_order(db, tenant_id).await?;
    let mut tracking = tracking_by_order(db, tenant_id).await?;
    let available = available_by_item(db, tenant_id).await?;
    let reserved = reserved_by_order_item(db, tenant_id).await?;
    rows.iter()
        .map(|r| {
            let mut o = map_order(r)?;
            o.order_items = items.remove(&o.id).unwrap_or_default();
            o.tracking_numbers = tracking.remove(&o.id).unwrap_or_default();
            apply_order_stock_state(&mut o, &available, &reserved);
            Ok(o)
        })
        .collect()
}

pub async fn get_orders_page(
    db: &Db,
    tenant_id: TenantId,
    limit: i64,
    offset: i64,
    status: Option<OrderStatus>,
    search: Option<&str>,
) -> AppResult<OrderPage> {
    get_orders_page_with_scope(
        db,
        tenant_id,
        &ScopeBindings::unrestricted(),
        OrderPageParameters {
            limit,
            offset,
            status,
            search,
        },
    )
    .await
}

pub async fn get_orders_page_in_scope(
    db: &Db,
    access: &TenantAccess,
    limit: i64,
    offset: i64,
    status: Option<OrderStatus>,
    search: Option<&str>,
) -> AppResult<OrderPage> {
    let scope = ScopeBindings::for_access(access);
    get_orders_page_with_scope(
        db,
        access.tenant_id,
        &scope,
        OrderPageParameters {
            limit,
            offset,
            status,
            search,
        },
    )
    .await
}

async fn get_orders_page_with_scope(
    db: &Db,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    parameters: OrderPageParameters<'_>,
) -> AppResult<OrderPage> {
    let status_text = parameters.status.map(|status| status.as_str().to_owned());
    let search_pattern = parameters
        .search
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"));
    let summaries = order_summaries(
        db,
        tenant_id,
        scope,
        status_text.as_deref(),
        search_pattern.as_deref(),
    )
    .await?;
    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM orders o
        LEFT JOIN addresses a ON a.tenant_id = o.tenant_id AND a.id = o.address_id
        INNER JOIN inventory_owners acct
            ON acct.tenant_id = o.tenant_id AND acct.id = o.inventory_owner_id
        WHERE o.tenant_id = $1
          AND o.deleted IS NULL
          AND ($4 OR o.inventory_owner_id = ANY($5))
          AND ($2::TEXT IS NULL OR o.status = $2)
          AND (
              $3::TEXT IS NULL
              OR o.order_key ILIKE $3
              OR o.id::TEXT ILIKE $3
              OR a.city ILIKE $3
              OR a.state ILIKE $3
              OR a.postal_code ILIKE $3
              OR acct.name ILIKE $3
          )
        "#,
    )
    .bind(tenant_id.get())
    .bind(status_text.as_deref())
    .bind(search_pattern.as_deref())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(db)
    .await?;

    let rows = sqlx::query(
        r#"
        SELECT o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.line1 AS line1, a.line2 AS line2, a.city AS city,
               a.state AS state, a.postal_code AS postal_code, a.country AS country
        FROM orders o
        LEFT JOIN addresses a ON a.tenant_id = o.tenant_id AND a.id = o.address_id
        INNER JOIN inventory_owners acct
            ON acct.tenant_id = o.tenant_id AND acct.id = o.inventory_owner_id
        WHERE o.tenant_id = $1
          AND o.deleted IS NULL
          AND ($4 OR o.inventory_owner_id = ANY($5))
          AND ($2::TEXT IS NULL OR o.status = $2)
          AND (
              $3::TEXT IS NULL
              OR o.order_key ILIKE $3
              OR o.id::TEXT ILIKE $3
              OR a.city ILIKE $3
              OR a.state ILIKE $3
              OR a.postal_code ILIKE $3
              OR acct.name ILIKE $3
          )
        ORDER BY o.created DESC, o.id DESC
        LIMIT $6 OFFSET $7
        "#,
    )
    .bind(tenant_id.get())
    .bind(status_text.as_deref())
    .bind(search_pattern.as_deref())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(parameters.limit)
    .bind(parameters.offset)
    .fetch_all(db)
    .await?;

    let mut orders = rows.iter().map(map_order).collect::<AppResult<Vec<_>>>()?;
    let order_ids = orders.iter().map(|order| order.id).collect::<Vec<_>>();
    let mut items = items_by_order_ids(db, tenant_id, &order_ids).await?;
    let mut tracking = tracking_by_order_ids(db, tenant_id, &order_ids).await?;
    let available = available_by_item_in_scope(db, tenant_id, scope).await?;
    let reserved = reserved_by_order_ids_in_scope(db, tenant_id, &order_ids, scope).await?;
    for order in &mut orders {
        order.order_items = items.remove(&order.id).unwrap_or_default();
        order.tracking_numbers = tracking.remove(&order.id).unwrap_or_default();
        apply_order_stock_state(order, &available, &reserved);
    }
    Ok(OrderPage {
        page: Paged::new(orders, total, parameters.limit, parameters.offset),
        summaries,
    })
}

pub async fn get_order(db: &Db, tenant_id: TenantId, order_id: i64) -> AppResult<Option<Order>> {
    get_order_with_scope(db, tenant_id, order_id, &ScopeBindings::unrestricted()).await
}

pub async fn get_order_in_scope(
    db: &Db,
    access: &TenantAccess,
    order_id: i64,
) -> AppResult<Option<Order>> {
    let scope = ScopeBindings::for_access(access);
    get_order_with_scope(db, access.tenant_id, order_id, &scope).await
}

async fn get_order_with_scope(
    db: &Db,
    tenant_id: TenantId,
    order_id: i64,
    scope: &ScopeBindings,
) -> AppResult<Option<Order>> {
    let row = sqlx::query(
        r#"
        SELECT o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.line1 AS line1, a.line2 AS line2, a.city AS city,
               a.state AS state, a.postal_code AS postal_code, a.country AS country
        FROM orders o
        LEFT JOIN addresses a ON a.tenant_id = o.tenant_id AND a.id = o.address_id
        INNER JOIN inventory_owners acct
            ON acct.tenant_id = o.tenant_id AND acct.id = o.inventory_owner_id
        WHERE o.tenant_id = $1
          AND o.id = $2
          AND o.deleted IS NULL
          AND ($3 OR o.inventory_owner_id = ANY($4))
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(db)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let mut order = map_order(&row)?;
    let order_ids = [order.id];
    let mut items = items_by_order_ids(db, tenant_id, &order_ids).await?;
    let mut tracking = tracking_by_order_ids(db, tenant_id, &order_ids).await?;
    let available = available_by_item_in_scope(db, tenant_id, scope).await?;
    let reserved = reserved_by_order_ids_in_scope(db, tenant_id, &order_ids, scope).await?;

    order.order_items = items.remove(&order.id).unwrap_or_default();
    order.tracking_numbers = tracking.remove(&order.id).unwrap_or_default();
    order.reservations = reservations_for_order_in_scope(db, tenant_id, order.id, scope).await?;
    order.activity = activity_for_order(db, tenant_id, order.id).await?;
    apply_order_stock_state(&mut order, &available, &reserved);

    Ok(Some(order))
}

async fn order_summaries(
    db: &Db,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    status: Option<&str>,
    search: Option<&str>,
) -> AppResult<Vec<SummaryCount>> {
    let available = available_by_item_in_scope(db, tenant_id, scope).await?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT o.id AS order_id,
               o.inventory_owner_id AS inventory_owner_id,
               o.status AS status,
               oi.item_id AS item_id,
               oi.qty AS qty,
               COALESCE(res.reserved_qty, 0)::BIGINT AS reserved_qty
        FROM orders o
        LEFT JOIN addresses a ON a.tenant_id = o.tenant_id AND a.id = o.address_id
        INNER JOIN inventory_owners acct
            ON acct.tenant_id = o.tenant_id AND acct.id = o.inventory_owner_id
        LEFT JOIN order_items oi
            ON oi.tenant_id = o.tenant_id
           AND oi.inventory_owner_id = o.inventory_owner_id
           AND oi.order_id = o.id
           AND oi.deleted IS NULL
        LEFT JOIN (
            SELECT r.tenant_id AS tenant_id,
                   r.inventory_owner_id AS inventory_owner_id,
                   r.order_id AS order_id,
                   ib.item_id AS item_id,
                   COALESCE(SUM(r.qty), 0)::BIGINT AS reserved_qty
            FROM inventory_reservations r
            INNER JOIN item_batches ib
                ON ib.tenant_id = r.tenant_id
               AND ib.inventory_owner_id = r.inventory_owner_id
               AND ib.id = r.item_batch_id
            WHERE r.deleted IS NULL
              AND r.status = 'reserved'
              AND ($4 OR r.facility_id = ANY($5))
              AND ($6 OR r.inventory_owner_id = ANY($7))
            GROUP BY r.tenant_id, r.inventory_owner_id, r.order_id, ib.item_id
        ) res ON res.tenant_id = o.tenant_id
             AND res.inventory_owner_id = o.inventory_owner_id
             AND res.order_id = o.id
             AND res.item_id = oi.item_id
        WHERE o.tenant_id = $1
          AND o.deleted IS NULL
          AND ($6 OR o.inventory_owner_id = ANY($7))
          AND o.status <> 'shipped'
          AND ($2::TEXT IS NULL OR o.status = $2)
          AND (
              $3::TEXT IS NULL
              OR o.order_key ILIKE $3
              OR o.id::TEXT ILIKE $3
              OR a.city ILIKE $3
              OR a.state ILIKE $3
              OR a.postal_code ILIKE $3
              OR acct.name ILIKE $3
          )
        ORDER BY o.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(status)
    .bind(search)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;

    #[derive(Default)]
    struct SummaryOrder {
        status: OrderStatus,
        out_of_stock: bool,
    }

    let mut orders: HashMap<i64, SummaryOrder> = HashMap::new();
    for row in &rows {
        let order_id: i64 = row.try_get("order_id")?;
        let status_text: String = row.try_get("status")?;
        let status = OrderStatus::parse(&status_text).ok_or_else(|| {
            AppError::internal(format!("invalid order status in database: {status_text}"))
        })?;
        let entry = orders.entry(order_id).or_insert(SummaryOrder {
            status,
            out_of_stock: false,
        });
        if matches!(status, OrderStatus::Open | OrderStatus::AwaitingShipment) {
            let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
            let item_id: Option<i64> = row.try_get("item_id")?;
            let qty: Option<i64> = row.try_get("qty")?;
            let reserved_qty: i64 = row.try_get("reserved_qty")?;
            if let (Some(item_id), Some(qty)) = (item_id, qty) {
                let available_to_reserve = available
                    .get(&(inventory_owner_id, item_id))
                    .copied()
                    .unwrap_or_default();
                if reserved_qty + available_to_reserve < qty {
                    entry.out_of_stock = true;
                }
            }
        }
    }

    let mut out_of_stock = 0_i64;
    let mut awaiting = 0_i64;
    let mut processing = 0_i64;
    let mut open = 0_i64;
    let mut held = 0_i64;
    let mut cancelled = 0_i64;
    let mut void = 0_i64;
    for order in orders.values() {
        if order.out_of_stock {
            out_of_stock += 1;
            continue;
        }
        match order.status {
            OrderStatus::AwaitingShipment => awaiting += 1,
            OrderStatus::Processing => processing += 1,
            OrderStatus::Open => open += 1,
            OrderStatus::Held => held += 1,
            OrderStatus::Cancelled => cancelled += 1,
            OrderStatus::Void => void += 1,
            OrderStatus::Shipped => {}
        }
    }

    let summaries = [
        ("out_of_stock", "Out of Stock", out_of_stock),
        ("processing", "Partial Pick", processing),
        ("held", "Held", held),
        ("awaiting shipment", "Awaiting Shipment", awaiting),
        ("open", "Open", open),
        ("cancelled", "Cancelled", cancelled),
        ("void", "Void", void),
    ]
    .into_iter()
    .filter(|&(_key, _label, count)| count > 0)
    .map(|(key, label, count)| SummaryCount {
        key: key.to_owned(),
        label: label.to_owned(),
        count,
    })
    .collect::<Vec<_>>();
    tx.commit().await?;
    Ok(summaries)
}

pub async fn orders_by_load(db: &Db, tenant_id: TenantId) -> AppResult<HashMap<i64, Vec<Order>>> {
    let rows = sqlx::query(
        r#"
        SELECT lo.load_id AS load_id,
               o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.line1 AS line1, a.line2 AS line2, a.city AS city,
               a.state AS state, a.postal_code AS postal_code, a.country AS country
        FROM load_orders lo
        INNER JOIN orders o
            ON o.tenant_id = lo.tenant_id
           AND o.inventory_owner_id = lo.inventory_owner_id
           AND o.id = lo.order_id
        LEFT JOIN addresses a ON a.tenant_id = o.tenant_id AND a.id = o.address_id
        INNER JOIN inventory_owners acct
            ON acct.tenant_id = o.tenant_id AND acct.id = o.inventory_owner_id
        WHERE lo.tenant_id = $1
          AND lo.deleted IS NULL
          AND o.deleted IS NULL
        ORDER BY lo.load_id, o.created DESC, o.id DESC
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let items = items_by_order(db, tenant_id).await?;
    let tracking = tracking_by_order(db, tenant_id).await?;
    let available = available_by_item(db, tenant_id).await?;
    let reserved = reserved_by_order_item(db, tenant_id).await?;
    let mut by_load: HashMap<i64, Vec<Order>> = HashMap::new();
    for r in &rows {
        let load_id = r.try_get("load_id")?;
        let mut order = map_order(r)?;
        order.order_items = items.get(&order.id).cloned().unwrap_or_default();
        order.tracking_numbers = tracking.get(&order.id).cloned().unwrap_or_default();
        apply_order_stock_state(&mut order, &available, &reserved);
        by_load.entry(load_id).or_default().push(order);
    }
    Ok(by_load)
}

pub async fn orders_for_load(db: &Db, tenant_id: TenantId, load_id: i64) -> AppResult<Vec<Order>> {
    let rows = sqlx::query(
        r#"
        SELECT o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.line1 AS line1, a.line2 AS line2, a.city AS city,
               a.state AS state, a.postal_code AS postal_code, a.country AS country
        FROM load_orders lo
        INNER JOIN orders o
            ON o.tenant_id = lo.tenant_id
           AND o.inventory_owner_id = lo.inventory_owner_id
           AND o.id = lo.order_id
        LEFT JOIN addresses a ON a.tenant_id = o.tenant_id AND a.id = o.address_id
        INNER JOIN inventory_owners acct
            ON acct.tenant_id = o.tenant_id AND acct.id = o.inventory_owner_id
        WHERE lo.tenant_id = $1
          AND lo.load_id = $2
          AND lo.deleted IS NULL
          AND o.deleted IS NULL
        ORDER BY o.created DESC, o.id DESC
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .fetch_all(db)
    .await?;
    let mut orders = rows.iter().map(map_order).collect::<AppResult<Vec<_>>>()?;
    if orders.is_empty() {
        return Ok(orders);
    }

    let order_ids = orders.iter().map(|order| order.id).collect::<Vec<_>>();
    let item_rows = sqlx::query(
        r#"
        SELECT oi.id, oi.tenant_id, oi.inventory_owner_id, oi.created, oi.deleted,
               oi.qty, oi.item_id, i.description AS item_description, oi.order_id, oi.item_batch_id
        FROM order_items oi
        LEFT JOIN items i ON i.tenant_id = oi.tenant_id AND i.id = oi.item_id
        WHERE oi.tenant_id = $1 AND oi.deleted IS NULL AND oi.order_id = ANY($2)
        "#,
    )
    .bind(tenant_id.get())
    .bind(&order_ids)
    .fetch_all(db)
    .await?;
    let mut items: HashMap<i64, Vec<OrderItem>> = HashMap::new();
    for r in &item_rows {
        let oid = r.try_get("order_id")?;
        items.entry(oid).or_default().push(map_order_item(r)?);
    }

    let tracking_rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, deleted, order_id,
               tracking_number, carrier, service
        FROM order_tracking_numbers
        WHERE tenant_id = $1 AND deleted IS NULL AND order_id = ANY($2)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(&order_ids)
    .fetch_all(db)
    .await?;
    let mut tracking: HashMap<i64, Vec<OrderTrackingNumber>> = HashMap::new();
    for r in &tracking_rows {
        let oid = r.try_get("order_id")?;
        tracking
            .entry(oid)
            .or_default()
            .push(map_tracking_number(r)?);
    }

    let available = available_by_item(db, tenant_id).await?;
    let mut reservation_tx = db.begin().await?;
    bind_tenant_context(&mut reservation_tx, tenant_id).await?;
    let reserved_rows = sqlx::query(
        r#"
        SELECT r.order_id AS order_id,
               ib.item_id AS item_id,
               COALESCE(SUM(r.qty), 0)::BIGINT AS reserved_qty
        FROM inventory_reservations r
        INNER JOIN item_batches ib
            ON ib.tenant_id = r.tenant_id
           AND ib.inventory_owner_id = r.inventory_owner_id
           AND ib.id = r.item_batch_id
        WHERE r.tenant_id = $1
          AND r.deleted IS NULL
          AND r.status = 'reserved'
          AND r.order_id = ANY($2)
        GROUP BY r.order_id, ib.item_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(&order_ids)
    .fetch_all(&mut *reservation_tx)
    .await?;
    reservation_tx.commit().await?;
    let mut reserved = HashMap::new();
    for r in &reserved_rows {
        reserved.insert(
            (r.try_get("order_id")?, r.try_get("item_id")?),
            r.try_get("reserved_qty")?,
        );
    }

    for order in &mut orders {
        order.order_items = items.remove(&order.id).unwrap_or_default();
        order.tracking_numbers = tracking.remove(&order.id).unwrap_or_default();
        apply_order_stock_state(order, &available, &reserved);
    }
    Ok(orders)
}

pub async fn add_order(db: &Db, tenant_id: TenantId, o: &NewOrder) -> AppResult<bool> {
    let inventory_owner_id = InventoryOwnerId::new(o.inventory_owner_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let mut tx = db.begin().await?;
    let address_id = address::insert_address_tx(
        &mut tx,
        tenant_id,
        o.line1.as_deref(),
        o.line2.as_deref(),
        Some(&o.city),
        Some(&o.state),
        Some(&o.postal_code),
        Some(&o.country),
    )
    .await?;

    let order_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO orders
            (tenant_id, inventory_owner_id, order_key, created, rush, status, address_id, ship_by)
        VALUES ($1, $2, $3, $4, $5, 'open', $6, $7)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(&o.order_key)
    .bind(now_iso())
    .bind(o.rush.unwrap_or(false))
    .bind(address_id)
    .bind(o.ship_by)
    .fetch_one(&mut *tx)
    .await?;
    insert_order_activity_tx(
        &mut tx,
        tenant_id,
        inventory_owner_id,
        order_id,
        "created order",
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

async fn insert_order_activity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: i64,
    action: &str,
) -> AppResult<i64> {
    bind_tenant_context(tx, tenant_id).await?;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO order_activity
            (tenant_id, inventory_owner_id, created, order_id, action)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(now_iso())
    .bind(order_id)
    .bind(action)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn update_order(db: &Db, tenant_id: TenantId, u: &OrderUpdate) -> AppResult<bool> {
    update_order_inner(db, tenant_id, u).await
}

async fn update_order_inner(db: &Db, tenant_id: TenantId, u: &OrderUpdate) -> AppResult<bool> {
    if matches!(u.status, Some(OrderStatus::Cancelled)) {
        return Err(AppError::bad_request(
            "cancel orders with the facility-qualified cancellation command",
        ));
    }
    let has_address = u.line1.is_some()
        || u.line2.is_some()
        || u.city.is_some()
        || u.state.is_some()
        || u.postal_code.is_some()
        || u.country.is_some();

    let mut tx = db.begin().await?;
    let new_address_id = if has_address {
        Some(
            address::insert_address_tx(
                &mut tx,
                tenant_id,
                u.line1.as_deref(),
                u.line2.as_deref(),
                u.city.as_deref(),
                u.state.as_deref(),
                u.postal_code.as_deref(),
                u.country.as_deref(),
            )
            .await?,
        )
    } else {
        None
    };

    let sql = format!(
        r#"
        UPDATE orders SET
            order_key = COALESCE($1, order_key),
            status = COALESCE($2, status),
            rush = COALESCE($3, rush),
            confirmed = COALESCE($4, confirmed),
            closed = COALESCE($5, closed),
            ship_by = COALESCE($6, ship_by),
            wave_id = COALESCE($7, wave_id),
            address_id = COALESCE($8, address_id)
        WHERE tenant_id = $9
          AND id = $10
          AND deleted IS NULL
          AND status IN {MUTABLE}
          AND status <> 'cancelled'
        RETURNING inventory_owner_id
        "#
    );
    let inventory_owner_id: Option<i64> = sqlx::query_scalar(&sql)
        .bind(u.order_key.as_deref())
        .bind(u.status.map(|s| s.as_str()))
        .bind(u.rush)
        .bind(u.confirmed)
        .bind(u.closed)
        .bind(u.ship_by)
        .bind(u.wave_id)
        .bind(new_address_id)
        .bind(tenant_id.get())
        .bind(u.order_id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(inventory_owner_id) = inventory_owner_id {
        let action = u
            .status
            .map(|status| format!("updated order status to {status}"))
            .unwrap_or_else(|| "updated order".to_owned());
        let inventory_owner_id = InventoryOwnerId::new(inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?;
        insert_order_activity_tx(&mut tx, tenant_id, inventory_owner_id, u.order_id, &action)
            .await?;
    }
    let changed = inventory_owner_id.is_some();
    tx.commit().await?;
    Ok(changed)
}

pub async fn cancel_order_with_unpack_task(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    order_id: i64,
    facility_id: i64,
) -> AppResult<Option<i64>> {
    require_command_context(access, command)?;
    let prepared = PreparedCommand::new(command, "order.cancel.v1", &(order_id, facility_id))?;
    let mut tx = db.begin().await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    if !scope.all_facilities && !scope.facility_ids.contains(&facility_id) {
        return Err(AppError::forbidden());
    }
    if let Some(task_id) = prepared.replayed::<i64>(&mut tx).await? {
        tasks::require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(Some(task_id));
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(access.tenant_id.get())
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    let order: Option<(i64, String)> = sqlx::query_as(
        r#"
        SELECT inventory_owner_id, status
        FROM orders
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
          AND status IN ('cancelled', 'held', 'open', 'void')
          AND ($3 OR inventory_owner_id = ANY($4))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order_id)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((inventory_owner_id, status)) = order else {
        tx.rollback().await?;
        return Ok(None);
    };
    let inventory_owner_id = InventoryOwnerId::new(inventory_owner_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    if status != "cancelled" {
        sqlx::query("UPDATE orders SET status = 'cancelled' WHERE tenant_id = $1 AND id = $2")
            .bind(access.tenant_id.get())
            .bind(order_id)
            .execute(&mut *tx)
            .await?;
        insert_order_activity_tx(
            &mut tx,
            access.tenant_id,
            inventory_owner_id,
            order_id,
            "cancelled order",
        )
        .await?;
    }
    let task_id = tasks::create_unpack_cancelled_order_task_tx(
        &mut tx,
        access.tenant_id,
        Some(command.actor_id.get()),
        order_id,
        facility_id,
        None,
        None,
        None,
        None,
        Some("Unpack inventory allocated to cancelled order".to_owned()),
        Some(&scope),
    )
    .await?;
    prepared.commit(tx, task_id).await.map(Some)
}

pub async fn delete_order(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let sql = format!(
        r#"
        UPDATE orders SET deleted = $1
        WHERE tenant_id = $2
          AND id = $3
          AND deleted IS NULL
          AND status IN {MUTABLE}
          AND closed IS NULL
          AND confirmed IS NULL
          AND NOT EXISTS (
              SELECT 1
              FROM unpack_cancelled_order_tasks unpack
              INNER JOIN work_tasks task
                  ON task.tenant_id = unpack.tenant_id
                 AND task.id = unpack.task_id
              WHERE unpack.tenant_id = orders.tenant_id
                AND unpack.order_id = orders.id
                AND task.deleted IS NULL
                AND task.status IN ('open', 'assigned', 'in_progress')
          )
        RETURNING inventory_owner_id
        "#
    );
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(tenant_id.get())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let inventory_owner_id: Option<i64> = sqlx::query_scalar(&sql)
        .bind(now_iso())
        .bind(tenant_id.get())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(inventory_owner_id) = inventory_owner_id {
        let inventory_owner_id = InventoryOwnerId::new(inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?;
        insert_order_activity_tx(&mut tx, tenant_id, inventory_owner_id, id, "deleted order")
            .await?;
    }
    tx.commit().await?;
    Ok(inventory_owner_id.is_some())
}

pub async fn restore_order(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let inventory_owner_id: Option<i64> = sqlx::query_scalar(
        r#"
        UPDATE orders
        SET deleted = NULL
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NOT NULL
        RETURNING inventory_owner_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(inventory_owner_id) = inventory_owner_id {
        let inventory_owner_id = InventoryOwnerId::new(inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?;
        insert_order_activity_tx(&mut tx, tenant_id, inventory_owner_id, id, "restored order")
            .await?;
    }
    tx.commit().await?;
    Ok(inventory_owner_id.is_some())
}
