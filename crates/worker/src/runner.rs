use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tokio::task::{JoinError, JoinSet};
use wareboxes_application::outbox::OutboxEvent;
use wareboxes_domain::TenantId;

use crate::publisher::{FailureClass, PublishError, Publisher};
use crate::store::OutboxStore;

const MAX_PERSISTED_ERROR_CHARS: usize = 1_000;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub batch_size: i64,
    pub max_in_flight: usize,
    pub tenant_page_size: usize,
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
        if !(1..=1_000).contains(&self.max_in_flight) {
            bail!("maximum in-flight deliveries must be between 1 and 1000");
        }
        if !(1..=10_000).contains(&self.tenant_page_size) {
            bail!("tenant page size must be between 1 and 10000");
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
    execution_capacity: Arc<Semaphore>,
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
        let execution_capacity = Arc::new(Semaphore::new(config.max_in_flight));
        Ok(Self {
            store,
            publisher,
            worker_id: Arc::from(worker_id),
            config,
            execution_capacity,
        })
    }

    pub async fn run_once(&self, tenant_id: TenantId) -> anyhow::Result<RunSummary> {
        let mut summary = RunSummary::default();
        let mut tasks = JoinSet::new();
        let mut first_error = None;
        self.claim_and_spawn(tenant_id, &mut tasks, &mut summary)
            .await?;
        drain_tasks(&mut tasks, &mut summary, &mut first_error).await;
        finish_summary(summary, first_error)
    }

    pub async fn run_cycle(&self, tenant_ids: &[TenantId]) -> anyhow::Result<RunSummary> {
        let mut summary = RunSummary::default();
        let mut first_error = None;
        let mut tasks = JoinSet::new();
        self.run_tenants(tenant_ids, &mut tasks, &mut summary, &mut first_error)
            .await;
        drain_tasks(&mut tasks, &mut summary, &mut first_error).await;
        finish_summary(summary, first_error)
    }

    pub async fn run_discovered_cycle(&self) -> anyhow::Result<RunSummary> {
        let mut summary = RunSummary::default();
        let mut first_error = None;
        let mut tasks = JoinSet::new();
        let mut after = None;
        loop {
            collect_ready_tasks(&mut tasks, &mut summary, &mut first_error);
            let tenant_ids = match self
                .store
                .delivery_tenants(after, self.config.tenant_page_size)
                .await
            {
                Ok(tenant_ids) => tenant_ids,
                Err(error) => {
                    record_first_error(
                        &mut first_error,
                        error.context("discovering tenants for outbox delivery"),
                    );
                    break;
                }
            };
            if tenant_ids.is_empty() {
                break;
            }
            if let Err(error) = validate_tenant_page(after, &tenant_ids) {
                record_first_error(&mut first_error, error);
                break;
            }
            after = tenant_ids.last().copied();
            self.run_tenants(&tenant_ids, &mut tasks, &mut summary, &mut first_error)
                .await;
            if tenant_ids.len() < self.config.tenant_page_size {
                break;
            }
        }
        drain_tasks(&mut tasks, &mut summary, &mut first_error).await;
        finish_summary(summary, first_error)
    }

    pub fn publisher_name(&self) -> &'static str {
        self.publisher.name()
    }

    async fn run_tenants(
        &self,
        tenant_ids: &[TenantId],
        tasks: &mut JoinSet<anyhow::Result<EventOutcome>>,
        summary: &mut RunSummary,
        first_error: &mut Option<anyhow::Error>,
    ) {
        for tenant_id in tenant_ids {
            collect_ready_tasks(tasks, summary, first_error);
            if let Err(error) = self.claim_and_spawn(*tenant_id, tasks, summary).await {
                record_first_error(
                    first_error,
                    error.context(format!(
                        "processing outbox events for tenant {}",
                        tenant_id.get()
                    )),
                );
            }
        }
    }

    async fn reserve_claim_slots(&self) -> anyhow::Result<OwnedSemaphorePermit> {
        let desired_capacity = usize::try_from(self.config.batch_size)
            .context("outbox batch size does not fit in usize")?
            .min(self.config.max_in_flight);
        let mut permits = Arc::clone(&self.execution_capacity)
            .acquire_owned()
            .await
            .context("worker execution capacity was closed")?;
        while permits.num_permits() < desired_capacity {
            match Arc::clone(&self.execution_capacity).try_acquire_owned() {
                Ok(permit) => permits.merge(permit),
                Err(TryAcquireError::NoPermits) => break,
                Err(TryAcquireError::Closed) => {
                    bail!("worker execution capacity was closed");
                }
            }
        }
        Ok(permits)
    }

    async fn claim_and_spawn(
        &self,
        tenant_id: TenantId,
        tasks: &mut JoinSet<anyhow::Result<EventOutcome>>,
        summary: &mut RunSummary,
    ) -> anyhow::Result<()> {
        let mut execution_permits = self.reserve_claim_slots().await?;
        let claim_limit = execution_permits.num_permits();
        let events = self
            .store
            .claim(
                tenant_id,
                &self.worker_id,
                self.publisher.name(),
                i64::try_from(claim_limit).context("outbox claim limit does not fit in i64")?,
                self.config.lease,
            )
            .await?;
        if events.len() > claim_limit {
            bail!("outbox store returned more events than the requested claim limit");
        }
        let unused_permits = claim_limit - events.len();
        if unused_permits > 0 {
            drop(
                execution_permits.split(unused_permits).context(
                    "reserved worker capacity is smaller than the unused claim capacity",
                )?,
            );
        }
        summary.claimed +=
            u64::try_from(events.len()).context("claimed event count exceeds u64")?;
        for event in events {
            let execution_permit = execution_permits
                .split(1)
                .context("reserved worker capacity is smaller than the claimed event count")?;
            let store = Arc::clone(&self.store);
            let publisher = Arc::clone(&self.publisher);
            let worker_id = Arc::clone(&self.worker_id);
            let config = self.config.clone();
            tasks.spawn(async move {
                let _execution_permit = execution_permit;
                process_event(store, publisher, worker_id, config, event).await
            });
        }
        Ok(())
    }
}

fn record_first_error(first_error: &mut Option<anyhow::Error>, error: anyhow::Error) {
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

fn record_task_result(
    result: Result<anyhow::Result<EventOutcome>, JoinError>,
    summary: &mut RunSummary,
    first_error: &mut Option<anyhow::Error>,
) {
    match result {
        Ok(Ok(outcome)) => summary.record(outcome),
        Ok(Err(error)) => record_first_error(first_error, error),
        Err(error) => record_first_error(
            first_error,
            anyhow::Error::new(error)
                .context("joining an outbox event task after publisher panic containment"),
        ),
    }
}

fn collect_ready_tasks(
    tasks: &mut JoinSet<anyhow::Result<EventOutcome>>,
    summary: &mut RunSummary,
    first_error: &mut Option<anyhow::Error>,
) {
    while let Some(result) = tasks.try_join_next() {
        record_task_result(result, summary, first_error);
    }
}

async fn drain_tasks(
    tasks: &mut JoinSet<anyhow::Result<EventOutcome>>,
    summary: &mut RunSummary,
    first_error: &mut Option<anyhow::Error>,
) {
    while let Some(result) = tasks.join_next().await {
        record_task_result(result, summary, first_error);
    }
}

fn finish_summary(
    summary: RunSummary,
    first_error: Option<anyhow::Error>,
) -> anyhow::Result<RunSummary> {
    if let Some(error) = first_error {
        Err(error)
    } else {
        Ok(summary)
    }
}

fn validate_tenant_page(after: Option<TenantId>, tenant_ids: &[TenantId]) -> anyhow::Result<()> {
    let mut previous_id = after.map_or(0, TenantId::get);
    for tenant_id in tenant_ids {
        if tenant_id.get() <= previous_id {
            bail!("tenant discovery page must be strictly increasing after its cursor");
        }
        previous_id = tenant_id.get();
    }
    Ok(())
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
                .mark_failed(
                    &event,
                    &worker_id,
                    &diagnostic,
                    failure_class,
                    retry_after,
                    max_attempts,
                )
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
            FailureClass::Retryable,
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::json;
    use tokio::sync::Semaphore;

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
        tenant_pages: Mutex<Vec<(Option<TenantId>, usize)>>,
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
            self.tenant_pages.lock().unwrap().push((after, limit));
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
            _publisher_name: &str,
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
            _failure_class: FailureClass,
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

    struct BlockingPublisher {
        active: AtomicUsize,
        max_seen: AtomicUsize,
        calls: AtomicUsize,
        started: Semaphore,
        release: Semaphore,
    }

    impl BlockingPublisher {
        fn new() -> Self {
            Self {
                active: AtomicUsize::new(0),
                max_seen: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
                started: Semaphore::new(0),
                release: Semaphore::new(0),
            }
        }
    }

    #[async_trait]
    impl Publisher for BlockingPublisher {
        fn name(&self) -> &'static str {
            "blocking"
        }

        async fn publish(&self, _event: &OutboxEvent) -> Result<(), PublishError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_seen.fetch_max(active, Ordering::SeqCst);
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
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
    async fn publisher_concurrency_and_claims_are_bounded_by_shared_capacity() {
        let tenant_a = TenantId::new(1).unwrap();
        let tenant_b = TenantId::new(2).unwrap();
        let store = Arc::new(FakeStore::default());
        store.insert(event(tenant_a, 1));
        for id in 2..=6 {
            store.insert(event(tenant_b, id));
        }
        let publisher = Arc::new(BlockingPublisher::new());
        let mut config = test_config(10);
        config.max_in_flight = 2;
        let worker = Arc::new(
            Worker::new(
                Arc::clone(&store),
                Arc::clone(&publisher),
                "bounded-worker",
                config,
            )
            .unwrap(),
        );

        let worker_a = Arc::clone(&worker);
        let run_a = tokio::spawn(async move { worker_a.run_once(tenant_a).await });
        publisher.started.acquire().await.unwrap().forget();
        let worker_b = Arc::clone(&worker);
        let run_b = tokio::spawn(async move { worker_b.run_once(tenant_b).await });
        publisher.started.acquire().await.unwrap().forget();

        assert_eq!(publisher.active.load(Ordering::SeqCst), 2);
        assert_eq!(publisher.max_seen.load(Ordering::SeqCst), 2);
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            store.claims.lock().unwrap().as_slice(),
            &[(tenant_a, 2), (tenant_b, 1)]
        );
        assert_eq!(store.events.lock().unwrap()[&tenant_b].len(), 4);
        assert!(store.published.lock().unwrap().is_empty());

        publisher.release.add_permits(2);
        let summary_a = run_a.await.unwrap().unwrap();
        let summary_b = run_b.await.unwrap().unwrap();
        assert_eq!(summary_a.claimed, 1);
        assert_eq!(summary_a.published, 1);
        assert_eq!(summary_b.claimed, 1);
        assert_eq!(summary_b.published, 1);
        assert_eq!(publisher.active.load(Ordering::SeqCst), 0);
        assert_eq!(publisher.max_seen.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn paged_discovery_continues_after_tenant_error_without_starvation() {
        let tenants = (1..=5)
            .map(|id| TenantId::new(id).unwrap())
            .collect::<Vec<_>>();
        let store = Arc::new(FakeStore::default());
        for id in 1..=4 {
            store.insert(event(tenants[0], id));
        }
        for (tenant_id, event_id) in tenants.iter().skip(1).zip(5..=8) {
            store.insert(event(*tenant_id, event_id));
        }
        store.claim_failures.lock().unwrap().push(tenants[1]);
        let publisher = Arc::new(ScriptedPublisher {
            scripts: Mutex::new(HashMap::from([
                (1, Script::Success),
                (2, Script::Success),
                (6, Script::Success),
                (7, Script::Success),
                (8, Script::Success),
            ])),
        });
        let mut config = test_config(2);
        config.max_in_flight = 2;
        config.tenant_page_size = 2;
        let worker =
            Worker::new(Arc::clone(&store), publisher, "discovery-worker", config).unwrap();

        let error = worker.run_discovered_cycle().await.unwrap_err();

        assert!(error.to_string().contains("tenant 2"));
        assert_eq!(
            store.tenant_pages.lock().unwrap().as_slice(),
            &[(None, 2), (Some(tenants[1]), 2), (Some(tenants[3]), 2),]
        );
        assert_eq!(
            store
                .claims
                .lock()
                .unwrap()
                .iter()
                .map(|(tenant_id, _)| *tenant_id)
                .collect::<Vec<_>>(),
            tenants
        );
        let mut published = store.published.lock().unwrap().clone();
        published.sort_unstable();
        assert_eq!(published, vec![1, 2, 6, 7, 8]);
        assert_eq!(store.events.lock().unwrap()[&tenants[0]].len(), 2);
        assert_eq!(store.events.lock().unwrap()[&tenants[1]].len(), 1);
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
            max_in_flight: usize::try_from(batch_size).unwrap(),
            tenant_page_size: 100,
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
