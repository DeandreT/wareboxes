#[path = "seed_demo/billing.rs"]
mod billing;
#[path = "seed_demo/configuration.rs"]
mod configuration;
#[path = "seed_demo/cross_dock.rs"]
mod cross_dock;
#[path = "seed_demo/fulfillment.rs"]
mod fulfillment;
#[path = "seed_demo/operations.rs"]
mod operations;
#[path = "seed_demo/support.rs"]
mod support;
#[path = "seed_demo/yard.rs"]
mod yard;

use anyhow::Context;
use support::SeedContext;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,wareboxes_api=debug")),
        )
        .try_init()
        .ok();
    let context = SeedContext::connect().await?;

    fulfillment::seed(&context).await?;
    operations::seed(&context).await?;
    cross_dock::seed(&context).await?;
    configuration::seed(&context).await?;
    billing::seed(&context).await?;
    yard::seed(&context).await?;
    context.verify().await?;

    println!(
        "Typed demo workflows are ready for tenant {}.",
        context.tenant_id
    );
    println!("Seed actor: {}", context.email);
    context
        .close()
        .await
        .context("closing demo seed database pools")?;
    Ok(())
}
