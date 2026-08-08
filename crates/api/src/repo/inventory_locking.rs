//! Shared row-lock ordering for inventory tied to license plates.

use sqlx::Row;
use wareboxes_domain::TenantId;

use crate::error::{AppError, AppResult};

pub(crate) async fn balance_license_plate_hint(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_balance_id: i64,
) -> AppResult<Option<i64>> {
    let row = sqlx::query(
        r#"
        SELECT license_plate_id
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory balance"))?;
    Ok(row.try_get("license_plate_id")?)
}

pub(crate) async fn lock_license_plate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    license_plate_id: Option<i64>,
) -> AppResult<()> {
    lock_license_plates(tx, tenant_id, license_plate_id.into_iter().collect()).await
}

pub(crate) async fn lock_license_plates(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    mut license_plate_ids: Vec<i64>,
) -> AppResult<()> {
    license_plate_ids.sort_unstable();
    license_plate_ids.dedup();
    if license_plate_ids.is_empty() {
        return Ok(());
    }
    let locked: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM license_plates
        WHERE tenant_id = $1 AND id = ANY($2)
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(&license_plate_ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked != license_plate_ids {
        return Err(AppError::internal(
            "inventory operation references a missing license plate",
        ));
    }
    Ok(())
}
