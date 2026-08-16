mod config;
mod publisher;
mod reconciliation_store;
mod store;

use std::sync::Arc;

use anyhow::Context;
use chrono::{Timelike, Utc};
use config::{Config, PublisherConfig};
use publisher::ConfiguredPublisher;
use reconciliation_store::PostgresInventoryReconciliationStore;
use store::PostgresOutboxStore;
use tokio::time::MissedTickBehavior;
use tracing_subscriber::EnvFilter;
use wareboxes_persistence_postgres::db;
use wareboxes_worker::{InventoryReconciliationWorker, Worker};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing()?;

    let config = Config::from_env()?;
    let publisher = match config.publisher {
        PublisherConfig::Http {
            endpoint,
            bearer_token,
            signing_secret,
        } => ConfiguredPublisher::http(endpoint, bearer_token, signing_secret)?,
        PublisherConfig::Sftp {
            host,
            port,
            username,
            private_key_file,
            known_hosts_file,
            remote_directory,
        } => ConfiguredPublisher::sftp(
            host,
            port,
            username,
            private_key_file,
            known_hosts_file,
            remote_directory,
        )?,
        PublisherConfig::Stdout => {
            tracing::warn!("stdout outbox publisher is enabled; delivered events will be consumed");
            ConfiguredPublisher::stdout()
        }
    };
    let pool = db::connect_runtime(&config.database_url)
        .await
        .context("connecting the outbox worker to PostgreSQL")?;
    let worker = Worker::new(
        Arc::new(PostgresOutboxStore::new(pool.clone())),
        Arc::new(publisher),
        config.worker_id.clone(),
        config.worker,
    )?;
    let reconciliation_worker = InventoryReconciliationWorker::new(
        Arc::new(PostgresInventoryReconciliationStore::new(pool.clone())),
        config.worker_id,
        config.reconciliation,
    )?;
    let mut interval = tokio::time::interval(config.poll_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut reconciliation_interval = tokio::time::interval(config.reconciliation_poll_interval);
    reconciliation_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    tracing::info!(
        publisher = worker.publisher_name(),
        "starting outbox worker"
    );
    loop {
        tokio::select! {
            result = &mut shutdown => {
                result.context("installing outbox worker shutdown signal")?;
                break;
            }
            _ = interval.tick() => {
                match worker.run_discovered_cycle().await {
                    Ok(summary) if summary.claimed > 0 => tracing::info!(
                        claimed = summary.claimed,
                        published = summary.published,
                        retryable_failures = summary.retryable_failures,
                        permanent_failures = summary.permanent_failures,
                        lost_claims = summary.lost_claims,
                        "completed outbox delivery cycle"
                    ),
                    Ok(_) => {}
                    Err(error) => tracing::error!(%error, "outbox delivery cycle failed"),
                }
            }
            _ = reconciliation_interval.tick() => {
                let now = Utc::now();
                let Some(scheduled_for) = now.with_second(0).and_then(|value| value.with_nanosecond(0)) else {
                    tracing::error!("could not align inventory reconciliation schedule to a minute");
                    continue;
                };
                match reconciliation_worker.run_discovered_cycle(scheduled_for).await {
                    Ok(summary) => {
                        for failure in &summary.failures {
                            tracing::error!(
                                tenant_id = failure.tenant_id.get(),
                                error = %failure.error,
                                "inventory reconciliation failed for tenant"
                            );
                        }
                        tracing::info!(
                            attempted = summary.attempted,
                            completed = summary.completed,
                            replayed = summary.replayed,
                            alerts = summary.alerts,
                            failures = summary.failures.len(),
                            "completed inventory reconciliation cycle"
                        );
                    }
                    Err(error) => tracing::error!(%error, "inventory reconciliation cycle failed"),
                }
            }
        }
    }

    pool.close().await;
    tracing::info!("outbox worker stopped");
    Ok(())
}

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

fn init_tracing() -> anyhow::Result<()> {
    let format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "compact".into());
    let filter = || {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("info,wareboxes_worker=debug,wareboxes_worker_process=debug")
        })
    };
    match format.trim().to_ascii_lowercase().as_str() {
        "json" => tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_env_filter(filter())
            .try_init()
            .map_err(|error| anyhow::anyhow!("initializing tracing: {error}"))?,
        "compact" => tracing_subscriber::fmt()
            .compact()
            .with_env_filter(filter())
            .try_init()
            .map_err(|error| anyhow::anyhow!("initializing tracing: {error}"))?,
        _ => anyhow::bail!("LOG_FORMAT must be json or compact"),
    }
    Ok(())
}
