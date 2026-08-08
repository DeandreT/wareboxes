use crate::db::{bind_tenant_context, now_iso};
use crate::error::AppResult;
use wareboxes_domain::TenantId;

#[derive(Debug, Clone, Copy)]
pub struct NewAddress<'a> {
    pub name: Option<&'a str>,
    pub company: Option<&'a str>,
    pub line1: &'a str,
    pub line2: Option<&'a str>,
    pub city: Option<&'a str>,
    pub state: Option<&'a str>,
    pub postal_code: Option<&'a str>,
    pub country: &'a str,
    pub phone: Option<&'a str>,
    pub email: Option<&'a str>,
}

pub async fn insert_address_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    address: NewAddress<'_>,
) -> AppResult<i64> {
    bind_tenant_context(tx, tenant_id).await?;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO addresses (
            tenant_id, created, name, company, line1, line2,
            city, state, postal_code, country, phone, email
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(address.name)
    .bind(address.company)
    .bind(address.line1)
    .bind(address.line2)
    .bind(address.city)
    .bind(address.state)
    .bind(address.postal_code)
    .bind(address.country)
    .bind(address.phone)
    .bind(address.email)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}
