use crate::db::now_iso;
use crate::error::AppResult;
use wareboxes_domain::TenantId;

/// Insert an address row from order-shipping fields and return its id.
/// Mirrors the `db.insert(addresses)` calls in `app/utils/orders.ts`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_address_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    line1: Option<&str>,
    line2: Option<&str>,
    city: Option<&str>,
    state: Option<&str>,
    postal_code: Option<&str>,
    country: Option<&str>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO addresses (tenant_id, created, line1, line2, city, state, postal_code, country)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(line1.unwrap_or(""))
    .bind(line2)
    .bind(city)
    .bind(state)
    .bind(postal_code)
    .bind(country.unwrap_or(""))
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}
