//! Tenant- and facility-scoped employee persistence.

use sqlx::{Postgres, Row, Transaction};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::workforce_identity::{
    EmployeeIdentityChangeResult, LinkEmployeeIdentityCommand, UnlinkEmployeeIdentityCommand,
    LINK_EMPLOYEE_IDENTITY_OPERATION, UNLINK_EMPLOYEE_IDENTITY_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{Employee, TenantAccess, Timestamp};
use wareboxes_domain::{
    EmployeeId, EmployeeIdentityChangeId, EmployeeIdentityChangeKind, EmployeeIdentityReason,
    FacilityId, SiteScope, TenantId, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders;

const IDENTITY_PERMISSION: &str = "admin";

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
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let rows = sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(show_deleted)
        .bind(site_scope.all_facilities)
        .bind(&facility_ids)
        .fetch_all(&mut *tx)
        .await?;
    let employees = rows.iter().map(map).collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(employees)
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
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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

struct LockedEmployeeIdentity {
    user_id: Option<i64>,
    revision: i64,
    facility_ids: Vec<i64>,
}

/// Links an employee to an active interactive tenant member, or atomically relinks
/// it when `expected_user_id` names the currently linked user.
pub async fn link_employee_identity(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &LinkEmployeeIdentityCommand,
) -> AppResult<EmployeeIdentityChangeResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, LINK_EMPLOYEE_IDENTITY_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let actor_scope =
        lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        IDENTITY_PERMISSION,
    )
    .await?;

    if let Some(result) = prepared
        .replayed::<EmployeeIdentityChangeResult>(&mut tx)
        .await?
    {
        require_replayed_identity_change_visible_tx(
            &mut tx,
            access.tenant_id,
            &actor_scope,
            &result,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let employee = lock_scoped_employee_identity_tx(
        &mut tx,
        access.tenant_id,
        command.employee_id,
        &actor_scope,
    )
    .await?;
    let expected_user_id = command.expected_user_id.map(UserId::get);
    if employee.user_id != expected_user_id {
        return Err(AppError::conflict(
            "employee identity changed since it was observed",
        ));
    }
    if employee.user_id == Some(command.user_id.get()) {
        return Err(AppError::conflict(
            "employee is already linked to that interactive user",
        ));
    }

    require_target_user_can_work_employee_facilities_tx(
        &mut tx,
        access.tenant_id,
        command.user_id,
        &employee.facility_ids,
    )
    .await?;
    let kind = if employee.user_id.is_some() {
        EmployeeIdentityChangeKind::Relinked
    } else {
        EmployeeIdentityChangeKind::Linked
    };
    change_employee_identity_tx(
        tx,
        access.tenant_id,
        context.actor_id,
        command.employee_id,
        employee,
        Some(command.user_id),
        kind,
        command.reason.clone(),
        prepared,
    )
    .await
}

/// Removes an employee's interactive identity using compare-and-set semantics.
pub async fn unlink_employee_identity(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &UnlinkEmployeeIdentityCommand,
) -> AppResult<EmployeeIdentityChangeResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, UNLINK_EMPLOYEE_IDENTITY_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let actor_scope =
        lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        IDENTITY_PERMISSION,
    )
    .await?;

    if let Some(result) = prepared
        .replayed::<EmployeeIdentityChangeResult>(&mut tx)
        .await?
    {
        require_replayed_identity_change_visible_tx(
            &mut tx,
            access.tenant_id,
            &actor_scope,
            &result,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let employee = lock_scoped_employee_identity_tx(
        &mut tx,
        access.tenant_id,
        command.employee_id,
        &actor_scope,
    )
    .await?;
    if employee.user_id != Some(command.expected_user_id.get()) {
        return Err(AppError::conflict(
            "employee identity changed since it was observed",
        ));
    }
    change_employee_identity_tx(
        tx,
        access.tenant_id,
        context.actor_id,
        command.employee_id,
        employee,
        None,
        EmployeeIdentityChangeKind::Unlinked,
        command.reason.clone(),
        prepared,
    )
    .await
}

async fn lock_scoped_employee_identity_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    employee_id: EmployeeId,
    scope: &ScopeBindings,
) -> AppResult<LockedEmployeeIdentity> {
    let row = sqlx::query(
        r#"
        SELECT user_id, identity_revision
        FROM employees
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
          AND (terminated IS NULL OR terminated > clock_timestamp())
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(employee_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("employee"))?;
    let facility_ids = employee_facility_ids(tx, tenant_id, employee_id.get()).await?;
    if !scope_assignments_are_mutable(scope, &facility_ids) {
        return Err(AppError::not_found("employee"));
    }
    Ok(LockedEmployeeIdentity {
        user_id: row.try_get("user_id")?,
        revision: row.try_get("identity_revision")?,
        facility_ids,
    })
}

fn scope_assignments_are_mutable(scope: &ScopeBindings, facility_ids: &[i64]) -> bool {
    scope.all_facilities
        || (!facility_ids.is_empty()
            && facility_ids
                .iter()
                .all(|facility_id| scope.includes_facility(*facility_id)))
}

async fn require_target_user_can_work_employee_facilities_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    user_id: UserId,
    employee_facility_ids: &[i64],
) -> AppResult<()> {
    let target_scope = lock_current_scope_tx(tx, tenant_id, user_id.get()).await?;
    let active: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM tenant_memberships membership
            INNER JOIN users user_account ON user_account.id = membership.user_id
            WHERE membership.tenant_id = $1 AND membership.user_id = $2
              AND membership.deleted IS NULL AND user_account.deleted IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if !active {
        return Err(AppError::bad_request(
            "interactive user is not an active tenant member",
        ));
    }
    if employee_facility_ids.is_empty()
        || !employee_facility_ids
            .iter()
            .all(|facility_id| target_scope.includes_facility(*facility_id))
    {
        return Err(AppError::conflict(
            "interactive user's facility scope does not cover every employee assignment",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn change_employee_identity_tx(
    mut tx: Transaction<'_, Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    employee_id: EmployeeId,
    employee: LockedEmployeeIdentity,
    user_id: Option<UserId>,
    kind: EmployeeIdentityChangeKind,
    reason: EmployeeIdentityReason,
    prepared: PreparedCommand,
) -> AppResult<EmployeeIdentityChangeResult> {
    let changed_at = now_iso();
    let resulting_revision = employee
        .revision
        .checked_add(1)
        .ok_or_else(|| AppError::internal("employee identity revision overflow"))?;
    let updated = sqlx::query(
        r#"
        UPDATE employees
        SET user_id = $1, identity_revision = $2,
            identity_changed_by_user_id = $3, identity_changed_at = $4
        WHERE tenant_id = $5 AND id = $6 AND identity_revision = $7
        "#,
    )
    .bind(user_id.map(UserId::get))
    .bind(resulting_revision)
    .bind(actor_id.get())
    .bind(changed_at)
    .bind(tenant_id.get())
    .bind(employee_id.get())
    .bind(employee.revision)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "employee identity changed during the command",
        ));
    }
    let change_id = EmployeeIdentityChangeId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO employee_identity_changes (
                tenant_id, employee_id, previous_user_id, user_id, change_kind,
                reason, resulting_revision, changed_by_user_id, changed_at
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            RETURNING id
            "#,
        )
        .bind(tenant_id.get())
        .bind(employee_id.get())
        .bind(employee.user_id)
        .bind(user_id.map(UserId::get))
        .bind(kind.as_str())
        .bind(reason.as_str())
        .bind(resulting_revision)
        .bind(actor_id.get())
        .bind(changed_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let result = EmployeeIdentityChangeResult {
        change_id,
        employee_id,
        previous_user_id: employee
            .user_id
            .map(UserId::new)
            .transpose()
            .map_err(|error| {
                AppError::internal(format!("stored employee user ID is invalid: {error}"))
            })?,
        user_id,
        kind,
        reason,
        changed_by: actor_id,
        changed_at,
        resulting_revision,
    };
    enqueue_identity_changed_event_tx(&mut tx, tenant_id, &result).await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn enqueue_identity_changed_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    result: &EmployeeIdentityChangeResult,
) -> AppResult<()> {
    let ordering_key = format!("employee:{}", result.employee_id.get());
    let aggregate_sequence = orders::next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "employee:{}:identity:{}",
        result.employee_id.get(),
        result.resulting_revision
    );
    let aggregate_id = result.employee_id.get().to_string();
    let event_type = match result.kind {
        EmployeeIdentityChangeKind::Linked => "workforce.employee_identity.linked",
        EmployeeIdentityChangeKind::Relinked => "workforce.employee_identity.relinked",
        EmployeeIdentityChangeKind::Unlinked => "workforce.employee_identity.unlinked",
    };
    let payload = serde_json::to_value(result).map_err(|error| {
        AppError::internal(format!("identity event serialization failed: {error}"))
    })?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: Some(result.changed_by.get()),
            event_key: &event_key,
            aggregate_type: "employee",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at: result.changed_at,
        },
    )
    .await?;
    Ok(())
}

async fn require_replayed_identity_change_visible_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    result: &EmployeeIdentityChangeResult,
) -> AppResult<()> {
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM employee_identity_changes change
            INNER JOIN employees employee
              ON employee.tenant_id = change.tenant_id AND employee.id = change.employee_id
            WHERE change.tenant_id = $1 AND change.id = $2 AND change.employee_id = $3
              AND employee.deleted IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(result.change_id.get())
    .bind(result.employee_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if !visible {
        return Err(AppError::not_found("employee identity change"));
    }
    let facility_ids = employee_facility_ids(tx, tenant_id, result.employee_id.get()).await?;
    if !scope_assignments_are_mutable(scope, &facility_ids) {
        return Err(AppError::not_found("employee identity change"));
    }
    Ok(())
}
