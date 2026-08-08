use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::picking::{
    PickShortageAllocationReadModel, PickShortageTaskReadModel, ReallocatePickShortageCommand,
    ReallocatePickShortageResult, REALLOCATE_PICK_SHORTAGE_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    plan_fefo_allocation, ActualPickQuantity, AllocationCandidate, AllocationExecutionStage,
    AllocationOutcome, AllocationQuantity, InventoryAllocationId, InventoryBalanceId,
    InventoryOwnerId, ItemBatchId, LicensePlateId, LocationId, OrderId, OrderRevision, OrderStatus,
    PickContentId, PickQuantity, PickScanValue, PickShortageId, PickShortageReallocationRunId,
    PickShortageRevision, PickShortageStatus, PickTaskId, PlannedAllocation, TenantId, Timestamp,
    UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::{insert_order_activity_tx, next_outbox_sequence_tx};

const PICK_LEASE_SECONDS: i64 = 30 * 60;

#[derive(Debug)]
struct LockedShortage {
    id: PickShortageId,
    revision: PickShortageRevision,
    status: PickShortageStatus,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    release_id: i64,
    order_id: OrderId,
    order_item_id: i64,
    reservation_id: i64,
    item_id: i64,
    uom: String,
    reallocated_quantity: i64,
    recovery_terminal_quantity: i64,
    remaining_quantity: i64,
    destination_location_id: i64,
    order_revision: OrderRevision,
    order_status: OrderStatus,
    rush: bool,
    ship_by: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy)]
struct CandidateHint {
    balance_id: InventoryBalanceId,
    batch_id: ItemBatchId,
    location_id: LocationId,
    plate_id: Option<LicensePlateId>,
}

#[derive(Debug, Clone)]
struct LockedCandidate {
    balance_id: InventoryBalanceId,
    batch_id: ItemBatchId,
    location_id: LocationId,
    location_name: Option<String>,
    location_barcode: String,
    plate_id: Option<LicensePlateId>,
    plate_barcode: Option<String>,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<Timestamp>,
    received_at: Timestamp,
    available_quantity: i64,
}

#[derive(Debug)]
struct CreatedRecoveryWork {
    allocation: PickShortageAllocationReadModel,
    task: PickShortageTaskReadModel,
}

pub async fn reallocate_shortage(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReallocatePickShortageCommand,
) -> AppResult<ReallocatePickShortageResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, REALLOCATE_PICK_SHORTAGE_OPERATION, command)?;
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

    require_stored_shortage_visible_before_replay_tx(
        &mut tx,
        access.tenant_id,
        prepared.idempotency_key(),
        &scope,
    )
    .await?;

    if let Some(result) = prepared
        .replayed::<ReallocatePickShortageResult>(&mut tx)
        .await?
    {
        require_replayed_run_visible_tx(
            &mut tx,
            access.tenant_id,
            result.reallocation_run_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id =
        shortage_order_hint_tx(&mut tx, access.tenant_id, command.shortage_id, &scope).await?;
    lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    let shortage = lock_shortage_tx(&mut tx, access.tenant_id, command.shortage_id, &scope).await?;
    if shortage.order_id != order_id {
        return Err(AppError::internal("pick shortage order identity changed"));
    }
    if shortage.revision != command.expected_shortage_revision {
        return Err(AppError::conflict(
            "pick shortage revision does not match expected revision",
        ));
    }
    if shortage.order_revision != command.expected_order_revision {
        return Err(AppError::conflict(
            "order revision does not match expected revision",
        ));
    }
    if shortage.status == PickShortageStatus::Resolved || shortage.remaining_quantity <= 0 {
        return Err(AppError::conflict("pick shortage is already resolved"));
    }
    if shortage.order_status != OrderStatus::Processing {
        return Err(AppError::conflict(
            "order is not in shortage recovery execution",
        ));
    }
    lock_reservation_tx(&mut tx, access.tenant_id, &shortage).await?;

    let executed_at = now_iso();
    let candidates = lock_candidates_tx(&mut tx, access.tenant_id, &shortage, executed_at).await?;
    let demand = AllocationQuantity::new(shortage.remaining_quantity)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let plan = plan_fefo_allocation(
        demand,
        candidates
            .values()
            .map(LockedCandidate::domain_candidate)
            .collect::<AppResult<Vec<_>>>()?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let newly_allocated = plan.allocated_quantity();
    let remaining = shortage
        .remaining_quantity
        .checked_sub(newly_allocated)
        .ok_or_else(|| AppError::internal("shortage recovery over-allocated demand"))?;
    let outcome = if newly_allocated == 0 {
        AllocationOutcome::NotAllocated
    } else if remaining == 0 {
        AllocationOutcome::FullyAllocated
    } else {
        AllocationOutcome::PartiallyAllocated
    };
    let resulting_shortage_revision = shortage
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("pick shortage revision overflow"))?;
    let resulting_order_revision = shortage
        .order_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let run_id = insert_run_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        &shortage,
        resulting_shortage_revision,
        resulting_order_revision,
        outcome,
        newly_allocated,
        remaining,
        i64::try_from(plan.allocations().len())
            .map_err(|_| AppError::internal("recovery allocation count exceeds i64"))?,
        executed_at,
    )
    .await?;
    let created = persist_recovery_work_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &shortage,
        run_id,
        plan.allocations(),
        &candidates,
        executed_at,
    )
    .await?;
    let reallocated_quantity = shortage
        .reallocated_quantity
        .checked_add(newly_allocated)
        .ok_or_else(|| AppError::internal("reallocated quantity exceeds i64"))?;
    let resulting_status = if reallocated_quantity == shortage.recovery_terminal_quantity {
        PickShortageStatus::AwaitingInventory
    } else {
        PickShortageStatus::RecoveryInProgress
    };
    update_shortage_tx(
        &mut tx,
        access.tenant_id,
        &shortage,
        resulting_shortage_revision,
        resulting_status,
        reallocated_quantity,
        remaining,
        executed_at,
    )
    .await?;
    update_order_revision_tx(
        &mut tx,
        access.tenant_id,
        shortage.order_id,
        shortage.order_revision,
        resulting_order_revision,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        shortage.inventory_owner_id,
        shortage.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "reallocated {} of {} short pick units ({} remaining)",
            newly_allocated, shortage.remaining_quantity, remaining
        ),
    )
    .await?;

    let result = ReallocatePickShortageResult {
        reallocation_run_id: run_id,
        shortage_id: shortage.id,
        shortage_revision: resulting_shortage_revision,
        shortage_status: resulting_status,
        order_id: shortage.order_id,
        order_revision: resulting_order_revision,
        strategy: command.strategy,
        outcome,
        newly_allocated_quantity: ActualPickQuantity::new(newly_allocated)
            .map_err(|error| AppError::internal(error.to_string()))?,
        reallocated_quantity: ActualPickQuantity::new(reallocated_quantity)
            .map_err(|error| AppError::internal(error.to_string()))?,
        recovery_terminal_quantity: ActualPickQuantity::new(shortage.recovery_terminal_quantity)
            .map_err(|error| AppError::internal(error.to_string()))?,
        remaining_to_allocate_quantity: ActualPickQuantity::new(remaining)
            .map_err(|error| AppError::internal(error.to_string()))?,
        new_allocations: created.iter().map(|work| work.allocation.clone()).collect(),
        new_tasks: created.into_iter().map(|work| work.task).collect(),
        executed_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        executed_at,
    };
    enqueue_reallocation_event_tx(
        &mut tx,
        access.tenant_id,
        shortage.inventory_owner_id,
        shortage.facility_id,
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn require_stored_shortage_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    idempotency_key: &str,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let shortage_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT (result_json->>'shortage_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = $2 AND idempotency_key = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(REALLOCATE_PICK_SHORTAGE_OPERATION)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(shortage_id) = shortage_id {
        let visible: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM pick_shortages shortage
                WHERE shortage.tenant_id = $1 AND shortage.id = $2
                  AND ($3 OR shortage.inventory_owner_id = ANY($4))
                  AND ($5 OR shortage.facility_id = ANY($6))
            )
            "#,
        )
        .bind(tenant_id.get())
        .bind(shortage_id)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .fetch_one(&mut **tx)
        .await?;
        if !visible {
            return Err(AppError::not_found("pick shortage"));
        }
    }
    Ok(())
}

async fn shortage_order_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage_id: PickShortageId,
    scope: &ScopeBindings,
) -> AppResult<OrderId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        SELECT order_id
        FROM pick_shortages
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
    OrderId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM orders
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
    .await?;
    id.map(|_| ())
        .ok_or_else(|| AppError::not_found("pick shortage"))
}

async fn lock_shortage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage_id: PickShortageId,
    scope: &ScopeBindings,
) -> AppResult<LockedShortage> {
    let row = sqlx::query(
        r#"
        SELECT shortage.id, shortage.revision, shortage.status,
               shortage.inventory_owner_id, shortage.facility_id,
               shortage.order_release_id, shortage.order_id,
               shortage.order_item_id, shortage.reservation_id,
               shortage.item_id, shortage.uom,
               shortage.reallocated_qty, shortage.recovery_terminal_qty,
               shortage.remaining_to_allocate_qty,
               release.destination_location_id,
               order_header.revision AS order_revision,
               order_header.status AS order_status,
               order_header.rush, order_header.ship_by
        FROM pick_shortages shortage
        INNER JOIN order_releases release
          ON release.tenant_id = shortage.tenant_id
         AND release.inventory_owner_id = shortage.inventory_owner_id
         AND release.facility_id = shortage.facility_id
         AND release.id = shortage.order_release_id
         AND release.order_id = shortage.order_id
        INNER JOIN orders order_header
          ON order_header.tenant_id = shortage.tenant_id
         AND order_header.inventory_owner_id = shortage.inventory_owner_id
         AND order_header.id = shortage.order_id AND order_header.deleted IS NULL
        WHERE shortage.tenant_id = $1 AND shortage.id = $2
        FOR UPDATE OF shortage
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick shortage"))?;
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    if !scope.includes_inventory_owner(owner_id) || !scope.includes_facility(facility_id) {
        return Err(AppError::not_found("pick shortage"));
    }
    let status = PickShortageStatus::parse(&row.try_get::<String, _>("status")?)
        .ok_or_else(|| AppError::internal("pick shortage has invalid status"))?;
    let order_status = OrderStatus::parse(&row.try_get::<String, _>("order_status")?)
        .ok_or_else(|| AppError::internal("pick shortage order has invalid status"))?;
    Ok(LockedShortage {
        id: shortage_id,
        revision: PickShortageRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status,
        inventory_owner_id: InventoryOwnerId::new(owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id,
        release_id: row.try_get("order_release_id")?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_item_id: row.try_get("order_item_id")?,
        reservation_id: row.try_get("reservation_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        reallocated_quantity: row.try_get("reallocated_qty")?,
        recovery_terminal_quantity: row.try_get("recovery_terminal_qty")?,
        remaining_quantity: row.try_get("remaining_to_allocate_qty")?,
        destination_location_id: row.try_get("destination_location_id")?,
        order_revision: OrderRevision::new(row.try_get("order_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_status,
        rush: row.try_get("rush")?,
        ship_by: row.try_get("ship_by")?,
    })
}

async fn lock_reservation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage: &LockedShortage,
) -> AppResult<()> {
    let id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM inventory_reservations
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND facility_id = $3 AND order_id = $4 AND order_item_id = $5
          AND id = $6 AND status = 'active' AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.facility_id)
    .bind(shortage.order_id.get())
    .bind(shortage.order_item_id)
    .bind(shortage.reservation_id)
    .fetch_optional(&mut **tx)
    .await?;
    id.map(|_| ())
        .ok_or_else(|| AppError::conflict("shortage reservation is no longer active"))
}

async fn lock_candidates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage: &LockedShortage,
    occurred_at: Timestamp,
) -> AppResult<HashMap<i64, LockedCandidate>> {
    let hint_rows = sqlx::query(
        r#"
        WITH eligible AS (
            SELECT balance.id, balance.item_batch_id, balance.location_id,
                   balance.license_plate_id,
                   balance.qty_on_hand - balance.qty_reserved - balance.qty_held AS available_qty,
                   batch.expiration, batch.created AS received_at
            FROM inventory_balances balance
            INNER JOIN item_batches batch
              ON batch.tenant_id = balance.tenant_id
             AND batch.inventory_owner_id = balance.inventory_owner_id
             AND batch.id = balance.item_batch_id AND batch.deleted IS NULL
             AND (batch.expiration IS NULL OR batch.expiration > $7)
            INNER JOIN locations location
              ON location.tenant_id = balance.tenant_id
             AND location.facility_id = balance.facility_id
             AND location.id = balance.location_id AND location.deleted IS NULL
             AND location.active AND location.pickable
            LEFT JOIN license_plates plate
              ON plate.tenant_id = balance.tenant_id
             AND plate.inventory_owner_id = balance.inventory_owner_id
             AND plate.facility_id = balance.facility_id
             AND plate.id = balance.license_plate_id
            WHERE balance.tenant_id = $1 AND balance.inventory_owner_id = $2
              AND balance.facility_id = $3 AND balance.item_id = $4
              AND balance.uom = $5 AND balance.id <> $6
              AND balance.status = 'available' AND balance.deleted IS NULL
              AND balance.qty_on_hand - balance.qty_reserved - balance.qty_held > 0
              AND (balance.license_plate_id IS NULL OR (plate.id IS NOT NULL AND plate.deleted IS NULL))
        ), ranked AS (
            SELECT eligible.*,
                   COALESCE(SUM(available_qty) OVER (
                       ORDER BY expiration ASC NULLS LAST, received_at, id
                       ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING
                   ), 0) AS available_before
            FROM eligible
        )
        SELECT id, item_batch_id, location_id, license_plate_id
        FROM ranked
        WHERE available_before < $8
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.facility_id)
    .bind(shortage.item_id)
    .bind(&shortage.uom)
    .bind(shortage_source_balance_id_tx(tx, tenant_id, shortage.id).await?)
    .bind(occurred_at)
    .bind(shortage.remaining_quantity)
    .fetch_all(&mut **tx)
    .await?;
    let hints = hint_rows
        .iter()
        .map(|row| {
            Ok(CandidateHint {
                balance_id: InventoryBalanceId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                location_id: LocationId::new(row.try_get("location_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                plate_id: row
                    .try_get::<Option<i64>, _>("license_plate_id")?
                    .map(LicensePlateId::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    if hints.is_empty() {
        return Ok(HashMap::new());
    }
    lock_candidate_dimensions_tx(tx, tenant_id, shortage, &hints).await?;
    let balance_ids = hints
        .iter()
        .map(|hint| hint.balance_id.get())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT balance.id, balance.item_batch_id, balance.location_id,
               balance.license_plate_id, balance.item_id, balance.uom,
               balance.status, balance.deleted AS balance_deleted,
               balance.qty_on_hand, balance.qty_reserved, balance.qty_held,
               batch.created AS batch_created, batch.deleted AS batch_deleted,
               batch.lot, batch.serial, batch.expiration,
               location.name AS location_name, location.barcode AS location_barcode,
               location.deleted AS location_deleted,
               location.active AS location_active, location.pickable AS location_pickable,
               plate.barcode AS plate_barcode, plate.deleted AS plate_deleted
        FROM inventory_balances balance
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
        INNER JOIN locations location
          ON location.tenant_id = balance.tenant_id
         AND location.facility_id = balance.facility_id
         AND location.id = balance.location_id
        LEFT JOIN license_plates plate
          ON plate.tenant_id = balance.tenant_id
         AND plate.inventory_owner_id = balance.inventory_owner_id
         AND plate.facility_id = balance.facility_id
         AND plate.id = balance.license_plate_id
        WHERE balance.tenant_id = $1 AND balance.id = ANY($2)
        ORDER BY balance.id FOR UPDATE OF balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(&balance_ids)
    .fetch_all(&mut **tx)
    .await?;
    let hints_by_id = hints
        .into_iter()
        .map(|hint| (hint.balance_id.get(), hint))
        .collect::<HashMap<_, _>>();
    let mut candidates = HashMap::with_capacity(rows.len());
    for row in rows {
        let balance_id: i64 = row.try_get("id")?;
        let hint = hints_by_id
            .get(&balance_id)
            .ok_or_else(|| AppError::internal("locked an unexpected recovery balance"))?;
        if row.try_get::<i64, _>("item_batch_id")? != hint.batch_id.get()
            || row.try_get::<i64, _>("location_id")? != hint.location_id.get()
            || row.try_get::<Option<i64>, _>("license_plate_id")?
                != hint.plate_id.map(|id| id.get())
        {
            return Err(AppError::conflict(
                "recovery balance dimensions changed while acquiring locks",
            ));
        }
        let available = row
            .try_get::<i64, _>("qty_on_hand")?
            .checked_sub(row.try_get("qty_reserved")?)
            .and_then(|value| value.checked_sub(row.try_get("qty_held").ok()?))
            .ok_or_else(|| AppError::internal("recovery inventory commitments are invalid"))?;
        let valid = available > 0
            && row.try_get::<i64, _>("item_id")? == shortage.item_id
            && row.try_get::<String, _>("uom")? == shortage.uom
            && row.try_get::<String, _>("status")? == "available"
            && row
                .try_get::<Option<Timestamp>, _>("balance_deleted")?
                .is_none()
            && row
                .try_get::<Option<Timestamp>, _>("batch_deleted")?
                .is_none()
            && row
                .try_get::<Option<Timestamp>, _>("expiration")?
                .is_none_or(|value| value > occurred_at)
            && row
                .try_get::<Option<Timestamp>, _>("location_deleted")?
                .is_none()
            && row.try_get::<bool, _>("location_active")?
            && row.try_get::<bool, _>("location_pickable")?
            && row
                .try_get::<Option<String>, _>("location_barcode")?
                .is_some_and(|value| !value.trim().is_empty())
            && hint.plate_id.is_none_or(|_| {
                row.try_get::<Option<Timestamp>, _>("plate_deleted")
                    .ok()
                    .flatten()
                    .is_none()
                    && row
                        .try_get::<Option<String>, _>("plate_barcode")
                        .ok()
                        .flatten()
                        .is_some_and(|value| !value.trim().is_empty())
            });
        if valid {
            candidates.insert(
                balance_id,
                LockedCandidate {
                    balance_id: hint.balance_id,
                    batch_id: hint.batch_id,
                    location_id: hint.location_id,
                    location_name: row.try_get("location_name")?,
                    location_barcode: row
                        .try_get::<Option<String>, _>("location_barcode")?
                        .ok_or_else(|| AppError::internal("recovery location has no barcode"))?,
                    plate_id: hint.plate_id,
                    plate_barcode: row.try_get("plate_barcode")?,
                    lot: row.try_get("lot")?,
                    serial: row.try_get("serial")?,
                    expiration: row.try_get("expiration")?,
                    received_at: row.try_get("batch_created")?,
                    available_quantity: available,
                },
            );
        }
    }
    Ok(candidates)
}

async fn shortage_source_balance_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage_id: PickShortageId,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT source_inventory_balance_id FROM pick_shortages WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(shortage_id.get())
    .fetch_one(&mut **tx)
    .await?)
}

async fn lock_candidate_dimensions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage: &LockedShortage,
    hints: &[CandidateHint],
) -> AppResult<()> {
    let mut batch_ids = hints
        .iter()
        .map(|hint| hint.batch_id.get())
        .collect::<Vec<_>>();
    batch_ids.sort_unstable();
    batch_ids.dedup();
    sqlx::query(
        r#"
        SELECT id FROM item_batches
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND id = ANY($3)
        ORDER BY id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(&batch_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut location_ids = hints
        .iter()
        .map(|hint| hint.location_id.get())
        .collect::<Vec<_>>();
    location_ids.sort_unstable();
    location_ids.dedup();
    sqlx::query(
        r#"
        SELECT id FROM locations
        WHERE tenant_id = $1 AND facility_id = $2 AND id = ANY($3)
        ORDER BY id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage.facility_id)
    .bind(&location_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut plate_ids = hints
        .iter()
        .filter_map(|hint| hint.plate_id.map(|id| id.get()))
        .collect::<Vec<_>>();
    plate_ids.sort_unstable();
    plate_ids.dedup();
    if !plate_ids.is_empty() {
        sqlx::query(
            r#"
            SELECT id FROM license_plates
            WHERE tenant_id = $1 AND id = ANY($2)
            ORDER BY id FOR UPDATE
            "#,
        )
        .bind(tenant_id.get())
        .bind(&plate_ids)
        .fetch_all(&mut **tx)
        .await?;
    }
    Ok(())
}

impl LockedCandidate {
    fn domain_candidate(&self) -> AppResult<AllocationCandidate> {
        Ok(AllocationCandidate::new(
            self.balance_id,
            self.batch_id,
            self.location_id,
            self.plate_id,
            self.lot.clone(),
            self.serial.clone(),
            self.expiration,
            self.received_at,
            AllocationQuantity::new(self.available_quantity)
                .map_err(|error| AppError::internal(error.to_string()))?,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_run_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ReallocatePickShortageCommand,
    shortage: &LockedShortage,
    resulting_shortage_revision: PickShortageRevision,
    resulting_order_revision: OrderRevision,
    outcome: AllocationOutcome,
    allocated_quantity: i64,
    remaining_quantity: i64,
    allocation_count: i64,
    occurred_at: Timestamp,
) -> AppResult<PickShortageReallocationRunId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO pick_shortage_reallocation_runs (
            tenant_id, inventory_owner_id, facility_id, order_release_id,
            order_id, order_item_id, reservation_id, pick_shortage_id,
            created_by_user_id, created_at, expected_shortage_revision,
            resulting_shortage_revision, expected_order_revision,
            resulting_order_revision, requested_qty, allocated_qty,
            remaining_qty, allocation_count, outcome
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17, $18, $19
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.facility_id)
    .bind(shortage.release_id)
    .bind(shortage.order_id.get())
    .bind(shortage.order_item_id)
    .bind(shortage.reservation_id)
    .bind(shortage.id.get())
    .bind(actor_user_id)
    .bind(occurred_at)
    .bind(command.expected_shortage_revision.get())
    .bind(resulting_shortage_revision.get())
    .bind(command.expected_order_revision.get())
    .bind(resulting_order_revision.get())
    .bind(shortage.remaining_quantity)
    .bind(allocated_quantity)
    .bind(remaining_quantity)
    .bind(allocation_count)
    .bind(outcome.as_str())
    .fetch_one(&mut **tx)
    .await?;
    PickShortageReallocationRunId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn persist_recovery_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    shortage: &LockedShortage,
    run_id: PickShortageReallocationRunId,
    planned: &[PlannedAllocation],
    candidates: &HashMap<i64, LockedCandidate>,
    occurred_at: Timestamp,
) -> AppResult<Vec<CreatedRecoveryWork>> {
    let base_sequence: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(travel_sequence), 0)
        FROM order_release_allocations
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND order_release_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.release_id)
    .fetch_one(&mut **tx)
    .await?;
    let priority = if shortage.rush { 100_i64 } else { 0_i64 };
    let mut created = Vec::with_capacity(planned.len());
    for (index, allocation) in planned.iter().enumerate() {
        let candidate = candidates
            .get(&allocation.inventory_balance_id().get())
            .ok_or_else(|| AppError::internal("recovery plan references an unknown balance"))?;
        let travel_sequence = base_sequence
            .checked_add(
                i64::try_from(index)
                    .map_err(|_| AppError::internal("recovery travel sequence exceeds i64"))?,
            )
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| AppError::internal("recovery travel sequence exceeds i64"))?;
        let allocation_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO inventory_allocations (
                tenant_id, inventory_owner_id, created, modified, created_by,
                reservation_id, inventory_balance_id, facility_id, location_id,
                license_plate_id, item_batch_id, item_id, uom,
                inventory_status, allocation_run_id, qty, status, execution_stage
            ) VALUES (
                $1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                $12, 'available', NULL, $13, 'allocated', 'pick_source'
            ) RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(occurred_at)
        .bind(actor_user_id)
        .bind(shortage.reservation_id)
        .bind(candidate.balance_id.get())
        .bind(shortage.facility_id)
        .bind(candidate.location_id.get())
        .bind(candidate.plate_id.map(|id| id.get()))
        .bind(candidate.batch_id.get())
        .bind(shortage.item_id)
        .bind(&shortage.uom)
        .bind(allocation.quantity().get())
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO order_release_allocations (
                tenant_id, inventory_owner_id, facility_id, order_release_id,
                order_id, order_item_id, reservation_id, allocation_id,
                inventory_balance_id, source_location_id, source_license_plate_id,
                item_batch_id, item_id, uom, inventory_status, planned_qty,
                travel_sequence, source_kind, pick_shortage_id,
                pick_shortage_reallocation_run_id
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, 'available', $15, $16, 'shortage_recovery', $17, $18
            )
            "#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(shortage.facility_id)
        .bind(shortage.release_id)
        .bind(shortage.order_id.get())
        .bind(shortage.order_item_id)
        .bind(shortage.reservation_id)
        .bind(allocation_id)
        .bind(candidate.balance_id.get())
        .bind(candidate.location_id.get())
        .bind(candidate.plate_id.map(|id| id.get()))
        .bind(candidate.batch_id.get())
        .bind(shortage.item_id)
        .bind(&shortage.uom)
        .bind(allocation.quantity().get())
        .bind(travel_sequence)
        .bind(shortage.id.get())
        .bind(run_id.get())
        .execute(&mut **tx)
        .await?;
        let task_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO pick_tasks (
                tenant_id, inventory_owner_id, facility_id, order_release_id,
                order_id, order_item_id, reservation_id, source_allocation_id,
                destination_location_id, created_at, status, priority, ship_by,
                task_timeout_seconds
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                'open', $11, $12, $13
            ) RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(shortage.facility_id)
        .bind(shortage.release_id)
        .bind(shortage.order_id.get())
        .bind(shortage.order_item_id)
        .bind(shortage.reservation_id)
        .bind(allocation_id)
        .bind(shortage.destination_location_id)
        .bind(occurred_at)
        .bind(priority)
        .bind(shortage.ship_by)
        .bind(PICK_LEASE_SECONDS)
        .fetch_one(&mut **tx)
        .await?;
        let content_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO pick_task_contents (
                tenant_id, inventory_owner_id, facility_id, task_id,
                order_release_id, order_id, order_item_id, reservation_id,
                source_allocation_id, source_inventory_balance_id,
                source_location_id, source_license_plate_id, item_batch_id,
                item_id, uom, inventory_status, planned_qty, travel_sequence, state
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, 'available', $16, $17, 'pending'
            ) RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(shortage.facility_id)
        .bind(task_id)
        .bind(shortage.release_id)
        .bind(shortage.order_id.get())
        .bind(shortage.order_item_id)
        .bind(shortage.reservation_id)
        .bind(allocation_id)
        .bind(candidate.balance_id.get())
        .bind(candidate.location_id.get())
        .bind(candidate.plate_id.map(|id| id.get()))
        .bind(candidate.batch_id.get())
        .bind(shortage.item_id)
        .bind(&shortage.uom)
        .bind(allocation.quantity().get())
        .bind(travel_sequence)
        .fetch_one(&mut **tx)
        .await?;
        let allocation_id = InventoryAllocationId::new(allocation_id)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let task_id =
            PickTaskId::new(task_id).map_err(|error| AppError::internal(error.to_string()))?;
        let content_id = PickContentId::new(content_id)
            .map_err(|error| AppError::internal(error.to_string()))?;
        created.push(CreatedRecoveryWork {
            allocation: PickShortageAllocationReadModel {
                allocation_id,
                inventory_balance_id: candidate.balance_id,
                item_batch_id: candidate.batch_id,
                location_id: candidate.location_id,
                location_name: candidate.location_name.clone(),
                location_barcode: PickScanValue::new(candidate.location_barcode.clone())
                    .map_err(|error| AppError::internal(error.to_string()))?,
                license_plate_id: candidate.plate_id,
                license_plate_barcode: candidate
                    .plate_barcode
                    .clone()
                    .map(PickScanValue::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
                lot: candidate.lot.clone(),
                serial: candidate.serial.clone(),
                expiration: candidate.expiration,
                quantity: allocation.quantity(),
                execution_stage: AllocationExecutionStage::PickSource,
            },
            task: PickShortageTaskReadModel {
                task_id,
                content_id,
                source_allocation_id: allocation_id,
                source_inventory_balance_id: candidate.balance_id,
                source_location_id: candidate.location_id,
                source_location_barcode: PickScanValue::new(candidate.location_barcode.clone())
                    .map_err(|error| AppError::internal(error.to_string()))?,
                source_license_plate_id: candidate.plate_id,
                source_license_plate_barcode: candidate
                    .plate_barcode
                    .clone()
                    .map(PickScanValue::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
                planned_quantity: PickQuantity::new(allocation.quantity().get())
                    .map_err(|error| AppError::internal(error.to_string()))?,
            },
        });
    }
    Ok(created)
}

#[allow(clippy::too_many_arguments)]
async fn update_shortage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage: &LockedShortage,
    revision: PickShortageRevision,
    status: PickShortageStatus,
    reallocated_quantity: i64,
    remaining_quantity: i64,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE pick_shortages
        SET modified_at = $1, revision = $2, status = $3,
            reallocated_qty = $4, remaining_to_allocate_qty = $5
        WHERE tenant_id = $6 AND inventory_owner_id = $7 AND id = $8
          AND revision = $9 AND status = $10
          AND reallocated_qty = $11 AND remaining_to_allocate_qty = $12
        "#,
    )
    .bind(occurred_at)
    .bind(revision.get())
    .bind(status.as_str())
    .bind(reallocated_quantity)
    .bind(remaining_quantity)
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.id.get())
    .bind(shortage.revision.get())
    .bind(shortage.status.as_str())
    .bind(shortage.reallocated_quantity)
    .bind(shortage.remaining_quantity)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "pick shortage changed during reallocation",
        ));
    }
    Ok(())
}

async fn update_order_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    expected: OrderRevision,
    resulting: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders SET revision = $1
        WHERE tenant_id = $2 AND id = $3 AND revision = $4
          AND status = 'processing' AND deleted IS NULL
        "#,
    )
    .bind(resulting.get())
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(expected.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "order changed during shortage reallocation",
        ));
    }
    Ok(())
}

async fn require_replayed_run_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    run_id: PickShortageReallocationRunId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id
        FROM pick_shortage_reallocation_runs
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(run_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick shortage reallocation"))?;
    if !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
        || !scope.includes_facility(row.try_get("facility_id")?)
    {
        return Err(AppError::not_found("pick shortage reallocation"));
    }
    Ok(())
}

async fn enqueue_reallocation_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    result: &ReallocatePickShortageResult,
) -> AppResult<()> {
    let facility_id = wareboxes_domain::FacilityId::new(facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!(
        "pick-shortage-reallocation:{}",
        result.reallocation_run_id.get()
    );
    let aggregate_id = result.shortage_id.get().to_string();
    let ordering_key = format!("order:{}", result.order_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let payload = serde_json::json!({
        "reallocation_run_id": result.reallocation_run_id,
        "pick_shortage_id": result.shortage_id,
        "order_id": result.order_id,
        "outcome": result.outcome,
        "newly_allocated_quantity": result.newly_allocated_quantity,
        "reallocated_quantity": result.reallocated_quantity,
        "recovery_terminal_quantity": result.recovery_terminal_quantity,
        "remaining_to_allocate_quantity": result.remaining_to_allocate_quantity,
        "new_task_ids": result.new_tasks.iter().map(|task| task.task_id).collect::<Vec<_>>(),
        "shortage_revision": result.shortage_revision,
        "order_revision": result.order_revision,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(result.executed_by.get()),
            event_key: &event_key,
            aggregate_type: "pick_shortage",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.pick.shortage_reallocated",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.executed_at,
        },
    )
    .await?;
    Ok(())
}
