mod config;
mod publisher;
mod store;

use std::sync::Arc;

use anyhow::Context;
use config::{Config, PublisherConfig};
use publisher::ConfiguredPublisher;
use store::PostgresOutboxStore;
use tokio::time::MissedTickBehavior;
use tracing_subscriber::EnvFilter;
use wareboxes_persistence_postgres::db;
use wareboxes_worker::Worker;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("info,wareboxes_worker=debug,wareboxes_worker_process=debug")
        }))
        .init();

    let config = Config::from_env()?;
    let publisher = match config.publisher {
        PublisherConfig::Http {
            endpoint,
            bearer_token,
        } => ConfiguredPublisher::http(endpoint, bearer_token)?,
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
        config.worker_id,
        config.worker,
    )?;
    let mut interval = tokio::time::interval(config.poll_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let shutdown = tokio::signal::ctrl_c();
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
        }
    }

    pool.close().await;
    tracing::info!("outbox worker stopped");
    Ok(())
}
