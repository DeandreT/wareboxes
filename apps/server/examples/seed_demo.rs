#[path = "seed_demo/fulfillment.rs"]
mod fulfillment;
#[path = "seed_demo/operations.rs"]
mod operations;
#[path = "seed_demo/support.rs"]
mod support;

use anyhow::Context;
use support::SeedContext;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let context = SeedContext::connect().await?;

    fulfillment::seed(&context).await?;
    operations::seed(&context).await?;
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
