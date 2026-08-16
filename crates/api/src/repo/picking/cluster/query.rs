use sqlx::Row;
use wareboxes_application::pick_cluster::{
    PickClusterCandidateReadModel, PickClusterWorkspace, PickClusterWorkspaceQuery,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{PickClusterId, PickTaskId};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

use super::models::{internal, read_cart_tx, read_cluster_tx};

const MAX_CARTS: usize = 100;
const MAX_CANDIDATES: usize = 200;
const MAX_CLUSTERS: usize = 100;

pub async fn workspace(
    db: &Db,
    access: &TenantAccess,
    query: PickClusterWorkspaceQuery,
) -> AppResult<PickClusterWorkspace> {
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
    if !scope.includes_facility(query.facility_id.get())
        || !scope.includes_inventory_owner(query.inventory_owner_id.get())
    {
        return Err(AppError::not_found("pick cluster workspace"));
    }
    let owner_facility_exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM inventory_owner_facilities
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND deleted IS NULL)"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.inventory_owner_id.get())
    .bind(query.facility_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !owner_facility_exists {
        return Err(AppError::not_found("pick cluster workspace"));
    }

    let cart_ids: Vec<i64> = sqlx::query_scalar(
        r#"SELECT id FROM pick_carts WHERE tenant_id=$1 AND facility_id=$2
        ORDER BY CASE status WHEN 'active' THEN 0 WHEN 'out_of_service' THEN 1 ELSE 2 END,
          name,id LIMIT $3"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.facility_id.get())
    .bind(i64::try_from(MAX_CARTS + 1).map_err(internal)?)
    .fetch_all(&mut *tx)
    .await?;
    require_complete(cart_ids.len(), MAX_CARTS, "pick carts")?;
    let mut carts = Vec::with_capacity(cart_ids.len());
    for cart_id in cart_ids {
        carts.push(
            read_cart_tx(
                &mut tx,
                access.tenant_id,
                wareboxes_domain::PickCartId::new(cart_id).map_err(internal)?,
            )
            .await?,
        );
    }

    let candidate_rows = sqlx::query(
        r#"SELECT task.id AS task_id,task.order_id,orders.order_key,
          content.source_location_id,location.barcode AS source_location_barcode,
          location.name AS source_location_name,
          release_allocation.travel_sequence AS source_travel_sequence,
          content.item_id,COALESCE(item.description,'Item #'||content.item_id::text)
            AS item_description,
          content.uom,content.planned_qty,task.priority,task.ship_by,task.created_at
        FROM pick_tasks task
        JOIN pick_task_contents content ON content.tenant_id=task.tenant_id
          AND content.task_id=task.id AND content.state='pending'
        JOIN order_release_allocations release_allocation
          ON release_allocation.tenant_id=task.tenant_id
          AND release_allocation.inventory_owner_id=task.inventory_owner_id
          AND release_allocation.facility_id=task.facility_id
          AND release_allocation.order_release_id=task.order_release_id
          AND release_allocation.allocation_id=task.source_allocation_id
        JOIN orders ON orders.tenant_id=task.tenant_id AND orders.id=task.order_id
          AND orders.deleted IS NULL
        JOIN locations location ON location.tenant_id=content.tenant_id
          AND location.facility_id=content.facility_id AND location.id=content.source_location_id
          AND location.deleted IS NULL AND location.active AND location.pickable
        JOIN items item ON item.tenant_id=content.tenant_id AND item.id=content.item_id
          AND item.deleted IS NULL
        WHERE task.tenant_id=$1 AND task.inventory_owner_id=$2 AND task.facility_id=$3
          AND task.status='open' AND task.assigned_user_id IS NULL
          AND NOT EXISTS(
            SELECT 1 FROM pick_cluster_members member
            JOIN pick_clusters cluster ON cluster.tenant_id=member.tenant_id
              AND cluster.id=member.cluster_id
            WHERE member.tenant_id=task.tenant_id AND member.task_id=task.id
              AND cluster.status IN('planned','in_progress'))
        ORDER BY task.priority DESC,task.ship_by ASC NULLS LAST,
          lower(location.barcode),task.id LIMIT $4"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.inventory_owner_id.get())
    .bind(query.facility_id.get())
    .bind(i64::try_from(MAX_CANDIDATES + 1).map_err(internal)?)
    .fetch_all(&mut *tx)
    .await?;
    require_complete(
        candidate_rows.len(),
        MAX_CANDIDATES,
        "pick cluster candidates",
    )?;
    let candidates = candidate_rows
        .iter()
        .map(|row| {
            Ok(PickClusterCandidateReadModel {
                task_id: PickTaskId::new(row.try_get("task_id")?).map_err(internal)?,
                order_id: wareboxes_domain::OrderId::new(row.try_get("order_id")?)
                    .map_err(internal)?,
                order_key: row.try_get("order_key")?,
                source_location_id: row.try_get("source_location_id")?,
                source_location_barcode: row.try_get("source_location_barcode")?,
                source_location_name: row.try_get("source_location_name")?,
                source_travel_sequence: row.try_get("source_travel_sequence")?,
                item_id: row.try_get("item_id")?,
                item_description: row.try_get("item_description")?,
                uom: row.try_get("uom")?,
                planned_quantity: row.try_get("planned_qty")?,
                priority: row.try_get("priority")?,
                ship_by: row.try_get("ship_by")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    let cluster_ids: Vec<i64> = sqlx::query_scalar(
        r#"SELECT id FROM pick_clusters
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND ($4 OR status IN('planned','in_progress'))
        ORDER BY CASE status WHEN 'in_progress' THEN 0 WHEN 'planned' THEN 1 ELSE 2 END,
          planned_at DESC,id DESC LIMIT $5"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.inventory_owner_id.get())
    .bind(query.facility_id.get())
    .bind(query.include_history)
    .bind(i64::try_from(MAX_CLUSTERS + 1).map_err(internal)?)
    .fetch_all(&mut *tx)
    .await?;
    require_complete(cluster_ids.len(), MAX_CLUSTERS, "pick clusters")?;
    let mut clusters = Vec::with_capacity(cluster_ids.len());
    for cluster_id in cluster_ids {
        clusters.push(
            read_cluster_tx(
                &mut tx,
                access.tenant_id,
                PickClusterId::new(cluster_id).map_err(internal)?,
            )
            .await?,
        );
    }
    tx.commit().await?;
    Ok(PickClusterWorkspace {
        carts,
        candidates,
        clusters,
    })
}

fn require_complete(actual: usize, maximum: usize, label: &str) -> AppResult<()> {
    if actual > maximum {
        Err(AppError::conflict(format!(
            "{label} exceed the bounded workspace; narrow the facility/client scope"
        )))
    } else {
        Ok(())
    }
}
