use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, UserId};

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;

pub(super) fn invalid_data(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

pub(super) fn require_facility_scope(
    scope: &ScopeBindings,
    facility_id: i64,
    label: &str,
) -> AppResult<()> {
    if scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found(label))
    }
}

pub(super) fn require_owner_scope(
    scope: &ScopeBindings,
    inventory_owner_id: i64,
    label: &str,
) -> AppResult<()> {
    if scope.includes_inventory_owner(inventory_owner_id) {
        Ok(())
    } else {
        Err(AppError::not_found(label))
    }
}

pub(super) fn require_command_scope(
    scope: &ScopeBindings,
    facility_id: FacilityId,
    inventory_owner_id: Option<InventoryOwnerId>,
    label: &str,
) -> AppResult<()> {
    require_facility_scope(scope, facility_id.get(), label)?;
    if let Some(owner_id) = inventory_owner_id {
        require_owner_scope(scope, owner_id.get(), label)?;
    }
    Ok(())
}

pub(super) async fn require_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    inventory_owner_id: Option<InventoryOwnerId>,
    label: &str,
) -> AppResult<()> {
    let Some(owner_id) = inventory_owner_id else {
        return Ok(());
    };
    let exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM inventory_owner_facilities
           WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
             AND deleted IS NULL)"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found(label))
    }
}

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
