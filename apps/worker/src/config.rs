use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context};
use reqwest::Url;
use wareboxes_worker::{CarrierManifestWorkerConfig, InventoryReconciliationConfig, WorkerConfig};

const DEFAULT_DATABASE_URL: &str =
    "postgres://wareboxes_app:wareboxes_app@127.0.0.1:5433/wareboxes";

pub struct Config {
    pub database_url: String,
    pub worker_id: String,
    pub poll_interval: Duration,
    pub reconciliation_poll_interval: Duration,
    pub reconciliation: InventoryReconciliationConfig,
    pub carrier_poll_interval: Duration,
    pub carrier_worker: CarrierManifestWorkerConfig,
    pub carrier_gateway: Option<CarrierGatewayConfig>,
    pub worker: WorkerConfig,
    pub publisher: PublisherConfig,
}

pub struct CarrierGatewayConfig {
    pub endpoint: Url,
    pub bearer_token: String,
    pub signing_secret: String,
}

pub enum PublisherConfig {
    Http {
        endpoint: Url,
        bearer_token: String,
        signing_secret: String,
    },
    Sftp {
        host: String,
        port: u16,
        username: String,
        private_key_file: PathBuf,
        known_hosts_file: PathBuf,
        remote_directory: String,
    },
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
            optional_env("OUTBOX_WEBHOOK_SIGNING_SECRET")?,
            parse_bool_env("OUTBOX_ALLOW_INSECURE_HTTP", false)?,
            SftpEnvironment {
                host: optional_env("OUTBOX_SFTP_HOST")?,
                port: optional_env("OUTBOX_SFTP_PORT")?,
                username: optional_env("OUTBOX_SFTP_USERNAME")?,
                private_key_file: optional_env("OUTBOX_SFTP_PRIVATE_KEY_FILE")?,
                known_hosts_file: optional_env("OUTBOX_SFTP_KNOWN_HOSTS_FILE")?,
                remote_directory: optional_env("OUTBOX_SFTP_REMOTE_DIRECTORY")?,
            },
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
        let reconciliation = InventoryReconciliationConfig {
            interval_seconds: parse_i64_env(
                "INVENTORY_RECONCILIATION_INTERVAL_SECONDS",
                60,
                60,
                86_400,
            )?,
            tenant_page_size: parse_usize_env(
                "INVENTORY_RECONCILIATION_TENANT_PAGE_SIZE",
                100,
                1,
                10_000,
            )?,
        };
        reconciliation.validate()?;
        let carrier_gateway = carrier_gateway_config(
            optional_env("CARRIER_GATEWAY_URL")?,
            optional_env("CARRIER_GATEWAY_BEARER_TOKEN")?,
            optional_env("CARRIER_GATEWAY_SIGNING_SECRET")?,
            parse_bool_env("CARRIER_GATEWAY_ALLOW_INSECURE_HTTP", false)?,
        )?;
        let carrier_worker = CarrierManifestWorkerConfig {
            batch_size: parse_i64_env("CARRIER_BATCH_SIZE", 20, 1, 100)?,
            tenant_page_size: parse_usize_env("CARRIER_TENANT_PAGE_SIZE", 100, 1, 10_000)?,
            lease: duration_env("CARRIER_LEASE_SECONDS", 60, 2, 3_600)?,
            request_timeout: duration_env("CARRIER_REQUEST_TIMEOUT_SECONDS", 20, 1, 3_599)?,
            retry_delay: duration_env("CARRIER_RETRY_DELAY_SECONDS", 5, 0, 86_400)?,
            retry_delay_cap: duration_env("CARRIER_RETRY_DELAY_CAP_SECONDS", 300, 0, 86_400)?,
            max_attempts: u32::try_from(parse_i64_env("CARRIER_MAX_ATTEMPTS", 10, 1, 1_000)?)
                .context("CARRIER_MAX_ATTEMPTS does not fit in u32")?,
        };
        carrier_worker.validate()?;

        Ok(Self {
            database_url,
            worker_id,
            poll_interval: duration_env("OUTBOX_POLL_INTERVAL_SECONDS", 1, 1, 300)?,
            reconciliation_poll_interval: Duration::from_secs(
                u64::try_from(reconciliation.interval_seconds)
                    .context("inventory reconciliation interval cannot be negative")?,
            ),
            reconciliation,
            carrier_poll_interval: duration_env("CARRIER_POLL_INTERVAL_SECONDS", 1, 1, 300)?,
            carrier_worker,
            carrier_gateway,
            worker,
            publisher,
        })
    }
}

fn carrier_gateway_config(
    endpoint: Option<String>,
    bearer_token: Option<String>,
    signing_secret: Option<String>,
    allow_insecure_http: bool,
) -> anyhow::Result<Option<CarrierGatewayConfig>> {
    let Some(endpoint) = endpoint else {
        if bearer_token.is_some() || signing_secret.is_some() {
            bail!("CARRIER_GATEWAY_URL is required when carrier gateway credentials are set");
        }
        return Ok(None);
    };
    let endpoint = Url::parse(&endpoint).context("CARRIER_GATEWAY_URL must be a valid URL")?;
    if endpoint.scheme() != "https" && !(allow_insecure_http && endpoint.scheme() == "http") {
        bail!("CARRIER_GATEWAY_URL must use HTTPS unless insecure HTTP is explicitly enabled");
    }
    let bearer_token = required_value("CARRIER_GATEWAY_BEARER_TOKEN", bearer_token)?;
    let signing_secret = required_value("CARRIER_GATEWAY_SIGNING_SECRET", signing_secret)?;
    if signing_secret.len() < 32 {
        bail!("CARRIER_GATEWAY_SIGNING_SECRET must contain at least 32 bytes");
    }
    Ok(Some(CarrierGatewayConfig {
        endpoint,
        bearer_token,
        signing_secret,
    }))
}

fn publisher_config(
    publisher: Option<String>,
    endpoint: Option<String>,
    bearer_token: Option<String>,
    signing_secret: Option<String>,
    allow_insecure_http: bool,
    sftp: SftpEnvironment,
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
            let signing_secret = required_value("OUTBOX_WEBHOOK_SIGNING_SECRET", signing_secret)?;
            if signing_secret.len() < 32 {
                bail!("OUTBOX_WEBHOOK_SIGNING_SECRET must contain at least 32 bytes");
            }
            Ok(PublisherConfig::Http {
                endpoint,
                bearer_token,
                signing_secret,
            })
        }
        Some("sftp") => {
            let host = required_value("OUTBOX_SFTP_HOST", sftp.host)?;
            let username = required_value("OUTBOX_SFTP_USERNAME", sftp.username)?;
            let private_key_file = PathBuf::from(required_value(
                "OUTBOX_SFTP_PRIVATE_KEY_FILE",
                sftp.private_key_file,
            )?);
            let known_hosts_file = PathBuf::from(required_value(
                "OUTBOX_SFTP_KNOWN_HOSTS_FILE",
                sftp.known_hosts_file,
            )?);
            let remote_directory =
                required_value("OUTBOX_SFTP_REMOTE_DIRECTORY", sftp.remote_directory)?;
            validate_remote_directory(&remote_directory)?;
            let port = sftp
                .port
                .map(|value| {
                    value
                        .parse::<u16>()
                        .context("OUTBOX_SFTP_PORT must be a TCP port")
                })
                .transpose()?
                .unwrap_or(22);
            if port == 0 {
                bail!("OUTBOX_SFTP_PORT must be positive");
            }
            Ok(PublisherConfig::Sftp {
                host,
                port,
                username,
                private_key_file,
                known_hosts_file,
                remote_directory,
            })
        }
        Some("stdout") => Ok(PublisherConfig::Stdout),
        None | Some("") => bail!("OUTBOX_PUBLISHER must be set to http, sftp, or stdout"),
        Some(value) => bail!("unsupported OUTBOX_PUBLISHER: {value}"),
    }
}

#[derive(Default)]
struct SftpEnvironment {
    host: Option<String>,
    port: Option<String>,
    username: Option<String>,
    private_key_file: Option<String>,
    known_hosts_file: Option<String>,
    remote_directory: Option<String>,
}

fn validate_remote_directory(value: &str) -> anyhow::Result<()> {
    if !value.starts_with('/')
        || value.ends_with('/')
        || value.split('/').any(|segment| segment == "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
    {
        bail!(
            "OUTBOX_SFTP_REMOTE_DIRECTORY must be an absolute safe path without a trailing slash"
        );
    }
    Ok(())
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
        let error = publisher_config(None, None, None, None, false, SftpEnvironment::default())
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
            Some("x".repeat(32)),
            false,
            SftpEnvironment::default(),
        )
        .is_err());
        assert!(publisher_config(
            Some("http".into()),
            Some("https://example.com/events".into()),
            None,
            Some("x".repeat(32)),
            false,
            SftpEnvironment::default(),
        )
        .is_err());
        assert!(publisher_config(
            Some("http".into()),
            Some("https://example.com/events".into()),
            Some("token".into()),
            Some("short".into()),
            false,
            SftpEnvironment::default(),
        )
        .is_err());
    }

    #[test]
    fn accepts_explicit_development_stdout_publisher() {
        assert!(matches!(
            publisher_config(
                Some("stdout".into()),
                None,
                None,
                None,
                false,
                SftpEnvironment::default(),
            )
            .unwrap(),
            PublisherConfig::Stdout
        ));
    }

    #[test]
    fn sftp_requires_strict_host_identity_and_safe_remote_path() {
        let environment = |remote_directory: &str| SftpEnvironment {
            host: Some("sftp.example.test".into()),
            username: Some("warehouse".into()),
            private_key_file: Some("/run/secrets/sftp-key".into()),
            known_hosts_file: Some("/run/secrets/known-hosts".into()),
            remote_directory: Some(remote_directory.into()),
            ..SftpEnvironment::default()
        };
        assert!(matches!(
            publisher_config(
                Some("sftp".into()),
                None,
                None,
                None,
                false,
                environment("/exchange/outbound"),
            )
            .unwrap(),
            PublisherConfig::Sftp { port: 22, .. }
        ));
        assert!(publisher_config(
            Some("sftp".into()),
            None,
            None,
            None,
            false,
            environment("../../unsafe"),
        )
        .is_err());
    }

    #[test]
    fn carrier_gateway_is_optional_but_complete_when_enabled() {
        assert!(carrier_gateway_config(None, None, None, false)
            .unwrap()
            .is_none());
        assert!(carrier_gateway_config(
            Some("http://gateway.test/manifests".into()),
            Some("token".into()),
            Some("x".repeat(32)),
            false,
        )
        .is_err());
        assert!(carrier_gateway_config(
            Some("https://gateway.test/manifests".into()),
            Some("token".into()),
            Some("x".repeat(32)),
            false,
        )
        .unwrap()
        .is_some());
    }
}
