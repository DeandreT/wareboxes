use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use wareboxes_application::outbox::OutboxEvent;
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, Timestamp};
use wareboxes_worker::{PublishError, Publisher};

type HmacSha256 = Hmac<Sha256>;

const WEBHOOK_SIGNATURE_VERSION: &str = "v1";

pub enum ConfiguredPublisher {
    Http(HttpPublisher),
    Sftp(SftpPublisher),
    Stdout(StdoutPublisher),
}

impl ConfiguredPublisher {
    pub fn http(
        endpoint: Url,
        bearer_token: String,
        signing_secret: String,
    ) -> anyhow::Result<Self> {
        Ok(Self::Http(HttpPublisher {
            client: Client::builder().build()?,
            endpoint,
            authorization: format!("Bearer {bearer_token}"),
            signing_secret: signing_secret.into_bytes(),
        }))
    }

    pub fn sftp(
        host: String,
        port: u16,
        username: String,
        private_key_file: PathBuf,
        known_hosts_file: PathBuf,
        remote_directory: String,
    ) -> anyhow::Result<Self> {
        validate_regular_file("OUTBOX_SFTP_PRIVATE_KEY_FILE", &private_key_file)?;
        validate_regular_file("OUTBOX_SFTP_KNOWN_HOSTS_FILE", &known_hosts_file)?;
        Ok(Self::Sftp(SftpPublisher {
            command: PathBuf::from("sftp"),
            host,
            port,
            username,
            private_key_file,
            known_hosts_file,
            remote_directory,
        }))
    }

    pub fn stdout() -> Self {
        Self::Stdout(StdoutPublisher)
    }
}

#[async_trait]
impl Publisher for ConfiguredPublisher {
    fn name(&self) -> &'static str {
        match self {
            Self::Http(publisher) => publisher.name(),
            Self::Sftp(publisher) => publisher.name(),
            Self::Stdout(publisher) => publisher.name(),
        }
    }

    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError> {
        match self {
            Self::Http(publisher) => publisher.publish(event).await,
            Self::Sftp(publisher) => publisher.publish(event).await,
            Self::Stdout(publisher) => publisher.publish(event).await,
        }
    }
}

pub struct HttpPublisher {
    client: Client,
    endpoint: Url,
    authorization: String,
    signing_secret: Vec<u8>,
}

#[derive(Serialize)]
struct PublishedEvent<'a> {
    event_key: &'a str,
    tenant_id: TenantId,
    inventory_owner_id: Option<InventoryOwnerId>,
    facility_id: Option<FacilityId>,
    actor_user_id: Option<i64>,
    aggregate_type: &'a str,
    aggregate_id: &'a str,
    ordering_key: &'a str,
    aggregate_sequence: i64,
    event_type: &'a str,
    schema_version: i32,
    payload: &'a serde_json::Value,
    occurred_at: Timestamp,
}

impl<'a> From<&'a OutboxEvent> for PublishedEvent<'a> {
    fn from(event: &'a OutboxEvent) -> Self {
        Self {
            event_key: &event.event_key,
            tenant_id: event.tenant_id,
            inventory_owner_id: event.inventory_owner_id,
            facility_id: event.facility_id,
            actor_user_id: event.actor_user_id,
            aggregate_type: &event.aggregate_type,
            aggregate_id: &event.aggregate_id,
            ordering_key: &event.ordering_key,
            aggregate_sequence: event.aggregate_sequence,
            event_type: &event.event_type,
            schema_version: event.schema_version,
            payload: &event.payload,
            occurred_at: event.occurred_at,
        }
    }
}

#[async_trait]
impl Publisher for HttpPublisher {
    fn name(&self) -> &'static str {
        "http"
    }

    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError> {
        let request = self.signed_request(event, Utc::now().timestamp())?;
        let response = self
            .client
            .execute(request)
            .await
            .map_err(|error| PublishError::retryable("http_transport", error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs);
        let response_body = response.text().await.unwrap_or_default();
        let message = bounded_response_message(status, &response_body);
        if retryable_status(status) {
            let error = PublishError::retryable("http_status", message);
            return Err(match retry_after {
                Some(retry_after) => error.with_retry_after(retry_after),
                None => error,
            });
        }
        Err(PublishError::permanent("http_status", message))
    }
}

impl HttpPublisher {
    fn signed_request(
        &self,
        event: &OutboxEvent,
        timestamp: i64,
    ) -> Result<reqwest::Request, PublishError> {
        let body = serde_json::to_vec(&PublishedEvent::from(event))
            .map_err(|error| PublishError::permanent("serialize_event", error.to_string()))?;
        let signature = webhook_signature(&self.signing_secret, timestamp, &body)
            .map_err(|error| PublishError::permanent("sign_event", error.to_string()))?;
        self.client
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, &self.authorization)
            .header(CONTENT_TYPE, "application/json")
            .header("idempotency-key", &event.event_key)
            .header("x-wareboxes-webhook-id", &event.event_key)
            .header("x-wareboxes-webhook-timestamp", timestamp.to_string())
            .header(
                "x-wareboxes-webhook-signature",
                format!("{WEBHOOK_SIGNATURE_VERSION}={signature}"),
            )
            .header("x-wareboxes-event-type", &event.event_type)
            .header("x-wareboxes-tenant-id", event.tenant_id.to_string())
            .body(body)
            .build()
            .map_err(|error| PublishError::permanent("build_http_request", error.to_string()))
    }
}

fn webhook_signature(secret: &[u8], timestamp: i64, body: &[u8]) -> anyhow::Result<String> {
    let mut signer = HmacSha256::new_from_slice(secret)
        .map_err(|error| anyhow::anyhow!("initializing HMAC signer: {error}"))?;
    signer.update(timestamp.to_string().as_bytes());
    signer.update(b".");
    signer.update(body);
    Ok(hex::encode(signer.finalize().into_bytes()))
}

pub struct SftpPublisher {
    command: PathBuf,
    host: String,
    port: u16,
    username: String,
    private_key_file: PathBuf,
    known_hosts_file: PathBuf,
    remote_directory: String,
}

#[async_trait]
impl Publisher for SftpPublisher {
    fn name(&self) -> &'static str {
        "sftp"
    }

    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError> {
        let body = serde_json::to_vec(&PublishedEvent::from(event))
            .map_err(|error| PublishError::permanent("serialize_event", error.to_string()))?;
        let event_digest = hex::encode(Sha256::digest(event.event_key.as_bytes()));
        let final_path = format!("{}/{event_digest}.json", self.remote_directory);
        let staging_path = format!("{}/.{event_digest}.json.upload", self.remote_directory);
        let mut local_file = tempfile::Builder::new()
            .prefix("wareboxes-sftp-")
            .suffix(".json")
            .tempfile()
            .map_err(|error| PublishError::retryable("sftp_local_file", error.to_string()))?;
        local_file
            .write_all(&body)
            .and_then(|()| local_file.flush())
            .map_err(|error| PublishError::retryable("sftp_local_file", error.to_string()))?;
        let local_path = quote_batch_path(local_file.path())
            .map_err(|error| PublishError::permanent("sftp_local_path", error.to_string()))?;
        let batch = sftp_batch(&local_path, &staging_path, &final_path);

        let destination = format!("{}@{}", self.username, self.host);
        let mut child = Command::new(&self.command)
            .arg("-q")
            .arg("-b")
            .arg("-")
            .arg("-P")
            .arg(self.port.to_string())
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("StrictHostKeyChecking=yes")
            .arg("-o")
            .arg(format!(
                "UserKnownHostsFile={}",
                self.known_hosts_file.display()
            ))
            .arg("-o")
            .arg(format!("IdentityFile={}", self.private_key_file.display()))
            .arg("--")
            .arg(destination)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| PublishError::retryable("sftp_spawn", error.to_string()))?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            PublishError::retryable("sftp_stdin", "SFTP child stdin was unavailable")
        })?;
        stdin
            .write_all(batch.as_bytes())
            .await
            .map_err(|error| PublishError::retryable("sftp_stdin", error.to_string()))?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .await
            .map_err(|error| PublishError::retryable("sftp_wait", error.to_string()))?;
        if output.status.success() {
            return Ok(());
        }

        let diagnostic = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(500)
            .collect::<String>();
        let message = if diagnostic.is_empty() {
            format!("sftp exited with {}", output.status)
        } else {
            format!("sftp exited with {}: {diagnostic}", output.status)
        };
        Err(PublishError::retryable("sftp_exit", message))
    }
}

fn validate_regular_file(name: &str, path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| anyhow::anyhow!("{name} {} is unavailable: {error}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{name} {} must be a regular file", path.display());
    }
    Ok(())
}

fn quote_batch_path(path: &Path) -> anyhow::Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("temporary path is not valid UTF-8"))?;
    if value.chars().any(char::is_control) {
        anyhow::bail!("temporary path contains a control character");
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn sftp_batch(local_path: &str, staging_path: &str, final_path: &str) -> String {
    format!(
        "put {local_path} {staging_path}\n-rm {final_path}\nrename {staging_path} {final_path}\n"
    )
}

fn retryable_status(status: StatusCode) -> bool {
    status.is_server_error()
        || matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY | StatusCode::TOO_MANY_REQUESTS
        )
}

fn bounded_response_message(status: StatusCode, body: &str) -> String {
    let body = body.trim().chars().take(500).collect::<String>();
    if body.is_empty() {
        format!("publisher returned HTTP {status}")
    } else {
        format!("publisher returned HTTP {status}: {body}")
    }
}

pub struct StdoutPublisher;

#[async_trait]
impl Publisher for StdoutPublisher {
    fn name(&self) -> &'static str {
        "stdout"
    }

    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError> {
        let event = serde_json::to_string(&PublishedEvent::from(event))
            .map_err(|error| PublishError::permanent("serialize_event", error.to_string()))?;
        println!("{event}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn status_classification_only_retries_transient_responses() {
        assert!(retryable_status(StatusCode::REQUEST_TIMEOUT));
        assert!(retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(retryable_status(StatusCode::BAD_GATEWAY));
        assert!(!retryable_status(StatusCode::BAD_REQUEST));
        assert!(!retryable_status(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn response_diagnostics_are_bounded() {
        let diagnostic = bounded_response_message(StatusCode::BAD_REQUEST, &"x".repeat(1_000));
        assert_eq!(
            diagnostic
                .chars()
                .filter(|character| *character == 'x')
                .count(),
            500
        );
    }

    #[test]
    fn webhook_signature_binds_timestamp_and_exact_body() {
        let secret = b"0123456789abcdef0123456789abcdef";
        let signature = webhook_signature(secret, 1_700_000_000, br#"{\"event\":1}"#).unwrap();
        assert_eq!(signature.len(), 64);
        assert_ne!(
            signature,
            webhook_signature(secret, 1_700_000_001, br#"{\"event\":1}"#).unwrap()
        );
        assert_ne!(
            signature,
            webhook_signature(secret, 1_700_000_000, br#"{\"event\":2}"#).unwrap()
        );
    }

    #[test]
    fn webhook_request_signs_the_exact_transmitted_body_and_identity() {
        let secret = b"0123456789abcdef0123456789abcdef".to_vec();
        let publisher = HttpPublisher {
            client: Client::new(),
            endpoint: Url::parse("https://partner.example.test/events").unwrap(),
            authorization: "Bearer partner-token".into(),
            signing_secret: secret.clone(),
        };
        let request = publisher.signed_request(&event(), 1_700_000_000).unwrap();
        let body = request.body().and_then(reqwest::Body::as_bytes).unwrap();
        let signature = webhook_signature(&secret, 1_700_000_000, body).unwrap();
        assert_eq!(
            request.headers()["x-wareboxes-webhook-signature"],
            format!("v1={signature}")
        );
        assert_eq!(request.headers()["x-wareboxes-webhook-id"], "event-key-1");
        assert_eq!(request.headers()["idempotency-key"], "event-key-1");
        assert_eq!(request.headers()["authorization"], "Bearer partner-token");
        let envelope: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(envelope["event_key"], "event-key-1");
        assert_eq!(envelope["payload"]["shipment_id"], 51);
    }

    #[test]
    fn sftp_batch_publishes_from_a_staging_name() {
        assert_eq!(
            sftp_batch(
                "\"/tmp/event.json\"",
                "/exchange/.digest.json.upload",
                "/exchange/digest.json",
            ),
            concat!(
                "put \"/tmp/event.json\" /exchange/.digest.json.upload\n",
                "-rm /exchange/digest.json\n",
                "rename /exchange/.digest.json.upload /exchange/digest.json\n",
            )
        );
    }

    fn event() -> OutboxEvent {
        let now = Utc::now();
        OutboxEvent {
            id: 1,
            tenant_id: TenantId::new(7).unwrap(),
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: Some(9),
            created: now,
            event_key: "event-key-1".into(),
            aggregate_type: "shipment".into(),
            aggregate_id: "51".into(),
            ordering_key: "shipment:51".into(),
            aggregate_sequence: 2,
            event_type: "shipment.departed".into(),
            schema_version: 1,
            payload: json!({"shipment_id": 51}),
            occurred_at: now,
            available_at: now,
            claimed_at: Some(now),
            claimed_by: Some("worker-test".into()),
            lease_expires_at: Some(now),
            claim_version: 1,
            attempts: 1,
            last_error: None,
            dead_lettered_at: None,
            replay_count: 0,
            discarded_at: None,
            discard_reason: None,
            discarded_by_user_id: None,
            published_at: None,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sftp_invocation_pins_host_and_atomically_promotes_the_event() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let command = directory.path().join("fake-sftp");
        let batch_capture = directory.path().join("batch");
        let args_capture = directory.path().join("args");
        std::fs::write(
            &command,
            format!(
                "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' \"$@\" > '{}'\n",
                batch_capture.display(),
                args_capture.display(),
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&command, permissions).unwrap();
        let key = directory.path().join("key");
        let known_hosts = directory.path().join("known-hosts");
        std::fs::write(&key, "test-key").unwrap();
        std::fs::write(&known_hosts, "sftp.example.test test-host-key").unwrap();
        let publisher = SftpPublisher {
            command,
            host: "sftp.example.test".into(),
            port: 2222,
            username: "warehouse".into(),
            private_key_file: key.clone(),
            known_hosts_file: known_hosts.clone(),
            remote_directory: "/exchange/outbound".into(),
        };

        publisher.publish(&event()).await.unwrap();

        let args = std::fs::read_to_string(args_capture).unwrap();
        assert!(args.contains("StrictHostKeyChecking=yes"));
        assert!(args.contains(&format!("UserKnownHostsFile={}", known_hosts.display())));
        assert!(args.contains(&format!("IdentityFile={}", key.display())));
        assert!(args.contains("warehouse@sftp.example.test"));
        let digest = hex::encode(Sha256::digest(b"event-key-1"));
        let batch = std::fs::read_to_string(batch_capture).unwrap();
        assert!(batch.contains(&format!(
            "put \"{}",
            directory
                .path()
                .parent()
                .unwrap_or(directory.path())
                .display()
        )));
        assert!(batch.contains(&format!("/exchange/outbound/.{digest}.json.upload")));
        assert!(batch.contains(&format!("-rm /exchange/outbound/{digest}.json")));
        assert!(batch.contains(&format!(
            "rename /exchange/outbound/.{digest}.json.upload /exchange/outbound/{digest}.json"
        )));
    }
}
