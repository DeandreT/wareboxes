use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::packing::{
    OpenPackSessionCommand, OpenPackSessionResult, OPEN_PACK_SESSION_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    begin_packing, InventoryOwnerId, OrderRevision, PackSessionId, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::inventory_locking;
use crate::repo::orders::insert_order_activity_tx;
use crate::repo::picking::order_pick_readiness_tx;

use super::policy::{policy_bindings, resolve_decision_policy_tx};
use super::read_model::load_session_tx;
use super::{enqueue_order_event_tx, lock_order_tx, require_replayed_session_visible_tx};

#[derive(Debug)]
struct PickedAllocation {
    order_release_id: i64,
    order_item_id: i64,
    reservation_id: i64,
    outbound_order_container_id: i64,
    pick_confirmation_id: i64,
    allocation_id: i64,
    inventory_balance_id: i64,
    location_id: i64,
    license_plate_id: i64,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    inventory_status: String,
    quantity: i64,
}

pub async fn open_session(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &OpenPackSessionCommand,
) -> AppResult<OpenPackSessionResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, OPEN_PACK_SESSION_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    if let Some(result) = prepared.replayed::<OpenPackSessionResult>(&mut tx).await? {
        require_replayed_session_visible_tx(&mut tx, access.tenant_id, &result.session, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }
    if !scope.includes_facility(command.facility_id.get()) {
        return Err(AppError::not_found("facility"));
    }
    let order = lock_order_tx(&mut tx, access.tenant_id, command.order_id, &scope).await?;
    let next_status =
        begin_packing(order.status).map_err(|error| AppError::conflict(error.to_string()))?;
    if order.revision != command.expected_revision {
        return Err(AppError::conflict("packing revision is stale"));
    }
    require_active_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.facility_id.get(),
    )
    .await?;
    let station_barcode = require_packing_location_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id.get(),
        command.station_location_id.get(),
    )
    .await?;
    let started_at = now_iso();
    let pack_policy = resolve_decision_policy_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.facility_id,
        started_at,
    )
    .await?;
    let station_scan_verified = match command.station_location_barcode.as_ref() {
        Some(scanned) if scanned.as_str() == station_barcode => true,
        Some(_) => {
            return Err(AppError::bad_request(
                "scanned station does not match the selected packing location",
            ))
        }
        None if pack_policy.require_station_scan => {
            return Err(AppError::bad_request(
                "the effective Pack policy requires a station scan",
            ))
        }
        None => false,
    };
    lock_station_policy_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id.get(),
        command.station_location_id.get(),
        pack_policy.allow_mixed_orders,
    )
    .await?;
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM packing_sessions WHERE tenant_id = $1 AND order_id = $2 AND state <> 'abandoned')",
    )
    .bind(access.tenant_id.get())
    .bind(command.order_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if existing {
        return Err(AppError::conflict("order already has a packing session"));
    }

    let allocations = lock_picked_allocations_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id.get(),
        command.facility_id.get(),
        command.station_location_id.get(),
    )
    .await?;
    require_fresh_pick_execution_tx(
        &mut tx,
        access.tenant_id,
        command.order_id.get(),
        &allocations,
    )
    .await?;
    let expected_count = i64::try_from(allocations.len())
        .map_err(|_| AppError::internal("packing allocation count exceeds i64"))?;
    let expected_qty = allocations.iter().try_fold(0_i64, |total, allocation| {
        total
            .checked_add(allocation.quantity)
            .ok_or_else(|| AppError::internal("packing quantity exceeds i64"))
    })?;
    let release_id = allocations
        .first()
        .map(|allocation| allocation.order_release_id)
        .ok_or_else(|| AppError::conflict("order has no completed picks to pack"))?;
    require_all_picks_complete_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id.get(),
        expected_count,
        expected_qty,
    )
    .await?;

    let revision = order
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("order revision overflow"))?;
    let session_id = insert_session_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        release_id,
        context.actor_id.get(),
        command,
        revision,
        expected_count,
        expected_qty,
        started_at,
        &pack_policy,
        station_scan_verified,
    )
    .await?;
    insert_snapshots_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command,
        session_id,
        &allocations,
    )
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE orders SET status = $1, revision = $2
        WHERE tenant_id = $3 AND id = $4 AND status = $5 AND revision = $6
        "#,
    )
    .bind(next_status.as_str())
    .bind(revision.get())
    .bind(access.tenant_id.get())
    .bind(command.order_id.get())
    .bind(order.status.as_str())
    .bind(order.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed while opening packing"));
    }
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id.get(),
        Some(context.actor_id.get()),
        &format!("opened packing session {session_id} with {expected_count} allocation(s)"),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.facility_id.get(),
        context.actor_id.get(),
        command.order_id,
        "packing.session_opened",
        &format!("packing-session:{}:opened", session_id.get()),
        serde_json::json!({
            "packing_session_id": session_id,
            "order_id": command.order_id,
            "facility_id": command.facility_id,
            "packing_location_id": command.station_location_id,
            "expected_revision": command.expected_revision,
            "revision": revision,
            "expected_allocation_count": expected_count,
            "expected_quantity": expected_qty,
            "pack_policy": pack_policy,
            "station_scan_verified": station_scan_verified,
            "started_at": started_at,
        }),
        started_at,
    )
    .await?;
    let session = load_session_tx(&mut tx, access.tenant_id, session_id, &scope).await?;
    Ok(prepared
        .commit(tx, OpenPackSessionResult { session })
        .await?)
}

async fn require_active_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: i64,
) -> AppResult<()> {
    let id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT assignment.id
        FROM inventory_owner_facilities assignment
        INNER JOIN inventory_owners owner
          ON owner.tenant_id = assignment.tenant_id
         AND owner.id = assignment.inventory_owner_id AND owner.deleted IS NULL
        INNER JOIN facilities facility
          ON facility.tenant_id = assignment.tenant_id
         AND facility.id = assignment.facility_id AND facility.deleted IS NULL
        WHERE assignment.tenant_id = $1 AND assignment.inventory_owner_id = $2
          AND assignment.facility_id = $3 AND assignment.deleted IS NULL
        FOR SHARE OF assignment, owner
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id)
    .fetch_optional(&mut **tx)
    .await?;
    if id.is_none() {
        return Err(AppError::conflict(
            "inventory owner is not active at the packing facility",
        ));
    }
    Ok(())
}

async fn require_packing_location_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: i64,
    location_id: i64,
) -> AppResult<String> {
    let row = sqlx::query(
        r#"
        SELECT active, pickable, type, barcode
        FROM locations
        WHERE tenant_id = $1 AND facility_id = $2 AND id = $3 AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(location_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing location"))?;
    let barcode: Option<String> = row.try_get("barcode")?;
    if !row.try_get::<bool, _>("active")?
        || row.try_get::<bool, _>("pickable")?
        || !row
            .try_get::<String, _>("type")?
            .eq_ignore_ascii_case("packing")
        || barcode
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(AppError::conflict(
            "packing station must be an active, scannable packing location",
        ));
    }
    barcode.ok_or_else(|| AppError::internal("packing station barcode is missing"))
}

async fn lock_station_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: i64,
    station_location_id: i64,
    allow_mixed_orders: bool,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "packing-station:{}:{facility_id}:{station_location_id}",
            tenant_id.get()
        ))
        .execute(&mut **tx)
        .await?;
    let conflict: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
            SELECT 1 FROM packing_sessions
            WHERE tenant_id=$1 AND facility_id=$2 AND packing_location_id=$3
              AND state='open' AND (NOT $4 OR NOT allow_mixed_orders)
        )"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(station_location_id)
    .bind(allow_mixed_orders)
    .fetch_one(&mut **tx)
    .await?;
    if conflict {
        Err(AppError::conflict(
            "the effective Pack policy reserves this station for one order",
        ))
    } else {
        Ok(())
    }
}

async fn require_fresh_pick_execution_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: i64,
    allocations: &[PickedAllocation],
) -> AppResult<()> {
    let confirmation_ids = allocations
        .iter()
        .map(|allocation| allocation.pick_confirmation_id)
        .collect::<Vec<_>>();
    let already_abandoned: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM packing_session_allocations snapshot
            INNER JOIN packing_sessions session
              ON session.tenant_id=snapshot.tenant_id
             AND session.inventory_owner_id=snapshot.inventory_owner_id
             AND session.facility_id=snapshot.facility_id
             AND session.id=snapshot.packing_session_id
            WHERE snapshot.tenant_id=$1 AND snapshot.order_id=$2
              AND snapshot.pick_confirmation_id=ANY($3)
              AND session.state='abandoned'
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(&confirmation_ids)
    .fetch_one(&mut **tx)
    .await?;
    if already_abandoned {
        return Err(AppError::conflict(
            "restored picks must be reversed and repicked before reopening packing",
        ));
    }
    Ok(())
}

async fn lock_picked_allocations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    order_id: i64,
    facility_id: i64,
    packing_location_id: i64,
) -> AppResult<Vec<PickedAllocation>> {
    let plate_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT destination_license_plate_id
        FROM pick_confirmations
        WHERE tenant_id = $1 AND inventory_owner_id = $2
          AND facility_id = $3 AND order_id = $4
        ORDER BY destination_license_plate_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id)
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    inventory_locking::lock_license_plates(tx, tenant_id, plate_ids).await?;
    let rows = sqlx::query(
        r#"
        SELECT confirmation.order_release_id, confirmation.order_item_id,
               confirmation.reservation_id,
               container.id AS outbound_order_container_id,
               confirmation.id AS pick_confirmation_id,
               confirmation.destination_inventory_allocation_id AS allocation_id,
               confirmation.destination_inventory_balance_id AS inventory_balance_id,
               confirmation.destination_location_id AS location_id,
               confirmation.destination_license_plate_id AS license_plate_id,
               confirmation.item_batch_id, confirmation.item_id,
               confirmation.uom, confirmation.inventory_status,
               confirmation.picked_qty,
               allocation.reservation_id AS allocation_reservation_id,
               allocation.inventory_balance_id AS allocation_balance_id,
               allocation.location_id AS allocation_location_id,
               allocation.license_plate_id AS allocation_plate_id,
               allocation.item_batch_id AS allocation_batch_id,
               allocation.item_id AS allocation_item_id,
               allocation.uom AS allocation_uom,
               allocation.inventory_status AS allocation_inventory_status,
               allocation.qty AS allocation_qty,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_plate_id,
               balance.item_batch_id AS balance_batch_id,
               balance.item_id AS balance_item_id,
               balance.uom AS balance_uom, balance.status AS balance_status,
               balance.qty_on_hand, balance.qty_reserved
        FROM pick_confirmations confirmation
        INNER JOIN outbound_order_containers container
          ON container.tenant_id = confirmation.tenant_id
         AND container.inventory_owner_id = confirmation.inventory_owner_id
         AND container.facility_id = confirmation.facility_id
         AND container.order_release_id = confirmation.order_release_id
         AND container.order_id = confirmation.order_id
         AND container.license_plate_id = confirmation.destination_license_plate_id
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id = confirmation.tenant_id
         AND allocation.inventory_owner_id = confirmation.inventory_owner_id
         AND allocation.id = confirmation.destination_inventory_allocation_id
         AND allocation.status = 'allocated' AND allocation.deleted IS NULL
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = confirmation.tenant_id
         AND balance.inventory_owner_id = confirmation.inventory_owner_id
         AND balance.facility_id = confirmation.facility_id
         AND balance.id = confirmation.destination_inventory_balance_id
         AND balance.deleted IS NULL
        WHERE confirmation.tenant_id = $1 AND confirmation.inventory_owner_id = $2
          AND confirmation.facility_id = $3 AND confirmation.order_id = $4
        ORDER BY confirmation.id
        FOR UPDATE OF allocation, balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id)
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let allocation = PickedAllocation {
                order_release_id: row.try_get("order_release_id")?,
                order_item_id: row.try_get("order_item_id")?,
                reservation_id: row.try_get("reservation_id")?,
                outbound_order_container_id: row.try_get("outbound_order_container_id")?,
                pick_confirmation_id: row.try_get("pick_confirmation_id")?,
                allocation_id: row.try_get("allocation_id")?,
                inventory_balance_id: row.try_get("inventory_balance_id")?,
                location_id: row.try_get("location_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                inventory_status: row.try_get("inventory_status")?,
                quantity: row.try_get("picked_qty")?,
            };
            let matches = row.try_get::<i64, _>("allocation_reservation_id")?
                == allocation.reservation_id
                && row.try_get::<i64, _>("allocation_balance_id")?
                    == allocation.inventory_balance_id
                && row.try_get::<i64, _>("allocation_location_id")? == allocation.location_id
                && row.try_get::<Option<i64>, _>("allocation_plate_id")?
                    == Some(allocation.license_plate_id)
                && row.try_get::<i64, _>("allocation_batch_id")? == allocation.item_batch_id
                && row.try_get::<i64, _>("allocation_item_id")? == allocation.item_id
                && row.try_get::<String, _>("allocation_uom")? == allocation.uom
                && row.try_get::<String, _>("allocation_inventory_status")?
                    == allocation.inventory_status
                && row.try_get::<i64, _>("allocation_qty")? == allocation.quantity
                && row.try_get::<i64, _>("balance_location_id")? == allocation.location_id
                && row.try_get::<Option<i64>, _>("balance_plate_id")?
                    == Some(allocation.license_plate_id)
                && row.try_get::<i64, _>("balance_batch_id")? == allocation.item_batch_id
                && row.try_get::<i64, _>("balance_item_id")? == allocation.item_id
                && row.try_get::<String, _>("balance_uom")? == allocation.uom
                && row.try_get::<String, _>("balance_status")? == allocation.inventory_status
                && row.try_get::<i64, _>("qty_on_hand")? >= allocation.quantity
                && row.try_get::<i64, _>("qty_reserved")? >= allocation.quantity;
            if !matches
                || allocation.location_id != packing_location_id
                || allocation.quantity <= 0
                || allocation.inventory_status != "available"
            {
                return Err(AppError::conflict(
                    "picked allocation is not ready at the selected packing station",
                ));
            }
            Ok(allocation)
        })
        .collect()
}

async fn require_all_picks_complete_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    order_id: i64,
    staged_allocation_count: i64,
    staged_quantity: i64,
) -> AppResult<()> {
    let order_id = wareboxes_domain::OrderId::new(order_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let readiness = order_pick_readiness_tx(tx, tenant_id, owner_id, order_id).await?;
    if !readiness.is_ready_to_pack()
        || readiness.staged_quantity != staged_quantity
        || readiness.staged_allocation_count != staged_allocation_count
    {
        return Err(AppError::conflict("all picks must complete before packing"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    release_id: i64,
    actor_user_id: i64,
    command: &OpenPackSessionCommand,
    revision: OrderRevision,
    expected_count: i64,
    expected_qty: i64,
    started_at: Timestamp,
    pack_policy: &wareboxes_application::packing_decision_policy::PackDecisionPolicyReadModel,
    station_scan_verified: bool,
) -> AppResult<PackSessionId> {
    let policy = policy_bindings(pack_policy);
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO packing_sessions (
            tenant_id, inventory_owner_id, facility_id, order_release_id,
            order_id, packing_location_id, state, revision,
            expected_allocation_count, expected_qty, packed_allocation_count,
            packed_qty, open_carton_count, closed_carton_count,
            started_by_user_id, started_at, pack_policy_source,
            pack_configuration_id, pack_configuration_revision, pack_scope_level,
            pack_inventory_owner_id, pack_facility_id, require_station_scan,
            require_weight, allow_mixed_orders, pack_policy_hash,
            station_scan_value, station_scan_verified
        ) VALUES (
            $1, $2, $3, $4, $5, $6, 'open', $7, $8, $9, 0, 0, 0, 0, $10, $11,
            $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23
        )
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(command.facility_id.get())
    .bind(release_id)
    .bind(command.order_id.get())
    .bind(command.station_location_id.get())
    .bind(revision.get())
    .bind(expected_count)
    .bind(expected_qty)
    .bind(actor_user_id)
    .bind(started_at)
    .bind(policy.source)
    .bind(policy.configuration_id)
    .bind(policy.configuration_revision)
    .bind(policy.scope_level)
    .bind(policy.inventory_owner_id)
    .bind(policy.facility_id)
    .bind(policy.require_station_scan)
    .bind(policy.require_weight)
    .bind(policy.allow_mixed_orders)
    .bind(policy.policy_hash)
    .bind(
        command
            .station_location_barcode
            .as_ref()
            .map(|value| value.as_str()),
    )
    .bind(station_scan_verified)
    .fetch_one(&mut **tx)
    .await?;
    PackSessionId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn insert_snapshots_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    command: &OpenPackSessionCommand,
    session_id: PackSessionId,
    allocations: &[PickedAllocation],
) -> AppResult<()> {
    for (index, allocation) in allocations.iter().enumerate() {
        let sequence = i64::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| AppError::internal("packing sequence exceeds i64"))?;
        sqlx::query(
            r#"
            INSERT INTO packing_session_allocations (
                tenant_id, inventory_owner_id, facility_id, packing_session_id,
                order_release_id, order_id, order_item_id, reservation_id,
                outbound_order_container_id, pick_confirmation_id,
                source_inventory_allocation_id, source_inventory_balance_id,
                source_location_id, source_license_plate_id, item_batch_id,
                item_id, uom, inventory_status, planned_qty, sequence
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19, $20
            )
            "#,
        )
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(command.facility_id.get())
        .bind(session_id.get())
        .bind(allocation.order_release_id)
        .bind(command.order_id.get())
        .bind(allocation.order_item_id)
        .bind(allocation.reservation_id)
        .bind(allocation.outbound_order_container_id)
        .bind(allocation.pick_confirmation_id)
        .bind(allocation.allocation_id)
        .bind(allocation.inventory_balance_id)
        .bind(allocation.location_id)
        .bind(allocation.license_plate_id)
        .bind(allocation.item_batch_id)
        .bind(allocation.item_id)
        .bind(&allocation.uom)
        .bind(&allocation.inventory_status)
        .bind(allocation.quantity)
        .bind(sequence)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
