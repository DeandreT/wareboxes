use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::order_allocation::{
    OrderAllocationDetail, OrderAllocationFacilityReadModel, OrderAllocationLineState,
};
use wareboxes_domain::{
    AllocationQuantity, FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryOwnerId,
    InventoryReservationId, ItemBatchId, LicensePlateId, LocationId, OrderId, OrderLineId,
    TenantId,
};

use super::workflow::{cumulative_outcome, cumulative_shortage_reason};
use super::AllocationTotals;
use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;

pub(super) async fn eligible_facilities_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    scope: &ScopeBindings,
) -> AppResult<Vec<OrderAllocationFacilityReadModel>> {
    let rows = sqlx::query(
        r#"
        SELECT facility.id,
               COALESCE(NULLIF(BTRIM(facility.name), ''), 'Facility ' || facility.id::TEXT) AS name
        FROM inventory_owner_facilities assignment
        INNER JOIN inventory_owners owner
            ON owner.tenant_id = assignment.tenant_id
           AND owner.id = assignment.inventory_owner_id
           AND owner.deleted IS NULL
        INNER JOIN facilities facility
            ON facility.tenant_id = assignment.tenant_id
           AND facility.id = assignment.facility_id
           AND facility.deleted IS NULL
        WHERE assignment.tenant_id = $1
          AND assignment.inventory_owner_id = $2
          AND assignment.deleted IS NULL
          AND ($3 OR assignment.facility_id = ANY($4))
        ORDER BY LOWER(COALESCE(facility.name, '')), facility.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(OrderAllocationFacilityReadModel {
                facility_id: FacilityId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                facility_name: row.try_get("name")?,
            })
        })
        .collect()
}

pub(super) async fn load_line_states_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
    facility_id: FacilityId,
) -> AppResult<Vec<OrderAllocationLineState>> {
    let rows = sqlx::query(
        r#"
        SELECT line.id, line.line_key, line.item_id, item.description,
               line.uom, line.qty,
               reservation.id AS reservation_id, reservation.qty AS reserved_qty
        FROM order_items line
        INNER JOIN items item
            ON item.tenant_id = line.tenant_id AND item.id = line.item_id
        LEFT JOIN inventory_reservations reservation
            ON reservation.tenant_id = line.tenant_id
           AND reservation.inventory_owner_id = line.inventory_owner_id
           AND reservation.order_id = line.order_id
           AND reservation.order_item_id = line.id
           AND reservation.facility_id = $4
           AND reservation.deleted IS NULL
           AND reservation.status = 'active'
        WHERE line.tenant_id = $1
          AND line.inventory_owner_id = $2
          AND line.order_id = $3
          AND line.deleted IS NULL
        ORDER BY line.line_number, line.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .bind(facility_id.get())
    .fetch_all(&mut **tx)
    .await?;

    let reservation_ids = rows
        .iter()
        .filter_map(|row| row.try_get::<Option<i64>, _>("reservation_id").transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let allocations_by_reservation =
        load_allocation_details_tx(tx, tenant_id, &reservation_ids).await?;
    rows.iter()
        .map(|row| {
            let demand_quantity = AllocationQuantity::new(row.try_get("qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?;
            let reservation_id = row
                .try_get::<Option<i64>, _>("reservation_id")?
                .map(InventoryReservationId::new)
                .transpose()
                .map_err(|error| AppError::internal(error.to_string()))?;
            let reserved_quantity = row.try_get::<Option<i64>, _>("reserved_qty")?.unwrap_or(0);
            let allocations = reservation_id
                .and_then(|id| allocations_by_reservation.get(&id.get()).cloned())
                .unwrap_or_default();
            let allocated_quantity = allocations
                .iter()
                .try_fold(0_i64, |total, allocation| {
                    total.checked_add(allocation.quantity.get())
                })
                .ok_or_else(|| AppError::internal("line allocation quantity exceeds i64"))?;
            let shortage_quantity = demand_quantity
                .get()
                .checked_sub(allocated_quantity)
                .ok_or_else(|| {
                    AppError::internal("allocated quantity exceeds order-line demand")
                })?;
            let state = OrderAllocationLineState {
                order_line_id: OrderLineId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                line_key: row.try_get("line_key")?,
                item_id: row.try_get("item_id")?,
                item_description: row.try_get("description")?,
                uom: row.try_get("uom")?,
                demand_quantity,
                reservation_id,
                reserved_quantity,
                allocated_quantity,
                shortage_quantity,
                shortage_reason: cumulative_shortage_reason(allocated_quantity, shortage_quantity),
                allocations,
            };
            if !state.quantities_are_consistent() {
                return Err(AppError::internal(
                    "allocation line state does not conserve demand",
                ));
            }
            Ok(state)
        })
        .collect()
}

async fn load_allocation_details_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    reservation_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<OrderAllocationDetail>>> {
    if reservation_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT allocation.id, allocation.reservation_id,
               allocation.inventory_balance_id, allocation.item_batch_id,
               allocation.location_id, location.name AS location_name,
               location.barcode AS location_barcode,
               allocation.license_plate_id,
               plate.barcode AS license_plate_barcode,
               batch.lot, batch.serial, batch.expiration, allocation.qty
        FROM inventory_allocations allocation
        INNER JOIN item_batches batch
            ON batch.tenant_id = allocation.tenant_id
           AND batch.inventory_owner_id = allocation.inventory_owner_id
           AND batch.id = allocation.item_batch_id
        INNER JOIN locations location
            ON location.tenant_id = allocation.tenant_id
           AND location.facility_id = allocation.facility_id
           AND location.id = allocation.location_id
        LEFT JOIN license_plates plate
            ON plate.tenant_id = allocation.tenant_id
           AND plate.inventory_owner_id = allocation.inventory_owner_id
           AND plate.facility_id = allocation.facility_id
           AND plate.id = allocation.license_plate_id
        WHERE allocation.tenant_id = $1
          AND allocation.reservation_id = ANY($2)
          AND allocation.deleted IS NULL
          AND allocation.status = 'allocated'
        ORDER BY allocation.reservation_id, batch.expiration ASC NULLS LAST,
                 batch.created, allocation.inventory_balance_id, allocation.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(reservation_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut allocations_by_reservation = HashMap::new();
    for row in &rows {
        let reservation_id: i64 = row.try_get("reservation_id")?;
        let location_barcode = row.try_get("location_barcode")?;
        let quantity = AllocationQuantity::new(row.try_get("qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        allocations_by_reservation
            .entry(reservation_id)
            .or_insert_with(Vec::new)
            .push(OrderAllocationDetail {
                allocation_id: InventoryAllocationId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                reservation_id: InventoryReservationId::new(reservation_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                inventory_balance_id: InventoryBalanceId::new(row.try_get("inventory_balance_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                location_id: LocationId::new(row.try_get("location_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                location_name: row.try_get("location_name")?,
                location_barcode,
                license_plate_id: row
                    .try_get::<Option<i64>, _>("license_plate_id")?
                    .map(LicensePlateId::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
                license_plate_barcode: row.try_get("license_plate_barcode")?,
                lot: row.try_get("lot")?,
                serial: row.try_get("serial")?,
                expiration: row.try_get("expiration")?,
                quantity,
            });
    }
    Ok(allocations_by_reservation)
}

pub(super) fn line_state_totals(lines: &[OrderAllocationLineState]) -> AppResult<AllocationTotals> {
    let (demand_quantity, reserved_quantity, allocated_quantity) = lines
        .iter()
        .try_fold(
            (0_i64, 0_i64, 0_i64),
            |(demand, reserved, allocated), line| {
                Some((
                    demand.checked_add(line.demand_quantity.get())?,
                    reserved.checked_add(line.reserved_quantity)?,
                    allocated.checked_add(line.allocated_quantity)?,
                ))
            },
        )
        .ok_or_else(|| AppError::internal("allocation readiness totals exceed i64"))?;
    let shortage_quantity = demand_quantity
        .checked_sub(allocated_quantity)
        .ok_or_else(|| AppError::internal("allocated quantity exceeds order demand"))?;
    Ok(AllocationTotals {
        demand_quantity,
        reserved_quantity,
        allocated_quantity,
        shortage_quantity,
        outcome: cumulative_outcome(demand_quantity, allocated_quantity)?,
    })
}
