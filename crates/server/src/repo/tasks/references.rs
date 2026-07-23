use sqlx::Row;
use wareboxes_domain::TenantId;

use crate::error::{AppError, AppResult};

pub(super) async fn lock_active_location_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    location_id: i64,
) -> AppResult<i64> {
    sqlx::query_scalar(
        r#"
        SELECT facility_id
        FROM locations
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL AND active
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(location_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::bad_request("location not found"))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn lock_item_cycle_references_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    location_id: i64,
    item_id: i64,
    order_id: Option<i64>,
    order_item_id: Option<i64>,
    inventory_balance_id: Option<i64>,
) -> AppResult<()> {
    lock_active_location_tx(tx, tenant_id, location_id).await?;
    let item: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM items WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL FOR SHARE",
    )
    .bind(tenant_id.get())
    .bind(item_id)
    .fetch_optional(&mut **tx)
    .await?;
    if item.is_none() {
        return Err(AppError::bad_request("item not found"));
    }
    if let Some(order_id) = order_id {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM orders WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL FOR SHARE",
        )
        .bind(tenant_id.get())
        .bind(order_id)
        .fetch_optional(&mut **tx)
        .await?;
        if found.is_none() {
            return Err(AppError::bad_request("order not found"));
        }
    }
    if let Some(order_item_id) = order_item_id {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM order_items WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL FOR SHARE",
        )
        .bind(tenant_id.get())
        .bind(order_item_id)
        .fetch_optional(&mut **tx)
        .await?;
        if found.is_none() {
            return Err(AppError::bad_request("order item not found"));
        }
    }
    if let Some(inventory_balance_id) = inventory_balance_id {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM inventory_balances WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL FOR SHARE",
        )
        .bind(tenant_id.get())
        .bind(inventory_balance_id)
        .fetch_optional(&mut **tx)
        .await?;
        if found.is_none() {
            return Err(AppError::bad_request("inventory balance not found"));
        }
    }
    Ok(())
}

pub(super) async fn lock_unpack_order_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    order_id: i64,
) -> AppResult<()> {
    let rows = sqlx::query(
        r#"
        SELECT item_id, item_batch_id
        FROM order_items
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND order_id = $3
          AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut item_ids = Vec::with_capacity(rows.len());
    let mut batch_ids = Vec::new();
    for row in &rows {
        item_ids.push(row.try_get::<i64, _>("item_id")?);
        if let Some(item_batch_id) = row.try_get::<Option<i64>, _>("item_batch_id")? {
            batch_ids.push(item_batch_id);
        }
    }
    item_ids.sort_unstable();
    item_ids.dedup();
    batch_ids.sort_unstable();
    batch_ids.dedup();
    let locked_items = sqlx::query(
        "SELECT id FROM items WHERE tenant_id = $1 AND id = ANY($2) AND deleted IS NULL FOR SHARE",
    )
    .bind(tenant_id.get())
    .bind(&item_ids)
    .fetch_all(&mut **tx)
    .await?;
    let locked_batches = sqlx::query(
        r#"
        SELECT id
        FROM item_batches
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND id = ANY($3)
          AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(&batch_ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked_items.len() != item_ids.len() || locked_batches.len() != batch_ids.len() {
        return Err(AppError::conflict(
            "cancelled order has inactive item references",
        ));
    }
    Ok(())
}
