use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    /// `postgres://...` / `postgresql://...`.
    pub database_url: String,
    pub bind_addr: String,
    /// Optional bootstrap admin created on first startup if the users table is empty.
    pub bootstrap_admin_email: Option<String>,
    pub bootstrap_admin_password: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://wareboxes:wareboxes@127.0.0.1:5433/wareboxes".to_string()
        });
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        Ok(Self {
            database_url,
            bind_addr,
            bootstrap_admin_email: env::var("BOOTSTRAP_ADMIN_EMAIL").ok(),
            bootstrap_admin_password: env::var("BOOTSTRAP_ADMIN_PASSWORD").ok(),
        })
    }
}
