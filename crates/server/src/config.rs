use std::env;

use anyhow::{bail, Context};
use axum::http::HeaderValue;

#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub allow_public_registration: bool,
    pub cors_allowed_origins: Vec<HeaderValue>,
    pub max_request_body_bytes: usize,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allow_public_registration: false,
            cors_allowed_origins: Vec::new(),
            max_request_body_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// `postgres://...` / `postgresql://...`.
    pub database_url: String,
    pub bind_addr: String,
    /// Optional bootstrap admin created on first startup if the users table is empty.
    pub bootstrap_admin_email: Option<String>,
    pub bootstrap_admin_password: Option<String>,
    pub security: SecurityConfig,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://wareboxes:wareboxes@127.0.0.1:5433/wareboxes".to_string()
        });
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let allow_public_registration = parse_bool_env("ALLOW_PUBLIC_REGISTRATION", false)?;
        let cors_allowed_origins = env::var("CORS_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(|origin| {
                HeaderValue::from_str(origin)
                    .with_context(|| format!("invalid CORS_ALLOWED_ORIGINS entry: {origin}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let max_request_body_bytes = env::var("MAX_REQUEST_BODY_BYTES")
            .unwrap_or_else(|_| (1024 * 1024).to_string())
            .parse::<usize>()
            .context("MAX_REQUEST_BODY_BYTES must be a positive integer")?;
        if max_request_body_bytes == 0 {
            bail!("MAX_REQUEST_BODY_BYTES must be greater than zero");
        }

        Ok(Self {
            database_url,
            bind_addr,
            bootstrap_admin_email: env::var("BOOTSTRAP_ADMIN_EMAIL").ok(),
            bootstrap_admin_password: env::var("BOOTSTRAP_ADMIN_PASSWORD").ok(),
            security: SecurityConfig {
                allow_public_registration,
                cors_allowed_origins,
                max_request_body_bytes,
            },
        })
    }
}

fn parse_bool_env(name: &str, default: bool) -> anyhow::Result<bool> {
    let Ok(value) = env::var(name) else {
        return Ok(default);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => bail!("{name} must be true or false"),
    }
}
