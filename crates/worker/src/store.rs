use std::time::Duration;

use async_trait::async_trait;
use wareboxes_application::outbox::OutboxEvent;
use wareboxes_domain::TenantId;

use crate::publisher::FailureClass;

#[async_trait]
pub trait OutboxStore: Send + Sync + 'static {
    async fn delivery_tenants(
        &self,
        after: Option<TenantId>,
        limit: usize,
    ) -> anyhow::Result<Vec<TenantId>>;

    async fn claim(
        &self,
        tenant_id: TenantId,
        worker_id: &str,
        publisher_name: &str,
        batch_size: i64,
        lease: Duration,
    ) -> anyhow::Result<Vec<OutboxEvent>>;

    async fn mark_published(&self, event: &OutboxEvent, worker_id: &str) -> anyhow::Result<bool>;

    async fn mark_failed(
        &self,
        event: &OutboxEvent,
        worker_id: &str,
        error: &str,
        failure_class: FailureClass,
        retry_after: Duration,
        max_attempts: i32,
    ) -> anyhow::Result<bool>;
}
