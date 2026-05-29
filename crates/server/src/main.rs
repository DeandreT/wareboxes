use anyhow::Context;
use tracing_subscriber::EnvFilter;
use wareboxes_server::config::Config;
use wareboxes_server::state::AppState;
use wareboxes_server::{auth, db, repo, routes};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,wareboxes_server=debug")),
        )
        .init();

    let cfg = Config::from_env()?;
    tracing::info!(database = %cfg.database_url, "starting wareboxes-server");

    let pool = db::connect(&cfg.database_url).await?;
    db::run_migrations(&pool)
        .await
        .context("running migrations")?;

    bootstrap_admin(&pool, &cfg).await?;

    let state = AppState::new(pool);
    let app = routes::app(state);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr)
        .await
        .with_context(|| format!("binding {}", cfg.bind_addr))?;
    tracing::info!("listening on http://{}", cfg.bind_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Mirrors the original `addDevAdmin`: make sure an `admin` permission exists
/// and is attached to the bootstrap user's per-user "self role" so the first
/// account can administer the system.
async fn bootstrap_admin(pool: &db::Db, cfg: &Config) -> anyhow::Result<()> {
    let (Some(email), Some(password)) = (&cfg.bootstrap_admin_email, &cfg.bootstrap_admin_password)
    else {
        return Ok(());
    };

    if repo::users::get_user_by_email(pool, email, true)
        .await?
        .is_some()
    {
        return Ok(());
    }

    auth::register_user(pool, email, password, Some("Admin"), None).await?;
    let perm_id =
        repo::permissions::add_permission(pool, "admin", Some("Admin permission")).await?;

    // register_user provisioned the self role; attach admin to it.
    if let Some(self_role) = repo::roles::get_roles(pool, true, true)
        .await?
        .into_iter()
        .find(|r| r.name == *email)
    {
        repo::roles::add_role_permission(pool, self_role.id, perm_id).await?;
    }
    tracing::info!(%email, "bootstrapped admin user");
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
