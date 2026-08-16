use sqlx::Row;
use wareboxes_application::replenishment::{
    ReplenishmentLatestPlanReadModel, ReplenishmentLocationReadModel, ReplenishmentPolicyPage,
    ReplenishmentPolicyPageFilter, ReplenishmentPolicyReadinessReadModel, ReplenishmentWorkPage,
    ReplenishmentWorkPageFilter, ReplenishmentWorkReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    plan_replenishment, CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId,
    ItemBatchId, LocationId, ReplenishmentMoveQuantity, ReplenishmentPlanId,
    ReplenishmentPlanningOutcome, ReplenishmentPlanningSnapshot, ReplenishmentPolicyId,
    ReplenishmentPolicyRevision, ReplenishmentScanValue, ReplenishmentUom, ReplenishmentWorkId,
    ReplenishmentWorkStatus, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

use super::{decision_policy::decision_policy_from_readiness_row, level, policy_from_row};

pub async fn policy_page(
    db: &Db,
    access: &TenantAccess,
    filter: ReplenishmentPolicyPageFilter,
) -> AppResult<ReplenishmentPolicyPage> {
    let offset = i64::try_from(filter.offset)
        .map_err(|_| AppError::bad_request("replenishment policy page offset is invalid"))?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    let fetch_limit = i64::from(filter.limit) + 1;
    let rows = sqlx::query(
        r#"
        WITH readiness AS (
        SELECT policy.id, policy.tenant_id, policy.inventory_owner_id, policy.facility_id,
               policy.pick_face_location_id, policy.item_id, policy.uom,
               policy.minimum_qty, policy.target_qty, policy.revision,
               policy.effective_from, policy.effective_to,
               owner.name AS inventory_owner_name, facility.name AS facility_name,
               item.description AS item_description,
               (SELECT barcode.name FROM barcodes barcode
                WHERE barcode.tenant_id=policy.tenant_id AND barcode.item_id=policy.item_id
                  AND barcode.deleted IS NULL ORDER BY barcode.id LIMIT 1) AS primary_sku,
               pick_face.barcode AS pick_face_barcode, pick_face.name AS pick_face_name,
               ARRAY(SELECT source.source_location_id
                 FROM replenishment_policy_sources source
                 WHERE source.tenant_id=policy.tenant_id AND source.policy_id=policy.id
                 ORDER BY source.source_sequence) AS source_ids,
               (SELECT COALESCE(sum(GREATEST(balance.qty_on_hand-balance.qty_reserved-balance.qty_held,0)),0)::bigint
                FROM inventory_balances balance
                JOIN locations location ON location.tenant_id=balance.tenant_id
                  AND location.facility_id=balance.facility_id AND location.id=balance.location_id
                WHERE balance.tenant_id=policy.tenant_id
                  AND balance.inventory_owner_id=policy.inventory_owner_id
                  AND balance.facility_id=policy.facility_id
                  AND balance.location_id=policy.pick_face_location_id
                  AND balance.item_id=policy.item_id AND balance.uom=policy.uom
                  AND balance.license_plate_id IS NULL AND balance.status='available'
                  AND balance.deleted IS NULL AND location.deleted IS NULL AND location.active
                  AND NULLIF(btrim(location.barcode),'') IS NOT NULL
                  AND location.pickable AND NOT location.receivable) AS pick_face_free,
               (SELECT COALESCE(sum(task.planned_qty),0)::bigint
                FROM replenishment_tasks task
                WHERE task.tenant_id=policy.tenant_id AND task.policy_id=policy.id
                  AND task.closed_at IS NULL) AS active_inbound,
               (SELECT COALESCE(sum(GREATEST(
                    reservation.qty-COALESCE(disposition.accepted,0)
                      -COALESCE(backorder.qty,0)-COALESCE(allocation.allocated,0),0
                  )),0)::bigint
                FROM inventory_reservations reservation
                LEFT JOIN LATERAL (SELECT sum(value.accepted_short_qty)::bigint accepted
                  FROM pick_short_ship_dispositions value
                  WHERE value.tenant_id=reservation.tenant_id
                    AND value.inventory_owner_id=reservation.inventory_owner_id
                    AND value.reservation_id=reservation.id) disposition ON true
                LEFT JOIN LATERAL (SELECT sum(value.newly_backordered_qty)::bigint qty
                  FROM order_backorder_split_lines value
                  WHERE value.tenant_id=reservation.tenant_id
                    AND value.inventory_owner_id=reservation.inventory_owner_id
                    AND value.parent_order_id=reservation.order_id
                    AND value.parent_order_item_id=reservation.order_item_id) backorder ON true
                LEFT JOIN LATERAL (SELECT sum(value.qty)::bigint allocated
                  FROM inventory_allocations value
                  WHERE value.tenant_id=reservation.tenant_id
                    AND value.inventory_owner_id=reservation.inventory_owner_id
                    AND value.reservation_id=reservation.id AND value.status='allocated'
                    AND value.deleted IS NULL) allocation ON true
                WHERE reservation.tenant_id=policy.tenant_id
                  AND reservation.inventory_owner_id=policy.inventory_owner_id
                  AND reservation.facility_id=policy.facility_id
                  AND reservation.item_id=policy.item_id AND reservation.uom=policy.uom
                  AND reservation.status='active' AND reservation.deleted IS NULL) AS unallocated_demand,
               (SELECT COALESCE(sum(GREATEST(balance.qty_on_hand-balance.qty_reserved-balance.qty_held,0)),0)::bigint
                FROM inventory_balances balance
                JOIN locations location ON location.tenant_id=balance.tenant_id
                  AND location.facility_id=balance.facility_id AND location.id=balance.location_id
                WHERE balance.tenant_id=policy.tenant_id
                  AND balance.inventory_owner_id=policy.inventory_owner_id
                  AND balance.facility_id=policy.facility_id
                  AND balance.location_id IN (SELECT source.source_location_id
                    FROM replenishment_policy_sources source
                    WHERE source.tenant_id=policy.tenant_id AND source.policy_id=policy.id)
                  AND balance.item_id=policy.item_id AND balance.uom=policy.uom
                  AND balance.license_plate_id IS NULL AND balance.status='available'
                  AND balance.deleted IS NULL AND location.deleted IS NULL AND location.active
                  AND NULLIF(btrim(location.barcode),'') IS NOT NULL
                  AND NOT location.pickable AND NOT location.receivable
                  AND NOT EXISTS (SELECT 1 FROM loose_inventory_movement_claims claim
                    WHERE claim.tenant_id=balance.tenant_id
                      AND claim.inventory_owner_id=balance.inventory_owner_id
                      AND claim.facility_id=balance.facility_id
                      AND claim.source_inventory_balance_id=balance.id
                      AND claim.released_at IS NULL)) AS reserve_free,
               active.work_count AS active_work_count,
               active.work_quantity AS active_work_quantity,
               decision_config.id AS decision_configuration_id,
               decision_config.revision AS decision_configuration_revision,
               decision_config.scope_level AS decision_scope_level,
               decision_config.inventory_owner_id AS decision_inventory_owner_id,
               decision_config.facility_id AS decision_facility_id,
               decision_config.definition AS decision_definition,
               latest.id AS latest_plan_id, latest.outcome AS latest_plan_outcome,
               latest.planned_qty AS latest_plan_planned_qty,
               latest.target_gap_qty-latest.planned_qty AS latest_plan_remaining_qty,
               latest.planned_by_user_id AS latest_plan_planned_by,
               latest.planned_at AS latest_plan_planned_at
        FROM replenishment_policies policy
        JOIN inventory_owners owner ON owner.tenant_id=policy.tenant_id
          AND owner.id=policy.inventory_owner_id AND owner.deleted IS NULL
        JOIN facilities facility ON facility.tenant_id=policy.tenant_id
          AND facility.id=policy.facility_id AND facility.deleted IS NULL
        JOIN items item ON item.tenant_id=policy.tenant_id
          AND item.id=policy.item_id AND item.deleted IS NULL
        JOIN locations pick_face ON pick_face.tenant_id=policy.tenant_id
          AND pick_face.facility_id=policy.facility_id AND pick_face.id=policy.pick_face_location_id
          AND pick_face.deleted IS NULL
        LEFT JOIN LATERAL (
          SELECT count(*)::bigint AS work_count,
                 COALESCE(sum(task.planned_qty),0)::bigint AS work_quantity
          FROM replenishment_tasks task
          WHERE task.tenant_id=policy.tenant_id AND task.policy_id=policy.id
            AND task.closed_at IS NULL
        ) active ON true
        LEFT JOIN LATERAL (
          SELECT configuration.id,configuration.revision,configuration.scope_level,
                 configuration.inventory_owner_id,configuration.facility_id,
                 configuration.definition
          FROM configuration_versions configuration
          WHERE configuration.tenant_id=policy.tenant_id
            AND configuration.kind='replenishment' AND configuration.status='active'
            AND configuration.activated_at<=transaction_timestamp()
            AND configuration.effective_from<=transaction_timestamp()
            AND (configuration.effective_until IS NULL
              OR configuration.effective_until>transaction_timestamp())
            AND (configuration.inventory_owner_id IS NULL
              OR configuration.inventory_owner_id=policy.inventory_owner_id)
            AND (configuration.facility_id IS NULL
              OR configuration.facility_id=policy.facility_id)
          ORDER BY CASE configuration.scope_level
            WHEN 'owner_facility' THEN 2
            WHEN 'inventory_owner' THEN 1
            WHEN 'facility' THEN 1
            ELSE 0 END DESC,
            configuration.effective_from DESC,configuration.revision DESC,
            configuration.id DESC
          LIMIT 1
        ) decision_config ON true
        LEFT JOIN LATERAL (
          SELECT run.id,run.outcome,run.planned_qty,run.target_gap_qty,
                 run.planned_by_user_id,run.planned_at
          FROM replenishment_plan_runs run
          WHERE run.tenant_id=policy.tenant_id AND run.policy_id=policy.id
          ORDER BY run.id DESC LIMIT 1
        ) latest ON true
        WHERE policy.tenant_id=$1 AND policy.effective_to IS NULL
          AND ($2 OR policy.facility_id=ANY($3))
          AND ($4 OR policy.inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR policy.facility_id=$6)
          AND ($7::bigint IS NULL OR policy.inventory_owner_id=$7)
          AND ($8::bigint IS NULL OR policy.item_id=$8)
          AND ($9::bigint IS NULL OR policy.pick_face_location_id=$9)
        ), resolved AS (
          SELECT readiness.*,
                 CASE WHEN readiness.decision_configuration_id IS NULL
                   THEN readiness.minimum_qty
                   ELSE floor(readiness.target_qty::numeric
                     *(readiness.decision_definition->>'minimum_percent')::numeric/100)::bigint
                 END AS effective_minimum_qty,
                 CASE WHEN readiness.decision_configuration_id IS NULL
                   THEN readiness.target_qty
                   ELSE ceil(readiness.target_qty::numeric
                     *(readiness.decision_definition->>'target_percent')::numeric/100)::bigint
                 END AS effective_target_qty,
                 CASE WHEN readiness.decision_configuration_id IS NULL
                     OR (readiness.decision_definition->>'include_inbound_projection')::boolean
                   THEN readiness.active_inbound ELSE 0
                 END AS included_active_inbound
          FROM readiness
        ), decision AS (
          SELECT resolved.*,
                 resolved.pick_face_free+resolved.included_active_inbound AS projected_free,
                 GREATEST(resolved.effective_target_qty,resolved.unallocated_demand)
                   AS required_level
          FROM resolved
        ), sortable AS (
          SELECT decision.*,
                 CASE
                   WHEN decision.projected_free < decision.effective_minimum_qty
                     OR decision.projected_free < decision.unallocated_demand
                   THEN GREATEST(decision.required_level-decision.projected_free,0)
                   ELSE 0
                 END AS target_gap_sort
          FROM decision
        ), ranked AS (
          SELECT sortable.*,
               CASE
                 WHEN sortable.target_gap_sort=0 THEN 3
                 WHEN LEAST(sortable.target_gap_sort,sortable.reserve_free)=0 THEN 0
                 WHEN LEAST(sortable.target_gap_sort,sortable.reserve_free)<sortable.target_gap_sort THEN 1
                 ELSE 2
               END AS outcome_sort
          FROM sortable
        )
        SELECT ranked.*
        FROM ranked
        ORDER BY
          CASE WHEN $10='inventory_owner' AND $11 THEN LOWER(ranked.inventory_owner_name) END ASC,
          CASE WHEN $10='inventory_owner' AND NOT $11 THEN LOWER(ranked.inventory_owner_name) END DESC,
          CASE WHEN $10='facility' AND $11 THEN LOWER(ranked.facility_name) END ASC,
          CASE WHEN $10='facility' AND NOT $11 THEN LOWER(ranked.facility_name) END DESC,
          CASE WHEN $10='item' AND $11 THEN ranked.item_id END ASC,
          CASE WHEN $10='item' AND NOT $11 THEN ranked.item_id END DESC,
          CASE WHEN $10='pick_face' AND $11 THEN LOWER(ranked.pick_face_barcode) END ASC,
          CASE WHEN $10='pick_face' AND NOT $11 THEN LOWER(ranked.pick_face_barcode) END DESC,
          CASE WHEN $10='projected' AND $11 THEN ranked.projected_free END ASC,
          CASE WHEN $10='projected' AND NOT $11 THEN ranked.projected_free END DESC,
          CASE WHEN $10='demand' AND $11 THEN ranked.unallocated_demand END ASC,
          CASE WHEN $10='demand' AND NOT $11 THEN ranked.unallocated_demand END DESC,
          CASE WHEN $10='reserve' AND $11 THEN ranked.reserve_free END ASC,
          CASE WHEN $10='reserve' AND NOT $11 THEN ranked.reserve_free END DESC,
          CASE WHEN $10='target_gap' AND $11 THEN ranked.target_gap_sort END ASC,
          CASE WHEN $10='target_gap' AND NOT $11 THEN ranked.target_gap_sort END DESC,
          CASE WHEN $10='outcome' AND $11 THEN ranked.outcome_sort END ASC,
          CASE WHEN $10='outcome' AND NOT $11 THEN ranked.outcome_sort END DESC,
          CASE WHEN $10='active_work' AND $11 THEN ranked.active_work_quantity END ASC,
          CASE WHEN $10='active_work' AND NOT $11 THEN ranked.active_work_quantity END DESC,
          CASE WHEN $11 THEN ranked.id END ASC,
          CASE WHEN NOT $11 THEN ranked.id END DESC
        OFFSET $12 LIMIT $13
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(filter.inventory_owner_id.map(|id| id.get()))
    .bind(filter.item_id.map(|id| id.get()))
    .bind(filter.pick_face_location_id.map(|id| id.get()))
    .bind(filter.sort.as_str())
    .bind(filter.direction.is_ascending())
    .bind(offset)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(filter.limit);
    let rows = rows.into_iter().take(usize::from(filter.limit));
    let mut items = Vec::new();
    for row in rows {
        let id = ReplenishmentPolicyId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let sources: Vec<i64> = row.try_get("source_ids")?;
        let policy = policy_from_row(&row, sources)?;
        let decision_policy =
            decision_policy_from_readiness_row(&row, policy.definition.thresholds())?;
        let observed_active_inbound = level(row.try_get("active_inbound")?)?;
        let snapshot = ReplenishmentPlanningSnapshot::new(
            level(row.try_get("pick_face_free")?)?,
            level(row.try_get("included_active_inbound")?)?,
            level(row.try_get("unallocated_demand")?)?,
            level(row.try_get("reserve_free")?)?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?;
        let decision = plan_replenishment(
            decision_policy
                .effective_thresholds()
                .map_err(|error| AppError::internal(error.to_string()))?,
            snapshot,
        );
        items.push(ReplenishmentPolicyReadinessReadModel {
            policy_id: id,
            revision: policy.revision,
            definition: policy.definition,
            decision_policy,
            inventory_owner_name: row.try_get("inventory_owner_name")?,
            facility_name: row.try_get("facility_name")?,
            item_description: row.try_get("item_description")?,
            primary_sku: row.try_get("primary_sku")?,
            pick_face: ReplenishmentLocationReadModel {
                location_id: LocationId::new(row.try_get("pick_face_location_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                barcode: scan_db(row.try_get("pick_face_barcode")?)?,
                name: row.try_get("pick_face_name")?,
            },
            observed_active_inbound,
            snapshot,
            required_level: decision.required_level,
            target_gap: decision.target_gap,
            suggested_outcome: decision.outcome,
            suggested_quantity: decision.planned,
            suggested_remaining: decision.remaining,
            active_work_count: row.try_get("active_work_count")?,
            active_work_quantity: level(row.try_get("active_work_quantity")?)?,
            latest_plan: latest_plan_from_row(&row)?,
        });
    }
    let next_offset = has_more.then(|| filter.offset + u64::from(filter.limit));
    tx.commit().await?;
    Ok(ReplenishmentPolicyPage { items, next_offset })
}

pub async fn work_page(
    db: &Db,
    access: &TenantAccess,
    filter: ReplenishmentWorkPageFilter,
) -> AppResult<ReplenishmentWorkPage> {
    let offset = i64::try_from(filter.offset)
        .map_err(|_| AppError::bad_request("replenishment work page offset is invalid"))?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    let status = filter.status.map(status_database_values);
    let fetch_limit = i64::from(filter.limit) + 1;
    let rows = sqlx::query(
        r#"
        SELECT work.id, detail.plan_run_id, detail.policy_id, detail.policy_revision,
               work.status, work.inventory_owner_id, owner.name AS inventory_owner_name,
               work.facility_id, facility.name AS facility_name, detail.travel_sequence,
               work.priority, detail.item_id, item.description AS item_description,
               (SELECT barcode.name FROM barcodes barcode
                WHERE barcode.tenant_id=detail.tenant_id AND barcode.item_id=detail.item_id
                  AND barcode.deleted IS NULL ORDER BY barcode.id LIMIT 1) AS primary_sku,
               detail.uom, detail.source_lot AS lot, detail.source_serial AS serial,
               detail.source_expiration AS expiration, detail.planned_qty,
               detail.source_inventory_balance_id, detail.item_batch_id,
               detail.source_location_id, source.barcode AS source_barcode,
               source.name AS source_name, detail.destination_location_id,
               destination.barcode AS destination_barcode, destination.name AS destination_name,
               work.assigned_user_id, work.lease_expires_at, work.due_at,
               work.created, work.completed_at
        FROM work_tasks work
        JOIN replenishment_tasks detail ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id
        JOIN inventory_owners owner ON owner.tenant_id=work.tenant_id
          AND owner.id=work.inventory_owner_id AND owner.deleted IS NULL
        JOIN facilities facility ON facility.tenant_id=work.tenant_id
          AND facility.id=work.facility_id AND facility.deleted IS NULL
        JOIN items item ON item.tenant_id=detail.tenant_id AND item.id=detail.item_id
        JOIN item_batches batch ON batch.tenant_id=detail.tenant_id
          AND batch.inventory_owner_id=detail.inventory_owner_id AND batch.id=detail.item_batch_id
        JOIN locations source ON source.tenant_id=detail.tenant_id
          AND source.facility_id=detail.facility_id AND source.id=detail.source_location_id
        JOIN locations destination ON destination.tenant_id=detail.tenant_id
          AND destination.facility_id=detail.facility_id AND destination.id=detail.destination_location_id
        WHERE work.tenant_id=$1 AND work.task_type='replenishment' AND work.deleted IS NULL
          AND ($2 OR work.facility_id=ANY($3)) AND ($4 OR work.inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR work.facility_id=$6)
          AND ($7::bigint IS NULL OR work.inventory_owner_id=$7)
          AND ($8::bigint IS NULL OR detail.item_id=$8)
          AND ($9::bigint IS NULL OR detail.destination_location_id=$9)
          AND ($10::text[] IS NULL OR work.status=ANY($10))
          AND ($10::text[] IS NOT NULL OR work.status IN ('open','assigned','in_progress'))
        ORDER BY
          CASE WHEN $11='created' AND $12 THEN work.created END ASC,
          CASE WHEN $11='created' AND NOT $12 THEN work.created END DESC,
          CASE WHEN $11='priority' AND $12 THEN work.priority END ASC,
          CASE WHEN $11='priority' AND NOT $12 THEN work.priority END DESC,
          CASE WHEN $11='inventory_owner' AND $12 THEN LOWER(owner.name) END ASC,
          CASE WHEN $11='inventory_owner' AND NOT $12 THEN LOWER(owner.name) END DESC,
          CASE WHEN $11='facility' AND $12 THEN LOWER(facility.name) END ASC,
          CASE WHEN $11='facility' AND NOT $12 THEN LOWER(facility.name) END DESC,
          CASE WHEN $11='item' AND $12 THEN LOWER(COALESCE(item.description,'')) END ASC,
          CASE WHEN $11='item' AND NOT $12 THEN LOWER(COALESCE(item.description,'')) END DESC,
          CASE WHEN $11='source' AND $12 THEN LOWER(source.barcode) END ASC,
          CASE WHEN $11='source' AND NOT $12 THEN LOWER(source.barcode) END DESC,
          CASE WHEN $11='destination' AND $12 THEN LOWER(destination.barcode) END ASC,
          CASE WHEN $11='destination' AND NOT $12 THEN LOWER(destination.barcode) END DESC,
          CASE WHEN $11='quantity' AND $12 THEN detail.planned_qty END ASC,
          CASE WHEN $11='quantity' AND NOT $12 THEN detail.planned_qty END DESC,
          CASE WHEN $11='status' AND $12 THEN work.status END ASC,
          CASE WHEN $11='status' AND NOT $12 THEN work.status END DESC,
          CASE WHEN $11='lease' AND $12 THEN COALESCE(work.lease_expires_at,work.due_at) END ASC NULLS LAST,
          CASE WHEN $11='lease' AND NOT $12 THEN COALESCE(work.lease_expires_at,work.due_at) END DESC NULLS LAST,
          CASE WHEN $12 THEN detail.travel_sequence END ASC,
          CASE WHEN NOT $12 THEN detail.travel_sequence END DESC,
          CASE WHEN $12 THEN work.id END ASC,
          CASE WHEN NOT $12 THEN work.id END DESC
        OFFSET $13 LIMIT $14
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(filter.inventory_owner_id.map(|id| id.get()))
        .bind(filter.item_id.map(|id| id.get()))
        .bind(filter.pick_face_location_id.map(|id| id.get()))
        .bind(status)
        .bind(filter.sort.as_str())
        .bind(filter.direction.is_ascending())
        .bind(offset)
        .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(filter.limit);
    let items = rows
        .into_iter()
        .take(usize::from(filter.limit))
        .map(map_work_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_offset = has_more.then(|| filter.offset + u64::from(filter.limit));
    tx.commit().await?;
    Ok(ReplenishmentWorkPage { items, next_offset })
}

fn latest_plan_from_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<Option<ReplenishmentLatestPlanReadModel>> {
    let Some(plan_id) = row.try_get::<Option<i64>, _>("latest_plan_id")? else {
        return Ok(None);
    };
    Ok(Some(ReplenishmentLatestPlanReadModel {
        plan_id: ReplenishmentPlanId::new(plan_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        outcome: parse_outcome(&row.try_get::<String, _>("latest_plan_outcome")?)?,
        planned: level(row.try_get("latest_plan_planned_qty")?)?,
        remaining: level(row.try_get("latest_plan_remaining_qty")?)?,
        planned_by: UserId::new(row.try_get("latest_plan_planned_by")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        planned_at: row.try_get("latest_plan_planned_at")?,
    }))
}

fn map_work_row(row: sqlx::postgres::PgRow) -> AppResult<ReplenishmentWorkReadModel> {
    Ok(ReplenishmentWorkReadModel {
        work_id: ReplenishmentWorkId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        plan_id: ReplenishmentPlanId::new(row.try_get("plan_run_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        policy_id: ReplenishmentPolicyId::new(row.try_get("policy_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        policy_revision: ReplenishmentPolicyRevision::new(row.try_get("policy_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: parse_work_status(&row.try_get::<String, _>("status")?)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        sequence: u32::try_from(row.try_get::<i64, _>("travel_sequence")?)
            .map_err(|_| AppError::internal("replenishment sequence overflow"))?,
        priority: row.try_get("priority")?,
        item_id: CatalogItemId::new(row.try_get("item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        uom: ReplenishmentUom::new(row.try_get::<String, _>("uom")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        expiration: row.try_get("expiration")?,
        quantity: ReplenishmentMoveQuantity::new(row.try_get("planned_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_inventory_balance_id: InventoryBalanceId::new(
            row.try_get("source_inventory_balance_id")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location: ReplenishmentLocationReadModel {
            location_id: LocationId::new(row.try_get("source_location_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            barcode: scan_db(row.try_get("source_barcode")?)?,
            name: row.try_get("source_name")?,
        },
        destination_pick_face: ReplenishmentLocationReadModel {
            location_id: LocationId::new(row.try_get("destination_location_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            barcode: scan_db(row.try_get("destination_barcode")?)?,
            name: row.try_get("destination_name")?,
        },
        claimed_by: row
            .try_get::<Option<i64>, _>("assigned_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        due_at: row.try_get("due_at")?,
        created_at: row.try_get("created")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn scan_db(value: String) -> AppResult<ReplenishmentScanValue> {
    ReplenishmentScanValue::new(value)
        .map_err(|error| AppError::internal(format!("invalid scannable value: {error}")))
}

fn parse_outcome(value: &str) -> AppResult<ReplenishmentPlanningOutcome> {
    match value {
        "not_needed" => Ok(ReplenishmentPlanningOutcome::NotNeeded),
        "insufficient_reserve" => Ok(ReplenishmentPlanningOutcome::InsufficientReserve),
        "partially_planned" => Ok(ReplenishmentPlanningOutcome::PartiallyPlanned),
        "fully_planned" => Ok(ReplenishmentPlanningOutcome::FullyPlanned),
        _ => Err(AppError::internal("invalid replenishment planning outcome")),
    }
}

fn parse_work_status(value: &str) -> AppResult<ReplenishmentWorkStatus> {
    match value {
        "open" | "assigned" => Ok(ReplenishmentWorkStatus::Pending),
        "in_progress" => Ok(ReplenishmentWorkStatus::Claimed),
        "completed" => Ok(ReplenishmentWorkStatus::Completed),
        "cancelled" => Ok(ReplenishmentWorkStatus::Cancelled),
        _ => Err(AppError::internal("invalid replenishment work status")),
    }
}

fn status_database_values(status: ReplenishmentWorkStatus) -> Vec<String> {
    match status {
        ReplenishmentWorkStatus::Pending => vec!["open".into(), "assigned".into()],
        ReplenishmentWorkStatus::Claimed => vec!["in_progress".into()],
        ReplenishmentWorkStatus::Completed => vec!["completed".into()],
        ReplenishmentWorkStatus::Cancelled => vec!["cancelled".into()],
    }
}
