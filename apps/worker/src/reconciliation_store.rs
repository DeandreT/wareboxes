use anyhow::{bail, Context};
use async_trait::async_trait;
use wareboxes_application::inventory_integrity::InventoryReconciliationRunResult;
use wareboxes_domain::{TenantId, Timestamp};
use wareboxes_persistence_postgres::{db::Db, inventory_reconciliation};
use wareboxes_worker::InventoryReconciliationStore;

#[derive(Clone)]
pub struct PostgresInventoryReconciliationStore {
    db: Db,
}

impl PostgresInventoryReconciliationStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl InventoryReconciliationStore for PostgresInventoryReconciliationStore {
    async fn active_tenants(
        &self,
        after: Option<TenantId>,
        limit: usize,
    ) -> anyhow::Result<Vec<TenantId>> {
        if !(1..=10_000).contains(&limit) {
            bail!("tenant page limit must be between 1 and 10000");
        }
        let rows: Vec<i64> = sqlx::query_scalar(
            r#"
            SELECT id
            FROM tenants
            WHERE deleted IS NULL
              AND status = 'active'
              AND id > $1
            ORDER BY id
            LIMIT $2
            "#,
        )
        .bind(after.map_or(0, TenantId::get))
        .bind(i64::try_from(limit).context("tenant page limit does not fit in i64")?)
        .fetch_all(&self.db)
        .await?;
        rows.into_iter()
            .map(|id| TenantId::new(id).context("database returned an invalid tenant ID"))
            .collect()
    }

    async fn reconcile(
        &self,
        tenant_id: TenantId,
        worker_id: &str,
        scheduled_for: Timestamp,
        interval_seconds: i64,
    ) -> anyhow::Result<InventoryReconciliationRunResult> {
        inventory_reconciliation::execute(
            &self.db,
            tenant_id,
            worker_id,
            scheduled_for,
            interval_seconds,
        )
        .await
        .map_err(Into::into)
    }
}
