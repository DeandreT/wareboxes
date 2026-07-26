use std::time::Duration;

use anyhow::{bail, Context};
use async_trait::async_trait;
use wareboxes_domain::TenantId;
use wareboxes_server::db::Db;
use wareboxes_server::repo::outbox::{self, FailOutboxEvent, OutboxEvent};

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
        batch_size: i64,
        lease: Duration,
    ) -> anyhow::Result<Vec<OutboxEvent>>;

    async fn mark_published(&self, event: &OutboxEvent, worker_id: &str) -> anyhow::Result<bool>;

    async fn mark_failed(
        &self,
        event: &OutboxEvent,
        worker_id: &str,
        error: &str,
        retry_after: Duration,
        max_attempts: i32,
    ) -> anyhow::Result<bool>;
}

#[derive(Clone)]
pub struct PostgresOutboxStore {
    db: Db,
}

impl PostgresOutboxStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }
}

#[async_trait]
impl OutboxStore for PostgresOutboxStore {
    async fn delivery_tenants(
        &self,
        after: Option<TenantId>,
        limit: usize,
    ) -> anyhow::Result<Vec<TenantId>> {
        if !(1..=10_000).contains(&limit) {
            bail!("tenant page limit must be between 1 and 10000");
        }
        let limit = i64::try_from(limit).context("tenant page limit does not fit in i64")?;
        let rows: Vec<i64> = sqlx::query_scalar(
            r#"
            SELECT id
            FROM tenants
            WHERE deleted IS NULL
              AND id > $1
            ORDER BY id
            LIMIT $2
            "#,
        )
        .bind(after.map_or(0, TenantId::get))
        .bind(limit)
        .fetch_all(&self.db)
        .await?;
        rows.into_iter()
            .map(|id| TenantId::new(id).context("database returned an invalid tenant ID"))
            .collect()
    }

    async fn claim(
        &self,
        tenant_id: TenantId,
        worker_id: &str,
        batch_size: i64,
        lease: Duration,
    ) -> anyhow::Result<Vec<OutboxEvent>> {
        outbox::claim_events(
            &self.db,
            tenant_id,
            worker_id,
            batch_size,
            duration_seconds(lease, "outbox lease")?,
        )
        .await
        .map_err(Into::into)
    }

    async fn mark_published(&self, event: &OutboxEvent, worker_id: &str) -> anyhow::Result<bool> {
        outbox::mark_published(
            &self.db,
            event.tenant_id,
            event.id,
            worker_id,
            event.claim_version,
        )
        .await
        .map_err(Into::into)
    }

    async fn mark_failed(
        &self,
        event: &OutboxEvent,
        worker_id: &str,
        error: &str,
        retry_after: Duration,
        max_attempts: i32,
    ) -> anyhow::Result<bool> {
        outbox::mark_failed(
            &self.db,
            &FailOutboxEvent {
                tenant_id: event.tenant_id,
                event_id: event.id,
                worker_id,
                claim_version: event.claim_version,
                error,
                retry_after_seconds: duration_seconds(retry_after, "outbox retry delay")?,
                max_attempts,
            },
        )
        .await
        .map_err(Into::into)
    }
}

fn duration_seconds(duration: Duration, label: &str) -> anyhow::Result<i64> {
    let seconds = duration
        .as_secs()
        .checked_add(u64::from(duration.subsec_nanos() != 0))
        .with_context(|| format!("{label} does not fit in whole seconds"))?;
    i64::try_from(seconds).with_context(|| format!("{label} does not fit in i64 seconds"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_conversion_rounds_up() {
        assert_eq!(
            duration_seconds(Duration::from_millis(1), "duration").unwrap(),
            1
        );
        assert_eq!(
            duration_seconds(Duration::from_millis(1_001), "duration").unwrap(),
            2
        );
        assert_eq!(
            duration_seconds(Duration::from_secs(2), "duration").unwrap(),
            2
        );
    }
}
