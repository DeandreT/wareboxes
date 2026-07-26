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
    let Some(license_plate_id) = license_plate_id else {
        return Ok(());
    };
    let locked: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM license_plates WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(license_plate_id)
    .fetch_optional(&mut **tx)
    .await?;
    if locked.is_none() {
        return Err(AppError::internal(
            "inventory balance references a missing license plate",
        ));
    }
    Ok(())
}
