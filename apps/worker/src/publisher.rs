use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, RETRY_AFTER};
use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use wareboxes_application::outbox::OutboxEvent;
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, Timestamp};
use wareboxes_worker::{PublishError, Publisher};

pub enum ConfiguredPublisher {
    Http(HttpPublisher),
    Stdout(StdoutPublisher),
}

impl ConfiguredPublisher {
    pub fn http(endpoint: Url, bearer_token: String) -> anyhow::Result<Self> {
        Ok(Self::Http(HttpPublisher {
            client: Client::builder().build()?,
            endpoint,
            authorization: format!("Bearer {bearer_token}"),
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
            Self::Stdout(publisher) => publisher.name(),
        }
    }

    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError> {
        match self {
            Self::Http(publisher) => publisher.publish(event).await,
            Self::Stdout(publisher) => publisher.publish(event).await,
        }
    }
}

pub struct HttpPublisher {
    client: Client,
    endpoint: Url,
    authorization: String,
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
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, &self.authorization)
            .header("idempotency-key", &event.event_key)
            .header("x-wareboxes-event-type", &event.event_type)
            .header("x-wareboxes-tenant-id", event.tenant_id.to_string())
            .json(&PublishedEvent::from(event))
            .send()
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
}
