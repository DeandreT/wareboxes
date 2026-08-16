use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use wareboxes_api_contract::v1::{
    AcknowledgeAutomationCommandRequest, AutomationCommandDeliveryPage, AutomationCommandResponse,
    AutomationDeviceResponse, AutomationEdgeDevicePage, AutomationHeartbeatResponse,
    PullAutomationCommandsRequest, RecordAutomationHeartbeatRequest,
    ReportAutomationCommandRequest,
};

use super::CloudTransport;

const TENANT_HEADER: &str = "X-Tenant-Id";
const IDEMPOTENCY_HEADER: &str = "Idempotency-Key";
const MAX_ERROR_BODY: usize = 2_000;

pub struct CloudClientConfig {
    pub base_url: String,
    pub tenant_id: i64,
    pub bearer_token: String,
    pub timeout: Duration,
}

#[derive(Debug, Error)]
pub enum CloudTransportError {
    #[error("invalid cloud client configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("cloud transport failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("cloud returned HTTP {status}: {message}")]
    HttpStatus { status: StatusCode, message: String },
    #[error("cloud URL cannot be joined with {0}")]
    InvalidPath(String),
}

pub struct CloudClient {
    client: Client,
    base_url: Url,
    tenant_id: i64,
    authorization: String,
}

impl CloudClient {
    pub fn new(config: CloudClientConfig) -> Result<Self, CloudTransportError> {
        if config.tenant_id <= 0 {
            return Err(CloudTransportError::InvalidConfig(
                "tenant ID must be positive",
            ));
        }
        if config.bearer_token.trim() != config.bearer_token
            || config.bearer_token.is_empty()
            || config.bearer_token.chars().any(char::is_whitespace)
        {
            return Err(CloudTransportError::InvalidConfig(
                "bearer token must be non-empty and contain no whitespace",
            ));
        }
        if config.timeout.is_zero() {
            return Err(CloudTransportError::InvalidConfig(
                "HTTP timeout must be positive",
            ));
        }
        let base_url = Url::parse(&config.base_url)
            .map_err(|_| CloudTransportError::InvalidConfig("base URL is invalid"))?;
        let local_development = matches!(base_url.host_str(), Some("localhost" | "127.0.0.1"));
        if base_url.scheme() != "https" && !(base_url.scheme() == "http" && local_development) {
            return Err(CloudTransportError::InvalidConfig(
                "base URL must use HTTPS outside local development",
            ));
        }
        if !base_url.username().is_empty() || base_url.password().is_some() {
            return Err(CloudTransportError::InvalidConfig(
                "base URL must not contain credentials",
            ));
        }
        if base_url.path() != "/" || base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(CloudTransportError::InvalidConfig(
                "base URL must be an origin without a path, query, or fragment",
            ));
        }
        let client = Client::builder().timeout(config.timeout).build()?;
        Ok(Self {
            client,
            base_url,
            tenant_id: config.tenant_id,
            authorization: format!("Bearer {}", config.bearer_token),
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, CloudTransportError> {
        self.base_url
            .join(path)
            .map_err(|_| CloudTransportError::InvalidPath(path.to_owned()))
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        request
            .header(AUTHORIZATION, &self.authorization)
            .header(TENANT_HEADER, self.tenant_id)
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, CloudTransportError> {
        let request = self.authorize(self.client.get(self.endpoint(path)?));
        decode(request.send()?)
    }

    fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        idempotency_key: &str,
    ) -> Result<T, CloudTransportError> {
        if idempotency_key.trim().is_empty() || idempotency_key.len() > 200 {
            return Err(CloudTransportError::InvalidConfig(
                "idempotency key must contain between 1 and 200 characters",
            ));
        }
        let request = self
            .authorize(self.client.post(self.endpoint(path)?))
            .header(CONTENT_TYPE, "application/json")
            .header(IDEMPOTENCY_HEADER, idempotency_key)
            .json(body);
        decode(request.send()?)
    }
}

impl CloudTransport for CloudClient {
    fn assigned_devices(
        &mut self,
        facility_id: i64,
    ) -> Result<Vec<AutomationDeviceResponse>, CloudTransportError> {
        let page: AutomationEdgeDevicePage = self.get(&format!(
            "api/v1/edge/automation/devices?facility_id={facility_id}"
        ))?;
        Ok(page.items)
    }

    fn pull_commands(
        &mut self,
        request: &PullAutomationCommandsRequest,
        idempotency_key: &str,
    ) -> Result<AutomationCommandDeliveryPage, CloudTransportError> {
        self.post(
            "api/v1/edge/automation/command-pulls",
            request,
            idempotency_key,
        )
    }

    fn acknowledge_command(
        &mut self,
        command_id: i64,
        request: &AcknowledgeAutomationCommandRequest,
        idempotency_key: &str,
    ) -> Result<AutomationCommandResponse, CloudTransportError> {
        self.post(
            &format!("api/v1/edge/automation/commands/{command_id}/acknowledgements"),
            request,
            idempotency_key,
        )
    }

    fn report_command(
        &mut self,
        command_id: i64,
        request: &ReportAutomationCommandRequest,
        idempotency_key: &str,
    ) -> Result<AutomationCommandResponse, CloudTransportError> {
        self.post(
            &format!("api/v1/edge/automation/commands/{command_id}/reports"),
            request,
            idempotency_key,
        )
    }

    fn record_heartbeat(
        &mut self,
        device_id: i64,
        request: &RecordAutomationHeartbeatRequest,
        idempotency_key: &str,
    ) -> Result<AutomationHeartbeatResponse, CloudTransportError> {
        self.post(
            &format!("api/v1/edge/automation/devices/{device_id}/heartbeats"),
            request,
            idempotency_key,
        )
    }
}

fn decode<T: DeserializeOwned>(
    response: reqwest::blocking::Response,
) -> Result<T, CloudTransportError> {
    let status = response.status();
    if status.is_success() {
        return response.json().map_err(CloudTransportError::from);
    }
    let mut message = response
        .text()
        .unwrap_or_else(|_| "cloud error body was unreadable".into());
    message.truncate(MAX_ERROR_BODY);
    Err(CloudTransportError::HttpStatus { status, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_client_requires_tls_outside_local_development() {
        assert!(CloudClient::new(CloudClientConfig {
            base_url: "http://warehouse.example/api/".into(),
            tenant_id: 1,
            bearer_token: "wbs_sa_secret".into(),
            timeout: Duration::from_secs(1),
        })
        .is_err());
        assert!(CloudClient::new(CloudClientConfig {
            base_url: "http://127.0.0.1:8080/".into(),
            tenant_id: 1,
            bearer_token: "wbs_sa_secret".into(),
            timeout: Duration::from_secs(1),
        })
        .is_ok());
        assert!(CloudClient::new(CloudClientConfig {
            base_url: "https://warehouse.example/tenant-path".into(),
            tenant_id: 1,
            bearer_token: "wbs_sa_secret".into(),
            timeout: Duration::from_secs(1),
        })
        .is_err());
    }
}
