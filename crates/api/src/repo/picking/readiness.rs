use sqlx::Row;
use wareboxes_domain::{InventoryOwnerId, OrderId, TenantId};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::repo) struct OrderPickReadiness {
    pub(in crate::repo) staged_allocation_count: i64,
    pub(in crate::repo) staged_quantity: i64,
    pub(in crate::repo) ordered_quantity: i64,
    pub(in crate::repo) accepted_short_quantity: i64,
    pub(in crate::repo) accepted_substitute_quantity: i64,
    pub(in crate::repo) effective_demand_quantity: i64,
    has_executable_work: bool,
    has_unresolved_shortage: bool,
    has_line_demand_mismatch: bool,
}

impl OrderPickReadiness {
    pub(in crate::repo) const fn is_ready_to_pack(self) -> bool {
        !self.has_executable_work
            && !self.has_unresolved_shortage
            && !self.has_line_demand_mismatch
            && self.effective_demand_quantity > 0
            && self.staged_quantity == self.effective_demand_quantity
    }
}

pub(in crate::repo) async fn order_pick_readiness_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<OrderPickReadiness> {
    let row = sqlx::query(
        r#"
        SELECT
            EXISTS (
                SELECT 1 FROM pick_tasks task
                WHERE task.tenant_id = $1 AND task.inventory_owner_id = $2
                  AND task.order_id = $3 AND task.status IN ('open', 'in_progress')
            ) AS has_executable_work,
            EXISTS (
                SELECT 1 FROM pick_shortages shortage
                WHERE shortage.tenant_id = $1 AND shortage.inventory_owner_id = $2
                  AND shortage.order_id = $3 AND shortage.status <> 'resolved'
            ) AS has_unresolved_shortage,
            EXISTS (
                SELECT 1
                FROM outbound_effective_demand line_demand
                WHERE line_demand.tenant_id = $1
                  AND line_demand.inventory_owner_id = $2
                  AND line_demand.order_id = $3
                  AND line_demand.effective_qty <> COALESCE((
                      SELECT SUM(allocation.qty)
                      FROM inventory_allocations allocation
                      INNER JOIN inventory_reservations reservation
                        ON reservation.tenant_id = allocation.tenant_id
                       AND reservation.inventory_owner_id = allocation.inventory_owner_id
                       AND reservation.id = allocation.reservation_id
                      WHERE allocation.tenant_id = line_demand.tenant_id
                        AND allocation.inventory_owner_id = line_demand.inventory_owner_id
                        AND reservation.order_id = line_demand.order_id
                        AND reservation.order_item_id = line_demand.order_item_id
                        AND reservation.status = 'active' AND reservation.deleted IS NULL
                        AND allocation.status = 'allocated'
                        AND allocation.execution_stage = 'staged'
                        AND allocation.deleted IS NULL
                  ), 0)
            ) AS has_line_demand_mismatch,
            demand.ordered_quantity,
            demand.accepted_short_quantity,
            demand.accepted_substitute_quantity,
            demand.effective_demand_quantity,
            COALESCE(staged.allocation_count, 0)::BIGINT AS staged_allocation_count,
            COALESCE(staged.quantity, 0)::BIGINT AS staged_quantity
        FROM (
            SELECT COALESCE(SUM(demand.original_qty), 0)::BIGINT AS ordered_quantity,
                   COALESCE(SUM(demand.accepted_short_qty), 0)::BIGINT
                       AS accepted_short_quantity,
                   COALESCE(SUM(demand.accepted_substitute_qty), 0)::BIGINT
                       AS accepted_substitute_quantity,
                   COALESCE(SUM(demand.effective_qty), 0)::BIGINT
                       AS effective_demand_quantity
            FROM outbound_effective_demand demand
            WHERE demand.tenant_id = $1 AND demand.inventory_owner_id = $2
              AND demand.order_id = $3
        ) demand
        CROSS JOIN (
            SELECT COUNT(*)::BIGINT AS allocation_count,
                   COALESCE(SUM(allocation.qty), 0)::BIGINT AS quantity
            FROM inventory_allocations allocation
            INNER JOIN inventory_reservations reservation
              ON reservation.tenant_id = allocation.tenant_id
             AND reservation.inventory_owner_id = allocation.inventory_owner_id
             AND reservation.id = allocation.reservation_id
             AND reservation.status = 'active' AND reservation.deleted IS NULL
            WHERE allocation.tenant_id = $1
              AND allocation.inventory_owner_id = $2
              AND reservation.order_id = $3
              AND allocation.status = 'allocated'
              AND allocation.execution_stage = 'staged'
              AND allocation.deleted IS NULL
        ) staged
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    let readiness = OrderPickReadiness {
        staged_allocation_count: row.try_get("staged_allocation_count")?,
        staged_quantity: row.try_get("staged_quantity")?,
        ordered_quantity: row.try_get("ordered_quantity")?,
        accepted_short_quantity: row.try_get("accepted_short_quantity")?,
        accepted_substitute_quantity: row.try_get("accepted_substitute_quantity")?,
        effective_demand_quantity: row.try_get("effective_demand_quantity")?,
        has_executable_work: row.try_get("has_executable_work")?,
        has_unresolved_shortage: row.try_get("has_unresolved_shortage")?,
        has_line_demand_mismatch: row.try_get("has_line_demand_mismatch")?,
    };
    if readiness.staged_allocation_count < 0
        || readiness.staged_quantity < 0
        || readiness.ordered_quantity <= 0
        || readiness.accepted_short_quantity < 0
        || readiness.accepted_substitute_quantity < 0
        || readiness.effective_demand_quantity < 0
        || readiness
            .accepted_short_quantity
            .checked_add(readiness.accepted_substitute_quantity)
            .and_then(|accepted| accepted.checked_add(readiness.effective_demand_quantity))
            != Some(readiness.ordered_quantity)
    {
        return Err(AppError::internal("order pick readiness is invalid"));
    }
    Ok(readiness)
}
