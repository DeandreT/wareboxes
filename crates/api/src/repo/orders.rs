//! Order queries and commands. Metadata updates are limited to orders whose
//! workflow state is still mutable; workflow transitions use dedicated commands.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::dto::{OrderPage, Paged, SummaryCount};
use wareboxes_core::models::{
    AllocationStatus, InventoryAllocation, InventoryReservation, InventoryStatus, Order,
    OrderActivity, OrderHold, OrderHoldReason, OrderItem, OrderStatus, OrderTrackingNumber,
    ReservationStatus, TenantAccess,
};
use wareboxes_domain::{InventoryOwnerId, TenantId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

const MUTABLE: &str = "('cancelled', 'held', 'open', 'void')";

#[derive(Debug, Clone, Copy)]
struct OrderPageParameters<'a> {
    limit: i64,
    offset: i64,
    status: Option<OrderStatus>,
    search: Option<&'a str>,
    sort: OrderPageSort,
    direction: OrderPageSortDirection,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OrderPageSort {
    #[default]
    Created,
    Order,
    Client,
    Status,
    Units,
    ShipBy,
    Destination,
}

impl OrderPageSort {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Order => "order",
            Self::Client => "client",
            Self::Status => "status",
            Self::Units => "units",
            Self::ShipBy => "ship_by",
            Self::Destination => "destination",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OrderPageSortDirection {
    Ascending,
    #[default]
    Descending,
}

impl OrderPageSortDirection {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "asc",
            Self::Descending => "desc",
        }
    }
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
        revision: row.try_get("revision")?,
        confirmed: row.try_get("confirmed")?,
        closed: row.try_get("closed")?,
        ship_by: row.try_get("ship_by")?,
        wave_id: row.try_get("wave_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        recipient_name: row.try_get("recipient_name")?,
        destination_company: row.try_get("destination_company")?,
        destination_phone: row.try_get("destination_phone")?,
        destination_email: row.try_get("destination_email")?,
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
        holds: Vec::new(),
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
        actor_user_id: row.try_get("actor_user_id")?,
        action: row.try_get("action")?,
    })
}

fn map_order_hold(row: &sqlx::postgres::PgRow) -> AppResult<OrderHold> {
    let id: i64 = row.try_get("id")?;
    let reason_value: String = row.try_get("reason_code")?;
    let reason = OrderHoldReason::parse(&reason_value).ok_or_else(|| {
        AppError::internal(format!(
            "order hold {id} has unknown reason code {reason_value:?}"
        ))
    })?;
    Ok(OrderHold {
        id,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_id: row.try_get("order_id")?,
        created: row.try_get("created")?,
        created_by_user_id: row.try_get("created_by_user_id")?,
        reason,
        note: row.try_get("note")?,
        released_at: row.try_get("released_at")?,
        released_by_user_id: row.try_get("released_by_user_id")?,
        release_note: row.try_get("release_note")?,
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
        line_key: row.try_get("line_key")?,
        line_number: row.try_get("line_number")?,
        qty: row.try_get("qty")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        order_id: row.try_get("order_id")?,
        uom: row.try_get("uom")?,
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
        facility_id: row.try_get("facility_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        qty: row.try_get("qty")?,
        status,
        allocated_qty: row.try_get("allocated_qty")?,
        allocations: Vec::new(),
    })
}

fn map_allocation(row: &sqlx::postgres::PgRow) -> AppResult<InventoryAllocation> {
    let inventory_status: String = row.try_get("inventory_status")?;
    let inventory_status = InventoryStatus::parse(&inventory_status).ok_or_else(|| {
        AppError::internal(format!(
            "invalid inventory status in database: {inventory_status}"
        ))
    })?;
    let status: String = row.try_get("status")?;
    let status = AllocationStatus::parse(&status).ok_or_else(|| {
        AppError::internal(format!("invalid allocation status in database: {status}"))
    })?;
    Ok(InventoryAllocation {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        modified: row.try_get("modified")?,
        deleted: row.try_get("deleted")?,
        reservation_id: row.try_get("reservation_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        inventory_status,
        qty: row.try_get("qty")?,
        status,
    })
}

async fn items_by_order(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
) -> AppResult<HashMap<i64, Vec<OrderItem>>> {
    let rows = sqlx::query(
        r#"
        SELECT oi.id, oi.tenant_id, oi.inventory_owner_id, oi.created, oi.deleted,
               oi.line_key, oi.line_number, oi.qty, oi.item_id, i.description AS item_description,
               oi.order_id, oi.uom
        FROM order_items oi
        LEFT JOIN items i ON i.tenant_id = oi.tenant_id AND i.id = oi.item_id
        WHERE oi.tenant_id = $1 AND oi.deleted IS NULL
        ORDER BY oi.order_id, oi.line_number
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let mut map: HashMap<i64, Vec<OrderItem>> = HashMap::new();
    for r in &rows {
        let oid = r.try_get("order_id")?;
        map.entry(oid).or_default().push(map_order_item(r)?);
    }
    Ok(map)
}

async fn items_by_order_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<OrderItem>>> {
    if order_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT oi.id, oi.tenant_id, oi.inventory_owner_id, oi.created, oi.deleted,
               oi.line_key, oi.line_number, oi.qty, oi.item_id, i.description AS item_description,
               oi.order_id, oi.uom
        FROM order_items oi
        LEFT JOIN items i ON i.tenant_id = oi.tenant_id AND i.id = oi.item_id
        WHERE oi.tenant_id = $1 AND oi.deleted IS NULL AND oi.order_id = ANY($2)
        ORDER BY oi.order_id, oi.line_number
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut map: HashMap<i64, Vec<OrderItem>> = HashMap::new();
    for r in &rows {
        let oid = r.try_get("order_id")?;
        map.entry(oid).or_default().push(map_order_item(r)?);
    }
    Ok(map)
}

async fn available_by_item(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
) -> AppResult<HashMap<(i64, i64), i64>> {
    available_by_item_in_scope(tx, tenant_id, &ScopeBindings::unrestricted()).await
}

async fn available_by_item_in_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: &ScopeBindings,
) -> AppResult<HashMap<(i64, i64), i64>> {
    let rows = sqlx::query(
        r#"
        SELECT inv.inventory_owner_id AS inventory_owner_id, inv.item_id AS item_id,
               COALESCE(
                   SUM(
                       GREATEST(
                           inv.qty_on_hand - inv.qty_reserved - inv.qty_held,
                           0
                       )
                   ),
                   0
               )::BIGINT AS available_qty
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
    .fetch_all(&mut **tx)
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
) -> AppResult<HashMap<(i64, i64), i64>> {
    let rows = sqlx::query(
        r#"
        SELECT reservation.order_id AS order_id,
               allocation.item_id AS item_id,
               COALESCE(SUM(allocation.qty), 0)::BIGINT AS reserved_qty
        FROM inventory_allocations allocation
        INNER JOIN inventory_reservations reservation
            ON reservation.tenant_id = allocation.tenant_id
           AND reservation.inventory_owner_id = allocation.inventory_owner_id
           AND reservation.id = allocation.reservation_id
        WHERE allocation.tenant_id = $1
          AND allocation.deleted IS NULL
          AND allocation.status = 'allocated'
        GROUP BY reservation.order_id, allocation.item_id
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let mut map = HashMap::new();
    for r in &rows {
        map.insert(
            (r.try_get("order_id")?, r.try_get("item_id")?),
            r.try_get("reserved_qty")?,
        );
    }
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .fetch_all(&mut **tx)
    .await?;
    let mut map: HashMap<i64, Vec<OrderTrackingNumber>> = HashMap::new();
    for r in &rows {
        let oid = r.try_get("order_id")?;
        map.entry(oid).or_default().push(map_tracking_number(r)?);
    }
    Ok(map)
}

async fn tracking_by_order_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .fetch_all(&mut **tx)
    .await?;
    let mut map: HashMap<i64, Vec<OrderTrackingNumber>> = HashMap::new();
    for r in &rows {
        let oid = r.try_get("order_id")?;
        map.entry(oid).or_default().push(map_tracking_number(r)?);
    }
    Ok(map)
}

async fn reserved_by_order_ids_in_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_ids: &[i64],
    scope: &ScopeBindings,
) -> AppResult<HashMap<(i64, i64), i64>> {
    if order_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT reservation.order_id AS order_id,
               allocation.item_id AS item_id,
               COALESCE(SUM(allocation.qty), 0)::BIGINT AS reserved_qty
        FROM inventory_allocations allocation
        INNER JOIN inventory_reservations reservation
            ON reservation.tenant_id = allocation.tenant_id
           AND reservation.inventory_owner_id = allocation.inventory_owner_id
           AND reservation.id = allocation.reservation_id
        WHERE allocation.tenant_id = $1
          AND allocation.deleted IS NULL
          AND allocation.status = 'allocated'
          AND reservation.order_id = ANY($2)
          AND ($3 OR allocation.facility_id = ANY($4))
          AND ($5 OR allocation.inventory_owner_id = ANY($6))
        GROUP BY reservation.order_id, allocation.item_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut map = HashMap::new();
    for r in &rows {
        map.insert(
            (r.try_get("order_id")?, r.try_get("item_id")?),
            r.try_get("reserved_qty")?,
        );
    }
    Ok(map)
}

async fn reservations_for_order_in_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
    scope: &ScopeBindings,
) -> AppResult<Vec<InventoryReservation>> {
    let rows = sqlx::query(
        r#"
        SELECT reservation.id, reservation.tenant_id,
               reservation.inventory_owner_id, reservation.created,
               reservation.modified, reservation.deleted, reservation.order_id,
               reservation.order_item_id, reservation.facility_id,
               reservation.item_id, reservation.uom, reservation.qty,
               reservation.status,
               COALESCE(SUM(allocation.qty) FILTER (
                   WHERE allocation.deleted IS NULL
                     AND allocation.status = 'allocated'
               ), 0)::BIGINT AS allocated_qty
        FROM inventory_reservations reservation
        LEFT JOIN inventory_allocations allocation
            ON allocation.tenant_id = reservation.tenant_id
           AND allocation.inventory_owner_id = reservation.inventory_owner_id
           AND allocation.reservation_id = reservation.id
        WHERE reservation.tenant_id = $1
          AND reservation.deleted IS NULL
          AND reservation.order_id = $2
          AND ($3 OR reservation.facility_id = ANY($4))
          AND ($5 OR reservation.inventory_owner_id = ANY($6))
        GROUP BY reservation.id
        ORDER BY reservation.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut reservations = rows
        .iter()
        .map(map_reservation)
        .collect::<AppResult<Vec<_>>>()?;
    let reservation_ids = reservations
        .iter()
        .map(|reservation| reservation.id)
        .collect::<Vec<_>>();
    if !reservation_ids.is_empty() {
        let allocation_rows = sqlx::query(
            r#"
            SELECT id, tenant_id, inventory_owner_id, created, modified, deleted,
                   reservation_id, inventory_balance_id, facility_id, location_id,
                   license_plate_id, item_batch_id, item_id, uom,
                   inventory_status, qty, status
            FROM inventory_allocations
            WHERE tenant_id = $1
              AND reservation_id = ANY($2)
              AND deleted IS NULL
              AND ($3 OR facility_id = ANY($4))
              AND ($5 OR inventory_owner_id = ANY($6))
            ORDER BY reservation_id, id
            "#,
        )
        .bind(tenant_id.get())
        .bind(&reservation_ids)
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_all(&mut **tx)
        .await?;
        let mut allocations_by_reservation: HashMap<i64, Vec<InventoryAllocation>> = HashMap::new();
        for row in &allocation_rows {
            let allocation = map_allocation(row)?;
            allocations_by_reservation
                .entry(allocation.reservation_id)
                .or_default()
                .push(allocation);
        }
        for reservation in &mut reservations {
            reservation.allocations = allocations_by_reservation
                .remove(&reservation.id)
                .unwrap_or_default();
        }
    }
    Ok(reservations)
}

async fn activity_for_order(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
) -> AppResult<Vec<OrderActivity>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, deleted, order_id,
               actor_user_id, action
        FROM order_activity
        WHERE tenant_id = $1
          AND deleted IS NULL
          AND order_id = $2
        ORDER BY created DESC, id DESC
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    let activity = rows
        .iter()
        .map(map_order_activity)
        .collect::<AppResult<Vec<_>>>()?;
    Ok(activity)
}

async fn holds_for_order(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
) -> AppResult<Vec<OrderHold>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, order_id, created,
               created_by_user_id, reason_code, note, released_at,
               released_by_user_id, release_note
        FROM order_holds
        WHERE tenant_id = $1 AND order_id = $2
        ORDER BY created DESC, id DESC
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter().map(map_order_hold).collect()
}

pub async fn get_orders(db: &Db, tenant_id: TenantId) -> AppResult<Vec<Order>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.revision AS revision, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.name AS recipient_name, a.company AS destination_company,
               a.phone AS destination_phone, a.email AS destination_email,
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
    .fetch_all(&mut *tx)
    .await?;
    let mut items = items_by_order(&mut tx, tenant_id).await?;
    let mut tracking = tracking_by_order(&mut tx, tenant_id).await?;
    let available = available_by_item(&mut tx, tenant_id).await?;
    let reserved = reserved_by_order_item(&mut tx, tenant_id).await?;
    let orders = rows
        .iter()
        .map(|r| {
            let mut o = map_order(r)?;
            o.order_items = items.remove(&o.id).unwrap_or_default();
            o.tracking_numbers = tracking.remove(&o.id).unwrap_or_default();
            apply_order_stock_state(&mut o, &available, &reserved);
            Ok(o)
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(orders)
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
            sort: OrderPageSort::Created,
            direction: OrderPageSortDirection::Descending,
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
            sort: OrderPageSort::Created,
            direction: OrderPageSortDirection::Descending,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn get_orders_page_in_scope_sorted(
    db: &Db,
    access: &TenantAccess,
    limit: i64,
    offset: i64,
    status: Option<OrderStatus>,
    search: Option<&str>,
    sort: OrderPageSort,
    direction: OrderPageSortDirection,
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
            sort,
            direction,
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
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let status_text = parameters.status.map(|status| status.as_str().to_owned());
    let search_pattern = parameters
        .search
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"));
    let summaries = order_summaries(
        &mut tx,
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
    .fetch_one(&mut *tx)
    .await?;

    let rows = sqlx::query(
        r#"
        SELECT o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.revision AS revision, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.name AS recipient_name, a.company AS destination_company,
               a.phone AS destination_phone, a.email AS destination_email,
               a.line1 AS line1, a.line2 AS line2, a.city AS city,
               a.state AS state, a.postal_code AS postal_code, a.country AS country
        FROM orders o
        LEFT JOIN addresses a ON a.tenant_id = o.tenant_id AND a.id = o.address_id
        INNER JOIN inventory_owners acct
            ON acct.tenant_id = o.tenant_id AND acct.id = o.inventory_owner_id
        LEFT JOIN LATERAL (
            SELECT COALESCE(SUM(item.qty), 0)::BIGINT AS ordered_qty
            FROM order_items item
            WHERE item.tenant_id = o.tenant_id
              AND item.order_id = o.id
              AND item.deleted IS NULL
        ) quantity ON TRUE
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
        ORDER BY
          CASE WHEN $6 = 'created' AND $7 = 'asc' THEN o.created END ASC,
          CASE WHEN $6 = 'created' AND $7 = 'desc' THEN o.created END DESC,
          CASE WHEN $6 = 'order' AND $7 = 'asc' THEN LOWER(o.order_key) END ASC,
          CASE WHEN $6 = 'order' AND $7 = 'desc' THEN LOWER(o.order_key) END DESC,
          CASE WHEN $6 = 'client' AND $7 = 'asc' THEN LOWER(acct.name) END ASC,
          CASE WHEN $6 = 'client' AND $7 = 'desc' THEN LOWER(acct.name) END DESC,
          CASE WHEN $6 = 'status' AND $7 = 'asc' THEN o.status END ASC,
          CASE WHEN $6 = 'status' AND $7 = 'desc' THEN o.status END DESC,
          CASE WHEN $6 = 'units' AND $7 = 'asc' THEN quantity.ordered_qty END ASC,
          CASE WHEN $6 = 'units' AND $7 = 'desc' THEN quantity.ordered_qty END DESC,
          CASE WHEN $6 = 'ship_by' AND $7 = 'asc' THEN o.ship_by END ASC NULLS LAST,
          CASE WHEN $6 = 'ship_by' AND $7 = 'desc' THEN o.ship_by END DESC NULLS LAST,
          CASE WHEN $6 = 'destination' AND $7 = 'asc' THEN
            LOWER(CONCAT_WS(', ', NULLIF(a.city, ''), NULLIF(a.state, ''), NULLIF(a.postal_code, ''), NULLIF(a.country, '')))
          END ASC,
          CASE WHEN $6 = 'destination' AND $7 = 'desc' THEN
            LOWER(CONCAT_WS(', ', NULLIF(a.city, ''), NULLIF(a.state, ''), NULLIF(a.postal_code, ''), NULLIF(a.country, '')))
          END DESC,
          CASE WHEN $7 = 'asc' THEN o.id END ASC,
          CASE WHEN $7 = 'desc' THEN o.id END DESC
        LIMIT $8 OFFSET $9
        "#,
    )
    .bind(tenant_id.get())
    .bind(status_text.as_deref())
    .bind(search_pattern.as_deref())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(parameters.sort.as_str())
    .bind(parameters.direction.as_str())
    .bind(parameters.limit)
    .bind(parameters.offset)
    .fetch_all(&mut *tx)
    .await?;

    let mut orders = rows.iter().map(map_order).collect::<AppResult<Vec<_>>>()?;
    let order_ids = orders.iter().map(|order| order.id).collect::<Vec<_>>();
    let mut items = items_by_order_ids(&mut tx, tenant_id, &order_ids).await?;
    let mut tracking = tracking_by_order_ids(&mut tx, tenant_id, &order_ids).await?;
    let available = available_by_item_in_scope(&mut tx, tenant_id, scope).await?;
    let reserved = reserved_by_order_ids_in_scope(&mut tx, tenant_id, &order_ids, scope).await?;
    for order in &mut orders {
        order.order_items = items.remove(&order.id).unwrap_or_default();
        order.tracking_numbers = tracking.remove(&order.id).unwrap_or_default();
        apply_order_stock_state(order, &available, &reserved);
    }
    let page = OrderPage {
        page: Paged::new(orders, total, parameters.limit, parameters.offset),
        summaries,
    };
    tx.commit().await?;
    Ok(page)
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
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let row = sqlx::query(
        r#"
        SELECT o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.revision AS revision, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.name AS recipient_name, a.company AS destination_company,
               a.phone AS destination_phone, a.email AS destination_email,
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
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };

    let mut order = map_order(&row)?;
    let order_ids = [order.id];
    let mut items = items_by_order_ids(&mut tx, tenant_id, &order_ids).await?;
    let mut tracking = tracking_by_order_ids(&mut tx, tenant_id, &order_ids).await?;
    let available = available_by_item_in_scope(&mut tx, tenant_id, scope).await?;
    let reserved = reserved_by_order_ids_in_scope(&mut tx, tenant_id, &order_ids, scope).await?;

    order.order_items = items.remove(&order.id).unwrap_or_default();
    order.tracking_numbers = tracking.remove(&order.id).unwrap_or_default();
    order.reservations =
        reservations_for_order_in_scope(&mut tx, tenant_id, order.id, scope).await?;
    order.activity = activity_for_order(&mut tx, tenant_id, order.id).await?;
    order.holds = holds_for_order(&mut tx, tenant_id, order.id).await?;
    apply_order_stock_state(&mut order, &available, &reserved);

    tx.commit().await?;
    Ok(Some(order))
}

async fn order_summaries(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    status: Option<&str>,
    search: Option<&str>,
) -> AppResult<Vec<SummaryCount>> {
    let available = available_by_item_in_scope(tx, tenant_id, scope).await?;
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
            SELECT allocation.tenant_id AS tenant_id,
                   allocation.inventory_owner_id AS inventory_owner_id,
                   reservation.order_id AS order_id,
                   allocation.item_id AS item_id,
                   COALESCE(SUM(allocation.qty), 0)::BIGINT AS reserved_qty
            FROM inventory_allocations allocation
            INNER JOIN inventory_reservations reservation
                ON reservation.tenant_id = allocation.tenant_id
               AND reservation.inventory_owner_id =
                   allocation.inventory_owner_id
               AND reservation.id = allocation.reservation_id
            WHERE allocation.deleted IS NULL
              AND allocation.status = 'allocated'
              AND ($4 OR allocation.facility_id = ANY($5))
              AND ($6 OR allocation.inventory_owner_id = ANY($7))
            GROUP BY allocation.tenant_id, allocation.inventory_owner_id,
                     reservation.order_id, allocation.item_id
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
    .fetch_all(&mut **tx)
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
        if matches!(status, OrderStatus::Open) {
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
    let mut awaiting_packing = 0_i64;
    let mut packing = 0_i64;
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
            OrderStatus::AwaitingPacking => awaiting_packing += 1,
            OrderStatus::AwaitingShipment => awaiting += 1,
            OrderStatus::Packing => packing += 1,
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
        ("awaiting packing", "Awaiting Packing", awaiting_packing),
        ("packing", "Packing", packing),
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
    Ok(summaries)
}

pub async fn orders_by_load(db: &Db, tenant_id: TenantId) -> AppResult<HashMap<i64, Vec<Order>>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT lo.load_id AS load_id,
               o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.revision AS revision, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.name AS recipient_name, a.company AS destination_company,
               a.phone AS destination_phone, a.email AS destination_email,
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
    .fetch_all(&mut *tx)
    .await?;
    let items = items_by_order(&mut tx, tenant_id).await?;
    let tracking = tracking_by_order(&mut tx, tenant_id).await?;
    let available = available_by_item(&mut tx, tenant_id).await?;
    let reserved = reserved_by_order_item(&mut tx, tenant_id).await?;
    let mut by_load: HashMap<i64, Vec<Order>> = HashMap::new();
    for r in &rows {
        let load_id = r.try_get("load_id")?;
        let mut order = map_order(r)?;
        order.order_items = items.get(&order.id).cloned().unwrap_or_default();
        order.tracking_numbers = tracking.get(&order.id).cloned().unwrap_or_default();
        apply_order_stock_state(&mut order, &available, &reserved);
        by_load.entry(load_id).or_default().push(order);
    }
    tx.commit().await?;
    Ok(by_load)
}

pub async fn orders_for_load(db: &Db, tenant_id: TenantId, load_id: i64) -> AppResult<Vec<Order>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT o.id AS id, o.tenant_id AS tenant_id, o.order_key AS order_key, o.created AS created,
               o.deleted AS deleted, o.rush AS rush, o.status AS status,
               o.address_id AS address_id, o.revision AS revision, o.confirmed AS confirmed,
               o.closed AS closed, o.ship_by AS ship_by, o.wave_id AS wave_id,
               o.inventory_owner_id AS inventory_owner_id, acct.name AS inventory_owner_name,
               a.name AS recipient_name, a.company AS destination_company,
               a.phone AS destination_phone, a.email AS destination_email,
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
    .fetch_all(&mut *tx)
    .await?;
    let mut orders = rows.iter().map(map_order).collect::<AppResult<Vec<_>>>()?;
    if orders.is_empty() {
        tx.commit().await?;
        return Ok(orders);
    }

    let order_ids = orders.iter().map(|order| order.id).collect::<Vec<_>>();
    let mut items = items_by_order_ids(&mut tx, tenant_id, &order_ids).await?;
    let mut tracking = tracking_by_order_ids(&mut tx, tenant_id, &order_ids).await?;
    let available = available_by_item(&mut tx, tenant_id).await?;
    let reserved = reserved_by_order_ids_in_scope(
        &mut tx,
        tenant_id,
        &order_ids,
        &ScopeBindings::unrestricted(),
    )
    .await?;

    for order in &mut orders {
        order.order_items = items.remove(&order.id).unwrap_or_default();
        order.tracking_numbers = tracking.remove(&order.id).unwrap_or_default();
        apply_order_stock_state(order, &available, &reserved);
    }
    tx.commit().await?;
    Ok(orders)
}

pub(crate) async fn insert_order_activity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: i64,
    actor_user_id: Option<i64>,
    action: &str,
) -> AppResult<i64> {
    bind_tenant_context(tx, tenant_id).await?;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO order_activity
            (tenant_id, inventory_owner_id, created, order_id, actor_user_id, action)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(now_iso())
    .bind(order_id)
    .bind(actor_user_id)
    .bind(action)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

pub(crate) async fn require_replayed_order_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let inventory_owner_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT inventory_owner_id
        FROM orders
        WHERE tenant_id = $1 AND id = $2
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(inventory_owner_id) = inventory_owner_id else {
        return Err(AppError::not_found("order"));
    };
    if scope.all_inventory_owners || scope.inventory_owner_ids.contains(&inventory_owner_id) {
        Ok(())
    } else {
        Err(AppError::not_found("order"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderHoldCommandResult {
    pub order_id: i64,
    pub hold_id: i64,
    pub order_status: OrderStatus,
    pub active_hold_count: i64,
}

pub async fn place_order_hold(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    order_id: i64,
    reason: OrderHoldReason,
    note: Option<&str>,
) -> AppResult<OrderHoldCommandResult> {
    command.require_actor(access.tenant_id, access.user_id)?;
    validate_hold_note(note, reason == OrderHoldReason::Other)?;
    if order_id <= 0 {
        return Err(AppError::bad_request("order ID must be positive"));
    }
    let prepared =
        PreparedCommand::new_v1(command, "order.place_hold.v1", &(order_id, reason, note))?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    if let Some(result) = prepared.replayed::<OrderHoldCommandResult>(&mut tx).await? {
        require_replayed_order_visible_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, status
        FROM orders
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
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
    let Some(row) = row else {
        return Err(AppError::not_found("order"));
    };
    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let current_status_value: String = row.try_get("status")?;
    let current_status = OrderStatus::parse(&current_status_value).ok_or_else(|| {
        AppError::internal(format!(
            "order {order_id} has unknown status {current_status_value:?}"
        ))
    })?;
    let order_status = current_status
        .place_hold()
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let duplicate: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM order_holds
            WHERE tenant_id = $1
              AND inventory_owner_id = $2
              AND order_id = $3
              AND reason_code = $4
              AND released_at IS NULL
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id)
    .bind(reason.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict(format!(
            "order already has an active {} hold",
            reason.as_str()
        )));
    }

    let occurred_at = now_iso();
    let hold_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO order_holds
            (tenant_id, inventory_owner_id, order_id, created,
             created_by_user_id, reason_code, note)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id)
    .bind(occurred_at)
    .bind(command.actor_id.get())
    .bind(reason.as_str())
    .bind(note)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE orders SET status = $1, revision = revision + 1 WHERE tenant_id = $2 AND id = $3",
    )
    .bind(order_status.as_str())
    .bind(access.tenant_id.get())
    .bind(order_id)
    .execute(&mut *tx)
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        order_id,
        Some(command.actor_id.get()),
        "placed order hold",
    )
    .await?;
    let active_hold_count = active_order_hold_count_tx(&mut tx, access.tenant_id, order_id).await?;
    enqueue_order_hold_event(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        command.actor_id.get(),
        order_id,
        hold_id,
        "placed",
        occurred_at,
        serde_json::json!({
            "order_id": order_id,
            "order_hold_id": hold_id,
            "reason": reason.as_str(),
            "order_status": order_status.as_str(),
            "active_hold_count": active_hold_count,
        }),
    )
    .await?;
    let result = OrderHoldCommandResult {
        order_id,
        hold_id,
        order_status,
        active_hold_count,
    };
    Ok(prepared.commit(tx, result).await?)
}

pub async fn release_order_hold(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    order_id: i64,
    hold_id: i64,
    note: Option<&str>,
) -> AppResult<OrderHoldCommandResult> {
    command.require_actor(access.tenant_id, access.user_id)?;
    validate_hold_note(note, false)?;
    if order_id <= 0 || hold_id <= 0 {
        return Err(AppError::bad_request(
            "order ID and order hold ID must be positive",
        ));
    }
    let prepared =
        PreparedCommand::new_v1(command, "order.release_hold.v1", &(order_id, hold_id, note))?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    if let Some(result) = prepared.replayed::<OrderHoldCommandResult>(&mut tx).await? {
        require_replayed_order_visible_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT hold.inventory_owner_id, hold.released_at, orders.status
        FROM order_holds hold
        INNER JOIN orders
            ON orders.tenant_id = hold.tenant_id
           AND orders.inventory_owner_id = hold.inventory_owner_id
           AND orders.id = hold.order_id
        WHERE hold.tenant_id = $1
          AND hold.order_id = $2
          AND hold.id = $3
          AND orders.deleted IS NULL
          AND ($4 OR hold.inventory_owner_id = ANY($5))
        FOR UPDATE OF hold, orders
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order_id)
    .bind(hold_id)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(AppError::not_found("order hold"));
    };
    if row
        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("released_at")?
        .is_some()
    {
        return Err(AppError::conflict("order hold is already released"));
    }
    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let current_status_value: String = row.try_get("status")?;
    let current_status = OrderStatus::parse(&current_status_value).ok_or_else(|| {
        AppError::internal(format!(
            "order {order_id} has unknown status {current_status_value:?}"
        ))
    })?;
    let occurred_at = now_iso();
    sqlx::query(
        r#"
        UPDATE order_holds
        SET released_at = $1, released_by_user_id = $2, release_note = $3
        WHERE tenant_id = $4 AND order_id = $5 AND id = $6 AND released_at IS NULL
        "#,
    )
    .bind(occurred_at)
    .bind(command.actor_id.get())
    .bind(note)
    .bind(access.tenant_id.get())
    .bind(order_id)
    .bind(hold_id)
    .execute(&mut *tx)
    .await?;
    let active_hold_count = active_order_hold_count_tx(&mut tx, access.tenant_id, order_id).await?;
    let order_status = current_status
        .release_hold(active_hold_count > 0)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    sqlx::query(
        "UPDATE orders SET status = $1, revision = revision + 1 WHERE tenant_id = $2 AND id = $3",
    )
    .bind(order_status.as_str())
    .bind(access.tenant_id.get())
    .bind(order_id)
    .execute(&mut *tx)
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        order_id,
        Some(command.actor_id.get()),
        "released order hold",
    )
    .await?;
    enqueue_order_hold_event(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        command.actor_id.get(),
        order_id,
        hold_id,
        "released",
        occurred_at,
        serde_json::json!({
            "order_id": order_id,
            "order_hold_id": hold_id,
            "order_status": order_status.as_str(),
            "active_hold_count": active_hold_count,
        }),
    )
    .await?;
    let result = OrderHoldCommandResult {
        order_id,
        hold_id,
        order_status,
        active_hold_count,
    };
    Ok(prepared.commit(tx, result).await?)
}

fn validate_hold_note(note: Option<&str>, required: bool) -> AppResult<()> {
    if required && note.is_none() {
        return Err(AppError::bad_request(
            "note is required when the hold reason is other",
        ));
    }
    if let Some(note) = note {
        if note.trim() != note || note.is_empty() {
            return Err(AppError::bad_request(
                "order hold note must be trimmed and nonempty",
            ));
        }
        if note.chars().count() > 1_000 {
            return Err(AppError::bad_request(
                "order hold note cannot exceed 1000 characters",
            ));
        }
    }
    Ok(())
}

async fn active_order_hold_count_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM order_holds WHERE tenant_id = $1 AND order_id = $2 AND released_at IS NULL",
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_one(&mut **tx)
    .await?)
}

pub(crate) async fn next_outbox_sequence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    ordering_key: &str,
) -> AppResult<i64> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("outbox-sequence:{tenant_id}:{ordering_key}"))
        .execute(&mut **tx)
        .await?;
    Ok(sqlx::query_scalar(
        r#"
        SELECT COALESCE(
            (SELECT last_sequence
             FROM outbox_aggregate_sequences
             WHERE tenant_id = $1 AND ordering_key = $2),
            0
        ) + 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(ordering_key)
    .fetch_one(&mut **tx)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue_order_hold_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    actor_user_id: i64,
    order_id: i64,
    hold_id: i64,
    transition: &str,
    occurred_at: chrono::DateTime<chrono::Utc>,
    payload: serde_json::Value,
) -> AppResult<()> {
    let ordering_key = format!("order:{order_id}");
    let aggregate_sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!("order-hold:{hold_id}:{transition}");
    let aggregate_id = order_id.to_string();
    let event_type = format!("order.hold.{transition}");
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: None,
            actor_user_id: Some(actor_user_id),
            event_key: &event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

pub async fn delete_order(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let sql = format!(
        r#"
        UPDATE orders SET deleted = $1, revision = revision + 1
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
        insert_order_activity_tx(
            &mut tx,
            tenant_id,
            inventory_owner_id,
            id,
            None,
            "deleted order",
        )
        .await?;
    }
    tx.commit().await?;
    Ok(inventory_owner_id.is_some())
}

pub async fn restore_order(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let inventory_owner_id: Option<i64> = sqlx::query_scalar(
        r#"
        UPDATE orders
        SET deleted = NULL, revision = revision + 1
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
        insert_order_activity_tx(
            &mut tx,
            tenant_id,
            inventory_owner_id,
            id,
            None,
            "restored order",
        )
        .await?;
    }
    tx.commit().await?;
    Ok(inventory_owner_id.is_some())
}
