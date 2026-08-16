use sqlx::Row;
use wareboxes_application::pick_cluster::{
    PickCartReadModel, PickCartSlotReadModel, PickClusterMemberReadModel, PickClusterReadModel,
};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, OrderId, PickCartBarcode, PickCartId, PickCartName,
    PickCartSlotCode, PickCartSlotId, PickCartStatus, PickClusterId, PickClusterMemberId,
    PickClusterStatus, PickTaskId, TenantId, UserId,
};

use crate::error::{AppError, AppResult};

pub(super) fn internal(error: impl ToString) -> AppError {
    AppError::internal(error.to_string())
}

pub(super) fn cart_status(value: &str) -> AppResult<PickCartStatus> {
    match value {
        "active" => Ok(PickCartStatus::Active),
        "out_of_service" => Ok(PickCartStatus::OutOfService),
        "retired" => Ok(PickCartStatus::Retired),
        _ => Err(AppError::internal("invalid stored pick cart status")),
    }
}

pub(super) const fn cart_status_text(value: PickCartStatus) -> &'static str {
    match value {
        PickCartStatus::Active => "active",
        PickCartStatus::OutOfService => "out_of_service",
        PickCartStatus::Retired => "retired",
    }
}

fn cluster_status(value: &str) -> AppResult<PickClusterStatus> {
    match value {
        "planned" => Ok(PickClusterStatus::Planned),
        "in_progress" => Ok(PickClusterStatus::InProgress),
        "completed" => Ok(PickClusterStatus::Completed),
        "cancelled" => Ok(PickClusterStatus::Cancelled),
        _ => Err(AppError::internal("invalid stored pick cluster status")),
    }
}

pub(super) async fn read_cart_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    cart_id: PickCartId,
) -> AppResult<PickCartReadModel> {
    let row = sqlx::query("SELECT * FROM pick_carts WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(cart_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("pick cart"))?;
    let slot_rows = sqlx::query(
        "SELECT * FROM pick_cart_slots WHERE tenant_id=$1 AND cart_id=$2 ORDER BY sequence",
    )
    .bind(tenant_id.get())
    .bind(cart_id.get())
    .fetch_all(&mut **tx)
    .await?;
    Ok(PickCartReadModel {
        cart_id,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        barcode: PickCartBarcode::new(row.try_get("barcode")?).map_err(internal)?,
        name: PickCartName::new(row.try_get("name")?).map_err(internal)?,
        status: cart_status(&row.try_get::<String, _>("status")?)?,
        revision: row.try_get("revision")?,
        slots: slot_rows
            .iter()
            .map(|slot| {
                Ok(PickCartSlotReadModel {
                    slot_id: PickCartSlotId::new(slot.try_get("id")?).map_err(internal)?,
                    code: PickCartSlotCode::new(slot.try_get("code")?).map_err(internal)?,
                    sequence: slot.try_get("sequence")?,
                })
            })
            .collect::<AppResult<Vec<_>>>()?,
        created_by: UserId::new(row.try_get("created_by_user_id")?).map_err(internal)?,
        created_at: row.try_get("created_at")?,
        status_changed_by: row
            .try_get::<Option<i64>, _>("status_changed_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        status_changed_at: row.try_get("status_changed_at")?,
    })
}

pub(super) async fn read_cluster_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    cluster_id: PickClusterId,
) -> AppResult<PickClusterReadModel> {
    let row = sqlx::query(
        r#"SELECT cluster.*,cart.barcode AS cart_barcode,cart.name AS cart_name
        FROM pick_clusters cluster
        JOIN pick_carts cart ON cart.tenant_id=cluster.tenant_id
          AND cart.facility_id=cluster.facility_id AND cart.id=cluster.cart_id
        WHERE cluster.tenant_id=$1 AND cluster.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(cluster_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick cluster"))?;
    let member_rows = sqlx::query(
        r#"SELECT member.*,task.status AS task_status,orders.order_key,slot.code AS slot_code,
          content.source_location_id,content.item_id,content.uom,content.planned_qty,
          location.barcode AS source_location_barcode,location.name AS source_location_name,
          COALESCE(item.description,'Item #'||content.item_id::text) AS item_description
        FROM pick_cluster_members member
        JOIN pick_tasks task ON task.tenant_id=member.tenant_id AND task.id=member.task_id
        JOIN orders ON orders.tenant_id=member.tenant_id AND orders.id=member.order_id
        JOIN pick_cart_slots slot ON slot.tenant_id=member.tenant_id
          AND slot.facility_id=member.facility_id AND slot.cart_id=member.cart_id
          AND slot.id=member.slot_id
        JOIN pick_task_contents content ON content.tenant_id=member.tenant_id
          AND content.task_id=member.task_id
        JOIN locations location ON location.tenant_id=content.tenant_id
          AND location.facility_id=content.facility_id AND location.id=content.source_location_id
        JOIN items item ON item.tenant_id=content.tenant_id AND item.id=content.item_id
        WHERE member.tenant_id=$1 AND member.cluster_id=$2 ORDER BY member.sequence"#,
    )
    .bind(tenant_id.get())
    .bind(cluster_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let completed_task_count = member_rows
        .iter()
        .filter(|member| {
            member
                .try_get::<String, _>("task_status")
                .is_ok_and(|status| {
                    matches!(status.as_str(), "completed" | "shorted" | "cancelled")
                })
        })
        .count();
    Ok(PickClusterReadModel {
        cluster_id,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        cart_id: PickCartId::new(row.try_get("cart_id")?).map_err(internal)?,
        cart_barcode: PickCartBarcode::new(row.try_get("cart_barcode")?).map_err(internal)?,
        cart_name: PickCartName::new(row.try_get("cart_name")?).map_err(internal)?,
        status: cluster_status(&row.try_get::<String, _>("status")?)?,
        revision: row.try_get("revision")?,
        task_count: row.try_get("task_count")?,
        order_count: row.try_get("order_count")?,
        completed_task_count: i64::try_from(completed_task_count).map_err(internal)?,
        assigned_user_id: row
            .try_get::<Option<i64>, _>("assigned_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        planned_by: UserId::new(row.try_get("planned_by_user_id")?).map_err(internal)?,
        planned_at: row.try_get("planned_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        cancelled_by: row
            .try_get::<Option<i64>, _>("cancelled_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        cancelled_at: row.try_get("cancelled_at")?,
        cancellation_note: row.try_get("cancellation_note")?,
        members: member_rows
            .iter()
            .map(|member| {
                Ok(PickClusterMemberReadModel {
                    member_id: PickClusterMemberId::new(member.try_get("id")?).map_err(internal)?,
                    sequence: member.try_get("sequence")?,
                    task_id: PickTaskId::new(member.try_get("task_id")?).map_err(internal)?,
                    task_status: member.try_get("task_status")?,
                    order_id: OrderId::new(member.try_get("order_id")?).map_err(internal)?,
                    order_key: member.try_get("order_key")?,
                    slot_id: PickCartSlotId::new(member.try_get("slot_id")?).map_err(internal)?,
                    slot_code: PickCartSlotCode::new(member.try_get("slot_code")?)
                        .map_err(internal)?,
                    source_location_id: member.try_get("source_location_id")?,
                    source_location_barcode: member.try_get("source_location_barcode")?,
                    source_location_name: member.try_get("source_location_name")?,
                    item_id: member.try_get("item_id")?,
                    item_description: member.try_get("item_description")?,
                    uom: member.try_get("uom")?,
                    planned_quantity: member.try_get("planned_qty")?,
                })
            })
            .collect::<AppResult<Vec<_>>>()?,
    })
}
