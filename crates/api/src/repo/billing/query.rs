use wareboxes_application::billing::BillingWorkspace;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{BillingContractId, BillingReconciliationRunId, InventoryOwnerId};

use super::models::{contract, event, rate, read_run_tx};
use super::{require_owner, PERMISSION};
use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn workspace(
    db: &Db,
    access: &TenantAccess,
    inventory_owner_id: Option<InventoryOwnerId>,
    contract_id: Option<BillingContractId>,
    after_run_id: Option<BillingReconciliationRunId>,
    limit: u16,
) -> AppResult<BillingWorkspace> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    if let Some(owner_id) = inventory_owner_id {
        require_owner(&scope, owner_id)?;
    }
    if let Some(contract_id) = contract_id {
        let owner_id = sqlx::query_scalar::<_, i64>(
            "SELECT inventory_owner_id FROM billing_contracts WHERE tenant_id=$1 AND id=$2",
        )
        .bind(access.tenant_id.get())
        .bind(contract_id.get())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::not_found("billing contract"))?;
        let owner_id = InventoryOwnerId::new(owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?;
        require_owner(&scope, owner_id)?;
        if inventory_owner_id.is_some_and(|requested| requested != owner_id) {
            return Err(AppError::not_found("billing contract"));
        }
    }

    let contract_rows = sqlx::query(
        r#"SELECT contract.*,owner.name AS inventory_owner_name
           FROM billing_contracts contract
           JOIN inventory_owners owner ON owner.tenant_id=contract.tenant_id
             AND owner.id=contract.inventory_owner_id
           WHERE contract.tenant_id=$1
             AND ($2 OR contract.inventory_owner_id=ANY($3))
             AND ($4::BIGINT IS NULL OR contract.inventory_owner_id=$4)
             AND ($5::BIGINT IS NULL OR contract.id=$5)
           ORDER BY contract.id DESC LIMIT 200"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(inventory_owner_id.map(InventoryOwnerId::get))
    .bind(contract_id.map(BillingContractId::get))
    .fetch_all(&mut *tx)
    .await?;
    let rate_rows = sqlx::query(
        r#"SELECT * FROM billing_rate_versions rate WHERE tenant_id=$1
             AND ($2 OR inventory_owner_id=ANY($3))
             AND ($4::BIGINT IS NULL OR inventory_owner_id=$4)
             AND ($5::BIGINT IS NULL OR contract_id=$5)
           ORDER BY id DESC LIMIT 500"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(inventory_owner_id.map(InventoryOwnerId::get))
    .bind(contract_id.map(BillingContractId::get))
    .fetch_all(&mut *tx)
    .await?;
    let event_rows = sqlx::query(
        r#"SELECT * FROM billable_events event WHERE tenant_id=$1
             AND ($2 OR inventory_owner_id=ANY($3))
             AND ($4 OR facility_id=ANY($5))
             AND ($6::BIGINT IS NULL OR inventory_owner_id=$6)
             AND ($7::BIGINT IS NULL OR contract_id=$7)
           ORDER BY id DESC LIMIT 500"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(inventory_owner_id.map(InventoryOwnerId::get))
    .bind(contract_id.map(BillingContractId::get))
    .fetch_all(&mut *tx)
    .await?;
    let run_ids = sqlx::query_scalar::<_, i64>(
        r#"SELECT id FROM billing_reconciliation_runs run WHERE tenant_id=$1
             AND ($2 OR inventory_owner_id=ANY($3))
             AND ($4 OR (facility_id IS NOT NULL AND facility_id=ANY($5)))
             AND ($6::BIGINT IS NULL OR inventory_owner_id=$6)
             AND ($7::BIGINT IS NULL OR contract_id=$7)
             AND ($8::BIGINT IS NULL OR id<$8)
           ORDER BY id DESC LIMIT $9"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(inventory_owner_id.map(InventoryOwnerId::get))
    .bind(contract_id.map(BillingContractId::get))
    .bind(after_run_id.map(BillingReconciliationRunId::get))
    .bind(i64::from(limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = run_ids.len() > usize::from(limit);
    let visible_ids = run_ids
        .iter()
        .take(usize::from(limit))
        .copied()
        .collect::<Vec<_>>();
    let mut runs = Vec::with_capacity(visible_ids.len());
    for run_id in &visible_ids {
        runs.push(
            read_run_tx(
                &mut tx,
                access.tenant_id,
                BillingReconciliationRunId::new(*run_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            )
            .await?,
        );
    }
    let next_run_id = has_more
        .then(|| visible_ids.last().copied())
        .flatten()
        .map(BillingReconciliationRunId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let result = BillingWorkspace {
        contracts: contract_rows
            .iter()
            .map(contract)
            .collect::<AppResult<Vec<_>>>()?,
        rates: rate_rows.iter().map(rate).collect::<AppResult<Vec<_>>>()?,
        events: event_rows
            .iter()
            .map(event)
            .collect::<AppResult<Vec<_>>>()?,
        runs,
        next_run_id,
    };
    tx.commit().await?;
    Ok(result)
}
