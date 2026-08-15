use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use serde_json::json;
use tokio::task::JoinSet;

const TENANT_HEADER: &str = "x-wareboxes-tenant-id";
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
    active_tenant: ActiveTenant,
}

#[derive(Deserialize)]
struct ActiveTenant {
    tenant_id: i64,
}

#[derive(Deserialize)]
struct InventoryOwner {
    id: i64,
}

#[derive(Deserialize)]
struct Item {
    id: i64,
    packaging_unit: String,
}

#[derive(Clone)]
struct RequestContext {
    client: Client,
    base_url: Url,
    token: Arc<str>,
    tenant_id: i64,
    run_id: Arc<str>,
}

#[derive(Clone)]
enum Phase {
    Read,
    Command {
        payloads: Arc<Vec<Vec<u8>>>,
        expected_bodies: Option<Arc<Vec<Vec<u8>>>>,
    },
}

struct PhaseOutcome {
    elapsed: Duration,
    durations: Vec<Duration>,
    bodies: Vec<Vec<u8>>,
    error_count: usize,
    errors: Vec<String>,
}

#[derive(Clone, Copy)]
struct Budget {
    p95: Duration,
    p99: Duration,
    minimum_requests_per_second: f64,
    maximum_error_basis_points: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let client = Client::builder()
        .pool_max_idle_per_host(config.concurrency)
        .tcp_nodelay(true)
        .timeout(Duration::from_secs(5))
        .build()
        .context("building load-test HTTP client")?;
    let session = login(&client, &config).await?;
    let run_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time predates Unix epoch")?
            .as_millis()
    );
    let context = RequestContext {
        client,
        base_url: config.base_url,
        token: Arc::from(session.token),
        tenant_id: session.active_tenant.tenant_id,
        run_id: Arc::from(run_id),
    };

    verify_readiness(&context).await?;
    configure_integration_mapping(&context).await?;

    if config.warmup_requests > 0 {
        let warmup = run_phase(
            context.clone(),
            Phase::Read,
            config.warmup_requests,
            config.concurrency,
        )
        .await?;
        if !warmup.errors.is_empty() {
            bail!("load-envelope warmup failed: {}", warmup.errors.join("; "));
        }
    }

    let read = run_phase(
        context.clone(),
        Phase::Read,
        config.read_requests,
        config.concurrency,
    )
    .await?;
    enforce("scoped_reads", &read, config.read_budget)?;

    let payloads = Arc::new(
        (0..config.command_requests)
            .map(|index| command_payload(&context.run_id, index))
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    let commands = run_phase(
        context.clone(),
        Phase::Command {
            payloads: payloads.clone(),
            expected_bodies: None,
        },
        config.command_requests,
        config.command_concurrency,
    )
    .await?;
    enforce("durable_commands", &commands, config.command_budget)?;

    let replay = run_phase(
        context,
        Phase::Command {
            payloads,
            expected_bodies: Some(Arc::new(commands.bodies)),
        },
        config.command_requests,
        config.command_concurrency,
    )
    .await?;
    enforce("exact_replays", &replay, config.replay_budget)?;

    println!(
        "event=load_envelope_passed read_requests={} command_requests={} read_concurrency={} command_concurrency={}",
        config.read_requests,
        config.command_requests,
        config.concurrency,
        config.command_concurrency
    );
    Ok(())
}

struct Config {
    base_url: Url,
    email: String,
    password: String,
    read_requests: usize,
    command_requests: usize,
    warmup_requests: usize,
    concurrency: usize,
    command_concurrency: usize,
    read_budget: Budget,
    command_budget: Budget,
    replay_budget: Budget,
}

impl Config {
    fn from_env() -> anyhow::Result<Self> {
        let base_url = env::var("LOAD_BASE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:18084".into())
            .parse::<Url>()
            .context("LOAD_BASE_URL must be a valid URL")?;
        if !matches!(base_url.scheme(), "http" | "https") {
            bail!("LOAD_BASE_URL must use HTTP or HTTPS");
        }
        let email = required_env("LOAD_USER_EMAIL")?;
        let password = required_env("LOAD_USER_PASSWORD")?;
        let read_requests = integer_env("LOAD_READ_REQUESTS", 400, 1, 100_000)?;
        let command_requests = integer_env("LOAD_COMMAND_REQUESTS", 100, 1, 10_000)?;
        let warmup_requests = integer_env("LOAD_WARMUP_REQUESTS", 20, 0, 10_000)?;
        let concurrency = integer_env("LOAD_READ_CONCURRENCY", 16, 1, 1_000)?;
        let command_concurrency = integer_env("LOAD_COMMAND_CONCURRENCY", 8, 1, 1_000)?;
        if concurrency > read_requests || command_concurrency > command_requests {
            bail!("load concurrency must not exceed its phase request count");
        }
        Ok(Self {
            base_url,
            email,
            password,
            read_requests,
            command_requests,
            warmup_requests,
            concurrency,
            command_concurrency,
            read_budget: Budget {
                p95: millis_env("LOAD_READ_P95_MILLIS", 250)?,
                p99: millis_env("LOAD_READ_P99_MILLIS", 750)?,
                minimum_requests_per_second: integer_env("LOAD_READ_MIN_RPS", 50, 1, 100_000)?
                    as f64,
                maximum_error_basis_points: integer_env(
                    "LOAD_MAX_ERROR_BASIS_POINTS",
                    0,
                    0,
                    10_000,
                )?,
            },
            command_budget: Budget {
                p95: millis_env("LOAD_COMMAND_P95_MILLIS", 1_000)?,
                p99: millis_env("LOAD_COMMAND_P99_MILLIS", 2_000)?,
                minimum_requests_per_second: integer_env("LOAD_COMMAND_MIN_RPS", 10, 1, 100_000)?
                    as f64,
                maximum_error_basis_points: integer_env(
                    "LOAD_MAX_ERROR_BASIS_POINTS",
                    0,
                    0,
                    10_000,
                )?,
            },
            replay_budget: Budget {
                p95: millis_env("LOAD_REPLAY_P95_MILLIS", 500)?,
                p99: millis_env("LOAD_REPLAY_P99_MILLIS", 1_000)?,
                minimum_requests_per_second: integer_env("LOAD_REPLAY_MIN_RPS", 20, 1, 100_000)?
                    as f64,
                maximum_error_basis_points: integer_env(
                    "LOAD_MAX_ERROR_BASIS_POINTS",
                    0,
                    0,
                    10_000,
                )?,
            },
        })
    }
}

fn required_env(name: &str) -> anyhow::Result<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => bail!("{name} is required and must not be empty"),
    }
}

fn integer_env(
    name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> anyhow::Result<usize> {
    let value = env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("{name} must be an integer"))
        })
        .transpose()?
        .unwrap_or(default);
    if !(minimum..=maximum).contains(&value) {
        bail!("{name} must be between {minimum} and {maximum}");
    }
    Ok(value)
}

fn millis_env(name: &str, default: usize) -> anyhow::Result<Duration> {
    let millis = integer_env(name, default, 1, 60_000)?;
    Ok(Duration::from_millis(
        u64::try_from(millis).context("millisecond budget does not fit in u64")?,
    ))
}

async fn login(client: &Client, config: &Config) -> anyhow::Result<LoginResponse> {
    let response = client
        .post(config.base_url.join("/api/auth/login")?)
        .json(&json!({"email": config.email, "password": config.password}))
        .send()
        .await
        .context("sending load-test login")?
        .error_for_status()
        .context("load-test login was rejected")?;
    response
        .json()
        .await
        .context("decoding load-test login response")
}

async fn verify_readiness(context: &RequestContext) -> anyhow::Result<()> {
    context
        .client
        .get(context.base_url.join("/health/ready")?)
        .send()
        .await
        .context("checking load target readiness")?
        .error_for_status()
        .context("load target is not ready")?;
    Ok(())
}

async fn configure_integration_mapping(context: &RequestContext) -> anyhow::Result<()> {
    let owners = context
        .client
        .get(context.base_url.join("/api/inventory-owners")?)
        .bearer_auth(context.token.as_ref())
        .header(TENANT_HEADER, context.tenant_id)
        .send()
        .await?
        .error_for_status()
        .context("listing load-test inventory owners")?
        .json::<Vec<InventoryOwner>>()
        .await?;
    let owner = owners
        .first()
        .context("load target has no inventory owner")?;
    let items = context
        .client
        .get(context.base_url.join("/api/items")?)
        .bearer_auth(context.token.as_ref())
        .header(TENANT_HEADER, context.tenant_id)
        .send()
        .await?
        .error_for_status()
        .context("listing load-test items")?
        .json::<Vec<Item>>()
        .await?;
    let item = items.first().context("load target has no item")?;

    let owner_mapping = json!({
        "source_key": "load-envelope",
        "external_inventory_owner_key": "PRIMARY",
        "inventory_owner_id": owner.id,
        "expected_revision": null
    });
    context
        .client
        .post(
            context
                .base_url
                .join("/api/v1/integration-order-owner-mappings")?,
        )
        .bearer_auth(context.token.as_ref())
        .header(TENANT_HEADER, context.tenant_id)
        .header(IDEMPOTENCY_HEADER, format!("load-owner-{}", context.run_id))
        .json(&owner_mapping)
        .send()
        .await?
        .error_for_status()
        .context("configuring load-test owner mapping")?;

    let item_mapping = json!({
        "inventory_owner_id": owner.id,
        "source_key": "load-envelope",
        "external_item_key": "PRIMARY-ITEM",
        "external_uom": "EA",
        "item_id": item.id,
        "requested_uom": item.packaging_unit,
        "expected_revision": null
    });
    context
        .client
        .post(
            context
                .base_url
                .join("/api/v1/integration-order-item-mappings")?,
        )
        .bearer_auth(context.token.as_ref())
        .header(TENANT_HEADER, context.tenant_id)
        .header(IDEMPOTENCY_HEADER, format!("load-item-{}", context.run_id))
        .json(&item_mapping)
        .send()
        .await?
        .error_for_status()
        .context("configuring load-test item mapping")?;
    Ok(())
}

fn command_payload(run_id: &str, index: usize) -> anyhow::Result<Vec<u8>> {
    serde_json::to_vec(&json!({
        "order_key": format!("LOAD-{run_id}-{index:06}"),
        "rush": index.is_multiple_of(10),
        "ship_by": null,
        "destination": {
            "recipient_name": "Load Envelope Receiver",
            "company": "Wareboxes Test",
            "phone": null,
            "email": null,
            "line1": "100 Benchmark Way",
            "line2": null,
            "city": "Portland",
            "region": "OR",
            "postal_code": "97205",
            "country": "US"
        },
        "lines": [{
            "line_key": "1",
            "external_item_key": "PRIMARY-ITEM",
            "external_uom": "EA",
            "quantity": 1
        }]
    }))
    .context("encoding load-test order")
}

async fn run_phase(
    context: RequestContext,
    phase: Phase,
    requests: usize,
    concurrency: usize,
) -> anyhow::Result<PhaseOutcome> {
    let next_index = Arc::new(AtomicUsize::new(0));
    let started_at = Instant::now();
    let mut workers = JoinSet::new();
    for _ in 0..concurrency {
        let context = context.clone();
        let phase = phase.clone();
        let next_index = next_index.clone();
        workers.spawn(async move {
            let mut outcomes = Vec::new();
            loop {
                let index = next_index.fetch_add(1, Ordering::Relaxed);
                if index >= requests {
                    break;
                }
                let request_started_at = Instant::now();
                let result = execute_request(&context, &phase, index).await;
                outcomes.push((index, request_started_at.elapsed(), result));
            }
            outcomes
        });
    }

    let mut durations = Vec::with_capacity(requests);
    let mut bodies = vec![Vec::new(); requests];
    let mut error_count = 0;
    let mut errors = Vec::new();
    while let Some(worker) = workers.join_next().await {
        for (index, duration, result) in worker.context("load worker panicked")? {
            durations.push(duration);
            match result {
                Ok(body) => bodies[index] = body,
                Err(error) => {
                    error_count += 1;
                    if errors.len() < 20 {
                        errors.push(format!("request {index}: {error}"));
                    }
                }
            }
        }
    }
    Ok(PhaseOutcome {
        elapsed: started_at.elapsed(),
        durations,
        bodies,
        error_count,
        errors,
    })
}

async fn execute_request(
    context: &RequestContext,
    phase: &Phase,
    index: usize,
) -> anyhow::Result<Vec<u8>> {
    let request_id = format!("load-{}-{index}", context.run_id);
    let response = match phase {
        Phase::Read => {
            let path = if index.is_multiple_of(2) {
                "/api/v1/inventory/balances?limit=50"
            } else {
                "/api/orders?limit=50"
            };
            context
                .client
                .get(context.base_url.join(path)?)
                .bearer_auth(context.token.as_ref())
                .header(TENANT_HEADER, context.tenant_id)
                .header(REQUEST_ID_HEADER, request_id)
                .send()
                .await?
        }
        Phase::Command { payloads, .. } => context
            .client
            .post(context.base_url.join(
                "/api/v1/integrations/order-intake/load-envelope/inventory-owners/PRIMARY/orders",
            )?)
            .bearer_auth(context.token.as_ref())
            .header(TENANT_HEADER, context.tenant_id)
            .header(
                IDEMPOTENCY_HEADER,
                format!("load-command-{}-{index}", context.run_id),
            )
            .header(REQUEST_ID_HEADER, request_id)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payloads[index].clone())
            .send()
            .await?,
    };
    let status = response.status();
    let body = response.bytes().await?.to_vec();
    let expected_status = match phase {
        Phase::Read => StatusCode::OK,
        Phase::Command { .. } => StatusCode::ACCEPTED,
    };
    if status != expected_status {
        bail!("expected {expected_status}, received {status}");
    }
    if let Phase::Command {
        expected_bodies: Some(expected),
        ..
    } = phase
    {
        if body != expected[index] {
            bail!("replay response differed from the original command result");
        }
    }
    Ok(body)
}

fn enforce(name: &str, outcome: &PhaseOutcome, budget: Budget) -> anyhow::Result<()> {
    let mut durations = outcome.durations.clone();
    durations.sort_unstable();
    let p50 = percentile(&durations, 50);
    let p95 = percentile(&durations, 95);
    let p99 = percentile(&durations, 99);
    let requests_per_second = durations.len() as f64 / outcome.elapsed.as_secs_f64();
    let errors = outcome.error_count;
    println!(
        "event=load_phase_completed phase={name} requests={} errors={errors} concurrency_duration_seconds={:.3} requests_per_second={requests_per_second:.1} p50_millis={:.1} p95_millis={:.1} p99_millis={:.1}",
        durations.len(),
        outcome.elapsed.as_secs_f64(),
        p50.as_secs_f64() * 1_000.0,
        p95.as_secs_f64() * 1_000.0,
        p99.as_secs_f64() * 1_000.0,
    );
    if errors * 10_000 > durations.len() * budget.maximum_error_basis_points {
        bail!(
            "{name} exceeded its error budget: {errors}/{} failed: {}",
            durations.len(),
            outcome.errors.join("; ")
        );
    }
    if p95 > budget.p95 {
        bail!(
            "{name} p95 {:.1}ms exceeded {:.1}ms",
            p95.as_secs_f64() * 1_000.0,
            budget.p95.as_secs_f64() * 1_000.0
        );
    }
    if p99 > budget.p99 {
        bail!(
            "{name} p99 {:.1}ms exceeded {:.1}ms",
            p99.as_secs_f64() * 1_000.0,
            budget.p99.as_secs_f64() * 1_000.0
        );
    }
    if requests_per_second < budget.minimum_requests_per_second {
        bail!(
            "{name} throughput {requests_per_second:.1} rps was below {:.1} rps",
            budget.minimum_requests_per_second
        );
    }
    Ok(())
}

fn percentile(sorted: &[Duration], percentile: usize) -> Duration {
    let rank = sorted.len().saturating_mul(percentile).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}
