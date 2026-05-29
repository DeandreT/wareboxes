//! Database access layer.
//!
//! We currently target PostgreSQL in production and test.
//!
//! The repository layer uses PostgreSQL connections and row types directly.

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use wareboxes_core::models::Timestamp;

pub type Db = PgPool;

pub fn now_iso() -> Timestamp {
    chrono::Utc::now()
}

static PG_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

pub async fn connect(database_url: &str) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .with_context(|| format!("connecting to database at {database_url}"))?;
    Ok(pool)
}

pub async fn run_migrations(pool: &Db) -> anyhow::Result<()> {
    PG_MIGRATIONS.run(pool).await?;
    Ok(())
}
