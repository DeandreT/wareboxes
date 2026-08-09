use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::item_substitution::{
    SubstitutePickShortageCommand, SubstitutePickShortageResult, SubstitutePickWorkReadModel,
    SUBSTITUTE_PICK_SHORTAGE_OPERATION,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    substitute_pick_shortage, ActualPickQuantity, CatalogItemId, FacilityId, InventoryAllocationId,
    InventoryBalanceId, InventoryOwnerId, ItemBatchId, ItemSubstitutionDefinition,
    ItemSubstitutionId, ItemSubstitutionPolicyId, ItemSubstitutionPolicyRevision, LicensePlateId,
    LocationId, OrderId, OrderLineId, OrderRevision, OrderStatus, PickContentId, PickShortageId,
    PickShortageRevision, PickShortageStatus, PickTaskId, SubstitutionQuantity, SubstitutionUom,
    TenantId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::{insert_order_activity_tx, next_outbox_sequence_tx};

const PICK_LEASE_SECONDS: i64 = 120;

#[derive(Debug)]
struct LockedShortage {
    id: PickShortageId,
    revision: PickShortageRevision,
    status: PickShortageStatus,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    order_release_id: i64,
    order_id: OrderId,
    order_item_id: OrderLineId,
    reservation_id: i64,
    item_id: CatalogItemId,
    uom: SubstitutionUom,
    short_quantity: i64,
    reallocated_quantity: ActualPickQuantity,
    recovery_terminal_quantity: ActualPickQuantity,
    remaining_quantity: ActualPickQuantity,
    destination_location_id: LocationId,
    order_revision: OrderRevision,
    order_status: OrderStatus,
    rush: bool,
    ship_by: Option<Timestamp>,
}

#[derive(Debug)]
struct LockedPolicy {
    id: ItemSubstitutionPolicyId,
    revision: ItemSubstitutionPolicyRevision,
    definition: ItemSubstitutionDefinition,
}

#[derive(Debug)]
struct Candidate {
    balance_id: InventoryBalanceId,
    batch_id: ItemBatchId,
    location_id: LocationId,
    plate_id: Option<LicensePlateId>,
    available: i64,
}

#[derive(Debug)]
struct PlannedCandidate {
    candidate: Candidate,
    quantity: SubstitutionQuantity,
}

pub async fn substitute_shortage(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &SubstitutePickShortageCommand,
) -> AppResult<SubstitutePickShortageResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, SUBSTITUTE_PICK_SHORTAGE_OPERATION, command)?;
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
    require_replay_visibility_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<SubstitutePickShortageResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let order_id =
        scoped_order_hint_tx(&mut tx, access.tenant_id, command.shortage_id, &scope).await?;
    lock_order_tx(&mut tx, access.tenant_id, order_id).await?;
    let shortage = lock_shortage_tx(&mut tx, access.tenant_id, command.shortage_id, &scope).await?;
    if shortage.order_id != order_id
        || shortage.revision != command.expected_shortage_revision
        || shortage.order_revision != command.expected_order_revision
    {
        return Err(AppError::conflict(
            "pick shortage or order revision is stale",
        ));
    }
    let policy = lock_policy_tx(&mut tx, access.tenant_id, command.policy_id, &shortage).await?;
    if policy.revision != command.expected_policy_revision {
        return Err(AppError::conflict(
            "item substitution policy revision is stale",
        ));
    }
    reject_substitution_cycle_tx(&mut tx, access.tenant_id, &shortage, &policy).await?;
    let transition = substitute_pick_shortage(
        shortage.status,
        shortage.order_status,
        shortage.revision,
        shortage.order_revision,
        shortage.short_quantity,
        shortage.reallocated_quantity,
        shortage.recovery_terminal_quantity,
        shortage.remaining_quantity,
        shortage.item_id,
        &shortage.uom,
        &policy.definition,
    )
    .map_err(|error| AppError::conflict(error.to_string()))?;
    lock_source_reservation_tx(&mut tx, access.tenant_id, &shortage).await?;
    let substituted_at = now_iso();
    let planned = lock_and_plan_candidates_tx(
        &mut tx,
        access.tenant_id,
        &shortage,
        &policy,
        transition.substitute_quantity,
        substituted_at,
    )
    .await?;
    let substitute_line_id = reserve_identity_tx(&mut tx, "order_items", "id").await?;
    let substitute_reservation_id =
        reserve_identity_tx(&mut tx, "inventory_reservations", "id").await?;
    let allocation_count = i64::try_from(planned.len())
        .map_err(|_| AppError::internal("substitute allocation count exceeds i64"))?;
    let substitution_id = insert_substitution_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        command,
        &shortage,
        &policy,
        &transition,
        substitute_line_id,
        substitute_reservation_id,
        allocation_count,
        substituted_at,
    )
    .await?;
    sqlx::query("SELECT set_config('wareboxes.item_substitution_id',$1,true)")
        .bind(substitution_id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_substitute_demand_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &shortage,
        &policy,
        transition.substitute_quantity,
        substitute_line_id,
        substitute_reservation_id,
        substituted_at,
    )
    .await?;
    let work = insert_substitute_work_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &shortage,
        &policy,
        substitution_id,
        substitute_line_id,
        substitute_reservation_id,
        &planned,
        substituted_at,
    )
    .await?;
    resolve_shortage_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &shortage,
        transition.shortage_revision,
        transition.accepted_source_quantity,
        substituted_at,
    )
    .await?;
    update_order_revision_tx(
        &mut tx,
        access.tenant_id,
        shortage.order_id,
        shortage.order_revision,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        shortage.inventory_owner_id,
        shortage.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "substituted {} {} with {} {}",
            transition.accepted_source_quantity.get(),
            shortage.uom,
            transition.substitute_quantity.get(),
            policy.definition.substitute_uom
        ),
    )
    .await?;
    let result = SubstitutePickShortageResult {
        substitution_id,
        shortage_id: shortage.id,
        shortage_revision: transition.shortage_revision,
        policy_id: policy.id,
        policy_revision: policy.revision,
        inventory_owner_id: shortage.inventory_owner_id,
        facility_id: shortage.facility_id,
        order_id: shortage.order_id,
        order_revision: transition.order_revision,
        order_status: OrderStatus::Processing,
        source_order_line_id: shortage.order_item_id,
        substitute_order_line_id: OrderLineId::new(substitute_line_id).map_err(internal)?,
        substitute_reservation_id,
        accepted_source_quantity: transition.accepted_source_quantity,
        substitute_quantity: transition.substitute_quantity,
        substitute_item_id: policy.definition.substitute_item_id.get(),
        substitute_uom: policy.definition.substitute_uom.to_string(),
        work,
        details: command.details.clone(),
        substituted_by: context.actor_id,
        substituted_at,
    };
    enqueue_event_tx(&mut tx, access.tenant_id, &result).await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn scoped_order_hint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage_id: PickShortageId,
    scope: &ScopeBindings,
) -> AppResult<OrderId> {
    let value: i64 = sqlx::query_scalar(
        r#"SELECT order_id FROM pick_shortages
           WHERE tenant_id=$1 AND id=$2
             AND ($3 OR inventory_owner_id=ANY($4))
             AND ($5 OR facility_id=ANY($6))"#,
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
    OrderId::new(value).map_err(internal)
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
) -> AppResult<()> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM orders WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    found
        .map(|_| ())
        .ok_or_else(|| AppError::not_found("order"))
}

async fn lock_shortage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage_id: PickShortageId,
    scope: &ScopeBindings,
) -> AppResult<LockedShortage> {
    let row = sqlx::query(
        r#"SELECT shortage.revision,shortage.status,shortage.inventory_owner_id,
                  shortage.facility_id,shortage.order_release_id,shortage.order_id,
                  shortage.order_item_id,shortage.reservation_id,shortage.item_id,
                  shortage.uom,shortage.short_qty,shortage.reallocated_qty,
                  shortage.recovery_terminal_qty,shortage.remaining_to_allocate_qty,
                  release.destination_location_id,order_header.revision AS order_revision,
                  order_header.status AS order_status,order_header.rush,order_header.ship_by
           FROM pick_shortages shortage
           JOIN order_releases release ON release.tenant_id=shortage.tenant_id
             AND release.inventory_owner_id=shortage.inventory_owner_id
             AND release.facility_id=shortage.facility_id
             AND release.id=shortage.order_release_id AND release.order_id=shortage.order_id
           JOIN orders order_header ON order_header.tenant_id=shortage.tenant_id
             AND order_header.inventory_owner_id=shortage.inventory_owner_id
             AND order_header.id=shortage.order_id AND order_header.deleted IS NULL
           WHERE shortage.tenant_id=$1 AND shortage.id=$2
           FOR UPDATE OF shortage"#,
    )
    .bind(tenant_id.get())
    .bind(shortage_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick shortage"))?;
    let owner_id = row.try_get::<i64, _>("inventory_owner_id")?;
    let facility_id = row.try_get::<i64, _>("facility_id")?;
    if !scope.includes_inventory_owner(owner_id) || !scope.includes_facility(facility_id) {
        return Err(AppError::not_found("pick shortage"));
    }
    Ok(LockedShortage {
        id: shortage_id,
        revision: PickShortageRevision::new(row.try_get("revision")?).map_err(internal)?,
        status: PickShortageStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or_else(|| AppError::internal("invalid pick shortage status"))?,
        inventory_owner_id: InventoryOwnerId::new(owner_id).map_err(internal)?,
        facility_id: FacilityId::new(facility_id).map_err(internal)?,
        order_release_id: row.try_get("order_release_id")?,
        order_id: OrderId::new(row.try_get("order_id")?).map_err(internal)?,
        order_item_id: OrderLineId::new(row.try_get("order_item_id")?).map_err(internal)?,
        reservation_id: row.try_get("reservation_id")?,
        item_id: CatalogItemId::new(row.try_get("item_id")?).map_err(internal)?,
        uom: SubstitutionUom::new(row.try_get::<String, _>("uom")?).map_err(internal)?,
        short_quantity: row.try_get("short_qty")?,
        reallocated_quantity: ActualPickQuantity::new(row.try_get("reallocated_qty")?)
            .map_err(internal)?,
        recovery_terminal_quantity: ActualPickQuantity::new(row.try_get("recovery_terminal_qty")?)
            .map_err(internal)?,
        remaining_quantity: ActualPickQuantity::new(row.try_get("remaining_to_allocate_qty")?)
            .map_err(internal)?,
        destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(internal)?,
        order_revision: OrderRevision::new(row.try_get("order_revision")?).map_err(internal)?,
        order_status: OrderStatus::parse(&row.try_get::<String, _>("order_status")?)
            .ok_or_else(|| AppError::internal("invalid order status"))?,
        rush: row.try_get("rush")?,
        ship_by: row.try_get("ship_by")?,
    })
}

async fn lock_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ItemSubstitutionPolicyId,
    shortage: &LockedShortage,
) -> AppResult<LockedPolicy> {
    let row = sqlx::query(
        r#"SELECT revision,source_item_id,source_uom,substitute_item_id,
                  substitute_uom,source_qty,substitute_qty
           FROM item_substitution_policies
           WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
             AND id=$4 AND effective_to IS NULL FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.facility_id.get())
    .bind(policy_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("item substitution policy is not active"))?;
    Ok(LockedPolicy {
        id: policy_id,
        revision: ItemSubstitutionPolicyRevision::new(row.try_get("revision")?)
            .map_err(internal)?,
        definition: ItemSubstitutionDefinition::new(
            CatalogItemId::new(row.try_get("source_item_id")?).map_err(internal)?,
            SubstitutionUom::new(row.try_get::<String, _>("source_uom")?).map_err(internal)?,
            CatalogItemId::new(row.try_get("substitute_item_id")?).map_err(internal)?,
            SubstitutionUom::new(row.try_get::<String, _>("substitute_uom")?).map_err(internal)?,
            SubstitutionQuantity::new(row.try_get("source_qty")?).map_err(internal)?,
            SubstitutionQuantity::new(row.try_get("substitute_qty")?).map_err(internal)?,
        )
        .map_err(internal)?,
    })
}

async fn reject_substitution_cycle_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage: &LockedShortage,
    policy: &LockedPolicy,
) -> AppResult<()> {
    let cycle: bool = sqlx::query_scalar(
        r#"WITH RECURSIVE lineage(order_item_id) AS (
               SELECT $3::bigint
               UNION
               SELECT substitution.source_order_item_id
               FROM pick_shortage_substitutions substitution
               JOIN lineage ON lineage.order_item_id=substitution.substitute_order_item_id
               WHERE substitution.tenant_id=$1 AND substitution.order_id=$2)
           SELECT EXISTS (
               SELECT 1 FROM lineage
               JOIN order_items item ON item.tenant_id=$1 AND item.order_id=$2
                                    AND item.id=lineage.order_item_id
               WHERE item.item_id=$4 AND item.uom=$5)"#,
    )
    .bind(tenant_id.get())
    .bind(shortage.order_id.get())
    .bind(shortage.order_item_id.get())
    .bind(policy.definition.substitute_item_id.get())
    .bind(policy.definition.substitute_uom.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if cycle {
        Err(AppError::conflict(
            "item substitution would create a demand cycle",
        ))
    } else {
        Ok(())
    }
}

async fn lock_source_reservation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage: &LockedShortage,
) -> AppResult<()> {
    let found: Option<i64> = sqlx::query_scalar(
        r#"SELECT id FROM inventory_reservations
           WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
             AND id=$4 AND order_id=$5 AND order_item_id=$6
             AND status='active' AND deleted IS NULL FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.facility_id.get())
    .bind(shortage.reservation_id)
    .bind(shortage.order_id.get())
    .bind(shortage.order_item_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    found
        .map(|_| ())
        .ok_or_else(|| AppError::conflict("shortage reservation is no longer active"))
}

async fn lock_and_plan_candidates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shortage: &LockedShortage,
    policy: &LockedPolicy,
    demand: SubstitutionQuantity,
    occurred_at: Timestamp,
) -> AppResult<Vec<PlannedCandidate>> {
    let hints = sqlx::query(
        r#"WITH eligible AS (
               SELECT balance.id,balance.item_batch_id,balance.location_id,
                      balance.license_plate_id,
                      balance.qty_on_hand-balance.qty_reserved-balance.qty_held AS available_qty,
                      batch.expiration,batch.created AS received_at
               FROM inventory_balances balance
               JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
                 AND batch.inventory_owner_id=balance.inventory_owner_id
                 AND batch.id=balance.item_batch_id AND batch.deleted IS NULL
                 AND (batch.expiration IS NULL OR batch.expiration>$7)
               JOIN locations location ON location.tenant_id=balance.tenant_id
                 AND location.facility_id=balance.facility_id
                 AND location.id=balance.location_id AND location.deleted IS NULL
                 AND location.active AND location.pickable
                 AND location.barcode IS NOT NULL AND btrim(location.barcode)<>''
               LEFT JOIN license_plates plate ON plate.tenant_id=balance.tenant_id
                 AND plate.inventory_owner_id=balance.inventory_owner_id
                 AND plate.facility_id=balance.facility_id AND plate.id=balance.license_plate_id
               WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
                 AND balance.facility_id=$3 AND balance.item_id=$4 AND balance.uom=$5
                 AND balance.status='available' AND balance.deleted IS NULL
                 AND balance.qty_on_hand-balance.qty_reserved-balance.qty_held>0
                 AND (balance.license_plate_id IS NULL OR
                      (plate.id IS NOT NULL AND plate.deleted IS NULL
                       AND plate.barcode IS NOT NULL AND btrim(plate.barcode)<>''))
           ), ranked AS (
               SELECT eligible.*,COALESCE(SUM(available_qty) OVER (
                   ORDER BY expiration ASC NULLS LAST,received_at,id
                   ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING),0) AS available_before
               FROM eligible)
           SELECT id,item_batch_id,location_id,license_plate_id
           FROM ranked WHERE available_before<$6 ORDER BY id"#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.facility_id.get())
    .bind(policy.definition.substitute_item_id.get())
    .bind(policy.definition.substitute_uom.as_str())
    .bind(demand.get())
    .bind(occurred_at)
    .fetch_all(&mut **tx)
    .await?;
    if hints.is_empty() {
        return Err(AppError::conflict(
            "approved substitute inventory is unavailable",
        ));
    }
    let batch_ids = hints
        .iter()
        .map(|row| row.try_get::<i64, _>("item_batch_id"))
        .collect::<Result<Vec<_>, _>>()?;
    let location_ids = hints
        .iter()
        .map(|row| row.try_get::<i64, _>("location_id"))
        .collect::<Result<Vec<_>, _>>()?;
    let plate_ids = hints
        .iter()
        .map(|row| row.try_get::<Option<i64>, _>("license_plate_id"))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    lock_ids_tx(tx, "item_batches", tenant_id, &batch_ids, false).await?;
    lock_ids_tx(tx, "locations", tenant_id, &location_ids, false).await?;
    lock_ids_tx(tx, "license_plates", tenant_id, &plate_ids, true).await?;
    let balance_ids = hints
        .iter()
        .map(|row| row.try_get::<i64, _>("id"))
        .collect::<Result<Vec<_>, _>>()?;
    let rows = sqlx::query(
        r#"SELECT balance.id,balance.item_batch_id,balance.location_id,
                  balance.license_plate_id,balance.item_id,balance.uom,balance.status,
                  balance.deleted,balance.qty_on_hand,balance.qty_reserved,balance.qty_held,
                  batch.expiration,batch.created AS received_at,batch.deleted AS batch_deleted,
                  location.active,location.pickable,location.deleted AS location_deleted,
                  location.barcode,plate.deleted AS plate_deleted,plate.barcode AS plate_barcode
           FROM inventory_balances balance
           JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
             AND batch.inventory_owner_id=balance.inventory_owner_id
             AND batch.id=balance.item_batch_id
           JOIN locations location ON location.tenant_id=balance.tenant_id
             AND location.facility_id=balance.facility_id AND location.id=balance.location_id
           LEFT JOIN license_plates plate ON plate.tenant_id=balance.tenant_id
             AND plate.inventory_owner_id=balance.inventory_owner_id
             AND plate.facility_id=balance.facility_id AND plate.id=balance.license_plate_id
           WHERE balance.tenant_id=$1 AND balance.id=ANY($2)
           ORDER BY balance.id FOR UPDATE OF balance"#,
    )
    .bind(tenant_id.get())
    .bind(&balance_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut candidates = rows
        .into_iter()
        .map(|row| {
            let available = row
                .try_get::<i64, _>("qty_on_hand")?
                .checked_sub(row.try_get("qty_reserved")?)
                .and_then(|value| value.checked_sub(row.try_get("qty_held").ok()?))
                .ok_or_else(|| AppError::internal("invalid substitute stock commitments"))?;
            let valid = available > 0
                && row.try_get::<i64, _>("item_id")? == policy.definition.substitute_item_id.get()
                && row.try_get::<String, _>("uom")? == policy.definition.substitute_uom.as_str()
                && row.try_get::<String, _>("status")? == "available"
                && row.try_get::<Option<Timestamp>, _>("deleted")?.is_none()
                && row
                    .try_get::<Option<Timestamp>, _>("batch_deleted")?
                    .is_none()
                && row
                    .try_get::<Option<Timestamp>, _>("expiration")?
                    .is_none_or(|expiration| expiration > occurred_at)
                && row
                    .try_get::<Option<Timestamp>, _>("location_deleted")?
                    .is_none()
                && row.try_get::<bool, _>("active")?
                && row.try_get::<bool, _>("pickable")?
                && row
                    .try_get::<Option<String>, _>("barcode")?
                    .is_some_and(|barcode| !barcode.trim().is_empty())
                && row
                    .try_get::<Option<i64>, _>("license_plate_id")?
                    .is_none_or(|_| {
                        row.try_get::<Option<Timestamp>, _>("plate_deleted")
                            .ok()
                            .flatten()
                            .is_none()
                            && row
                                .try_get::<Option<String>, _>("plate_barcode")
                                .ok()
                                .flatten()
                                .is_some_and(|barcode| !barcode.trim().is_empty())
                    });
            if !valid {
                return Err(AppError::conflict(
                    "approved substitute inventory changed during planning",
                ));
            }
            Ok((
                row.try_get::<Option<Timestamp>, _>("expiration")?,
                row.try_get::<Timestamp, _>("received_at")?,
                Candidate {
                    balance_id: InventoryBalanceId::new(row.try_get("id")?).map_err(internal)?,
                    batch_id: ItemBatchId::new(row.try_get("item_batch_id")?).map_err(internal)?,
                    location_id: LocationId::new(row.try_get("location_id")?).map_err(internal)?,
                    plate_id: row
                        .try_get::<Option<i64>, _>("license_plate_id")?
                        .map(LicensePlateId::new)
                        .transpose()
                        .map_err(internal)?,
                    available,
                },
            ))
        })
        .collect::<AppResult<Vec<_>>>()?;
    candidates.sort_by(|left, right| {
        left.0
            .is_none()
            .cmp(&right.0.is_none())
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.balance_id.get().cmp(&right.2.balance_id.get()))
    });
    let mut remaining = demand.get();
    let mut planned = Vec::new();
    for (_, _, candidate) in candidates {
        if remaining == 0 {
            break;
        }
        let quantity = candidate.available.min(remaining);
        remaining -= quantity;
        planned.push(PlannedCandidate {
            candidate,
            quantity: SubstitutionQuantity::new(quantity).map_err(internal)?,
        });
    }
    if remaining != 0 {
        return Err(AppError::conflict(
            "approved substitute inventory cannot fulfill the exact converted quantity",
        ));
    }
    Ok(planned)
}

async fn lock_ids_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    tenant_id: TenantId,
    ids: &[i64],
    update: bool,
) -> AppResult<()> {
    let mut expected = ids.to_vec();
    expected.sort_unstable();
    expected.dedup();
    if expected.is_empty() {
        return Ok(());
    }
    let lock = if update { "UPDATE" } else { "SHARE" };
    let query =
        format!("SELECT id FROM {table} WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR {lock}");
    let actual = sqlx::query_scalar::<_, i64>(&query)
        .bind(tenant_id.get())
        .bind(&expected)
        .fetch_all(&mut **tx)
        .await?;
    if actual == expected {
        Ok(())
    } else {
        Err(AppError::conflict(
            "substitute inventory dimensions changed while locking",
        ))
    }
}

async fn reserve_identity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    column: &str,
) -> AppResult<i64> {
    let query = format!("SELECT nextval(pg_get_serial_sequence('{table}','{column}'))");
    Ok(sqlx::query_scalar(&query).fetch_one(&mut **tx).await?)
}

#[allow(clippy::too_many_arguments)]
async fn insert_substitution_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: i64,
    command: &SubstitutePickShortageCommand,
    shortage: &LockedShortage,
    policy: &LockedPolicy,
    transition: &wareboxes_domain::SubstitutePickShortageTransition,
    substitute_line_id: i64,
    substitute_reservation_id: i64,
    allocation_count: i64,
    occurred_at: Timestamp,
) -> AppResult<ItemSubstitutionId> {
    ItemSubstitutionId::new(
        sqlx::query_scalar(
            r#"INSERT INTO pick_shortage_substitutions (
                 tenant_id,inventory_owner_id,facility_id,order_release_id,order_id,
                 pick_shortage_id,policy_id,policy_revision,source_order_item_id,
                 source_reservation_id,source_item_id,source_uom,substitute_order_item_id,
                 substitute_reservation_id,substitute_item_id,substitute_uom,
                 accepted_source_qty,substitute_qty,expected_shortage_revision,
                 resulting_shortage_revision,expected_order_revision,resulting_order_revision,
                 allocation_count,reason_code,note,substituted_by_user_id,substituted_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,
                       $17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27) RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(shortage.facility_id.get())
        .bind(shortage.order_release_id)
        .bind(shortage.order_id.get())
        .bind(shortage.id.get())
        .bind(policy.id.get())
        .bind(policy.revision.get())
        .bind(shortage.order_item_id.get())
        .bind(shortage.reservation_id)
        .bind(shortage.item_id.get())
        .bind(shortage.uom.as_str())
        .bind(substitute_line_id)
        .bind(substitute_reservation_id)
        .bind(policy.definition.substitute_item_id.get())
        .bind(policy.definition.substitute_uom.as_str())
        .bind(transition.accepted_source_quantity.get())
        .bind(transition.substitute_quantity.get())
        .bind(command.expected_shortage_revision.get())
        .bind(transition.shortage_revision.get())
        .bind(command.expected_order_revision.get())
        .bind(transition.order_revision.get())
        .bind(allocation_count)
        .bind(command.details.reason.as_str())
        .bind(command.details.note.as_ref().map(|note| note.as_str()))
        .bind(actor_id)
        .bind(occurred_at)
        .fetch_one(&mut **tx)
        .await?,
    )
    .map_err(internal)
}

#[allow(clippy::too_many_arguments)]
async fn insert_substitute_demand_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: i64,
    shortage: &LockedShortage,
    policy: &LockedPolicy,
    quantity: SubstitutionQuantity,
    line_id: i64,
    reservation_id: i64,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let line_number: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(line_number),0)+1 FROM order_items WHERE tenant_id=$1 AND order_id=$2",
    )
    .bind(tenant_id.get())
    .bind(shortage.order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO order_items (
             id,tenant_id,inventory_owner_id,created,line_key,line_number,qty,item_id,order_id,uom)
           OVERRIDING SYSTEM VALUE VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
    )
    .bind(line_id)
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(occurred_at)
    .bind(format!("SUB-{}-{}", shortage.id, policy.id))
    .bind(line_number)
    .bind(quantity.get())
    .bind(policy.definition.substitute_item_id.get())
    .bind(shortage.order_id.get())
    .bind(policy.definition.substitute_uom.as_str())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO inventory_reservations (
             id,tenant_id,inventory_owner_id,created,modified,created_by,order_id,
             order_item_id,facility_id,item_id,uom,qty,status)
           OVERRIDING SYSTEM VALUE VALUES (
             $1,$2,$3,$4,$4,$5,$6,$7,$8,$9,$10,$11,'active')"#,
    )
    .bind(reservation_id)
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(occurred_at)
    .bind(actor_id)
    .bind(shortage.order_id.get())
    .bind(line_id)
    .bind(shortage.facility_id.get())
    .bind(policy.definition.substitute_item_id.get())
    .bind(policy.definition.substitute_uom.as_str())
    .bind(quantity.get())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_substitute_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: i64,
    shortage: &LockedShortage,
    policy: &LockedPolicy,
    substitution_id: ItemSubstitutionId,
    line_id: i64,
    reservation_id: i64,
    planned: &[PlannedCandidate],
    occurred_at: Timestamp,
) -> AppResult<Vec<SubstitutePickWorkReadModel>> {
    let base_sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(travel_sequence),0) FROM order_release_allocations WHERE tenant_id=$1 AND order_release_id=$2",
    )
    .bind(tenant_id.get())
    .bind(shortage.order_release_id)
    .fetch_one(&mut **tx)
    .await?;
    let mut work = Vec::with_capacity(planned.len());
    for (index, plan) in planned.iter().enumerate() {
        let sequence = base_sequence
            .checked_add(i64::try_from(index).map_err(|_| AppError::internal("sequence overflow"))?)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| AppError::internal("sequence overflow"))?;
        let allocation_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO inventory_allocations (
                 tenant_id,inventory_owner_id,created,modified,created_by,reservation_id,
                 inventory_balance_id,facility_id,location_id,license_plate_id,item_batch_id,
                 item_id,uom,inventory_status,qty,status,execution_stage)
               VALUES ($1,$2,$3,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'available',$13,
                       'allocated','pick_source') RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(occurred_at)
        .bind(actor_id)
        .bind(reservation_id)
        .bind(plan.candidate.balance_id.get())
        .bind(shortage.facility_id.get())
        .bind(plan.candidate.location_id.get())
        .bind(plan.candidate.plate_id.map(|id| id.get()))
        .bind(plan.candidate.batch_id.get())
        .bind(policy.definition.substitute_item_id.get())
        .bind(policy.definition.substitute_uom.as_str())
        .bind(plan.quantity.get())
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query(
            r#"INSERT INTO order_release_allocations (
                 tenant_id,inventory_owner_id,facility_id,order_release_id,order_id,
                 order_item_id,reservation_id,allocation_id,inventory_balance_id,
                 source_location_id,source_license_plate_id,item_batch_id,item_id,uom,
                 inventory_status,planned_qty,travel_sequence,source_kind,pick_shortage_id,
                 item_substitution_id)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'available',
                       $15,$16,'item_substitution',$17,$18)"#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(shortage.facility_id.get())
        .bind(shortage.order_release_id)
        .bind(shortage.order_id.get())
        .bind(line_id)
        .bind(reservation_id)
        .bind(allocation_id)
        .bind(plan.candidate.balance_id.get())
        .bind(plan.candidate.location_id.get())
        .bind(plan.candidate.plate_id.map(|id| id.get()))
        .bind(plan.candidate.batch_id.get())
        .bind(policy.definition.substitute_item_id.get())
        .bind(policy.definition.substitute_uom.as_str())
        .bind(plan.quantity.get())
        .bind(sequence)
        .bind(Option::<i64>::None)
        .bind(substitution_id.get())
        .execute(&mut **tx)
        .await?;
        let task_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO pick_tasks (
                 tenant_id,inventory_owner_id,facility_id,order_release_id,order_id,
                 order_item_id,reservation_id,source_allocation_id,destination_location_id,
                 created_at,status,priority,ship_by,task_timeout_seconds)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'open',$11,$12,$13)
               RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(shortage.facility_id.get())
        .bind(shortage.order_release_id)
        .bind(shortage.order_id.get())
        .bind(line_id)
        .bind(reservation_id)
        .bind(allocation_id)
        .bind(shortage.destination_location_id.get())
        .bind(occurred_at)
        .bind(if shortage.rush { 100_i64 } else { 0_i64 })
        .bind(shortage.ship_by)
        .bind(PICK_LEASE_SECONDS)
        .fetch_one(&mut **tx)
        .await?;
        let content_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO pick_task_contents (
                 tenant_id,inventory_owner_id,facility_id,task_id,order_release_id,order_id,
                 order_item_id,reservation_id,source_allocation_id,source_inventory_balance_id,
                 source_location_id,source_license_plate_id,item_batch_id,item_id,uom,
                 inventory_status,planned_qty,travel_sequence,state)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,
                       'available',$16,$17,'pending') RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(shortage.inventory_owner_id.get())
        .bind(shortage.facility_id.get())
        .bind(task_id)
        .bind(shortage.order_release_id)
        .bind(shortage.order_id.get())
        .bind(line_id)
        .bind(reservation_id)
        .bind(allocation_id)
        .bind(plan.candidate.balance_id.get())
        .bind(plan.candidate.location_id.get())
        .bind(plan.candidate.plate_id.map(|id| id.get()))
        .bind(plan.candidate.batch_id.get())
        .bind(policy.definition.substitute_item_id.get())
        .bind(policy.definition.substitute_uom.as_str())
        .bind(plan.quantity.get())
        .bind(sequence)
        .fetch_one(&mut **tx)
        .await?;
        work.push(SubstitutePickWorkReadModel {
            task_id: PickTaskId::new(task_id).map_err(internal)?,
            content_id: PickContentId::new(content_id).map_err(internal)?,
            inventory_allocation_id: InventoryAllocationId::new(allocation_id).map_err(internal)?,
            inventory_balance_id: plan.candidate.balance_id,
            source_location_id: plan.candidate.location_id,
            quantity: plan.quantity,
        });
    }
    Ok(work)
}

async fn resolve_shortage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: i64,
    shortage: &LockedShortage,
    revision: PickShortageRevision,
    accepted: SubstitutionQuantity,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE pick_shortages SET modified_at=$1,revision=$2,status='resolved',
                  resolution='substituted',accepted_substitute_qty=$3,
                  resolved_by_user_id=$4,resolved_at=$1
           WHERE tenant_id=$5 AND inventory_owner_id=$6 AND id=$7
             AND revision=$8 AND status='awaiting_inventory' AND resolution IS NULL
             AND accepted_short_qty=0 AND accepted_substitute_qty=0"#,
    )
    .bind(occurred_at)
    .bind(revision.get())
    .bind(accepted.get())
    .bind(actor_id)
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.id.get())
    .bind(shortage.revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict(
            "pick shortage changed during substitution",
        ))
    }
}

async fn update_order_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    revision: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        "UPDATE orders SET revision=revision+1 WHERE tenant_id=$1 AND id=$2 AND revision=$3 AND status='processing' AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict("order changed during substitution"))
    }
}

async fn require_replay_visibility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let substitution_id: Option<i64> = sqlx::query_scalar(
        r#"SELECT (result_json->>'substitution_id')::bigint
           FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3"#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let Some(substitution_id) = substitution_id else {
        return Ok(());
    };
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM pick_shortage_substitutions WHERE tenant_id=$1 AND id=$2",
    )
    .bind(prepared.tenant_id().get())
    .bind(substitution_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick shortage substitution"))?;
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    if scope.includes_inventory_owner(owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found("pick shortage substitution"))
    }
}

async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &SubstitutePickShortageResult,
) -> AppResult<()> {
    let ordering_key = format!("order:{}", result.order_id);
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "{ordering_key}:item-substitution:{}",
        result.substitution_id
    );
    let aggregate_id = result.order_id.to_string();
    let payload = serde_json::to_value(result).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(result.inventory_owner_id),
            facility_id: Some(result.facility_id),
            actor_user_id: Some(result.substituted_by.get()),
            event_key: &event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.pick.shortage_substituted",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.substituted_at,
        },
    )
    .await?;
    Ok(())
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}
