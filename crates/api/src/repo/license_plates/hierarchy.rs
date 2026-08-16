use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::license_plate::{
    ChangeLicensePlateParentCommand, ChangeLicensePlateParentResult, LicensePlateHierarchyAction,
    LicensePlateHierarchyEventReadModel, LicensePlateHierarchyNodeReadModel,
    LicensePlateHierarchyReadModel,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    validate_license_plate_attachment, FacilityId, InventoryOwnerId,
    LicensePlateAttachmentSnapshot, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::db::{begin_tenant_transaction, bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

const PERMISSION: &str = "wms";
const MAX_TREE_NODES: usize = 1_000;
const MAX_HISTORY_EVENTS: usize = 1_000;

#[derive(Debug, Clone)]
struct PlateRow {
    id: i64,
    barcode: Option<String>,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: Option<i64>,
    parent_id: Option<i64>,
    revision: i64,
    depth: i32,
    direct_units: i64,
    updated_at: Option<Timestamp>,
    updated_by: Option<i64>,
}

#[derive(Debug, Clone)]
struct PlateSnapshot {
    id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: Option<i64>,
    parent_id: Option<i64>,
    revision: i64,
    deleted: bool,
}

fn invalid_data(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

async fn bind_actor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: UserId,
) -> AppResult<()> {
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(actor_id.get().to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn require_scope(scope: &ScopeBindings, plate: &PlateSnapshot) -> AppResult<()> {
    if scope.includes_facility(plate.facility_id)
        && scope.includes_inventory_owner(plate.inventory_owner_id)
    {
        Ok(())
    } else {
        Err(AppError::not_found("license plate"))
    }
}

async fn load_snapshot_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    id: i64,
) -> AppResult<Option<PlateSnapshot>> {
    let row = sqlx::query(
        r#"
        SELECT id,inventory_owner_id,facility_id,location_id,
               parent_license_plate_id,hierarchy_revision,deleted IS NOT NULL AS deleted
        FROM license_plates
        WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(PlateSnapshot {
            id: row.try_get("id")?,
            inventory_owner_id: row.try_get("inventory_owner_id")?,
            facility_id: row.try_get("facility_id")?,
            location_id: row.try_get("location_id")?,
            parent_id: row.try_get("parent_license_plate_id")?,
            revision: row.try_get("hierarchy_revision")?,
            deleted: row.try_get("deleted")?,
        })
    })
    .transpose()
}

async fn root_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    id: i64,
) -> AppResult<(i64, u8)> {
    let row = sqlx::query(
        r#"
        WITH RECURSIVE ancestors AS (
          SELECT plate.id,plate.parent_license_plate_id,0::INTEGER AS depth
          FROM license_plates plate WHERE plate.tenant_id=$1 AND plate.id=$2
          UNION ALL
          SELECT parent.id,parent.parent_license_plate_id,ancestors.depth+1
          FROM ancestors JOIN license_plates parent
            ON parent.tenant_id=$1 AND parent.id=ancestors.parent_license_plate_id
          WHERE ancestors.depth<8
        )
        SELECT id,depth FROM ancestors ORDER BY depth DESC LIMIT 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("license plate"))?;
    let depth: i32 = row.try_get("depth")?;
    Ok((
        row.try_get("id")?,
        u8::try_from(depth).map_err(invalid_data)?,
    ))
}

fn map_plate_row(row: &sqlx::postgres::PgRow) -> AppResult<PlateRow> {
    Ok(PlateRow {
        id: row.try_get("id")?,
        barcode: row.try_get("barcode")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        parent_id: row.try_get("parent_license_plate_id")?,
        revision: row.try_get("hierarchy_revision")?,
        depth: row.try_get("depth")?,
        direct_units: row.try_get("direct_units")?,
        updated_at: row.try_get("hierarchy_updated_at")?,
        updated_by: row.try_get("hierarchy_updated_by_user_id")?,
    })
}

async fn load_tree_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    root_id: i64,
) -> AppResult<Vec<PlateRow>> {
    let row_limit = i64::try_from(MAX_TREE_NODES + 1).map_err(invalid_data)?;
    let rows = sqlx::query(
        r#"
        WITH RECURSIVE tree AS (
          SELECT plate.id,plate.barcode,plate.inventory_owner_id,plate.facility_id,
                 plate.location_id,plate.parent_license_plate_id,
                 plate.hierarchy_revision,plate.hierarchy_updated_at,
                 plate.hierarchy_updated_by_user_id,0::INTEGER AS depth
          FROM license_plates plate
          WHERE plate.tenant_id=$1 AND plate.id=$2 AND plate.deleted IS NULL
          UNION ALL
          SELECT child.id,child.barcode,child.inventory_owner_id,child.facility_id,
                 child.location_id,child.parent_license_plate_id,
                 child.hierarchy_revision,child.hierarchy_updated_at,
                 child.hierarchy_updated_by_user_id,tree.depth+1
          FROM tree JOIN license_plates child
            ON child.tenant_id=$1 AND child.parent_license_plate_id=tree.id
           AND child.deleted IS NULL
          WHERE tree.depth<8
        )
        SELECT tree.*,
               COALESCE(SUM(balance.qty_on_hand),0)::BIGINT AS direct_units
        FROM tree
        LEFT JOIN inventory_balances balance ON balance.tenant_id=$1
          AND balance.license_plate_id=tree.id AND balance.deleted IS NULL
        GROUP BY tree.id,tree.barcode,tree.inventory_owner_id,tree.facility_id,
                 tree.location_id,tree.parent_license_plate_id,
                 tree.hierarchy_revision,tree.hierarchy_updated_at,
                 tree.hierarchy_updated_by_user_id,tree.depth
        ORDER BY tree.depth,tree.id LIMIT $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(root_id)
    .bind(row_limit)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() > MAX_TREE_NODES {
        return Err(AppError::conflict(
            "license plate hierarchy exceeds the 1000-node detail limit",
        ));
    }
    rows.iter().map(map_plate_row).collect()
}

fn is_descendant(rows: &HashMap<i64, PlateRow>, candidate_id: i64, ancestor_id: i64) -> bool {
    let mut cursor = Some(candidate_id);
    for _ in 0..=8 {
        let Some(id) = cursor else { return false };
        if id == ancestor_id {
            return true;
        }
        cursor = rows.get(&id).and_then(|row| row.parent_id);
    }
    false
}

fn map_node(
    rows: &HashMap<i64, PlateRow>,
    root_id: i64,
    id: i64,
) -> AppResult<LicensePlateHierarchyNodeReadModel> {
    let row = rows
        .get(&id)
        .ok_or_else(|| AppError::internal("license plate hierarchy row is missing"))?;
    let mut direct_child_ids = rows
        .values()
        .filter(|candidate| candidate.parent_id == Some(id))
        .map(|candidate| candidate.id)
        .collect::<Vec<_>>();
    direct_child_ids.sort_unstable();
    let mut descendant_ids = rows
        .keys()
        .copied()
        .filter(|candidate_id| *candidate_id != id && is_descendant(rows, *candidate_id, id))
        .collect::<Vec<_>>();
    descendant_ids.sort_unstable_by_key(|candidate_id| {
        rows.get(candidate_id)
            .map_or((i32::MAX, *candidate_id), |candidate| {
                (candidate.depth, candidate.id)
            })
    });
    let contained_unit_quantity = std::iter::once(id)
        .chain(descendant_ids.iter().copied())
        .try_fold(0_i64, |total, candidate_id| {
            total
                .checked_add(
                    rows.get(&candidate_id)
                        .map_or(0, |candidate| candidate.direct_units),
                )
                .ok_or_else(|| AppError::internal("license plate contained quantity overflow"))
        })?;
    Ok(LicensePlateHierarchyNodeReadModel {
        license_plate_id: row.id,
        barcode: row.barcode.clone(),
        inventory_owner_id: InventoryOwnerId::new(row.inventory_owner_id).map_err(invalid_data)?,
        facility_id: FacilityId::new(row.facility_id).map_err(invalid_data)?,
        location_id: row.location_id,
        parent_license_plate_id: row.parent_id,
        root_license_plate_id: root_id,
        depth: u8::try_from(row.depth).map_err(invalid_data)?,
        hierarchy_revision: row.revision,
        direct_child_ids,
        descendant_ids,
        direct_unit_quantity: row.direct_units,
        contained_unit_quantity,
        hierarchy_updated_at: row.updated_at,
        hierarchy_updated_by: row
            .updated_by
            .map(UserId::new)
            .transpose()
            .map_err(invalid_data)?,
    })
}

async fn load_events_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    child_id: i64,
) -> AppResult<Vec<LicensePlateHierarchyEventReadModel>> {
    let limit = i64::try_from(MAX_HISTORY_EVENTS + 1).map_err(invalid_data)?;
    let rows = sqlx::query(
        r#"
        SELECT id,child_license_plate_id,previous_parent_license_plate_id,
               parent_license_plate_id,resulting_revision,action,actor_user_id,
               occurred_at,reason
        FROM license_plate_hierarchy_events
        WHERE tenant_id=$1 AND child_license_plate_id=$2
        ORDER BY id DESC LIMIT $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(child_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() > MAX_HISTORY_EVENTS {
        return Err(AppError::conflict(
            "license plate hierarchy history exceeds the 1000-event detail limit",
        ));
    }
    rows.iter()
        .map(|row| {
            let action = match row.try_get::<String, _>("action")?.as_str() {
                "attached" => LicensePlateHierarchyAction::Attached,
                "detached" => LicensePlateHierarchyAction::Detached,
                value => {
                    return Err(AppError::internal(format!(
                        "invalid hierarchy action: {value}"
                    )))
                }
            };
            Ok(LicensePlateHierarchyEventReadModel {
                event_id: row.try_get("id")?,
                child_license_plate_id: row.try_get("child_license_plate_id")?,
                previous_parent_license_plate_id: row
                    .try_get("previous_parent_license_plate_id")?,
                parent_license_plate_id: row.try_get("parent_license_plate_id")?,
                resulting_revision: row.try_get("resulting_revision")?,
                action,
                actor_id: UserId::new(row.try_get("actor_user_id")?).map_err(invalid_data)?,
                occurred_at: row.try_get("occurred_at")?,
                reason: row.try_get("reason")?,
            })
        })
        .collect()
}

pub async fn hierarchy(
    db: &Db,
    access: &TenantAccess,
    license_plate_id: i64,
) -> AppResult<LicensePlateHierarchyReadModel> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    let snapshot = load_snapshot_tx(&mut tx, access.tenant_id, license_plate_id)
        .await?
        .ok_or_else(|| AppError::not_found("license plate"))?;
    require_scope(&scope, &snapshot)?;
    if snapshot.deleted {
        return Err(AppError::not_found("license plate"));
    }
    let (root_id, _) = root_id_tx(&mut tx, access.tenant_id, license_plate_id).await?;
    let tree = load_tree_tx(&mut tx, access.tenant_id, root_id).await?;
    let rows = tree
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let node = map_node(&rows, root_id, license_plate_id)?;
    let mut ancestor_ids = Vec::new();
    let mut cursor = node.parent_license_plate_id;
    while let Some(id) = cursor {
        ancestor_ids.push(id);
        cursor = rows.get(&id).and_then(|row| row.parent_id);
    }
    ancestor_ids.reverse();
    let ancestors = ancestor_ids
        .into_iter()
        .map(|id| map_node(&rows, root_id, id))
        .collect::<AppResult<Vec<_>>>()?;
    let descendants = node
        .descendant_ids
        .iter()
        .copied()
        .map(|id| map_node(&rows, root_id, id))
        .collect::<AppResult<Vec<_>>>()?;
    let events = load_events_tx(&mut tx, access.tenant_id, license_plate_id).await?;
    tx.commit().await?;
    Ok(LicensePlateHierarchyReadModel {
        node,
        ancestors,
        descendants,
        events,
    })
}

async fn hierarchy_shape_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    child_id: i64,
    parent_id: i64,
) -> AppResult<(u8, u8, bool, u32, u32)> {
    let row = sqlx::query(
        r#"
        WITH RECURSIVE parent_chain AS (
          SELECT plate.id,plate.parent_license_plate_id,0::INTEGER AS depth
          FROM license_plates plate WHERE plate.tenant_id=$1 AND plate.id=$3
          UNION ALL
          SELECT parent.id,parent.parent_license_plate_id,parent_chain.depth+1
          FROM parent_chain JOIN license_plates parent
            ON parent.tenant_id=$1 AND parent.id=parent_chain.parent_license_plate_id
          WHERE parent_chain.depth<9
        ), parent_root AS (
          SELECT id FROM parent_chain ORDER BY depth DESC LIMIT 1
        ), parent_tree AS (
          SELECT plate.id
          FROM license_plates plate JOIN parent_root ON parent_root.id=plate.id
          WHERE plate.tenant_id=$1
          UNION ALL
          SELECT child.id
          FROM parent_tree JOIN license_plates child
            ON child.tenant_id=$1 AND child.parent_license_plate_id=parent_tree.id
          WHERE child.deleted IS NULL
        ), child_tree AS (
          SELECT plate.id,0::INTEGER AS depth
          FROM license_plates plate WHERE plate.tenant_id=$1 AND plate.id=$2
          UNION ALL
          SELECT child.id,child_tree.depth+1
          FROM child_tree JOIN license_plates child
            ON child.tenant_id=$1 AND child.parent_license_plate_id=child_tree.id
          WHERE child_tree.depth<9
        )
        SELECT COALESCE((SELECT MAX(depth) FROM parent_chain),0)::INTEGER AS parent_depth,
               COALESCE((SELECT MAX(depth) FROM child_tree),0)::INTEGER AS child_height,
               EXISTS(SELECT 1 FROM parent_chain WHERE id=$2) AS contains_child,
               (SELECT COUNT(*) FROM parent_tree)::BIGINT AS parent_tree_size,
               (SELECT COUNT(*) FROM child_tree)::BIGINT AS child_tree_size
        "#,
    )
    .bind(tenant_id.get())
    .bind(child_id)
    .bind(parent_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok((
        u8::try_from(row.try_get::<i32, _>("parent_depth")?).map_err(invalid_data)?,
        u8::try_from(row.try_get::<i32, _>("child_height")?).map_err(invalid_data)?,
        row.try_get("contains_child")?,
        u32::try_from(row.try_get::<i64, _>("parent_tree_size")?).map_err(invalid_data)?,
        u32::try_from(row.try_get::<i64, _>("child_tree_size")?).map_err(invalid_data)?,
    ))
}

async fn require_no_active_movement_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    child_id: i64,
    parent_id: Option<i64>,
) -> AppResult<()> {
    let active: bool = sqlx::query_scalar(
        r#"
        WITH RECURSIVE child_tree AS (
          SELECT plate.id FROM license_plates plate
          WHERE plate.tenant_id=$1 AND plate.id=$2
          UNION ALL
          SELECT child.id FROM child_tree JOIN license_plates child
            ON child.tenant_id=$1 AND child.parent_license_plate_id=child_tree.id
        ), parent_chain AS (
          SELECT plate.id,plate.parent_license_plate_id FROM license_plates plate
          WHERE plate.tenant_id=$1 AND plate.id=$3
          UNION ALL
          SELECT parent.id,parent.parent_license_plate_id
          FROM parent_chain JOIN license_plates parent
            ON parent.tenant_id=$1 AND parent.id=parent_chain.parent_license_plate_id
        ), affected AS (
          SELECT id FROM child_tree UNION SELECT id FROM parent_chain
        )
        SELECT EXISTS(
          SELECT 1 FROM license_plate_putaway_tasks task
          WHERE task.tenant_id=$1 AND task.license_plate_id IN (SELECT id FROM affected)
            AND task.closed_at IS NULL
          UNION ALL
          SELECT 1 FROM inventory_relocation_tasks task
          WHERE task.tenant_id=$1 AND task.license_plate_id IN (SELECT id FROM affected)
            AND task.closed_at IS NULL
          UNION ALL
          SELECT 1 FROM packed_inventory_positions position
          WHERE position.tenant_id=$1
            AND position.current_license_plate_id IN (SELECT id FROM affected)
            AND position.state IN ('packed','staged','loaded')
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(child_id)
    .bind(parent_id)
    .fetch_one(&mut **tx)
    .await?;
    if active {
        Err(AppError::conflict(
            "license plate hierarchy cannot change during active movement or packed shipment work",
        ))
    } else {
        Ok(())
    }
}

async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &ChangeLicensePlateParentResult,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    reason: &str,
) -> AppResult<()> {
    let event_key = format!(
        "license_plate:{}:hierarchy:{}",
        result.license_plate_id, result.resulting_revision
    );
    let aggregate_id = result.license_plate_id.to_string();
    let ordering_key = format!("license_plate:{}", result.license_plate_id);
    let event_type = if result.parent_license_plate_id.is_some() {
        "inventory.license_plate.attached"
    } else {
        "inventory.license_plate.detached"
    };
    let payload = serde_json::json!({
        "license_plate_id": result.license_plate_id,
        "previous_parent_license_plate_id": result.previous_parent_license_plate_id,
        "parent_license_plate_id": result.parent_license_plate_id,
        "root_license_plate_id": result.root_license_plate_id,
        "depth": result.depth,
        "resulting_revision": result.resulting_revision,
        "changed_at": result.changed_at,
        "changed_by": result.changed_by,
        "reason": reason,
    });
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(result.changed_by.get()),
            event_key: &event_key,
            aggregate_type: "license_plate",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at: result.changed_at,
        },
    )
    .await?;
    Ok(())
}

pub async fn change_parent(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ChangeLicensePlateParentCommand,
) -> AppResult<ChangeLicensePlateParentResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, "change_license_plate_parent", command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;

    let initial = load_snapshot_tx(&mut tx, access.tenant_id, command.license_plate_id)
        .await?
        .ok_or_else(|| AppError::not_found("license plate"))?;
    require_scope(&scope, &initial)?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "license-plate-hierarchy:{}:{}:{}",
            access.tenant_id.get(),
            initial.inventory_owner_id,
            initial.facility_id
        ))
        .execute(&mut *tx)
        .await?;
    let child = load_snapshot_tx(&mut tx, access.tenant_id, command.license_plate_id)
        .await?
        .ok_or_else(|| AppError::not_found("license plate"))?;
    require_scope(&scope, &child)?;
    if child.deleted {
        return Err(AppError::not_found("license plate"));
    }
    if child.revision != command.expected_revision {
        return Err(AppError::conflict(
            "license plate hierarchy revision changed",
        ));
    }
    if command.parent_license_plate_id == child.parent_id {
        return Err(AppError::bad_request(
            "requested parent must differ from the current parent",
        ));
    }

    if let Some(parent_id) = command.parent_license_plate_id {
        let parent = load_snapshot_tx(&mut tx, access.tenant_id, parent_id)
            .await?
            .ok_or_else(|| AppError::not_found("parent license plate"))?;
        require_scope(&scope, &parent).map_err(|_| AppError::not_found("parent license plate"))?;
        let (parent_depth, child_height, contains_child, parent_tree_size, child_tree_size) =
            hierarchy_shape_tx(&mut tx, access.tenant_id, child.id, parent.id).await?;
        validate_license_plate_attachment(LicensePlateAttachmentSnapshot {
            child_id: child.id,
            parent_id: parent.id,
            child_has_parent: child.parent_id.is_some(),
            child_deleted: child.deleted,
            parent_deleted: parent.deleted,
            same_inventory_owner: child.inventory_owner_id == parent.inventory_owner_id,
            same_facility: child.facility_id == parent.facility_id,
            same_location: child.location_id == parent.location_id,
            parent_chain_contains_child: contains_child,
            parent_depth,
            child_subtree_height: child_height,
            parent_tree_size,
            child_tree_size,
        })
        .map_err(|error| AppError::conflict(error.to_string()))?;
    } else if child.parent_id.is_none() {
        return Err(AppError::bad_request("license plate is not nested"));
    }
    require_no_active_movement_tx(
        &mut tx,
        access.tenant_id,
        child.id,
        command.parent_license_plate_id.or(child.parent_id),
    )
    .await?;

    let changed_at = now_iso();
    let resulting_revision = child
        .revision
        .checked_add(1)
        .ok_or_else(|| AppError::internal("license plate hierarchy revision overflow"))?;
    sqlx::query(
        r#"
        UPDATE license_plates SET parent_license_plate_id=$1,hierarchy_revision=$2,
          hierarchy_updated_at=$3,hierarchy_updated_by_user_id=$4
        WHERE tenant_id=$5 AND id=$6 AND hierarchy_revision=$7 AND deleted IS NULL
        "#,
    )
    .bind(command.parent_license_plate_id)
    .bind(resulting_revision)
    .bind(changed_at)
    .bind(context.actor_id.get())
    .bind(access.tenant_id.get())
    .bind(child.id)
    .bind(child.revision)
    .execute(&mut *tx)
    .await?;
    let action = if command.parent_license_plate_id.is_some() {
        "attached"
    } else {
        "detached"
    };
    sqlx::query(
        r#"
        INSERT INTO license_plate_hierarchy_events(
          tenant_id,inventory_owner_id,facility_id,child_license_plate_id,
          previous_parent_license_plate_id,parent_license_plate_id,resulting_revision,
          action,actor_user_id,occurred_at,reason,idempotency_key,request_hash)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(child.inventory_owner_id)
    .bind(child.facility_id)
    .bind(child.id)
    .bind(child.parent_id)
    .bind(command.parent_license_plate_id)
    .bind(resulting_revision)
    .bind(action)
    .bind(context.actor_id.get())
    .bind(changed_at)
    .bind(&command.reason)
    .bind(prepared.idempotency_key())
    .bind(prepared.request_hash())
    .execute(&mut *tx)
    .await?;
    let (root_license_plate_id, depth) = root_id_tx(&mut tx, access.tenant_id, child.id).await?;
    let result = ChangeLicensePlateParentResult {
        license_plate_id: child.id,
        previous_parent_license_plate_id: child.parent_id,
        parent_license_plate_id: command.parent_license_plate_id,
        root_license_plate_id,
        depth,
        resulting_revision,
        changed_at,
        changed_by: context.actor_id,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        InventoryOwnerId::new(child.inventory_owner_id).map_err(invalid_data)?,
        FacilityId::new(child.facility_id).map_err(invalid_data)?,
        &command.reason,
    )
    .await?;
    prepared.commit(tx, result).await.map_err(AppError::from)
}
