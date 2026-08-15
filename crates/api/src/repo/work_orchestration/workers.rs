use sqlx::Row;
use wareboxes_application::work_orchestration::{
    WorkOrchestrationWorkerCursor, WorkOrchestrationWorkerOptionReadModel,
    WorkOrchestrationWorkerPage, WorkOrchestrationWorkerPageQuery,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{EmployeeId, InventoryOwnerId, UserId};

use super::scope::{invalid_data, require_command_scope, require_owner_facility_tx};
use super::SUPERVISOR_PERMISSION;
use crate::db::{begin_tenant_transaction, Db};
use crate::error::AppResult;
use crate::repo::access::{current_scope_tx, require_permission_tx};

pub async fn worker_page(
    db: &Db,
    access: &TenantAccess,
    query: WorkOrchestrationWorkerPageQuery,
) -> AppResult<WorkOrchestrationWorkerPage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    require_command_scope(
        &scope,
        query.facility_id,
        query.inventory_owner_id,
        "work orchestration worker",
    )?;
    require_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        query.facility_id,
        query.inventory_owner_id,
        "work orchestration worker",
    )
    .await?;

    let rows = sqlx::query(
        r#"WITH RECURSIVE granted_roles(user_id,role_id,parent_id) AS (
          SELECT membership.user_id,role.id,role.parent_id
          FROM tenant_memberships membership
          JOIN user_roles user_role ON user_role.tenant_id=membership.tenant_id
            AND user_role.user_id=membership.user_id AND user_role.deleted IS NULL
          JOIN roles role ON role.tenant_id=user_role.tenant_id
            AND role.id=user_role.role_id AND role.deleted IS NULL
          WHERE membership.tenant_id=$1 AND membership.deleted IS NULL
          UNION
          SELECT child.user_id,parent.id,parent.parent_id
          FROM granted_roles child
          JOIN roles parent ON parent.tenant_id=$1 AND parent.id=child.parent_id
            AND parent.deleted IS NULL
        )
        SELECT employee.id AS employee_id,employee.user_id,
          COALESCE(NULLIF(trim(concat_ws(' ',employee.first_name,employee.last_name)),''),
            NULLIF(employee.email,''),'Employee #'||employee.id::text) AS display_name,
          employee.title
        FROM employees employee
        JOIN employee_facilities assignment ON assignment.tenant_id=employee.tenant_id
          AND assignment.employee_id=employee.id AND assignment.facility_id=$2
          AND assignment.deleted IS NULL
        JOIN tenant_memberships membership ON membership.tenant_id=employee.tenant_id
          AND membership.user_id=employee.user_id AND membership.deleted IS NULL
        WHERE employee.tenant_id=$1 AND employee.user_id IS NOT NULL
          AND employee.deleted IS NULL AND employee.hired<=transaction_timestamp()
          AND (employee.terminated IS NULL OR employee.terminated>transaction_timestamp())
          AND (membership.all_facilities OR EXISTS(
            SELECT 1 FROM user_facilities site
            WHERE site.tenant_id=membership.tenant_id AND site.user_id=membership.user_id
              AND site.facility_id=$2 AND site.deleted IS NULL))
          AND ($3::bigint IS NULL OR membership.all_inventory_owners OR EXISTS(
            SELECT 1 FROM user_inventory_owners owner_scope
            WHERE owner_scope.tenant_id=membership.tenant_id
              AND owner_scope.user_id=membership.user_id
              AND owner_scope.inventory_owner_id=$3 AND owner_scope.deleted IS NULL))
          AND EXISTS(SELECT 1 FROM granted_roles role
            JOIN role_permissions role_permission ON role_permission.tenant_id=$1
              AND role_permission.role_id=role.role_id AND role_permission.deleted IS NULL
            JOIN permissions permission ON permission.tenant_id=role_permission.tenant_id
              AND permission.id=role_permission.permission_id AND permission.deleted IS NULL
            WHERE role.user_id=employee.user_id
              AND lower(permission.name) IN ('admin','wms'))
          AND ($4::bigint IS NULL OR employee.id>$4)
        ORDER BY employee.id LIMIT $5"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.facility_id.get())
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.cursor.map(|cursor| cursor.after_employee_id.get()))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;

    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(|row| {
            Ok(WorkOrchestrationWorkerOptionReadModel {
                employee_id: EmployeeId::new(row.try_get("employee_id")?).map_err(invalid_data)?,
                user_id: UserId::new(row.try_get("user_id")?).map_err(invalid_data)?,
                display_name: row.try_get("display_name")?,
                title: row.try_get("title")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = (rows.len() > usize::from(query.limit))
        .then(|| items.last())
        .flatten()
        .map(|item| WorkOrchestrationWorkerCursor {
            after_employee_id: item.employee_id,
        });
    tx.commit().await?;
    Ok(WorkOrchestrationWorkerPage { items, next_cursor })
}
