//! Ported from `app/utils/types/db/employees.ts`.

use sqlx::Row;
use wareboxes_core::models::{Employee, Timestamp};
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};

fn map(row: &sqlx::postgres::PgRow) -> AppResult<Employee> {
    Ok(Employee {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        user_id: row.try_get("user_id")?,
        first_name: row.try_get("first_name")?,
        last_name: row.try_get("last_name")?,
        email: row.try_get("email")?,
        phone: row.try_get("phone")?,
        title: row.try_get("title")?,
        r#type: row.try_get("type")?,
        hired: row.try_get("hired")?,
        terminated: row.try_get("terminated")?,
    })
}

pub async fn get_employees(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<Employee>> {
    let sql = if show_deleted {
        "SELECT id, tenant_id, created, deleted, user_id, first_name, last_name, email, phone, title, type, hired, terminated FROM employees WHERE tenant_id = $1 ORDER BY id"
    } else {
        "SELECT id, tenant_id, created, deleted, user_id, first_name, last_name, email, phone, title, type, hired, terminated FROM employees WHERE tenant_id = $1 AND deleted IS NULL ORDER BY id"
    };
    let rows = sqlx::query(sql).bind(tenant_id.get()).fetch_all(db).await?;
    rows.iter().map(map).collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn add_employee(
    db: &Db,
    tenant_id: TenantId,
    first_name: &str,
    last_name: &str,
    title: &str,
    employee_type: &str,
    email: Option<&str>,
    phone: Option<&str>,
    hired: Timestamp,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO employees (tenant_id, created, first_name, last_name, title, type, email, phone, hired) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(first_name)
    .bind(last_name)
    .bind(title)
    .bind(employee_type)
    .bind(email)
    .bind(phone)
    .bind(hired)
    .fetch_one(db)
    .await?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_employee(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    first_name: Option<&str>,
    last_name: Option<&str>,
    title: Option<&str>,
    employee_type: Option<&str>,
    email: Option<&str>,
    phone: Option<&str>,
    terminated: Option<Timestamp>,
) -> AppResult<bool> {
    let res = sqlx::query(
        r#"
        UPDATE employees SET
            first_name = COALESCE($1, first_name),
            last_name = COALESCE($2, last_name),
            title = COALESCE($3, title),
            type = COALESCE($4, type),
            email = COALESCE($5, email),
            phone = COALESCE($6, phone),
            terminated = COALESCE($7, terminated)
        WHERE tenant_id = $8 AND id = $9
        "#,
    )
    .bind(first_name)
    .bind(last_name)
    .bind(title)
    .bind(employee_type)
    .bind(email)
    .bind(phone)
    .bind(terminated)
    .bind(tenant_id.get())
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_employee_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let res = sqlx::query("UPDATE employees SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(tenant_id.get())
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
