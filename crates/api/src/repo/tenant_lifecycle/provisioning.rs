use wareboxes_domain::{TenantId, Timestamp};

use crate::error::AppResult;

pub(super) async fn provision_initial_administrator_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    administrator_id: i64,
    administrator_email: &str,
    occurred_at: Timestamp,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO tenant_memberships
        (tenant_id,user_id,created,is_default,all_facilities,all_inventory_owners)
        VALUES($1,$2,$3,FALSE,TRUE,TRUE)"#,
    )
    .bind(tenant_id.get())
    .bind(administrator_id)
    .bind(occurred_at)
    .execute(&mut **tx)
    .await?;
    let permission_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO permissions(tenant_id,created,name,description)
        VALUES($1,$2,'admin','Tenant administrator') RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(occurred_at)
    .fetch_one(&mut **tx)
    .await?;
    let role_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO roles(tenant_id,created,name,description,self_user_id)
        VALUES($1,$2,$3,'Self role',$4) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(occurred_at)
    .bind(administrator_email)
    .bind(administrator_id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO user_roles(tenant_id,created,user_id,role_id)
        VALUES($1,$2,$3,$4)"#,
    )
    .bind(tenant_id.get())
    .bind(occurred_at)
    .bind(administrator_id)
    .bind(role_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO role_permissions(tenant_id,created,role_id,permission_id)
        VALUES($1,$2,$3,$4)"#,
    )
    .bind(tenant_id.get())
    .bind(occurred_at)
    .bind(role_id)
    .bind(permission_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
