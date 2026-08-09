//! Versioned backorder policy and atomic pre-release shortage splitting.

use sqlx::Row;
use wareboxes_application::backorder::{
    BackorderPolicyReadModel, BackorderSplitLineReadModel, ConfigureBackorderPolicyCommand,
    ConfigureBackorderPolicyResult, SplitOrderBackorderCommand, SplitOrderBackorderResult,
    CONFIGURE_BACKORDER_POLICY_OPERATION, SPLIT_ORDER_BACKORDER_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    split_current_allocation_shortage, BackorderLineSnapshot, BackorderPolicyId,
    BackorderPolicyMode, BackorderPolicyRevision, BackorderSplitId, FacilityId, InventoryOwnerId,
    OrderId, OrderLineId, OrderRevision, OrderStatus, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::{insert_order_activity_tx, next_outbox_sequence_tx};

#[derive(Debug, Clone)]
pub(crate) struct ActiveBackorderPolicy {
    pub policy_id: BackorderPolicyId,
    pub mode: BackorderPolicyMode,
    pub revision: BackorderPolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

#[derive(Debug)]
struct LockedOrder {
    owner_id: InventoryOwnerId,
    order_key: String,
    status: OrderStatus,
    revision: OrderRevision,
    address_id: i64,
    rush: bool,
    ship_by: Option<Timestamp>,
}

#[derive(Debug)]
struct SplitLine {
    snapshot: BackorderLineSnapshot,
    line_key: String,
    item_id: i64,
    uom: String,
}

pub async fn configure_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureBackorderPolicyCommand,
) -> AppResult<ConfigureBackorderPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CONFIGURE_BACKORDER_POLICY_OPERATION, command)?;
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
    require_stored_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ConfigureBackorderPolicyResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    require_scope(
        &scope,
        command.inventory_owner_id.get(),
        command.facility_id.get(),
        "backorder policy",
    )?;
    lock_policy_key_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
    )
    .await?;
    require_active_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
    )
    .await?;
    let predecessor = active_policy_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        true,
    )
    .await?;
    match (command.expected_revision, predecessor.as_ref()) {
        (None, None) => {}
        (Some(expected), Some(current)) if expected == current.revision => {}
        (None, Some(_)) => {
            return Err(AppError::conflict(
                "backorder policy already exists at this scope",
            ));
        }
        _ => return Err(AppError::conflict("backorder policy revision is stale")),
    }
    let configured_at = now_iso();
    if let Some(current) = predecessor.as_ref() {
        let updated = sqlx::query(
            "UPDATE backorder_policies SET effective_to=$1 WHERE tenant_id=$2 AND id=$3 AND effective_to IS NULL",
        )
        .bind(configured_at)
        .bind(access.tenant_id.get())
        .bind(current.policy_id.get())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict("backorder policy changed"));
        }
    }
    let revision = predecessor.as_ref().map_or_else(
        || BackorderPolicyRevision::new(1).map_err(internal),
        |current| {
            current
                .revision
                .checked_next()
                .ok_or_else(|| AppError::internal("backorder policy revision overflow"))
        },
    )?;
    let policy_id = BackorderPolicyId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO backorder_policies (
                tenant_id, inventory_owner_id, facility_id, mode, revision,
                supersedes_policy_id, effective_from, configured_by_user_id, configured_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$7) RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(command.mode.as_str())
        .bind(revision.get())
        .bind(predecessor.as_ref().map(|policy| policy.policy_id.get()))
        .bind(configured_at)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    let result = ConfigureBackorderPolicyResult {
        policy_id,
        inventory_owner_id: command.inventory_owner_id,
        facility_id: command.facility_id,
        mode: command.mode,
        revision,
        configured_by: context.actor_id,
        configured_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        context.actor_id.get(),
        &format!(
            "backorder-policy:{}:{}",
            command.inventory_owner_id, command.facility_id
        ),
        "backorder_policy",
        policy_id.get(),
        "outbound.backorder.policy_configured",
        &format!("configured:{}", revision.get()),
        &serde_json::to_value(&result).map_err(internal)?,
        configured_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn active_policy(
    db: &Db,
    access: &TenantAccess,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<Option<BackorderPolicyReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "orders").await?;
    require_scope(
        &scope,
        owner_id.get(),
        facility_id.get(),
        "backorder policy",
    )?;
    let policy = active_policy_tx(&mut tx, access.tenant_id, owner_id, facility_id, false).await?;
    tx.commit().await?;
    Ok(policy.map(|policy| BackorderPolicyReadModel {
        policy_id: policy.policy_id,
        inventory_owner_id: owner_id,
        facility_id,
        mode: policy.mode,
        revision: policy.revision,
        configured_by: policy.configured_by,
        configured_at: policy.configured_at,
    }))
}

pub async fn split_shortage(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &SplitOrderBackorderCommand,
) -> AppResult<SplitOrderBackorderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, SPLIT_ORDER_BACKORDER_OPERATION, command)?;
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
    require_stored_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<SplitOrderBackorderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    if !scope.includes_facility(command.facility_id.get()) {
        return Err(AppError::not_found("order"));
    }
    let order = lock_order_tx(&mut tx, access.tenant_id, command.order_id, &scope).await?;
    if order.revision != command.expected_order_revision {
        return Err(AppError::conflict("order revision is stale"));
    }
    require_active_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        command.facility_id,
    )
    .await?;
    let policy = active_policy_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        command.facility_id,
        true,
    )
    .await?
    .ok_or_else(|| AppError::conflict("no active backorder policy is configured"))?;
    if policy.revision != command.expected_policy_revision {
        return Err(AppError::conflict("backorder policy revision is stale"));
    }
    require_no_hold_or_execution_tx(&mut tx, access.tenant_id, order.owner_id, command.order_id)
        .await?;
    let lines = lock_split_lines_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        command.order_id,
        command.facility_id,
    )
    .await?;
    let snapshots = lines.iter().map(|line| line.snapshot).collect::<Vec<_>>();
    let transition =
        split_current_allocation_shortage(order.status, order.revision, policy.mode, &snapshots)
            .map_err(|error| AppError::conflict(error.to_string()))?;
    let split_at = now_iso();
    let child_order_key = next_child_order_key_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        command.order_id,
        &order.order_key,
    )
    .await?;
    let child_order_id = insert_child_order_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        &order,
        &child_order_key,
        split_at,
    )
    .await?;
    let transition_lines = transition
        .lines
        .iter()
        .map(|transition_line| {
            let source = lines
                .iter()
                .find(|line| line.snapshot.order_line_id == transition_line.order_line_id)
                .ok_or_else(|| AppError::internal("backorder transition lost its source line"))?;
            Ok((source, transition_line))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let child_line_ids = insert_child_lines_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        child_order_id,
        split_at,
        &transition_lines,
    )
    .await?;
    update_parent_revision_tx(&mut tx, access.tenant_id, command.order_id, order.revision).await?;
    let split_id = insert_split_header_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        context.actor_id.get(),
        command,
        &order,
        &policy,
        child_order_id,
        &transition,
        split_at,
    )
    .await?;
    let result_lines = insert_split_lines_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        command.facility_id,
        split_id,
        command.order_id,
        child_order_id,
        &transition_lines,
        &child_line_ids,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        command.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "split {} units to backorder {}",
            transition.newly_backordered_quantity, child_order_key
        ),
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.owner_id,
        child_order_id.get(),
        Some(context.actor_id.get()),
        &format!("created from backorder split of {}", order.order_key),
    )
    .await?;
    let result = SplitOrderBackorderResult {
        split_id,
        policy_id: policy.policy_id,
        policy_revision: policy.revision,
        inventory_owner_id: order.owner_id,
        facility_id: command.facility_id,
        parent_order_id: command.order_id,
        parent_order_key: order.order_key,
        parent_status: OrderStatus::Open,
        parent_revision: transition.parent_revision,
        child_order_id,
        child_order_key,
        child_status: OrderStatus::Open,
        child_revision: transition.child_revision,
        original_quantity: transition.original_quantity,
        allocated_quantity: transition.allocated_quantity,
        previously_backordered_quantity: transition.previously_backordered_quantity,
        newly_backordered_quantity: transition.newly_backordered_quantity,
        parent_effective_quantity: transition.parent_effective_quantity,
        lines: result_lines,
        details: command.details.clone(),
        split_by: context.actor_id,
        split_at,
    };
    if !result.quantities_are_consistent() {
        return Err(AppError::internal(
            "backorder split quantities are inconsistent",
        ));
    }
    enqueue_split_events_tx(&mut tx, access.tenant_id, &result).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub(crate) async fn active_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    lock: bool,
) -> AppResult<Option<ActiveBackorderPolicy>> {
    let suffix = if lock { " FOR SHARE" } else { "" };
    let sql = format!(
        "SELECT id,mode,revision,configured_by_user_id,configured_at FROM backorder_policies WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3 AND effective_to IS NULL{suffix}"
    );
    sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .map(|row| {
            let mode: String = row.try_get("mode")?;
            Ok(ActiveBackorderPolicy {
                policy_id: BackorderPolicyId::new(row.try_get("id")?).map_err(internal)?,
                mode: BackorderPolicyMode::parse(&mode)
                    .ok_or_else(|| AppError::internal("backorder policy has invalid mode"))?,
                revision: BackorderPolicyRevision::new(row.try_get("revision")?)
                    .map_err(internal)?,
                configured_by: UserId::new(row.try_get("configured_by_user_id")?)
                    .map_err(internal)?,
                configured_at: row.try_get("configured_at")?,
            })
        })
        .transpose()
}

async fn lock_policy_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "backorder-policy:{tenant_id}:{owner_id}:{facility_id}"
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn require_active_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let found: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
            SELECT 1 FROM inventory_owner_facilities assignment
            JOIN inventory_owners owner ON owner.tenant_id=assignment.tenant_id
             AND owner.id=assignment.inventory_owner_id AND owner.deleted IS NULL
            JOIN facilities facility ON facility.tenant_id=assignment.tenant_id
             AND facility.id=assignment.facility_id AND facility.deleted IS NULL
            WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=$2
              AND assignment.facility_id=$3 AND assignment.deleted IS NULL)"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if found {
        Ok(())
    } else {
        Err(AppError::not_found("backorder policy"))
    }
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"SELECT inventory_owner_id,order_key,status,revision,address_id,rush,ship_by
           FROM orders WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
             AND ($3 OR inventory_owner_id=ANY($4)) FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    let status: String = row.try_get("status")?;
    Ok(LockedOrder {
        owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?,
        order_key: row.try_get("order_key")?,
        status: OrderStatus::parse(&status)
            .ok_or_else(|| AppError::internal("order has invalid status"))?,
        revision: OrderRevision::new(row.try_get("revision")?).map_err(internal)?,
        address_id: row.try_get("address_id")?,
        rush: row.try_get("rush")?,
        ship_by: row.try_get("ship_by")?,
    })
}

async fn require_no_hold_or_execution_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<()> {
    let blocked: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (SELECT 1 FROM order_holds WHERE tenant_id=$1
               AND inventory_owner_id=$2 AND order_id=$3 AND released_at IS NULL)
           OR EXISTS (SELECT 1 FROM order_releases WHERE tenant_id=$1
               AND inventory_owner_id=$2 AND order_id=$3)
           OR EXISTS (SELECT 1 FROM packing_sessions WHERE tenant_id=$1
               AND inventory_owner_id=$2 AND order_id=$3)
           OR EXISTS (SELECT 1 FROM shipments WHERE tenant_id=$1
               AND inventory_owner_id=$2 AND order_id=$3)"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if blocked {
        Err(AppError::conflict(
            "release holds and finish active execution before splitting a backorder",
        ))
    } else {
        Ok(())
    }
}

async fn lock_split_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    order_id: OrderId,
    facility_id: FacilityId,
) -> AppResult<Vec<SplitLine>> {
    let rows = sqlx::query(
        r#"
        SELECT line.id,line.line_key,line.item_id,line.uom,line.qty,
               COALESCE((SELECT SUM(split_line.newly_backordered_qty)
                 FROM order_backorder_split_lines split_line
                 WHERE split_line.tenant_id=line.tenant_id
                   AND split_line.inventory_owner_id=line.inventory_owner_id
                   AND split_line.parent_order_id=line.order_id
                   AND split_line.parent_order_item_id=line.id),0)::bigint AS backordered_qty,
               reservation.qty AS reservation_qty,
               COALESCE((SELECT SUM(allocation.qty)
                 FROM inventory_allocations allocation
                 WHERE allocation.tenant_id=reservation.tenant_id
                   AND allocation.inventory_owner_id=reservation.inventory_owner_id
                   AND allocation.reservation_id=reservation.id
                   AND allocation.status='allocated' AND allocation.deleted IS NULL),0)::bigint
                   AS allocated_qty
        FROM order_items line
        JOIN inventory_reservations reservation
          ON reservation.tenant_id=line.tenant_id
         AND reservation.inventory_owner_id=line.inventory_owner_id
         AND reservation.order_id=line.order_id AND reservation.order_item_id=line.id
         AND reservation.facility_id=$4 AND reservation.status='active'
         AND reservation.deleted IS NULL
        WHERE line.tenant_id=$1 AND line.inventory_owner_id=$2
          AND line.order_id=$3 AND line.deleted IS NULL
        ORDER BY line.line_number,line.id
        FOR UPDATE OF line, reservation
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(order_id.get())
    .bind(facility_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Err(AppError::conflict(
            "run allocation before splitting a backorder",
        ));
    }
    rows.into_iter()
        .map(|row| {
            let original: i64 = row.try_get("qty")?;
            let backordered: i64 = row.try_get("backordered_qty")?;
            let reservation: i64 = row.try_get("reservation_qty")?;
            if reservation != original {
                return Err(AppError::conflict(
                    "active reservation no longer matches original demand",
                ));
            }
            Ok(SplitLine {
                snapshot: BackorderLineSnapshot {
                    order_line_id: OrderLineId::new(row.try_get("id")?).map_err(internal)?,
                    original_quantity: original,
                    previously_backordered_quantity: backordered,
                    effective_quantity: original.checked_sub(backordered).ok_or_else(|| {
                        AppError::internal("backorder quantity exceeds original demand")
                    })?,
                    allocated_quantity: row.try_get("allocated_qty")?,
                },
                line_key: row.try_get("line_key")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
            })
        })
        .collect()
}

async fn next_child_order_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    parent_order_id: OrderId,
    parent_key: &str,
) -> AppResult<String> {
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint + 1 FROM order_backorder_splits WHERE tenant_id=$1 AND inventory_owner_id=$2 AND parent_order_id=$3",
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(parent_order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    let readable = format!("{parent_key}-B{sequence:03}");
    Ok(if readable.chars().count() <= 200 {
        readable
    } else {
        format!("BO-{}-{sequence:03}", parent_order_id.get())
    })
}

async fn insert_child_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    parent: &LockedOrder,
    child_key: &str,
    occurred_at: Timestamp,
) -> AppResult<OrderId> {
    OrderId::new(
        sqlx::query_scalar(
            r#"INSERT INTO orders (tenant_id,inventory_owner_id,order_key,created,rush,status,address_id,ship_by)
               VALUES ($1,$2,$3,$4,$5,'open',$6,$7) RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(child_key)
        .bind(occurred_at)
        .bind(parent.rush)
        .bind(parent.address_id)
        .bind(parent.ship_by)
        .fetch_one(&mut **tx)
        .await?,
    )
    .map_err(internal)
}

async fn insert_child_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    child_id: OrderId,
    occurred_at: Timestamp,
    lines: &[(&SplitLine, &wareboxes_domain::BackorderSplitLineTransition)],
) -> AppResult<Vec<OrderLineId>> {
    let mut ids = Vec::with_capacity(lines.len());
    for (index, (source, transition)) in lines.iter().enumerate() {
        let line_number = i64::try_from(index + 1)
            .map_err(|_| AppError::internal("backorder line count exceeds i64"))?;
        let id: i64 = sqlx::query_scalar(
            r#"INSERT INTO order_items (tenant_id,inventory_owner_id,created,line_key,line_number,qty,item_id,order_id,uom)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(occurred_at)
        .bind(&source.line_key)
        .bind(line_number)
        .bind(transition.newly_backordered_quantity)
        .bind(source.item_id)
        .bind(child_id.get())
        .bind(&source.uom)
        .fetch_one(&mut **tx)
        .await?;
        ids.push(OrderLineId::new(id).map_err(internal)?);
    }
    Ok(ids)
}

async fn update_parent_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    expected: OrderRevision,
) -> AppResult<()> {
    let updated = sqlx::query(
        "UPDATE orders SET revision=revision+1 WHERE tenant_id=$1 AND id=$2 AND status='open' AND revision=$3 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(expected.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict("order changed during backorder split"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_split_header_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    actor_id: i64,
    command: &SplitOrderBackorderCommand,
    order: &LockedOrder,
    policy: &ActiveBackorderPolicy,
    child_id: OrderId,
    transition: &wareboxes_domain::BackorderSplitTransition,
    occurred_at: Timestamp,
) -> AppResult<BackorderSplitId> {
    let line_count = i64::try_from(transition.lines.len())
        .map_err(|_| AppError::internal("backorder line count exceeds i64"))?;
    BackorderSplitId::new(
        sqlx::query_scalar(
            r#"INSERT INTO order_backorder_splits (
                 tenant_id,inventory_owner_id,facility_id,policy_id,policy_revision,
                 parent_order_id,child_order_id,expected_parent_revision,resulting_parent_revision,
                 child_revision,line_count,original_qty,allocated_qty,previously_backordered_qty,
                 newly_backordered_qty,parent_effective_qty,reason_code,note,split_by_user_id,split_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
               RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(command.facility_id.get())
        .bind(policy.policy_id.get())
        .bind(policy.revision.get())
        .bind(command.order_id.get())
        .bind(child_id.get())
        .bind(order.revision.get())
        .bind(transition.parent_revision.get())
        .bind(transition.child_revision.get())
        .bind(line_count)
        .bind(transition.original_quantity)
        .bind(transition.allocated_quantity)
        .bind(transition.previously_backordered_quantity)
        .bind(transition.newly_backordered_quantity)
        .bind(transition.parent_effective_quantity)
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
async fn insert_split_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    split_id: BackorderSplitId,
    parent_id: OrderId,
    child_id: OrderId,
    lines: &[(&SplitLine, &wareboxes_domain::BackorderSplitLineTransition)],
    child_line_ids: &[OrderLineId],
) -> AppResult<Vec<BackorderSplitLineReadModel>> {
    let mut result = Vec::with_capacity(lines.len());
    for ((source, transition), child_line_id) in lines.iter().zip(child_line_ids) {
        sqlx::query(
            r#"INSERT INTO order_backorder_split_lines (
                 tenant_id,inventory_owner_id,facility_id,backorder_split_id,parent_order_id,
                 child_order_id,parent_order_item_id,child_order_item_id,line_key,item_id,uom,
                 original_qty,allocated_qty,previously_backordered_qty,newly_backordered_qty,
                 resulting_parent_qty)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)"#,
        )
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(split_id.get())
        .bind(parent_id.get())
        .bind(child_id.get())
        .bind(source.snapshot.order_line_id.get())
        .bind(child_line_id.get())
        .bind(&source.line_key)
        .bind(source.item_id)
        .bind(&source.uom)
        .bind(transition.original_quantity)
        .bind(transition.allocated_quantity)
        .bind(transition.previously_backordered_quantity)
        .bind(transition.newly_backordered_quantity)
        .bind(transition.resulting_effective_quantity)
        .execute(&mut **tx)
        .await?;
        result.push(BackorderSplitLineReadModel {
            parent_order_line_id: source.snapshot.order_line_id,
            child_order_line_id: *child_line_id,
            line_key: source.line_key.clone(),
            item_id: source.item_id,
            uom: source.uom.clone(),
            original_quantity: transition.original_quantity,
            allocated_quantity: transition.allocated_quantity,
            previously_backordered_quantity: transition.previously_backordered_quantity,
            newly_backordered_quantity: transition.newly_backordered_quantity,
            resulting_parent_quantity: transition.resulting_effective_quantity,
        });
    }
    Ok(result)
}

async fn enqueue_split_events_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &SplitOrderBackorderResult,
) -> AppResult<()> {
    let payload = serde_json::to_value(result).map_err(internal)?;
    enqueue_event_tx(
        tx,
        tenant_id,
        result.inventory_owner_id,
        result.facility_id,
        result.split_by.get(),
        &format!("order:{}", result.parent_order_id),
        "order",
        result.parent_order_id.get(),
        "outbound.order.backorder_split",
        &format!("backorder-split:{}", result.split_id),
        &payload,
        result.split_at,
    )
    .await?;
    enqueue_event_tx(
        tx,
        tenant_id,
        result.inventory_owner_id,
        result.facility_id,
        result.split_by.get(),
        &format!("order:{}", result.child_order_id),
        "order",
        result.child_order_id.get(),
        "outbound.order.created_from_backorder",
        "created",
        &payload,
        result.split_at,
    )
    .await
}

async fn require_stored_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored = sqlx::query(
        r#"SELECT (result_json->>'policy_id')::bigint AS policy_id,
                  (result_json->>'split_id')::bigint AS split_id
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
    let split_id: Option<i64> = stored.try_get("split_id")?;
    let (owner_id, facility_id) = if let Some(split_id) = split_id {
        let row = sqlx::query(
            "SELECT inventory_owner_id,facility_id FROM order_backorder_splits WHERE tenant_id=$1 AND id=$2",
        )
        .bind(prepared.tenant_id().get())
        .bind(split_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("backorder split"))?;
        (
            row.try_get("inventory_owner_id")?,
            row.try_get("facility_id")?,
        )
    } else {
        let policy_id: i64 = stored
            .try_get::<Option<i64>, _>("policy_id")?
            .ok_or_else(|| AppError::internal("stored backorder result is invalid"))?;
        let row = sqlx::query(
            "SELECT inventory_owner_id,facility_id FROM backorder_policies WHERE tenant_id=$1 AND id=$2",
        )
        .bind(prepared.tenant_id().get())
        .bind(policy_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("backorder policy"))?;
        (
            row.try_get("inventory_owner_id")?,
            row.try_get("facility_id")?,
        )
    };
    require_scope(scope, owner_id, facility_id, "backorder")
}

fn require_scope(
    scope: &ScopeBindings,
    owner_id: i64,
    facility_id: i64,
    resource: &'static str,
) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found(resource))
    }
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_id: i64,
    ordering_key: &str,
    aggregate_type: &str,
    aggregate_id: i64,
    event_type: &str,
    event_suffix: &str,
    payload: &serde_json::Value,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let sequence = next_outbox_sequence_tx(tx, tenant_id, ordering_key).await?;
    let event_key = format!("{ordering_key}:{event_suffix}");
    let aggregate_id = aggregate_id.to_string();
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_id),
            event_key: &event_key,
            aggregate_type,
            aggregate_id: &aggregate_id,
            ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}
