//! Tenant- and facility-scoped employee persistence.

use sqlx::{Postgres, Row, Transaction};
use wareboxes_core::models::{Employee, SiteScope, Timestamp};
use wareboxes_domain::{FacilityId, TenantId};

use crate::db::Db;
use crate::error::{AppError, AppResult};

const EMPLOYEE_COLUMNS: &str = r#"
    employee.id, employee.tenant_id, employee.created, employee.deleted,
    employee.user_id, employee.first_name, employee.last_name, employee.email,
    employee.phone, employee.title, employee.type, employee.hired,
    employee.terminated,
    ARRAY(
        SELECT employee_facility.facility_id
        FROM employee_facilities employee_facility
        INNER JOIN facilities facility
            ON facility.tenant_id = employee_facility.tenant_id
           AND facility.id = employee_facility.facility_id
           AND facility.deleted IS NULL
        WHERE employee_facility.tenant_id = employee.tenant_id
          AND employee_facility.employee_id = employee.id
          AND employee_facility.deleted IS NULL
          AND ($3 OR employee_facility.facility_id = ANY($4))
        ORDER BY employee_facility.facility_id
    ) AS facility_ids,
    (
        $3
        OR NOT EXISTS (
            SELECT 1
            FROM employee_facilities outside_scope
            INNER JOIN facilities facility
                ON facility.tenant_id = outside_scope.tenant_id
               AND facility.id = outside_scope.facility_id
               AND facility.deleted IS NULL
            WHERE outside_scope.tenant_id = employee.tenant_id
              AND outside_scope.employee_id = employee.id
              AND outside_scope.deleted IS NULL
              AND NOT (outside_scope.facility_id = ANY($4))
        )
    ) AS can_manage
"#;

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
        facility_ids: row.try_get("facility_ids")?,
        can_manage: row.try_get("can_manage")?,
    })
}

fn scope_facility_ids(site_scope: &SiteScope) -> Vec<i64> {
    site_scope
        .facility_ids
        .iter()
        .map(|facility_id| facility_id.get())
        .collect()
}

fn validate_requested_facility_ids(site_scope: &SiteScope, facility_ids: &[i64]) -> AppResult<()> {
    if facility_ids.is_empty() {
        return Err(AppError::bad_request(
            "at least one employee facility is required",
        ));
    }

    let mut sorted_ids = facility_ids.to_vec();
    sorted_ids.sort_unstable();
    if sorted_ids.windows(2).any(|ids| ids[0] == ids[1]) {
        return Err(AppError::bad_request(
            "employee facility IDs must be unique",
        ));
    }
    for facility_id in sorted_ids {
        let facility_id = FacilityId::new(facility_id)
            .map_err(|_| AppError::bad_request("employee facility IDs must be positive"))?;
        if !site_scope.includes(facility_id) {
            return Err(AppError::forbidden());
        }
    }
    Ok(())
}

async fn lock_active_facilities(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    facility_ids: &[i64],
) -> AppResult<()> {
    validate_requested_facility_ids(site_scope, facility_ids)?;
    let rows = sqlx::query(
        r#"
        SELECT id
        FROM facilities
        WHERE tenant_id = $1 AND id = ANY($2) AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != facility_ids.len() {
        return Err(AppError::bad_request(
            "employee facility IDs contain an unavailable facility",
        ));
    }
    Ok(())
}

async fn employee_facility_ids(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    employee_id: i64,
) -> AppResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT facility_id
        FROM employee_facilities employee_facility
        INNER JOIN facilities facility
            ON facility.tenant_id = employee_facility.tenant_id
           AND facility.id = employee_facility.facility_id
           AND facility.deleted IS NULL
        WHERE employee_facility.tenant_id = $1
          AND employee_facility.employee_id = $2
          AND employee_facility.deleted IS NULL
        ORDER BY employee_facility.facility_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .fetch_all(&mut **tx)
    .await?)
}

fn current_assignments_are_mutable(site_scope: &SiteScope, facility_ids: &[i64]) -> bool {
    site_scope.all_facilities
        || (!facility_ids.is_empty()
            && facility_ids.iter().all(|facility_id| {
                FacilityId::new(*facility_id)
                    .is_ok_and(|facility_id| site_scope.includes(facility_id))
            }))
}

async fn lock_mutable_employee(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    employee_id: i64,
) -> AppResult<bool> {
    let employee_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM employees
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(employee_id) = employee_id else {
        return Ok(false);
    };

    let facility_ids = employee_facility_ids(tx, tenant_id, employee_id).await?;
    Ok(current_assignments_are_mutable(site_scope, &facility_ids))
}

pub async fn get_employees_in_scope(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    show_deleted: bool,
) -> AppResult<Vec<Employee>> {
    let facility_ids = scope_facility_ids(site_scope);
    let sql = format!(
        r#"
        SELECT {EMPLOYEE_COLUMNS}
        FROM employees employee
        WHERE employee.tenant_id = $1
          AND ($2 OR employee.deleted IS NULL)
          AND (
              $3
              OR EXISTS (
                  SELECT 1
                  FROM employee_facilities employee_facility
                  INNER JOIN facilities facility
                      ON facility.tenant_id = employee_facility.tenant_id
                     AND facility.id = employee_facility.facility_id
                     AND facility.deleted IS NULL
                  WHERE employee_facility.tenant_id = employee.tenant_id
                    AND employee_facility.employee_id = employee.id
                    AND employee_facility.deleted IS NULL
                    AND employee_facility.facility_id = ANY($4)
              )
          )
        ORDER BY employee.id
        "#,
    );
    let rows = sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(show_deleted)
        .bind(site_scope.all_facilities)
        .bind(&facility_ids)
        .fetch_all(db)
        .await?;
    rows.iter().map(map).collect()
}

pub struct NewEmployee<'a> {
    pub first_name: &'a str,
    pub last_name: &'a str,
    pub title: &'a str,
    pub employee_type: &'a str,
    pub email: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub hired: Timestamp,
    pub facility_ids: &'a [i64],
}

pub async fn add_employee(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    employee: &NewEmployee<'_>,
) -> AppResult<i64> {
    let mut tx = db.begin().await?;
    lock_active_facilities(&mut tx, tenant_id, site_scope, employee.facility_ids).await?;

    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO employees
            (tenant_id, created, first_name, last_name, title, type, email, phone, hired)
        VALUES ($1, clock_timestamp(), $2, $3, $4, $5, $6, $7, $8)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(employee.first_name)
    .bind(employee.last_name)
    .bind(employee.title)
    .bind(employee.employee_type)
    .bind(employee.email)
    .bind(employee.phone)
    .bind(employee.hired)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO employee_facilities (tenant_id, created, employee_id, facility_id)
        SELECT $1, clock_timestamp(), $2, UNNEST($3::BIGINT[])
        "#,
    )
    .bind(tenant_id.get())
    .bind(id)
    .bind(employee.facility_ids)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(id)
}

pub struct EmployeeChanges<'a> {
    pub first_name: Option<&'a str>,
    pub last_name: Option<&'a str>,
    pub title: Option<&'a str>,
    pub employee_type: Option<&'a str>,
    pub email: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub terminated: Option<Timestamp>,
    pub facility_ids: Option<&'a [i64]>,
}

pub async fn update_employee(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    employee_id: i64,
    changes: &EmployeeChanges<'_>,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    if !lock_mutable_employee(&mut tx, tenant_id, site_scope, employee_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }
    if let Some(facility_ids) = changes.facility_ids {
        lock_active_facilities(&mut tx, tenant_id, site_scope, facility_ids).await?;
    }

    let result = sqlx::query(
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
    .bind(changes.first_name)
    .bind(changes.last_name)
    .bind(changes.title)
    .bind(changes.employee_type)
    .bind(changes.email)
    .bind(changes.phone)
    .bind(changes.terminated)
    .bind(tenant_id.get())
    .bind(employee_id)
    .execute(&mut *tx)
    .await?;

    if let Some(facility_ids) = changes.facility_ids {
        sqlx::query(
            r#"
            UPDATE employee_facilities
            SET deleted = clock_timestamp()
            WHERE tenant_id = $1 AND employee_id = $2 AND deleted IS NULL
              AND NOT (facility_id = ANY($3))
            "#,
        )
        .bind(tenant_id.get())
        .bind(employee_id)
        .bind(facility_ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO employee_facilities (tenant_id, created, employee_id, facility_id)
            SELECT $1, clock_timestamp(), $2, UNNEST($3::BIGINT[])
            ON CONFLICT (tenant_id, employee_id, facility_id) DO UPDATE
            SET created = clock_timestamp(), deleted = NULL
            "#,
        )
        .bind(tenant_id.get())
        .bind(employee_id)
        .bind(facility_ids)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_employee_deleted(
    db: &Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    employee_id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    if !lock_mutable_employee(&mut tx, tenant_id, site_scope, employee_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }
    let result = sqlx::query(
        "UPDATE employees SET deleted = CASE WHEN $1 THEN clock_timestamp() ELSE NULL END WHERE tenant_id = $2 AND id = $3",
    )
    .bind(deleted)
    .bind(tenant_id.get())
    .bind(employee_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}
