use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use async_trait::async_trait;
use wareboxes_application::carrier::{
    validate_carrier_response, CarrierManifestAdapterRequest, CarrierManifestAdapterResponse,
    CarrierManifestClaim,
};
use wareboxes_domain::TenantId;

const MAX_ERROR_CHARS: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarrierFailureClass {
    Retryable,
    Permanent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierGatewayError {
    class: CarrierFailureClass,
    code: String,
    message: String,
    retry_after: Option<Duration>,
}

impl CarrierGatewayError {
    pub fn retryable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(CarrierFailureClass::Retryable, code, message)
    }

    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(CarrierFailureClass::Permanent, code, message)
    }

    fn new(
        class: CarrierFailureClass,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            class,
            code: bounded(code.into(), 100),
            message: bounded(message.into(), MAX_ERROR_CHARS),
            retry_after: None,
        }
    }

    pub fn with_retry_after(mut self, retry_after: Duration) -> Self {
        self.retry_after = Some(retry_after);
        self
    }

    pub const fn class(&self) -> CarrierFailureClass {
        self.class
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }
}

#[async_trait]
pub trait CarrierGateway: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    async fn manifest(
        &self,
        request: &CarrierManifestAdapterRequest,
    ) -> Result<CarrierManifestAdapterResponse, CarrierGatewayError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarrierFailureDisposition {
    RetryScheduled,
    Failed,
    LostClaim,
}

#[async_trait]
pub trait CarrierManifestStore: Send + Sync + 'static {
    async fn ready_tenants(
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
    ) -> anyhow::Result<Vec<CarrierManifestClaim>>;

    async fn complete(
        &self,
        claim: &CarrierManifestClaim,
        worker_id: &str,
        response: &CarrierManifestAdapterResponse,
    ) -> anyhow::Result<bool>;

    async fn fail(
        &self,
        claim: &CarrierManifestClaim,
        worker_id: &str,
        error: &CarrierGatewayError,
        retry_after: Duration,
        max_attempts: u32,
    ) -> anyhow::Result<CarrierFailureDisposition>;
}

#[derive(Debug, Clone)]
pub struct CarrierManifestWorkerConfig {
    pub batch_size: i64,
    pub tenant_page_size: usize,
    pub lease: Duration,
    pub request_timeout: Duration,
    pub retry_delay: Duration,
    pub retry_delay_cap: Duration,
    pub max_attempts: u32,
}

impl CarrierManifestWorkerConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !(1..=100).contains(&self.batch_size) {
            bail!("carrier worker batch size must be between 1 and 100");
        }
        if !(1..=10_000).contains(&self.tenant_page_size) {
            bail!("carrier worker tenant page size must be between 1 and 10000");
        }
        if self.lease.is_zero() || self.request_timeout.is_zero() {
            bail!("carrier worker lease and request timeout must be positive");
        }
        if self.request_timeout >= self.lease {
            bail!("carrier request timeout must be shorter than its claim lease");
        }
        if self.retry_delay_cap < self.retry_delay {
            bail!("carrier retry cap must not be shorter than the base delay");
        }
        if self.max_attempts == 0 {
            bail!("carrier worker maximum attempts must be positive");
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CarrierManifestRunSummary {
    pub claimed: u64,
    pub succeeded: u64,
    pub retry_scheduled: u64,
    pub failed: u64,
    pub lost_claims: u64,
}

pub struct CarrierManifestWorker<S, G> {
    store: Arc<S>,
    gateway: Arc<G>,
    worker_id: Arc<str>,
    config: CarrierManifestWorkerConfig,
}

impl<S, G> CarrierManifestWorker<S, G>
where
    S: CarrierManifestStore,
    G: CarrierGateway,
{
    pub fn new(
        store: Arc<S>,
        gateway: Arc<G>,
        worker_id: impl Into<String>,
        config: CarrierManifestWorkerConfig,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        let worker_id = worker_id.into();
        if worker_id.trim() != worker_id || worker_id.is_empty() || worker_id.chars().count() > 200
        {
            bail!("carrier worker ID must contain between 1 and 200 trimmed characters");
        }
        Ok(Self {
            store,
            gateway,
            worker_id: Arc::from(worker_id),
            config,
        })
    }

    pub fn gateway_name(&self) -> &'static str {
        self.gateway.name()
    }

    pub async fn run_discovered_cycle(&self) -> anyhow::Result<CarrierManifestRunSummary> {
        let mut summary = CarrierManifestRunSummary::default();
        let mut after = None;
        loop {
            let tenants = self
                .store
                .ready_tenants(after, self.config.tenant_page_size)
                .await
                .context("discovering tenants with carrier manifest work")?;
            if tenants.is_empty() {
                break;
            }
            validate_tenant_page(after, &tenants)?;
            after = tenants.last().copied();
            for tenant_id in &tenants {
                let claims = self
                    .store
                    .claim(
                        *tenant_id,
                        &self.worker_id,
                        self.config.batch_size,
                        self.config.lease,
                    )
                    .await
                    .with_context(|| format!("claiming carrier jobs for tenant {tenant_id}"))?;
                summary.claimed +=
                    u64::try_from(claims.len()).context("carrier claim count exceeds u64")?;
                for claim in claims {
                    self.execute(&claim, &mut summary).await?;
                }
            }
            if tenants.len() < self.config.tenant_page_size {
                break;
            }
        }
        Ok(summary)
    }

    async fn execute(
        &self,
        claim: &CarrierManifestClaim,
        summary: &mut CarrierManifestRunSummary,
    ) -> anyhow::Result<()> {
        let result = tokio::time::timeout(
            self.config.request_timeout,
            self.gateway.manifest(&claim.request),
        )
        .await;
        match result {
            Ok(Ok(response)) => {
                if let Err(error) = validate_carrier_response(&claim.request, &response) {
                    return self
                        .record_failure(
                            claim,
                            CarrierGatewayError::permanent("invalid_response", error.to_string()),
                            summary,
                        )
                        .await;
                }
                match self.store.complete(claim, &self.worker_id, &response).await {
                    Ok(true) => summary.succeeded += 1,
                    Ok(false) => summary.lost_claims += 1,
                    Err(error) => {
                        self.record_failure(
                            claim,
                            CarrierGatewayError::retryable(
                                "commit_failed",
                                format!("carrier response could not be committed: {error:#}"),
                            ),
                            summary,
                        )
                        .await?;
                    }
                }
                Ok(())
            }
            Ok(Err(error)) => self.record_failure(claim, error, summary).await,
            Err(_) => {
                self.record_failure(
                    claim,
                    CarrierGatewayError::retryable(
                        "request_timeout",
                        "carrier gateway request timed out",
                    ),
                    summary,
                )
                .await
            }
        }
    }

    async fn record_failure(
        &self,
        claim: &CarrierManifestClaim,
        error: CarrierGatewayError,
        summary: &mut CarrierManifestRunSummary,
    ) -> anyhow::Result<()> {
        let retry_after = error
            .retry_after()
            .unwrap_or_else(|| retry_delay(&self.config, claim.job.attempt_count));
        match self
            .store
            .fail(
                claim,
                &self.worker_id,
                &error,
                retry_after.min(self.config.retry_delay_cap),
                self.config.max_attempts,
            )
            .await
            .context("recording carrier manifest failure")?
        {
            CarrierFailureDisposition::RetryScheduled => summary.retry_scheduled += 1,
            CarrierFailureDisposition::Failed => summary.failed += 1,
            CarrierFailureDisposition::LostClaim => summary.lost_claims += 1,
        }
        Ok(())
    }
}

fn retry_delay(config: &CarrierManifestWorkerConfig, attempt_count: u32) -> Duration {
    let exponent = attempt_count.saturating_sub(1).min(20);
    config
        .retry_delay
        .checked_mul(1_u32 << exponent)
        .unwrap_or(config.retry_delay_cap)
        .min(config.retry_delay_cap)
}

fn validate_tenant_page(after: Option<TenantId>, tenants: &[TenantId]) -> anyhow::Result<()> {
    let mut previous = after.map_or(0, TenantId::get);
    for tenant in tenants {
        if tenant.get() <= previous {
            bail!("carrier tenant page must be strictly increasing");
        }
        previous = tenant.get();
    }
    Ok(())
}

fn bounded(value: String, max: usize) -> String {
    let value = value.trim();
    let mut result = value
        .chars()
        .filter(|character| !character.is_control())
        .take(max)
        .collect::<String>();
    if result.is_empty() {
        result.push_str("carrier_error");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_is_exponential_and_capped() {
        let config = CarrierManifestWorkerConfig {
            batch_size: 1,
            tenant_page_size: 1,
            lease: Duration::from_secs(30),
            request_timeout: Duration::from_secs(10),
            retry_delay: Duration::from_secs(2),
            retry_delay_cap: Duration::from_secs(10),
            max_attempts: 3,
        };
        assert_eq!(retry_delay(&config, 1), Duration::from_secs(2));
        assert_eq!(retry_delay(&config, 3), Duration::from_secs(8));
        assert_eq!(retry_delay(&config, 10), Duration::from_secs(10));
    }

    #[test]
    fn configuration_rejects_timeouts_longer_than_the_lease() {
        let config = CarrierManifestWorkerConfig {
            batch_size: 1,
            tenant_page_size: 1,
            lease: Duration::from_secs(5),
            request_timeout: Duration::from_secs(5),
            retry_delay: Duration::from_secs(1),
            retry_delay_cap: Duration::from_secs(2),
            max_attempts: 1,
        };
        assert!(config.validate().is_err());
    }
}
