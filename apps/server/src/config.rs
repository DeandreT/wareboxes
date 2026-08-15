use std::env;
use std::time::Duration;

use anyhow::{bail, Context};
use axum::http::HeaderValue;
use wareboxes_api::config::SecurityConfig;

#[derive(Debug, Clone)]
pub struct Config {
    /// Restricted application-role connection used for runtime requests.
    pub database_url: String,
    /// Schema-owner connection used only for migrations and bootstrap provisioning.
    pub migration_database_url: String,
    pub bind_addr: String,
    /// Optional bootstrap admin created on first startup if the users table is empty.
    pub bootstrap_admin_email: Option<String>,
    pub bootstrap_admin_password: Option<String>,
    pub security: SecurityConfig,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let (database_url, migration_database_url) = resolve_database_urls(
            optional_env("DATABASE_URL")?,
            optional_env("MIGRATION_DATABASE_URL")?,
        )?;
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
        let web_session_absolute_ttl_seconds = parse_i32_env(
            "WEB_SESSION_ABSOLUTE_TTL_SECONDS",
            12 * 60 * 60,
            300,
            86_400,
        )?;
        let web_session_idle_ttl_seconds =
            parse_i32_env("WEB_SESSION_IDLE_TTL_SECONDS", 30 * 60, 60, 86_400)?;
        if web_session_idle_ttl_seconds > web_session_absolute_ttl_seconds {
            bail!("WEB_SESSION_IDLE_TTL_SECONDS must not exceed the absolute session TTL");
        }
        let secure_web_session_cookie = parse_bool_env("SECURE_WEB_SESSION_COOKIE", false)?;
        let max_in_flight_requests = parse_usize_env("MAX_IN_FLIGHT_REQUESTS", 256, 1, 10_000)?;
        let request_rate_limit_per_second =
            parse_usize_env("REQUEST_RATE_LIMIT_PER_SECOND", 1_000, 1, 100_000)?;
        let login_rate_limit_per_minute =
            parse_usize_env("LOGIN_RATE_LIMIT_PER_MINUTE", 60, 1, 10_000)?;
        let request_timeout_seconds = parse_i32_env("REQUEST_TIMEOUT_SECONDS", 30, 1, 300)?;

        Ok(Self {
            database_url,
            migration_database_url,
            bind_addr,
            bootstrap_admin_email: env::var("BOOTSTRAP_ADMIN_EMAIL").ok(),
            bootstrap_admin_password: env::var("BOOTSTRAP_ADMIN_PASSWORD").ok(),
            security: SecurityConfig {
                allow_public_registration,
                cors_allowed_origins,
                max_request_body_bytes,
                web_session_absolute_ttl_seconds,
                web_session_idle_ttl_seconds,
                secure_web_session_cookie,
                max_in_flight_requests,
                request_rate_limit_per_second,
                login_rate_limit_per_minute,
                request_timeout: Duration::from_secs(
                    u64::try_from(request_timeout_seconds)
                        .context("REQUEST_TIMEOUT_SECONDS cannot be negative")?,
                ),
            },
        })
    }
}

fn optional_env(name: &str) -> anyhow::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => bail!("{name} must contain valid UTF-8"),
    }
}

fn resolve_database_urls(
    database_url: Option<String>,
    migration_database_url: Option<String>,
) -> anyhow::Result<(String, String)> {
    let database_url = non_empty_database_url("DATABASE_URL", database_url)?;
    let migration_database_url =
        non_empty_database_url("MIGRATION_DATABASE_URL", migration_database_url)?;

    match (database_url, migration_database_url) {
        (None, None) => Ok((
            "postgres://wareboxes_app:wareboxes_app@127.0.0.1:5433/wareboxes".to_string(),
            "postgres://wareboxes_admin:wareboxes_admin@127.0.0.1:5433/wareboxes".to_string(),
        )),
        (Some(database_url), Some(migration_database_url)) => {
            Ok((database_url, migration_database_url))
        }
        (Some(_), None) => {
            bail!("MIGRATION_DATABASE_URL is required when DATABASE_URL is configured")
        }
        (None, Some(_)) => {
            bail!("DATABASE_URL is required when MIGRATION_DATABASE_URL is configured")
        }
    }
}

fn non_empty_database_url(name: &str, value: Option<String>) -> anyhow::Result<Option<String>> {
    match value {
        Some(value) if value.trim().is_empty() => bail!("{name} must not be empty"),
        value => Ok(value),
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

fn parse_i32_env(name: &str, default: i32, min: i32, max: i32) -> anyhow::Result<i32> {
    let value = match env::var(name) {
        Ok(value) => value
            .parse::<i32>()
            .with_context(|| format!("{name} must be an integer"))?,
        Err(env::VarError::NotPresent) => default,
        Err(env::VarError::NotUnicode(_)) => bail!("{name} must contain valid UTF-8"),
    };
    validate_i32(name, value, min, max)
}

fn parse_usize_env(name: &str, default: usize, min: usize, max: usize) -> anyhow::Result<usize> {
    let value = match env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .with_context(|| format!("{name} must be an integer"))?,
        Err(env::VarError::NotPresent) => default,
        Err(env::VarError::NotUnicode(_)) => bail!("{name} must contain valid UTF-8"),
    };
    validate_usize(name, value, min, max)
}

fn validate_i32(name: &str, value: i32, min: i32, max: i32) -> anyhow::Result<i32> {
    if !(min..=max).contains(&value) {
        bail!("{name} must be between {min} and {max}");
    }
    Ok(value)
}

fn validate_usize(name: &str, value: usize, min: usize, max: usize) -> anyhow::Result<usize> {
    if !(min..=max).contains(&value) {
        bail!("{name} must be between {min} and {max}");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{resolve_database_urls, validate_i32, validate_usize};

    const RUNTIME_URL: &str = "postgres://app@database/wareboxes";
    const MIGRATION_URL: &str = "postgres://admin@database/wareboxes";

    #[test]
    fn uses_local_defaults_only_when_both_database_urls_are_absent() {
        let (runtime, migration) =
            resolve_database_urls(None, None).expect("local defaults should resolve");

        assert!(runtime.contains("wareboxes_app"));
        assert!(runtime.contains("127.0.0.1:5433"));
        assert!(migration.contains("wareboxes_admin"));
        assert!(migration.contains("127.0.0.1:5433"));
    }

    #[test]
    fn accepts_explicit_database_url_pair() {
        let urls = resolve_database_urls(
            Some(RUNTIME_URL.to_string()),
            Some(MIGRATION_URL.to_string()),
        )
        .expect("paired database URLs should resolve");

        assert_eq!(urls, (RUNTIME_URL.to_string(), MIGRATION_URL.to_string()));
    }

    #[test]
    fn rejects_runtime_url_without_migration_url() {
        let error = resolve_database_urls(Some(RUNTIME_URL.to_string()), None)
            .expect_err("an incomplete database URL pair must fail");

        assert_eq!(
            error.to_string(),
            "MIGRATION_DATABASE_URL is required when DATABASE_URL is configured"
        );
    }

    #[test]
    fn rejects_migration_url_without_runtime_url() {
        let error = resolve_database_urls(None, Some(MIGRATION_URL.to_string()))
            .expect_err("an incomplete database URL pair must fail");

        assert_eq!(
            error.to_string(),
            "DATABASE_URL is required when MIGRATION_DATABASE_URL is configured"
        );
    }

    #[test]
    fn rejects_empty_database_urls() {
        for (runtime, migration, expected) in [
            (
                Some(String::new()),
                Some(MIGRATION_URL.to_string()),
                "DATABASE_URL must not be empty",
            ),
            (
                Some(RUNTIME_URL.to_string()),
                Some(" \t".to_string()),
                "MIGRATION_DATABASE_URL must not be empty",
            ),
        ] {
            let error = resolve_database_urls(runtime, migration)
                .expect_err("empty database URLs must fail");
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn integer_security_bounds_are_enforced() {
        let error = validate_i32("SECURITY_SECONDS", 299, 300, 86_400)
            .expect_err("values below the security floor must fail");
        assert!(error.to_string().contains("between 300 and 86400"));
    }

    #[test]
    fn unsigned_http_limit_bounds_are_enforced() {
        let error = validate_usize("HTTP_LIMIT", 0, 1, 100)
            .expect_err("zero must not disable a safety limit");
        assert!(error.to_string().contains("between 1 and 100"));
    }
}
