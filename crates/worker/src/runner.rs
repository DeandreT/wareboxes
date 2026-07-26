use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use tokio::task::JoinSet;
use wareboxes_domain::TenantId;
use wareboxes_server::repo::outbox::OutboxEvent;

use crate::publisher::{FailureClass, PublishError, Publisher};
use crate::store::OutboxStore;

const MAX_PERSISTED_ERROR_CHARS: usize = 1_000;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub batch_size: i64,
    pub lease: Duration,
    pub publish_timeout: Duration,
    pub retry_delay: Duration,
    pub retry_delay_cap: Duration,
    pub max_attempts: i32,
}

impl WorkerConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !(1..=1_000).contains(&self.batch_size) {
            bail!("worker batch size must be between 1 and 1000");
        }
        if self.lease.is_zero() {
            bail!("worker lease must be positive");
        }
        if self.publish_timeout.is_zero() {
            bail!("publisher timeout must be positive");
        }
        if self.publish_timeout >= self.lease {
            bail!("publisher timeout must be shorter than the claim lease");
        }
        if self.retry_delay_cap < self.retry_delay {
            bail!("retry delay cap must not be shorter than the base retry delay");
        }
        if self.max_attempts <= 0 {
            bail!("maximum delivery attempts must be positive");
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RunSummary {
    pub claimed: u64,
    pub published: u64,
    pub retryable_failures: u64,
    pub permanent_failures: u64,
    pub lost_claims: u64,
}

impl RunSummary {
    fn record(&mut self, outcome: EventOutcome) {
        match outcome {
            EventOutcome::Published => self.published += 1,
            EventOutcome::RetryableFailure => self.retryable_failures += 1,
            EventOutcome::PermanentFailure => self.permanent_failures += 1,
            EventOutcome::LostClaim => self.lost_claims += 1,
        }
    }

    fn merge(&mut self, other: Self) {
        self.claimed += other.claimed;
        self.published += other.published;
        self.retryable_failures += other.retryable_failures;
        self.permanent_failures += other.permanent_failures;
        self.lost_claims += other.lost_claims;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventOutcome {
    Published,
    RetryableFailure,
    PermanentFailure,
    LostClaim,
}

pub struct Worker<S, P> {
    store: Arc<S>,
    publisher: Arc<P>,
    worker_id: Arc<str>,
    config: WorkerConfig,
}

impl<S, P> Worker<S, P>
where
    S: OutboxStore,
    P: Publisher,
{
    pub fn new(
        store: Arc<S>,
        publisher: Arc<P>,
        worker_id: impl Into<String>,
        config: WorkerConfig,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        let worker_id = worker_id.into();
        if worker_id.trim().is_empty() || worker_id.len() > 200 {
            bail!("worker ID must contain between 1 and 200 characters");
        }
        Ok(Self {
            store,
            publisher,
            worker_id: Arc::from(worker_id),
            config,
        })
    }

    pub async fn run_once(&self, tenant_id: TenantId) -> anyhow::Result<RunSummary> {
        let events = self
            .store
            .claim(
                tenant_id,
                &self.worker_id,
                self.config.batch_size,
                self.config.lease,
            )
            .await?;
        let mut summary = RunSummary {
            claimed: u64::try_from(events.len()).context("claimed event count exceeds u64")?,
            ..RunSummary::default()
        };
        let mut tasks = JoinSet::new();
        for event in events {
            let store = Arc::clone(&self.store);
            let publisher = Arc::clone(&self.publisher);
            let worker_id = Arc::clone(&self.worker_id);
            let config = self.config.clone();
            tasks.spawn(
                async move { process_event(store, publisher, worker_id, config, event).await },
            );
        }

        let mut first_error = None;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(outcome)) => summary.record(outcome),
                Ok(Err(error)) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(anyhow::Error::new(error).context(
                            "joining an outbox event task after publisher panic containment",
                        ));
                    }
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(summary)
    }

    pub async fn run_cycle(&self, tenant_ids: &[TenantId]) -> anyhow::Result<RunSummary> {
        let mut summary = RunSummary::default();
        let mut first_error = None;
        for tenant_id in tenant_ids {
            match self.run_once(*tenant_id).await {
                Ok(tenant_summary) => summary.merge(tenant_summary),
                Err(error) if first_error.is_none() => {
                    first_error = Some(error.context(format!(
                        "processing outbox events for tenant {}",
                        tenant_id.get()
                    )));
                }
                Err(_) => {}
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(summary)
    }

    pub fn publisher_name(&self) -> &'static str {
        self.publisher.name()
    }
}

async fn process_event<S, P>(
    store: Arc<S>,
    publisher: Arc<P>,
    worker_id: Arc<str>,
    config: WorkerConfig,
    event: OutboxEvent,
) -> anyhow::Result<EventOutcome>
where
    S: OutboxStore,
    P: Publisher,
{
    let publish_event = event.clone();
    let publish = tokio::spawn(async move {
        tokio::time::timeout(config.publish_timeout, publisher.publish(&publish_event)).await
    })
    .await;

    match publish {
        Ok(Ok(Ok(()))) => {
            if store.mark_published(&event, &worker_id).await? {
                Ok(EventOutcome::Published)
            } else {
                Ok(EventOutcome::LostClaim)
            }
        }
        Ok(Ok(Err(error))) => {
            let failure_class = error.class;
            let retry_after = retry_after(&config, &error);
            let max_attempts = match failure_class {
                FailureClass::Retryable => config.max_attempts,
                FailureClass::Permanent => event.attempts.max(1),
            };
            let diagnostic = bounded_diagnostic(error.code, &error.message);
            if !store
                .mark_failed(&event, &worker_id, &diagnostic, retry_after, max_attempts)
                .await?
            {
                return Ok(EventOutcome::LostClaim);
            }
            Ok(match failure_class {
                FailureClass::Retryable => EventOutcome::RetryableFailure,
                FailureClass::Permanent => EventOutcome::PermanentFailure,
            })
        }
        Ok(Err(_elapsed)) => {
            record_runtime_failure(
                &*store,
                &event,
                &worker_id,
                &config,
                "publisher_timeout",
                "publisher exceeded its configured timeout",
            )
            .await
        }
        Err(join_error) => {
            let message = if join_error.is_panic() {
                "publisher panicked"
            } else {
                "publisher task was cancelled"
            };
            record_runtime_failure(
                &*store,
                &event,
                &worker_id,
                &config,
                "publisher_task_failed",
                message,
            )
            .await
        }
    }
}

async fn record_runtime_failure<S: OutboxStore>(
    store: &S,
    event: &OutboxEvent,
    worker_id: &str,
    config: &WorkerConfig,
    code: &str,
    message: &str,
) -> anyhow::Result<EventOutcome> {
    let diagnostic = bounded_diagnostic(code, message);
    if store
        .mark_failed(
            event,
            worker_id,
            &diagnostic,
            config.retry_delay,
            config.max_attempts,
        )
        .await?
    {
        Ok(EventOutcome::RetryableFailure)
    } else {
        Ok(EventOutcome::LostClaim)
    }
}

fn retry_after(config: &WorkerConfig, error: &PublishError) -> Duration {
    error
        .retry_after
        .unwrap_or(config.retry_delay)
        .max(config.retry_delay)
        .min(config.retry_delay_cap)
}

fn bounded_diagnostic(code: &str, message: &str) -> String {
    format!("{code}: {message}")
        .chars()
        .take(MAX_PERSISTED_ERROR_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::publisher::PublishError;

    #[derive(Debug, Clone, Copy)]
    enum Script {
        Success,
        Retryable,
        Permanent,
        Panic,
        Pending,
    }

    #[derive(Debug, Clone)]
    struct FailureRecord {
        event_id: i64,
        retry_after: Duration,
        max_attempts: i32,
        error: String,
    }

    #[derive(Default)]
    struct FakeStore {
        events: Mutex<HashMap<TenantId, VecDeque<OutboxEvent>>>,
        published: Mutex<Vec<i64>>,
        failed: Mutex<Vec<FailureRecord>>,
        claims: Mutex<Vec<(TenantId, i64)>>,
        claim_failures: Mutex<Vec<TenantId>>,
    }

    impl FakeStore {
        fn insert(&self, event: OutboxEvent) {
            self.events
                .lock()
                .unwrap()
                .entry(event.tenant_id)
                .or_default()
                .push_back(event);
        }
    }

    #[async_trait]
    impl OutboxStore for FakeStore {
        async fn delivery_tenants(
            &self,
            after: Option<TenantId>,
            limit: usize,
        ) -> anyhow::Result<Vec<TenantId>> {
            let after = after.map_or(0, TenantId::get);
            let mut tenants = self
                .events
                .lock()
                .unwrap()
                .keys()
                .copied()
                .filter(|tenant_id| tenant_id.get() > after)
                .collect::<Vec<_>>();
            tenants.sort_unstable_by_key(|tenant_id| tenant_id.get());
            tenants.truncate(limit);
            Ok(tenants)
        }

        async fn claim(
            &self,
            tenant_id: TenantId,
            _worker_id: &str,
            batch_size: i64,
            _lease: Duration,
        ) -> anyhow::Result<Vec<OutboxEvent>> {
            self.claims.lock().unwrap().push((tenant_id, batch_size));
            if self.claim_failures.lock().unwrap().contains(&tenant_id) {
                anyhow::bail!("scripted claim failure");
            }
            let mut events = self.events.lock().unwrap();
            let queue = events.entry(tenant_id).or_default();
            let mut claimed = Vec::new();
            for _ in 0..batch_size {
                let Some(event) = queue.pop_front() else {
                    break;
                };
                claimed.push(event);
            }
            Ok(claimed)
        }

        async fn mark_published(
            &self,
            event: &OutboxEvent,
            _worker_id: &str,
        ) -> anyhow::Result<bool> {
            self.published.lock().unwrap().push(event.id);
            Ok(true)
        }

        async fn mark_failed(
            &self,
            event: &OutboxEvent,
            _worker_id: &str,
            error: &str,
            retry_after: Duration,
            max_attempts: i32,
        ) -> anyhow::Result<bool> {
            self.failed.lock().unwrap().push(FailureRecord {
                event_id: event.id,
                retry_after,
                max_attempts,
                error: error.to_owned(),
            });
            Ok(true)
        }
    }

    struct ScriptedPublisher {
        scripts: Mutex<HashMap<i64, Script>>,
    }

    #[async_trait]
    impl Publisher for ScriptedPublisher {
        fn name(&self) -> &'static str {
            "scripted"
        }

        async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError> {
            let script = self.scripts.lock().unwrap().remove(&event.id).unwrap();
            match script {
                Script::Success => Ok(()),
                Script::Retryable => Err(PublishError::retryable("unavailable", "try again")
                    .with_retry_after(Duration::from_secs(20))),
                Script::Permanent => {
                    Err(PublishError::permanent("invalid_event", "cannot publish"))
                }
                Script::Panic => panic!("scripted publisher panic"),
                Script::Pending => std::future::pending().await,
            }
        }
    }

    #[tokio::test]
    async fn run_once_records_outcomes_and_continues_after_publisher_panic() {
        let tenant_id = TenantId::new(1).unwrap();
        let store = Arc::new(FakeStore::default());
        for id in 1..=5 {
            store.insert(event(tenant_id, id));
        }
        let publisher = Arc::new(ScriptedPublisher {
            scripts: Mutex::new(HashMap::from([
                (1, Script::Success),
                (2, Script::Retryable),
                (3, Script::Permanent),
                (4, Script::Panic),
                (5, Script::Success),
            ])),
        });
        let worker =
            Worker::new(Arc::clone(&store), publisher, "test-worker", test_config(5)).unwrap();

        let summary = worker.run_once(tenant_id).await.unwrap();

        assert_eq!(
            summary,
            RunSummary {
                claimed: 5,
                published: 2,
                retryable_failures: 2,
                permanent_failures: 1,
                lost_claims: 0,
            }
        );
        let mut published = store.published.lock().unwrap().clone();
        published.sort_unstable();
        assert_eq!(published, vec![1, 5]);
        let failed = store.failed.lock().unwrap();
        assert_eq!(failed.len(), 3);
        assert!(failed.iter().any(|failure| failure.event_id == 2
            && failure.retry_after == Duration::from_secs(20)
            && failure.max_attempts == 5));
        assert!(failed
            .iter()
            .any(|failure| failure.event_id == 3 && failure.max_attempts == 1));
        assert!(failed.iter().any(|failure| failure.event_id == 4
            && failure.error == "publisher_task_failed: publisher panicked"));
    }

    #[tokio::test]
    async fn run_cycle_caps_each_tenant_batch() {
        let tenant_a = TenantId::new(1).unwrap();
        let tenant_b = TenantId::new(2).unwrap();
        let store = Arc::new(FakeStore::default());
        for id in 1..=4 {
            store.insert(event(tenant_a, id));
        }
        store.insert(event(tenant_b, 5));
        let publisher = Arc::new(ScriptedPublisher {
            scripts: Mutex::new(HashMap::from([
                (1, Script::Success),
                (2, Script::Success),
                (5, Script::Success),
            ])),
        });
        let worker =
            Worker::new(Arc::clone(&store), publisher, "fair-worker", test_config(2)).unwrap();

        let summary = worker.run_cycle(&[tenant_a, tenant_b]).await.unwrap();

        assert_eq!(summary.claimed, 3);
        let mut published = store.published.lock().unwrap().clone();
        published.sort_unstable();
        assert_eq!(published, vec![1, 2, 5]);
        assert_eq!(
            store.claims.lock().unwrap().as_slice(),
            &[(tenant_a, 2), (tenant_b, 2)]
        );
        assert_eq!(store.events.lock().unwrap()[&tenant_a].len(), 2);
    }

    #[tokio::test]
    async fn run_cycle_gives_later_tenants_a_turn_after_store_error() {
        let tenant_a = TenantId::new(1).unwrap();
        let tenant_b = TenantId::new(2).unwrap();
        let store = Arc::new(FakeStore::default());
        store.claim_failures.lock().unwrap().push(tenant_a);
        store.insert(event(tenant_b, 1));
        let publisher = Arc::new(ScriptedPublisher {
            scripts: Mutex::new(HashMap::from([(1, Script::Success)])),
        });
        let worker =
            Worker::new(Arc::clone(&store), publisher, "fair-worker", test_config(1)).unwrap();

        assert!(worker.run_cycle(&[tenant_a, tenant_b]).await.is_err());

        assert_eq!(store.published.lock().unwrap().as_slice(), &[1]);
        assert_eq!(
            store.claims.lock().unwrap().as_slice(),
            &[(tenant_a, 1), (tenant_b, 1)]
        );
    }

    #[tokio::test]
    async fn run_once_records_publisher_timeout_as_retryable() {
        let tenant_id = TenantId::new(1).unwrap();
        let store = Arc::new(FakeStore::default());
        store.insert(event(tenant_id, 1));
        let publisher = Arc::new(ScriptedPublisher {
            scripts: Mutex::new(HashMap::from([(1, Script::Pending)])),
        });
        let mut config = test_config(1);
        config.publish_timeout = Duration::from_millis(5);
        let worker = Worker::new(Arc::clone(&store), publisher, "test-worker", config).unwrap();

        let summary = worker.run_once(tenant_id).await.unwrap();

        assert_eq!(summary.retryable_failures, 1);
        assert_eq!(
            store.failed.lock().unwrap()[0].error,
            "publisher_timeout: publisher exceeded its configured timeout"
        );
    }

    #[test]
    fn config_rejects_timeouts_that_can_outlive_the_lease() {
        let mut config = test_config(1);
        config.publish_timeout = config.lease;
        assert!(config.validate().is_err());
    }

    fn test_config(batch_size: i64) -> WorkerConfig {
        WorkerConfig {
            batch_size,
            lease: Duration::from_secs(60),
            publish_timeout: Duration::from_secs(30),
            retry_delay: Duration::from_secs(5),
            retry_delay_cap: Duration::from_secs(60),
            max_attempts: 5,
        }
    }

    fn event(tenant_id: TenantId, id: i64) -> OutboxEvent {
        let now = Utc::now();
        OutboxEvent {
            id,
            tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: None,
            created: now,
            event_key: format!("event-{id}"),
            aggregate_type: "test".to_owned(),
            aggregate_id: id.to_string(),
            ordering_key: format!("event-{id}"),
            aggregate_sequence: 1,
            event_type: "test.event".to_owned(),
            schema_version: 1,
            payload: json!({"id": id}),
            occurred_at: now,
            available_at: now,
            claimed_at: Some(now),
            claimed_by: Some("test-worker".to_owned()),
            lease_expires_at: Some(now),
            claim_version: 1,
            attempts: 1,
            last_error: None,
            dead_lettered_at: None,
            replay_count: 0,
            discarded_at: None,
            discard_reason: None,
            discarded_by_user_id: None,
            published_at: None,
        }
    }
}
