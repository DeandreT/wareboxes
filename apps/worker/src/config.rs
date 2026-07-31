use std::env;
use std::time::Duration;

use anyhow::{bail, Context};
use reqwest::Url;
use wareboxes_worker::WorkerConfig;

const DEFAULT_DATABASE_URL: &str =
    "postgres://wareboxes_app:wareboxes_app@127.0.0.1:5433/wareboxes";

pub struct Config {
    pub database_url: String,
    pub worker_id: String,
    pub poll_interval: Duration,
    pub worker: WorkerConfig,
    pub publisher: PublisherConfig,
}

pub enum PublisherConfig {
    Http { endpoint: Url, bearer_token: String },
    Stdout,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url =
            optional_env("DATABASE_URL")?.unwrap_or_else(|| DEFAULT_DATABASE_URL.to_owned());
        if database_url.trim().is_empty() {
            bail!("DATABASE_URL must not be empty");
        }

        let worker_id =
            optional_env("WORKER_ID")?.unwrap_or_else(|| format!("outbox-{}", std::process::id()));
        let publisher = publisher_config(
            optional_env("OUTBOX_PUBLISHER")?,
            optional_env("OUTBOX_PUBLISH_URL")?,
            optional_env("OUTBOX_PUBLISH_BEARER_TOKEN")?,
            parse_bool_env("OUTBOX_ALLOW_INSECURE_HTTP", false)?,
        )?;
        let worker = WorkerConfig {
            batch_size: parse_i64_env("OUTBOX_BATCH_SIZE", 100, 1, 1_000)?,
            max_in_flight: parse_usize_env("OUTBOX_MAX_IN_FLIGHT", 32, 1, 1_000)?,
            tenant_page_size: parse_usize_env("OUTBOX_TENANT_PAGE_SIZE", 1_000, 1, 10_000)?,
            lease: duration_env("OUTBOX_LEASE_SECONDS", 60, 1, 3_600)?,
            publish_timeout: duration_env("OUTBOX_PUBLISH_TIMEOUT_SECONDS", 20, 1, 3_599)?,
            retry_delay: duration_env("OUTBOX_RETRY_DELAY_SECONDS", 5, 0, 86_400)?,
            retry_delay_cap: duration_env("OUTBOX_RETRY_DELAY_CAP_SECONDS", 300, 0, 86_400)?,
            max_attempts: parse_i32_env("OUTBOX_MAX_ATTEMPTS", 10, 1, 1_000)?,
        };
        worker.validate()?;

        Ok(Self {
            database_url,
            worker_id,
            poll_interval: duration_env("OUTBOX_POLL_INTERVAL_SECONDS", 1, 1, 300)?,
            worker,
            publisher,
        })
    }
}

fn publisher_config(
    publisher: Option<String>,
    endpoint: Option<String>,
    bearer_token: Option<String>,
    allow_insecure_http: bool,
) -> anyhow::Result<PublisherConfig> {
    match publisher.as_deref().map(str::trim) {
        Some("http") => {
            let endpoint = required_value("OUTBOX_PUBLISH_URL", endpoint)?;
            let endpoint =
                Url::parse(&endpoint).context("OUTBOX_PUBLISH_URL must be a valid URL")?;
            if endpoint.scheme() != "https" && !(allow_insecure_http && endpoint.scheme() == "http")
            {
                bail!(
                    "OUTBOX_PUBLISH_URL must use HTTPS unless OUTBOX_ALLOW_INSECURE_HTTP is true"
                );
            }
            let bearer_token = required_value("OUTBOX_PUBLISH_BEARER_TOKEN", bearer_token)?;
            Ok(PublisherConfig::Http {
                endpoint,
                bearer_token,
            })
        }
        Some("stdout") => Ok(PublisherConfig::Stdout),
        None | Some("") => bail!("OUTBOX_PUBLISHER must be set to http or stdout"),
        Some(value) => bail!("unsupported OUTBOX_PUBLISHER: {value}"),
    }
}

fn required_value(name: &str, value: Option<String>) -> anyhow::Result<String> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => bail!("{name} is required and must not be empty"),
    }
}

fn optional_env(name: &str) -> anyhow::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => bail!("{name} must contain valid UTF-8"),
    }
}

fn parse_bool_env(name: &str, default: bool) -> anyhow::Result<bool> {
    let Some(value) = optional_env(name)? else {
        return Ok(default);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => bail!("{name} must be true or false"),
    }
}

fn parse_i64_env(name: &str, default: i64, min: i64, max: i64) -> anyhow::Result<i64> {
    let value = optional_env(name)?
        .map(|value| {
            value
                .parse::<i64>()
                .with_context(|| format!("{name} must be an integer"))
        })
        .transpose()?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        bail!("{name} must be between {min} and {max}");
    }
    Ok(value)
}

fn parse_i32_env(name: &str, default: i32, min: i32, max: i32) -> anyhow::Result<i32> {
    let value = parse_i64_env(name, i64::from(default), i64::from(min), i64::from(max))?;
    i32::try_from(value).with_context(|| format!("{name} does not fit in i32"))
}

fn parse_usize_env(name: &str, default: usize, min: usize, max: usize) -> anyhow::Result<usize> {
    let value = parse_i64_env(
        name,
        i64::try_from(default).context("default does not fit in i64")?,
        i64::try_from(min).context("minimum does not fit in i64")?,
        i64::try_from(max).context("maximum does not fit in i64")?,
    )?;
    usize::try_from(value).with_context(|| format!("{name} does not fit in usize"))
}

fn duration_env(
    name: &str,
    default_seconds: i64,
    min_seconds: i64,
    max_seconds: i64,
) -> anyhow::Result<Duration> {
    let seconds = parse_i64_env(name, default_seconds, min_seconds, max_seconds)?;
    Ok(Duration::from_secs(
        u64::try_from(seconds).with_context(|| format!("{name} cannot be negative"))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_an_explicit_publisher() {
        let error = publisher_config(None, None, None, false)
            .err()
            .expect("missing publisher must fail");
        assert!(error.to_string().contains("OUTBOX_PUBLISHER"));
    }

    #[test]
    fn http_publisher_requires_https_and_credentials() {
        assert!(publisher_config(
            Some("http".into()),
            Some("http://example.com/events".into()),
            Some("token".into()),
            false,
        )
        .is_err());
        assert!(publisher_config(
            Some("http".into()),
            Some("https://example.com/events".into()),
            None,
            false,
        )
        .is_err());
    }

    #[test]
    fn accepts_explicit_development_stdout_publisher() {
        assert!(matches!(
            publisher_config(Some("stdout".into()), None, None, false).unwrap(),
            PublisherConfig::Stdout
        ));
    }
}
