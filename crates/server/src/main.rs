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
    let app = routes::app(state);
    #[cfg(feature = "ssr")]
    let app = with_web_app(app)?;

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr)
        .await
        .with_context(|| format!("binding {}", cfg.bind_addr))?;
    tracing::info!("listening on http://{}", cfg.bind_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(feature = "ssr")]
fn with_web_app(api: axum::Router) -> anyhow::Result<axum::Router> {
    use axum::Router;
    use leptos::prelude::get_configuration;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use wareboxes_web_ops::app::{shell, App};

    let configuration = get_configuration(None)?;
    let mut leptos_options = configuration.leptos_options;
    if leptos_options.output_name.is_empty() {
        leptos_options.output_name = "wareboxes-web".into();
    }
    let routes = generate_route_list(App);
    let web = Router::new()
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);
    Ok(api.merge(web))
}

/// Mirrors the original `addDevAdmin`: make sure an `admin` permission exists
/// and is attached to the bootstrap user's per-user "self role" so the first
/// administrator can administer the system.
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
    let perm_id =
        repo::permissions::add_permission(pool, tenant_id, "admin", Some("Admin permission"))
            .await?;

    // register_user provisioned the self role; attach admin to it.
    if let Some(self_role) = repo::roles::get_roles(pool, tenant_id, true, true)
        .await?
        .into_iter()
        .find(|r| r.name == *email)
    {
        repo::roles::add_role_permission(pool, tenant_id, self_role.id, perm_id).await?;
    }
    tracing::info!(%email, "bootstrapped admin user");
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
