use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::picking::{
    ClaimNextPickCommand, ClaimPickByIdCommand, PickClaim, PickClaimContent, PickExecutionEvidence,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryOwnerId, ItemBatchId,
    LicensePlateId, LocationId, OrderId, OrderLineId, OrderRevision, PickClusterId, PickContentId,
    PickContentState, PickExecutionMethod, PickQuantity, PickScanValue, PickTaskId, TenantId,
    Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

use super::policy::{decision_policy_from_task_row, policy_bindings, resolve_decision_policy_tx};
use super::{CLAIM_BY_ID_OPERATION, CLAIM_NEXT_OPERATION};

pub async fn claim_next(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ClaimNextPickCommand,
) -> AppResult<Option<PickClaim>> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLAIM_NEXT_OPERATION, &())?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    if let Some(claim) = prepared.replayed::<Option<PickClaim>>(&mut tx).await? {
        if let Some(claim) = claim.as_ref() {
            require_task_visible_tx(&mut tx, access.tenant_id, claim.task_id, &scope).await?;
        }
        tx.commit().await?;
        return Ok(claim);
    }

    let _ = command;
    release_expired_claims_tx(&mut tx, access.tenant_id, &scope).await?;
    release_inaccessible_claim_tx(&mut tx, access.tenant_id, context.actor_id.get(), &scope)
        .await?;
    if active_task_for_user_tx(&mut tx, access.tenant_id, context.actor_id.get())
        .await?
        .is_some()
    {
        return Err(AppError::conflict(
            "operator already has active pick work; resume or release it first",
        ));
    }

    let claimed_at = now_iso();
    let candidate = sqlx::query(
        r#"
        SELECT id,inventory_owner_id,facility_id
        FROM pick_tasks
        WHERE tenant_id = $1 AND status = 'open'
          AND assigned_user_id IS NULL
          AND NOT EXISTS (
            SELECT 1 FROM pick_cluster_members member
            JOIN pick_clusters cluster ON cluster.tenant_id=member.tenant_id
              AND cluster.id=member.cluster_id
            WHERE member.tenant_id=pick_tasks.tenant_id
              AND member.task_id=pick_tasks.id
              AND cluster.status IN('planned','in_progress')
          )
          AND ($2 OR facility_id = ANY($3))
          AND ($4 OR inventory_owner_id = ANY($5))
        ORDER BY priority DESC, ship_by ASC NULLS LAST, created_at, id
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let claim = match candidate {
        Some(candidate) => {
            let task_id: i64 = candidate.try_get("id")?;
            claim_open_task_tx(
                &mut tx,
                access.tenant_id,
                task_id,
                InventoryOwnerId::new(candidate.try_get("inventory_owner_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                FacilityId::new(candidate.try_get("facility_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                context.actor_id.get(),
                claimed_at,
            )
            .await?;
            Some(
                load_claim_tx(
                    &mut tx,
                    access.tenant_id,
                    PickTaskId::new(task_id)
                        .map_err(|error| AppError::internal(error.to_string()))?,
                    context.actor_id.get(),
                )
                .await?,
            )
        }
        None => None,
    };
    Ok(prepared.commit(tx, claim).await?)
}

pub async fn claim_by_id(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ClaimPickByIdCommand,
) -> AppResult<PickClaim> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLAIM_BY_ID_OPERATION, &command.task_id)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    if let Some(claim) = prepared.replayed::<PickClaim>(&mut tx).await? {
        require_task_visible_tx(&mut tx, access.tenant_id, command.task_id, &scope).await?;
        tx.commit().await?;
        return Ok(claim);
    }

    release_expired_claims_tx(&mut tx, access.tenant_id, &scope).await?;
    release_inaccessible_claim_tx(&mut tx, access.tenant_id, context.actor_id.get(), &scope)
        .await?;
    let row = sqlx::query(
        r#"
        SELECT status, assigned_user_id,
               lease_expires_at > statement_timestamp() AS lease_is_current,
               facility_id, inventory_owner_id,
               EXISTS (
                 SELECT 1 FROM pick_cluster_members member
                 JOIN pick_clusters cluster ON cluster.tenant_id=member.tenant_id
                   AND cluster.id=member.cluster_id
                 WHERE member.tenant_id=pick_tasks.tenant_id
                   AND member.task_id=pick_tasks.id
                   AND cluster.status IN('planned','in_progress')
               ) AS cluster_reserved
        FROM pick_tasks
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.task_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick task"))?;
    require_scope_row(&row, &scope)?;
    let status: String = row.try_get("status")?;
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    if status == "in_progress"
        && assigned_user_id == Some(context.actor_id.get())
        && row.try_get::<Option<bool>, _>("lease_is_current")? == Some(true)
    {
        let claim = load_claim_tx(
            &mut tx,
            access.tenant_id,
            command.task_id,
            context.actor_id.get(),
        )
        .await?;
        return Ok(prepared.commit(tx, claim).await?);
    }
    if status != "open" || assigned_user_id.is_some() {
        return Err(AppError::conflict("pick task cannot be claimed"));
    }
    if row.try_get::<bool, _>("cluster_reserved")? {
        return Err(AppError::conflict(
            "pick task is reserved for a cluster-cart route",
        ));
    }
    if active_task_for_user_tx(&mut tx, access.tenant_id, context.actor_id.get())
        .await?
        .is_some()
    {
        return Err(AppError::conflict(
            "operator already has active pick work; resume or release it first",
        ));
    }

    let claimed_at = now_iso();
    claim_open_task_tx(
        &mut tx,
        access.tenant_id,
        command.task_id.get(),
        InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        context.actor_id.get(),
        claimed_at,
    )
    .await?;
    let claim = load_claim_tx(
        &mut tx,
        access.tenant_id,
        command.task_id,
        context.actor_id.get(),
    )
    .await?;
    Ok(prepared.commit(tx, claim).await?)
}

pub async fn current(db: &Db, access: &TenantAccess) -> AppResult<Option<PickClaim>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    release_expired_claims_tx(&mut tx, access.tenant_id, &scope).await?;
    release_inaccessible_claim_tx(&mut tx, access.tenant_id, access.user_id.get(), &scope).await?;
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM pick_tasks
        WHERE tenant_id = $1 AND assigned_user_id = $2
          AND status = 'in_progress' AND lease_expires_at > statement_timestamp()
        ORDER BY id LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(access.user_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    let claim = match task_id {
        Some(task_id) => Some(
            load_claim_tx(
                &mut tx,
                access.tenant_id,
                PickTaskId::new(task_id).map_err(|error| AppError::internal(error.to_string()))?,
                access.user_id.get(),
            )
            .await?,
        ),
        None => None,
    };
    tx.commit().await?;
    Ok(claim)
}

pub(super) async fn load_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: PickTaskId,
    actor_user_id: i64,
) -> AppResult<PickClaim> {
    let row = sqlx::query(
        r#"
        SELECT task.inventory_owner_id, task.facility_id, task.order_id,
               task.priority, task.ship_by, task.lease_expires_at,
               task.destination_location_id, orders.order_key,
               orders.revision AS order_revision,
               destination.barcode AS destination_barcode,
               destination.name AS destination_name,
               task.pick_policy_source,task.pick_configuration_id,
               task.pick_configuration_revision,task.pick_scope_level,
               task.pick_inventory_owner_id,task.pick_facility_id,
               task.require_source_location_scan,task.require_item_scan,
               task.require_destination_container_scan,task.pick_policy_hash,
               cluster.id AS cluster_id,cluster.task_count AS cluster_task_count,
               cart.barcode AS cluster_cart_barcode,
               slot.code AS cluster_slot_code,member.sequence AS cluster_sequence,
               ARRAY(
                   SELECT plate.barcode
                   FROM outbound_order_containers container
                   INNER JOIN license_plates plate
                     ON plate.tenant_id=container.tenant_id
                    AND plate.inventory_owner_id=container.inventory_owner_id
                    AND plate.facility_id=container.facility_id
                    AND plate.id=container.license_plate_id
                    AND plate.deleted IS NULL
                   WHERE container.tenant_id=task.tenant_id
                     AND container.inventory_owner_id=task.inventory_owner_id
                     AND container.facility_id=task.facility_id
                     AND container.order_release_id=task.order_release_id
                     AND container.order_id=task.order_id
                     AND container.destination_location_id=task.destination_location_id
                     AND container.released_at IS NULL
                   ORDER BY container.id LIMIT 2
               ) AS destination_container_barcodes,
               destination.active AS destination_active,
               destination.pickable AS destination_pickable,
               content.id AS content_id, content.order_item_id,
               content.source_allocation_id, content.source_inventory_balance_id,
               content.item_batch_id, content.source_location_id,
               content.source_license_plate_id, content.item_id, content.uom,
               content.inventory_status, content.planned_qty, content.state,
               source.barcode AS source_barcode, source.name AS source_name,
               source.active AS source_active, source.pickable AS source_pickable,
               source_plate.barcode AS source_license_plate_barcode,
               source_plate.deleted AS source_license_plate_deleted,
               allocation.inventory_balance_id AS allocation_balance_id,
               allocation.location_id AS allocation_location_id,
               allocation.license_plate_id AS allocation_license_plate_id,
               allocation.item_batch_id AS allocation_batch_id,
               allocation.item_id AS allocation_item_id,
               allocation.uom AS allocation_uom,
               allocation.inventory_status AS allocation_status,
               allocation.qty AS allocation_qty,
               allocation.status AS allocation_lifecycle,
               allocation.execution_stage AS allocation_execution_stage,
               allocation.deleted AS allocation_deleted,
               balance.location_id AS balance_location_id,
               balance.license_plate_id AS balance_license_plate_id,
               balance.item_batch_id AS balance_batch_id,
               balance.item_id AS balance_item_id,
               balance.uom AS balance_uom, balance.status AS balance_status,
               balance.qty_on_hand, balance.qty_reserved,
               balance.deleted AS balance_deleted,
               batch.lot, batch.serial, batch.expiration,
               batch.deleted AS batch_deleted,
               item.description AS item_description,
               item.deleted AS item_deleted,
               ARRAY(
                   SELECT barcode.name FROM barcodes barcode
                   WHERE barcode.tenant_id = content.tenant_id
                     AND barcode.item_id = content.item_id
                     AND barcode.deleted IS NULL
                   ORDER BY barcode.id
               ) AS item_barcodes
        FROM pick_tasks task
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        INNER JOIN orders
          ON orders.tenant_id = task.tenant_id
         AND orders.inventory_owner_id = task.inventory_owner_id
         AND orders.id = task.order_id AND orders.deleted IS NULL
        INNER JOIN locations source
          ON source.tenant_id = content.tenant_id
         AND source.facility_id = content.facility_id
         AND source.id = content.source_location_id AND source.deleted IS NULL
        INNER JOIN locations destination
          ON destination.tenant_id = task.tenant_id
         AND destination.facility_id = task.facility_id
         AND destination.id = task.destination_location_id
         AND destination.deleted IS NULL
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id = content.tenant_id
         AND allocation.inventory_owner_id = content.inventory_owner_id
         AND allocation.id = content.source_allocation_id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = content.tenant_id
         AND balance.inventory_owner_id = content.inventory_owner_id
         AND balance.facility_id = content.facility_id
         AND balance.id = content.source_inventory_balance_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = content.tenant_id
         AND batch.inventory_owner_id = content.inventory_owner_id
         AND batch.id = content.item_batch_id
        INNER JOIN items item
          ON item.tenant_id = content.tenant_id AND item.id = content.item_id
        LEFT JOIN license_plates source_plate
          ON source_plate.tenant_id = content.tenant_id
         AND source_plate.inventory_owner_id = content.inventory_owner_id
         AND source_plate.facility_id = content.facility_id
         AND source_plate.id = content.source_license_plate_id
        LEFT JOIN pick_cluster_members member
          ON member.tenant_id=task.tenant_id AND member.task_id=task.id
        LEFT JOIN pick_clusters cluster
          ON cluster.tenant_id=member.tenant_id AND cluster.id=member.cluster_id
        LEFT JOIN pick_carts cart
          ON cart.tenant_id=cluster.tenant_id AND cart.facility_id=cluster.facility_id
         AND cart.id=cluster.cart_id
        LEFT JOIN pick_cart_slots slot
          ON slot.tenant_id=member.tenant_id AND slot.facility_id=member.facility_id
         AND slot.cart_id=member.cart_id AND slot.id=member.slot_id
        WHERE task.tenant_id = $1 AND task.id = $2
          AND task.status = 'in_progress' AND task.assigned_user_id = $3
          AND task.lease_expires_at > statement_timestamp()
          AND content.state = 'pending'
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id.get())
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("pick claim is no longer executable"))?;
    validate_claim_row(&row)?;

    let item_barcodes: Vec<String> = row.try_get("item_barcodes")?;
    let pick_policy = decision_policy_from_task_row(&row)?;
    let destination_container_barcodes: Vec<String> =
        row.try_get("destination_container_barcodes")?;
    let suggested_destination_license_plate_barcode = if !pick_policy
        .require_destination_container_scan
        && destination_container_barcodes.len() == 1
    {
        Some(
            PickScanValue::new(destination_container_barcodes[0].clone())
                .map_err(|error| AppError::internal(error.to_string()))?,
        )
    } else {
        None
    };
    let execution = match row.try_get::<Option<i64>, _>("cluster_id")? {
        Some(cluster_id) => PickExecutionEvidence {
            method: PickExecutionMethod::ClusterCart,
            cluster_id: Some(
                PickClusterId::new(cluster_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            cart_barcode: Some(row.try_get("cluster_cart_barcode")?),
            slot_code: Some(row.try_get("cluster_slot_code")?),
            sequence: Some(row.try_get("cluster_sequence")?),
            task_count: Some(row.try_get("cluster_task_count")?),
        },
        None => PickExecutionEvidence::discrete(),
    };
    Ok(PickClaim {
        task_id,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_key: row.try_get("order_key")?,
        order_revision: OrderRevision::new(row.try_get("order_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        priority: row.try_get("priority")?,
        ship_by: row.try_get("ship_by")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        destination_location_barcode: required_scan(
            row.try_get("destination_barcode")?,
            "destination location",
        )?,
        destination_location_name: row.try_get("destination_name")?,
        execution,
        pick_policy,
        suggested_destination_license_plate_barcode,
        content: PickClaimContent {
            content_id: PickContentId::new(row.try_get("content_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            order_line_id: OrderLineId::new(row.try_get("order_item_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            inventory_allocation_id: InventoryAllocationId::new(
                row.try_get("source_allocation_id")?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
            source_inventory_balance_id: InventoryBalanceId::new(
                row.try_get("source_inventory_balance_id")?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
            item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            source_location_id: LocationId::new(row.try_get("source_location_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            source_location_barcode: required_scan(
                row.try_get("source_barcode")?,
                "source location",
            )?,
            source_location_name: row.try_get("source_name")?,
            source_license_plate_id: row
                .try_get::<Option<i64>, _>("source_license_plate_id")?
                .map(LicensePlateId::new)
                .transpose()
                .map_err(|error| AppError::internal(error.to_string()))?,
            source_license_plate_barcode: row
                .try_get::<Option<String>, _>("source_license_plate_barcode")?
                .map(PickScanValue::new)
                .transpose()
                .map_err(|error| AppError::internal(error.to_string()))?,
            item_id: row.try_get("item_id")?,
            item_description: row.try_get("item_description")?,
            item_barcodes: item_barcodes
                .into_iter()
                .map(PickScanValue::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::internal(error.to_string()))?,
            uom: row.try_get("uom")?,
            lot: row.try_get("lot")?,
            serial: row.try_get("serial")?,
            expiration: row.try_get("expiration")?,
            planned_quantity: PickQuantity::new(row.try_get("planned_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            state: PickContentState::Pending,
        },
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn claim_open_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: i64,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_user_id: i64,
    claimed_at: Timestamp,
) -> AppResult<()> {
    let existing = sqlx::query(
        r#"SELECT pick_policy_source,pick_configuration_id,
        pick_configuration_revision,pick_scope_level,pick_inventory_owner_id,
        pick_facility_id,require_source_location_scan,require_item_scan,
        require_destination_container_scan,pick_policy_hash
        FROM pick_tasks WHERE tenant_id=$1 AND id=$2 FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await?;
    let policy = if existing
        .try_get::<Option<String>, _>("pick_policy_source")?
        .is_some()
    {
        decision_policy_from_task_row(&existing)?
    } else {
        resolve_decision_policy_tx(tx, tenant_id, inventory_owner_id, facility_id, claimed_at)
            .await?
    };
    let policy = policy_bindings(&policy);
    let updated = sqlx::query(
        r#"UPDATE pick_tasks
        SET status='in_progress',assigned_user_id=$1,claimed_at=$2,
            lease_expires_at=$2+make_interval(secs=>task_timeout_seconds::INT),
            pick_policy_source=$3,pick_configuration_id=$4,
            pick_configuration_revision=$5,pick_scope_level=$6,
            pick_inventory_owner_id=$7,pick_facility_id=$8,
            require_source_location_scan=$9,require_item_scan=$10,
            require_destination_container_scan=$11,pick_policy_hash=$12
        WHERE tenant_id=$13 AND id=$14 AND status='open' AND assigned_user_id IS NULL"#,
    )
    .bind(actor_user_id)
    .bind(claimed_at)
    .bind(policy.source)
    .bind(policy.configuration_id)
    .bind(policy.configuration_revision)
    .bind(policy.scope_level)
    .bind(policy.inventory_owner_id)
    .bind(policy.facility_id)
    .bind(policy.require_source_location_scan)
    .bind(policy.require_item_scan)
    .bind(policy.require_destination_container_scan)
    .bind(policy.policy_hash)
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("pick task cannot be claimed"));
    }
    Ok(())
}

fn validate_claim_row(row: &sqlx::postgres::PgRow) -> AppResult<()> {
    let planned_qty: i64 = row.try_get("planned_qty")?;
    let source_license_plate_id: Option<i64> = row.try_get("source_license_plate_id")?;
    let source_plate_barcode: Option<String> = row.try_get("source_license_plate_barcode")?;
    let item_barcodes: Vec<String> = row.try_get("item_barcodes")?;
    let valid = row.try_get::<bool, _>("destination_active")?
        && !row.try_get::<bool, _>("destination_pickable")?
        && row.try_get::<bool, _>("source_active")?
        && row.try_get::<bool, _>("source_pickable")?
        && row.try_get::<String, _>("state")? == "pending"
        && row.try_get::<i64, _>("allocation_balance_id")?
            == row.try_get::<i64, _>("source_inventory_balance_id")?
        && row.try_get::<i64, _>("allocation_location_id")?
            == row.try_get::<i64, _>("source_location_id")?
        && row.try_get::<Option<i64>, _>("allocation_license_plate_id")? == source_license_plate_id
        && row.try_get::<i64, _>("allocation_batch_id")?
            == row.try_get::<i64, _>("item_batch_id")?
        && row.try_get::<i64, _>("allocation_item_id")? == row.try_get::<i64, _>("item_id")?
        && row.try_get::<String, _>("allocation_uom")? == row.try_get::<String, _>("uom")?
        && row.try_get::<String, _>("allocation_status")? == "available"
        && row.try_get::<i64, _>("allocation_qty")? == planned_qty
        && row.try_get::<String, _>("allocation_lifecycle")? == "allocated"
        && row.try_get::<String, _>("allocation_execution_stage")? == "pick_source"
        && row
            .try_get::<Option<Timestamp>, _>("allocation_deleted")?
            .is_none()
        && row.try_get::<i64, _>("balance_location_id")?
            == row.try_get::<i64, _>("source_location_id")?
        && row.try_get::<Option<i64>, _>("balance_license_plate_id")? == source_license_plate_id
        && row.try_get::<i64, _>("balance_batch_id")? == row.try_get::<i64, _>("item_batch_id")?
        && row.try_get::<i64, _>("balance_item_id")? == row.try_get::<i64, _>("item_id")?
        && row.try_get::<String, _>("balance_uom")? == row.try_get::<String, _>("uom")?
        && row.try_get::<String, _>("balance_status")? == "available"
        && row.try_get::<i64, _>("qty_on_hand")? >= planned_qty
        && row.try_get::<i64, _>("qty_reserved")? >= planned_qty
        && row
            .try_get::<Option<Timestamp>, _>("balance_deleted")?
            .is_none()
        && row
            .try_get::<Option<Timestamp>, _>("batch_deleted")?
            .is_none()
        && row
            .try_get::<Option<Timestamp>, _>("item_deleted")?
            .is_none()
        && !item_barcodes.is_empty()
        && source_license_plate_id.is_none_or(|_| {
            row.try_get::<Option<Timestamp>, _>("source_license_plate_deleted")
                .ok()
                .flatten()
                .is_none()
                && source_plate_barcode.is_some_and(|value| !value.trim().is_empty())
        });
    if !valid {
        return Err(AppError::conflict("pick claim is no longer executable"));
    }
    Ok(())
}

fn required_scan(value: Option<String>, label: &str) -> AppResult<PickScanValue> {
    value
        .ok_or_else(|| AppError::conflict(format!("{label} must have a scannable barcode")))
        .and_then(|value| {
            PickScanValue::new(value).map_err(|error| AppError::conflict(error.to_string()))
        })
}

pub(super) async fn require_task_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: PickTaskId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT facility_id, inventory_owner_id FROM pick_tasks WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(task_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick task"))?;
    require_scope_row(&row, scope)
}

fn require_scope_row(row: &sqlx::postgres::PgRow, scope: &ScopeBindings) -> AppResult<()> {
    if !scope.includes_facility(row.try_get("facility_id")?)
        || !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
    {
        return Err(AppError::not_found("pick task"));
    }
    Ok(())
}

pub(super) async fn active_task_for_user_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
) -> AppResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT id FROM pick_tasks
        WHERE tenant_id = $1 AND assigned_user_id = $2 AND status = 'in_progress'
        ORDER BY id LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?)
}

pub(super) async fn release_expired_claims_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE pick_tasks
        SET status = 'open', assigned_user_id = NULL, claimed_at = NULL,
            lease_expires_at = NULL, last_released_at = statement_timestamp(),
            last_release_reason = 'lease_expired', last_release_note = NULL,
            release_count = release_count + 1
        WHERE tenant_id = $1 AND status = 'in_progress'
          AND lease_expires_at <= statement_timestamp()
          AND ($2 OR facility_id = ANY($3))
          AND ($4 OR inventory_owner_id = ANY($5))
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn release_inaccessible_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE pick_tasks
        SET status = 'open', assigned_user_id = NULL, claimed_at = NULL,
            lease_expires_at = NULL, last_released_at = statement_timestamp(),
            last_release_reason = 'scope_revoked', last_release_note = NULL,
            release_count = release_count + 1
        WHERE tenant_id = $1 AND assigned_user_id = $2 AND status = 'in_progress'
          AND (NOT $3 AND facility_id <> ALL($4)
               OR NOT $5 AND inventory_owner_id <> ALL($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(actor_user_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
