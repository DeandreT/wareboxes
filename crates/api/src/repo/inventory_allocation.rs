//! Soft inventory demand reservations and concrete stock allocations.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_core::dto::{
    AllocateInventoryResult, CancelInventoryAllocationResult, CancelInventoryReservationResult,
    CreateInventoryReservationResult,
};
use wareboxes_core::models::{
    AllocationStatus, InventoryAllocation, InventoryReservation, InventoryStatus,
    ReservationStatus, TenantAccess, Timestamp,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::inventory_journal;
use crate::repo::inventory_locking::{balance_license_plate_hint, lock_license_plate};

fn parse_inventory_status(s: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {s}")))
}

fn parse_reservation_status(s: &str) -> AppResult<ReservationStatus> {
    ReservationStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid reservation status in database: {s}")))
}

fn parse_allocation_status(s: &str) -> AppResult<AllocationStatus> {
    AllocationStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid allocation status in database: {s}")))
}

fn map_reservation(row: &sqlx::postgres::PgRow) -> AppResult<InventoryReservation> {
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
        status: parse_reservation_status(row.try_get::<String, _>("status")?.as_str())?,
        allocated_qty: row.try_get("allocated_qty")?,
        allocations: Vec::new(),
    })
}

fn map_allocation(row: &sqlx::postgres::PgRow) -> AppResult<InventoryAllocation> {
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
        inventory_status: parse_inventory_status(
            row.try_get::<String, _>("inventory_status")?.as_str(),
        )?,
        qty: row.try_get("qty")?,
        status: parse_allocation_status(row.try_get::<String, _>("status")?.as_str())?,
    })
}

pub async fn get_reservations(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<InventoryReservation>> {
    get_reservations_with_scope(db, tenant_id, &ScopeBindings::unrestricted(), show_deleted).await
}

pub async fn get_reservations_in_scope(
    db: &Db,
    access: &TenantAccess,
    show_deleted: bool,
) -> AppResult<Vec<InventoryReservation>> {
    let scope = ScopeBindings::for_access(access);
    get_reservations_with_scope(db, access.tenant_id, &scope, show_deleted).await
}

async fn get_reservations_with_scope(
    db: &Db,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    show_deleted: bool,
) -> AppResult<Vec<InventoryReservation>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, modified, deleted, order_id,
               order_item_id, facility_id, item_id, uom, qty, status,
               COALESCE((
                   SELECT SUM(allocation.qty)
                   FROM inventory_allocations allocation
                   WHERE allocation.tenant_id = reservation.tenant_id
                     AND allocation.reservation_id = reservation.id
                     AND allocation.deleted IS NULL
                     AND allocation.status = 'allocated'
               ), 0)::BIGINT AS allocated_qty
        FROM inventory_reservations reservation
        WHERE tenant_id = $1
          AND ($2 OR deleted IS NULL)
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
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
              AND ($3 OR deleted IS NULL)
              AND ($4 OR facility_id = ANY($5))
              AND ($6 OR inventory_owner_id = ANY($7))
            ORDER BY reservation_id, id
            "#,
        )
        .bind(tenant_id.get())
        .bind(&reservation_ids)
        .bind(show_deleted)
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_all(&mut *tx)
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
    tx.commit().await?;
    Ok(reservations)
}

pub async fn get_allocations_in_scope(
    db: &Db,
    access: &TenantAccess,
    show_deleted: bool,
) -> AppResult<Vec<InventoryAllocation>> {
    let scope = ScopeBindings::for_access(access);
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, created, modified, deleted,
               reservation_id, inventory_balance_id, facility_id, location_id,
               license_plate_id, item_batch_id, item_id, uom, inventory_status,
               qty, status
        FROM inventory_allocations
        WHERE tenant_id = $1
          AND ($2 OR deleted IS NULL)
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        ORDER BY id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(show_deleted)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    let allocations = rows
        .iter()
        .map(map_allocation)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(allocations)
}

#[derive(Debug, Clone, Copy)]
pub struct CreateInventoryReservationCommand<'a> {
    pub order_id: i64,
    pub order_item_id: i64,
    pub facility_id: i64,
    pub qty: i64,
    pub idempotency_key: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct AllocateInventoryCommand<'a> {
    pub reservation_id: i64,
    pub inventory_balance_id: i64,
    pub qty: i64,
    pub idempotency_key: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct CancelInventoryAllocationCommand<'a> {
    pub allocation_id: i64,
    pub idempotency_key: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct CancelInventoryReservationCommand<'a> {
    pub reservation_id: i64,
    pub idempotency_key: &'a str,
}

#[derive(Debug)]
struct LockedReservation {
    inventory_owner_id: i64,
    order_id: i64,
    order_item_id: i64,
    facility_id: i64,
    item_id: i64,
    uom: String,
    qty: i64,
    status: String,
}

#[derive(Debug)]
struct LockedAllocation {
    id: i64,
    inventory_balance_id: i64,
    facility_id: i64,
    qty: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CancelledOrderCommitments {
    pub reservation_count: usize,
    pub allocation_count: usize,
    pub released_quantity: i64,
    pub affected_facility_ids: Vec<i64>,
}

#[derive(Debug, Clone, Copy)]
struct InventoryEventContext<'a> {
    tenant_id: TenantId,
    actor_user_id: i64,
    transition: &'a str,
    aggregate_sequence: i64,
    occurred_at: Timestamp,
}

fn require_command_scope(
    scope: &ScopeBindings,
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<()> {
    if !scope.includes_inventory_owner(inventory_owner_id) || !scope.includes_facility(facility_id)
    {
        return Err(AppError::forbidden());
    }
    Ok(())
}

async fn lock_reservation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    reservation_id: i64,
) -> AppResult<LockedReservation> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, order_id, order_item_id, facility_id,
               item_id, uom, qty, status
        FROM inventory_reservations
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(reservation_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory reservation"))?;

    Ok(LockedReservation {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        order_id: row.try_get("order_id")?,
        order_item_id: row.try_get("order_item_id")?,
        facility_id: row.try_get("facility_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        qty: row.try_get("qty")?,
        status: row.try_get("status")?,
    })
}

async fn enqueue_reservation_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reservation_id: i64,
    reservation: &LockedReservation,
    event: InventoryEventContext<'_>,
    payload: &serde_json::Value,
) -> AppResult<()> {
    let event_key = format!(
        "inventory-reservation:{reservation_id}:{}",
        event.transition
    );
    let aggregate_id = reservation_id.to_string();
    let ordering_key = format!("inventory-reservation:{reservation_id}");
    let event_type = format!("inventory.reservation.{}", event.transition);
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: Some(
                InventoryOwnerId::new(reservation.inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                FacilityId::new(reservation.facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(event.actor_user_id),
            event_key: &event_key,
            aggregate_type: "inventory_reservation",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: event.aggregate_sequence,
            event_type: &event_type,
            schema_version: 1,
            payload,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}

async fn enqueue_allocation_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    inventory_owner_id: i64,
    allocation: &LockedAllocation,
    event: InventoryEventContext<'_>,
    payload: &serde_json::Value,
) -> AppResult<()> {
    let event_key = format!(
        "inventory-allocation:{}:{}",
        allocation.id, event.transition
    );
    let aggregate_id = allocation.id.to_string();
    let ordering_key = format!("inventory-allocation:{}", allocation.id);
    let event_type = format!("inventory.allocation.{}", event.transition);
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: Some(
                InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                FacilityId::new(allocation.facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(event.actor_user_id),
            event_key: &event_key,
            aggregate_type: "inventory_allocation",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: event.aggregate_sequence,
            event_type: &event_type,
            schema_version: 1,
            payload,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}

pub(crate) async fn cancel_order_commitments_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    order_id: i64,
    scope: &ScopeBindings,
) -> AppResult<CancelledOrderCommitments> {
    let reservation_rows = sqlx::query(
        r#"
        SELECT inventory_owner_id, order_id, order_item_id, facility_id,
               item_id, uom, qty, status, id
        FROM inventory_reservations
        WHERE tenant_id = $1
          AND order_id = $2
          AND deleted IS NULL
          AND status = 'active'
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut reservations = Vec::with_capacity(reservation_rows.len());
    for row in &reservation_rows {
        let reservation_id: i64 = row.try_get("id")?;
        let reservation = LockedReservation {
            inventory_owner_id: row.try_get("inventory_owner_id")?,
            order_id: row.try_get("order_id")?,
            order_item_id: row.try_get("order_item_id")?,
            facility_id: row.try_get("facility_id")?,
            item_id: row.try_get("item_id")?,
            uom: row.try_get("uom")?,
            qty: row.try_get("qty")?,
            status: row.try_get("status")?,
        };
        require_command_scope(
            scope,
            reservation.inventory_owner_id,
            reservation.facility_id,
        )?;
        reservations.push((reservation_id, reservation));
    }

    let reservation_ids = reservations
        .iter()
        .map(|(reservation_id, _)| *reservation_id)
        .collect::<Vec<_>>();
    let reservation_indexes = reservations
        .iter()
        .enumerate()
        .map(|(index, (reservation_id, _))| (*reservation_id, index))
        .collect::<HashMap<_, _>>();
    let allocation_rows = if reservation_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query(
            r#"
            SELECT allocation.id, allocation.reservation_id,
                   allocation.inventory_balance_id, allocation.facility_id,
                   allocation.qty
            FROM inventory_allocations allocation
            WHERE allocation.tenant_id = $1
              AND allocation.reservation_id = ANY($2)
              AND allocation.deleted IS NULL
              AND allocation.status = 'allocated'
            ORDER BY allocation.id
            FOR UPDATE
            "#,
        )
        .bind(tenant_id.get())
        .bind(&reservation_ids)
        .fetch_all(&mut **tx)
        .await?
    };
    let mut allocations = Vec::with_capacity(allocation_rows.len());
    for row in &allocation_rows {
        allocations.push((
            row.try_get::<i64, _>("reservation_id")?,
            LockedAllocation {
                id: row.try_get("id")?,
                inventory_balance_id: row.try_get("inventory_balance_id")?,
                facility_id: row.try_get("facility_id")?,
                qty: row.try_get("qty")?,
            },
        ));
    }

    let mut balance_ids = allocations
        .iter()
        .map(|(_, allocation)| allocation.inventory_balance_id)
        .collect::<Vec<_>>();
    balance_ids.sort_unstable();
    balance_ids.dedup();
    if !balance_ids.is_empty() {
        sqlx::query(
            r#"
            SELECT id
            FROM inventory_balances
            WHERE tenant_id = $1 AND id = ANY($2)
            ORDER BY id
            FOR UPDATE
            "#,
        )
        .bind(tenant_id.get())
        .bind(&balance_ids)
        .fetch_all(&mut **tx)
        .await?;
    }

    let now = now_iso();
    let mut released_quantity = 0_i64;
    let mut released_by_reservation = HashMap::<i64, i64>::new();
    for (reservation_id, allocation) in &allocations {
        let reservation = reservation_indexes
            .get(reservation_id)
            .and_then(|index| reservations.get(*index))
            .map(|(_, reservation)| reservation)
            .ok_or_else(|| AppError::internal("allocation reservation lock was not retained"))?;
        let updated = sqlx::query(
            r#"
            UPDATE inventory_allocations
            SET modified = $1, deleted = $1, status = 'released'
            WHERE tenant_id = $2 AND id = $3 AND status = 'allocated'
            "#,
        )
        .bind(now)
        .bind(tenant_id.get())
        .bind(allocation.id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "order inventory allocations could not be released",
            ));
        }
        released_quantity = released_quantity
            .checked_add(allocation.qty)
            .ok_or_else(|| AppError::internal("released allocation quantity overflow"))?;
        let reservation_released = released_by_reservation.entry(*reservation_id).or_default();
        *reservation_released = reservation_released
            .checked_add(allocation.qty)
            .ok_or_else(|| AppError::internal("reservation release quantity overflow"))?;
        let payload = serde_json::json!({
            "allocation_id": allocation.id,
            "reservation_id": reservation_id,
            "inventory_balance_id": allocation.inventory_balance_id,
            "inventory_owner_id": reservation.inventory_owner_id,
            "facility_id": allocation.facility_id,
            "released_quantity": allocation.qty,
            "reason": "order_cancelled",
        });
        enqueue_allocation_event(
            tx,
            reservation.inventory_owner_id,
            allocation,
            InventoryEventContext {
                tenant_id,
                actor_user_id,
                transition: "released",
                aggregate_sequence: 2,
                occurred_at: now,
            },
            &payload,
        )
        .await?;
    }

    for (reservation_id, reservation) in &reservations {
        let released_quantity = released_by_reservation
            .get(reservation_id)
            .copied()
            .unwrap_or_default();
        let updated = sqlx::query(
            r#"
            UPDATE inventory_reservations
            SET modified = $1, deleted = $1, status = 'cancelled'
            WHERE tenant_id = $2 AND id = $3 AND status = 'active'
            "#,
        )
        .bind(now)
        .bind(tenant_id.get())
        .bind(reservation_id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "order inventory reservations could not be cancelled",
            ));
        }
        let payload = serde_json::json!({
            "reservation_id": reservation_id,
            "order_id": reservation.order_id,
            "order_item_id": reservation.order_item_id,
            "inventory_owner_id": reservation.inventory_owner_id,
            "facility_id": reservation.facility_id,
            "item_id": reservation.item_id,
            "uom": reservation.uom,
            "quantity": reservation.qty,
            "released_quantity": released_quantity,
            "reason": "order_cancelled",
        });
        enqueue_reservation_event(
            tx,
            *reservation_id,
            reservation,
            InventoryEventContext {
                tenant_id,
                actor_user_id,
                transition: "cancelled",
                aggregate_sequence: 2,
                occurred_at: now,
            },
            &payload,
        )
        .await?;
    }

    Ok(CancelledOrderCommitments {
        reservation_count: reservations.len(),
        allocation_count: allocations.len(),
        released_quantity,
        affected_facility_ids: {
            let mut facility_ids = reservations
                .iter()
                .map(|(_, reservation)| reservation.facility_id)
                .collect::<Vec<_>>();
            facility_ids.sort_unstable();
            facility_ids.dedup();
            facility_ids
        },
    })
}

pub async fn create_inventory_reservation(
    db: &Db,
    access: &TenantAccess,
    command: &CreateInventoryReservationCommand<'_>,
) -> AppResult<CreateInventoryReservationResult> {
    if command.qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }

    let tenant_id = access.tenant_id;
    let actor_user_id = access.user_id.get();
    let prepared = PreparedCommand::from_parts_v1(
        tenant_id,
        access.user_id,
        None,
        command.idempotency_key,
        "create_inventory_reservation",
        &(
            command.order_id,
            command.order_item_id,
            command.facility_id,
            command.qty,
        ),
    )?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, tenant_id, actor_user_id).await?;
    let order_line = sqlx::query(
        r#"
        SELECT orders.inventory_owner_id, orders.status AS order_status,
               orders.deleted AS order_deleted, order_item.item_id,
               order_item.qty AS ordered_qty, order_item.uom,
               order_item.deleted AS order_item_deleted
        FROM orders
        INNER JOIN order_items order_item
            ON order_item.tenant_id = orders.tenant_id
           AND order_item.inventory_owner_id = orders.inventory_owner_id
           AND order_item.order_id = orders.id
           AND order_item.id = $3
        WHERE orders.tenant_id = $1 AND orders.id = $2
        FOR UPDATE OF orders, order_item
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.order_id)
    .bind(command.order_item_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("order line"))?;
    let inventory_owner_id: i64 = order_line.try_get("inventory_owner_id")?;
    require_command_scope(&scope, inventory_owner_id, command.facility_id)?;

    if let Some(result) = prepared
        .replayed::<CreateInventoryReservationResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let order_deleted: Option<Timestamp> = order_line.try_get("order_deleted")?;
    let order_item_deleted: Option<Timestamp> = order_line.try_get("order_item_deleted")?;
    let order_status: String = order_line.try_get("order_status")?;
    if order_deleted.is_some()
        || order_item_deleted.is_some()
        || matches!(order_status.as_str(), "shipped" | "cancelled" | "void")
    {
        return Err(AppError::conflict(
            "inventory cannot be reserved for an inactive order line",
        ));
    }

    let owner_facility =
        inventory_journal::owner_facility_scope(inventory_owner_id, command.facility_id)?;
    inventory_journal::lock_active_owner_facility_tx(&mut tx, tenant_id, owner_facility).await?;

    let ordered_qty: i64 = order_line.try_get("ordered_qty")?;
    let reserved_qty: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(qty), 0)::BIGINT
        FROM inventory_reservations
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND order_item_id = $3
          AND deleted IS NULL
          AND status = 'active'
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(command.order_item_id)
    .fetch_one(&mut *tx)
    .await?;
    if reserved_qty + command.qty > ordered_qty {
        return Err(AppError::conflict(
            "reservation quantity exceeds remaining order line demand",
        ));
    }

    let item_id: i64 = order_line.try_get("item_id")?;
    let uom: String = order_line.try_get("uom")?;
    let reservation_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_reservations
            (tenant_id, inventory_owner_id, created, modified, created_by,
             order_id, order_item_id, facility_id, item_id, uom, qty, status)
        VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, 'active')
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(now)
    .bind(actor_user_id)
    .bind(command.order_id)
    .bind(command.order_item_id)
    .bind(command.facility_id)
    .bind(item_id)
    .bind(&uom)
    .bind(command.qty)
    .fetch_one(&mut *tx)
    .await?;
    let reservation = LockedReservation {
        inventory_owner_id,
        order_id: command.order_id,
        order_item_id: command.order_item_id,
        facility_id: command.facility_id,
        item_id,
        uom,
        qty: command.qty,
        status: "active".to_string(),
    };
    let payload = serde_json::json!({
        "reservation_id": reservation_id,
        "order_id": reservation.order_id,
        "order_item_id": reservation.order_item_id,
        "inventory_owner_id": reservation.inventory_owner_id,
        "facility_id": reservation.facility_id,
        "item_id": reservation.item_id,
        "uom": reservation.uom,
        "quantity": reservation.qty,
    });
    enqueue_reservation_event(
        &mut tx,
        reservation_id,
        &reservation,
        InventoryEventContext {
            tenant_id,
            actor_user_id,
            transition: "created",
            aggregate_sequence: 1,
            occurred_at: now,
        },
        &payload,
    )
    .await?;

    let result = CreateInventoryReservationResult { reservation_id };
    prepared.commit(tx, result).await.map_err(AppError::from)
}

pub async fn allocate_inventory(
    db: &Db,
    access: &TenantAccess,
    command: &AllocateInventoryCommand<'_>,
) -> AppResult<AllocateInventoryResult> {
    if command.qty <= 0 {
        return Err(AppError::bad_request("quantity must be positive"));
    }

    let tenant_id = access.tenant_id;
    let actor_user_id = access.user_id.get();
    let prepared = PreparedCommand::from_parts_v1(
        tenant_id,
        access.user_id,
        None,
        command.idempotency_key,
        "allocate_inventory",
        &(
            command.reservation_id,
            command.inventory_balance_id,
            command.qty,
        ),
    )?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, tenant_id, actor_user_id).await?;
    let reservation = lock_reservation_tx(&mut tx, tenant_id, command.reservation_id).await?;
    require_command_scope(
        &scope,
        reservation.inventory_owner_id,
        reservation.facility_id,
    )?;

    let license_plate_id =
        balance_license_plate_hint(&mut tx, tenant_id, command.inventory_balance_id).await?;
    lock_license_plate(&mut tx, tenant_id, license_plate_id).await?;
    let balance = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, location_id, license_plate_id,
               item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved,
               qty_held, deleted
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.inventory_balance_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory balance"))?;
    let balance_owner_id: i64 = balance.try_get("inventory_owner_id")?;
    let balance_facility_id: i64 = balance.try_get("facility_id")?;
    require_command_scope(&scope, balance_owner_id, balance_facility_id)?;

    if let Some(result) = prepared
        .replayed::<AllocateInventoryResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    if balance.try_get::<Option<i64>, _>("license_plate_id")? != license_plate_id {
        return Err(AppError::conflict(
            "inventory balance license plate changed while acquiring locks",
        ));
    }
    let balance_status: String = balance.try_get("status")?;
    let balance_deleted: Option<Timestamp> = balance.try_get("deleted")?;
    let balance_item_id: i64 = balance.try_get("item_id")?;
    let balance_uom: String = balance.try_get("uom")?;
    if reservation.status != "active" {
        return Err(AppError::conflict("reservation is not active"));
    }
    if balance_deleted.is_some()
        || balance_status != "available"
        || balance_owner_id != reservation.inventory_owner_id
        || balance_facility_id != reservation.facility_id
        || balance_item_id != reservation.item_id
        || balance_uom != reservation.uom
    {
        return Err(AppError::conflict(
            "inventory balance dimensions do not match the reservation",
        ));
    }
    let allocated_qty: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(qty), 0)::BIGINT
        FROM inventory_allocations
        WHERE tenant_id = $1
          AND reservation_id = $2
          AND deleted IS NULL
          AND status = 'allocated'
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.reservation_id)
    .fetch_one(&mut *tx)
    .await?;
    if allocated_qty + command.qty > reservation.qty {
        return Err(AppError::conflict(
            "allocation quantity exceeds remaining reservation demand",
        ));
    }
    let qty_on_hand: i64 = balance.try_get("qty_on_hand")?;
    let qty_reserved: i64 = balance.try_get("qty_reserved")?;
    let qty_held: i64 = balance.try_get("qty_held")?;
    if qty_on_hand - qty_reserved - qty_held < command.qty {
        return Err(AppError::conflict(
            "insufficient available inventory to allocate",
        ));
    }

    let location_id: i64 = balance.try_get("location_id")?;
    let item_batch_id: i64 = balance.try_get("item_batch_id")?;
    let allocation_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_allocations
            (tenant_id, inventory_owner_id, created, modified, created_by,
             reservation_id, inventory_balance_id, facility_id, location_id,
             license_plate_id, item_batch_id, item_id, uom, inventory_status,
             qty, status, execution_stage)
        VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                $14, 'allocated', 'pick_source')
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(reservation.inventory_owner_id)
    .bind(now)
    .bind(actor_user_id)
    .bind(command.reservation_id)
    .bind(command.inventory_balance_id)
    .bind(reservation.facility_id)
    .bind(location_id)
    .bind(license_plate_id)
    .bind(item_batch_id)
    .bind(reservation.item_id)
    .bind(&reservation.uom)
    .bind(&balance_status)
    .bind(command.qty)
    .fetch_one(&mut *tx)
    .await?;
    let allocation = LockedAllocation {
        id: allocation_id,
        inventory_balance_id: command.inventory_balance_id,
        facility_id: reservation.facility_id,
        qty: command.qty,
    };
    let payload = serde_json::json!({
        "allocation_id": allocation_id,
        "reservation_id": command.reservation_id,
        "inventory_balance_id": command.inventory_balance_id,
        "inventory_owner_id": reservation.inventory_owner_id,
        "facility_id": reservation.facility_id,
        "location_id": location_id,
        "license_plate_id": license_plate_id,
        "item_batch_id": item_batch_id,
        "item_id": reservation.item_id,
        "uom": reservation.uom,
        "inventory_status": balance_status,
        "quantity": command.qty,
    });
    enqueue_allocation_event(
        &mut tx,
        reservation.inventory_owner_id,
        &allocation,
        InventoryEventContext {
            tenant_id,
            actor_user_id,
            transition: "created",
            aggregate_sequence: 1,
            occurred_at: now,
        },
        &payload,
    )
    .await?;

    let result = AllocateInventoryResult { allocation_id };
    prepared.commit(tx, result).await.map_err(AppError::from)
}

pub async fn cancel_inventory_allocation(
    db: &Db,
    access: &TenantAccess,
    command: &CancelInventoryAllocationCommand<'_>,
) -> AppResult<CancelInventoryAllocationResult> {
    let tenant_id = access.tenant_id;
    let actor_user_id = access.user_id.get();
    let prepared = PreparedCommand::from_parts_v1(
        tenant_id,
        access.user_id,
        None,
        command.idempotency_key,
        "cancel_inventory_allocation",
        &command.allocation_id,
    )?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, tenant_id, actor_user_id).await?;
    let hint = sqlx::query(
        r#"
        SELECT reservation_id, inventory_balance_id
        FROM inventory_allocations
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.allocation_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory allocation"))?;
    let reservation_id: i64 = hint.try_get("reservation_id")?;
    let inventory_balance_id: i64 = hint.try_get("inventory_balance_id")?;
    let reservation = lock_reservation_tx(&mut tx, tenant_id, reservation_id).await?;
    require_command_scope(
        &scope,
        reservation.inventory_owner_id,
        reservation.facility_id,
    )?;
    sqlx::query("SELECT id FROM inventory_allocations WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
        .bind(tenant_id.get())
        .bind(command.allocation_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("SELECT id FROM inventory_balances WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
        .bind(tenant_id.get())
        .bind(inventory_balance_id)
        .fetch_one(&mut *tx)
        .await?;
    let allocation_row = sqlx::query(
        r#"
        SELECT id, inventory_balance_id, facility_id, qty, status
        FROM inventory_allocations
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.allocation_id)
    .fetch_one(&mut *tx)
    .await?;
    let allocation = LockedAllocation {
        id: allocation_row.try_get("id")?,
        inventory_balance_id: allocation_row.try_get("inventory_balance_id")?,
        facility_id: allocation_row.try_get("facility_id")?,
        qty: allocation_row.try_get("qty")?,
    };
    require_command_scope(
        &scope,
        reservation.inventory_owner_id,
        allocation.facility_id,
    )?;

    if let Some(result) = prepared
        .replayed::<CancelInventoryAllocationResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let status: String = allocation_row.try_get("status")?;
    if status != "allocated" {
        return Err(AppError::conflict("inventory allocation is not active"));
    }
    let updated = sqlx::query(
        r#"
        UPDATE inventory_allocations
        SET modified = $1, deleted = $1, status = 'released'
        WHERE tenant_id = $2 AND id = $3 AND status = 'allocated'
        "#,
    )
    .bind(now)
    .bind(tenant_id.get())
    .bind(command.allocation_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inventory allocation could not be cancelled",
        ));
    }

    let payload = serde_json::json!({
        "allocation_id": allocation.id,
        "reservation_id": reservation_id,
        "inventory_balance_id": allocation.inventory_balance_id,
        "inventory_owner_id": reservation.inventory_owner_id,
        "facility_id": allocation.facility_id,
        "released_quantity": allocation.qty,
    });
    enqueue_allocation_event(
        &mut tx,
        reservation.inventory_owner_id,
        &allocation,
        InventoryEventContext {
            tenant_id,
            actor_user_id,
            transition: "released",
            aggregate_sequence: 2,
            occurred_at: now,
        },
        &payload,
    )
    .await?;

    let result = CancelInventoryAllocationResult {
        allocation_id: allocation.id,
        released_qty: allocation.qty,
    };
    prepared.commit(tx, result).await.map_err(AppError::from)
}

pub async fn cancel_inventory_reservation(
    db: &Db,
    access: &TenantAccess,
    command: &CancelInventoryReservationCommand<'_>,
) -> AppResult<CancelInventoryReservationResult> {
    let tenant_id = access.tenant_id;
    let actor_user_id = access.user_id.get();
    let prepared = PreparedCommand::from_parts_v1(
        tenant_id,
        access.user_id,
        None,
        command.idempotency_key,
        "cancel_inventory_reservation",
        &command.reservation_id,
    )?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, tenant_id, actor_user_id).await?;
    let reservation = lock_reservation_tx(&mut tx, tenant_id, command.reservation_id).await?;
    require_command_scope(
        &scope,
        reservation.inventory_owner_id,
        reservation.facility_id,
    )?;

    if let Some(result) = prepared
        .replayed::<CancelInventoryReservationResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    if reservation.status != "active" {
        return Err(AppError::conflict("inventory reservation is not active"));
    }

    let allocation_rows = sqlx::query(
        r#"
        SELECT id, inventory_balance_id, facility_id, qty
        FROM inventory_allocations
        WHERE tenant_id = $1
          AND reservation_id = $2
          AND deleted IS NULL
          AND status = 'allocated'
        ORDER BY inventory_balance_id, id
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.reservation_id)
    .fetch_all(&mut *tx)
    .await?;
    let allocations = allocation_rows
        .iter()
        .map(|row| {
            Ok(LockedAllocation {
                id: row.try_get("id")?,
                inventory_balance_id: row.try_get("inventory_balance_id")?,
                facility_id: row.try_get("facility_id")?,
                qty: row.try_get("qty")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let mut balance_ids = allocations
        .iter()
        .map(|allocation| allocation.inventory_balance_id)
        .collect::<Vec<_>>();
    balance_ids.sort_unstable();
    balance_ids.dedup();
    if !balance_ids.is_empty() {
        sqlx::query(
            r#"
            SELECT id
            FROM inventory_balances
            WHERE tenant_id = $1 AND id = ANY($2)
            ORDER BY id
            FOR UPDATE
            "#,
        )
        .bind(tenant_id.get())
        .bind(&balance_ids)
        .fetch_all(&mut *tx)
        .await?;
    }

    let mut released_qty = 0_i64;
    for allocation in &allocations {
        require_command_scope(
            &scope,
            reservation.inventory_owner_id,
            allocation.facility_id,
        )?;
        let updated = sqlx::query(
            r#"
            UPDATE inventory_allocations
            SET modified = $1, deleted = $1, status = 'released'
            WHERE tenant_id = $2 AND id = $3 AND status = 'allocated'
            "#,
        )
        .bind(now)
        .bind(tenant_id.get())
        .bind(allocation.id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "inventory reservation allocations could not be released",
            ));
        }
        released_qty = released_qty
            .checked_add(allocation.qty)
            .ok_or_else(|| AppError::internal("released allocation quantity overflow"))?;
        let payload = serde_json::json!({
            "allocation_id": allocation.id,
            "reservation_id": command.reservation_id,
            "inventory_balance_id": allocation.inventory_balance_id,
            "inventory_owner_id": reservation.inventory_owner_id,
            "facility_id": allocation.facility_id,
            "released_quantity": allocation.qty,
            "reason": "reservation_cancelled",
        });
        enqueue_allocation_event(
            &mut tx,
            reservation.inventory_owner_id,
            allocation,
            InventoryEventContext {
                tenant_id,
                actor_user_id,
                transition: "released",
                aggregate_sequence: 2,
                occurred_at: now,
            },
            &payload,
        )
        .await?;
    }

    let updated = sqlx::query(
        r#"
        UPDATE inventory_reservations
        SET modified = $1, deleted = $1, status = 'cancelled'
        WHERE tenant_id = $2 AND id = $3 AND status = 'active'
        "#,
    )
    .bind(now)
    .bind(tenant_id.get())
    .bind(command.reservation_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inventory reservation could not be cancelled",
        ));
    }
    let payload = serde_json::json!({
        "reservation_id": command.reservation_id,
        "order_id": reservation.order_id,
        "order_item_id": reservation.order_item_id,
        "inventory_owner_id": reservation.inventory_owner_id,
        "facility_id": reservation.facility_id,
        "item_id": reservation.item_id,
        "uom": reservation.uom,
        "quantity": reservation.qty,
        "released_quantity": released_qty,
    });
    enqueue_reservation_event(
        &mut tx,
        command.reservation_id,
        &reservation,
        InventoryEventContext {
            tenant_id,
            actor_user_id,
            transition: "cancelled",
            aggregate_sequence: 2,
            occurred_at: now,
        },
        &payload,
    )
    .await?;

    let result = CancelInventoryReservationResult {
        reservation_id: command.reservation_id,
        released_qty,
    };
    prepared.commit(tx, result).await.map_err(AppError::from)
}
