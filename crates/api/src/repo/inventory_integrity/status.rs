use sqlx::Row;
use wareboxes_application::inventory_integrity::{
    InventoryReconciliationCoverage, InventoryReconciliationHealth,
    InventoryReconciliationMonitorState, InventoryReconciliationStatusReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::InventoryReconciliationRunId;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn reconciliation_status(
    db: &Db,
    access: &TenantAccess,
) -> AppResult<InventoryReconciliationStatusReadModel> {
    let mut tx = crate::db::begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let row = sqlx::query(
        r#"
        WITH issue AS (
          SELECT 'journal_projection'::TEXT AS kind,
            reconciliation.inventory_owner_id,reconciliation.facility_id,
            ABS(reconciliation.variance)::BIGINT AS severity
          FROM inventory_reconciliation reconciliation
          WHERE reconciliation.tenant_id=$1
            AND ($2 OR reconciliation.facility_id=ANY($3))
            AND ($4 OR reconciliation.inventory_owner_id=ANY($5))
          UNION ALL
          SELECT 'commitments'::TEXT,reconciliation.inventory_owner_id,
            reconciliation.facility_id,GREATEST(
              ABS(reconciliation.qty_reserved-reconciliation.allocated_qty),
              ABS(reconciliation.qty_held-reconciliation.held_qty),
              reconciliation.overcommitted_qty)::BIGINT
          FROM inventory_hold_reconciliation reconciliation
          WHERE reconciliation.tenant_id=$1
            AND ($2 OR reconciliation.facility_id=ANY($3))
            AND ($4 OR reconciliation.inventory_owner_id=ANY($5))
        ), aggregate AS (
          SELECT COUNT(*) FILTER(WHERE kind='journal_projection')::BIGINT
                   AS journal_projection_issue_count,
                 COUNT(*) FILTER(WHERE kind='commitments')::BIGINT
                   AS commitment_issue_count,
                 COUNT(DISTINCT inventory_owner_id)::BIGINT
                   AS affected_inventory_owner_count,
                 COUNT(DISTINCT facility_id)::BIGINT AS affected_facility_count,
                 COALESCE(MAX(severity),0)::BIGINT AS max_severity_quantity
          FROM issue
        )
        SELECT clock_timestamp() AS observed_at,state.last_run_id,
          state.last_scheduled_for,state.last_completed_at,state.next_due_at,
          state.revision AS state_revision,
          aggregate.journal_projection_issue_count,
          aggregate.commitment_issue_count,
          aggregate.affected_inventory_owner_count,
          aggregate.affected_facility_count,aggregate.max_severity_quantity
        FROM aggregate
        LEFT JOIN inventory_reconciliation_state state ON state.tenant_id=$1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let observed_at = row.try_get("observed_at")?;
    let next_due_at = row.try_get("next_due_at")?;
    let last_run_id = row
        .try_get::<Option<i64>, _>("last_run_id")?
        .map(InventoryReconciliationRunId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let journal_projection_issue_count = row.try_get("journal_projection_issue_count")?;
    let commitment_issue_count = row.try_get("commitment_issue_count")?;
    Ok(InventoryReconciliationStatusReadModel {
        monitor_state: match (last_run_id, next_due_at) {
            (None, _) => InventoryReconciliationMonitorState::NeverRun,
            (Some(_), Some(due_at)) if observed_at > due_at => {
                InventoryReconciliationMonitorState::Overdue
            }
            (Some(_), _) => InventoryReconciliationMonitorState::Current,
        },
        coverage: if scope.all_facilities && scope.all_inventory_owners {
            InventoryReconciliationCoverage::FullTenant
        } else {
            InventoryReconciliationCoverage::AccessScope
        },
        last_run_id,
        last_scheduled_for: row.try_get("last_scheduled_for")?,
        last_completed_at: row.try_get("last_completed_at")?,
        next_due_at,
        state_revision: row.try_get("state_revision")?,
        observed_at,
        health: if journal_projection_issue_count == 0 && commitment_issue_count == 0 {
            InventoryReconciliationHealth::Healthy
        } else {
            InventoryReconciliationHealth::IssuesDetected
        },
        journal_projection_issue_count,
        commitment_issue_count,
        affected_inventory_owner_count: row.try_get("affected_inventory_owner_count")?,
        affected_facility_count: row.try_get("affected_facility_count")?,
        max_severity_quantity: row.try_get("max_severity_quantity")?,
    })
}
