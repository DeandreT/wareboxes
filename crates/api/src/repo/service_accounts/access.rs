use std::collections::HashSet;

use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, ServiceAccountAccessPolicy, ServiceAccountId, TenantId, UserId,
};

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;

pub(super) async fn bind_actor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: UserId,
) -> AppResult<()> {
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(actor_id.get().to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(super) async fn validate_access_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_access: &TenantAccess,
    policy: &ServiceAccountAccessPolicy,
) -> AppResult<()> {
    policy
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let actor_scope = ScopeBindings::for_access(actor_access);
    if policy.all_facilities && !actor_scope.all_facilities {
        return Err(AppError::forbidden());
    }
    if policy
        .facility_ids
        .iter()
        .any(|id| !actor_scope.includes_facility(id.get()))
    {
        return Err(AppError::forbidden());
    }
    if policy.all_inventory_owners && !actor_scope.all_inventory_owners {
        return Err(AppError::forbidden());
    }
    if policy
        .inventory_owner_ids
        .iter()
        .any(|id| !actor_scope.includes_inventory_owner(id.get()))
    {
        return Err(AppError::forbidden());
    }

    let facility_ids = policy
        .facility_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let facility_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM facilities WHERE tenant_id=$1 AND id=ANY($2) AND deleted IS NULL",
    )
    .bind(actor_access.tenant_id.get())
    .bind(&facility_ids)
    .fetch_one(&mut **tx)
    .await?;
    if facility_count != i64::try_from(facility_ids.len()).map_err(internal)? {
        return Err(AppError::not_found("service account facility scope"));
    }

    let owner_ids = policy
        .inventory_owner_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let owner_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inventory_owners WHERE tenant_id=$1 AND id=ANY($2) AND deleted IS NULL",
    )
    .bind(actor_access.tenant_id.get())
    .bind(&owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if owner_count != i64::try_from(owner_ids.len()).map_err(internal)? {
        return Err(AppError::not_found("service account inventory-owner scope"));
    }
    if !policy.all_inventory_owners {
        let linked_owner_count: i64 = sqlx::query_scalar(
            r#"SELECT count(DISTINCT assignment.inventory_owner_id)
            FROM inventory_owner_facilities assignment
            WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=ANY($2)
              AND assignment.deleted IS NULL
              AND ($3 OR assignment.facility_id=ANY($4))"#,
        )
        .bind(actor_access.tenant_id.get())
        .bind(&owner_ids)
        .bind(policy.all_facilities)
        .bind(&facility_ids)
        .fetch_one(&mut **tx)
        .await?;
        if linked_owner_count != i64::try_from(owner_ids.len()).map_err(internal)? {
            return Err(AppError::not_found("service account owner-facility scope"));
        }
    }

    let permission_count: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM permissions WHERE tenant_id=$1 AND name=ANY($2)
        AND name<>'admin' AND deleted IS NULL"#,
    )
    .bind(actor_access.tenant_id.get())
    .bind(&policy.permission_names)
    .fetch_one(&mut **tx)
    .await?;
    if permission_count != i64::try_from(policy.permission_names.len()).map_err(internal)? {
        return Err(AppError::not_found("service account permission"));
    }
    Ok(())
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

pub(super) async fn replace_access_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    service_account_id: ServiceAccountId,
    actor_id: UserId,
    occurred_at: wareboxes_domain::Timestamp,
    policy: &ServiceAccountAccessPolicy,
) -> AppResult<()> {
    replace_facilities_tx(
        tx,
        tenant_id,
        service_account_id,
        actor_id,
        occurred_at,
        &policy.facility_ids,
    )
    .await?;
    replace_owners_tx(
        tx,
        tenant_id,
        service_account_id,
        actor_id,
        occurred_at,
        &policy.inventory_owner_ids,
    )
    .await?;
    replace_permissions_tx(
        tx,
        tenant_id,
        service_account_id,
        actor_id,
        occurred_at,
        &policy.permission_names,
    )
    .await
}

async fn replace_facilities_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    account_id: ServiceAccountId,
    actor_id: UserId,
    at: wareboxes_domain::Timestamp,
    desired: &[FacilityId],
) -> AppResult<()> {
    let desired = desired.iter().map(|id| id.get()).collect::<Vec<_>>();
    sqlx::query(
        r#"UPDATE service_account_facilities SET revoked_at=$4,revoked_by_user_id=$3
        WHERE tenant_id=$1 AND service_account_id=$2 AND revoked_at IS NULL
          AND NOT(facility_id=ANY($5))"#,
    )
    .bind(tenant_id.get())
    .bind(account_id.get())
    .bind(actor_id.get())
    .bind(at)
    .bind(&desired)
    .execute(&mut **tx)
    .await?;
    let existing = sqlx::query_scalar::<_, i64>(
        r#"SELECT facility_id FROM service_account_facilities
        WHERE tenant_id=$1 AND service_account_id=$2 AND revoked_at IS NULL"#,
    )
    .bind(tenant_id.get())
    .bind(account_id.get())
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    for facility_id in desired
        .into_iter()
        .filter(|facility_id| !existing.contains(facility_id))
    {
        sqlx::query(
            r#"INSERT INTO service_account_facilities
            (tenant_id,service_account_id,facility_id,granted_at,granted_by_user_id)
            VALUES($1,$2,$3,$4,$5)"#,
        )
        .bind(tenant_id.get())
        .bind(account_id.get())
        .bind(facility_id)
        .bind(at)
        .bind(actor_id.get())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn replace_owners_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    account_id: ServiceAccountId,
    actor_id: UserId,
    at: wareboxes_domain::Timestamp,
    desired: &[InventoryOwnerId],
) -> AppResult<()> {
    let desired = desired.iter().map(|id| id.get()).collect::<Vec<_>>();
    sqlx::query(
        r#"UPDATE service_account_inventory_owners SET revoked_at=$4,revoked_by_user_id=$3
        WHERE tenant_id=$1 AND service_account_id=$2 AND revoked_at IS NULL
          AND NOT(inventory_owner_id=ANY($5))"#,
    )
    .bind(tenant_id.get())
    .bind(account_id.get())
    .bind(actor_id.get())
    .bind(at)
    .bind(&desired)
    .execute(&mut **tx)
    .await?;
    let existing = sqlx::query_scalar::<_, i64>(
        r#"SELECT inventory_owner_id FROM service_account_inventory_owners
        WHERE tenant_id=$1 AND service_account_id=$2 AND revoked_at IS NULL"#,
    )
    .bind(tenant_id.get())
    .bind(account_id.get())
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    for owner_id in desired
        .into_iter()
        .filter(|owner_id| !existing.contains(owner_id))
    {
        sqlx::query(
            r#"INSERT INTO service_account_inventory_owners
            (tenant_id,service_account_id,inventory_owner_id,granted_at,granted_by_user_id)
            VALUES($1,$2,$3,$4,$5)"#,
        )
        .bind(tenant_id.get())
        .bind(account_id.get())
        .bind(owner_id)
        .bind(at)
        .bind(actor_id.get())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn replace_permissions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    account_id: ServiceAccountId,
    actor_id: UserId,
    at: wareboxes_domain::Timestamp,
    desired: &[String],
) -> AppResult<()> {
    sqlx::query(
        r#"UPDATE service_account_permissions grant_record
        SET revoked_at=$4,revoked_by_user_id=$3 FROM permissions permission
        WHERE grant_record.tenant_id=$1 AND grant_record.service_account_id=$2
          AND grant_record.revoked_at IS NULL AND permission.tenant_id=grant_record.tenant_id
          AND permission.id=grant_record.permission_id AND NOT(permission.name=ANY($5))"#,
    )
    .bind(tenant_id.get())
    .bind(account_id.get())
    .bind(actor_id.get())
    .bind(at)
    .bind(desired)
    .execute(&mut **tx)
    .await?;
    let existing = sqlx::query_scalar::<_, String>(
        r#"SELECT permission.name FROM service_account_permissions grant_record
        JOIN permissions permission ON permission.tenant_id=grant_record.tenant_id
          AND permission.id=grant_record.permission_id
        WHERE grant_record.tenant_id=$1 AND grant_record.service_account_id=$2
          AND grant_record.revoked_at IS NULL"#,
    )
    .bind(tenant_id.get())
    .bind(account_id.get())
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    for permission in desired
        .iter()
        .filter(|permission| !existing.contains(*permission))
    {
        sqlx::query(
            r#"INSERT INTO service_account_permissions
            (tenant_id,service_account_id,permission_id,granted_at,granted_by_user_id)
            SELECT $1,$2,permission.id,$3,$4 FROM permissions permission
            WHERE permission.tenant_id=$1 AND permission.name=$5 AND permission.deleted IS NULL"#,
        )
        .bind(tenant_id.get())
        .bind(account_id.get())
        .bind(at)
        .bind(actor_id.get())
        .bind(permission)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
