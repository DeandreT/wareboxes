//! Scope-bound supervisor views for putaway planning and execution.

use sqlx::Row;
use wareboxes_application::putaway::{
    PutawayCandidatePage, PutawayCandidateQuery, PutawayCandidateReadModel, PutawayCandidateSort,
    PutawayCursor, PutawayLocationReadModel, PutawaySortDirection, PutawayWorkPage,
    PutawayWorkQuery, PutawayWorkReadModel, PutawayWorkSort, PutawayWorkStatus, PutawayWorkflow,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::{bind_tenant_context, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn putaway_candidate_page(
    db: &Db,
    access: &TenantAccess,
    query: PutawayCandidateQuery,
) -> AppResult<PutawayCandidatePage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;

    let sort_expression = candidate_sort_expression(query.sort);
    let direction = sort_direction(query.direction);
    let sql = format!(
        r#"
        WITH RECURSIVE plate_tree AS (
            SELECT plate.id AS root_id,plate.id AS member_id
            FROM license_plates plate
            WHERE plate.tenant_id=$1 AND plate.deleted IS NULL
              AND plate.parent_license_plate_id IS NULL
            UNION ALL
            SELECT plate_tree.root_id,child.id
            FROM plate_tree
            JOIN license_plates child ON child.tenant_id=$1
              AND child.parent_license_plate_id=plate_tree.member_id
              AND child.deleted IS NULL
        ), candidates AS (
            SELECT 'loose'::text AS workflow,
                   balance.id AS anchor_id,
                   balance.inventory_owner_id,
                   owner.name AS inventory_owner_name,
                   balance.facility_id,
                   facility.name AS facility_name,
                   balance.id AS source_inventory_balance_id,
                   NULL::bigint AS license_plate_id,
                   NULL::text AS license_plate_barcode,
                   balance.location_id AS source_location_id,
                   source.barcode AS source_barcode,
                   source.name AS source_name,
                   1::bigint AS item_count,
                   1::bigint AS balance_count,
                   balance.item_id,
                   item.description AS item_description,
                   (SELECT barcode.name
                    FROM barcodes barcode
                    WHERE barcode.tenant_id=balance.tenant_id
                      AND barcode.item_id=balance.item_id
                      AND barcode.deleted IS NULL
                    ORDER BY barcode.id LIMIT 1) AS primary_sku,
                   balance.uom,
                   batch.lot,
                   batch.serial,
                   (balance.qty_on_hand-balance.qty_reserved-balance.qty_held)::bigint
                       AS available_quantity,
                   balance.created AS received_at
            FROM inventory_balances balance
            JOIN inventory_owners owner ON owner.tenant_id=balance.tenant_id
              AND owner.id=balance.inventory_owner_id AND owner.deleted IS NULL
            JOIN facilities facility ON facility.tenant_id=balance.tenant_id
              AND facility.id=balance.facility_id AND facility.deleted IS NULL
            JOIN locations source ON source.tenant_id=balance.tenant_id
              AND source.facility_id=balance.facility_id AND source.id=balance.location_id
              AND source.deleted IS NULL AND source.active AND source.receivable
            JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
              AND batch.inventory_owner_id=balance.inventory_owner_id
              AND batch.id=balance.item_batch_id AND batch.deleted IS NULL
            JOIN items item ON item.tenant_id=balance.tenant_id
              AND item.id=balance.item_id AND item.deleted IS NULL
            WHERE balance.tenant_id=$1 AND balance.deleted IS NULL
              AND balance.license_plate_id IS NULL AND balance.status='available'
              AND balance.qty_on_hand-balance.qty_reserved-balance.qty_held>0
              AND NOT EXISTS (
                SELECT 1 FROM loose_inventory_movement_claims claim
                WHERE claim.tenant_id=balance.tenant_id
                  AND claim.inventory_owner_id=balance.inventory_owner_id
                  AND claim.source_inventory_balance_id=balance.id
                  AND claim.released_at IS NULL
              )

            UNION ALL

            SELECT 'license_plate'::text AS workflow,
                   plate.id AS anchor_id,
                   plate.inventory_owner_id,
                   owner.name AS inventory_owner_name,
                   plate.facility_id,
                   facility.name AS facility_name,
                   NULL::bigint AS source_inventory_balance_id,
                   plate.id AS license_plate_id,
                   plate.barcode AS license_plate_barcode,
                   plate.location_id AS source_location_id,
                   source.barcode AS source_barcode,
                   source.name AS source_name,
                   COUNT(DISTINCT balance.item_id)::bigint AS item_count,
                   COUNT(*)::bigint AS balance_count,
                   CASE WHEN COUNT(DISTINCT balance.item_id)=1 THEN MIN(balance.item_id) END
                       AS item_id,
                   CASE WHEN COUNT(DISTINCT balance.item_id)=1 THEN MIN(item.description) END
                       AS item_description,
                   CASE WHEN COUNT(DISTINCT balance.item_id)=1 THEN MIN(primary_sku.name) END
                       AS primary_sku,
                   CASE WHEN COUNT(DISTINCT balance.uom)=1 THEN MIN(balance.uom) END AS uom,
                   CASE WHEN COUNT(DISTINCT batch.lot)=1 THEN MIN(batch.lot) END AS lot,
                   CASE WHEN COUNT(DISTINCT batch.serial)=1 THEN MIN(batch.serial) END AS serial,
                   SUM(balance.qty_on_hand)::bigint AS available_quantity,
                   MIN(balance.created) AS received_at
            FROM license_plates plate
            JOIN inventory_owners owner ON owner.tenant_id=plate.tenant_id
              AND owner.id=plate.inventory_owner_id AND owner.deleted IS NULL
            JOIN facilities facility ON facility.tenant_id=plate.tenant_id
              AND facility.id=plate.facility_id AND facility.deleted IS NULL
            JOIN locations source ON source.tenant_id=plate.tenant_id
              AND source.facility_id=plate.facility_id AND source.id=plate.location_id
              AND source.deleted IS NULL AND source.active AND source.receivable
            JOIN plate_tree ON plate_tree.root_id=plate.id
            JOIN inventory_balances balance ON balance.tenant_id=plate.tenant_id
              AND balance.inventory_owner_id=plate.inventory_owner_id
              AND balance.facility_id=plate.facility_id
              AND balance.license_plate_id=plate_tree.member_id
              AND balance.location_id=plate.location_id
              AND balance.deleted IS NULL AND balance.qty_on_hand>0
            JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
              AND batch.inventory_owner_id=balance.inventory_owner_id
              AND batch.id=balance.item_batch_id AND batch.deleted IS NULL
            JOIN items item ON item.tenant_id=balance.tenant_id
              AND item.id=balance.item_id AND item.deleted IS NULL
            LEFT JOIN LATERAL (
                SELECT barcode.name
                FROM barcodes barcode
                WHERE barcode.tenant_id=balance.tenant_id
                  AND barcode.item_id=balance.item_id
                  AND barcode.deleted IS NULL
                ORDER BY barcode.id LIMIT 1
            ) primary_sku ON true
            WHERE plate.tenant_id=$1 AND plate.deleted IS NULL
              AND plate.parent_license_plate_id IS NULL
              AND plate.location_id IS NOT NULL
              AND plate.barcode IS NOT NULL AND btrim(plate.barcode)<>''
              AND NOT EXISTS (
                SELECT 1 FROM license_plate_putaway_tasks active
                WHERE active.tenant_id=plate.tenant_id
                  AND active.inventory_owner_id=plate.inventory_owner_id
                  AND active.license_plate_id IN (
                    SELECT member_id FROM plate_tree WHERE root_id=plate.id
                  ) AND active.closed_at IS NULL
              )
              AND NOT EXISTS (
                SELECT 1 FROM inventory_relocation_tasks active
                WHERE active.tenant_id=plate.tenant_id
                  AND active.inventory_owner_id=plate.inventory_owner_id
                  AND active.license_plate_id IN (
                    SELECT member_id FROM plate_tree WHERE root_id=plate.id
                  ) AND active.closed_at IS NULL
              )
            GROUP BY plate.id,plate.inventory_owner_id,owner.name,plate.facility_id,
                     facility.name,plate.barcode,plate.location_id,source.barcode,source.name
            HAVING BOOL_AND(balance.status='available'
                         AND balance.qty_reserved=0 AND balance.qty_held=0)
        )
        SELECT * FROM candidates
        WHERE ($2 OR facility_id=ANY($3)) AND ($4 OR inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR facility_id=$6)
          AND ($7::bigint IS NULL OR inventory_owner_id=$7)
          AND ($8::text IS NULL OR workflow=$8)
        ORDER BY {sort_expression} {direction} NULLS LAST, workflow, anchor_id
        OFFSET $9 LIMIT $10
        "#,
    );
    let fetch_limit = i64::from(query.limit) + 1;
    let offset = query.cursor.map_or(0, |cursor| cursor.offset);
    let offset =
        i64::try_from(offset).map_err(|_| AppError::bad_request("putaway cursor overflow"))?;
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.facility_id.map(FacilityId::get))
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.workflow.map(PutawayWorkflow::as_str))
        .bind(offset)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .into_iter()
        .take(usize::from(query.limit))
        .map(map_candidate)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = has_more.then_some(PutawayCursor {
        offset: u64::try_from(offset)
            .map_err(|_| AppError::internal("putaway cursor is negative"))?
            + u64::from(query.limit),
    });
    tx.commit().await?;
    Ok(PutawayCandidatePage { items, next_cursor })
}

pub async fn putaway_work_page(
    db: &Db,
    access: &TenantAccess,
    query: PutawayWorkQuery,
) -> AppResult<PutawayWorkPage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;

    let sort_expression = work_sort_expression(query.sort);
    let direction = sort_direction(query.direction);
    let sql = format!(
        r#"
        WITH work AS (
          SELECT task.id AS task_id,
                 CASE task.task_type WHEN 'putaway' THEN 'loose'
                      ELSE 'license_plate' END::text AS workflow,
                 CASE task.status WHEN 'open' THEN 'pending'
                      WHEN 'assigned' THEN 'pending'
                      WHEN 'in_progress' THEN 'claimed'
                      WHEN 'completed' THEN 'completed'
                      ELSE 'cancelled' END::text AS lifecycle_status,
                 task.inventory_owner_id,owner.name AS inventory_owner_name,
                 task.facility_id,facility.name AS facility_name,
                 loose.source_inventory_balance_id,
                 plate.license_plate_id,lp.barcode AS license_plate_barcode,
                 COALESCE(loose.source_location_id,plate.source_location_id) AS source_location_id,
                 source.barcode AS source_barcode,source.name AS source_name,
                 COALESCE(loose.destination_location_id,plate.destination_location_id)
                   AS destination_location_id,
                 destination.barcode AS destination_barcode,destination.name AS destination_name,
                 CASE WHEN loose.task_id IS NOT NULL THEN 1 ELSE plate_summary.item_count END
                   AS item_count,
                 CASE WHEN loose.task_id IS NOT NULL THEN 1 ELSE plate.planned_balance_count END
                   AS balance_count,
                 COALESCE(loose.item_id,plate_summary.item_id) AS item_id,
                 COALESCE(loose_item.description,plate_summary.item_description)
                   AS item_description,
                 COALESCE(loose_sku.name,plate_summary.primary_sku) AS primary_sku,
                 COALESCE(loose_balance.uom,plate_summary.uom) AS uom,
                 COALESCE(loose.planned_quantity,plate_summary.planned_quantity)
                   AS planned_quantity,
                 task.priority,task.instructions,task.assigned_user_id,
                 task.lease_expires_at,task.due_at,task.created,task.completed_at
          FROM work_tasks task
          LEFT JOIN putaway_tasks loose ON loose.tenant_id=task.tenant_id
            AND loose.task_id=task.id
          LEFT JOIN license_plate_putaway_tasks plate ON plate.tenant_id=task.tenant_id
            AND plate.task_id=task.id
          JOIN inventory_owners owner ON owner.tenant_id=task.tenant_id
            AND owner.id=task.inventory_owner_id AND owner.deleted IS NULL
          JOIN facilities facility ON facility.tenant_id=task.tenant_id
            AND facility.id=task.facility_id AND facility.deleted IS NULL
          JOIN locations source ON source.tenant_id=task.tenant_id
            AND source.facility_id=task.facility_id
            AND source.id=COALESCE(loose.source_location_id,plate.source_location_id)
          JOIN locations destination ON destination.tenant_id=task.tenant_id
            AND destination.facility_id=task.facility_id
            AND destination.id=COALESCE(loose.destination_location_id,plate.destination_location_id)
          LEFT JOIN inventory_balances loose_balance ON loose_balance.tenant_id=loose.tenant_id
            AND loose_balance.inventory_owner_id=loose.inventory_owner_id
            AND loose_balance.id=loose.source_inventory_balance_id
          LEFT JOIN items loose_item ON loose_item.tenant_id=loose.tenant_id
            AND loose_item.id=loose.item_id
          LEFT JOIN LATERAL (
            SELECT barcode.name FROM barcodes barcode
            WHERE barcode.tenant_id=loose.tenant_id AND barcode.item_id=loose.item_id
              AND barcode.deleted IS NULL ORDER BY barcode.id LIMIT 1
          ) loose_sku ON true
          LEFT JOIN license_plates lp ON lp.tenant_id=plate.tenant_id
            AND lp.inventory_owner_id=plate.inventory_owner_id
            AND lp.facility_id=plate.facility_id AND lp.id=plate.license_plate_id
          LEFT JOIN LATERAL (
            SELECT COUNT(DISTINCT content.item_id)::bigint AS item_count,
                   CASE WHEN COUNT(DISTINCT content.item_id)=1 THEN MIN(content.item_id) END
                     AS item_id,
                   CASE WHEN COUNT(DISTINCT content.item_id)=1 THEN MIN(item.description) END
                     AS item_description,
                   CASE WHEN COUNT(DISTINCT content.item_id)=1 THEN MIN(sku.name) END
                     AS primary_sku,
                   CASE WHEN COUNT(DISTINCT content.uom)=1 THEN MIN(content.uom) END AS uom,
                   SUM(content.planned_quantity)::bigint AS planned_quantity
            FROM license_plate_putaway_task_contents content
            JOIN items item ON item.tenant_id=content.tenant_id AND item.id=content.item_id
            LEFT JOIN LATERAL (
              SELECT barcode.name FROM barcodes barcode
              WHERE barcode.tenant_id=content.tenant_id AND barcode.item_id=content.item_id
                AND barcode.deleted IS NULL ORDER BY barcode.id LIMIT 1
            ) sku ON true
            WHERE content.tenant_id=plate.tenant_id AND content.task_id=plate.task_id
          ) plate_summary ON true
          WHERE task.tenant_id=$1 AND task.deleted IS NULL
            AND task.task_type IN ('putaway','license_plate_putaway')
        )
        SELECT * FROM work
        WHERE ($2 OR facility_id=ANY($3)) AND ($4 OR inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR facility_id=$6)
          AND ($7::bigint IS NULL OR inventory_owner_id=$7)
          AND ($8::text IS NULL OR workflow=$8)
          AND ($9::text IS NULL OR lifecycle_status=$9)
          AND ($9::text IS NOT NULL OR lifecycle_status IN ('pending','claimed'))
        ORDER BY {sort_expression} {direction} NULLS LAST, task_id
        OFFSET $10 LIMIT $11
        "#,
    );
    let fetch_limit = i64::from(query.limit) + 1;
    let offset = query.cursor.map_or(0, |cursor| cursor.offset);
    let offset =
        i64::try_from(offset).map_err(|_| AppError::bad_request("putaway cursor overflow"))?;
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.facility_id.map(FacilityId::get))
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.workflow.map(PutawayWorkflow::as_str))
        .bind(query.status.map(PutawayWorkStatus::as_str))
        .bind(offset)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .into_iter()
        .take(usize::from(query.limit))
        .map(map_work)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = has_more.then_some(PutawayCursor {
        offset: u64::try_from(offset)
            .map_err(|_| AppError::internal("putaway cursor is negative"))?
            + u64::from(query.limit),
    });
    tx.commit().await?;
    Ok(PutawayWorkPage { items, next_cursor })
}

fn candidate_sort_expression(sort: PutawayCandidateSort) -> &'static str {
    match sort {
        PutawayCandidateSort::ReceivedAt => "received_at",
        PutawayCandidateSort::Client => "lower(inventory_owner_name)",
        PutawayCandidateSort::Facility => "lower(facility_name)",
        PutawayCandidateSort::Source => "lower(COALESCE(source_name,source_barcode))",
        PutawayCandidateSort::Item => "lower(COALESCE(item_description,primary_sku,''))",
        PutawayCandidateSort::Quantity => "available_quantity",
        PutawayCandidateSort::Workflow => "workflow",
    }
}

fn work_sort_expression(sort: PutawayWorkSort) -> &'static str {
    match sort {
        PutawayWorkSort::Priority => "priority",
        PutawayWorkSort::CreatedAt => "created",
        PutawayWorkSort::Client => "lower(inventory_owner_name)",
        PutawayWorkSort::Facility => "lower(facility_name)",
        PutawayWorkSort::Source => "lower(COALESCE(source_name,source_barcode))",
        PutawayWorkSort::Destination => "lower(COALESCE(destination_name,destination_barcode))",
        PutawayWorkSort::Quantity => "planned_quantity",
        PutawayWorkSort::Status => "lifecycle_status",
        PutawayWorkSort::Workflow => "workflow",
    }
}

fn sort_direction(direction: PutawaySortDirection) -> &'static str {
    match direction {
        PutawaySortDirection::Asc => "ASC",
        PutawaySortDirection::Desc => "DESC",
    }
}

fn map_candidate(row: sqlx::postgres::PgRow) -> AppResult<PutawayCandidateReadModel> {
    Ok(PutawayCandidateReadModel {
        workflow: parse_workflow(&row.try_get::<String, _>("workflow")?)?,
        inventory_owner_id: owner_id(row.try_get("inventory_owner_id")?)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: facility_id(row.try_get("facility_id")?)?,
        facility_name: row.try_get("facility_name")?,
        source_inventory_balance_id: row.try_get("source_inventory_balance_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        license_plate_barcode: row.try_get("license_plate_barcode")?,
        source_location: location_from_row(&row, "source")?,
        item_count: row.try_get("item_count")?,
        balance_count: row.try_get("balance_count")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        available_quantity: row.try_get("available_quantity")?,
        received_at: row.try_get("received_at")?,
    })
}

fn map_work(row: sqlx::postgres::PgRow) -> AppResult<PutawayWorkReadModel> {
    Ok(PutawayWorkReadModel {
        task_id: row.try_get("task_id")?,
        workflow: parse_workflow(&row.try_get::<String, _>("workflow")?)?,
        status: parse_status(&row.try_get::<String, _>("lifecycle_status")?)?,
        inventory_owner_id: owner_id(row.try_get("inventory_owner_id")?)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: facility_id(row.try_get("facility_id")?)?,
        facility_name: row.try_get("facility_name")?,
        source_inventory_balance_id: row.try_get("source_inventory_balance_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        license_plate_barcode: row.try_get("license_plate_barcode")?,
        source_location: location_from_row(&row, "source")?,
        destination_location: location_from_row(&row, "destination")?,
        item_count: row.try_get("item_count")?,
        balance_count: row.try_get("balance_count")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        uom: row.try_get("uom")?,
        planned_quantity: row.try_get("planned_quantity")?,
        priority: row.try_get("priority")?,
        instructions: row.try_get("instructions")?,
        assigned_user_id: row.try_get("assigned_user_id")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        due_at: row.try_get("due_at")?,
        created_at: row.try_get("created")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn location_from_row(
    row: &sqlx::postgres::PgRow,
    prefix: &str,
) -> AppResult<PutawayLocationReadModel> {
    let barcode = row
        .try_get::<Option<String>, _>(format!("{prefix}_barcode").as_str())?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::internal(format!("putaway {prefix} location is not scannable")))?;
    Ok(PutawayLocationReadModel {
        location_id: row.try_get(format!("{prefix}_location_id").as_str())?,
        barcode,
        name: row.try_get(format!("{prefix}_name").as_str())?,
    })
}

fn parse_workflow(value: &str) -> AppResult<PutawayWorkflow> {
    match value {
        "loose" => Ok(PutawayWorkflow::Loose),
        "license_plate" => Ok(PutawayWorkflow::LicensePlate),
        _ => Err(AppError::internal("invalid putaway workflow")),
    }
}

fn parse_status(value: &str) -> AppResult<PutawayWorkStatus> {
    match value {
        "pending" => Ok(PutawayWorkStatus::Pending),
        "claimed" => Ok(PutawayWorkStatus::Claimed),
        "completed" => Ok(PutawayWorkStatus::Completed),
        "cancelled" => Ok(PutawayWorkStatus::Cancelled),
        _ => Err(AppError::internal("invalid putaway work status")),
    }
}

fn owner_id(value: i64) -> AppResult<InventoryOwnerId> {
    InventoryOwnerId::new(value).map_err(|error| AppError::internal(error.to_string()))
}

fn facility_id(value: i64) -> AppResult<FacilityId> {
    FacilityId::new(value).map_err(|error| AppError::internal(error.to_string()))
}
