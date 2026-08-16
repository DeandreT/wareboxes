use std::sync::Arc;

use anyhow::{bail, Context};
use async_trait::async_trait;
use wareboxes_application::inventory_integrity::InventoryReconciliationRunResult;
use wareboxes_domain::{TenantId, Timestamp};

#[async_trait]
pub trait InventoryReconciliationStore: Send + Sync + 'static {
    async fn active_tenants(
        &self,
        after: Option<TenantId>,
        limit: usize,
    ) -> anyhow::Result<Vec<TenantId>>;

    async fn reconcile(
        &self,
        tenant_id: TenantId,
        worker_id: &str,
        scheduled_for: Timestamp,
        interval_seconds: i64,
    ) -> anyhow::Result<InventoryReconciliationRunResult>;
}

#[derive(Debug, Clone, Copy)]
pub struct InventoryReconciliationConfig {
    pub interval_seconds: i64,
    pub tenant_page_size: usize,
}

impl InventoryReconciliationConfig {
    pub fn validate(self) -> anyhow::Result<()> {
        if !(60..=86_400).contains(&self.interval_seconds) {
            bail!("inventory reconciliation interval must be between 60 and 86400 seconds");
        }
        if !(1..=10_000).contains(&self.tenant_page_size) {
            bail!("inventory reconciliation tenant page size must be between 1 and 10000");
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InventoryReconciliationSummary {
    pub attempted: u64,
    pub completed: u64,
    pub replayed: u64,
    pub alerts: u64,
    pub failures: Vec<InventoryReconciliationFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryReconciliationFailure {
    pub tenant_id: TenantId,
    pub error: String,
}

pub struct InventoryReconciliationWorker<S> {
    store: Arc<S>,
    worker_id: Arc<str>,
    config: InventoryReconciliationConfig,
}

impl<S> InventoryReconciliationWorker<S>
where
    S: InventoryReconciliationStore,
{
    pub fn new(
        store: Arc<S>,
        worker_id: impl Into<String>,
        config: InventoryReconciliationConfig,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        let worker_id = worker_id.into();
        if worker_id.trim().is_empty() || worker_id.chars().count() > 200 {
            bail!("inventory reconciliation worker ID must contain between 1 and 200 characters");
        }
        Ok(Self {
            store,
            worker_id: Arc::from(worker_id),
            config,
        })
    }

    pub async fn run_discovered_cycle(
        &self,
        scheduled_for: Timestamp,
    ) -> anyhow::Result<InventoryReconciliationSummary> {
        let mut summary = InventoryReconciliationSummary::default();
        let mut after = None;
        loop {
            let tenants = self
                .store
                .active_tenants(after, self.config.tenant_page_size)
                .await
                .context("discovering tenants for inventory reconciliation")?;
            if tenants.is_empty() {
                break;
            }
            validate_tenant_page(after, &tenants)?;
            after = tenants.last().copied();
            for tenant_id in &tenants {
                summary.attempted += 1;
                match self
                    .store
                    .reconcile(
                        *tenant_id,
                        &self.worker_id,
                        scheduled_for,
                        self.config.interval_seconds,
                    )
                    .await
                {
                    Ok(result) => {
                        if result.created {
                            summary.completed += 1;
                            summary.alerts += u64::from(result.alert.is_some());
                        } else {
                            summary.replayed += 1;
                        }
                    }
                    Err(error) => summary.failures.push(InventoryReconciliationFailure {
                        tenant_id: *tenant_id,
                        error: format!("{error:#}"),
                    }),
                }
            }
            if tenants.len() < self.config.tenant_page_size {
                break;
            }
        }
        Ok(summary)
    }
}

fn validate_tenant_page(after: Option<TenantId>, tenant_ids: &[TenantId]) -> anyhow::Result<()> {
    let mut previous_id = after.map_or(0, TenantId::get);
    for tenant_id in tenant_ids {
        if tenant_id.get() <= previous_id {
            bail!("inventory reconciliation tenant page must be strictly increasing");
        }
        previous_id = tenant_id.get();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use chrono::{TimeZone, Utc};
    use wareboxes_application::inventory_integrity::{
        InventoryReconciliationAlert, InventoryReconciliationHealth,
    };
    use wareboxes_domain::InventoryReconciliationRunId;

    use super::*;

    struct Store {
        tenants: Vec<TenantId>,
        failures: BTreeMap<i64, &'static str>,
        calls: Mutex<Vec<TenantId>>,
    }

    #[async_trait]
    impl InventoryReconciliationStore for Store {
        async fn active_tenants(
            &self,
            after: Option<TenantId>,
            limit: usize,
        ) -> anyhow::Result<Vec<TenantId>> {
            Ok(self
                .tenants
                .iter()
                .copied()
                .filter(|tenant| after.is_none_or(|cursor| tenant.get() > cursor.get()))
                .take(limit)
                .collect())
        }

        async fn reconcile(
            &self,
            tenant_id: TenantId,
            _worker_id: &str,
            scheduled_for: Timestamp,
            _interval_seconds: i64,
        ) -> anyhow::Result<InventoryReconciliationRunResult> {
            self.calls.lock().unwrap().push(tenant_id);
            if let Some(message) = self.failures.get(&tenant_id.get()) {
                bail!(*message);
            }
            Ok(InventoryReconciliationRunResult {
                run_id: InventoryReconciliationRunId::new(tenant_id.get()).unwrap(),
                tenant_id,
                scheduled_for,
                completed_at: scheduled_for,
                next_due_at: scheduled_for,
                interval_seconds: 60,
                health: InventoryReconciliationHealth::Healthy,
                previous_health: Some(InventoryReconciliationHealth::Healthy),
                journal_projection_issue_count: 0,
                commitment_issue_count: 0,
                affected_inventory_owner_count: 0,
                affected_facility_count: 0,
                max_severity_quantity: 0,
                issue_digest: "0".repeat(32),
                state_revision: 1,
                created: true,
                alert: (tenant_id.get() == 3).then_some(InventoryReconciliationAlert::Restored),
            })
        }
    }

    #[tokio::test]
    async fn pages_tenants_and_continues_after_one_tenant_fails() {
        let store = Arc::new(Store {
            tenants: (1..=5).map(|id| TenantId::new(id).unwrap()).collect(),
            failures: BTreeMap::from([(2, "tenant two failed")]),
            calls: Mutex::new(Vec::new()),
        });
        let worker = InventoryReconciliationWorker::new(
            Arc::clone(&store),
            "reconcile-a",
            InventoryReconciliationConfig {
                interval_seconds: 60,
                tenant_page_size: 2,
            },
        )
        .unwrap();
        let scheduled_for = Utc.with_ymd_and_hms(2026, 8, 15, 12, 30, 0).unwrap();

        let summary = worker.run_discovered_cycle(scheduled_for).await.unwrap();

        assert_eq!(summary.attempted, 5);
        assert_eq!(summary.completed, 4);
        assert_eq!(summary.alerts, 1);
        assert_eq!(summary.failures.len(), 1);
        assert_eq!(summary.failures[0].tenant_id.get(), 2);
        assert_eq!(store.calls.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn rejects_non_monotonic_tenant_pages() {
        let store = Arc::new(Store {
            tenants: vec![TenantId::new(2).unwrap(), TenantId::new(1).unwrap()],
            failures: BTreeMap::new(),
            calls: Mutex::new(Vec::new()),
        });
        let worker = InventoryReconciliationWorker::new(
            store,
            "reconcile-a",
            InventoryReconciliationConfig {
                interval_seconds: 60,
                tenant_page_size: 10,
            },
        )
        .unwrap();
        let scheduled_for = Utc.with_ymd_and_hms(2026, 8, 15, 12, 30, 0).unwrap();
        assert!(worker.run_discovered_cycle(scheduled_for).await.is_err());
    }
}
