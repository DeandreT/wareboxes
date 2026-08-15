mod config;

use anyhow::Context;
use tracing_subscriber::EnvFilter;
use wareboxes_api::state::AppState;
use wareboxes_api::{auth, db, routes};

use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing()?;

    let cfg = Config::from_env()?;
    tracing::info!(bind_address = %cfg.bind_addr, "starting wareboxes-server");

    let migration_pool = db::connect(&cfg.migration_database_url).await?;
    let preflight_pool = db::connect(&cfg.database_url).await?;
    db::validate_same_database(&migration_pool, &preflight_pool)
        .await
        .context("validating migration and runtime database targets")?;
    preflight_pool.close().await;
    db::run_migrations(&migration_pool)
        .await
        .context("running migrations")?;

    let pool = db::connect_runtime(&cfg.database_url).await?;
    db::validate_same_database(&migration_pool, &pool)
        .await
        .context("validating migrated runtime database target")?;
    bootstrap_admin(&migration_pool, &cfg).await?;
    migration_pool.close().await;

    let state = AppState::with_security(pool, cfg.security.clone());
    let app = routes::app(state.clone());
    #[cfg(feature = "ssr")]
    let app = wareboxes_api::web_app::with_web_app(app, state)?;

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr)
        .await
        .with_context(|| format!("binding {}", cfg.bind_addr))?;
    tracing::info!("listening on http://{}", cfg.bind_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn init_tracing() -> anyhow::Result<()> {
    let format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "compact".into());
    let filter = || {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("info,wareboxes_api=debug,wareboxes_server=debug,sqlx::query=error")
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

/// Mirrors the original `addDevAdmin`: make sure an `admin` permission exists
/// and is attached to the bootstrap user's per-user "self role" so the first
/// administrator can administer the system.
async fn bootstrap_admin(pool: &db::Db, cfg: &Config) -> anyhow::Result<()> {
    let (Some(email), Some(password)) = (&cfg.bootstrap_admin_email, &cfg.bootstrap_admin_password)
    else {
        return Ok(());
    };

    if wareboxes_persistence_postgres::users::find_user_by_email(pool, email, true)
        .await?
        .is_some()
    {
        return Ok(());
    }

    let user = auth::register_user(pool, email, password, Some("Admin"), None).await?;
    let token = auth::create_session(pool, user.id).await?;
    let tenant_result = match auth::default_tenant_for_session(pool, &token).await {
        Ok(Some(tenant)) => Ok(tenant),
        Ok(None) => Err(anyhow::anyhow!("bootstrap admin has no tenant")),
        Err(error) => Err(error.into()),
    };
    let cleanup_result = auth::destroy_session(pool, &token).await;
    let tenant_id = match (tenant_result, cleanup_result) {
        (Ok(tenant), Ok(())) => tenant.tenant_id,
        (Err(error), _) => return Err(error),
        (Ok(_), Err(error)) => return Err(error.into()),
    };
    let perm_id = wareboxes_persistence_postgres::permissions::add_permission(
        pool,
        tenant_id,
        "admin",
        Some("Admin permission"),
    )
    .await?;

    // register_user provisioned the self role; attach admin to it.
    if let Some(self_role) =
        wareboxes_persistence_postgres::roles::get_roles(pool, tenant_id, true, true)
            .await?
            .into_iter()
            .find(|r| r.name == *email)
    {
        wareboxes_persistence_postgres::roles::add_role_permission(
            pool,
            tenant_id,
            self_role.id,
            perm_id,
        )
        .await?;
    }
    tracing::info!(%email, "bootstrapped admin user");
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not install SIGTERM handler");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
